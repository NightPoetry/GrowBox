//! 对话脊柱命令:发消息(流式/非流式/内部)+ 造物交互回合(写造物进聊天/端口静默二分)+ 取消/关闭。
//! 含 chat 引擎:三种 EventSink(Tauri/Noop/Artifact)+ run_chat 驱动 + 工具卡片渲染。

use super::*;

/// 流式对话返回体(与前端 ChatResponse 对齐)。
#[derive(Serialize)]
pub struct ChatResponse {
    pub content: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// 把脊柱事件翻译成前端事件(chat-chunk / ui-action / chat-status 等);授权走 request_decision。
struct TauriSink {
    app: AppHandle,
    /// 工具命令/路径是否截断显示(取自 Settings,回合开始时定格)。
    truncate: bool,
    /// UI 往返 ack 超时秒(取自 Settings,推论9 可设)。
    ui_ack_timeout_secs: u64,
    /// shell 批准弹窗等待超时秒(取自 Settings,推论9 可设)。
    shell_approval_timeout_secs: u64,
}

#[async_trait::async_trait]
impl EventSink for TauriSink {
    async fn emit(&self, event: AgentEvent) {
        match event {
            AgentEvent::Reasoning(s) => {
                let _ = self.app.emit("chat-chunk", json!({ "delta": s, "done": false, "kind": "thinking" }));
            }
            AgentEvent::Content(s) => {
                let _ = self.app.emit("chat-chunk", json!({ "delta": s, "done": false }));
            }
            AgentEvent::Notice(s) => {
                let _ = self.app.emit("chat-chunk", json!({ "delta": format!("\n> {s}\n"), "done": false, "kind": "tool_progress" }));
            }
            // 瞬态状态:不进聊天正文,单独发给动态指示器(自检动效"正在核查 xxx")。
            AgentEvent::Status(s) => {
                let _ = self.app.emit("chat-status", json!({ "label": s }));
            }
            AgentEvent::ToolStart { name, ref args } => {
                // 结构化工具标记(前端 markdown.ts 0a/0c 据此渲染终端风格代码框:`$ 工具名 命令 [状态]`
                // + 输出折叠区)。比旧的自然语言 `> 正在执行...` 有真正的代码框样式,且工具名走 i18n。
                let margs = tool_marker_args(&name, args);
                let _ = self.app.emit("chat-chunk", json!({ "delta": format!("\n▸ calling `{name}`:\n{margs}\n"), "done": false, "kind": "tool_progress" }));
            }
            AgentEvent::ToolEnd { name, ok, ref content } => {
                let output = tool_marker_output(&name, ok, content, self.truncate);
                let status = if ok { "OK" } else { "FAIL" };
                let _ = self.app.emit("chat-chunk", json!({ "delta": format!("[TOOL] `{name}` [{status}]\n```\n{output}\n```\n"), "done": false, "kind": "tool_progress" }));
            }
            AgentEvent::Intent(intent) => {
                // 交互类执行器:请前端打开预填 UI(控制反转)。
                // 前端监听 "ui-action"，payloak 格式 { action, data }。
                let _ = self.app.emit("ui-action", json!({ "action": intent.action, "data": intent.prefill }));
            }
            AgentEvent::Done => {}
        }
    }

    /// 回合级取消:读独立 `ChatControl` managed state(不走 AppState 锁,故 run_chat 持锁期间也读得到)。
    fn is_cancelled(&self) -> bool {
        self.app.state::<crate::chat_control::ChatControl>().is_cancelled()
    }

    /// 实时上下文压力:把本次请求的 prompt_tokens 写进独立 `ContextMeter`(不走 AppState 锁);
    /// get_status 取一份独立读出。见 `context_meter.rs`。
    fn note_context_tokens(&self, prompt_tokens: u32) {
        self.app.state::<crate::context_meter::ContextMeter>().set(prompt_tokens);
        // 事件驱动刷新(铁律:能触发就触发不轮询):每个 LLM 回合(=上一轮工具结果刚灌入上下文后)
        // 即推新值给前端,仪表立刻动起来,不必等 2s 轮询 → 用户看得到逐次工具调用后的上下文增长。
        let _ = self.app.emit("context-tokens", prompt_tokens);
    }

    /// 取消句柄:复用 `ChatControl`(与 is_cancelled 同源)→ 穿进 ExecCtx 让 shell 等长命令中途响应终止。
    fn cancel_flag(&self) -> growbox_core::CancelFlag {
        Some(self.app.state::<crate::chat_control::ChatControl>().flag())
    }

    /// 家族二 UI 往返(活的 IDE):发出带相关 id 的 "ui-action",等前端 `ui_action_ack` 回执。
    /// 登记表是独立 managed state(不走 AppState 锁),故 run_chat 持锁 await 不会与 ack 命令死锁。
    async fn ui_round_trip(&self, intent: &UiIntent) -> UiAck {
        let registry = self.app.state::<UiAckRegistry>();
        let (id, rx) = registry.register();
        let _ = self.app.emit(
            "ui-action",
            json!({ "action": intent.action, "data": intent.prefill, "id": id }),
        );
        match tokio::time::timeout(std::time::Duration::from_secs(self.ui_ack_timeout_secs), rx).await {
            Ok(Ok(ack)) => ack,
            // 接收端被取消(Canceled)或超时:撤登记 + 诚实判未生效。
            _ => {
                registry.cancel(&id);
                UiAck::unapplied("前端回执超时")
            }
        }
    }

