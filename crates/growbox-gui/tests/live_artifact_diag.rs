//! 诊断 —— 真机抓"陪我下五子棋"的完整后端事件序列(定位 Bug B:思考中卡住/工具不显示)。
//!
//! 记录每个 AgentEvent 的 (相对耗时ms, 摘要),看多轮结构、reasoning 时长、render_artifact 的 args。
//! 用会 ack 的 sink 模拟前端渲染造物成功。默认 #[ignore]。显式跑:
//!   DEEPSEEK_API_KEY=<key> cargo test -p growbox-gui --test live_artifact_diag -- --ignored --nocapture

use std::sync::Arc;
use std::time::Instant;

use growbox_core::UiIntent;
use growbox_gui::agent::{agent_loop, AgentConfig, AgentEvent, EventSink, StopReason};
use growbox_gui::bridge::{LlmBridge, LlmDriver};
use growbox_gui::registry::Registry;
use growbox_gui::tasks::TaskManager;
use growbox_gui::ui::UiAck;
use growbox_learn::Flywheel;
use growbox_llm::LlmClient;
use growbox_memory::Memory;
use growbox_safety::Sandbox;
use parking_lot::Mutex;
use tempfile::tempdir;

/// 记录 (elapsed_ms, 一行摘要),并对 render_artifact 往返自动 ack(模拟前端渲染成功)。
struct DiagSink {
    t0: Instant,
    log: Mutex<Vec<String>>,
    /// 每个事件类型的累计 reasoning/content 字符数(看是否真在产出)。
    reasoning_chars: Mutex<usize>,
}

impl DiagSink {
    fn push(&self, s: String) {
        let ms = self.t0.elapsed().as_millis();
        self.log.lock().push(format!("[{ms:>6}ms] {s}"));
    }
}

#[async_trait::async_trait]
impl EventSink for DiagSink {
    async fn emit(&self, ev: AgentEvent) {
        match &ev {
            AgentEvent::Reasoning(s) => {
                *self.reasoning_chars.lock() += s.chars().count();
            }
            AgentEvent::Content(s) => self.push(format!("Content({} 字): {}", s.chars().count(), s.chars().take(40).collect::<String>())),
            AgentEvent::ToolStart { name, args } => {
                let n: usize = *self.reasoning_chars.lock();
                self.push(format!("[本轮累计 reasoning {n} 字] ToolStart {name} args={}", args.chars().take(80).collect::<String>()));
                *self.reasoning_chars.lock() = 0;
            }
            AgentEvent::ToolEnd { name, ok, content } => self.push(format!("ToolEnd {name} ok={ok}: {}", content.chars().take(60).collect::<String>())),
            AgentEvent::Notice(s) => self.push(format!("Notice: {s}")),
            AgentEvent::Intent(i) => self.push(format!("Intent: {}", i.action)),
            AgentEvent::Status(s) => self.push(format!("Status: {s}")),
            AgentEvent::Done => self.push("Done".into()),
        }
    }

    async fn ui_round_trip(&self, intent: &UiIntent) -> UiAck {
        let canvas = intent.prefill.get("canvas_id").and_then(|v| v.as_str()).unwrap_or("?");
        let html = intent.prefill.get("html").and_then(|v| v.as_str()).unwrap_or("");
        let html_len = html.len();
        // ★dump 生成的完整 HTML 供逐行查(诊断"棋盘坏掉")★:写进 /tmp,按已有日志条数命名以区分多次 render。
        let path = std::env::temp_dir().join(format!("gx_artifact_{canvas}_{}.html", self.log.lock().len()));
        let _ = std::fs::write(&path, html);
        self.push(format!("ui_round_trip {} canvas={canvas} html_len={html_len} → 已 dump {} → ack applied", intent.action, path.display()));
        // 模拟前端渲染造物成功。
        UiAck { applied: true, state: serde_json::json!({ "rendered": true, "canvas_id": canvas }), note: None }
    }
}

