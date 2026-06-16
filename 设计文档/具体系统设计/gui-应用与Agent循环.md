# gui(growbox-gui)—— 应用外壳 + Agent 脊柱 + 前端

> 依赖全部其他 crate 的顶层 app。架构公理:**Agent 循环是唯一脊柱,一切能力皆执行器**(一个注册表、一条分发路径、一处安全门)。

## 后端模块(`crates/growbox-gui/src/`)
- `main.rs` / `lib.rs` —— Tauri app 入口、命令注册。
- `agent.rs` —— **Agent 循环脊柱**:user→assistant→tool 摄入 memory(`ingest_with_role`),驱动 LLM、解析工具调用、过执行器、收口(`finalize()`)。`max_turns` 默认 1000(0=无限);早停治本=finish 终止执行器 + 裸文本催续 + 空转兜底(催促对记忆不可见)。主驱动有沉默超时 `SILENCE_SECS=90`。
  - **P4 上下文组装**:回合开头 `memory.assemble_context(query)` 产出分区 `ContextBlock`,经 `render_working_region` / `render_recent_ring` 套"每区独特标记 + 区内角色说明 + 每块时间戳 + 明示按时间戳判先后"的外壳,拼成 `system → 工作记忆区 → 8K 最近 ring → 当前回合`(稳定→易变,命中 prompt 缓存)。置换策略在 memory(`context.rs`),此处只做提示词工程。
  - **`finalize()`(收口)**:★**已改(opusx5 2026-06-01)**★ 现在**只抛 `AgentEvent::Done`,不再做飞轮压缩**(原同步 `flywheel.turn` + `FLYWHEEL_IDLE_TIMEOUT=12s` 已删)。经验**采集**仍在前台每步(轻),**提炼/压缩**移到常驻 `IdleWorker`(`idle.rs`),只在静默阈值后、脱离前台回合/锁地做。根治"回合卡死/光标永转"。见记忆 `llm-calls-must-not-block-turn`。
- `bridge.rs::complete()` —— judge_relevant/distill 走的一次性 LLM 收集。**每次 `rx.recv()` 套沉默超时 `COMPLETE_SILENCE_SECS=60`**(流卡住即收手返回已累积内容,调用方 best-effort 降级),不无界阻塞。铁律:任何 LLM 调用都要有沉默超时。
- `registry.rs` —— **执行器注册表**:`Registry::with_builtins(TaskManager)`,统一分发。
- `executors/` —— 各执行器(文件读写、shell、后台任务三工具、finish 等),都实现 core 的 `Executor`(async)。
- `tasks.rs` —— **TaskManager**:后台任务生命周期(spawn/wait/list 三工具共享状态)。
- `supervisor.rs` —— 常驻 Supervisor:监听后台任务完成事件,自动起 agent_loop 回合(限轮防失控)。
- `idle.rs` —— ★opusx5 新增 / P5 加厚★ 常驻 `IdleWorker`(仿 Supervisor):静默 `IDLE_THRESHOLD=8min` 后做两件 idle 工作(顺序即优先级):**A. 睡眠维护(P5)**——疲劳≥0.5 或有碎片债时,逐步 `dream_once`(还碎片债)+ 少量 `rehearse_once`(预热网),每步取仲裁器 `Sleep` 档、逐步让位检查、步数有界(`MAX_SLEEP_STEPS`/`MAX_REHEARSALS`);**B. 飞轮压缩**——取镜像→无锁逐簇 distill(取 `Flywheel` 档)→极短锁写回,可被前台打断。
- `arbiter.rs` —— ★P5 新增·硬前置件★ **潜意识 LLM 仲裁器**:容量1优先级互斥闸 `Priority{Agent<Sleep<Flywheel}`,`acquire`/`acquire_owned`(RAII 守卫,取消安全)。放进 `AppState.arbiter`。真职责=**后台之间**(睡眠 vs 飞轮)的串行+优先级——它们的"想"那拍(慢 LLM)在 AppState 锁外会真并发;`run_chat` 全程持锁 + 取 Agent 档,前台对后台天然互斥且在飞的后台一结束就让位。补遗 `做梦睡眠期也在检索` 要求:造做梦前必须先有它。3 单测(取放/互斥/优先级)。
- `cmds.rs` 的 `connect` 加了 `probe_llm`(发 `max_tokens=1` 最小流式)真探测端点/模型,按 `MODEL_NOT_LOADED/NOT_FOUND/ERROR/API_UNREACHABLE` 返回结构化码(opusx5 修"虚假连接成功")。`connect()` 末尾 `memory.configure_context` 应用 P4 预算设置。
- `bridge.rs` —— 把 memory 的 `Subconscious` / `Embedder` 接到真 LLM 与嵌入实现。
- `state.rs` —— **`AppState`**:data_dir 落盘(redb)、Settings/Projects、`Memory::open(store, &data_dir)`(向量索引 LMDB 落 data_dir/vector-index)、`build_embedder`(按设置选 远程/candle 本地 e5/词法)、项目切换重建 Sandbox。
- `health.rs` —— **异常告知**(`设计文档/异常告知.md`):四级(绿/黄/橙/红);`Store::open` 失败从静默 None 改为红色 Fatal;`get_health` + `get_status.health` + `health-alert` 事件。铁律:严重异常不静默。
- `cmds.rs` —— Tauri 命令(聊天历史/引用上下文/状态/健康/设置等)。历史与引用面板已走时间线惰性 API(`metas()`+`content(id)`)。`pick_directory` 走 macOS osascript(**唯一平台专属代码**,迁 Win/Linux 要改)。**P5 接真**:`run_chat` 整回合取仲裁器 Agent 档;`get_fatigue_level`/`get_status.fatigue+fragment_count`/`get_memory_stats.fatigue` 接 `Memory::fatigue/fragment_count`;`dream_start`(取 Sleep 档跑 `Memory::sleep` 一轮)/`dream_status` 接真;新增 `nap` 命令 = `Memory::nap`(main.rs 注册)。**精确层阶段4 接真**:新增 `reference_history(target,from?)` 命令 = `Memory::pin_history_reference`(用户引用历史钉强制跳转指针,main.rs 注册);`get_memory_stats.secondary_indexes` 的 `total`(二级索引锚点数)/`forced_jumps`(强制跳转数)接真;MemoryViz 面板加"强制跳转"行。
- `debug.rs` —— 调试端点,**feature `debug-endpoints` 开关**(:19999 + debug_eval/e2e_report),正式包不含。