    /// 用户决定脊柱(唯一 round-trip):凡需用户裁决的动作都经此。独立 `Decisions` 登记表,不触
    /// AppState 锁(故 run_chat 持锁 await 与 `decision_ack` 命令不死锁)。超时按拒绝(安全侧)。
    /// shell 审批"已信任则免问 + 记忆"在此处理;路径授权的持久化交前端既有授权流(回合后落地)。
    async fn request_decision(&self, kind: DecisionKind) -> Decision {
        let reg = self.app.state::<Decisions>();
        // shell:已信任(本命令/信任本项目)→ 免问直接放行。
        if let DecisionKind::ShellApproval { command } = &kind {
            if reg.shell_trusted(command) {
                return Decision::Once;
            }
        }
        let (id, rx) = reg.register();
        // 弹给前端:按 kind 路由到对应弹窗(权限/shell)。payload 含 id + 扁平化的 kind 字段。
        let mut payload = serde_json::to_value(&kind).unwrap_or_else(|_| json!({}));
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("id".into(), json!(id));
        }
        let _ = self.app.emit("decision-request", payload);
        let decision = match tokio::time::timeout(
            std::time::Duration::from_secs(self.shell_approval_timeout_secs),
            rx,
        )
        .await
        {
            Ok(Ok(d)) => d,
            _ => {
                reg.cancel(&id);
                Decision::Deny // 超时/取消:拒绝
            }
        };
        // shell 记忆(会话级):记住本命令 / 信任本项目所有命令。路径授权的"记住"由前端落地。
        if let DecisionKind::ShellApproval { command } = &kind {
            match decision {
                Decision::Remember => reg.shell_remember(command),
                Decision::TrustProject => reg.shell_trust_all(),
                _ => {}
            }
        }
        decision
    }

    /// 交互式终端:开 PTY 会话,会话输出经 `terminal-output` 直推前端 xterm(同步 app.emit,
    /// 独立于 agent 回合),并 emit `terminal-open` 让前端自动挂载终端面板。返回 session id。
    async fn open_terminal(&self, command: &str, work_dir: &std::path::Path) -> Option<String> {
        let app = self.app.clone();
        let on_output: crate::pty::OutputSink = std::sync::Arc::new(move |id: &str, chunk: &str| {
            let _ = app.emit("terminal-output", json!({ "session_id": id, "data": chunk }));
        });
        match crate::pty::open(command, work_dir, 80, 24, on_output) {
            Ok(id) => {
                let _ = self.app.emit("terminal-open", json!({ "session_id": id, "command": command }));
                Some(id)
            }
            Err(e) => {
                let _ = self.app.emit("chat-chunk", json!({ "delta": format!("\n> 交互终端启动失败: {e}\n"), "done": false, "kind": "tool_progress" }));
                None
            }
        }
    }
}

/// 不抛事件的汇(send_message 非流式用)。
struct NoopSink;
#[async_trait::async_trait]
impl EventSink for NoopSink {
    async fn emit(&self, _event: AgentEvent) {}
}

/// 自驱续跑用的计数汇:包一层 `TauriSink`,数本回合"真干活"的工具调用数(排除 finish/ask_user)。
/// 用途:判断这一轮自驱**有没有实际推进**——0 = AI 只是回话没动手(典型 = 确实没事可做了),
/// 前端据此在连续空转后暂停续跑,不无谓地一直空烧 token。其余职责(取消/授权/UI 往返/上下文计量/
/// 交互终端)全部原样透传给内层,自驱回合与普通回合能力一致。
struct CountingSink {
    inner: TauriSink,
    work_tools: std::sync::atomic::AtomicUsize,
}

impl CountingSink {
    fn new(inner: TauriSink) -> Self {
        Self { inner, work_tools: std::sync::atomic::AtomicUsize::new(0) }
    }
    /// 这一轮是否真正推进了工作(调用过非 finish/ask_user 的工具)。
    fn did_work(&self) -> bool {
        self.work_tools.load(std::sync::atomic::Ordering::SeqCst) > 0
    }
}

