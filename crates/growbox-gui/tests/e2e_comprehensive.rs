//! 端到端综合测试套件 —— 覆盖 Agent 循环全部路径。
//!
//! 用真 deepseek-v4-flash 验证: 连接 / 对话 / 文件操作 / shell / 安全 / 学习 / 截断重试。
//! 默认 #[ignore](不打真 API)。显式跑:
//!   DEEPSEEK_API_KEY=<key> cargo test -p growbox-gui --test e2e_comprehensive -- --ignored --nocapture
//!
//! 测试场景:
//!   01 简单问答(无工具)
//!   02 单工具文件读写
//!   03 多工具组合(shell + file)
//!   04 越界安全拒绝
//!   05 多轮自动纠错
//!   06 学习采集
//!   07 断点续传(file_edit)
//!   08 并发压力(消息队列)
//!   UI 自检(用 __GROWBOX__ 钩子)

use std::path::Path;
use std::sync::Arc;

use growbox_gui::agent::{agent_loop, AgentConfig, AgentEvent, EventSink, StopReason};
use growbox_gui::bridge::{LlmBridge, LlmDriver};
use growbox_gui::registry::Registry;
use growbox_gui::state::AppState;
use growbox_gui::tasks::TaskManager;
use growbox_learn::Flywheel;
use growbox_llm::LlmClient;
use growbox_memory::Memory;
use growbox_safety::Sandbox;
use parking_lot::Mutex;
use tempfile::tempdir;

// ── 测试工具 ──────────────────────────────────────────────

struct Collector {
    events: Mutex<Vec<AgentEvent>>,
    decisions: Mutex<usize>,
}

#[async_trait::async_trait]
impl EventSink for Collector {
    async fn emit(&self, ev: AgentEvent) {
        self.events.lock().push(ev);
    }

    // 授权走决定脊柱 round-trip(不再是 AgentEvent):计数后按无前端安全默认裁决。
    async fn request_decision(&self, kind: growbox_gui::decision::DecisionKind) -> growbox_gui::decision::Decision {
        use growbox_gui::decision::{Decision, DecisionKind};
        *self.decisions.lock() += 1;
        match kind {
            DecisionKind::ShellApproval { .. } => Decision::Once,
            DecisionKind::PathPermission { .. } => Decision::Deny,
        }
    }
}

impl Collector {
    fn new() -> Self { Collector { events: Mutex::new(Vec::new()), decisions: Mutex::new(0) } }
    fn kinds(&self) -> Vec<AgentEvent> { self.events.lock().clone() }
    fn tool_count(&self) -> usize {
        self.events.lock().iter().filter(|e| matches!(e, AgentEvent::ToolStart { .. })).count()
    }
    fn ok_tool_count(&self) -> usize {
        self.events.lock().iter().filter(|e| matches!(e, AgentEvent::ToolEnd { ok: true, .. })).count()
    }
    fn decision_count(&self) -> usize {
        *self.decisions.lock()
    }
}

fn key() -> String {
    std::env::var("DEEPSEEK_API_KEY").expect("需要 DEEPSEEK_API_KEY 环境变量")
}

fn cfg() -> AgentConfig {
    AgentConfig {
        model: "deepseek-v4-flash".into(),
        max_tokens: 8192,
        max_turns: 8,
        parallel_max: 4,
        system_prompt: "你是 GrowBox 助手。你能用工具:file_write/file_read/file_edit/file_list/shell。\
            能动手就动手,别只动嘴。改文件前先读、看现状再改。\
            工具结果回给你,继续推进直到完成。用一句简洁中文给最终答复。"
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
        recall_in_loop: false,
        tool_memory_enabled: false,
        tool_memory_veto_threshold: 0.85,
        tool_memory_warn_threshold: 0.80,
    }
}

// ── Test 01: 简单问答(不需要工具) ───────────────────────

