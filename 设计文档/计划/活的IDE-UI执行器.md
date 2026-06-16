# 活的 IDE — UI 执行器工程设计

> 原理层:`设计/00-交互层` 推论 1/6/7;架构公理:`系统架构/00`(一切能力皆执行器、一个注册表一条分发路径)。
> 用户决策逐字:`用户决策/决策日志.md` 2026-06-01「活的 IDE」条。
> 本文是工程层设计稿(写代码前的第一性原理设计),未实现。
> 2026-06-01 优化:按架构"好"的标尺自审,修两处缺陷(目录跨语言重复 / fire-and-forget 虚假成功)。v1 草稿见 History 注记。

## 目标

让 LLM 像操控自己身体一样操控 GrowBox 的 UI/自身界面状态:用户想关某面板,LLM 直接关,省去用户找按钮/点击。把"一切能力皆执行器"从"对文件/shell 的手脚"推广到"对自身界面的手脚"。

## 一句话现状:链路已通 90%

脊柱 `registry.dispatch`(`registry.rs:76`)是唯一入口;`AgentEvent::Intent(UiIntent{action,prefill})`(`executor.rs:70`)→ `TauriSink` emit `"ui-action"`(`cmds.rs:65`)→ 前端 `listen("ui-action")`(`App.tsx:71`)是活的;已有 `OpenSettings`/`CreateProject` 两个交互类执行器范例。**缺的不是链路,是抽象**——见下。

## 第一性原理:身体的解剖 + 运动词汇,而非动作清单

UI 是 AI 的身体。身体有**固定解剖**(若干面板/界面元素)+ **固定运动词汇**(开/关/切换……)。LLM 通过"对某个部位施加某个动作"动身体。

这条原理**否定**逐面板逐动作建执行器(`CloseMemoryPanel`/`ToggleDreamPanel`…):那是 N 面板 × M 动作的组合爆炸。项目里已有反例为证:`OpenSettings` 注释——"所有设置项……统一一个执行器 + 一个 field 标识"。设置项没做成每项一个执行器。本设计 = 把这个已验证模式推广到所有 UI 面。

## 核心模型(优化后):前端是 UI 事实的唯一权威,后端只"请求"与"记录"

UI 是前端的东西。所以**"有哪些面板、各自什么状态"由前端独家作者**;后端**从不发明 UI 事实**——它只能(1)请求前端改某个面板,(2)记录前端确认的结果。两条铁的推论:

- **目录单一真相**:面板目录(解剖)由前端声明给后端,后端只持运行时派生副本(像缓存),不另存一份手写枚举 → 零漂移。
- **状态不撒谎**:后端关于"面板开没开"的认知,只来自前端确认;`ui_control` 的"成功"必须是前端回报的**验证过的**状态,不是"已发出请求"。

这个模型把原 v1 的两处缺陷一并解决(详见末节"为什么这版更好")。

## 两个执行器家族:本质不同,故两套机制(关键修正)

原 v1 把"Agent 关面板"塞进了"给用户弹表单"的同一机制,于是把**本可接受的 fire-and-forget** 误用成了**虚假成功**。正解是按本质区分:

| | 家族一:交付用户裁决(hand-off) | 家族二:Agent 自己对 UI 动手(control) |
|---|---|---|
| 例 | `OpenSettings` / `CreateProject` / `AddPath` / suggestion / choice | `ui_control(open/close/toggle 面板)` |
| 谁决策 | 用户(弹出预填表单,用户填了再定) | Agent(无人介入) |
| 机制 | `ui_intent()` 短路,emit 即返回 | `execute()` **往返**:请求→前端落地→ack→返回验证态 |
| "成功"含义 | "已把表单呈现给用户"(**诚实**,裁决在带外) | "面板确已关闭,open=false"(**验证过**) |
| 改动 | 不动,保留各自富预填 schema | 新建 |

要点:家族一的 fire-and-forget 是**对的**(它只声称"已呈现");家族二必须往返,否则就是项目反复打的"虚假成功",还会被飞轮当真结论学进永久记忆。

## 三根支柱(优化版)

### A. 后端:一个 `ui_control` 执行器,目录不自带、往返不撒谎

