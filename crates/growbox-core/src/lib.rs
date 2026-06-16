//! growbox-core — 全局共享类型(零内部依赖)。
//!
//! 实现 `设计文档/系统架构/01-core.md`。
//! 只放类型与最小纯逻辑;任何具体行为(检索/调用/执行)在上层 crate。

mod conclusion;
mod executor;
mod project;
mod workflow;

pub use conclusion::{Confidence, Conclusion, Scope};
pub use executor::{
    CancelFlag, Claim, ExecCtx, Executor, Risk, ToolCall, ToolDef, ToolLimits, ToolResult, UiIntent,
};
pub use project::{McpServerConfig, ProjectConfig, Settings};
pub use workflow::{Node, Transition, TransitionOn, WfTrigger, Workflow, WorkflowScope, END_NODE};

/// 统一时间戳类型(带时区)。
pub type Timestamp = chrono::DateTime<chrono::FixedOffset>;

/// 当前本地时间(带时区偏移)。
pub fn now() -> Timestamp {
    chrono::Local::now().fixed_offset()
}
