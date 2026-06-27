//! 整条 Agent 循环的端到端单测(脚本化 LLM + 收集器 sink),从 agent.rs 内联测试迁出。

use super::*;
use crate::decision::{Decision, DecisionKind};
use growbox_core::ToolCall;
use growbox_llm::lexical_embed;
use growbox_llm::StreamChunk;
use crate::tasks::TaskManager;
use growbox_llm::LlmResult;
use parking_lot::Mutex;
use std::collections::VecDeque;
use tempfile::tempdir;
use tokio::sync::mpsc;

/// 收集所有事件,便于断言。
#[derive(Default)]
struct Collector {
    events: Mutex<Vec<AgentEvent>>,
    decisions: Mutex<Vec<DecisionKind>>,
}
#[async_trait::async_trait]
impl EventSink for Collector {
    async fn emit(&self, event: AgentEvent) {
        self.events.lock().push(event);
    }
    async fn request_decision(&self, kind: DecisionKind) -> Decision {
        self.decisions.lock().push(kind.clone());
        // 沿用 trait 默认语义(无前端):shell 放行一次,路径授权拒绝(无用户在场)。
        match kind {
            DecisionKind::ShellApproval { .. } => Decision::Once,
            DecisionKind::PathPermission { .. } => Decision::Deny,
        }
    }
}
impl Collector {
    fn kinds(&self) -> Vec<AgentEvent> {
        self.events.lock().clone()
    }
    fn decision_kinds(&self) -> Vec<DecisionKind> {
        self.decisions.lock().clone()
    }
    fn has_content(&self, needle: &str) -> bool {
        self.events
            .lock()
            .iter()
            .any(|e| matches!(e, AgentEvent::Content(c) if c.contains(needle)))
    }
}

/// 脚本化 LLM:每次 chat_stream 取队列下一组 chunk 流式吐出。
/// 同时记录每次调用被暴露的工具名(供工作流"工具收窄"断言)。
struct Scripted {
    turns: Mutex<VecDeque<Vec<StreamChunk>>>,
    seen_tools: Mutex<Vec<Vec<String>>>,
    /// 每次 LLM 调用被发送的全部消息正文(供栈函数 v2 上下文切片/返回值回灌断言)。
    seen_messages: Mutex<Vec<String>>,
}
impl Scripted {
    fn new(turns: Vec<Vec<StreamChunk>>) -> Self {
        Scripted { turns: Mutex::new(turns.into()), seen_tools: Mutex::new(Vec::new()), seen_messages: Mutex::new(Vec::new()) }
    }
    /// 第 n 次 LLM 调用被暴露的工具名集合。
    fn tools_at(&self, n: usize) -> Vec<String> {
        self.seen_tools.lock().get(n).cloned().unwrap_or_default()
    }
    /// 第 n 次 LLM 调用所发送的全部消息正文拼接(用于断言哪些上下文可见/被隐藏)。
    fn messages_text_at(&self, n: usize) -> String {
        self.seen_messages.lock().get(n).cloned().unwrap_or_default()
    }
}
#[async_trait::async_trait]
impl LlmDriver for Scripted {
    async fn chat_stream(&self, req: ChatRequest) -> LlmResult<mpsc::Receiver<LlmResult<StreamChunk>>> {
        self.seen_tools.lock().push(req.tools.iter().map(|t| t.name.clone()).collect());
        self.seen_messages.lock().push(req.messages.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n----\n"));
        let chunks = self.turns.lock().pop_front().unwrap_or_default();
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            for c in chunks {
                if tx.send(Ok(c)).await.is_err() {
                    return;
                }
            }
        });
        Ok(rx)
    }
}

/// 不下沉的 mock 潜意识:embed 用本地词法,judge 永远空(测试不触发精确层)。
struct LocalSub;
#[async_trait::async_trait]
impl Subconscious for LocalSub {
    async fn embed(&self, text: &str) -> Vec<f32> {
        lexical_embed(text)
    }
    async fn judge_relevant(&self, _q: &str, _c: &[String]) -> Vec<usize> {
        Vec::new()
    }
}

/// Mock Reasoner:永不提炼出模式(测试时飞轮 turn 不产出知识,只走流程)。
struct NullReasoner;
#[async_trait::async_trait]
impl Reasoner for NullReasoner {
    async fn distill(&self, _cluster: &[growbox_core::Conclusion]) -> Option<growbox_learn::Distillation> {
        None
    }
}

fn tool_call_chunks(id: &str, name: &str, args: &str) -> Vec<StreamChunk> {
    vec![
        StreamChunk::Reasoning("我需要调用工具".into()),
        StreamChunk::ToolCallDelta {
            index: 0,
            id: Some(id.into()),
            name: Some(name.into()),
            args_fragment: args.into(),
        },
        StreamChunk::Done { finish_reason: "tool_calls".into() },
    ]
}

/// 最终答复:输出一段正文 + 调用 finish 收口(新语义:裸文本不再等于完成)。
fn answer_then_finish(text: &str) -> Vec<StreamChunk> {
    vec![
        StreamChunk::Content(text.into()),
        StreamChunk::ToolCallDelta {
            index: 0,
            id: Some("fin".into()),
            name: Some("finish".into()),
            args_fragment: "{}".into(),
        },
        StreamChunk::Done { finish_reason: "tool_calls".into() },
    ]
}

/// 只有裸文本、不调用任何工具(模拟"叙述下一步就停"的早停)。
fn bare_text(text: &str) -> Vec<StreamChunk> {
    vec![StreamChunk::Content(text.into()), StreamChunk::Done { finish_reason: "stop".into() }]
}

fn cfg() -> AgentConfig {
    // 仅列与基线不同的字段;其余经 AgentConfig::default()(测试基线)兜底,加新字段不必改本处。
    AgentConfig { model: "m".into(), system_prompt: "你是助手".into(), ..Default::default() }
}

/// 定位测试用 rust-analyzer 并设环境变量(让 LspManager 经 env 找到它):env → 仓库 `.tooling/ra`。
/// 无则 None(无 RA 时本测试跳过、不挂)。
fn ensure_test_ra_env() -> Option<String> {
    if let Ok(p) = std::env::var("GROWBOX_LSP_RUST_ANALYZER") {
        if std::path::Path::new(&p).is_file() {
            return Some(p);
        }
    }
    let local = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.tooling/ra");
    if std::path::Path::new(local).is_file() {
        std::env::set_var("GROWBOX_LSP_RUST_ANALYZER", local);
        return Some(local.to_string());
    }
    None
}