- 形态:`ui_control(target, op)`,`op ∈ {open, close, toggle}`(只收**改变状态、可验证**的动词;focus/scroll/toast 属家族一,`OpenSettings` 已做 scroll,不进此处 = YAGNI)。
- **schema 动态生成**:`UiControl` 持 `Arc<RwLock<SurfaceCatalog>>`(前端声明填充),`definition()` 读它把 `target`/`op` 列成 enum + 人话描述。`ui_intent()` 返回 `None`(不走家族一短路);`execute()` 先校验 `target/op∈目录`,非法即 `ToolResult::fail` 列出合法值(经失败→`perceive` 让 LLM 自纠)。
- **execute() 往返**:合法则 `ctx.ui.round_trip(intent)` → 拿 `UiAck{applied,state}` → 返回验证态文本。`ctx.ui` 为 None(无前端/测试)时诚实失败。
- `risk()=Safe`,走 `registry.dispatch` 正常 execute 路径——**ui_control 就是"另一个执行器",脊柱零特判、零新 Dispatch 变体**(比 v1 更纯)。

### B. 前端:单一 dispatcher + 声明式 PANELS + 中央 uiState

- 现状隐患:`App.tsx:71` 的 `if/else action` 链已 5 条且在长 = 前端版"三套分发打架"。
- 正解(后端公理在前端的镜像):
  - **中央 `uiState`**(store.ts):面板可见态单一真相;**把 MemoryViz/DreamPanel/HealthIndicator 各自本地 signal 迁进来**(消除散落)。
  - 声明式 **`PANELS`** 注册表(`ui-actions.ts`):`{ [id]: { open?(data), close?(), toggle?(), focus?(field) } }`——**它就是目录权威**,mount 时序列化上报后端(支柱 C)。
  - 单一 `dispatchUiAction(action, data, id?)`:`action==="ui_control"` → 查 `PANELS[target][op]()`,完成后带 `id` 回 `ui_action_ack`;5 个 legacy 扁平 action 映射到同批 handler。**替掉 if/else,删旧不留两套。**

### C. 双向通道:前端是后端关于 UI 的"信息源"(一举解决撒谎 + 感知)

三个前端→后端调用,把 UI 事实的作者权与确认权交还前端:

1. **`register_ui_surfaces(surfaces)`**(mount 时):前端声明解剖 → 后端填 `SurfaceCatalog` → `ui_control` 动态 schema。**单一真相、零漂移。**
2. **`ui_action_ack(id, ack)`**(每次 ui_control):前端落地后回报结果态 → 解锁后端往返的 oneshot → `execute()` 返回验证态。**不撒谎。**
3. **`ui_state_changed(panelId, open)`**(任何 uiState 变化,**含用户手动**开关):后端更新缓存 → `get_control_state` 恒真 + `perceive` 让 agent 看见**用户**的 UI 动作。**感知无盲区**,ack 缝彻底闭合(不再是债)。

## 机制细节(落地要点)

- **core(零依赖)**:加 `UiBridge` trait(`async fn round_trip(&self, intent: UiIntent, timeout_ms: u64) -> UiAck`)+ `UiAck{applied:bool, state:serde_json::Value, note:Option<String>}`;`ExecCtx` 加 `ui: Option<&dyn UiBridge>`。core 仍只持 trait + serde 类型,不碰 Tauri。
- **gui**:`TauriUiBridge impl UiBridge`——持 `AppState` 里的 `ui_acks: Mutex<HashMap<id, oneshot::Sender<UiAck>>>` + app handle;`round_trip`=生成 id、插 oneshot、emit `"ui-action"{id,target,op}`、await 接收(**短超时**,接 [[llm-calls-must-not-block-turn]] 沉默超时铁律;超时=诚实失败 + perceive)。`ui_action_ack` 命令按 id 取 sender 投递。
- **dispatch 注入**:`agent_loop` 持 `UiBridge` 句柄,`registry.dispatch` 把它放进 `ExecCtx.ui`。
- **wire 统一**:仍是**一个 `"ui-action"` 事件 + 前端一个 listener + 一个 dispatchUiAction**。家族二带 `id`(需 ack),家族一无 `id`(直接落地)。后端两处 emit 来源(家族一经 `TauriSink`、家族二经 `TauriUiBridge`),但同一 wire 契约。
- **工具表时机**:`ui_control` 的动态枚举要到达 LLM,要求 agent 每回合从 registry 现取工具定义(Phase 1 需核实非启动期缓存)。

