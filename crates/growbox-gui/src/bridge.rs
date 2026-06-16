//! LLM 桥接 —— 把真 LlmClient 接到 memory 的 `Subconscious` 与 learn 的 `Reasoner`。
//!
//! memory/learn 各自定义了"潜意识"接口(便于不打真 API 单测);app 在此用真 LLM 实现它们。
//! - embed:第一层 RAG 走独立的 `Embedder` 槽位(本地词法默认 / 远程 OpenAI 兼容可选,
//!   见 `embedding-service.md`);DeepSeek 无嵌入端点,故与聊天 provider 解耦。
//! - judge_relevant / distill:用真 chat 模型推理。
//!
//! 另定义 `LlmDriver`(chat_stream 的最小 trait)使 Agent 循环可用 mock 驱动单测。

use async_trait::async_trait;
use growbox_core::Conclusion;
use growbox_learn::{Distillation, ProposedSkill, Reasoner};
use growbox_llm::{ChatMessage, ChatRequest, EmbedKind, Embedder, LlmClient, LlmResult, StreamChunk};
use growbox_memory::Subconscious;
use std::sync::Arc;
use tokio::sync::mpsc;

/// chat_stream 的最小抽象 —— 真 client 转发,测试用脚本化 mock。
#[async_trait]
pub trait LlmDriver: Send + Sync {
    async fn chat_stream(&self, req: ChatRequest) -> LlmResult<mpsc::Receiver<LlmResult<StreamChunk>>>;
}

#[async_trait]
impl LlmDriver for LlmClient {
    async fn chat_stream(&self, req: ChatRequest) -> LlmResult<mpsc::Receiver<LlmResult<StreamChunk>>> {
        LlmClient::chat_stream(self, req).await
    }
}

/// 把一次对话流式收完,拼出正文(忽略 reasoning/工具)。judge/distill 这类一次性推理用。
/// `silence_secs` = 沉默超时(任何 chunk 含 reasoning 都算活动;真沉默超过即收手)。没有它,
/// `rx.recv()` 会在流卡住(连接挂着不关、模型不吐)时永久阻塞——"收口后光标一直转 + 状态锁被占住"
/// 的根因(distill/judge_relevant 都走 complete)。推论9 可设,默认 60(见 `LlmBridge.silence_secs`)。
pub async fn complete(driver: &dyn LlmDriver, req: ChatRequest, silence_secs: u64) -> LlmResult<String> {
    let mut rx = driver.chat_stream(req).await?;
    let mut content = String::new();
    loop {
        // 沉默超时:绝不无界等待。chunk(含 reasoning)算活动;真沉默就收手返回已累积内容
        //(通常为空)→ 调用方 best-effort 降级(distill 跳过 / judge 判无),不卡死本回合。
        match tokio::time::timeout(std::time::Duration::from_secs(silence_secs), rx.recv()).await {
            Ok(Some(chunk)) => {
                if let StreamChunk::Content(c) = chunk? {
                    content.push_str(&c);
                }
            }
            Ok(None) => break, // 流正常结束
            Err(_) => {
                eprintln!(
                    "[complete] LLM 响应沉默超过 {silence_secs}s,收手(已累积 {} 字节)",
                    content.len()
                );
                break;
            }
        }
    }
    Ok(content)
}

/// 从可能含杂质(markdown 围栏/前后说明)的 LLM 输出里抠出第一段 JSON。
fn extract_json(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{' || b == b'[')?;
    let open = bytes[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let mut depth = 0;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(&s[start..=i]);
            }
        }
    }
    None
}

// --- 真 LLM 实现 memory/learn 的潜意识接口 ---

/// 用真 LLM 实现的潜意识(检索判断 + 飞轮压缩)。
/// embed 走独立的 `Embedder` 槽位(DeepSeek 无嵌入端点,见 `embedding-service.md`);
/// judge_relevant / distill 走聊天 `driver`。
pub struct LlmBridge {
    driver: Arc<dyn LlmDriver>,
    model: String,
    max_tokens: Option<u32>,
    embedder: Arc<dyn Embedder>,
    /// `complete`(judge/distill 用)的沉默超时秒(默认 60,推论9 可设)。
    silence_secs: u64,
    /// 潜意识提示词语言(zh/en),用于提示词自转译取词(`transpile::catalog`)。默认 zh
    /// (与接线前硬编码中文逐字一致;`connect` 经 `with_prompt_lang` 设为 settings.lang)。
    prompt_lang: String,
}

