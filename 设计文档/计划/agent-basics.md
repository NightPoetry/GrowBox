# Agent 基本盘实现计划

> 目标:让 GrowBox 具备 Agent 核心能力——后台任务、学习闭环、可启动。

---

## 总览:4 个阶段,按依赖排序

| 阶段 | 内容 | 改动范围 | 预计影响 |
|------|------|----------|----------|
| 1 | launch.sh 修复 | 1 个脚本 | 立即可用 |
| 2 | 后台任务三工具 + agent_loop 集成 | agent.rs, state.rs, cmds.rs | Agent 能异步工作 |
| 3 | 常驻 Supervisor | supervisor.rs(新), state.rs, cmds.rs | Agent 能被后台任务唤醒 |
| 4 | 飞轮 idle 学习闭环 | agent.rs | Agent 能从经验中提炼知识 |

---

## 阶段 1:launch.sh 修复

**问题**:`scripts/launch.sh` 有两处 bug:
1. `BIN="target/release/growbox-gui"` -- 实际二进制名叫 `growbox`(见 `Cargo.toml` [[bin]] name)
2. 文件含不可见字符导致 `line 41: BIN: unbound variable`

**修复**:
- 第 24 行:`BIN="target/release/growbox"`
- 第 37 行 `pkill -f "growbox-gui"` → `pkill -f "growbox"`
- 第 41 行的 `$BIN` 引用确保无不可见字符(重写该行)
- 第 49 行 debug-server 端口 `7891` → 改为与 `debug.rs` 一致的端口(需确认;如果是正式包则无 debug-server,跳过等待)

**验证**:`bash scripts/launch.sh --no-build` 能正确启动(假设已 build)。

---

## 阶段 2:后台任务三工具 + agent_loop 集成

### 2.1 核心设计决策

**异步 vs 同步矛盾**:当前 `Executor::execute()` 是同步的,但 `wait_tasks` 必须 async(`wait_event()` 是 async)。解决方案:**任务三工具由 agent_loop 按名拦截,不走 Registry**。这与交接报告的设计一致。

安全门:spawn_task 的 command 在 spawn 前过 `sandbox.judge(Operation::Shell(cmd))` + `risk_gate`,后台不绕安全。

### 2.2 文件改动清单

#### (A) `crates/growbox-gui/src/state.rs` -- AppState 加 TaskManager

```rust
// AppState 新增字段:
pub task_mgr: Arc<TaskManager>,

// AppState::new() 中初始化:
task_mgr: TaskManager::new(),
```

#### (B) `crates/growbox-gui/src/agent.rs` -- agent_loop 接收 TaskManager + 拦截三工具

**签名变更**:
```rust
pub async fn agent_loop(
    // ... 现有参数不变 ...
    task_mgr: &Arc<TaskManager>,  // 新增
) -> AgentOutcome
```

**工具定义注入**:在 `registry.definitions()` 之后,追加三个任务工具的 ToolDef:
```rust
let mut tools = registry.definitions();
tools.extend(task_tool_definitions());
```

**分发拦截**:在 tool_calls 的 for 循环里,registry.dispatch 之前:
```rust
for call in &outcome.tool_calls {
    // 先检查是否是任务工具
    if is_task_tool(&call.name) {
        let result = handle_task_tool(call, task_mgr, sandbox, work_dir).await;
        // 回填结果到 messages,跳过 registry dispatch
        ...
        continue;
    }
    // 否则走原有 registry dispatch 逻辑
    ...
}
```

**三工具实现**:

1. `spawn_task { command, label, done_when }`:
   - 解析 done_when(字符串 → DoneWhen 枚举:"exit" → Exit, "file:PATH" → FileExists, "port:NUM" → PortOpen, "probe:CMD" → Probe)
   - **安全门**:`sandbox.judge(&Operation::Shell(&command))` → 拦截则返回失败
   - `task_mgr.spawn_shell(label, command, work_dir, done_when)`
   - 返回 `ToolResult::ok(format!("后台任务已启动: {id}"))`

2. `wait_tasks`:
   - 指数退避循环:`base=2s, cap=60s`
   - `tokio::select! { task_mgr.wait_event() => drain and report, sleep(backoff) => reap_hung + double backoff }`
   - 任一完成 → reset backoff
   - 返回所有已完成/卡死任务的摘要

3. `list_tasks`:
   - `task_mgr.snapshot()` → 格式化返回

**测试**:扩展 agent.rs 的 Scripted mock,添加 spawn_task + wait_tasks 场景。

#### (C) `crates/growbox-gui/src/cmds.rs` -- 传递 TaskManager

`run_chat` 中,从 `st.task_mgr` 取出传给 agent_loop:
```rust
let outcome = agent_loop(
    message, &cfg, llm.as_ref(), &st.registry, &st.sandbox,
    &mut st.memory, bridge.as_ref(), &st.flywheel,
    &work_dir, sink, &st.task_mgr,  // 新增
).await;
```

**系统提示词更新**:在 SYSTEM_PROMPT 中加入任务工具说明:
```
你可以用 spawn_task 启动后台命令(构建、测试、起服务),用 wait_tasks 等待完成,用 list_tasks 查看状态。
后台任务在安全沙箱内运行,危险命令会被拒绝。
```

`tool_start_text` / `tool_end_text` 补充 spawn_task / wait_tasks / list_tasks 的自然语言描述。

---

## 阶段 3:常驻 Supervisor

### 3.1 设计