## 前端(`crates/growbox-gui/frontend/`)
- **SolidJS 1.9**(不是 React!改 UI 注意)+ vite 6 + TypeScript 5.6。
- highlight.js(代码高亮)、@solid-primitives/i18n(多语言)。
- 调试桥 `growbox-debug.ts` + App.tsx 钩子,Vite env `VITE_GROWBOX_DEBUG` 开关(正式构建摇树删除)。
- e2e:puppeteer-core(devDep)。

## 用的库
tauri 2、tokio、parking_lot、tracing(+subscriber)、anyhow、async-trait、serde;`build-dependencies` 镜像同套(Tauri 构建期需要)。本地嵌入由 gui feature `local-embed`(默认)转发到 growbox-llm。

## features / 出包
- `default=["local-embed"]`(带 candle e5);`debug-endpoints`(测试包);`--no-default-features`(退词法嵌入,快迭代)。
- 出包:`scripts/build-official.sh` / `build-test.sh`(已先 `npm run build` 重建前端,绕开 `beforeBuildCommand` 空的坑)。**已实证 release 出 .app/.dmg 成功(含 LMDB-C)**。
- `scripts/launch.sh`:二进制名 `growbox`,带前端重建。

## 活的 IDE(UI 操控,`设计/00-交互层` 推论 7;2026-06-02)
LLM 经执行器操控自身 UI 面板,把架构公理从"对文件/shell 的手脚"推广到"对自身界面的手脚"。设计+as-built 见 `计划/活的IDE-UI执行器.md`。
- `ui.rs` —— 共享类型:`UiSurface`(前端声明的可控面板)、`UiSurfaceCatalog=Arc<RwLock<Vec<UiSurface>>>`(目录,**单一真相在前端**,后端只持运行时副本)、`UiAck`(往返回执)、`UiAckRegistry`(id→oneshot 的往返登记表,**独立 Tauri managed state**,避开 AppState 锁——run_chat 持锁 await 往返时 `ui_action_ack` 不能也抢 AppState 锁)。
- `executors/ui_control.rs` —— 一个参数化 `ui_control(target,op)` 执行器(非逐面板逐动作,避组合爆炸)。`definition()` 读目录建动态 `target` enum;合法 `ui_intent()` 返**家族二**意图(`await_ack=true`),非法返 None 落 `execute()` 报错列目录(经失败→perceive 自纠)。`risk=Safe`,走 `registry.dispatch` 正常 execute 路径(脊柱零特判)。
- `agent.rs` —— `EventSink` 加 `ui_round_trip`(家族二往返:发请求等前端回执返**验证态**,默认无前端=未应用);`Dispatch::Intent` 按 `UiIntent.await_ack` 分流(家族二往返 / 家族一 fire-and-forget)。core `UiIntent` 加 `await_ack` + `hand_off()`/`round_trip()`(往返没做成 core UiBridge trait,因 ExecCtx 够不到 sink、EventSink 本就是脊柱↔前端唯一抽象)。
- `cmds.rs` —— `TauriSink::ui_round_trip`(emit `ui-action{id}` + 3s 超时);命令 `register_ui_surfaces`/`ui_action_ack`(走独立 UiAckRegistry 不触 AppState 锁)/`ui_state_changed`;`get_control_state.ui_panels` 上浮真实可见态;`SYSTEM_PROMPT` 加 ui_control 一句。
- `state.rs` —— `AppState.ui_catalog`(交给 ui_control)+ `ui_panel_state`(可见态缓存)+ `note_ui_panel`(真实翻转才 perceive,去噪);`Registry::with_builtins_catalog`(真 app 带目录;`with_builtins` 保留=空目录,测试零改)。
- 前端 `ui-actions.ts` —— 后端"一注册表一分发"的镜像:`PANELS` 声明式注册表(memory/dream/health/history)+ 单一 `dispatchUiAction`(替掉 App.tsx if/else,收编 5 legacy)+ `registerUiSurfaces`(mount 上报)+ `watchPanelsAndReport`(`createEffect` 监视各 isOpen 自动上报变化,含用户手动)。`App.tsx` listen 体一行 dispatch;`tauri-api.ts` 三包装;`MemoryViz`/`DreamPanel` 导出 isOpen getter。**偏差**:未做中央 store,用 PANELS 注册表统一访问既有 signal(架构意图达成、少风险,如实记)。

## 现状
lib **70 单测** + e2e 绿(活的 IDE +13:ui_control 7 / 往返 2 / ui registry 3 / note_ui_panel 1;P5 arbiter 3)。`live_ui_control` ignored(需 key + 真 GUI 手验)。