impl LlmBridge {
    pub fn new(
        driver: Arc<dyn LlmDriver>,
        model: impl Into<String>,
        max_tokens: u32,
        embedder: Arc<dyn Embedder>,
        silence_secs: u64,
    ) -> Self {
        LlmBridge {
            driver,
            model: model.into(),
            max_tokens: if max_tokens > 0 { Some(max_tokens) } else { None },
            embedder,
            silence_secs: if silence_secs > 0 { silence_secs } else { 60 },
            prompt_lang: "zh".to_string(),
        }
    }

    /// 设潜意识提示词语言(zh/en)。`connect` 用 settings.lang 调,使 judge/distill 提示词与主模型同语言
    /// 且参与自转译(谁转译谁:潜意识那份由潜意识模型转)。非链式调用方默认保持 zh。
    pub fn with_prompt_lang(mut self, lang: &str) -> Self {
        self.prompt_lang = lang.to_string();
        self
    }

    /// 单条向量化:批量接口传长度 1,取首个;失败/空返回空向量(memory 视作未向量化,后续重试)。
    async fn embed_one(&self, text: &str, kind: EmbedKind) -> Vec<f32> {
        self.embedder
            .embed(std::slice::from_ref(&text.to_string()), kind)
            .await
            .ok()
            .and_then(|mut v| v.drain(..).next())
            .unwrap_or_default()
    }

    fn request(&self, system: &str, user: String) -> ChatRequest {
        let mut req = ChatRequest::new(
            self.model.clone(),
            vec![ChatMessage::system(system), ChatMessage::user(user)],
        );
        if let Some(mt) = self.max_tokens {
            req = req.with_max_tokens(mt);
        }
        req
    }
}

#[async_trait]
impl Subconscious for LlmBridge {
    async fn embed(&self, text: &str) -> Vec<f32> {
        // 用途未知时按文档处理(摄入是主路径)。
        self.embed_one(text, EmbedKind::Passage).await
    }

    async fn embed_query(&self, text: &str) -> Vec<f32> {
        self.embed_one(text, EmbedKind::Query).await
    }

    async fn embed_passage(&self, text: &str) -> Vec<f32> {
        self.embed_one(text, EmbedKind::Passage).await
    }

    fn embedding_version(&self) -> String {
        self.embedder.version()
    }

    async fn judge_relevant(&self, query: &str, candidates: &[String]) -> Vec<usize> {
        let mut user = format!("查询: {query}\n候选:\n");
        for (i, c) in candidates.iter().enumerate() {
            user.push_str(&format!("{i}. {}\n", c.replace('\n', " ")));
        }
        let sys = crate::transpile::catalog("subconscious.judge_relevant", &self.prompt_lang);
        let Ok(out) = complete(self.driver.as_ref(), self.request(&sys, user), self.silence_secs).await else {
            return Vec::new();
        };
        let Some(json) = extract_json(&out) else {
            return Vec::new();
        };
        serde_json::from_str::<Vec<usize>>(json)
            .unwrap_or_default()
            .into_iter()
            .filter(|&i| i < candidates.len())
            .collect()
    }

    async fn judge_edge(
        &self,
        query: &str,
        positives: &[String],
        negatives: &[String],
        target: &str,
    ) -> bool {
        let mut user = format!("当前提问: {query}\n\n这条联想边历次命中的提问(命中越多越说明对这类提问可靠):\n");
        if positives.is_empty() {
            user.push_str("(暂无)\n");
        }
        for p in positives {
            user.push_str(&format!("- {}\n", p.replace('\n', " ")));
        }
        user.push_str("\n这条边曾被判不相关的提问(命中相似提问应规避):\n");
        if negatives.is_empty() {
            user.push_str("(暂无)\n");
        }
        for n in negatives {
            user.push_str(&format!("- {}\n", n.replace('\n', " ")));
        }
        user.push_str(&format!("\n这条边通向的内容:\n{}\n", target.replace('\n', " ")));
        let sys = crate::transpile::catalog("subconscious.judge_edge", &self.prompt_lang);
        let Ok(out) = complete(self.driver.as_ref(), self.request(&sys, user), self.silence_secs).await else {
            return false; // 调不通 → 保守不跳(精确档宁漏不误召回)
        };
        let Some(json) = extract_json(&out) else {
            return false;
        };
        serde_json::from_str::<serde_json::Value>(json)
            .ok()
            .and_then(|v| v.get("jump").and_then(|x| x.as_bool()))
            .unwrap_or(false)
    }
}

