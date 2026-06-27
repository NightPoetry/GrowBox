//! 真机端到端 —— 活的 IDE 的 `ui_control` 在**真 deepseek LLM** 下验证(推论 7)。
//!
//! 验证:用户说"关掉记忆面板" → 真模型选中并调用 `ui_control{target:memory,op:close}` →
//! 脊柱往返(emit + 等回执)→ 拿到**验证过的**状态(open=false)回填。
//! 这里用一个会 ack 的 sink 模拟前端落地(真 GUI 的 DOM 关面板 headless 测不了,见文末说明)。
//! 默认 #[ignore](不打真 API 不计费)。显式跑:
//!   DEEPSEEK_API_KEY=<key> cargo test -p growbox-gui --test live_ui_control -- --ignored --nocapture
//!
//! 端到端边界(诚实说明):本测试覆盖"真 LLM 选对工具 + 真往返路径 + 验证态回填";
//! **未覆盖**真实前端 DOM 把面板关掉那一下(那需跑起 Tauri 窗口 + 人工观察,见 Phase 4 收口说明)。

use std::sync::Arc;

use growbox_core::UiIntent;
use growbox_gui::agent::{agent_loop, AgentConfig, AgentEvent, EventSink, StopReason};
use growbox_gui::bridge::{LlmBridge, LlmDriver};
use growbox_gui::registry::Registry;
use growbox_gui::tasks::TaskManager;
use growbox_gui::ui::{empty_catalog, UiAck, UiSurface};
use growbox_learn::Flywheel;
use growbox_llm::LlmClient;
use growbox_memory::Memory;
use growbox_safety::Sandbox;
use parking_lot::Mutex;
use tempfile::tempdir;

/// 会 ack 的 sink:打印事件 + 模拟前端落地 UI 操作(close → open=false),回报验证态。
struct AckingPrinter {
    events: Mutex<Vec<AgentEvent>>,
}

#[async_trait::async_trait]
impl EventSink for AckingPrinter {
    async fn emit(&self, ev: AgentEvent) {
        match &ev {
            AgentEvent::Reasoning(s) => print!("\x1b[90m{s}\x1b[0m"),
            AgentEvent::Content(s) => print!("{s}"),
            AgentEvent::ToolStart { name, args } => println!("\n>> 调用 {name} {args}"),
            AgentEvent::ToolEnd { name, ok, content } => println!("\n<< {name} ok={ok}: {content}"),
            AgentEvent::Notice(s) => println!("\n[通知] {s}"),
            AgentEvent::Intent(i) => println!("\n[UI意图] {}", i.action),
            AgentEvent::Status(s) => println!("\n[状态] {s}"),
            AgentEvent::Done => println!("\n[完成]"),
        }
        self.events.lock().push(ev);
    }

    async fn ui_round_trip(&self, intent: &UiIntent) -> UiAck {
        // 模拟前端:对 open 报 open=true,其余(close/toggle 此场景)报 open=false。
        let op = intent.prefill.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let open = op == "open";
        println!("\n[前端落地] {} → open={open}", intent.prefill);
        UiAck { applied: true, state: serde_json::json!({ "open": open }), note: None }
    }
}

#[tokio::test]
#[ignore = "打真 API,需 DEEPSEEK_API_KEY"]
async fn live_ui_control_closes_panel_end_to_end() {
    let key = std::env::var("DEEPSEEK_API_KEY").expect("需要环境变量 DEEPSEEK_API_KEY");
    let dir = tempdir().unwrap();

    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", key));
    let bridge = LlmBridge::new(driver.clone(), "deepseek-v4-flash", 8192, Arc::new(growbox_llm::LexicalEmbedder), 60);

    // 前端声明的面板目录:有 memory(支持 open/close/toggle)。
    let catalog = empty_catalog();
    *catalog.write() = vec![UiSurface {
        id: "memory".into(),
        label: "记忆可视化面板".into(),
        ops: vec!["open".into(), "close".into(), "toggle".into()],
    }];
    let registry = Registry::with_builtins_catalog(TaskManager::new(), catalog);
    let sandbox = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut memory = Memory::new();
    let flywheel = Flywheel::new();
    let sink = AckingPrinter { events: Mutex::new(Vec::new()) };

    let cfg = AgentConfig {
        model: "deepseek-v4-flash".into(),
        max_tokens: 8192,
        max_turns: 6,
        parallel_max: 4,
        system_prompt: "你是 GrowBox 助手。你能用 ui_control 直接操控界面面板的可见性\
            (target=面板标识,op=open/close/toggle),用户想开/关/切某面板时直接代劳。\
            做完用一句中文告诉用户。".into(),
        prompt_lang: "zh".into(),
        auto_mode: false,
        danger_mode: false,
        privacy_dirs: vec![],
        max_token_retries: 2,
        token_ceil: 32_768,
        silence_secs: 90,
        max_stall: 2,
        reasoning_effort: "max".into(),
        branch_log_max_gb: -1.0,
        ..Default::default()
    };

    let msg = "太挤了,帮我把记忆面板关掉。";

    println!("\n========== 活的 IDE 真机端到端开始 ==========\n");
    let out = agent_loop(
        msg, &cfg, driver.as_ref(), &registry, &sandbox, &mut memory, &bridge, &bridge, &flywheel, dir.path(), &sink,
    )
    .await;
    println!("\n\n========== 结束: stop={:?} turns={} ==========", out.stopped, out.turns);

    // 1) 循环正常完成。
    assert_eq!(out.stopped, StopReason::Completed, "整条循环应正常完成");

    // 2) 真模型选中并调用了 ui_control,且往返成功拿到验证态(open=false,不撒谎)。
    let events = sink.events.lock();
    let end = events.iter().find_map(|e| match e {
        AgentEvent::ToolEnd { name, ok, content } if name == "ui_control" => Some((*ok, content.clone())),
        _ => None,
    });
    let (ok, content) = end.expect("真模型应调用 ui_control 关闭记忆面板");
    assert!(ok, "ui_control 往返应成功");
    assert!(content.contains("false"), "结果应反映前端回报的验证态 open=false,实得: {content}");
}