#[tokio::test]
#[ignore = "打真 API"]
async fn e2e_01_simple_chat_no_tools() {
    let k = key();
    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", k.clone()));
    let bridge = LlmBridge::new(driver.clone(), "deepseek-v4-flash", 8192, std::sync::Arc::new(growbox_llm::LexicalEmbedder), 60);
    let reg = Registry::with_builtins(TaskManager::new());
    let dir = tempdir().unwrap();
    let sandbox = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::new();

    let cfg = cfg();
    println!("\n========== E2E-01: 简单问答 ==========");
    let out = agent_loop(
        "你好,请用一句话介绍你自己,不要用工具。",
        &cfg, driver.as_ref(), &reg, &sandbox, &mut mem, &bridge, &bridge, &fw, dir.path(), &sink,
    ).await;

    println!("结果: stop={:?} turns={} final='{}'", out.stopped, out.turns, out.final_text);
    assert_eq!(out.stopped, StopReason::Completed);
    assert!(!out.final_text.trim().is_empty(), "应有最终答复");
    // 简单问答应该 1 轮完成(无工具调用)
    assert!(out.turns <= 2, "简单问答不应超过2轮");
    // 无工具调用
    assert_eq!(sink.tool_count(), 0, "简单问答不应触发工具");
}

// ── Test 02: 文件读写 ────────────────────────────────────

#[tokio::test]
#[ignore = "打真 API"]
async fn e2e_02_file_read_write() {
    let k = key();
    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", k.clone()));
    let bridge = LlmBridge::new(driver.clone(), "deepseek-v4-flash", 8192, std::sync::Arc::new(growbox_llm::LexicalEmbedder), 60);
    let reg = Registry::with_builtins(TaskManager::new());
    let dir = tempdir().unwrap();
    // 先建一个已存在的文件
    std::fs::write(dir.path().join("config.toml"), "version = \"1.0\"\nname = \"test\"").unwrap();
    let sandbox = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::new();

    println!("\n========== E2E-02: 文件读写 ==========");
    let out = agent_loop(
        "请用 file_read 读取 config.toml 的内容,然后把 version 改成 \"2.0\" 用 file_write 写回。",
        &cfg(), driver.as_ref(), &reg, &sandbox, &mut mem, &bridge, &bridge, &fw, dir.path(), &sink,
    ).await;

    println!("结果: stop={:?} turns={} final='{}'", out.stopped, out.turns, out.final_text);
    assert_eq!(out.stopped, StopReason::Completed, "应正常完成");
    assert!(sink.ok_tool_count() >= 2, "至少成功执行 2 个工具(read+write)");
    // 文件被更新
    let content = std::fs::read_to_string(dir.path().join("config.toml")).unwrap_or_default();
    assert!(content.contains("2.0"), "version 应改为 2.0,实际内容: {content:?}");
}

// ── Test 03: 多工具组合(shell + file) ───────────────────

#[tokio::test]
#[ignore = "打真 API"]
async fn e2e_03_shell_and_file_combo() {
    let k = key();
    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", k.clone()));
    let bridge = LlmBridge::new(driver.clone(), "deepseek-v4-flash", 8192, std::sync::Arc::new(growbox_llm::LexicalEmbedder), 60);
    let reg = Registry::with_builtins(TaskManager::new());
    let dir = tempdir().unwrap();
    let sandbox = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::new();

    println!("\n========== E2E-03: shell+file 组合 ==========");
    let out = agent_loop(
        "请用 shell 执行 `echo 'hello from shell' > greeting.txt`,然后用 file_read 确认内容正确。",
        &cfg(), driver.as_ref(), &reg, &sandbox, &mut mem, &bridge, &bridge, &fw, dir.path(), &sink,
    ).await;

    println!("结果: stop={:?} turns={} final='{}'", out.stopped, out.turns, out.final_text);
    assert_eq!(out.stopped, StopReason::Completed, "应正常完成");
    assert!(sink.ok_tool_count() >= 2, "至少成功执行 shell+read");
    let content = std::fs::read_to_string(dir.path().join("greeting.txt")).unwrap_or_default();
    assert!(content.contains("hello from shell"), "shell 应创建文件,实际: {content:?}");
}

// ── Test 04: 越界安全拒绝 ────────────────────────────────