## 为什么这版更好(对照"什么是好架构"的标尺)

- **单一真相**:目录前端独家作者、后端运行时派生 → 跨语言重复消除,零漂移(原 v1 降级成"手工对齐+记账",此版根治)。
- **不撒谎**:`execute()` 返回验证态;**往返同时把感知做真**——支柱 C 不再是装饰(原 v1 的 fire-and-forget 把感知架空了)。
- **概念完整性更强**:`ui_control` 走正常 execute 路径,脊柱零特判、无新 Dispatch 变体——比 v1 更贴公理。
- **本质区分到位**:两家族两机制,fire-and-forget 与 round-trip 各用在对的地方。
- **YAGNI**:`ui_control` 只收状态动词(open/close/toggle);focus/scroll/toast 归家族一。
- **服务产品目的**:Agent 对自身 UI 的感知与掌控随用增强,且不引入它看不见的盲区(含用户手动操作)——正中 GrowBox"越用越强 + 感知一切"的承重。

## 完整执行计划

原则:每 Phase 收尾必须 `cargo test --workspace` 全绿 + `cargo clippy --workspace --all-targets` 零警告 + 前端 tsc/build 干净。构建前先 `source /Volumes/UserData/Claudex5/env.sh`([[claudex5-build-env]])。

### Phase 1 — 后端:UiBridge 往返机制 + ui_control + 动态目录
- core:`UiBridge` trait + `UiAck` + `ExecCtx.ui`。
- gui:`TauriUiBridge`(ack registry / emit / 超时);`SurfaceCatalog`(`Arc<RwLock>`,放 AppState);`register_ui_surfaces`/`ui_action_ack` 命令(main.rs 注册);`registry.dispatch` 注入 `UiBridge`。
- `executors/ui_control.rs`:读目录建 schema、execute 往返、非法报错带目录。
- 单测:往返成功(mock UiBridge)/ 超时诚实失败 / 非法 target / 校验通过路径。确认工具定义每回合现取。
- 收尾:cargo 全绿 + clippy 零。

### Phase 2 — 前端:中央 uiState + 表驱动 dispatcher + 接 ui_control
- store.ts 中央 `uiState`,迁入 Memory/Dream/Health 散落 signal。
- 新建 `ui-actions.ts`:`PANELS` 注册表 + `dispatchUiAction`(ui_control 分支完成后回 `ui_action_ack`;5 legacy 收编)。
- mount 调 `register_ui_surfaces(PANELS 导出的目录)`。
- App.tsx `listen("ui-action")` 体改为一行 dispatch,删旧 if/else。
- 现有 5 action 行为不回归(有 vitest 则表驱动单测)。收尾 tsc/build 干净。

### Phase 3 — 感知闭合:ui_state_changed + get_control_state 恒真
- 前端任何 uiState 变化(含用户手动)→ `ui_state_changed`;后端更新缓存 + `perceive`。
- `get_control_state` 上浮各面板真实可见态。**ack 缝至此闭合,不留债。**
- 收尾:cargo 全绿。

### Phase 4 — 系统提示 + live + 文档/记忆收口
- `SYSTEM_PROMPT` 加一句 ui_control 控面板可见性。
- live(测试 key 见 [[test-api-key-endpoint]]):真 LLM"关掉记忆面板" → 面板真关 **且 LLM 收到"已关 open=false"验证结果**;新增 `live_ui_control` 或手测留证。
- 文档收口(改既有文档前按 [[doc-history-backup-rule]] 备份):`具体系统设计/gui` as-built、`系统架构/01-core`(ExecCtx.ui/UiBridge)、决策日志落点、记忆快照同步。

## 测试策略

