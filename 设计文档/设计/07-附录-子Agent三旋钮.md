# 附录 · 子 Agent 三旋钮(派生即类型化函数调用)

> 本附录是 `07-工作流机制.md`「子 Agent = 三旋钮的函数调用」一节的工程实现展开 + 能力矩阵。
> 一句话:**一个派生子 Agent = 一次类型化函数调用,由三个正交旋钮定义 —— 看什么(上下文域)/ 能做什么(工具域)/ 返回什么(返回契约)**;常说的 fork、调查员、审核员只是这些旋钮的几组预设取值,不是几个互不相干的机制。

## 三(四)个旋钮

| 旋钮 | 取值 | 机制落点 |
|---|---|---|
| ① 看什么(上下文域) | inherit / isolated / fork | `context_mode` → `WfFrame.isolated/fork` + `context_floor` 切片(见 07 上下文三态节) |
| ② 能做什么(工具域) | 全工具 / 节点工具子集(如只读) | 节点 `tools` 白名单 → 唯一执行闸门 `tool_in_scope`(见 `03-安全审查` 推论6 工具可见性闸) |
| ③ 返回什么(返回契约) | digest 摘要 / full 直通 | `return_spec` + `workflow_return{value, full}`(07 原则4) |
| ④(隐含)用哪个模型 | 主模型 / 便宜潜意识模型 | 潜意识模型槽(自洽分支可降配,省成本) |

三个旋钮**正交**:任意组合都合法,不互相牵制。fork / 调查员 / 审核员 = 旋钮的几组常用取值。

## 旋钮①的本质:两个正交开关

`context_mode` 不是三选一的整体,而是**两个二元开关**的组合:

| | 退出**保留**分支工作 | 退出**截断**分支工作(只回摘要) |
|---|---|---|
| 分支**看得到**父上下文 | `inherit`(默认) | **`fork`** |
| 分支**看不到**父上下文 | (无意义,不暴露) | `isolated` |

- **隐藏父**(进入时抬 `context_floor` = 本帧 `msg_base`):仅 isolated。
- **退出截断**(出栈/返回时 `messages.truncate(msg_base)` 丢分支 append 的工作消息):isolated 与 fork。
- `full=true` 是对"退出截断"的特例豁免(原始工作上下文直通回父,零 LLM 搬运,慎用,易污染主上下文)。

> **fork = inherit 的"看" + isolated 的"丢"**:继承父全量上下文(它什么都知道)、但退出只把摘要交回(机械过程噪音不回灌父)。这正是"繁重子任务要全背景、但别拿过程噪音污染我"的解。

## 两个常用预设

| 预设 | ① 看 | ② 做 | ③ 回 | ④ 模型 | 典型场景 |
|---|---|---|---|---|---|
| **fork** | inherit/fork(继承全境) | 全工具或按需 | digest | 主模型 | 需我全部背景的繁重机械活:大范围重构、反复编译验证 |
| **调查员 / 审核员** | isolated(只看 input) | 只读工具子集 | digest | 可降配 | 自洽子任务:查 X 在哪、审这段 diff、核某断言;可并行扇出 |

> 两个预设的差别**只在旋钮①**(看不看得到父)——其余可同(都 digest、都可只读)。这就证明 fork 与调查员不是两个机制,是同一函数调用的两组取值。

## 何时派生 · 用哪个预设(决策规则,教给模型)

1. **值不值得外派?** 会产生大量工具噪音(读一堆文件 / 反复编译 / 全库搜)、能并行、或有干净交付物 → 外派;否则就地做。
2. **需不需要我此刻积累的全部上下文(计划、已定决策)?**
   - 需要(是我工作的延续)→ **fork**。
   - 不需要(自洽查询,看一眼代码 + 一份简报就能答)→ **调查员 / 审核员**。
3. **三条提醒**(真容易错的地方):
   - 成本不对称,**默认调查员**(fork 每轮背全上下文,贵);
   - **欠简报是调查员头号失败** —— 隔离上下文意味着要把它需要的都写进 `input`;若发现"要写巨长简报才够",这本身就是该改 fork 的信号;
   - 调查员可并行扇出(一回合发多个 isolated 调用 → 并发跑、各回摘要,机制见 `07-附录-并行子代理.md`),fork 通常串行。

## as-built 映射(`crates/growbox-gui/src/agent/mod.rs` + `workflow_store.rs`)

- `context_mode` 解析:`isolated = (cmode=="isolated")`、`fork = (cmode=="fork")`,二者互斥(同一字符串);都不是 = inherit。入口工具 schema 的 `context_mode.enum = ["inherit","isolated","fork"]`(`workflow_store.rs` wf_tool_def)。
- `WfFrame { isolated, fork, .. }`:`isolated` 管"隐藏父"(进入时 `if isolated { context_floor = msg_base }`);`fork` 与 `isolated` **共同**管"退出截断"(出栈返回、END 出栈、直接调用替换帧三处:`if isolated || fork { truncate(msg_base) }`,`full=true` 豁免)。
- 旋钮②(工具域)由脊柱循环 + `dispatch` 闸门按节点可用集 `Registry::tool_in_scope` 校验(见 `03` 推论6),**与上下文模式正交**——任何 context_mode 都可叠加任意工具子集。
- 单测:`agent/loop_tests.rs` —— `workflow_isolated_hides_parent_and_discards_on_return`(隐藏父 + 截断)/ `workflow_fork_inherits_parent_but_discards_on_return`(继承父 + 截断)/ `workflow_return_full_passthrough_does_not_truncate`(full 直通不截断)。

## 设计沿革:为什么 fork 要补一刀

旧实现把"隐藏父"与"退出截断"**耦合**在 `isolated` 一个开关里(截断条件写死 `isolated && !full`),于是"看得到父 + 退出截断"这个组合(= fork)**不可达**。补 fork = **解耦这两个开关**(加 `fork` 位,截断条件放宽成 `(isolated || fork) && !full`),fork 这个角自然落出。一处轻量解耦,**不动 inherit/isolated 既有语义**(它们 fork=false,行为逐字不变)。

> 教训(可复用):当一个枚举/开关同时控制两件正交的事,新需求往往就是"那两件的另一种组合"。先看能不能**解耦**成独立开关,而不是加第三个并列分支 —— 解耦后新组合是免费的。