/// ★A2 全链路自测(真 rust-analyzer)★:脊柱经 `registry.lsp_manager()`(与 lsp 执行器**共享的同一个 Arc**)
/// 拿到暖客户端 → 编辑后 `perceive_rust_diagnostics` 取真实诊断 → 写主记忆感知。
/// 验证单测未覆盖的"共享 Arc 接线 + 真 RA + perceive"端到端。无 RA 跳过。
#[tokio::test]
async fn a2_spine_perceives_real_diagnostic_via_shared_manager() {
    if ensure_test_ra_env().is_none() {
        eprintln!("skip: 无 rust-analyzer(.tooling/ra 或 GROWBOX_LSP_RUST_ANALYZER)");
        return;
    }
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"fix\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    let main_rs = dir.path().join("src/main.rs");
    let err_src = "fn main() {\n    let _ = no_such_symbol_zzz;\n}\n";
    std::fs::write(&main_rs, err_src).unwrap();

    let reg = Registry::with_builtins(TaskManager::new());
    // 经共享管理器(就是 lsp 执行器会用的同一个 Arc<LspManager>)暖起 RA 并等它分析出诊断。
    let client = reg.lsp_manager().rust_client(dir.path()).await.expect("起 rust-analyzer");
    let mut ready = false;
    for _ in 0..40 {
        client.did_open(&main_rs, err_src, "rust");
        if !client.diagnostics_for(&main_rs).is_empty() {
            ready = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(ready, "rust-analyzer 应在超时内对错误文件发出诊断");

    // 脊柱 A2 helper:经 registry.lsp_manager() 拉编辑后诊断 → perceive(双路:瞬态环 + 时间线)。
    let mut mem = Memory::new();
    super::perceive_rust_diagnostics(reg.lsp_manager(), &mut mem, "zh", dir.path(), std::slice::from_ref(&main_rs)).await;

    assert!(mem.internal_event_count() >= 1, "应感知到诊断事件");
    let state = mem.render_internal_state("zh").unwrap_or_default();
    eprintln!("[A2 全链路自测] {state}");
    assert!(state.contains("rust-analyzer") && state.contains("error"), "内部状态应含真实诊断: {state}");
    assert!(
        state.contains("no_such_symbol_zzz") || state.to_lowercase().contains("cannot find"),
        "应含未解析符号: {state}"
    );
}

#[tokio::test]
async fn loop_executes_tool_then_answers() {
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        // 第1轮:写文件
        tool_call_chunks("c1", "file_write", r#"{"path":"note.txt","content":"hello"}"#),
        // 第2轮:正文 + finish 收口
        answer_then_finish("已写好 note.txt"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("写个 note", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;

    assert_eq!(out.stopped, StopReason::Completed);
    assert_eq!(out.turns, 2);
    assert_eq!(out.final_text, "已写好 note.txt");
    // 工具真执行:文件落盘。
    assert_eq!(std::fs::read_to_string(dir.path().join("note.txt")).unwrap(), "hello");
    // 学习:采集了一条经验。
    assert_eq!(mem.conclusions().len(), 1);
    assert!(sink.has_content("已写好"));
    // 事件序列含 ToolStart/ToolEnd/Done。
    let kinds = sink.kinds();
    assert!(kinds.iter().any(|e| matches!(e, AgentEvent::ToolStart { name, .. } if name == "file_write")));
    assert!(kinds.iter().any(|e| matches!(e, AgentEvent::ToolEnd { ok: true, .. })));
    assert!(kinds.last() == Some(&AgentEvent::Done));
}

/// ★回归:跑满 max_turns 也要把最后一条已展示的回复落库(role=assistant)★
/// 修「AI 回复丢失」(2026-06-15 真机暴露):此前只有 finish 收口路径 ingest assistant,跑满
/// max_turns(尤其 Supervisor 后台回合 max_turns=4 常如此)时 final_text 只经 sink 流式展示过、
/// 从未落时间线 → 重载/切项目后"好多 AI 回复没了"。本测试在修复前会失败(时间线无该 assistant 节点)。
#[tokio::test]
async fn maxturns_persists_last_visible_reply() {
    let dir = tempdir().unwrap();
    // 每轮都只产出**不同**裸文本且从不调 finish → 不触发退化早停(同文本才退化)、干净耗尽 max_turns。
    let llm = Scripted::new(vec![
        bare_text("第一步:我先看看现状"),
        bare_text("第二步:接着我动手改"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;
    let mut c = cfg();
    c.max_turns = 2; // 两轮即到顶,经 MaxTurns 收口

    let out = agent_loop("做点事", &c, &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;

    assert_eq!(out.stopped, StopReason::MaxTurns, "应因跑满轮数收口");
    assert_eq!(out.final_text, "第二步:接着我动手改");
    assert!(sink.has_content("第二步"), "最后一条回复应已流式展示给用户");
    // ★核心断言★:最后一条已展示的回复必须落进时间线(role=assistant),否则重载后就没了。
    let tl = mem.timeline();
    let persisted = tl
        .metas()
        .iter()
        .any(|m| m.role == "assistant" && tl.content(&m.id).as_deref() == Some("第二步:接着我动手改"));
    assert!(persisted, "跑满 max_turns 时最后一条已展示的回复应落库为 assistant(不变式:展示过即落库)");
}

/// ★工具记忆·一票否决(计划/工具记忆-不犯第二遍 B)★:预记一条 file_write 的 infeasible 工具记忆 →
/// AI 再调 file_write 时分发前会诊命中 → **否决,不执行**(文件不写)+ 回执带否决说明。阈值设 0
/// 使测试与具体嵌入器无关(任何相似度都触发)。
#[tokio::test]
async fn loop_tool_memory_vetoes_known_infeasible() {
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "file_write", r#"{"path":"x.txt","content":"y"}"#),
        answer_then_finish("好的,换个做法"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    // 预记一条 infeasible 工具记忆 + 即时嵌入,让分发前会诊命中。
    mem.crystallize_tool_memory(
        "file_write",
        "写 x.txt",
        growbox_memory::tool_memory_format::Verdict::Infeasible,
        "演示:此处不可写",
        &LocalSub,
    )
    .await;
    assert_eq!(mem.tool_memory_count(), 1, "成本门:已有 1 条工具记忆");
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;
    let mut c = cfg();
    c.tool_memory_enabled = true;
    c.tool_memory_veto_threshold = 0.0; // 任何相似度都否决(测试确定性,不依赖嵌入器)
    let out = agent_loop("写 x", &c, &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;

    assert_eq!(out.stopped, StopReason::Completed);
    // ★被否决:文件未写★(会诊拦在 dispatch 前)。
    assert!(!dir.path().join("x.txt").exists(), "infeasible 工具记忆应否决该调用 → 文件不该被写");
    // 回执:file_write 的 ToolEnd ok=false 且带否决说明。
    assert!(
        sink.kinds().iter().any(|e| matches!(e, AgentEvent::ToolEnd { name, ok: false, content }
            if name == "file_write" && content.contains("一票否决"))),
        "应有一条带「一票否决」的失败回执"
    );
}

/// ★工具记忆·关时零行为(总开关)★:`tool_memory_enabled=false` → 同样预记 infeasible 也不会诊,
/// file_write 照常执行(文件写出)。证明特性是纯增量、可彻底关。
#[tokio::test]
async fn loop_tool_memory_disabled_does_not_veto() {
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "file_write", r#"{"path":"x.txt","content":"y"}"#),
        answer_then_finish("写好了"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    mem.crystallize_tool_memory("file_write", "写 x.txt", growbox_memory::tool_memory_format::Verdict::Infeasible, "x", &LocalSub).await;
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;
    let c = cfg(); // tool_memory_enabled = false(cfg 默认)
    let _ = agent_loop("写 x", &c, &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(std::fs::read_to_string(dir.path().join("x.txt")).unwrap(), "y", "关时不会诊,file_write 照常执行");
}

#[tokio::test]
async fn loop_emits_permission_on_out_of_sandbox() {
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let abs = outside.path().join("x.txt");
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "file_write", &format!(r#"{{"path":"{}","content":"y"}}"#, abs.display())),
        answer_then_finish("我没有权限写那里"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("写到外面", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;

    assert_eq!(out.stopped, StopReason::Completed);
    // 越界写 → 经决定脊柱请求路径授权(无前端 → 默认拒绝),文件未被创建。
    assert!(sink.decision_kinds().iter().any(|k| matches!(k, DecisionKind::PathPermission { .. })));
    assert!(!abs.exists());
}

#[tokio::test]
async fn loop_retries_on_truncated_tool_then_succeeds() {
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        // 第1轮:截断(finish=length)且工具空参 → 应重试
        vec![
            StreamChunk::Reasoning("想很久把 token 用光".into()),
            StreamChunk::ToolCallDelta { index: 0, id: Some("c1".into()), name: Some("file_list".into()), args_fragment: "".into() },
            StreamChunk::Done { finish_reason: "length".into() },
        ],
        // 重试:给出完整工具调用
        tool_call_chunks("c1", "file_list", r#"{"path":"."}"#),
        // 第2轮:正文 + finish 收口
        answer_then_finish("列好了"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("列目录", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    // 发了截断重试通知。
    assert!(sink.kinds().iter().any(|e| matches!(e, AgentEvent::Notice(_))));
}

/// 阻塞裁决类面板(create_project = gated hand_off):弹板即让位暂停,不继续、不催续。
/// 修正"弹板假成功 + 空转催续推着冲、绕过项目系统在错目录硬写"的缺陷(用户决策 2026-06-02)。
#[tokio::test]
async fn gated_create_project_yields_and_pauses() {
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        // 第1轮:发起新建项目(gated)。
        tool_call_chunks("c1", "create_project", r#"{"name":"个人博客"}"#),
        // 第2轮:若循环错误地继续,会消费这条 → 用它证明"没继续"。
        answer_then_finish("不应到达:面板未处理就继续了"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("帮我建个博客项目", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;

    // 让位暂停,而非 Completed/MaxTurns。
    assert_eq!(out.stopped, StopReason::AwaitingUser("open_new_project".into()));
    assert_eq!(out.turns, 1, "弹板即暂停,只跑一轮");
    let kinds = sink.kinds();
    // 发了打开面板的 Intent,且标记为 gated。
    assert!(
        kinds.iter().any(|e| matches!(e, AgentEvent::Intent(i) if i.action == "open_new_project" && i.gates)),
        "应 emit gated 的 open_new_project Intent"
    );
    // 没有空转催续 Notice(让位不是空转),也没继续消费第2轮的 finish。
    assert!(!sink.has_content("不应到达"), "暂停后不该继续往下跑");
    assert!(!kinds.iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("尚未完成"))), "让位不该触发催续");
    // 收口是 Done。
    assert!(kinds.last() == Some(&AgentEvent::Done));
}

/// ask_user:agent 显式提问 → AwaitingUser 干净暂停,不催续、不重复、不外泄催续提示。
/// 修真实 bug(用户 2026-06-03 报):纯文字问用户被"只说话=空转"催续碾过 → 重复回答 + "尚未完成"外泄。
#[tokio::test]
async fn ask_user_yields_awaiting_user_no_nudge() {
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        // 第1轮:调 ask_user 提问(附 options)。
        tool_call_chunks("a1", "ask_user", r#"{"question":"用哪种双语方案?","options":["i18next","查询参数"]}"#),
        // 第2轮:若循环错误地继续/催续,会消费这条 → 证明"没继续、没重复答"。
        answer_then_finish("不应到达:提问后未等用户就继续了"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("我的博客双语切换做了吗?", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;

    // 让位暂停等用户,而非 Completed/MaxTurns。
    assert_eq!(out.stopped, StopReason::AwaitingUser("ask_user".into()));
    assert_eq!(out.turns, 1, "提问即暂停,只跑一轮");
    // 问题(含选项)作为助手消息显示给用户。
    assert!(sink.has_content("用哪种双语方案?"), "问题应作为 Content 显示");
    // 没继续消费第2轮、没空转催续、不重复。
    assert!(!sink.has_content("不应到达"), "提问后不该继续往下跑");
    let kinds = sink.kinds();
    assert!(
        !kinds.iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("尚未完成"))),
        "提问让位不该触发催续"
    );
    assert!(kinds.last() == Some(&AgentEvent::Done));
}

/// 内部消息:AI 有权"仅当作信息"——只说一句、不调工具,即优雅结束,不催续、不强制 finish。
/// 与用户消息的本质区别(用户决策 2026-06-02)。
#[tokio::test]
async fn internal_seed_may_be_info_only_no_nudge() {
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        // 内部消息:AI 选择仅当信息——只说一句,不动手。
        bare_text("收到,我知道了。"),
        // 若错误地催续/继续,会消费这条 → 证明"没继续"。
        answer_then_finish("不应到达:内部消息不该被强制执行"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop_internal("项目已创建,可继续刚才的任务", None, &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink, false, false).await;

    assert_eq!(out.stopped, StopReason::Completed);
    assert_eq!(out.turns, 1, "内部消息无工具调用即优雅结束");
    assert!(!sink.kinds().iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("尚未完成"))), "内部消息不该催续");
    assert!(!sink.has_content("不应到达"), "不该继续往下跑");
}

/// 内部消息也可触发动手(AI 选择执行时照常推进到 finish):第1轮调工具,第2轮收口。
#[tokio::test]
async fn internal_seed_may_act_when_chosen() {
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "file_write", r#"{"path":"n.txt","content":"x"}"#),
        answer_then_finish("写好了"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop_internal("项目已创建,把文件写了", None, &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink, false, false).await;

    assert_eq!(out.stopped, StopReason::Completed);
    assert_eq!(out.turns, 2, "选择动手则照常推进到 finish");
    assert_eq!(std::fs::read_to_string(dir.path().join("n.txt")).unwrap(), "x");
}

/// ★A2 诊断推感知层·门控★:rust-analyzer 未起(无暖客户端)时,编辑 .rs 不应隐式拉起服务器、
/// 也不向感知层写任何东西(静默)—— 编辑回合零额外延迟、零噪音。
#[tokio::test]
async fn diagnostics_silent_when_no_warm_lsp_client() {
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    let rs = dir.path().join("src/lib.rs");
    std::fs::write(&rs, "pub fn f() {}\n").unwrap();

    let mgr = crate::lsp::LspManager::new(); // 全新,无任何客户端
    let mut mem = Memory::new();
    assert_eq!(mem.internal_event_count(), 0);

    perceive_rust_diagnostics(&mgr, &mut mem, "zh", dir.path(), &[rs]).await;

    // 无暖客户端 → 早退,不感知(瞬态环空、时间线无 internal 节点)。
    assert_eq!(mem.internal_event_count(), 0, "无 rust-analyzer 时不该感知诊断");
}

/// ★二期 B3 结晶端到端★:LLM 调 learn_process → 脊柱拦截 → crystallize_process 写主记忆。
/// 验证 process 节点真被结晶进时间线 + 回执确认(B3 报告-纠正回路的写入半接通)。
#[tokio::test]
async fn learn_process_crystallizes_into_memory() {
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        tool_call_chunks(
            "c1",
            "learn_process",
            r#"{"name":"加数值设置","recipe":"碰 Settings → 命令 → tauri-api → state → 面板 → i18n 四国"}"#,
        ),
        answer_then_finish("流程已记下"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out =
        agent_loop("以后加设置怎么做", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);

    // 主记忆里多了一条 process 节点(脊柱拦截 learn_process → crystallize_process 写入)。
    let process_nodes = mem
        .timeline()
        .metas()
        .iter()
        .filter(|m| m.role == growbox_memory::node_kind::PROCESS)
        .count();
    assert_eq!(process_nodes, 1, "learn_process 应结晶出一条 process 节点");
    // 回执确认结晶(非取代,因库里此前无近重复)。
    assert!(
        sink.kinds().iter().any(|e| matches!(
            e,
            AgentEvent::ToolEnd { name, content, .. } if name == "learn_process" && content.contains("结晶")
        )),
        "应有 learn_process 的结晶回执"
    );
}

/// ★二期 C2 可执行档端到端★:库里有一条"可执行流程"(content 带 `wf:` 指向已注册工作流)→ 用户任务
/// 起手召回它 → 脊柱解析 `wf:` + 物化该工作流 + 注入"可执行项目流程·直接运行"块,引导 AI 栈调用工作流。
#[tokio::test]
async fn executable_process_materializes_on_recall() {
    let dir = tempdir().unwrap();
    let reg = Registry::with_builtins(TaskManager::new());
    let mut mem = Memory::new();
    // 本测试验证 C2 的脊柱接线(召回→物化→注入),非召回阈值调参(那由 memory 单测覆盖)。
    // LocalSub 用词法向量,生产 RAG 阈值(0.85)对词法相似偏严,这里调低以稳定触发召回。
    let mut rcfg = mem.retrieval_config();
    rcfg.rag_hit_threshold = 0.5;
    mem.set_retrieval_config(rcfg);
    // 结晶一条可执行档:`wf:` 指向内置 create_artifact_workflow(确实已注册)。
    let content = "【新建造物】要新建一个可交互造物(小工具/小游戏)时,先想清结构与交互再一次性创建。\nwf: create_artifact_workflow";
    mem.crystallize_process(content, &LocalSub).await;

    let llm = Scripted::new(vec![answer_then_finish("好的,我用现成造物工作流来做")]);
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;
    let out = agent_loop(
        "帮我新建一个可交互造物小游戏",
        &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink,
    )
    .await;
    assert_eq!(out.stopped, StopReason::Completed);

    // 第 0 轮请求里应注入"可执行项目流程"块 + 引导调用该工作流(物化成功),且 `wf:` 标记行被剥掉。
    let msgs0 = llm.messages_text_at(0);
    assert!(msgs0.contains("可执行项目流程"), "应注入可执行流程块: {msgs0}");
    assert!(
        msgs0.contains("create_artifact_workflow") && msgs0.contains("调用工作流"),
        "应引导栈调用该工作流: {msgs0}"
    );
    // 该工作流作为动态工具可被 AI 栈调用(非懒模式下全量暴露;懒模式由 materialized 物化进来)。
    assert!(
        llm.tools_at(0).iter().any(|t| t == "create_artifact_workflow"),
        "工作流应作为可栈调用的工具出现"
    );
}

/// ★主动自检(grounded verification)★:做了实事(工具调用≥阈值)的任务,首次 finish 前脊柱注入
/// "重读相关文件核对你的汇报"指令;AI 重读后第二次 finish 才真正收口(至多一轮),最终用核实版结论。
#[tokio::test]
async fn self_verify_injects_recheck_before_finish() {
    let dir = tempdir().unwrap();
    let mut c = cfg();
    c.self_verify = true;
    c.self_verify_min_tools = 1;
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "file_write", r#"{"path":"a.txt","content":"hi"}"#),
        answer_then_finish("我改好了 a.txt"),         // 第一次 finish → 被自检拦截
        answer_then_finish("已核实:a.txt 内容为 hi"), // 重读后第二次 finish → 真收口
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;
    let out = agent_loop("改下 a.txt", &c, &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    // 第 3 次 LLM 调用(index 2)的上下文应含脊柱在首次 finish 后注入的自检指令。
    let ctx2 = llm.messages_text_at(2);
    assert!(ctx2.contains("收尾前验收") || ctx2.contains("拿真实证据"), "首次 finish 前应注入验收指令: {ctx2}");
    // 最终结论是核实版(第二次 finish),不是首次未核对那条。
    assert_eq!(out.final_text, "已核实:a.txt 内容为 hi");
    assert!(sink.kinds().iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("自检"))), "应有自检 Notice");
}

/// 自检未达阈值(或关):不注入,首次 finish 直接收口(省 token)。
#[tokio::test]
async fn self_verify_below_threshold_finishes_directly() {
    let dir = tempdir().unwrap();
    let mut c = cfg();
    c.self_verify = true;
    c.self_verify_min_tools = 5; // 阈值高,本任务 1 次工具调用达不到 → 不自检
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "file_write", r#"{"path":"a.txt","content":"hi"}"#),
        answer_then_finish("done"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let out = agent_loop("改下 a.txt", &c, &llm, &reg, &sb, &mut mem, &LocalSub, &NullReasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    assert_eq!(out.final_text, "done", "未达阈值:首次 finish 直接收口、不注入自检");
    assert!(!sink.kinds().iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("自检"))), "不应触发自检");
}

/// ★C1 懒加载端到端★:开关开时,LLM 调 tool_search 把 deferred 工具的 schema 拉回上下文。
#[tokio::test]
async fn lazy_tool_search_returns_deferred_schema() {
    let dir = tempdir().unwrap();
    let mut reg = Registry::with_builtins(TaskManager::new());
    reg.set_lazy_tools(true, vec!["code_search".into()]); // 懒加载开,code_search 露名
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "tool_search", r#"{"query":"code_search"}"#),
        answer_then_finish("了解了 code_search 用法"),
    ]);
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("帮我找代码", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    // tool_search 回执含 code_search 的 schema(脊柱拦截 → registry.search_tools)。
    assert!(
        sink.kinds().iter().any(|e| matches!(
            e,
            AgentEvent::ToolEnd { name, content, .. } if name == "tool_search" && content.contains("code_search") && content.contains("pattern")
        )),
        "tool_search 应返回 code_search 的完整 schema"
    );
}

/// ★C1 工作流节点软锁(懒加载开)★:进入只允许 {render_artifact/file_read/file_list} 的 design 节点后,
/// 调 shell(节点外)→ 脊柱派发前拒绝 + 引导,不执行;随后 finish 正常收口。
#[tokio::test]
async fn lazy_node_lock_rejects_off_node_tool() {
    let dir = tempdir().unwrap();
    let mut reg = Registry::with_builtins(TaskManager::new());
    reg.set_lazy_tools(true, vec![]); // 懒加载开(不 defer 任何工具,纯验节点软锁)
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "shell", r#"{"command":"echo hi"}"#), // design 节点不允许 shell
        answer_then_finish("好"),
    ]);
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop_internal(
        "画个东西",
        Some(("create_artifact_workflow".into(), "design".into())),
        &cfg(),
        &llm,
        &reg,
        &sb,
        &mut mem,
        &LocalSub,
        &reasoner,
        &fw,
        dir.path(),
        &sink,
        false,
        false,
    )
    .await;
    assert_eq!(out.stopped, StopReason::Completed);
    // shell 被节点软锁拒绝(ok=false + "不允许"),且未真执行。
    assert!(
        sink.kinds().iter().any(|e| matches!(
            e,
            AgentEvent::ToolEnd { name, ok: false, content } if name == "shell" && content.contains("不允许")
        )),
        "design 节点应拒绝 shell"
    );
}

#[tokio::test]
async fn finish_executor_terminates_cleanly() {
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![vec![
        StreamChunk::ToolCallDelta {
            index: 0,
            id: Some("fin".into()),
            name: Some("finish".into()),
            args_fragment: r#"{"summary":"博客已建好,npm start 可运行"}"#.into(),
        },
        StreamChunk::Done { finish_reason: "tool_calls".into() },
    ]]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("建博客", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    assert_eq!(out.turns, 1);
    assert_eq!(out.final_text, "博客已建好,npm start 可运行");
    // finish 是控制信号,不该被采集为经验。
    assert_eq!(mem.conclusions().len(), 0);
}

#[tokio::test]
async fn bare_text_nudges_instead_of_stopping() {
    // 模型先"叙述下一步就停"(早停),被提醒后继续动手,最后 finish。
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        bare_text("现在创建样式文件和示例文章。"), // 旧逻辑会在这里就结束
        tool_call_chunks("c1", "file_write", r#"{"path":"style.css","content":"body{}"}"#),
        answer_then_finish("样式写好了"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("建博客", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    // 没有在第1轮裸文本就停;真的写了文件。
    assert!(out.turns >= 2, "裸文本不该直接结束,应被提醒后继续");
    assert!(dir.path().join("style.css").exists(), "提醒后模型应真的动手");
    // 新语义(思考免死):内部温和引导,不再向用户外泄"尚未完成"催续 toast(用户曾报此泄漏)。
    assert!(!sink.kinds().iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("尚未完成"))), "不应外泄催续提示给用户");
    // 关键:自问自答的脚手架对记忆系统不可见——未完成的叙述与催促都不入 memory。
    let in_memory = |needle: &str| {
        mem.timeline()
            .metas()
            .iter()
            .filter_map(|m| mem.timeline().content(&m.id))
            .any(|c| c.contains(needle))
    };
    assert!(!in_memory("现在创建样式文件和示例文章"), "被催促的半截叙述不应进记忆");
    assert!(!in_memory("还没有调用 finish"), "催促提醒不应进记忆");
}

#[tokio::test]
async fn repeated_bare_text_stops_gracefully() {
    // 连续产出"近乎全等"= 真高频重复死循环 → 退化收口(思考免死的唯一例外)。
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        bare_text("我在等你的指示。"),
        bare_text("我在等你的指示。"), // 与上一轮近乎全等 → 第2轮触发退化收口
        // 若未收口会 pop 第3轮写文件,以验证不会走到
        tool_call_chunks("c1", "file_write", r#"{"path":"should_not.txt","content":"x"}"#),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("闲聊", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    assert_eq!(out.turns, 2, "连续近乎全等第2轮即退化收口");
    assert_eq!(out.final_text, "我在等你的指示。");
    assert!(!dir.path().join("should_not.txt").exists(), "退化收口后不应再执行第3轮");
    assert!(sink.kinds().iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("高频重复"))), "应告知退化收口");
}

#[tokio::test]
async fn novel_thinking_is_immune_from_finalize() {
    // ★思考免死★:每轮产出不同内容(在推进/想问题)→ 不因"没调工具"被收口,
    // 一直继续直到真动手(finish)。旧逻辑会在第 2 轮 max_stall 就误杀。
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        bare_text("先想想棋盘怎么画。"),
        bare_text("再想想 AI 怎么落子。"),
        bare_text("还要想想胜负判定。"),
        answer_then_finish("想清楚了,开始。"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("陪我下五子棋", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    assert_eq!(out.turns, 4, "新内容每轮免死,不被中途收口,直到 finish 才结束");
    assert!(!sink.kinds().iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("高频重复"))), "新内容不该判退化");
}

#[tokio::test]
async fn runaway_render_is_capped() {
    // ★重画上限 → 停下告知用户(用户 2026-06-04 真机:收不到交互就别反复重绘)★:AI 蒙眼反复重画(CPU 345% 失控真因)
    // → 超 MAX_RENDER_PER_RUN(3)即停下、给用户可见消息、收口,不再自动续重画(用户让重试时下一条消息再继续)。
    // 脚本强制连续 12 轮 render_artifact,验证第 4 次起被拦停、提前 Completed、用户收到说明。
    let dir = tempdir().unwrap();
    let turns: Vec<_> = (0..12)
        .map(|_| tool_call_chunks("r", "render_artifact", r#"{"html":"<div>board</div>"}"#))
        .collect();
    let llm = Scripted::new(turns);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;
    let mut c = cfg();
    c.max_turns = 12; // 给足轮数,验证不是靠 max_turns 收口而是第 4 次 render 主动拦停
    let out = agent_loop("画棋盘", &c, &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    // 拦停 ToolEnd(含"上限")+ 用户可见 Content(含"重画"说明)+ 提前收口(Completed,远不到 12 轮)。
    let capped = sink.kinds().iter().any(|e| matches!(e, AgentEvent::ToolEnd { content, .. } if content.contains("上限")));
    assert!(capped, "连续 render 超上限应被拦停");
    assert!(sink.has_content("重画"), "拦停后应给用户可见消息说明停下");
    assert!(matches!(out.stopped, StopReason::Completed), "拦停后回合 Completed 收口");
    assert!(out.turns <= 4, "第 4 次 render 即停,不跑满 12 轮(实际 {} 轮)", out.turns);
}

#[test]
fn degenerate_fingerprint_folds_whitespace() {
    // "近乎全等":仅排版/换行差异视为相同;内容不同则不同。
    let a = degenerate_fingerprint("想 A", "落子 (3,4)");
    let b = degenerate_fingerprint("想   A", "落子\n(3,4)  "); // 仅空白差异
    let c = degenerate_fingerprint("想 B", "落子 (5,5)");
    assert_eq!(a, b, "仅空白差异 = 近乎全等");
    assert_ne!(a, c, "内容不同 = 不同指纹");
}

// ===================== 并行子代理(调查员并发扇出)+ 禁 emoji =====================

#[test]
fn strip_emoji_removes_emoji_keeps_text_symbols() {
    // 去 emoji:补充平面(🕵 📋)+ BMP 常见表情(✅)+ 变体选择符(🕵️ 的 VS16)。
    assert_eq!(strip_emoji("查案🕵️完成✅"), "查案完成");
    assert_eq!(strip_emoji("报告📋:done"), "报告:done");
    // 保留项目用作文本符号的 ★(2605)/ →(2192)/ •(2022)/ ✓(2713,非 emoji)/ 中文。
    assert_eq!(strip_emoji("★先读 → 见 • 项 ✓ 中文"), "★先读 → 见 • 项 ✓ 中文");
}

/// 请求内容驱动的 mock LLM:并行调查员与主链**共用同一 llm**,并发下回合顺序不定,故不能用回合队列 —— 据
/// 请求消息内容判定该返回什么(调查员看到自己的 input"目标甲/乙"就 finish 给结论;主链看到两份结论就收口)。
struct ParallelMock;
#[async_trait::async_trait]
impl LlmDriver for ParallelMock {
    async fn chat_stream(&self, req: ChatRequest) -> LlmResult<mpsc::Receiver<LlmResult<StreamChunk>>> {
        let joined = req.messages.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
        let chunks = if joined.contains("目标甲") {
            answer_then_finish("ADONE") // 调查员甲:只读勘探完,finish 给结论
        } else if joined.contains("目标乙") {
            answer_then_finish("BDONE") // 调查员乙
        } else if joined.contains("ADONE") && joined.contains("BDONE") {
            answer_then_finish("汇总完成") // 主链:两份摘要都回灌到了 → 收口
        } else {
            // 主链第一轮:同一回合发出两个 isolated investigate 调用 → 触发并发批。
            vec![
                StreamChunk::ToolCallDelta {
                    index: 0,
                    id: Some("inv1".into()),
                    name: Some("investigate".into()),
                    args_fragment: r#"{"input":"目标甲","context_mode":"isolated"}"#.into(),
                },
                StreamChunk::ToolCallDelta {
                    index: 1,
                    id: Some("inv2".into()),
                    name: Some("investigate".into()),
                    args_fragment: r#"{"input":"目标乙","context_mode":"isolated"}"#.into(),
                },
                StreamChunk::Done { finish_reason: "tool_calls".into() },
            ]
        };
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(async move {
            for c in chunks {
                if tx.send(Ok(c)).await.is_err() {
                    return;
                }
            }
        });
        Ok(rx)
    }
}

/// ★并行子代理:一回合 2 个 isolated 调查员 → 并发跑、两份摘要都回灌 → 主链收口(设计/07-附录-并行子代理)★。
#[tokio::test]
async fn parallel_investigators_run_and_aggregate() {
    let dir = tempdir().unwrap();
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let out = agent_loop(
        "并行查两处", &cfg(), &ParallelMock, &reg, &sb, &mut mem, &LocalSub, &NullReasoner, &fw, dir.path(), &sink,
    )
    .await;

    assert_eq!(out.stopped, StopReason::Completed);
    assert_eq!(out.final_text, "汇总完成", "主链应在两份调查摘要回灌后收口");
    // 两个调查员都跑了、都回了摘要(并行批为每个 investigate 调用 emit 一条 ToolEnd)。
    let ends: Vec<String> = sink
        .kinds()
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolEnd { name, content, .. } if name == "investigate" => Some(content.clone()),
            _ => None,
        })
        .collect();
    assert!(ends.iter().any(|c| c == "ADONE"), "调查员甲应回摘要 ADONE: {ends:?}");
    assert!(ends.iter().any(|c| c == "BDONE"), "调查员乙应回摘要 BDONE: {ends:?}");
    // 并发批通知出现(用户可见的聚合状态,而非 N 路刷屏)。
    assert!(
        sink.kinds().iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("并发勘探"))),
        "应有并发勘探通知"
    );
}

/// ★parallel_max=1 退化为顺序、结果不丢★:并发上限设 1 时,两个调查员仍都跑完、两份摘要都回灌。
#[tokio::test]
async fn parallel_max_one_degrades_to_sequential_without_loss() {
    let dir = tempdir().unwrap();
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let mut c = cfg();
    c.parallel_max = 1; // 顺序跑,但两份都要回来
    let out = agent_loop(
        "并行查两处", &c, &ParallelMock, &reg, &sb, &mut mem, &LocalSub, &NullReasoner, &fw, dir.path(), &sink,
    )
    .await;
    assert_eq!(out.stopped, StopReason::Completed);
    assert_eq!(out.final_text, "汇总完成", "上限=1 也应在两份摘要回灌后收口(结果不丢)");
}

// ===================== 活的 IDE:ui_control 往返测试 =====================

/// 会回执的 sink:记录事件 + `ui_round_trip` 模拟前端落地(close → open=false)。
#[derive(Default)]
struct AckingCollector {
    events: Mutex<Vec<AgentEvent>>,
}
#[async_trait::async_trait]
impl EventSink for AckingCollector {
    async fn emit(&self, event: AgentEvent) {
        self.events.lock().push(event);
    }
    async fn ui_round_trip(&self, intent: &growbox_core::UiIntent) -> crate::ui::UiAck {
        let op = intent.prefill.get("op").and_then(|v| v.as_str()).unwrap_or("");
        crate::ui::UiAck { applied: true, state: serde_json::json!({ "open": op == "open" }), note: None }
    }
}

#[tokio::test]
async fn ui_control_round_trips_and_returns_verified_state() {
    use crate::ui::{empty_catalog, UiSurface};
    let dir = tempdir().unwrap();
    // 前端声明的目录里有 memory 面板(支持 open/close/toggle)。
    let catalog = empty_catalog();
    *catalog.write() = vec![UiSurface {
        id: "memory".into(),
        label: "记忆面板".into(),
        ops: vec!["open".into(), "close".into(), "toggle".into()],
    }];
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "ui_control", r#"{"target":"memory","op":"close"}"#),
        answer_then_finish("已经帮你关掉记忆面板"),
    ]);
    let reg = Registry::with_builtins_catalog(TaskManager::new(), catalog);
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = AckingCollector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("关掉记忆面板", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    // ui_control 往返成功:ToolEnd ok=true,且结果带验证态 open=false(不撒谎)。
    let events = sink.events.lock().clone();
    let end = events.iter().find_map(|e| match e {
        AgentEvent::ToolEnd { name, ok, content } if name == "ui_control" => Some((*ok, content.clone())),
        _ => None,
    });
    let (ok, content) = end.expect("应有 ui_control 的 ToolEnd");
    assert!(ok, "往返应成功");
    assert!(content.contains("false"), "结果应反映前端回报的验证态 open=false,实际: {content}");
}

#[tokio::test]
async fn ui_control_unapplied_ack_is_honest_failure() {
    // 默认 Collector 的 ui_round_trip 返回 unapplied → ui_control 应诚实失败(被 perceive)。
    let dir = tempdir().unwrap();
    use crate::ui::{empty_catalog, UiSurface};
    let catalog = empty_catalog();
    *catalog.write() = vec![UiSurface {
        id: "memory".into(),
        label: "记忆面板".into(),
        ops: vec!["close".into()],
    }];
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "ui_control", r#"{"target":"memory","op":"close"}"#),
        answer_then_finish("面板没关成"),
    ]);
    let reg = Registry::with_builtins_catalog(TaskManager::new(), catalog);
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default(); // 默认 ui_round_trip = unapplied
    let reasoner = NullReasoner;

    let out = agent_loop("关掉记忆面板", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    let events = sink.kinds();
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::ToolEnd { name, ok: false, .. } if name == "ui_control")),
        "未应用的 UI 操作应诚实标记失败"
    );
}

// ===================== 后台任务工具测试 =====================

#[tokio::test]
async fn spawn_task_runs_and_wait_completes() {
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        // 第1轮:spawn 一个后台任务
        tool_call_chunks("c1", "spawn_task", r#"{"command":"echo hello","label":"echo","done_when":"exit"}"#),
        // 第2轮:wait_tasks
        tool_call_chunks("c2", "wait_tasks", "{}"),
        // 第3轮:finish
        answer_then_finish("后台任务完成"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("跑个后台任务", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;

    assert_eq!(out.stopped, StopReason::Completed);
    // 工具执行了 3 次(spawn + wait + finish)。
    let events = sink.kinds();
    let tool_starts: Vec<_> = events.iter().filter(|e| matches!(e, AgentEvent::ToolStart { .. })).collect();
    assert_eq!(tool_starts.len(), 3);
}

#[tokio::test]
async fn spawn_task_dangerous_command_rejected() {
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        // 第1轮:尝试 spawn 危险命令
        tool_call_chunks("c1", "spawn_task", r#"{"command":"sudo rm -rf /","label":"危险","done_when":"exit"}"#),
        // 第2轮:finish
        answer_then_finish("危险命令被拒绝了"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("试试危险命令", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;

    assert_eq!(out.stopped, StopReason::Completed);
    // spawn_task 应该返回失败(被安全门拒绝)。
    let events = sink.kinds();
    let tool_ends: Vec<_> = events.iter().filter(|e| matches!(e, AgentEvent::ToolEnd { name, ok: false, .. } if name == "spawn_task")).collect();
    assert_eq!(tool_ends.len(), 1, "危险命令的 spawn_task 应返回失败");
}

#[tokio::test]
async fn list_tasks_shows_snapshot() {
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        // 第1轮:spawn 一个长任务 + list_tasks
        vec![
            StreamChunk::ToolCallDelta {
                index: 0, id: Some("c1".into()), name: Some("spawn_task".into()),
                args_fragment: r#"{"command":"sleep 30","label":"long","done_when":"exit"}"#.into(),
            },
            StreamChunk::ToolCallDelta {
                index: 1, id: Some("c2".into()), name: Some("list_tasks".into()),
                args_fragment: "{}".into(),
            },
            StreamChunk::Done { finish_reason: "tool_calls".into() },
        ],
        // 第2轮:finish
        answer_then_finish("任务列表中有一个在跑的任务"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("列任务", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;

    assert_eq!(out.stopped, StopReason::Completed);
    // list_tasks 应该返回含"运行中"的快照。
    let events = sink.kinds();
    let list_end = events.iter().find(|e| matches!(e, AgentEvent::ToolEnd { name, ok: true, .. } if name == "list_tasks"));
    assert!(list_end.is_some(), "list_tasks 应成功");
}

// ===================== 飞轮 idle 学习测试 =====================

/// 会提炼出模式的 Reasoner(验证飞轮 turn 被调用且产出知识)。
struct LearningReasoner;
#[async_trait::async_trait]
impl Reasoner for LearningReasoner {
    async fn distill(&self, cluster: &[growbox_core::Conclusion]) -> Option<growbox_learn::Distillation> {
        if cluster.len() >= 2 {
            Some(growbox_learn::Distillation {
                operation: "提炼的模式".into(),
                expected: "预期结果".into(),
                prerequisites: vec!["前提".into()],
            })
        } else {
            None
        }
    }
}

#[tokio::test]
async fn finalize_collects_but_does_not_compress() {
    // 新契约(见 finalize 文档 + idle.rs):前台回合只**采集**经验,**不压缩**。
    // 模型做两次相同操作(产生两条同类经验)然后 finish。
    let dir = tempdir().unwrap();
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "file_write", r#"{"path":"a.txt","content":"x"}"#),
        tool_call_chunks("c2", "file_write", r#"{"path":"b.txt","content":"y"}"#),
        answer_then_finish("两个文件都写好了"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = LearningReasoner;

    let out = agent_loop("写两个文件", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;

    assert_eq!(out.stopped, StopReason::Completed);
    // ① 经验已采集:存在活跃且未压缩(compression == 0)的经验,且 ≥2 条。
    let experiences: Vec<_> = mem.conclusions().iter().filter(|c| c.is_active() && c.compression == 0.0).collect();
    assert!(experiences.len() >= 2, "前台回合应采集到经验");
    // ② 但前台**没有**压缩(无压缩率 > 0 的知识)——压缩是 IdleWorker 的活。
    let knowledge_now = mem.conclusions().iter().filter(|c| c.is_active() && c.compression > 0.0).count();
    assert_eq!(knowledge_now, 0, "前台回合不应压缩经验(那是 idle 学习的活)");
    // ③ 也不再有"提炼了知识"的通知事件(finalize 只抛 Done)。
    assert!(!sink.kinds().iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("知识"))));

    // 模拟 IdleWorker 的消化路径(active_experiences → clusters_of → distill_cluster → apply_distilled),
    // 验证这批采集到的经验确实能在 idle 时被压成知识。
    let snapshot = Flywheel::active_experiences(&mem);
    for members in fw.clusters_of(&snapshot) {
        if let Some((knowledge, superseded)) = fw.distill_cluster(&members, &reasoner).await {
            Flywheel::apply_distilled(&mut mem, knowledge, &superseded);
        }
    }
    let knowledge_after: Vec<_> = mem.conclusions().iter().filter(|c| c.is_active() && c.compression > 0.0).collect();
    assert!(!knowledge_after.is_empty(), "idle 消化路径应把同类经验压成知识");
}

/// ★工作流机制端到端(P1)★:define_workflow 注册 → 调同名工具进入 → 节点内工具收窄(物理锁死)
/// → 调工具触发强制流转 → Always 流转到 END 退出工作流 → 回普通模式 → finish 收尾。
/// 覆盖 设计/07 原则1(强制顺序)、原则2(工作流即动态工具)、推论1(结构性降错)、推论6(融入脊柱)。
#[tokio::test]
async fn workflow_enters_narrows_transitions_and_exits() {
    let dir = tempdir().unwrap();
    // LLM 友好定义:write 节点只许 file_write,调了它流转到 done;done 节点 Always 流转到 END(退出)。
    let define_args = r#"{"name":"myflow","description":"测试工作流","entry":"write","nodes":[{"id":"write","prompt":"第一步:写文件","tools":["file_write"],"next":[{"to":"done","on_tool":"file_write"}]},{"id":"done","prompt":"第二步:再写一个就结束","tools":["file_write"],"next":[{"to":"END"}]}]}"#;
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "define_workflow", define_args), // 轮0:注册工作流
        tool_call_chunks("c2", "myflow", "{}"),                  // 轮1:调同名工具 → 进入(节点 write)
        tool_call_chunks("c3", "file_write", r#"{"path":"a.txt","content":"x"}"#), // 轮2:write 节点内 → 流转 done
        tool_call_chunks("c4", "file_write", r#"{"path":"b.txt","content":"y"}"#), // 轮3:done 节点内 → Always 到 END
        answer_then_finish("都写好了"),                          // 轮4:已回普通模式 → finish
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("帮我用工作流写两个文件", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;

    assert_eq!(out.stopped, StopReason::Completed);
    assert_eq!(out.turns, 5);
    // 工具真执行(两步都落盘)。
    assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "x");
    assert_eq!(std::fs::read_to_string(dir.path().join("b.txt")).unwrap(), "y");

    // 轮0(普通模式):见 define_workflow 与全集。
    let t0 = sink_tools(&llm, 0);
    assert!(t0.contains(&"define_workflow".to_string()));
    assert!(t0.contains(&"shell".to_string()));
    // 轮1(普通模式):刚注册的 myflow 已作为动态工具出现(工作流即动态工具)。
    assert!(sink_tools(&llm, 1).contains(&"myflow".to_string()), "进入前 myflow 应可调");

    // 轮2(在 write 节点):工具**物理收窄**到 file_write + 兜底 finish/ask_user;选不到 shell/其他工作流。
    let t2 = sink_tools(&llm, 2);
    assert!(t2.contains(&"file_write".to_string()));
    assert!(t2.contains(&"finish".to_string()) && t2.contains(&"ask_user".to_string()), "兜底工具应保留");
    assert!(!t2.contains(&"shell".to_string()), "节点外工具应被锁死(推论1)");
    assert!(!t2.contains(&"myflow".to_string()) && !t2.contains(&"create_artifact_workflow".to_string()));

    // 轮3(在 done 节点):仍收窄。
    let t3 = sink_tools(&llm, 3);
    assert!(t3.contains(&"file_write".to_string()));
    assert!(!t3.contains(&"shell".to_string()));

    // 轮4(Always→END 退出后,回到普通模式):全集恢复。
    let t4 = sink_tools(&llm, 4);
    assert!(t4.contains(&"shell".to_string()), "退出工作流应回普通模式(全工具)");

    // 退出工作流发了完成通知。
    assert!(sink.kinds().iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("myflow") && n.contains("完成"))));
}

fn sink_tools(llm: &Scripted, n: usize) -> Vec<String> {
    llm.tools_at(n)
}

/// ★工作流嵌套(P3 推论4)★:父工作流节点的工具子集里含子工作流工具 → 调它压栈进入子工作流;
/// 子工作流 END 出栈 → 回父节点,父据"调过该子工作流"续流转。验证栈式收窄 + 嵌套返回级联。
#[tokio::test]
async fn nested_workflow_pushes_and_returns_to_parent() {
    let dir = tempdir().unwrap();
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let parent = r#"{"name":"parent_workflow","description":"父","entry":"p1","nodes":[{"id":"p1","prompt":"调子工作流","tools":["child_workflow"],"next":[{"to":"p2","on_tool":"child_workflow"}]},{"id":"p2","prompt":"收尾","tools":["finish"]}]}"#;
    let child = r#"{"name":"child_workflow","description":"子","entry":"c1","nodes":[{"id":"c1","prompt":"写文件","tools":["file_write"],"next":[{"to":"END","on_tool":"file_write"}]}]}"#;
    for (i, def) in [parent, child].iter().enumerate() {
        let c = ToolCall { id: format!("d{i}"), name: "define_workflow".into(), arguments: (*def).into() };
        assert!(matches!(reg.dispatch(&c, &sb, dir.path()).await, Dispatch::Done(r) if r.ok));
    }
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "parent_workflow", "{}"),                                  // 进父(push p1)
        tool_call_chunks("c2", "child_workflow", "{}"),                                   // 嵌套进子(push c1)
        tool_call_chunks("c3", "file_write", r#"{"path":"x.txt","content":"y"}"#),        // c1→END 出栈,父 p1→p2
        answer_then_finish("done"),                                                       // p2 finish
    ]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("嵌套", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    assert_eq!(std::fs::read_to_string(dir.path().join("x.txt")).unwrap(), "y");

    // 轮1(父 p1 节点):收窄到 child_workflow + 兜底;无 file_write/shell。
    let t1 = sink_tools(&llm, 1);
    assert!(t1.contains(&"child_workflow".to_string()));
    assert!(!t1.contains(&"file_write".to_string()) && !t1.contains(&"shell".to_string()));
    // 轮2(子 c1 节点):收窄到 file_write + 兜底;无 child_workflow(父工具不串)。
    let t2 = sink_tools(&llm, 2);
    assert!(t2.contains(&"file_write".to_string()));
    assert!(!t2.contains(&"child_workflow".to_string()) && !t2.contains(&"shell".to_string()));
    // 轮3(嵌套返回父 p2 节点):收窄到 finish + 兜底;不再有 file_write/child_workflow。
    let t3 = sink_tools(&llm, 3);
    assert!(t3.contains(&"finish".to_string()));
    assert!(!t3.contains(&"file_write".to_string()) && !t3.contains(&"child_workflow".to_string()) && !t3.contains(&"shell".to_string()));
    // 子工作流完成通知。
    assert!(sink.kinds().iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("child_workflow") && n.contains("完成"))));
}

/// 在第 N 次取消检查后报告"已取消"的 sink(模拟用户中途按「终止」)。
#[derive(Default)]
struct CancelSink {
    events: Mutex<Vec<AgentEvent>>,
}
#[async_trait::async_trait]
impl EventSink for CancelSink {
    async fn emit(&self, event: AgentEvent) {
        self.events.lock().push(event);
    }
    // 真实模型:`is_cancelled` 读一个一经置位就恒真的标志(实际是 ChatControl 的 AtomicBool,多查无副作用)。
    // 这里用"已出现过 ToolEnd"模拟"用户看到第 1 个工具结果后按下终止"——一旦第 1 轮工具执行完即视为已取消,
    // 与脊柱"每轮顶 + drive_one 流中 + drive 后"多检查点兼容(多查同值,不依赖轮询计数)。
    fn is_cancelled(&self) -> bool {
        self.events.lock().iter().any(|e| matches!(e, AgentEvent::ToolEnd { .. }))
    }
}

/// ★终止机制(造物交互 v2 §2)★:用户中途按「终止」→ 脊柱下一检查点优雅收口 StopReason::Cancelled,
/// 已执行的工具保留、抛 Done 停转,不再调 LLM。
#[tokio::test]
async fn cancel_stops_loop_at_next_checkpoint() {
    let dir = tempdir().unwrap();
    // 脚本给足两轮;但第2轮检查点会取消,第2次 LLM 调用不应发生。
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "file_write", r#"{"path":"a.txt","content":"x"}"#),
        answer_then_finish("不该到这"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = CancelSink::default();
    let reasoner = NullReasoner;

    let out = agent_loop("写个文件", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;

    assert_eq!(out.stopped, StopReason::Cancelled);
    assert_eq!(out.turns, 1, "第1轮执行了,第2轮检查点取消");
    // 第1轮的工具真执行了(取消不回滚已做的)。
    assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "x");
    // 第2次 LLM 调用没发生(脚本里第二组 chunk 没被消费)——turn2 的回答"不该到这"不应出现。
    assert!(
        !sink.events.lock().iter().any(|e| matches!(e, AgentEvent::Content(c) if c.contains("不该到这"))),
        "取消后不应再调 LLM"
    );
    // 收尾抛了 Done。
    assert!(matches!(sink.events.lock().last(), Some(AgentEvent::Done)));
}

/// ★造物文件夹隔离★:写造物自己文件夹(.growbox/artifacts/...)不进主记忆(不采集经验/不入时间线);
/// 写项目普通文件照常进。验证 is_internal_state_file_op 在脊柱里生效。
#[tokio::test]
async fn artifact_folder_writes_skip_main_memory() {
    let tmp = tempdir().unwrap();
    // canonicalize:macOS tempdir 在符号链接 /var→/private/var 下,沙箱可写根会被规范化,
    // 而尚不存在的深层目标无法规范化前缀 → 前缀判失败。真实项目路径非符号链接,无此问题。
    let root = tmp.path().canonicalize().unwrap();
    let llm = Scripted::new(vec![
        // 轮1:把棋盘状态写进造物文件夹(隔离,不进主记忆)。
        tool_call_chunks("c1", "file_write", r#"{"path":".growbox/artifacts/gomoku/board.json","content":"{\"moves\":[]}"}"#),
        // 轮2:写一个项目普通文件(照常进主记忆)。
        tool_call_chunks("c2", "file_write", r#"{"path":"notes.txt","content":"hi"}"#),
        answer_then_finish("好了"),
    ]);
    let reg = Registry::with_builtins(TaskManager::new());
    // 造物文件夹在 work_dir 下,本就可写(work_dir 是可写根)。
    let sb = Sandbox::new(vec![root.clone()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("下棋", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, &root, &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    // 两个文件都真写了。
    assert!(root.join(".growbox/artifacts/gomoku/board.json").exists());
    assert_eq!(std::fs::read_to_string(root.join("notes.txt")).unwrap(), "hi");
    // 但只采集到 1 条经验(notes.txt);造物文件夹那次被隔离。
    let exps = mem.conclusions().iter().filter(|c| c.is_active()).count();
    assert_eq!(exps, 1, "造物文件夹写入不应进主记忆,只 notes.txt 进");
}

/// ★工作流 P2 端口触发端到端★:造物作用域工作流绑定画布 + trigger 端口;
/// 模拟"用户落子"(端口 place)→ resolve_trigger 命中 → 从 ai_move 节点起手(工具收窄到仅 artifact_command,
/// 物理无 push_artifact_notice/render_artifact → 不会再发通知卡住/反复重画失控)→ 调 artifact_command 强制流转判胜负 → finish。
/// 覆盖 设计/07 推论3(端口入口)+ 案例(五子棋对弈工作流)+ "跨 run 续跑靠端口恢复节点"。
#[tokio::test]
async fn workflow_port_trigger_enters_narrowed_gomoku_node() {
    let dir = tempdir().unwrap();
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);

    // AI 创建五子棋造物时定义并注册的对弈工作流(造物作用域,绑定画布 gomoku,端口 place→ai_move)。
    let define = r#"{"name":"gomoku_play","description":"五子棋对弈","scope":"artifact","canvas":"gomoku","entry":"ai_move","triggers":[{"port":"place","to":"ai_move"}],"nodes":[{"id":"ai_move","prompt":"轮到你(白),据棋盘分析下最佳一手,用 artifact_command 落子","tools":["artifact_command"],"next":[{"to":"judge","on_tool":"artifact_command"}]},{"id":"judge","prompt":"判断是否五连,有则宣布胜负后 finish,否则 finish 等用户下一手","tools":["artifact_command"],"next":[]}]}"#;
    let dc = ToolCall { id: "d".into(), name: "define_workflow".into(), arguments: define.into() };
    assert!(matches!(reg.dispatch(&dc, &sb, dir.path()).await, Dispatch::Done(r) if r.ok));

    // 模拟 cmds 在"用户落子"时做的端口触发解析(端口=回调 place)。
    let initial = reg.resolve_trigger("gomoku", "place").expect("落子端口应命中 gomoku 工作流");
    assert_eq!(initial, ("gomoku_play".to_string(), "ai_move".to_string()));

    // 落子回合:ai_move 节点调 artifact_command(→流转 judge),judge 节点 finish 收口。
    let llm = Scripted::new(vec![
        tool_call_chunks("c1", "artifact_command", r#"{"canvas_id":"gomoku","command":"place_white_7_7"}"#),
        answer_then_finish("白棋落 (7,7)"),
    ]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    // 端口触发种入(initial_wf):本回合从 ai_move 节点起手。中性 seed(只陈述事实)。
    let out = agent_loop_internal(
        "[造物交互] 用户在造物「gomoku」中操作:place = (7,8)",
        Some(initial), &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink, false, false,
    ).await;

    assert_eq!(out.stopped, StopReason::Completed);
    // 起手即在 ai_move 节点:工具物理收窄到 artifact_command + 兜底 finish/ask_user。
    let t0 = llm.tools_at(0);
    assert!(t0.contains(&"artifact_command".to_string()), "落子节点应有 artifact_command");
    assert!(t0.contains(&"finish".to_string()) && t0.contains(&"ask_user".to_string()), "兜底工具保留");
    // ★07 案例的杀手级保证★:落子节点没有 notice/render → AI 想发通知/重画都选不到。
    assert!(!t0.contains(&"push_artifact_notice".to_string()), "落子节点物理无 notice(不会再发通知卡住)");
    assert!(!t0.contains(&"render_artifact".to_string()), "落子节点物理无 render(不会反复重画失控)");
    // 调了 artifact_command → 强制流转到 judge 节点(仍收窄)。
    let t1 = llm.tools_at(1);
    assert!(!t1.contains(&"render_artifact".to_string()) && !t1.contains(&"shell".to_string()), "judge 节点仍收窄");
}

// ───────────────────────── 栈函数工作流 v2(设计/07 加强版)─────────────────────────

/// 已注册两个工作流到 reg(供 v2 测试复用):用 define_workflow 真分发注册。
async fn define_two(reg: &Registry, sb: &Sandbox, dir: &std::path::Path, defs: &[&str]) {
    for (i, def) in defs.iter().enumerate() {
        let c = ToolCall { id: format!("d{i}"), name: "define_workflow".into(), arguments: (*def).into() };
        assert!(matches!(reg.dispatch(&c, sb, dir).await, Dispatch::Done(r) if r.ok), "define_workflow 应成功: {def}");
    }
}

/// ★v2 原则4:栈调用 + workflow_return 把结构化返回值回灌给调用方★。
/// 父栈调用子(带 return_spec)→ 子 workflow_return{value} → 出栈回灌 → 父读到返回值续流转 → finish。
#[tokio::test]
async fn workflow_stack_call_returns_value_to_caller() {
    let dir = tempdir().unwrap();
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let parent = r#"{"name":"par_wf","description":"父","entry":"p1","nodes":[{"id":"p1","prompt":"调子函数","tools":["calc_wf"],"next":[{"to":"p2","on_tool":"calc_wf"}]},{"id":"p2","prompt":"读返回值收尾","tools":["finish"]}]}"#;
    let child = r#"{"name":"calc_wf","description":"子函数:算个东西返回","entry":"c1","nodes":[{"id":"c1","prompt":"算完用 workflow_return 返回","tools":[]}]}"#;
    define_two(&reg, &sb, dir.path(), &[parent, child]).await;

    let llm = Scripted::new(vec![
        tool_call_chunks("a", "par_wf", "{}"),                                                      // T0: 进父 p1
        tool_call_chunks("b", "calc_wf", r#"{"input":"算 6*7","return_spec":"返回 {answer}"}"#),     // T1: 栈调用子
        tool_call_chunks("c", "workflow_return", r#"{"value":"CHILD_RESULT_42"}"#),                  // T2: 子返回
        answer_then_finish("已收到结果"),                                                            // T3: 父 p2 finish
    ]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("用工作流算一下", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    // 子函数的开场带上了调用方 input + return_spec(T1 进子后,T2 是子的第一轮)。
    let child_view = llm.messages_text_at(2);
    assert!(child_view.contains("算 6*7"), "子应看到调用方 input");
    assert!(child_view.contains("返回 {answer}"), "子应看到调用方 return_spec");
    // 父在子返回后(T3)读到返回值(回灌进父上下文)。
    let parent_view = llm.messages_text_at(3);
    assert!(parent_view.contains("CHILD_RESULT_42"), "父应读到子的结构化返回值");
    assert!(sink.kinds().iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("calc_wf") && n.contains("返回"))));
}

/// ★v2 原则6:直接调用 = 尾调用,替换栈顶不压栈★。
/// 主栈调用 A;A 内**直接调用** B(替换 A);B 到 END 出栈 → 回**主**(普通模式),而非回 A。
/// 若是栈调用,B END 会回到 A(收窄到 A 的工具)——以"退出后工具是否回全集"区分。
#[tokio::test]
async fn workflow_direct_call_is_tail_returns_past_replaced_frame() {
    let dir = tempdir().unwrap();
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let a = r#"{"name":"a_wf","description":"A","entry":"a1","nodes":[{"id":"a1","prompt":"尾调用 B","tools":["b_wf"],"next":[]}]}"#;
    let b = r#"{"name":"b_wf","description":"B","entry":"b1","nodes":[{"id":"b1","prompt":"写文件后结束","tools":["file_write"],"next":[{"to":"END","on_tool":"file_write"}]}]}"#;
    define_two(&reg, &sb, dir.path(), &[a, b]).await;

    let llm = Scripted::new(vec![
        tool_call_chunks("a", "a_wf", "{}"),                                          // T0: 栈调用进 A a1
        tool_call_chunks("b", "b_wf", r#"{"direct":true}"#),                          // T1: 直接调用 B(替换 A)
        tool_call_chunks("c", "file_write", r#"{"path":"d.txt","content":"z"}"#),     // T2: B b1 → END 出栈 → 回主
        answer_then_finish("done"),                                                   // T3: 主普通模式 finish
    ]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("尾调用测试", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    // T2 在 B b1:收窄到 file_write(无 a_wf/shell)。
    let t2 = llm.tools_at(2);
    assert!(t2.contains(&"file_write".to_string()) && !t2.contains(&"shell".to_string()));
    // ★关键★:B END 后回到**主**(普通模式,全集含 shell),证明 A 被尾调用替换(否则会回 A、无 shell)。
    let t3 = llm.tools_at(3);
    assert!(t3.contains(&"shell".to_string()), "直接调用替换了 A → B 返回到主链(全集),而非回 A");
}

/// ★v2 原则3+最少充分信息:isolated 裁剪上下文(隐藏父)+ 退出截断丢弃分支噪音★。
#[tokio::test]
async fn workflow_isolated_hides_parent_and_discards_on_return() {
    let dir = tempdir().unwrap();
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let child = r#"{"name":"iso_wf","description":"隔离子","entry":"c1","nodes":[{"id":"c1","prompt":"干活后结束","tools":["file_write"],"next":[{"to":"END","on_tool":"file_write"}]}]}"#;
    define_two(&reg, &sb, dir.path(), &[child]).await;

    let llm = Scripted::new(vec![
        tool_call_chunks("a", "iso_wf", r#"{"context_mode":"isolated","input":"CHILDTOKEN 处理这个"}"#), // T0
        tool_call_chunks("b", "file_write", r#"{"path":"w.txt","content":"x"}"#),                          // T1: 子干活→END
        answer_then_finish("好了"),                                                                        // T2: 主 finish
    ]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("处理任务 PARENTTOKEN", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    // T1(子的第一轮,isolated):看得到自己的 input,看不到父对话。
    let child_view = llm.messages_text_at(1);
    assert!(child_view.contains("CHILDTOKEN"), "isolated 子应看到自己的 input");
    assert!(!child_view.contains("PARENTTOKEN"), "isolated 子不应看到父对话(裁剪上下文)");
    // T2(子返回后,父视角):父上下文恢复;子的工作消息(开场 input 等)被截断丢弃。
    let parent_view = llm.messages_text_at(2);
    assert!(parent_view.contains("PARENTTOKEN"), "返回后父上下文应恢复");
    assert!(!parent_view.contains("CHILDTOKEN"), "isolated 退出即丢弃分支噪音(开场 input 被截断)");
}

/// ★fork 模式(子 Agent 三旋钮):继承父全量上下文(看得到父=与 isolated 的关键区别)+ 退出截断丢弃分支噪音(只回摘要)★。
#[tokio::test]
async fn workflow_fork_inherits_parent_but_discards_on_return() {
    let dir = tempdir().unwrap();
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let child = r#"{"name":"fork_wf","description":"fork子","entry":"c1","nodes":[{"id":"c1","prompt":"干活后结束","tools":["file_write"],"next":[{"to":"END","on_tool":"file_write"}]}]}"#;
    define_two(&reg, &sb, dir.path(), &[child]).await;

    let llm = Scripted::new(vec![
        tool_call_chunks("a", "fork_wf", r#"{"context_mode":"fork","input":"CHILDTOKEN 处理这个"}"#), // T0
        tool_call_chunks("b", "file_write", r#"{"path":"w.txt","content":"x"}"#),                       // T1: 子干活→END
        answer_then_finish("好了"),                                                                     // T2: 主 finish
    ]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("处理任务 PARENTTOKEN", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    // T1(子第一轮,fork):既看得到自己的 input,也看得到父对话(继承全量 = 与 isolated 的关键区别)。
    let child_view = llm.messages_text_at(1);
    assert!(child_view.contains("CHILDTOKEN"), "fork 子应看到自己的 input");
    assert!(child_view.contains("PARENTTOKEN"), "fork 子应继承父全量上下文(看得到父,这是与 isolated 的关键区别)");
    // T2(子返回后,父视角):父上下文仍在;子的工作消息(开场 input 等)被截断丢弃(只回摘要,不污染父)。
    let parent_view = llm.messages_text_at(2);
    assert!(parent_view.contains("PARENTTOKEN"), "返回后父上下文仍在");
    assert!(!parent_view.contains("CHILDTOKEN"), "fork 退出截断分支噪音(开场 input 被丢弃,只回摘要)");
}

/// ★v2 原则4:full=true 全量返回,不截断,分支原始工作上下文直通回父(零 LLM 搬运)★。
#[tokio::test]
async fn workflow_return_full_passthrough_does_not_truncate() {
    let dir = tempdir().unwrap();
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let parent = r#"{"name":"pf_wf","description":"父","entry":"p1","nodes":[{"id":"p1","prompt":"调子","tools":["raw_wf"],"next":[{"to":"p2","on_tool":"raw_wf"}]},{"id":"p2","prompt":"收尾","tools":["finish"]}]}"#;
    // 子 isolated;干活产出含 RAWOUTPUT 的助手正文,然后 full 返回 → 不截断,父能看到原始内容。
    let child = r#"{"name":"raw_wf","description":"子","entry":"c1","nodes":[{"id":"c1","prompt":"产出后全量返回","tools":[]}]}"#;
    define_two(&reg, &sb, dir.path(), &[parent, child]).await;

    let llm = Scripted::new(vec![
        tool_call_chunks("a", "pf_wf", "{}"),                                                  // T0: 进父
        tool_call_chunks("b", "raw_wf", r#"{"context_mode":"isolated","input":"go"}"#),        // T1: 栈调用隔离子
        // T2: 子产出一段含 RAWOUTPUT 的正文(助手 content),不调工具(免死继续)。
        vec![StreamChunk::Content("这是子的原始输出 RAWOUTPUT_BLOB 很长".into()), StreamChunk::Done { finish_reason: "stop".into() }],
        tool_call_chunks("c", "workflow_return", r#"{"value":"见上","full":true}"#),            // T3: 全量返回
        answer_then_finish("收到全量"),                                                          // T4: 父 finish
    ]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("全量返回测试", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    // 父收尾(T4)能看到子的原始产出(full=true 不截断,直通)。
    let parent_view = llm.messages_text_at(4);
    assert!(parent_view.contains("RAWOUTPUT_BLOB"), "full=true 应把分支原始工作内容直通回父(不截断)");
}

/// ★v2 原则8:分支内直接调用循环,超 max_loops → 强制返回入口栈★。
#[tokio::test]
async fn workflow_max_loops_forces_return() {
    let dir = tempdir().unwrap();
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let a = r#"{"name":"loop_wf","description":"自循环","entry":"a1","nodes":[{"id":"a1","prompt":"直接调用自己循环","tools":["loop_wf"],"next":[]}]}"#;
    define_two(&reg, &sb, dir.path(), &[a]).await;

    let llm = Scripted::new(vec![
        tool_call_chunks("a", "loop_wf", r#"{"max_loops":1}"#),     // T0: 栈调用进入,设上限 1
        tool_call_chunks("b", "loop_wf", r#"{"direct":true}"#),     // T1: 第 1 次直接调用循环(loops_used 0→1,放行)
        tool_call_chunks("c", "loop_wf", r#"{"direct":true}"#),     // T2: 第 2 次 → 超上限 → 强制返回入口栈
        answer_then_finish("循环结束"),                              // T3: 主普通模式 finish
    ]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("循环上限测试", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    // 第 2 次直接调用被拦 → 强制返回通知。
    assert!(
        sink.kinds().iter().any(|e| matches!(e, AgentEvent::Notice(n) if n.contains("loop_wf") && n.contains("最大循环次数"))),
        "超 max_loops 应强制返回并通知"
    );
    // 强制返回后回到主链(普通模式,全集含 shell)。
    let t3 = llm.tools_at(3);
    assert!(t3.contains(&"shell".to_string()), "强制返回后回主链全集");
}

/// ★v2 原则9:分支链不与用户对话 —— 分支内的思考/正文不向用户展示,只主链展示★。
#[tokio::test]
async fn branch_content_is_not_shown_to_user() {
    let dir = tempdir().unwrap();
    let reg = Registry::with_builtins(TaskManager::new());
    let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let child = r#"{"name":"silent_wf","description":"分支","entry":"c1","nodes":[{"id":"c1","prompt":"干活后返回","tools":[]}]}"#;
    define_two(&reg, &sb, dir.path(), &[child]).await;

    let llm = Scripted::new(vec![
        tool_call_chunks("a", "silent_wf", "{}"),                                                  // T0: 栈调用进分支
        // T1: 分支内"说话"(Content)+ 返回 —— 这段不应展示给用户。
        vec![
            StreamChunk::Content("BRANCH_SECRET_TALK 我在分支里碎碎念".into()),
            StreamChunk::ToolCallDelta { index: 0, id: Some("r".into()), name: Some("workflow_return".into()), args_fragment: r#"{"value":"ok"}"#.into() },
            StreamChunk::Done { finish_reason: "tool_calls".into() },
        ],
        // T2: 回到主链,正常对用户说话 —— 这段应展示。
        vec![
            StreamChunk::Content("MAIN_VISIBLE_TALK 结果如下".into()),
            StreamChunk::ToolCallDelta { index: 0, id: Some("fin".into()), name: Some("finish".into()), args_fragment: "{}".into() },
            StreamChunk::Done { finish_reason: "tool_calls".into() },
        ],
    ]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::default();
    let reasoner = NullReasoner;

    let out = agent_loop("跑个分支", &cfg(), &llm, &reg, &sb, &mut mem, &LocalSub, &reasoner, &fw, dir.path(), &sink).await;
    assert_eq!(out.stopped, StopReason::Completed);
    assert!(!sink.has_content("BRANCH_SECRET_TALK"), "分支内的正文不应展示给用户(分支不与用户对话)");
    assert!(sink.has_content("MAIN_VISIBLE_TALK"), "主链的正文应正常展示给用户");
}