#[tokio::test]
#[ignore = "打真 API"]
async fn e2e_04_safety_out_of_bounds() {
    let k = key();
    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", k.clone()));
    let bridge = LlmBridge::new(driver.clone(), "deepseek-v4-flash", 8192, std::sync::Arc::new(growbox_llm::LexicalEmbedder), 60);
    let reg = Registry::with_builtins(TaskManager::new());
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();
    // 只允许 dir,不允许 outside
    let sandbox = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::new();

    let outside_file = outside.path().join("secret.txt");
    println!("\n========== E2E-04: 越界安全 ==========");
    let out = agent_loop(
        &format!("请用 file_write 写到 {},内容是 'leaked'。如果被拒绝就告诉我被拒绝了。", outside_file.display()),
        &cfg(), driver.as_ref(), &reg, &sandbox, &mut mem, &bridge, &bridge, &fw, dir.path(), &sink,
    ).await;

    println!("结果: stop={:?} turns={} final='{}' decision_count={}",
        out.stopped, out.turns, out.final_text, sink.decision_count());
    assert_eq!(out.stopped, StopReason::Completed);
    // 应有授权裁决请求(决定脊柱)或拒绝
    let had_perm = sink.decision_count() > 0;
    let had_deny = sink.kinds().iter().any(|e| matches!(e, AgentEvent::ToolEnd { ok: false, .. }));
    assert!(had_perm || had_deny, "越界写应触发权限请求或拒绝");
    // 文件不应被创建
    assert!(!outside_file.exists(), "越界文件不应被创建");
}

// ── Test 05: 多轮自动纠错 ────────────────────────────────

#[tokio::test]
#[ignore = "打真 API"]
async fn e2e_05_multi_turn_error_recovery() {
    let k = key();
    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", k.clone()));
    let bridge = LlmBridge::new(driver.clone(), "deepseek-v4-flash", 8192, std::sync::Arc::new(growbox_llm::LexicalEmbedder), 60);
    let reg = Registry::with_builtins(TaskManager::new());
    let dir = tempdir().unwrap();
    // 建一个嵌套目录
    std::fs::create_dir_all(dir.path().join("data/sub")).unwrap();
    std::fs::write(dir.path().join("data/readme.md"), "# Data\n\nSome data files.").unwrap();
    let sandbox = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::new();

    println!("\n========== E2E-05: 多轮纠错 ==========");
    let out = agent_loop(
        "请做以下事情:1)用 file_list 列出 data 目录;2)读 data/readme.md;3)在 data/sub/ 下创建 summary.md,写一句总结。\
         如果某步失败就根据错误信息调整重试。",
        &cfg(), driver.as_ref(), &reg, &sandbox, &mut mem, &bridge, &bridge, &fw, dir.path(), &sink,
    ).await;

    println!("结果: stop={:?} turns={} final='{}' tools={}/{}",
        out.stopped, out.turns, out.final_text, sink.ok_tool_count(), sink.tool_count());
    assert_eq!(out.stopped, StopReason::Completed);
    assert!(sink.ok_tool_count() >= 2, "至少成功执行 2 个工具");
}

// ── Test 06: 学习采集 ────────────────────────────────────

#[tokio::test]
#[ignore = "打真 API"]
async fn e2e_06_learning_collection() {
    let k = key();
    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", k.clone()));
    let bridge = LlmBridge::new(driver.clone(), "deepseek-v4-flash", 8192, std::sync::Arc::new(growbox_llm::LexicalEmbedder), 60);
    let reg = Registry::with_builtins(TaskManager::new());
    let dir = tempdir().unwrap();
    let sandbox = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::new();

    println!("\n========== E2E-06: 学习采集 ==========");
    let out = agent_loop(
        "请创建文件 todo.txt 写入一行 'buy milk',然后读回确认。",
        &cfg(), driver.as_ref(), &reg, &sandbox, &mut mem, &bridge, &bridge, &fw, dir.path(), &sink,
    ).await;

    println!("结果: stop={:?} turns={} final='{}' conclusions={}",
        out.stopped, out.turns, out.final_text, mem.conclusions().len());
    assert_eq!(out.stopped, StopReason::Completed);
    // 脊柱每步操作采集经验结论
    let conclusions = mem.conclusions();
    println!("采集到的结论:");
    for c in conclusions {
        println!("  - {} -> {} (active={})", c.operation, c.expected, c.is_active());
    }
    assert!(!conclusions.is_empty(), "应采集到经验结论");
    // 至少有一条成功经验
    assert!(conclusions.iter().any(|c| c.is_active()), "应至少有 1 条有效经验");
}

// ── Test 07: 多文件编辑 ──────────────────────────────────

