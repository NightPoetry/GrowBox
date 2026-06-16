//! 用户决定脊柱 —— "凡需用户裁决才能继续的动作"统一走这一条 round-trip。
//!
//! 取代散落的多套"暂停等用户"实现(shell 逐条审批 / NeedAuth 路径授权 / 隐私文件夹确认),
//! 收敛成单一注册表 + 单一 `EventSink::request_decision` + 单一 `decision_ack` 命令 + 前端单一
//! `decision-request` 监听(按 kind 路由到对应弹窗)。呼应架构公理"一条脊柱"。
//!
//! 拆分:**传输**(本注册表:round-trip 通道,独立 managed state、不触 AppState 锁、临界区不跨 await)
//! 与 **记忆**(各门自管:shell 信任命令在此随手放一份会话级集合;路径授权归 sandbox)分离 ——
//! 脊柱只负责"把问题发出去、把裁决收回来",不替各门决定"记住什么"。
//!
//! 与旧 `ShellApprovals`/`UiAckRegistry` 同构(oneshot + 短临界区),合并了 shell 的"已批准记忆"。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::oneshot;

/// 需要用户裁决的动作类别(驱动前端弹哪个窗;`#[serde(tag=...)]` 让前端按 kind 路由)。
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecisionKind {
    /// shell 命令逐条审批(手动模式;过了硬底线的普通命令)。
    ShellApproval { command: String },
    /// 路径/能力授权(越界文件路径 / shell 引用敏感路径 / 隐私文件夹 / 不可逆确认)。
    /// `access` = read|write|shell;`privacy` = 命中用户设置的隐私文件夹(着重 + 二次确认)。
    PathPermission { path: String, reason: String, access: String, privacy: bool },
}

/// 用户的裁决(粒度 = 用户的"配置":只这次 / 记住这一项 / 信任整个项目 / 拒绝)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// 拒绝执行。
    Deny,
    /// 仅这一次放行(不记忆)。
    Once,
    /// 放行,且记住这一项(shell:本命令;路径:本路径)——会话级。
    Remember,
    /// 放行,且信任整个项目的该能力(shell:本项目所有命令;路径:本项目所有路径)——会话级。
    TrustProject,
}

impl Decision {
    /// 从前端字符串解析(未知值按拒绝,安全侧)。
    pub fn parse(s: &str) -> Self {
        match s {
            "once" => Self::Once,
            "remember" => Self::Remember,
            "trust_project" => Self::TrustProject,
            _ => Self::Deny,
        }
    }
    /// 是否放行(非拒绝)。
    pub fn allows(self) -> bool {
        !matches!(self, Decision::Deny)
    }
}

/// 决定登记表 + 会话级 shell 信任记忆。注册为独立 Tauri managed state。
#[derive(Default)]
pub struct Decisions {
    seq: AtomicU64,
    pending: Mutex<HashMap<String, oneshot::Sender<Decision>>>,
    /// shell "本命令总是允许"的命令原文集合(会话级)。路径授权不在此 —— 归 sandbox。
    shell_approved: Mutex<HashSet<String>>,
    /// shell "信任本项目所有命令":置真后不再逐条问。
    shell_trust_all: AtomicBool,
}

impl Decisions {
    /// 登记一次待裁决:返回 id(随请求发前端)+ 接收端(脊柱 await 它)。
    pub fn register(&self) -> (String, oneshot::Receiver<Decision>) {
        let id = format!("dec-{}", self.seq.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id.clone(), tx);
        (id, rx)
    }

    /// 前端回执到达:按 id 投递给等待的脊柱。
    pub fn deliver(&self, id: &str, decision: Decision) {
        if let Some(tx) = self.pending.lock().remove(id) {
            let _ = tx.send(decision);
        }
    }

    /// 超时/放弃:撤掉登记(接收端会得到 Canceled)。
    pub fn cancel(&self, id: &str) {
        self.pending.lock().remove(id);
    }

    // --- shell 信任记忆(会话级) ---

    /// 该 shell 命令是否已被信任(本命令总是允许 / 信任本项目所有命令)。
    pub fn shell_trusted(&self, command: &str) -> bool {
        self.shell_trust_all.load(Ordering::Relaxed) || self.shell_approved.lock().contains(command)
    }

    /// 记住"本 shell 命令总是允许"。
    pub fn shell_remember(&self, command: &str) {
        self.shell_approved.lock().insert(command.to_string());
    }

    /// 置"信任本项目所有 shell 命令"。
    pub fn shell_trust_all(&self) {
        self.shell_trust_all.store(true, Ordering::Relaxed);
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_maps_known_else_deny() {
        assert_eq!(Decision::parse("once"), Decision::Once);
        assert_eq!(Decision::parse("remember"), Decision::Remember);
        assert_eq!(Decision::parse("trust_project"), Decision::TrustProject);
        assert_eq!(Decision::parse("nonsense"), Decision::Deny);
        assert!(Decision::Once.allows() && !Decision::Deny.allows());
    }

    #[test]
    fn shell_memory_trusts_remembered_and_trust_all() {
        let d = Decisions::default();
        assert!(!d.shell_trusted("npm run build"));
        d.shell_remember("npm run build");
        assert!(d.shell_trusted("npm run build"));
        assert!(!d.shell_trusted("rm x"));
        d.shell_trust_all();
        assert!(d.shell_trusted("rm x")); // 信任全部后任意命令都信任(硬底线仍在 judge 层另判)
    }

    #[tokio::test]
    async fn register_then_deliver_resolves() {
        let d = Decisions::default();
        let (id, rx) = d.register();
        d.deliver(&id, Decision::TrustProject);
        assert_eq!(rx.await.unwrap(), Decision::TrustProject);
        assert_eq!(d.pending_count(), 0);
    }
}