#[async_trait]
impl Reasoner for LlmBridge {
    async fn distill(&self, cluster: &[Conclusion]) -> Option<Distillation> {
        let mut user = String::from("同类经验:\n");
        for c in cluster {
            user.push_str(&format!("- 操作: {} | 预期: {}\n", c.operation, c.expected));
        }
        let sys = crate::transpile::catalog("subconscious.distill", &self.prompt_lang);
        let out = complete(self.driver.as_ref(), self.request(&sys, user), self.silence_secs).await.ok()?;
        let json = extract_json(&out)?;
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        if v.get("none").and_then(|x| x.as_bool()).unwrap_or(false) {
            return None;
        }
        let operation = v.get("operation")?.as_str()?.to_string();
        let expected = v.get("expected")?.as_str()?.to_string();
        let prerequisites = v
            .get("prerequisites")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        Some(Distillation { operation, expected, prerequisites })
    }

    async fn propose_skill(&self, cluster: &[Conclusion]) -> Option<ProposedSkill> {
        let mut user = String::from("反复出现的同类经验:\n");
        for c in cluster {
            user.push_str(&format!("- 操作: {} | 结果: {}\n", c.operation, c.expected));
        }
        let sys = crate::transpile::catalog("subconscious.propose_skill", &self.prompt_lang);
        let out = complete(self.driver.as_ref(), self.request(&sys, user), self.silence_secs).await.ok()?;
        let json = extract_json(&out)?;
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        if v.get("none").and_then(|x| x.as_bool()).unwrap_or(false) {
            return None;
        }
        // name/trigger/body 三者齐全且非空才算有效提议(质量闸:残缺输出当不提议)。
        let name = v.get("name")?.as_str()?.trim().to_string();
        let trigger = v.get("trigger")?.as_str()?.trim().to_string();
        let body = v.get("body")?.as_str()?.trim().to_string();
        if name.is_empty() || body.is_empty() {
            return None;
        }
        Some(ProposedSkill { name, trigger, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_json_strips_fences() {
        let s = "好的,结果如下:\n```json\n[0, 2]\n```";
        assert_eq!(extract_json(s), Some("[0, 2]"));
        let obj = "前言 {\"a\": {\"b\": 1}} 后语";
        assert_eq!(extract_json(obj), Some("{\"a\": {\"b\": 1}}"));
    }

    /// 脚本化 driver:把预设正文切成 Content 块流式吐出。
    struct ScriptDriver {
        content: String,
    }
    #[async_trait]
    impl LlmDriver for ScriptDriver {
        async fn chat_stream(&self, _req: ChatRequest) -> LlmResult<mpsc::Receiver<LlmResult<StreamChunk>>> {
            let (tx, rx) = mpsc::channel(8);
            let content = self.content.clone();
            tokio::spawn(async move {
                let _ = tx.send(Ok(StreamChunk::Content(content))).await;
                let _ = tx.send(Ok(StreamChunk::Done { finish_reason: "stop".into() })).await;
            });
            Ok(rx)
        }
    }

    #[tokio::test]
    async fn judge_relevant_parses_indices() {
        let driver = Arc::new(ScriptDriver { content: "相关的是 [0, 2]".into() });
        let bridge = LlmBridge::new(driver, "m", 1024, Arc::new(growbox_llm::LexicalEmbedder), 60);
        let got = bridge
            .judge_relevant("q", &["a".into(), "b".into(), "c".into()])
            .await;
        assert_eq!(got, vec![0, 2]);
    }

    #[tokio::test]
    async fn judge_edge_parses_jump_verdict() {
        // 档B:LLM 读正负 K + 当前 query 输出 {"jump":bool}。
        let yes = LlmBridge::new(
            Arc::new(ScriptDriver { content: "综合判断:{\"jump\": true}".into() }),
            "m",
            1024,
            Arc::new(growbox_llm::LexicalEmbedder),
            60,
        );
        assert!(
            yes.judge_edge("q", &["历次命中(命中3次)".into()], &["曾被拒".into()], "目标内容").await,
            "jump:true → 跳"
        );
        let no = LlmBridge::new(
            Arc::new(ScriptDriver { content: "{\"jump\":false}".into() }),
            "m",
            1024,
            Arc::new(growbox_llm::LexicalEmbedder),
            60,
        );
        assert!(!no.judge_edge("q", &[], &[], "目标内容").await, "jump:false → 不跳");
    }

    #[tokio::test]
    async fn distill_parses_pattern() {
        let driver = Arc::new(ScriptDriver {
            content: "{\"operation\":\"给足 token\",\"expected\":\"工具调用正常\",\"prerequisites\":[\"推理模型\"]}".into(),
        });
        let bridge = LlmBridge::new(driver, "m", 1024, Arc::new(growbox_llm::LexicalEmbedder), 60);
        let exp = vec![Conclusion::experience("op", "ex", "s")];
        let d = bridge.distill(&exp).await.unwrap();
        assert_eq!(d.operation, "给足 token");
        assert_eq!(d.prerequisites, vec!["推理模型".to_string()]);
    }

    #[tokio::test]
    async fn distill_none_when_no_pattern() {
        let driver = Arc::new(ScriptDriver { content: "{\"none\":true}".into() });
        let bridge = LlmBridge::new(driver, "m", 1024, Arc::new(growbox_llm::LexicalEmbedder), 60);
        let exp = vec![Conclusion::experience("op", "ex", "s")];
        assert!(bridge.distill(&exp).await.is_none());
    }

    #[tokio::test]
    async fn propose_skill_parses_and_gates() {
        // 有效提议:name/trigger/body 齐全 → Some。
        let driver = Arc::new(ScriptDriver {
            content: "{\"name\":\"retry-on-truncation\",\"trigger\":\"工具调用返回空参时\",\"body\":\"1. 判截断\\n2. 翻倍 token 重试\"}".into(),
        });
        let bridge = LlmBridge::new(driver, "m", 1024, Arc::new(growbox_llm::LexicalEmbedder), 60);
        let exp = vec![Conclusion::experience("op", "ex", "s"), Conclusion::experience("op2", "ex2", "s")];
        let p = bridge.propose_skill(&exp).await.expect("应起草提议");
        assert_eq!(p.name, "retry-on-truncation");
        assert!(p.body.contains("翻倍 token"));
        // none / 残缺都视为不提议(质量闸)。
        let none_driver = Arc::new(ScriptDriver { content: "{\"none\":true}".into() });
        let b2 = LlmBridge::new(none_driver, "m", 1024, Arc::new(growbox_llm::LexicalEmbedder), 60);
        assert!(b2.propose_skill(&exp).await.is_none(), "none → 不提议");
        let partial = Arc::new(ScriptDriver { content: "{\"name\":\"x\"}".into() });
        let b3 = LlmBridge::new(partial, "m", 1024, Arc::new(growbox_llm::LexicalEmbedder), 60);
        assert!(b3.propose_skill(&exp).await.is_none(), "残缺(缺 body)→ 不提议");
    }

    /// 卡死 driver:发一个 chunk 后**永不关闭通道、永不再发**(持有 tx)——模拟 LLM 流挂起。
    /// 这正是"收口后光标一直转"的根因场景。
    struct StallDriver;
    #[async_trait]
    impl LlmDriver for StallDriver {
        async fn chat_stream(&self, _req: ChatRequest) -> LlmResult<mpsc::Receiver<LlmResult<StreamChunk>>> {
            let (tx, rx) = mpsc::channel(8);
            tokio::spawn(async move {
                let _ = tx.send(Ok(StreamChunk::Content("半截".into()))).await;
                // 之后永不发、永不关:持有 tx 不 drop,模拟流卡住。
                std::future::pending::<()>().await;
                drop(tx);
            });
            Ok(rx)
        }
    }

    /// 回归:流卡住时 complete 不再无限挂起,而是沉默超时后收手返回已累积内容。
    /// 用暂停的虚拟时钟,空转时自动快进到超时,测试瞬间完成(不真等 60s)。
    #[tokio::test(start_paused = true)]
    async fn complete_does_not_hang_on_stalled_stream() {
        let req = ChatRequest::new("m", vec![ChatMessage::user("q")]);
        let out = complete(&StallDriver, req, 60).await.expect("沉默超时应收手返回 Ok,不挂起");
        assert_eq!(out, "半截", "返回已累积内容,而非永久阻塞");
    }
}