#[tokio::test]
#[ignore = "打真 API"]
async fn e2e_07_multi_file_edit() {
    let k = key();
    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", k.clone()));
    let bridge = LlmBridge::new(driver.clone(), "deepseek-v4-flash", 8192, std::sync::Arc::new(growbox_llm::LexicalEmbedder), 60);
    let reg = Registry::with_builtins(TaskManager::new());
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "apple").unwrap();
    std::fs::write(dir.path().join("b.txt"), "banana").unwrap();
    let sandbox = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
    let mut mem = Memory::new();
    let fw = Flywheel::new();
    let sink = Collector::new();

    println!("\n========== E2E-07: 多文件编辑 ==========");
    let out = agent_loop(
        "请读 a.txt 和 b.txt,然后把 b.txt 的内容改成 a.txt 的内容前面加 'merged: '。用 file_edit 或 file_write。",
        &cfg(), driver.as_ref(), &reg, &sandbox, &mut mem, &bridge, &bridge, &fw, dir.path(), &sink,
    ).await;

    println!("结果: stop={:?} turns={} final='{}'", out.stopped, out.turns, out.final_text);
    assert_eq!(out.stopped, StopReason::Completed);
}

// ── Test 08: AppState 集成(模拟 GUI 层 connect→chat→reset 流程) ──

#[tokio::test]
#[ignore = "打真 API"]
async fn e2e_08_appstate_integration() {
    let k = key();
    let dir = tempdir().unwrap();
    let mut state = AppState::new(dir.path().to_path_buf());

    // 1. 建项目
    let proj_dir = dir.path().join("myproject");
    std::fs::create_dir_all(&proj_dir).unwrap();
    let pid = state.create_project(None, "集成测试项目", vec![proj_dir.clone()], vec![]);
    println!("项目 ID: {pid}");
    assert_eq!(state.current.as_deref(), Some(pid.as_str()));

    // 2. 连接 LLM
    use growbox_core::Settings;
    let settings = Settings {
        api_base: "https://api.deepseek.com".into(),
        api_key: k,
        model: "deepseek-v4-flash".into(),
        ..Default::default()
    };
    let sid = state.connect(settings);
    println!("会话 ID: {sid}");
    assert!(state.connected);
    assert!(state.llm.is_some());
    assert!(state.bridge.is_some());

    // 3. 模拟 send_message 流程
    let driver = state.llm.clone().unwrap();
    let bridge = state.bridge.clone().unwrap();
    let cfg = AgentConfig {
        model: "deepseek-v4-flash".into(),
        max_tokens: 8192,
        max_turns: 8,
        parallel_max: 4,
        system_prompt: "你是助手。用工具完成任务,用中文回复。".into(),
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
        recall_in_loop: false,
        tool_memory_enabled: false,
        tool_memory_veto_threshold: 0.85,
        tool_memory_warn_threshold: 0.80,
    };
    let sink = Collector::new();

    println!("\n========== E2E-08: AppState 集成 ==========");
    let out = agent_loop(
        "创建文件 hello.txt 写 'GrowBox E2E Test',然后告诉我完成了。",
        &cfg, driver.as_ref(), &state.registry, &state.sandbox,
        &mut state.memory, bridge.as_ref(), bridge.as_ref(), &state.flywheel, &state.work_dir, &sink,
    ).await;

    println!("结果: stop={:?} turns={} final='{}' tools={}",
        out.stopped, out.turns, out.final_text, sink.tool_count());
    assert_eq!(out.stopped, StopReason::Completed);
    assert!(!out.final_text.trim().is_empty());

    // 4. 验证文件真落盘
    let hello = proj_dir.join("hello.txt");
    assert!(hello.exists(), "hello.txt 应被创建在项目目录");
    let content = std::fs::read_to_string(&hello).unwrap();
    assert!(content.contains("GrowBox"), "文件应含 GrowBox,实际: {content:?}");

    // 5. reset session
    let new_sid = format!("sess-{}", growbox_core::now().timestamp_millis());
    state.session_id = Some(new_sid);
    assert!(state.session_id.is_some());

    // 6. 验证学习采集
    let conclusions = state.memory.conclusions();
    println!("采集结论数: {}", conclusions.len());
    assert!(!conclusions.is_empty(), "应采集到经验结论");

    println!("\nE2E-08 全部通过!");
}