#[async_trait::async_trait]
impl EventSink for CountingSink {
    async fn emit(&self, event: AgentEvent) {
        // 工具开始 = 在动手。finish/ask_user 是控制信号(收尾/提问),不算"推进了工作"。
        if let AgentEvent::ToolStart { name, .. } = &event {
            if !matches!(name.as_str(), "finish" | "ask_user") {
                self.work_tools.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
        self.inner.emit(event).await;
    }
    fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
    fn note_context_tokens(&self, prompt_tokens: u32) {
        self.inner.note_context_tokens(prompt_tokens);
    }
    fn cancel_flag(&self) -> growbox_core::CancelFlag {
        self.inner.cancel_flag()
    }
    async fn ui_round_trip(&self, intent: &UiIntent) -> UiAck {
        self.inner.ui_round_trip(intent).await
    }
    async fn request_decision(&self, kind: DecisionKind) -> Decision {
        self.inner.request_decision(kind).await
    }
    async fn open_terminal(&self, command: &str, work_dir: &std::path::Path) -> Option<String> {
        self.inner.open_terminal(command, work_dir).await
    }
}

// ===================== 对话(脊柱)=====================

#[tauri::command]
pub async fn send_message_stream(
    state: State<'_, SharedState>,
    app: AppHandle,
    message: String,
) -> Result<ChatResponse, String> {
    let (truncate, ui_ack_timeout_secs, shell_approval_timeout_secs) = {
        let s = &state.lock().await.settings;
        (s.truncate_tool_display, s.ui_ack_timeout_secs as u64, s.shell_approval_timeout_secs as u64)
    };
    // 新回合开始:清上一回合的取消标志(造物交互 v2 §2)。
    app.state::<crate::chat_control::ChatControl>().begin();
    let sink = TauriSink { app, truncate, ui_ack_timeout_secs, shell_approval_timeout_secs };
    run_chat(&state, &message, false, None, &sink, false, false).await
}

/// 终止当前回合(造物交互 v2 §2「可终止」):前端「终止」按钮调用,瞬时置取消标志。
/// 独立 `ChatControl` managed state,不锁 AppState —— 故 run_chat 全程持 AppState 锁时也能即时生效;
/// 脊柱在下一轮检查点优雅收口(StopReason::Cancelled)。
#[tauri::command]
pub fn cancel_chat(chat_control: State<'_, crate::chat_control::ChatControl>) {
    chat_control.request_cancel();
}

#[tauri::command]
pub async fn send_message(state: State<'_, SharedState>, message: String) -> Result<ChatResponse, String> {
    run_chat(&state, &message, false, None, &NoopSink, false, false).await
}

/// 内部消息(非用户说的话):面板裁决回流(确认/取消)、感知层事件等。与用户消息分类:
/// 经 `Memory::perceive` 感知 + 以 system 角色 seed(不当用户学/显示)+ **AI 有权不执行**(无 finish 义务)。
/// 见用户决策 2026-06-02(内部消息 vs 用户消息)。
#[tauri::command]
pub async fn send_internal_message(
    state: State<'_, SharedState>,
    app: AppHandle,
    message: String,
) -> Result<ChatResponse, String> {
    let (truncate, ui_ack_timeout_secs, shell_approval_timeout_secs) = {
        let s = &state.lock().await.settings;
        (s.truncate_tool_display, s.ui_ack_timeout_secs as u64, s.shell_approval_timeout_secs as u64)
    };
    app.state::<crate::chat_control::ChatControl>().begin();
    let sink = TauriSink { app, truncate, ui_ack_timeout_secs, shell_approval_timeout_secs };
    run_chat(&state, &message, true, None, &sink, false, false).await
}

/// 自驱续跑单步返回:在普通对话返回(content/model/tokens)之上多带 `did_work`——这一轮自驱
/// 有没有真正动手(调了非 finish/ask_user 的工具)。前端据此决定继续下一轮、还是连续空转后暂停。
#[derive(Serialize)]
pub struct SelfDriveResponse {
    pub content: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    /// 这一轮是否真正推进了工作(调用了非 finish/ask_user 的工具)。
    pub did_work: bool,
}

/// 自驱续跑一步(全自动模式下的"自动鞭策")。程序停下来后,前端循环器调本命令注入一条系统
/// 自动生成的"继续推进"种子,驱动 AI 自己评估现状、判断要不要重构/治理屎山、并动手做下一步。
///
/// ★记录而非历史(用户原话:只进记录不进历史记录,就是个标签问题)★:种子以 `role=internal`
/// 落时间线 → 进记录、AI 全链路可感知,但 `get_chat_history` 只放 user/assistant,故**不进对话历史**;
/// AI 自己的回复/动手仍以 `assistant` 落地 → 用户照常看得见它在干活。
///
/// 仅全自动模式可用(自动审核 shell 才能真正无人值守地连续干活);非自动模式直接空返回让前端循环停。
#[tauri::command]
pub async fn self_drive_step(
    state: State<'_, SharedState>,
    app: AppHandle,
) -> Result<SelfDriveResponse, String> {
    let (truncate, ui_ack_timeout_secs, shell_approval_timeout_secs, auto_mode, lang, model) = {
        let s = &state.lock().await.settings;
        (
            s.truncate_tool_display,
            s.ui_ack_timeout_secs as u64,
            s.shell_approval_timeout_secs as u64,
            s.auto_mode,
            s.lang.clone(),
            s.model.clone(),
        )
    };
    // 仅全自动模式可自驱(需自动审核 shell 才能无人值守连续干活)。非自动模式:不驱动,空返回让前端循环停。
    if !auto_mode {
        return Ok(SelfDriveResponse {
            content: String::new(),
            model,
            input_tokens: 0,
            output_tokens: 0,
            did_work: false,
        });
    }
    app.state::<crate::chat_control::ChatControl>().begin();
    let inner = TauriSink { app, truncate, ui_ack_timeout_secs, shell_approval_timeout_secs };
    let sink = CountingSink::new(inner);
    let seed = self_drive_seed(&lang);
    let resp = run_chat(&state, &seed, true, None, &sink, false, true).await?;
    Ok(SelfDriveResponse {
        content: resp.content,
        model: resp.model,
        input_tokens: resp.input_tokens,
        output_tokens: resp.output_tokens,
        did_work: sink.did_work(),
    })
}

/// 自驱"自动鞭策"种子(系统自动生成,不当用户发言;按提示词语言出中/英)。设计目标:让 AI 自己
/// 核实真实状态 → 判断最有价值的下一步(推进/重构/补测试/清技术债)→ 重点防"屎山" → 真正动手做完
/// 一步 → 确实没事可做了就如实说明并停手(**不调工具 = 干净退出信号**,前端据此连续空转后暂停续跑)。
fn self_drive_seed(lang: &str) -> String {
    if lang.starts_with("zh") {
        "[自驱·继续推进 / 系统自动触发,不是用户在说话]\n\
         你正处于「自动续跑」模式:用户希望你自己一直往前推进,不用停下来等他确认。现在请你:\n\
         1. 先核实真实状态——必要时读文件 / 看 git diff / 跑测试,别凭记忆或假设就认为\"已经做完了\";\n\
         2. 想清楚接下来「最有价值的一步」是什么:是继续推进没做完的功能,还是该回头重构、补测试、\
         清理刚才留下的技术债;\n\
         3. 重点自检有没有正在滑向「屎山」的苗头——重复代码、超长函数、职责不清、命名混乱、缺测试、\
         临时 hack 没收尾。发现了就优先治理,别让它越滚越大;\n\
         4. 选定一步就**真正动手做完它**(调用工具实际执行,不要只列计划、空谈);\n\
         5. 一轮做一件事就行,下一轮系统会再来推动你。\n\n\
         只有当你确认「当前确实没有任何有价值的事可做了」时,才如实说明并停手——这种情况下\
         **不要调用任何工具**,直接用一两句话说明现状即可(系统据此暂停续跑)。不要为了凑数硬造工作,\
         也不要反过来问用户该做什么——自动模式下你自己按工程判断决定。"
            .to_string()
    } else {
        "[Self-drive · keep going / triggered automatically by the system, NOT the user speaking]\n\
         You are in \"auto-continue\" mode: the user wants you to keep making progress on your own \
         without stopping to wait for confirmation. Now:\n\
         1. First verify the real state — read files / check git diff / run tests as needed; don't \
         assume something is \"already done\" from memory;\n\
         2. Decide the single most valuable next step: keep pushing an unfinished feature, or step \
         back to refactor, add tests, and pay down the tech debt you just created;\n\
         3. Actively check for signs of the codebase turning into spaghetti — duplicated code, \
         over-long functions, muddled responsibilities, bad names, missing tests, leftover temporary \
         hacks. If you find any, fix it first before it snowballs;\n\
         4. Pick one step and **actually carry it out** (call tools to do it for real — don't just \
         describe a plan);\n\
         5. Do one thing this round; the system will prompt you again next round.\n\n\
         Only when you are sure there is genuinely nothing valuable left to do should you say so and \
         stop — in that case **do not call any tool**, just state the situation in a sentence or two \
         (the system pauses auto-continue based on that). Don't invent busywork, and don't turn around \
         and ask the user what to do — in auto mode you decide yourself by engineering judgment."
            .to_string()
    }
}

/// 锁状态、拆分借用、跑脊柱。`internal=true` = 内部消息(感知 + 内部 seed,AI 可不执行)。
async fn run_chat(
    state: &SharedState,
    message: &str,
    internal: bool,
    artifact: Option<(String, String, String)>,
    sink: &dyn EventSink,
    // ★流式窗口记忆★:true = 瞬态观察/共驾轮(交互终端事件点唤醒)——seed/工具转录/收尾不写主记忆,
    // 只进上下文+展示。见 agent::run_agent_loop 的 transient。
    transient: bool,
    // ★自驱续跑★:true = 这是"自动鞭策"种子(全自动模式下程序停下来后自动注入的"继续推进")。
    // 仅改感知层记录的 kind 标签(SELF_DRIVE 而非 UI_EVENT),让记录里这条内部种子一眼可辨;
    // role 仍是 internal(进记录、AI 可感知、不进对话历史)。其余驱动逻辑与普通内部消息一致。
    self_drive: bool,
) -> Result<ChatResponse, String> {
    // 潜意识仲裁器取 Agent 档(最高):整回合持槽,在飞的后台(睡眠/飞轮)调用一结束就让位前台。
    // **必须在拿 AppState 锁之前 acquire**——后台睡眠是"持 Sleep 档 → 等 AppState 锁",若前台
    // "持 AppState 锁 → 等 Agent 档"就死锁。先拿档(不持锁)、再锁状态,锁序一致无环(见 `arbiter.rs`)。
    let arbiter = { state.lock().await.arbiter.clone() };
    let _gate = arbiter.acquire_owned(crate::arbiter::Priority::Agent).await;

    let mut guard = state.lock().await;
    let st = &mut *guard;
    st.touch_activity(); // 回合开始:标记前台活动,IdleWorker 让位。
    // 持久化写失败 AI 也必须能感知(与 health 红警灯同源,这条面向 AI):仅在计数增长时 perceive 一次,
    // 落内部状态环 + 时间线(可检索)。见决策日志 2026-06-01。
    if let Some((count, last)) = st.store.as_ref().and_then(|s| s.write_fault()) {
        if count > st.perceived_write_faults {
            st.perceived_write_faults = count;
            crate::notify::perceive_notice(
                &mut st.memory,
                &st.settings.lang,
                "store.write_failed",
                &serde_json::json!({ "count": count, "last": last }),
            );
        }
    }
    let llm = st.llm.clone().ok_or("尚未连接 LLM,请先在设置里连接")?;
    let bridge = st.bridge.clone().ok_or("尚未连接 LLM")?;
    // 系统提示词 = 基础提示词(从资源文件加载,经提示词自转译 chokepoint) + 当前项目自我感知上下文。
    // 项目上下文是运行时动态数据(项目名/目录/记忆数),不转译;只转译静态基础提示词那半。
    let base = crate::transpile::tr(
        "agent.system",
        crate::transpile::PromptRole::Main,
        &st.settings.lang,
        &st.base_system_prompt,
    );
    let full_prompt = format!("{}\n\n{}", base, st.project_context());
    let cfg = AgentConfig {
        model: st.settings.model.clone(),
        max_tokens: st.settings.max_tokens,
        // ★造物交互回合限轮数(防失控)★:用户落子等触发的 ArtifactSink 回合静默不进聊天,
        // 若放任 max_turns=1000,AI 会把"更新棋盘"当无限可优化的事反复 render(每次样式略不同,
        // 思考免死的"近乎全等"判不出 → 不收口),致画布一直刷而聊天早停(用户 2026-06-04 真机惊到)。
        // 造物响应应收敛:更新一次回应交互即可。限 4 轮(render→可能 selftest→finish)。
        max_turns: if artifact.is_some() { st.settings.max_turns.min(4) } else { st.settings.max_turns },
        parallel_max: st.settings.parallel_max as usize,
        system_prompt: full_prompt,
        prompt_lang: st.settings.lang.clone(),
        auto_mode: st.settings.auto_mode,
        // ★danger 模式(为所欲为)★:跟随设置。造物交互回合不放开(秒级响应、用户在场,无需 danger)。
        danger_mode: st.settings.danger_mode && artifact.is_none(),
        privacy_dirs: st.settings.privacy_dirs.clone(),
        max_token_retries: st.settings.agent_max_token_retries as usize,
        token_ceil: st.settings.agent_token_ceil,
        silence_secs: st.settings.agent_silence_secs as u64,
        max_stall: st.settings.agent_max_stall as usize,
        // ★造物交互响应用 high(快回)★:用户落子等交互要秒级响应,max effort 会思考几分钟
        // (实测画棋盘 reasoning 5 万字/234s),致落子像没反应、用户反复点 → 积压回合陆续刷屏
        // (2026-06-04 真机"没法点击/造物不断出现"真因)。响应回合降 high;主回合保持用户设置。
        reasoning_effort: if artifact.is_some() { "high".into() } else { st.settings.reasoning_effort.clone() },
        branch_log_max_gb: st.settings.branch_log_max_gb,
        // ★主动自检★:造物交互回合要秒级响应、不自检(避免多花一轮);普通任务按用户设置。
        self_verify: st.settings.self_verify && artifact.is_none(),
        self_verify_min_tools: st.settings.self_verify_min_tools as usize,
        // ★回合内补检索★:造物交互回合要秒级响应、不补检索(避免每轮多一次嵌入/检索延迟);普通任务按用户设置。
        recall_in_loop: st.settings.recall_in_loop && artifact.is_none(),
        // ★工具记忆 + 不犯第二遍★:造物交互回合要秒级响应,不会诊(省一次嵌入);普通任务按用户设置。
        tool_memory_enabled: st.settings.tool_memory_enabled && artifact.is_none(),
        tool_memory_veto_threshold: st.settings.tool_memory_veto_threshold,
        tool_memory_warn_threshold: st.settings.tool_memory_warn_threshold,
    };
    // ★danger 模式★:把 sandbox 的 danger 标志与本回合 cfg 同步(judge 据此一律放行)。两处同源、每回合校准。
    st.sandbox.set_danger(cfg.danger_mode);
    let work_dir = st.work_dir.clone();
    let model = st.settings.model.clone();

    // 内部消息先经感知层(落内部状态环 + 时间线 role=internal,AI 全链路可见),再以内部 seed 驱动。
    // kind 走受控 kind 表(③):用 ui_event 而非散装中文,渲染时按表恢复双语标签。
    if let Some((cid, cb, val)) = &artifact {
        // 造物交互:入独立造物瞬态环(不落时间线、不进聊天),由 agent_loop_internal 内部 seed 驱动。
        st.memory.perceive_artifact(cid, cb, val);
    } else if internal && !transient {
        // transient(终端事件点观察轮):seed 不落时间线(流式窗口记忆)。
        if self_drive {
            // ★P3★ 自驱只记一条**精简** SELF_DRIVE 标记进记录(满足用户"在记录中"),**不落完整 ~500 字鞭策种子**
            // ——否则几百轮重复种子会膨胀 timeline + 把 recent-ring 挤满鞭策噪音、挤掉真实工作。
            // 完整种子只作本轮系统消息让 AI 看到(见 agent_loop_internal 的 self_drive:跳过全文 ingest)。
            st.memory.perceive(
                growbox_memory::node_kind::SELF_DRIVE,
                "自动续跑:系统鞭策 AI 评估现状并继续推进下一步(完整指令见本轮上下文,不入档以免记录膨胀)",
            );
        } else {
            st.memory.perceive(growbox_memory::node_kind::UI_EVENT, message);
        }
    }

    // ★工作流端口触发(P2)★:造物交互回调(cb=端口)若命中某绑定该画布的造物工作流的 trigger,
    // 就从对应节点进入工作流跑整套强制流转(current_wf 是 run-local → 每回合靠端口"恢复"到正确节点)。
    // 命中时用**中性 seed**(只陈述交互事实),把"该怎么做"交给节点引导 + 工具收窄(替代散装劝诫 seed)。
    let initial_wf = artifact.as_ref().and_then(|(cid, cb, _)| st.registry.resolve_trigger(cid, cb));
    let neutral_seed = artifact.as_ref().map(|(cid, cb, val)| {
        // 宽松兜底(与 perceive_artifact 一致):上报量 LLM 自己决定,只防 runaway 撑爆;超限的感知由 perceive_artifact 标记。
        let v: String = val.chars().take(16384).collect();
        format!("[造物交互] 用户在造物「{cid}」中操作:{cb} = {v}")
    });
    let effective_msg: &str = match (&initial_wf, &neutral_seed) {
        (Some(_), Some(seed)) => seed.as_str(), // 工作流接管:中性 seed,节点引导驱动。
        _ => message,                           // 无工作流:维持原 seed(主聊天 / free-form 造物劝诫 seed)。
    };

    let outcome = if internal {
        agent_loop_internal(
            effective_msg,
            initial_wf,
            &cfg,
            llm.as_ref(),
            &st.registry,
            &st.sandbox,
            &mut st.memory,
            bridge.as_ref(),
            bridge.as_ref(),
            &st.flywheel,
            &work_dir,
            sink,
            transient,
            self_drive,
        )
        .await
    } else {
        agent_loop(
            message,
            &cfg,
            llm.as_ref(),
            &st.registry,
            &st.sandbox,
            &mut st.memory,
            bridge.as_ref(),
            bridge.as_ref(),
            &st.flywheel,
            &work_dir,
            sink,
        )
        .await
    };

    st.touch_activity(); // 回合结束:8 分钟 idle 倒计时从此刻起算(而非回合开始)。
    Ok(ChatResponse { content: outcome.final_text, model, input_tokens: 0, output_tokens: 0 })
}

/// 造物回合的 Sink:AI 的自由文字/思考/工具进度**不进聊天**(造物交互不污染对话);
/// 但 render_artifact 更新画布(ui_round_trip)与家族一意图/授权请求照常落地。
/// Phase 3 把 Content 接到造物覆盖层;Phase 2 先静默。
struct ArtifactSink {
    inner: TauriSink,
}

/// 造物交互回合里「该不该进聊天」的二分(造物交互 v2 §1 + 灵魂二分):
/// - **写造物**(`render_artifact`/`selftest_artifact` = 写代码)→ 进聊天(写代码/文件天然可见、可终止)。
/// - **端口沟通**(`artifact_command` 落子、`push_artifact_notice` 覆盖层 = 高频)→ 不进(太频繁刷屏)。
/// - 自由文字/思考 → 不进(造物交互回合的"对话"归造物本身,不污染主聊天)。
/// - 家族一意图(弹表单)/ 系统提示(Notice,如终止)→ 仍要落地(需用户感知)。
/// - 授权请求不在此列:它走决定脊柱 `request_decision`(独立 round-trip,不经 emit 过滤)。
fn artifact_turn_emits_to_chat(event: &AgentEvent) -> bool {
    match event {
        AgentEvent::ToolStart { name, .. } | AgentEvent::ToolEnd { name, .. } => {
            matches!(name.as_str(), "render_artifact" | "selftest_artifact")
        }
        AgentEvent::Intent(_) | AgentEvent::Notice(_) => true,
        // 思考/正文/瞬态状态/Done:不进聊天(其它工具 artifact_command/push_notice/文件读写已在上面第一臂判 false)。
        AgentEvent::Reasoning(_) | AgentEvent::Content(_) | AgentEvent::Status(_) | AgentEvent::Done => false,
    }
}

#[async_trait::async_trait]
impl EventSink for ArtifactSink {
    async fn emit(&self, event: AgentEvent) {
        // 造物交互 v2 §1 二分:写造物(render/selftest)进聊天,端口沟通/思考/正文不进。
        if artifact_turn_emits_to_chat(&event) {
            self.inner.emit(event).await;
        }
    }
    fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled() // 造物回合也可被「终止」按钮叫停(反复 render 时)
    }
    async fn ui_round_trip(&self, intent: &UiIntent) -> UiAck {
        self.inner.ui_round_trip(intent).await
    }
    async fn request_decision(&self, kind: DecisionKind) -> Decision {
        self.inner.request_decision(kind).await
    }
}

/// 造物交互回传(流2):用户在沙箱画布点击/输入 → 前端 postMessage → 此命令。
/// 入独立造物瞬态环(不进聊天/不落时间线)+ 跑一个内部 agent 回合让 AI 感知并响应(更新造物)。
/// AI 的反应经 ArtifactSink 不污染聊天;render_artifact 仍往返生效。见 Phase 2。
#[tauri::command]
pub async fn artifact_event(
    state: State<'_, SharedState>,
    app: AppHandle,
    canvas_id: String,
    callback_id: String,
    value: String,
    realtime: bool,
) -> Result<(), String> {
    let (truncate, ui_ack_timeout_secs, shell_approval_timeout_secs) = {
        let s = &state.lock().await.settings;
        (s.truncate_tool_display, s.ui_ack_timeout_secs as u64, s.shell_approval_timeout_secs as u64)
    };
    // 新造物回合开始:清取消标志(用户可终止反复 render 的造物回合,造物交互 v2 §2)。
    app.state::<crate::chat_control::ChatControl>().begin();
    let inner = TauriSink { app, truncate, ui_ack_timeout_secs, shell_approval_timeout_secs };
    let sink = ArtifactSink { inner };
    // 宽松兜底(与 perceive_artifact 一致):上报量 LLM 自己决定,只防 runaway 撑爆;超限感知由 perceive_artifact 标记。
    let val_short: String = value.chars().take(16384).collect();
    let seed = if realtime {
        format!(
            "[造物交互·实时输入] 用户正在造物「{canvas_id}」的「{callback_id}」中输入(当前:{val_short})。这可能**还没输完**——若你只是想给提示/建议,可直接 push_artifact_notice;但若要据此采取行动(提交/做决定),必须先用 ask_user 问用户是否已输入完成。无需回应则可不动作。"
        )
    } else {
        format!(
            "[造物交互] 用户在造物「{canvas_id}」中操作:{callback_id} = {val_short}。如需回应,用 render_artifact **更新一次**给出新状态(如落子后的新棋盘)即可,然后立刻 finish;**不要反复微调样式或多次重渲**——用户只想看到一个最终结果,反复刷画布会打断他。若无需回应,直接 finish 不动作。"
        )
    };
    run_chat(&state, &seed, true, Some((canvas_id, callback_id, value)), &sink, false, false)
        .await
        .map(|_| ())
}

/// 造物窗口关闭硬机制(造物交互 v2 §4)。前端用户点 × 真关造物 → 调本命令。
/// **硬性(非 LLM 判断)**:① 立刻取消在跑的造物回合(端口已不通,别再操作)② AI 感知"造物没了、端口不通,
/// 别再 artifact_command 它"③ 用户侧 toast"造物窗口已关闭"。前端组件 onCleanup 自关监听(组件卸载即硬性关闭)。
#[tauri::command]
pub async fn artifact_closed(
    state: State<'_, SharedState>,
    app: AppHandle,
    chat_control: State<'_, crate::chat_control::ChatControl>,
    canvas_id: String,
) -> Result<(), String> {
    // ① 硬性叫停在跑的造物回合(独立标志,不锁 AppState,即时生效)。
    chat_control.request_cancel();
    // ③ 用户侧 toast(对外,不需锁)。
    crate::notify::emit_notice(&app, "artifact.closed", json!({ "canvas_id": canvas_id }));
    // ② AI 侧感知(对内):造物没了、端口不通,别再操作它。短锁写内存即可。
    let mut st = state.lock().await;
    let lang = st.settings.lang.clone();
    crate::notify::perceive_notice(&mut st.memory, &lang, "artifact.closed", &json!({ "canvas_id": canvas_id }));
    Ok(())
}

/// 交互式终端:用户在 xterm 里敲的键 → 写进 PTY 会话 stdin(共驾的"用户侧输入")。
/// 不驱动 AI(只喂终端);AI 由事件点经 terminal_event 唤醒。无需锁 AppState(pty 自带注册表)。
#[tauri::command]
pub async fn pty_input(session_id: String, data: String) -> Result<(), String> {
    crate::pty::send(&session_id, &data);
    Ok(())
}

/// 交互式终端事件点唤醒(对齐 artifact_event):前端 xterm 检测到事件点(输出静默 debounce / 匹配关键模式)
/// → 调本命令 → 跑一个内部 agent 回合,seed = 会话当前屏尾,让 AI 感知并决定(接管 / 提示用户 / 收尾)。
/// AI 的反应经 TauriSink 走聊天(用户能看到它在共驾)。
#[tauri::command]
pub async fn terminal_event(state: State<'_, SharedState>, app: AppHandle, session_id: String) -> Result<u64, String> {
    let (truncate, ui_ack_timeout_secs, shell_approval_timeout_secs) = {
        let s = &state.lock().await.settings;
        (s.truncate_tool_display, s.ui_ack_timeout_secs as u64, s.shell_approval_timeout_secs as u64)
    };
    app.state::<crate::chat_control::ChatControl>().begin();
    let sink = TauriSink { app, truncate, ui_ack_timeout_secs, shell_approval_timeout_secs };
    let screen = crate::pty::peek(&session_id, 4096).unwrap_or_default();
    let alive = crate::pty::is_alive(&session_id);
    let seed = format!(
        "[终端会话「{session_id}」事件点{tag}] 当前屏尾:\n{screen}\n\n请判断当下该怎么做:\
         需要你接管(如登录已成功)就 pty_send 敲后续命令;需要用户操作(如输用户名/密码等敏感输入)\
         就用 ask_user 或直接说明让他在终端里输入,别替他敲;还没到可动作的节点就不动作;\
         会话已结束或任务完成则 pty_close。",
        tag = if alive { "" } else { "·会话已结束" }
    );
    run_chat(&state, &seed, true, None, &sink, true, false).await?;
    // ★P3 自适应轮询★:回合里 AI 可能调过 pty_watch 设了"看一眼间隔";返回给前端排下一拍强制唤醒
    // (0=不轮询,回纯事件驱动)。看守期全程 transient(P2),不写主记忆。
    Ok(crate::pty::watch_interval(&session_id))
}

/// 交互式终端关闭(对齐 artifact_closed):用户在前端关掉终端面板 → kill 会话 + AI 感知"终端没了,别再操作"。
#[tauri::command]
pub async fn terminal_closed(state: State<'_, SharedState>, session_id: String) -> Result<(), String> {
    crate::pty::close(&session_id);
    let mut st = state.lock().await;
    st.memory.perceive(
        "terminal_closed",
        format!("用户关闭了交互终端会话「{session_id}」,该会话端口已不通,别再 pty_send/pty_peek 它。"),
    );
    Ok(())
}

// ===================== 工具卡片渲染(供 TauriSink)=====================

/// 工具参数 → 工具卡片展示用的精简 JSON(只留 command/path/name 等小字段;丢弃 content 等大字段
/// 避免刷屏)。前端 `toolBlockHtml` 据此提取命令/路径渲染。
fn tool_marker_args(name: &str, args: &str) -> String {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    let pick = |keys: &[&str]| -> String {
        let mut m = serde_json::Map::new();
        for k in keys {
            if let Some(x) = v.get(*k) {
                let empty = matches!(x, Value::String(s) if s.trim().is_empty());
                if !empty {
                    m.insert((*k).to_string(), x.clone());
                }
            }
        }
        // 没摘到任何字段 → 返回空串(而非 "{}");前端见空 args 不渲染参数行,免误导(Bug A)。
        if m.is_empty() {
            String::new()
        } else {
            Value::Object(m).to_string()
        }
    };
    match name {
        "shell" => pick(&["command"]),
        "file_write" | "file_edit" | "file_read" | "file_list" => pick(&["path"]),
        "create_project" => pick(&["name", "path"]),
        "spawn_task" => pick(&["command", "label"]),
        "open_settings" => pick(&["field", "note"]),
        "ui_control" => pick(&["target", "op"]),
        // 造物工具:html 太大不展示,只摘 canvas_id/text(否则 fallback 会显示一坨)。
        "render_artifact" | "selftest_artifact" => pick(&["canvas_id"]),
        "push_artifact_notice" => pick(&["canvas_id", "text"]),
        // 其余(含 finish 等空参工具)给空串 —— 前端 toolBlockHtml 见空 args 不渲染参数行,
        // 不再显示误导性的孤立 `{}`(Bug A,用户 2026-06-03 真机报)。
        _ => String::new(),
    }
}

/// 工具结束 → 卡片里的输出(终端风格代码框)。shell 成功给 stdout/stderr;任何失败给错误内容;
/// 列/等任务给结果;其余工具不刷内容(摘要由卡片按 path/name 显示)。防破坏 fence + 防超长刷屏。
fn tool_marker_output(name: &str, ok: bool, content: &str, truncate: bool) -> String {
    let show = !ok || matches!(name, "shell" | "list_tasks" | "wait_tasks");
    if !show || content.trim().is_empty() {
        return String::new();
    }
    let cap = if truncate { 600 } else { 3000 };
    let safe = content.replace("```", "'''"); // 防 ``` 破坏 markdown 代码围栏
    if safe.chars().count() > cap {
        format!("{}\n...[已截断]", safe.chars().take(cap).collect::<String>())
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造物交互 v2 §1 二分:写造物进聊天、端口沟通/思考/正文不进。
    #[test]
    fn artifact_chat_split_writes_vs_ports() {
        let ts = |n: &str| AgentEvent::ToolStart { name: n.into(), args: String::new() };
        let te = |n: &str| AgentEvent::ToolEnd { name: n.into(), ok: true, content: String::new() };
        // 写造物 → 进聊天。
        assert!(artifact_turn_emits_to_chat(&ts("render_artifact")));
        assert!(artifact_turn_emits_to_chat(&te("render_artifact")));
        assert!(artifact_turn_emits_to_chat(&ts("selftest_artifact")));
        // 端口沟通(高频)→ 不进。
        assert!(!artifact_turn_emits_to_chat(&ts("artifact_command")));
        assert!(!artifact_turn_emits_to_chat(&te("push_artifact_notice")));
        // 其它工具噪声(文件读写)→ 不进。
        assert!(!artifact_turn_emits_to_chat(&ts("file_read")));
        // 思考/正文 → 不进(造物回合的对话归造物本身)。
        assert!(!artifact_turn_emits_to_chat(&AgentEvent::Reasoning("想哪落子".into())));
        assert!(!artifact_turn_emits_to_chat(&AgentEvent::Content("我落 E5".into())));
        // 需用户感知的 → 仍要落地(授权请求不在 emit 流里:走 request_decision 独立 round-trip)。
        assert!(artifact_turn_emits_to_chat(&AgentEvent::Notice("已终止".into())));
        assert!(artifact_turn_emits_to_chat(&AgentEvent::Intent(growbox_core::UiIntent {
            action: "create_project".into(), prefill: serde_json::json!({}), await_ack: false, gates: true,
        })));
    }
}
