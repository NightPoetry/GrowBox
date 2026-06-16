# 01 — core

## 职责
只管**全局共享类型**(结论、经验、执行器接口、上下文窗口、项目/设置数据结构、Scope);不管任何具体逻辑(检索、调用、执行都在上层)。

## 接口
```rust
// 结论:经验/知识/理解同一模型,压缩率连续谱
pub struct Conclusion {
    pub id: String,
    pub compression: f32,          // 0~1,飞轮自动调
    pub prerequisites: Vec<String>,
    pub operation: String,
    pub expected: String,
    pub source: String,
    pub confidence: Confidence,    // 算出来的,非标签
    pub superseded_by: Option<String>,
}
pub enum Confidence { Experience, Knowledge{sup:u32,con:u32}, Understanding{verified:u32,total:u32} }

// 执行器:一切能力的统一形态(详见 06-app 注册/分发)
#[async_trait]
pub trait Executor: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDef;       // 给 LLM 的 schema
    fn risk(&self) -> Risk;                // 可逆/风险等级 → safety
    fn ui_intent(&self, args: &str) -> Option<UiIntent> { None } // 交互类:预填 UI
    fn terminal(&self) -> bool { false }   // 终止类(finish):命中即收口循环
    fn claim(&self, args, work_dir) -> Option<Claim> { None } // 本次动什么资源 → safety 单门
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult; // async:异步能力也是一等执行器
}
pub enum Risk { Safe, Reversible, Irreversible }
pub struct ToolDef { pub name: String, pub description: String, pub params: serde_json::Value }
pub struct ToolResult { pub ok: bool, pub content: String, /* + 自动产出经验 */ }
```

## 依赖
→ 依赖:仅 serde / chrono 等通用库,**零内部依赖**。 ← 被依赖:所有其他 crate。

## 数据流
不持有运行时流程。是被各 crate 引用的"词汇表":memory 存 `Conclusion`、safety 读 `Risk`、app 实现 `Executor`、learn 演化 `Conclusion`。

## 接原理
- `设计/04` 原则1(认知压缩):`Conclusion` 单模型 + 连续压缩率,无 EntryType 双轨。
- `设计/05` 原则1(统一执行器):`Executor` trait 是唯一形态。

## 已知坑
- 旧 `EntryType` 枚举与 `compression_rate` 双轨并存 → 本次只有 `Conclusion.compression: f32`。
- 旧类型散在 conclusion/context/project/settings 四个 crate → 本次全收进 core,杜绝循环依赖。
- `execute` 必须是 **async**:曾因它是同步,后台任务三工具被迫绕开注册表、在脊柱里按名拦截,还自己重写了一遍安全门——"两条分发路径、两处安全门",正是架构公理要根治的"三套分发打架"的复发。Opus 2026-05-31 改 async 后三工具回归普通执行器,分发与安全门各收回一处。**新增任何异步能力(网络/做梦),一律做成执行器,不准在循环里按名拦截。**