Supervisor 是 app 生命周期级的后台 tokio 任务,在 `connect` 时启动:
- 持有 `Arc<TaskManager>` + 共享 `AppState` 锁
- 前台空闲时 `wait_event`,任务完成 → 抢锁 → 发起一个 agent_loop 回合
- 前台有回合在跑 → 共享锁天然让位,等当前回合结束才轮到
- 用 `tokio_util::sync::CancellationToken` 实现可取消(断开连接时取消)

### 3.2 文件改动

#### (A) `crates/growbox-gui/src/supervisor.rs` -- 新文件

```rust
pub struct SupervisorHandle {
    cancel: tokio_util::sync::CancellationToken,
    join: tokio::task::JoinHandle<()>,
}

impl SupervisorHandle {
    pub fn spawn(task_mgr: Arc<TaskManager>, state: SharedState, app: AppHandle) -> Self { ... }
    pub fn cancel(&self) { self.cancel.cancel(); }
}
```

核心循环:
```rust
loop {
    tokio::select! {
        _ = cancel.cancelled() => break,
        _ = task_mgr.wait_event() => {
            let finished = task_mgr.drain_finished();
            if finished.is_empty() { continue; }
            // 合成消息发起 agent_loop
            let msg = format_task_completion(&finished);
            let mut guard = state.lock().await;
            // ... 调 agent_loop ...
            // 把结果 emit 给前端
        }
    }
}
```

#### (B) `crates/growbox-gui/src/state.rs` -- 存储 SupervisorHandle

```rust
pub supervisor: Option<SupervisorHandle>,
```

#### (C) `crates/growbox-gui/src/cmds.rs` -- connect 时启动 Supervisor

在 `connect` 命令里,启动 supervisor:
```rust
// 取消旧的(如果有)
if let Some(old) = st.supervisor.take() { old.cancel(); }
// 启动新的
st.supervisor = Some(SupervisorHandle::spawn(
    st.task_mgr.clone(), state_clone, app.clone()
));
```

#### (D) `crates/growbox-gui/src/lib.rs` -- 声明 supervisor 模块

```rust
pub mod supervisor;
```

#### (E) `Cargo.toml` -- 加 `tokio-util` 依赖

```toml
tokio-util = { version = "0.7", features = ["rt"] }
```

**注意**:Supervisor 自动发起 agent_loop 是行为变化最大的部分。建议先实现但加一个开关(设置项 `auto_supervisor: bool`,默认 true),让用户可关闭。

---

## 阶段 4:飞轮 idle 学习闭环

### 4.1 设计

在 agent_loop 里,任务完成后(无论是 finish 收口还是正常结束),调一次 `flywheel.turn()` 把积累的经验压缩成知识。这是 "学习" 的最小闭环。

### 4.2 文件改动

#### `crates/growbox-gui/src/agent.rs`

在 `agent_loop` 的两个退出点(finish 命中 + 空转兜底)前,加入 idle 学习:

```rust
// 任务完成,收口前转一轮飞轮:把积累的经验压缩成知识。
// 异步但不阻塞用户(finish 后本就要退出)。
let learned = flywheel.turn(memory, subconscious_as_reasoner).await;
if learned > 0 {
    sink.emit(AgentEvent::Notice(format!("从经验中提炼了 {learned} 条知识"))).await;
}
```

**问题**:`flywheel.turn()` 需要 `&dyn Reasoner`,但 agent_loop 的 `subconscious` 参数是 `&dyn Subconscious`。`LlmBridge` 同时实现了两个 trait,但传进来时只暴露了 Subconscious 面。

**解决**:给 agent_loop 加一个 `reasoner: &dyn Reasoner` 参数(或把 Subconscious trait 扩展为包含 Reasoner 能力)。最简方案:新增参数 `reasoner: &dyn Reasoner`。

**签名最终变更**:
```rust
pub async fn agent_loop(
    user_msg: &str,
    cfg: &AgentConfig,
    llm: &dyn LlmDriver,
    registry: &Registry,
    sandbox: &Sandbox,
    memory: &mut Memory,
    subconscious: &dyn Subconscious,
    reasoner: &dyn Reasoner,     // 新增
    flywheel: &Flywheel,
    work_dir: &Path,
    sink: &dyn EventSink,
    task_mgr: &Arc<TaskManager>, // 新增
) -> AgentOutcome
```

**测试**:扩展现有测试,加一个 mock Reasoner,验证 turn 被调用。

---

## 测试策略

每个阶段完成后:
1. `cargo test --workspace --lib` 全绿(预计从 93 增长到 ~110+)
2. 阶段 2 完成后可用真机 `live_agent` 测试 spawn_task → wait_tasks 流程
3. 阶段 4 完成后验证:多次操作后,conclusions 列表中出现压缩率 > 0 的知识级结论

---

## 不做的事(本轮)

- 精确层飞轮加厚(指针网络/三级缓存/二级索引/碎片)—— 这是记忆系统的优化,不是 Agent 基本盘
- 做梦/睡眠 —— 依赖精确层先就位
- 前端 Supervisor 事件面板 —— 后端先行,前端可后补
- 记忆面包屑(spawn 时写 memory)—— 可后补,先让核心流程跑通

---

## 实施顺序

1. **launch.sh** -- 5 分钟,立刻修
2. **阶段 2** -- 后台任务三工具 + agent_loop 集成(最大工作量)
3. **阶段 4** -- 飞轮 idle 学习(改动小,和阶段 2 的签名变更一起做)
4. **阶段 3** -- Supervisor(依赖阶段 2 完成,且行为变化最大,放最后)

阶段 2 和 4 可以合并在一次改动里完成(都改 agent_loop 签名)。