#[tokio::test]
#[ignore = "打真 API,需 DEEPSEEK_API_KEY"]
async fn diag_gomoku_event_sequence() {
    let key = std::env::var("DEEPSEEK_API_KEY").expect("需要环境变量 DEEPSEEK_API_KEY");
    let dir = tempdir().unwrap();

    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", key));
    let bridge = LlmBridge::new(driver.clone(), "deepseek-v4-flash", 8192, Arc::new(growbox_llm::LexicalEmbedder), 60);

    let registry = Registry::with_builtins(TaskManager::new());
    let sandbox = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut memory = Memory::new();
    let flywheel = Flywheel::new();
    let sink = DiagSink { t0: Instant::now(), log: Mutex::new(Vec::new()), reasoning_chars: Mutex::new(0) };

    // 真实系统提示词(system.zh.md,含造物灵魂/DOM 优先/单页布局),贴近真实 app。
    let prompt_path = format!("{}/../../prompts/agent/system.zh.md", env!("CARGO_MANIFEST_DIR"));
    let system_prompt = std::fs::read_to_string(&prompt_path).unwrap_or_else(|_| "你是 GrowBox。".into());
    let cfg = AgentConfig {
        model: "deepseek-v4-flash".into(),
        max_tokens: 0, // 贴近真实 app(不限,模型自然停)
        max_turns: 8,
        parallel_max: 4,
        system_prompt,
        prompt_lang: "zh".into(),
        auto_mode: false,
        danger_mode: false,
        privacy_dirs: vec![],
        max_token_retries: 2,
        token_ceil: 32_768,
        silence_secs: 90, // 真实 app 默认,测最严格情况
        max_stall: 2,
        // 默认 high(真实默认);设 GX_EFFORT=max 跑对比(定位 252s 是否=max)。
        reasoning_effort: std::env::var("GX_EFFORT").unwrap_or_else(|_| "high".into()),
        branch_log_max_gb: -1.0,
        self_verify: false,
        self_verify_min_tools: 3,
        recall_in_loop: false,
        tool_memory_enabled: false,
        tool_memory_veto_threshold: 0.85,
        tool_memory_warn_threshold: 0.80,
    };

    let msg = "陪我下五子棋,先把棋盘画出来。";

    println!("\n========== 五子棋诊断开始(reasoning_effort=max)==========\n");
    let out = agent_loop(
        msg, &cfg, driver.as_ref(), &registry, &sandbox, &mut memory, &bridge, &bridge, &flywheel, dir.path(), &sink,
    )
    .await;
    println!("\n---------- 事件序列 ----------");
    for line in sink.log.lock().iter() {
        println!("{line}");
    }
    println!("\n========== 结束: stop={:?} turns={} ==========", out.stopped, out.turns);

    // 不强断言行为(诊断用),只确保跑通。
    assert!(matches!(out.stopped, StopReason::Completed | StopReason::MaxTurns));
}

