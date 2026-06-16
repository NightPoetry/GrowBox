//! 真机端到端 —— 用真 deepseek-v4-flash 跑通整条 Agent 循环(装配后第一次打真 API)。
//!
//! 验证:流式 reasoning/content → 工具调用解析 → 唯一安全门 → 真执行器落盘 → 结果回填自纠 → 完成 + 学习采集。
//! 默认 #[ignore](不打真 API 不计费)。显式跑:
//!   DEEPSEEK_API_KEY=<key> cargo test -p growbox-gui --test live_agent -- --ignored --nocapture

use std::sync::Arc;

use growbox_gui::agent::{agent_loop, AgentConfig, AgentEvent, EventSink, StopReason};
use growbox_gui::bridge::{LlmBridge, LlmDriver};
use growbox_gui::registry::Registry;
use growbox_gui::tasks::TaskManager;
use growbox_learn::Flywheel;
use growbox_llm::LlmClient;
use growbox_memory::Memory;
use growbox_safety::Sandbox;
use parking_lot::Mutex;
use tempfile::tempdir;

/// 把脊柱事件实时打印出来,便于肉眼看真模型行为;同时收集供断言。
struct Printer {
    events: Mutex<Vec<AgentEvent>>,
}

#[async_trait::async_trait]
impl EventSink for Printer {
    async fn emit(&self, ev: AgentEvent) {
        match &ev {
            AgentEvent::Reasoning(s) => print!("\x1b[90m{s}\x1b[0m"),
            AgentEvent::Content(s) => print!("{s}"),
            AgentEvent::ToolStart { name, args } => println!("\n>> 调用 {name} {args}"),
            AgentEvent::ToolEnd { name, ok, content } => {
                let head: String = content.chars().take(120).collect();
                println!("\n<< {name} ok={ok}: {head}");
            }
            AgentEvent::Notice(s) => println!("\n[通知] {s}"),
            AgentEvent::Intent(i) => println!("\n[UI意图] {}", i.action),
            AgentEvent::Status(s) => println!("\n[状态] {s}"),
            AgentEvent::Done => println!("\n[完成]"),
        }
        self.events.lock().push(ev);
    }
}

#[tokio::test]
#[ignore = "打真 API,需 DEEPSEEK_API_KEY"]
async fn live_agent_creates_file_end_to_end() {
    let key = std::env::var("DEEPSEEK_API_KEY").expect("需要环境变量 DEEPSEEK_API_KEY");
    let dir = tempdir().unwrap();

    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", key));
    let bridge = LlmBridge::new(driver.clone(), "deepseek-v4-flash", 8192, std::sync::Arc::new(growbox_llm::LexicalEmbedder), 60);
    let registry = Registry::with_builtins(TaskManager::new());
    let sandbox = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut memory = Memory::new();
    let flywheel = Flywheel::new();
    let sink = Printer { events: Mutex::new(Vec::new()) };

    let cfg = AgentConfig {
        model: "deepseek-v4-flash".into(),
        max_tokens: 8192,
        max_turns: 8,
        system_prompt: "你是 GrowBox 助手,工作在项目沙箱内。你有工具:file_write/file_read/file_list/shell。\
            能动手就动手,别只动嘴;改文件前先看清现状。完成后用一句中文告诉用户结果。"
            .into(),
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
        self_verify: false,
        self_verify_min_tools: 3,
        tool_memory_enabled: false,
        tool_memory_veto_threshold: 0.85,
        tool_memory_warn_threshold: 0.80,
    };

    let msg = "请在当前项目目录用 file_write 创建文件 note.txt,内容写一行:GrowBox 工作正常。\
        然后用 file_read 读回确认,最后一句话告诉我结果。";

    println!("\n========== 真机端到端开始 ==========\n");
    let out = agent_loop(
        msg,
        &cfg,
        driver.as_ref(),
        &registry,
        &sandbox,
        &mut memory,
        &bridge,
        &bridge,
        &flywheel,
        dir.path(),
        &sink,
    )
    .await;
    println!("\n\n========== 结束: stop={:?} turns={} ==========", out.stopped, out.turns);
    println!("最终答复: {}\n", out.final_text);

    // 1) 循环正常完成。
    assert_eq!(out.stopped, StopReason::Completed, "整条循环应正常完成");

    // 2) 真模型经工具把文件落盘了(端到端最硬的证据)。
    let note = dir.path().join("note.txt");
    assert!(note.exists(), "note.txt 应由真模型经 file_write 创建");
    let content = std::fs::read_to_string(&note).unwrap();
    assert!(content.contains("GrowBox"), "note.txt 内容应含 GrowBox,实得: {content:?}");

    // 3) 至少成功执行过一个工具,且有最终答复。
    let events = sink.events.lock();
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::ToolEnd { ok: true, .. })),
        "应有成功的工具执行"
    );
    assert!(!out.final_text.trim().is_empty(), "应有最终文字答复");

    // 4) 学习:每步操作采集了经验。
    assert!(!memory.conclusions().is_empty(), "应采集到经验结论");
}
