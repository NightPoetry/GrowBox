//! 活的 IDE —— UI 操控的共享类型与往返登记表。
//!
//! 实现 `设计/00-交互层` 推论 7 + `计划/活的IDE-UI执行器.md`。
//! 核心模型:**前端是 UI 事实的唯一权威,后端只"请求"与"记录"**。
//! - 面板目录(解剖)由前端 `register_ui_surfaces` 声明 → 后端只持运行时派生副本(零漂移)。
//! - UI 操作走"往返":脊柱发出请求 → 前端落地 → `ui_action_ack` 回执 → 返回**验证过的**状态(不撒谎)。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::oneshot;

/// 一个可被 LLM 操控的 UI 面(由前端声明)。
///
/// 这是"身体的解剖":前端是唯一作者,后端据此为 `ui_control` 生成工具 schema 并校验。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiSurface {
    /// 稳定标识(如 "memory" / "dream" / "health" / "control")。
    pub id: String,
    /// 给 LLM 的人话说明(如 "记忆可视化面板")。
    pub label: String,
    /// 该面支持的动作(状态动词子集,如 ["open","close","toggle"])。
    pub ops: Vec<String>,
}

/// 面板目录 = 磁盘外的"解剖图"。后端只持前端声明的运行时副本(可被覆盖,无独立权威)。
/// `ui_control` 持一个克隆在 `definition()`/`ui_intent()` 时读它;`register_ui_surfaces` 写它。
pub type UiSurfaceCatalog = Arc<RwLock<Vec<UiSurface>>>;

/// 新建一个空目录(前端尚未声明时的初值)。
pub fn empty_catalog() -> UiSurfaceCatalog {
    Arc::new(RwLock::new(Vec::new()))
}

/// 状态动词:`ui_control` 只收"改变状态、可验证"的动作(focus/scroll/toast 属家族一,见 OpenSettings)。
pub const UI_CONTROL_OPS: [&str; 3] = ["open", "close", "toggle"];

/// 前端落地某个 UI 操作后的回执 —— 后端关于"面板状态"的认知只来自这里(不撒谎)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiAck {
    /// 前端是否真的落地了该操作。
    pub applied: bool,
    /// 落地后的相关状态(如 {"open": false});LLM 据此知道真实结果。
    pub state: serde_json::Value,
    /// 可选补充(超时/未知 target 等)。
    pub note: Option<String>,
}

impl UiAck {
    /// 无前端 / 超时 / 未应用时的诚实回执。
    pub fn unapplied(note: impl Into<String>) -> Self {
        UiAck { applied: false, state: serde_json::Value::Null, note: Some(note.into()) }
    }
}

/// UI 往返登记表 —— 把"发出请求"与"前端回执"凑成一次 request/response。
///
/// **独立于 AppState 锁**(注册为单独的 Tauri managed state):`run_chat` 全程持 AppState 锁
/// 并在往返期间 await,若 `ui_action_ack` 也要 AppState 锁就死锁。此表用自己的小锁,临界区不跨 await。
#[derive(Default)]
pub struct UiAckRegistry {
    seq: AtomicU64,
    pending: Mutex<HashMap<String, oneshot::Sender<UiAck>>>,
}

impl UiAckRegistry {
    /// 登记一次待回执:返回相关 id(随请求发给前端)+ 接收端(脊柱 await 它)。
    pub fn register(&self) -> (String, oneshot::Receiver<UiAck>) {
        let id = format!("ui-{}", self.seq.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id.clone(), tx);
        (id, rx)
    }

    /// 前端回执到达:按 id 投递给等待的脊柱。
    pub fn deliver(&self, id: &str, ack: UiAck) {
        if let Some(tx) = self.pending.lock().remove(id) {
            let _ = tx.send(ack);
        }
    }

    /// 超时/放弃:撤掉登记(接收端会得到 Canceled)。
    pub fn cancel(&self, id: &str) {
        self.pending.lock().remove(id);
    }

    /// 待回执条数(调试/测试用)。
    pub fn pending_count(&self) -> usize {
        self.pending.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_then_deliver_resolves() {
        let reg = UiAckRegistry::default();
        let (id, rx) = reg.register();
        assert_eq!(reg.pending_count(), 1);
        reg.deliver(&id, UiAck { applied: true, state: serde_json::json!({"open": false}), note: None });
        let ack = rx.await.expect("应收到回执");
        assert!(ack.applied);
        assert_eq!(ack.state.get("open").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(reg.pending_count(), 0, "投递后该条已移除");
    }

    #[tokio::test]
    async fn cancel_drops_sender() {
        let reg = UiAckRegistry::default();
        let (id, rx) = reg.register();
        reg.cancel(&id);
        assert_eq!(reg.pending_count(), 0);
        assert!(rx.await.is_err(), "撤销后接收端应得到 Canceled");
    }

    #[test]
    fn ids_are_unique() {
        let reg = UiAckRegistry::default();
        let (a, _ra) = reg.register();
        let (b, _rb) = reg.register();
        assert_ne!(a, b);
    }
}