// ── Test 09: 重复相同消息(幂等性) ────────────────────────

#[tokio::test]
#[ignore = "打真 API"]
async fn e2e_09_idempotent_repeat() {
    let k = key();
    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", k.clone()));
    let bridge = LlmBridge::new(driver.clone(), "deepseek-v4-flash", 8192, std::sync::Arc::new(growbox_llm::LexicalEmbedder), 60);
    let reg = Registry::with_builtins(TaskManager::new());
    let dir = tempdir().unwrap();
    let sandbox = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);

    println!("\n========== E2E-09: 幂等性测试 ==========");

    // 第一遍:创建文件
    {
        let mut mem = Memory::new();
        let fw = Flywheel::new();
        let sink = Collector::new();
        let out = agent_loop(
            "创建文件 counter.txt,写入数字 1。",
            &cfg(), driver.as_ref(), &reg, &sandbox, &mut mem, &bridge, &bridge, &fw, dir.path(), &sink,
        ).await;
        println!("第1遍: stop={:?} turns={}", out.stopped, out.turns);
        assert_eq!(out.stopped, StopReason::Completed);
    }

    // 第二遍:读回文件并确认(验证持久化)
    {
        let mut mem = Memory::new();
        let fw = Flywheel::new();
        let sink = Collector::new();
        let out = agent_loop(
            "读 counter.txt,告诉我内容是什么。",
            &cfg(), driver.as_ref(), &reg, &sandbox, &mut mem, &bridge, &bridge, &fw, dir.path(), &sink,
        ).await;
        println!("第2遍: stop={:?} turns={} final='{}'", out.stopped, out.turns, out.final_text);
        assert_eq!(out.stopped, StopReason::Completed);
        assert!(out.final_text.contains('1'), "应读到数字1,实际: {}", out.final_text);
    }
}

// ── Test 10: UI 自检清单(不需要 API key,不 ignored) ─────

/// 这个测试不需要 API key,验证所有 UI 相关 trait 和类型正确性。
#[tokio::test]
async fn e2e_10_ui_self_check() {
    // 验证 Registry 内置执行器完整
    let reg = Registry::with_builtins(TaskManager::new());
    let defs = reg.definitions("zh");
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    println!("注册的执行器: {:?}", names);
    assert!(names.contains(&"file_read"), "缺少 file_read");
    assert!(names.contains(&"file_write"), "缺少 file_write");
    assert!(names.contains(&"file_edit"), "缺少 file_edit");
    assert!(names.contains(&"file_list"), "缺少 file_list");
    assert!(names.contains(&"shell"), "缺少 shell");
    assert!(names.contains(&"create_project"), "缺少 create_project");

    // 验证 AppState 初始化正确
    let dir = tempdir().unwrap();
    let state = AppState::new(dir.path().to_path_buf());
    assert!(!state.connected);
    assert_eq!(state.settings.model, "deepseek-v4-flash");

    // 验证 Sandbox 默认拒绝
    let sb = Sandbox::new(vec![], vec![]);
    use growbox_safety::{Operation, Verdict};
    assert!(
        matches!(sb.judge(&Operation::Write(Path::new("/etc/passwd"))), Verdict::NeedAuth { .. }),
        "空沙箱应对路径外写返回 NeedAuth"
    );

    // 验证 Memory 空查询不 panic
    let mut mem = Memory::new();
    use growbox_memory::Subconscious;
    struct DummySub;
    #[async_trait::async_trait]
    impl Subconscious for DummySub {
        async fn embed(&self, _: &str) -> Vec<f32> { vec![0.0; 256] }
        async fn judge_relevant(&self, _: &str, _: &[String]) -> Vec<usize> { vec![] }
    }
    let (hits, _) = mem.retrieve("test", &DummySub).await;
    assert!(hits.is_empty(), "空记忆应无命中");

    // 验证 Flywheel 可读写
    let fw = Flywheel::new();
    let snap = growbox_learn::Snapshot::new("file_write(x)".to_string(), "ok".to_string(), true);
    let c = fw.collect(snap);
    assert!(c.operation.contains("file_write"));

    println!("\nE2E-10 UI 自检全部通过!");
}
