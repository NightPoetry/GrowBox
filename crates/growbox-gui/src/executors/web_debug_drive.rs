//! web_debug_drive —— 网页自反馈调试的"手":在「网页调试窗」里**真做一个操作**(click/fill/submit)
//! 或 scan(枚举交互点)/observe(读当前页),然后**读回结果**(url/title/本页报错/选择器是否匹配)。
//!
//! 为什么要它:只看 CSS / 枚举回调发现不了"按钮跳转坏、表单提交到错页"这类功能 bug(用户实测:
//! CSS 没问题了,但按钮跳转有问题)。补上"真操作 + 读结果"就能像真人 QA 一样把功能点过一遍。
//!
//! 家族二往返(round_trip):脊柱发出 → 前端 `dispatchUiAction("web_debug_drive")` 调同名 Tauri 命令
//! (`web_debug.rs`)在调试 webview 里真操作 + 观察 → 观察 JSON 经 `uiActionAck.state` 回执 → 脊柱
//! 把它格式化进工具结果交给 AI(`agent/mod.rs` 的 Intent·await_ack 分支)。**手把结果递给脑,零新基建。**
//! 计划/网页功能测试-人类QA工作流.md。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult, UiIntent};

pub struct WebDebugDrive;

/// 支持的操作(与 `web_debug.rs` 的 op 分发、`web_debug_runtime.js` 的 `__gxqa` 一一对应)。
const OPS: [&str; 5] = ["click", "fill", "submit", "scan", "observe"];
/// 必须带 selector 的操作(scan/observe 作用于整页,无需选择器)。
const NEEDS_SELECTOR: [&str; 3] = ["click", "fill", "submit"];

impl WebDebugDrive {
    /// 校验参数,返回规范化的 `(op, selector, value)`,或一条**给 AI 看的**具体错误。
    /// 单一校验源:`ui_intent`(产意图前)与 `execute`(兜底报错)都走它,错因一致。
    fn validate(args: &serde_json::Value) -> Result<(String, String, String), String> {
        let op = args.get("op").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if op.is_empty() {
            return Err("web_debug_drive 需要 op(click/fill/submit/scan/observe)".into());
        }
        if !OPS.contains(&op.as_str()) {
            return Err(format!("未知 op「{op}」,支持 click/fill/submit/scan/observe"));
        }
        let selector = args.get("selector").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        if NEEDS_SELECTOR.contains(&op.as_str()) && selector.is_empty() {
            return Err(format!(
                "op「{op}」需要 selector(CSS 选择器,如 button.submit 或 a[href*=archive];别用带单引号的选择器)"
            ));
        }
        let value = args.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();
        Ok((op, selector, value))
    }
}

#[async_trait::async_trait]
impl Executor for WebDebugDrive {
    fn name(&self) -> &str {
        "web_debug_drive"
    }

    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(),
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": ["click", "fill", "submit", "scan", "observe"],
                        "description": ""
                    },
                    "selector": { "type": "string", "description": "" },
                    "value": { "type": "string", "description": "" }
                },
                "required": ["op"]
            }),
        }
    }

    fn risk(&self) -> Risk {
        // 操作的是用户自己本地 dev server 的调试页,可逆、无本机文件写入 → Safe。
        // 「涉及真实金融交易的提交不要自动做」由 QA 工作流的金融授权节点拦(Phase 3),不靠这里的 risk。
        Risk::Safe
    }

    fn ui_intent(&self, args: &serde_json::Value) -> Option<UiIntent> {
        let (op, selector, value) = Self::validate(args).ok()?;
        Some(UiIntent::round_trip(
            "web_debug_drive",
            serde_json::json!({ "op": op, "selector": selector, "value": value }),
        ))
    }

    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        // 正常路径走 ui_intent 往返(前端真操作 + 观察回执);到此 = 参数没过校验 → 给 AI 具体原因。
        match Self::validate(&ctx.args) {
            Err(e) => ToolResult::fail(e),
            Ok(_) => ToolResult::fail("web_debug_drive 未能形成有效操作(内部错误,请重试)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(args: serde_json::Value) -> ExecCtx<'static> {
        ExecCtx { args, work_dir: std::path::Path::new("."), limits: Default::default(), cancel: None }
    }

    #[test]
    fn click_yields_round_trip_intent_with_op_and_selector() {
        let exec = WebDebugDrive;
        let intent = exec
            .ui_intent(&serde_json::json!({ "op": "click", "selector": "a[href*=archive]" }))
            .expect("合法 click 应产出意图");
        assert_eq!(intent.action, "web_debug_drive");
        assert!(intent.await_ack, "家族二:必须等观察回执");
        assert_eq!(intent.prefill["op"], "click");
        assert_eq!(intent.prefill["selector"], "a[href*=archive]");
    }

    #[test]
    fn scan_and_observe_need_no_selector() {
        let exec = WebDebugDrive;
        assert!(exec.ui_intent(&serde_json::json!({ "op": "scan" })).is_some());
        assert!(exec.ui_intent(&serde_json::json!({ "op": "observe" })).is_some());
    }

    #[test]
    fn fill_carries_value() {
        let exec = WebDebugDrive;
        let intent = exec
            .ui_intent(&serde_json::json!({ "op": "fill", "selector": "#email", "value": "a@b.com" }))
            .expect("合法 fill 应产出意图");
        assert_eq!(intent.prefill["value"], "a@b.com");
    }

    #[test]
    fn click_without_selector_is_rejected() {
        let exec = WebDebugDrive;
        assert!(exec.ui_intent(&serde_json::json!({ "op": "click" })).is_none(), "click 缺 selector 不应产意图");
    }

    #[test]
    fn unknown_op_is_rejected() {
        let exec = WebDebugDrive;
        assert!(exec.ui_intent(&serde_json::json!({ "op": "hover", "selector": "a" })).is_none());
        assert!(exec.ui_intent(&serde_json::json!({})).is_none(), "缺 op 不应产意图");
    }

    #[tokio::test]
    async fn execute_reports_specific_reason_for_bad_args() {
        let exec = WebDebugDrive;
        // 未知 op:错误信息点名 op。
        let r = exec.execute(&mut ctx(serde_json::json!({ "op": "hover", "selector": "a" }))).await;
        assert!(!r.ok && r.content.contains("hover"), "应报未知 op: {}", r.content);
        // click 缺 selector:错误信息点名 selector。
        let r2 = exec.execute(&mut ctx(serde_json::json!({ "op": "click" }))).await;
        assert!(!r2.ok && r2.content.contains("selector"), "应报缺 selector: {}", r2.content);
    }
}