- Rust:`ui_control` 往返单测(mock UiBridge:成功返验证态 / 超时返诚实失败 / 非法 target 列目录);确认走 `registry.dispatch` 不被特判。
- 前端:dispatcher 表驱动单测;5 legacy action 不回归。
- live:真 LLM 自然语言关面板 → 真关 + 收到验证结果,闭环。

## "保持最佳态"硬约束(每 Phase 都查)

1. 全绿:`cargo test --workspace` + 前端 tsc/build。
2. clippy 零警告(全工作区,不分谁引入都修)。
3. 无按名拦截:`ui_control` 只经 `registry.dispatch` 正常 execute 路径。
4. 无历史包袱:前端 if/else 删干净;signal 迁中央后删旧的;不留两套。
5. 不撒谎:`ui_control` 只返回前端确认的验证态;无前端/超时=诚实失败 + perceive。
6. 文档备份规矩:改既有设计文档前先 History 备份。

## as-built(2026-06-02,Phase 1-4 全部实现完,全绿)

四个 Phase 全落地,`cargo test --workspace` 全绿(gui lib 70)、`clippy --workspace --all-targets` 零警告、前端 `tsc -b && vite build` 干净。**与上文设计的两处偏差(更省/更贴码,如实记):**

1. **往返机制做在 `EventSink` 扩展,不是 core 加 `UiBridge` trait**。读码发现 `ExecCtx` 只有 args/work_dir、够不到事件 sink,而 `EventSink` 本就是"脊柱↔前端"的唯一抽象。故:core 只给 `UiIntent` 加 `await_ack: bool` + `hand_off()`/`round_trip()` 构造器;往返做成 `EventSink::ui_round_trip`(默认返回未应用),脊柱 `Dispatch::Intent` 按 `await_ack` 分流。比原案少一个 core trait、零改 `ExecCtx`/`dispatch`/`agent_loop` 签名(测试调用点零冲击),且 `ui_control` 仍是"普通执行器走正常 execute 路径"(脊柱零特判)。两家族区分由 `await_ack` 标志承载(家族二 round_trip / 家族一 hand_off)。
2. **前端没用"中央 store",改用 `ui-actions.ts` 的 `PANELS` 注册表统一访问**。`PANELS[id]` 提供 `isOpen/open/close/toggle`,接现有 signal(memory/dream 从组件导出 getter、health/history 在 store)。架构意图(单一分发 + 单一读点 + LLM 可读)达成,但**少了把各组件 signal 物理迁进一个 `createStore` 的大改动与风险**。这是"means(中央 store)≠ ends(单一访问)"的务实取舍。感知用 `createEffect` 监视各 `isOpen` 自动上报 `ui_state_changed`,捕获用户/Agent 任何变化,无需区分来源。

**面板集**:声明 memory/dream/health/history 四个可开关可视化面板(`control` 是常驻面板、不可开关,未列入)。

**实现要点对应**:core `executor.rs`(UiIntent await_ack);gui `ui.rs`(UiSurface/UiSurfaceCatalog/UiAck/UiAckRegistry)、`executors/ui_control.rs`、`agent.rs`(EventSink::ui_round_trip + Intent 分流)、`cmds.rs`(register_ui_surfaces/ui_action_ack/ui_state_changed + SYSTEM_PROMPT 一句)、`state.rs`(ui_catalog + ui_panel_state + note_ui_panel)、`main.rs`(manage UiAckRegistry + 注册三命令)。前端 `ui-actions.ts`/`App.tsx`(一行分发)/`tauri-api.ts`(三包装)/`MemoryViz`+`DreamPanel`(导出 getter)。

**ack 缝**:Phase 3 已闭合(`ui_state_changed` 上报含用户手动 + `get_control_state.ui_panels` 恒真),`空桩与待接真登记` 已更新,不留债。

**端到端边界(诚实)**:`live_ui_control`(默认 ignored,需 DEEPSEEK_API_KEY)验真 LLM 选对工具 + 真往返 + 验证态回填(前端用会 ack 的 sink 模拟)。**真实 GUI 把 DOM 面板关掉那一下 headless 测不了**,需用户跑起 app 手验(机械可靠:前端 dispatchUiAction→PANELS[target][op]()→signal 变→Show 收起面板)。