/// ★终止响应实测探针(定位"终止按钮没效果")★:真流式让模型长思考,模拟"开始思考就点终止"
/// (sink 在收到 N 个 reasoning chunk 后 is_cancelled 恒真),量从"置取消"到"循环抛 Done"的延迟。
/// 延迟小=后端中途打断生效(问题在前端/双重上报/begin);延迟到思考结束=drive_one 中断没生效。
///   DEEPSEEK_API_KEY=<key> cargo test -p growbox-gui --test live_artifact_diag diag_cancel_latency -- --ignored --nocapture
#[tokio::test]
#[ignore = "打真 API,需 DEEPSEEK_API_KEY"]
async fn diag_cancel_latency() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let key = std::env::var("DEEPSEEK_API_KEY").expect("需要环境变量 DEEPSEEK_API_KEY");
    let dir = tempdir().unwrap();

    struct CancelLatencySink {
        t0: Instant,
        reasoning_seen: AtomicUsize,
        cancel_after: usize,
        cancel_set_at: Mutex<Option<u128>>,
        done_at: Mutex<Option<u128>>,
    }
    #[async_trait::async_trait]
    impl EventSink for CancelLatencySink {
        async fn emit(&self, ev: AgentEvent) {
            if let AgentEvent::Reasoning(_) = ev {
                self.reasoning_seen.fetch_add(1, Ordering::SeqCst);
            }
            if let AgentEvent::Done = ev {
                *self.done_at.lock() = Some(self.t0.elapsed().as_millis());
            }
        }
        fn is_cancelled(&self) -> bool {
            if self.reasoning_seen.load(Ordering::SeqCst) >= self.cancel_after {
                let mut g = self.cancel_set_at.lock();
                if g.is_none() {
                    *g = Some(self.t0.elapsed().as_millis());
                }
                true
            } else {
                false
            }
        }
    }

    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", key));
    let bridge = LlmBridge::new(driver.clone(), "deepseek-v4-flash", 8192, Arc::new(growbox_llm::LexicalEmbedder), 60);
    let registry = Registry::with_builtins(TaskManager::new());
    let sandbox = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut memory = Memory::new();
    let flywheel = Flywheel::new();
    let sink = CancelLatencySink {
        t0: Instant::now(),
        reasoning_seen: AtomicUsize::new(0),
        cancel_after: 5, // 收到 5 个 reasoning chunk(思考刚铺开)即模拟"点了终止"
        cancel_set_at: Mutex::new(None),
        done_at: Mutex::new(None),
    };
    let cfg = AgentConfig {
        model: "deepseek-v4-flash".into(),
        max_tokens: 0,
        max_turns: 8,
        parallel_max: 4,
        system_prompt: "你是助手。先认真思考再回答。".into(),
        prompt_lang: "zh".into(),
        auto_mode: false,
        danger_mode: false,
        privacy_dirs: vec![],
        max_token_retries: 2,
        token_ceil: 32_768,
        silence_secs: 90,
        max_stall: 2,
        reasoning_effort: "high".into(),
        branch_log_max_gb: -1.0,
        self_verify: false,
        self_verify_min_tools: 3,
        recall_in_loop: false,
        tool_memory_enabled: false,
        tool_memory_veto_threshold: 0.85,
        tool_memory_warn_threshold: 0.80,
    };
    println!("\n========== 终止响应延迟实测 ==========");
    let out = agent_loop(
        "请详细推演五子棋开局的最优策略,分多种流派逐一展开。",
        &cfg, driver.as_ref(), &registry, &sandbox, &mut memory, &bridge, &bridge, &flywheel, dir.path(), &sink,
    )
    .await;
    let set_at = sink.cancel_set_at.lock().unwrap_or(0);
    let done_at = sink.done_at.lock().unwrap_or(0);
    println!("置取消(第5个reasoning chunk)@ {set_at} ms");
    println!("循环抛 Done @ {done_at} ms");
    println!("★终止延迟 = {} ms★(应 ~一个 chunk;若几十秒=drive_one 中断没生效)", done_at.saturating_sub(set_at));
    println!("stop={:?} reasoning_chunks={}", out.stopped, sink.reasoning_seen.load(Ordering::SeqCst));
    println!("======================================\n");
    assert_eq!(out.stopped, StopReason::Cancelled, "应被取消收口");
}

/// ★维护探针:改持久化设置里的 reasoning_effort(只动这一字段,不碰其它配置)★。
/// app 持 redb 独占锁,跑前先关 app。用法(默认设 high):
///   GX_REDB="$HOME/Library/Application Support/com.nightpoetry.growbox/growbox.redb" \
///   GX_SET_EFFORT=high cargo test -p growbox-gui --test live_artifact_diag set_persisted_reasoning_effort -- --ignored --nocapture
#[test]
#[ignore = "改持久化设置,需 GX_REDB 且 app 已关"]
fn set_persisted_reasoning_effort() {
    let path = std::env::var("GX_REDB").expect("需要 GX_REDB=growbox.redb 路径");
    let effort = std::env::var("GX_SET_EFFORT").unwrap_or_else(|_| "high".into());
    let store = growbox_memory::Store::open(&path).expect("打开 redb 失败(app 是否还开着占锁?)");
    let mut s: growbox_core::Settings = store.kv_get("settings").expect("settings 不存在(app 还没连过?)");
    println!("改前 reasoning_effort = {}", s.reasoning_effort);
    s.reasoning_effort = effort.clone();
    store.kv_put("settings", &s);
    let after: growbox_core::Settings = store.kv_get("settings").expect("回读 settings");
    println!("改后 reasoning_effort = {}", after.reasoning_effort);
    assert_eq!(after.reasoning_effort, effort, "落库应生效");
}

