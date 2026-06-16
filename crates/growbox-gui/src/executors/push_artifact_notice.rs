//! push_artifact_notice —— 被造物主动推送(流3):AI 往造物的"覆盖层"(右上角槽)推一条主动提示/建议。
//!
//! 实现 `设计/05-工具系统` 推论 8(造物覆盖层)+ `计划/被造物-自由展示与交互.md` Phase 3。
//! 覆盖层是 GrowBox 可信层、浮在沙箱内容之上(不进沙箱、不被造物内容污染)——AI 在任何造物里都有
//! 一条给用户推消息的常驻通道。家族二往返(`await_ack=true`):脊柱发出后等前端回执,返回验证态。
//! 用于"看到用户犹豫/卡壳就主动帮忙"(双受众:显示在覆盖层 + AI 自身回合记录)。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult, UiIntent};

pub struct PushArtifactNotice;

impl PushArtifactNotice {
    /// 校验 text 非空;合法返回 (canvas_id, text)。
    fn validate(args: &serde_json::Value) -> Option<(String, String)> {
        let text = args.get("text").and_then(|v| v.as_str())?.trim();
        if text.is_empty() {
            return None;
        }
        let canvas_id = args.get("canvas_id").and_then(|v| v.as_str()).unwrap_or("main").trim();
        let canvas_id = if canvas_id.is_empty() { "main" } else { canvas_id };
        Some((canvas_id.to_string(), text.to_string()))
    }
}

#[async_trait::async_trait]
impl Executor for PushArtifactNotice {
    fn name(&self) -> &str {
        "push_artifact_notice"
    }

    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(),
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "" },
                    "canvas_id": { "type": "string", "description": "" },
                },
                "required": ["text"]
            }),
        }
    }

    fn risk(&self) -> Risk {
        // 往可信覆盖层推一条文字提示,天然可逆、无副作用。
        Risk::Safe
    }

    fn ui_intent(&self, args: &serde_json::Value) -> Option<UiIntent> {
        let (canvas_id, text) = Self::validate(args)?;
        Some(UiIntent::round_trip(
            "push_artifact_notice",
            serde_json::json!({ "canvas_id": canvas_id, "text": text }),
        ))
    }

    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 正常路径走 ui_intent 往返;到此 = text 缺失/空。
        ToolResult::fail("push_artifact_notice 需要非空的 text 参数(给用户的主动提示/建议)。")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_text_yields_round_trip_intent() {
        let exec = PushArtifactNotice;
        let intent = exec
            .ui_intent(&serde_json::json!({ "text": "要我帮你填吗?" }))
            .expect("合法调用应产出意图");
        assert_eq!(intent.action, "push_artifact_notice");
        assert!(intent.await_ack, "push_artifact_notice 是家族二,必须等回执");
        assert_eq!(intent.prefill["text"], "要我帮你填吗?");
        assert_eq!(intent.prefill["canvas_id"], "main");
    }

    #[test]
    fn empty_or_missing_text_yields_none() {
        let exec = PushArtifactNotice;
        assert!(exec.ui_intent(&serde_json::json!({ "text": "" })).is_none());
        assert!(exec.ui_intent(&serde_json::json!({ "text": "  " })).is_none());
        assert!(exec.ui_intent(&serde_json::json!({})).is_none());
    }

    #[tokio::test]
    async fn execute_on_missing_text_fails() {
        let exec = PushArtifactNotice;
        let mut ctx = ExecCtx {
            args: serde_json::json!({}),
            work_dir: std::path::Path::new("."),
            limits: Default::default(), cancel: None,
        };
        let r = exec.execute(&mut ctx).await;
        assert!(!r.ok);
        assert!(r.content.contains("text"));
    }
}
