//! Embedding —— 第一层 RAG 的向量化槽位(与聊天 provider 独立)。
//!
//! 实现 `设计文档/计划/embedding-service.md`:
//! - DeepSeek 无 embedding 端点,故 embedder 是独立可切换槽位,不复用聊天 key。
//! - 远程(可选):OpenAI 兼容 `POST {base}/embeddings`。
//! - 本地(默认,当前):词法散列 `LexicalEmbedder`(离线、便宜)。candle e5-small 待接(见计划)。
//!
//! 关键牵连:换 embedder = 向量空间变,旧向量全失效。`version()` 是向量空间的指纹,
//! memory 用它判定是否重嵌(见 `Subconscious::embedding_version` / `Memory::ensure_embeddings`)。

use async_trait::async_trait;

use crate::error::{LlmError, LlmResult};

/// 嵌入用途 —— e5 类模型查询/文档要加不同前缀(实测坑),故在接口处区分。
/// 远程通用模型(text-embedding-3 等)不需要前缀,实现可忽略本参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedKind {
    /// 检索时的查询文本。
    Query,
    /// 入库的文档原文。
    Passage,
}

/// 向量化器。批量接口(远程一次请求多条更省);单条由调用方传长度 1 的切片。
#[async_trait]
pub trait Embedder: Send + Sync {
    /// 把一批文本向量化。返回与输入同序、同长度的向量列表。
    async fn embed(&self, texts: &[String], kind: EmbedKind) -> LlmResult<Vec<Vec<f32>>>;

    /// 向量空间指纹。换模型/换实现 → 此值变 → 旧向量失效需重嵌。
    fn version(&self) -> String;
}

// ===================== 词法版(当前默认,离线) =====================

const LEXICAL_DIM: usize = 256;

/// 词法散列向量化:确定、离线、便宜。无真语义(只抓字面词重叠),作 candle 落地前的默认。
pub struct LexicalEmbedder;

#[async_trait]
impl Embedder for LexicalEmbedder {
    async fn embed(&self, texts: &[String], _kind: EmbedKind) -> LlmResult<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| lexical_embed(t)).collect())
    }
    fn version(&self) -> String {
        "lexical-v1".to_string()
    }
}

/// 把文本散列成定长归一化向量。
pub fn lexical_embed(text: &str) -> Vec<f32> {
    let mut v = vec![0.0f32; LEXICAL_DIM];
    for tok in lex_tokens(text) {
        let idx = (fnv1a(&tok) % LEXICAL_DIM as u64) as usize;
        v[idx] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

/// ASCII 连续字母数字成词,CJK 单字成词。
fn lex_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            buf.push(ch.to_ascii_lowercase());
            continue;
        }
        if buf.chars().count() >= 2 {
            out.push(std::mem::take(&mut buf));
        } else {
            buf.clear();
        }
        if ('\u{4e00}'..='\u{9fff}').contains(&ch) {
            out.push(ch.to_string());
        }
    }
    if buf.chars().count() >= 2 {
        out.push(buf);
    }
    out
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ===================== 远程版(OpenAI 兼容) =====================

/// 远程向量化:OpenAI 兼容 `POST {base}/embeddings`。
/// 本地服务(Ollama / LM Studio)同此接口,base 指其 `/v1` 即可。
pub struct RemoteEmbedder {
    http: reqwest::Client,
    base: String,
    key: String,
    model: String,
}

impl RemoteEmbedder {
    pub fn new(base: impl Into<String>, key: impl Into<String>, model: impl Into<String>) -> Self {
        let base = base.into().trim_end_matches('/').to_string();
        RemoteEmbedder {
            // 本地/内网嵌入服务(Ollama/LM Studio)同样绕过系统代理,避免 502(见 client.rs)。
            http: crate::client::local_aware_client(&base),
            base,
            key: key.into(),
            model: model.into(),
        }
    }
}

#[async_trait]
impl Embedder for RemoteEmbedder {
    async fn embed(&self, texts: &[String], _kind: EmbedKind) -> LlmResult<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        // 远程通用模型不加 e5 前缀(那是 candle 本地 e5 实现的事);kind 在此忽略。
        let body = build_embed_body(&self.model, texts);
        let resp = self
            .http
            .post(format!("{}/embeddings", self.base))
            .header("Authorization", format!("Bearer {}", self.key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api { status, body });
        }
        let json: serde_json::Value = resp.json().await?;
        parse_embed_response(&json, texts.len())
    }

    fn version(&self) -> String {
        format!("remote:{}", self.model)
    }
}

/// 构造 OpenAI `/embeddings` 请求体(`model` + `input` 必填,实测)。
fn build_embed_body(model: &str, texts: &[String]) -> serde_json::Value {
    serde_json::json!({ "model": model, "input": texts })
}

/// 解析 `{data:[{embedding:[...],index}]}`,按 index 还原输入顺序。
fn parse_embed_response(json: &serde_json::Value, expected: usize) -> LlmResult<Vec<Vec<f32>>> {
    let data = json
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| LlmError::Parse("embeddings 响应缺 data 数组".into()))?;
    let mut out: Vec<Vec<f32>> = vec![Vec::new(); expected.max(data.len())];
    for (i, item) in data.iter().enumerate() {
        let idx = item.get("index").and_then(|x| x.as_u64()).map(|x| x as usize).unwrap_or(i);
        let vec = item
            .get("embedding")
            .and_then(|e| e.as_array())
            .ok_or_else(|| LlmError::Parse("embeddings 条目缺 embedding 数组".into()))?
            .iter()
            .map(|x| x.as_f64().unwrap_or(0.0) as f32)
            .collect();
        if idx < out.len() {
            out[idx] = vec;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lexical_is_deterministic_and_discriminative() {
        let e = LexicalEmbedder;
        let a = e.embed(&["flash 推理模型 token".into()], EmbedKind::Passage).await.unwrap();
        let b = e.embed(&["flash 推理模型 token".into()], EmbedKind::Query).await.unwrap();
        let c = e.embed(&["完全无关的内容 xyz".into()], EmbedKind::Passage).await.unwrap();
        assert_eq!(a[0], b[0], "同文本向量相同(词法版前缀无关)");
        assert_ne!(a[0], c[0], "不同文本向量不同");
        assert_eq!(e.version(), "lexical-v1");
    }

    #[test]
    fn embed_body_has_model_and_input() {
        let body = build_embed_body("text-embedding-3-small", &["a".into(), "b".into()]);
        assert_eq!(body["model"], "text-embedding-3-small");
        assert_eq!(body["input"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn parse_response_restores_index_order() {
        // 故意乱序:index 1 先来,index 0 后到。
        let json = serde_json::json!({
            "data": [
                { "index": 1, "embedding": [0.0, 1.0] },
                { "index": 0, "embedding": [1.0, 0.0] }
            ]
        });
        let out = parse_embed_response(&json, 2).unwrap();
        assert_eq!(out[0], vec![1.0, 0.0]);
        assert_eq!(out[1], vec![0.0, 1.0]);
    }

    #[test]
    fn parse_response_errors_without_data() {
        let json = serde_json::json!({ "error": "bad" });
        assert!(parse_embed_response(&json, 1).is_err());
    }

    #[test]
    fn remote_version_tracks_model() {
        let e = RemoteEmbedder::new("https://x/v1", "k", "text-embedding-3-small");
        assert_eq!(e.version(), "remote:text-embedding-3-small");
    }
}