/// ★创建回合计时探针(定位"思考太久"到底慢在哪)★:用**真实系统提示词 + 真实工具集**(含 render_artifact
/// 长 llm_desc)+ reasoning_effort=high,直连 LLM 流式,把 **TTFT(首 chunk≈prefill+排队)与生成段
/// (reasoning 流式)分开计时** —— TTFT 大=上下文太大/缓存 miss 的 prefill 慢;生成段大=模型 reasoning 量太大。
///   DEEPSEEK_API_KEY=<key> cargo test -p growbox-gui --test live_artifact_diag diag_creation_turn_timing -- --ignored --nocapture
#[tokio::test]
#[ignore = "打真 API,需 DEEPSEEK_API_KEY"]
async fn diag_creation_turn_timing() {
    use growbox_llm::{ChatMessage, ChatRequest, StreamChunk};
    let key = std::env::var("DEEPSEEK_API_KEY").expect("需要环境变量 DEEPSEEK_API_KEY");

    // 真实系统提示词(system.zh.md)+ 真实工具集(注册表全集,含 render_artifact 长 llm_desc)。
    let prompt_path = format!("{}/../../prompts/agent/system.zh.md", env!("CARGO_MANIFEST_DIR"));
    let system = std::fs::read_to_string(&prompt_path).unwrap_or_else(|_| "你是 GrowBox。".into());
    let registry = Registry::with_builtins(TaskManager::new());
    let tools = registry.tools_for("zh", None, &std::collections::HashSet::new());
    let tools_json = serde_json::to_string(&tools).unwrap_or_default();
    let user = "陪我下五子棋,先把棋盘画出来。";

    println!("\n========== 创建回合计时(reasoning_effort=high)==========");
    println!(
        "system: {} 字 | tools: {} 个 / {} 字(JSON)| user: {} 字 | 粗估 prompt ≈ {} 字",
        system.chars().count(),
        tools.len(),
        tools_json.chars().count(),
        user.chars().count(),
        system.chars().count() + tools_json.chars().count() + user.chars().count()
    );

    let client = LlmClient::new("https://api.deepseek.com", key);
    let req = ChatRequest::new("deepseek-v4-flash", vec![ChatMessage::system(system), ChatMessage::user(user)])
        .with_tools(tools)
        .with_reasoning_effort("high");

    let t0 = Instant::now();
    let mut rx = client.chat_stream(req).await.expect("chat_stream 应成功");
    let mut ttft: Option<u128> = None;
    let mut last_reasoning_at = 0u128;
    let (mut rchars, mut cchars) = (0usize, 0usize);
    let mut tool_names: Vec<String> = vec![];
    let mut finish = String::new();
    while let Some(chunk) = rx.recv().await {
        let chunk = chunk.expect("chunk 应正常");
        let ms = t0.elapsed().as_millis();
        if ttft.is_none() {
            ttft = Some(ms);
            println!("[首 chunk @ {ms} ms]");
        }
        match chunk {
            StreamChunk::Reasoning(s) => {
                rchars += s.chars().count();
                last_reasoning_at = ms;
            }
            StreamChunk::Content(s) => cchars += s.chars().count(),
            StreamChunk::ToolCallDelta { name, .. } => {
                if let Some(n) = name {
                    if !n.is_empty() {
                        tool_names.push(n);
                    }
                }
            }
            StreamChunk::Done { finish_reason } => finish = finish_reason,
            StreamChunk::Usage { .. } => {}
        }
    }
    let total = t0.elapsed().as_millis();
    let ttft = ttft.unwrap_or(0);
    println!("\n---------- 计时结果 ----------");
    println!("TTFT(首 chunk,≈prefill+排队): {ttft} ms");
    println!("reasoning: {rchars} 字(流式至 {last_reasoning_at} ms)");
    println!("content: {cchars} 字 | tool_calls: {tool_names:?} | finish: {finish}");
    println!("生成段(TTFT→完): {} ms", total.saturating_sub(ttft));
    println!("总耗时: {total} ms ({:.1}s)", total as f64 / 1000.0);
    println!("\n判读:TTFT 占大头 → prefill 慢(上下文太大 / 缓存 miss);生成段占大头 → reasoning 量太大(模型想太久)。");
    println!("==========================================\n");
}
