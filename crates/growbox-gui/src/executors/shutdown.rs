//! shutdown —— 自关机能力(关闭自己 / 系统关机),见 `计划/自关机能力.md`。
//!
//! 两种"关闭"(分清楚):
//! - `exit_self`:GrowBox 进程退出(app.exit)。"我下班了"。
//! - `system_shutdown`:关整台机器(detach 一个倒计时 `shutdown` 脚本,独立于本进程存活)。
//!
//! ★授权(对齐 shell_gate 拥有者裁决)★:关机是不可逆 + 影响整机的最高风险,默认每次弹**一次性临时授权**
//! (前端红框确认窗);只有用户在设置里显式开 `Settings.auto_shutdown_allowed` 才免弹、全自动。
//!
//! 架构:执行器拿不到 AppHandle(app.exit 在 cmds 层),故本执行器走**家族二往返**(返回 UiIntent),
//! 前端据 action 弹确认窗 → 确认后调后端 `do_shutdown` 命令真正执行。复用控制反转,不另起机制。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult, UiIntent};

pub struct Shutdown;

impl Shutdown {
    /// 校验 action 合法;返回 (action, delay_secs)。
    fn parse(args: &serde_json::Value) -> Option<(String, u64)> {
        let action = args.get("action").and_then(|v| v.as_str())?.trim();
        if action != "exit_self" && action != "system_shutdown" {
            return None;
        }
        let delay = args.get("delay_secs").and_then(|v| v.as_u64()).unwrap_or(0);
        Some((action.to_string(), delay))
    }
}

#[async_trait::async_trait]
impl Executor for Shutdown {
    fn name(&self) -> &str {
        "shutdown"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(),
            params: serde_json::json!({
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": { "type": "string", "enum": ["exit_self", "system_shutdown"] },
                    "delay_secs": { "type": "integer", "minimum": 0 }
                }
            }),
        }
    }
    fn risk(&self) -> Risk {
        // 关机不可逆;但真正的裁决在前端一次性授权窗 + do_shutdown(本执行器走 intent 往返,
        // risk_gate 不拦 intent)。标 Irreversible 作诚实记录。
        Risk::Irreversible
    }
    fn ui_intent(&self, args: &serde_json::Value) -> Option<UiIntent> {
        let (action, delay_secs) = Self::parse(args)?;
        // 家族二往返:前端弹一次性授权窗(或 auto_shutdown_allowed 时免弹)→ 确认即调 do_shutdown 执行 → 回执。
        Some(UiIntent::round_trip(
            "shutdown",
            serde_json::json!({ "action": action, "delay_secs": delay_secs }),
        ))
    }
    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 正常路径走 ui_intent 往返;到此 = action 非法/缺失。
        ToolResult::fail(
            "shutdown 需要 action: \"exit_self\"(关闭 GrowBox 自己)或 \"system_shutdown\"(关整台机器,可选 delay_secs)。",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_self_yields_round_trip() {
        let intent = Shutdown.ui_intent(&serde_json::json!({ "action": "exit_self" })).expect("合法");
        assert_eq!(intent.action, "shutdown");
        assert!(intent.await_ack, "关机是家族二,前端确认后回执");
        assert_eq!(intent.prefill["action"], "exit_self");
        assert_eq!(intent.prefill["delay_secs"], 0);
    }

    #[test]
    fn system_shutdown_carries_delay() {
        let intent = Shutdown
            .ui_intent(&serde_json::json!({ "action": "system_shutdown", "delay_secs": 60 }))
            .unwrap();
        assert_eq!(intent.prefill["action"], "system_shutdown");
        assert_eq!(intent.prefill["delay_secs"], 60);
    }

    #[test]
    fn invalid_action_no_intent() {
        assert!(Shutdown.ui_intent(&serde_json::json!({ "action": "nuke" })).is_none());
        assert!(Shutdown.ui_intent(&serde_json::json!({})).is_none());
    }

    #[tokio::test]
    async fn execute_without_valid_action_fails() {
        let mut ctx = ExecCtx { args: serde_json::json!({}), work_dir: std::path::Path::new("."), limits: Default::default(), cancel: None };
        assert!(!Shutdown.execute(&mut ctx).await.ok);
    }
}
