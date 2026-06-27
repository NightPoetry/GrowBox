//! ★C1 实验:懒加载下 agent 的真实行为★(真机端到端,默认 #[ignore])。
//!
//! 验证 C1 的"成本侧":懒加载开启后,真模型能否**自己 tool_search 加载 deferred 工具并完成真任务**
//! (会不会因为工具只露名、schema 不在场而卡住/变笨)。对照:同任务懒关时直接调工具。
//!
//! 跑法(用户开服务器 API 后):
//!   DEEPSEEK_API_KEY=<key> cargo test -p growbox-gui --test live_lazy_agent -- --ignored --nocapture

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
                let head: String = content.chars().take(160).collect();
                println!("\n<< {name} ok={ok}: {head}");
            }
            AgentEvent::Notice(s) => println!("\n[通知] {s}"),
            AgentEvent::Done => println!("\n[完成]"),
            _ => {}
        }
        self.events.lock().push(ev);
    }
}

fn cfg() -> AgentConfig {
    AgentConfig {
        model: "deepseek-v4-flash".into(),
        max_tokens: 8192,
        max_turns: 10,
        parallel_max: 4,
        system_prompt: "你是 GrowBox 编码助手,工作在项目沙箱内。优先用工具动手,别只动嘴。\
            注意:部分工具未直接加载(系统会列出'可用但未加载的工具'名单),需要用它们时先调 tool_search 拉回用法再调用。"
            .into(),
        prompt_lang: "zh".into(),
        auto_mode: true,
        danger_mode: false,
        privacy_dirs: vec![],
        max_token_retries: 2,
        token_ceil: 32_768,
        silence_secs: 90,
        max_stall: 2,
        reasoning_effort: "high".into(),
        branch_log_max_gb: -1.0,
        ..Default::default()
    }
}

/// 懒加载开:code_search 是 deferred,模型必须先 tool_search 才能用它完成"找用法"任务。
#[tokio::test]
#[ignore = "打真 API,需 DEEPSEEK_API_KEY + 服务器在线"]
async fn lazy_agent_uses_tool_search_to_load_deferred_tool() {
    let key = std::env::var("DEEPSEEK_API_KEY").expect("需要 DEEPSEEK_API_KEY");
    let dir = tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn greet(name: &str) -> String { format!(\"hi {name}\") }\n\
         pub fn run() { let _ = greet(\"a\"); let _ = greet(\"b\"); }\n",
    )
    .unwrap();

    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", key));
    let bridge = LlmBridge::new(driver.clone(), "deepseek-v4-flash", 8192, Arc::new(growbox_llm::LexicalEmbedder), 60);
    let mut registry = Registry::with_builtins(TaskManager::new());
    // ★懒加载开,code_search 在默认 deferred 名单里(只露名,需 tool_search 加载)★。
    registry.set_lazy_tools(true, growbox_core::Settings::default().deferred_tools);
    let sandbox = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut memory = Memory::new();
    let flywheel = Flywheel::new();
    let sink = Printer { events: Mutex::new(Vec::new()) };

    let msg = "用 code_search 找出 greet 这个函数在 src 里被调用的位置(报文件:行号);找到后用一句话告诉我。";

    println!("\n========== C1 懒加载·agent 行为实验(code_search 是 deferred)==========\n");
    let out = agent_loop(msg, &cfg(), driver.as_ref(), &registry, &sandbox, &mut memory, &bridge, &bridge, &flywheel, dir.path(), &sink).await;
    println!("\n\n========== 结束:{:?} ({} 轮) ==========", out.stopped, out.turns);

    let evs = sink.events.lock();
    let called = |n: &str| evs.iter().any(|e| matches!(e, AgentEvent::ToolStart { name, .. } if name == n));
    let searched = called("tool_search");
    let used_cs = called("code_search");
    println!("\n[行为判定] 调过 tool_search={searched}  调过 code_search={used_cs}  完成={:?}", out.stopped);

    assert_eq!(out.stopped, StopReason::Completed, "应正常完成(懒加载不该让 agent 卡死)");
    assert!(searched, "懒加载下,模型应先 tool_search 加载 deferred 的 code_search");
    assert!(used_cs, "应真的调用 code_search 完成'找用法'任务");
}
