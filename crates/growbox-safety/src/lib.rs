//! growbox-safety — 判定一个操作能不能做、要不要问用户。
//!
//! 实现 `设计文档/系统架构/03-safety.md` 与 `设计/03-安全审查.md`:
//! - 路径分级:可读写 / 只读 / 范围外。
//! - 永久黑名单:敏感路径 + 危险命令。
//! - 默认安全,越界即交还(返回 NeedAuth 带原因)。
//! - 三种授权范围:Once / ThisProjectPath / ThisProject。
//!
//! 只判定,不执行(执行是执行器的事)。

mod sandbox;

pub use sandbox::{
    host_is_private_literal, ip_is_private, parse_http_url, risk_gate, GrantScope, Operation,
    Sandbox, Verdict,
};
