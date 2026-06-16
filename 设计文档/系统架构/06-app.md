# 06 — app

## 职责
组装一切:**Agent 循环脊柱** + 执行器实现与注册表 + Tauri 命令 + 前端(交互复用旧设计)。不管各 crate 内部实现。

## 接口
```rust
// 执行器注册表:一处登记,一条分发
pub struct Registry { execs: HashMap<String, Box<dyn Executor>> }
impl Registry { pub fn dispatch(&self, call: ToolCall, safety: &Sandbox, ...) -> ToolResult; }

// Agent 循环脊柱
pub async fn agent_loop(user_msg: &str, state: &AppState) -> AgentResult;

// Tauri commands(薄壳,转发给上面)
#[tauri::command] async fn send_message_stream(...) -> ...;
#[tauri::command] async fn connect(...) -> ...;
```

## 依赖
→ 依赖:core、llm、memory、safety、learn(全部)。 ← 被依赖:无(顶层)。

## 数据流(Agent 循环 = 全局脊柱)
```
用户意图
 ①组装上下文 memory.retrieve + 系统提示词
 ②调 LLM     llm.chat_stream(reasoning/截断/方言)
 ③执行器分发 Registry.dispatch → safety.judge(可逆直跑/越界弹授权)
 ④感知/验证  收结果 → 自动纠错(≤3)
 ⑤学习       learn.collect(异步) → idle: learn.turn
 → 回①  或  完成/失败/等待用户
```

## 接原理
- `系统架构/00` 架构公理:Registry 一处分发;agent_loop 唯一脊柱。
- `设计/00` 控制反转:执行器 `ui_intent` 弹预填 UI,前端=AI 的身体。
- `设计/01`/`04`/`05`:循环 ④⑤ 接感知与飞轮。

## 已知坑(旧 gui 的系统病,本次根治)
- 三套分发打架 → 单一 Registry.dispatch。
- agent/ 空骨架 + cmds.rs 4827 行单体 → 脊柱在 agent_loop,cmds.rs 仅薄壳。
- 文件操作:macOS 沙箱阻断同步 I/O → 全部 `thread::spawn + recv_timeout`(此条旧代码经验,**待本工作区实验复核**后采用)。
- 沉默超时:把 reasoning chunk 计入"有活动"(`实验记录/00`)。
