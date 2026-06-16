//! artifact_command —— LLM→造物 指令端口(造物灵魂:LLM 是使用者,经端口操作自己造的 UI,不重画)。
//!
//! 实现 `计划/造物交互-v2.md` §0(造物灵魂)+ 决策日志 2026-06-04。
//! 与 render_artifact(写造物=写代码)对立:这是 LLM **使用**已渲染的造物 —— 发一条结构化指令
//! (如五子棋"落白子(5,5)"),造物自身 JS 的 handler 执行(本地画白子),**不重渲整个造物**。
//! 频繁的端口沟通,不进聊天(决策日志:端口双向沟通太频繁)。家族二往返(等前端转发回执)。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult, UiIntent};

pub struct ArtifactCommand;

impl ArtifactCommand {
    /// 校验 command 非空;合法返回 (canvas_id, command)。
    fn validate(args: &serde_json::Value) -> Option<(String, String)> {
        let command = args.get("command").and_then(|v| v.as_str())?.trim();
        if command.is_empty() {
            return None;
        }
        let canvas_id = args.get("canvas_id").and_then(|v| v.as_str()).unwrap_or("main").trim();
        let canvas_id = if canvas_id.is_empty() { "main" } else { canvas_id };
        Some((canvas_id.to_string(), command.to_string()))
    }
}

#[async_trait::async_trait]
impl Executor for ArtifactCommand {
    fn name(&self) -> &str {
        "artifact_command"
    }

    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            // 描述由注册表按 prompt_lang 从 tools.i18n.json 注入。
            description: String::new(),
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "" },
                    "canvas_id": { "type": "string", "description": "" },
                },
                "required": ["command"]
            }),
        }
    }

    fn risk(&self) -> Risk {
        // 仅向沙箱造物发消息,造物 JS 自行处理;无本机/网络访问,零裁决。
        Risk::Safe
    }

    fn ui_intent(&self, args: &serde_json::Value) -> Option<UiIntent> {
        let (canvas_id, command) = Self::validate(args)?;
        Some(UiIntent::round_trip(
            "artifact_command",
            serde_json::json!({ "canvas_id": canvas_id, "command": command }),
        ))
    }

    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        ToolResult::fail("artifact_command 需要非空的 command(发给造物 JS handler 的结构化指令,如 JSON)。")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_command_yields_round_trip() {
        let exec = ArtifactCommand;
        let intent = exec
            .ui_intent(&serde_json::json!({ "command": "{\"action\":\"place\",\"color\":\"white\",\"r\":7,\"c\":7}" }))
            .expect("合法指令应产出往返意图");
        assert_eq!(intent.action, "artifact_command");
        assert!(intent.await_ack, "家族二,等前端转发回执");
        assert_eq!(intent.prefill["canvas_id"], "main");
        assert!(intent.prefill["command"].as_str().unwrap().contains("place"));
    }

    #[test]
    fn empty_command_yields_none() {
        let exec = ArtifactCommand;
        assert!(exec.ui_intent(&serde_json::json!({ "command": "" })).is_none());
        assert!(exec.ui_intent(&serde_json::json!({})).is_none());
    }

    #[tokio::test]
    async fn execute_on_missing_command_fails() {
        let exec = ArtifactCommand;
        let mut ctx = ExecCtx { args: serde_json::json!({}), work_dir: std::path::Path::new("."), limits: Default::default(), cancel: None };
        let r = exec.execute(&mut ctx).await;
        assert!(!r.ok);
    }
}
