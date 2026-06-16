//! 回合级取消(造物交互 v2 §2「可终止」)。
//!
//! 现状:chat 回合无中断机制。需求:用户能按「终止」叫停当前回合(尤其写造物反复 render 时)。
//!
//! ★为何独立 managed state(不放 AppState)★:`run_chat` 全程持 AppState 锁跑脊柱;
//! 若 `cancel_chat` 命令也要锁 AppState,就会被阻塞到回合结束才执行,取消形同虚设。
//! 故与 `UiAckRegistry`/`ShellApprovals` 同构 —— 独立 managed state,一个原子标志,
//! cancel 命令瞬时置位,脊柱每轮读检查点。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 回合级取消信号(共享原子标志)。
#[derive(Default, Clone)]
pub struct ChatControl {
    cancel: Arc<AtomicBool>,
}

impl ChatControl {
    /// 新回合开始:清取消标志(上一回合的取消不殃及本回合)。
    pub fn begin(&self) {
        self.cancel.store(false, Ordering::SeqCst);
    }

    /// 请求取消当前回合(前端「终止」按钮 → cancel_chat 命令)。
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// 脊柱每轮检查:是否已请求取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// 取共享原子句柄,穿进 `ExecCtx.cancel` 让执行器(尤其 shell)在长命令中途响应终止。
    /// 与 `is_cancelled` 同一标志:cancel 命令置位 → 脊柱 LLM 循环 + 执行器 两处都看得到。
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_clears_then_cancel_sets() {
        let c = ChatControl::default();
        assert!(!c.is_cancelled());
        c.request_cancel();
        assert!(c.is_cancelled());
        c.begin();
        assert!(!c.is_cancelled(), "新回合应清掉上轮的取消");
    }

    #[test]
    fn clones_share_the_same_flag() {
        let a = ChatControl::default();
        let b = a.clone();
        a.request_cancel();
        assert!(b.is_cancelled(), "克隆共享同一原子标志(managed state 取一份、sink 取一份)");
    }
}
