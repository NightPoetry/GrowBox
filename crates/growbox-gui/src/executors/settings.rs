//! open_settings —— 交互类执行器(控制反转)。
//!
//! 实现 `设计/00-交互层`:AI 不直接写设置(那是用户的裁决权),而是打开设置面板、
//! 切到对应分区、滚动并高亮目标字段,把用户精确带到位,改不改由用户定。
//! 所有设置项都可这样被 AI "操作"——统一一个执行器 + 一个 field 标识,前端负责落位。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult, UiIntent};

pub struct OpenSettings;

#[async_trait::async_trait]
impl Executor for OpenSettings {
    fn name(&self) -> &str {
        "open_settings"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入

            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "field": { "type": "string", "description": "要定位的设置项标识(见描述里的可选值)" },
                    "note": { "type": "string", "description": "给用户的一句话建议(如:建议设为 0 开启无限模式)" }
                }
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn ui_intent(&self, args: &serde_json::Value) -> Option<UiIntent> {
        let mut prefill = serde_json::Map::new();
        if let Some(field) = args.get("field").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            prefill.insert("field".into(), serde_json::Value::String(field.to_string()));
        }
        if let Some(note) = args.get("note").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            prefill.insert("note".into(), serde_json::Value::String(note.to_string()));
        }
        // 家族一:打开设置面板并滚动高亮交用户裁决(发出即返回,改不改由用户定)。
        Some(UiIntent::hand_off("open_settings", serde_json::Value::Object(prefill)))
    }
    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 正常路径不会到这里(dispatch 见 ui_intent 即弹 UI)。
        ToolResult::ok("已请求打开设置面板")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_carries_field_and_note() {
        let intent = OpenSettings
            .ui_intent(&serde_json::json!({"field": "max_turns", "note": "建议设为 0"}))
            .unwrap();
        assert_eq!(intent.action, "open_settings");
        assert_eq!(intent.prefill.get("field").unwrap(), "max_turns");
        assert_eq!(intent.prefill.get("note").unwrap(), "建议设为 0");
    }

    #[test]
    fn empty_args_yield_bare_open() {
        let intent = OpenSettings.ui_intent(&serde_json::json!({})).unwrap();
        assert!(intent.prefill.as_object().unwrap().is_empty());
    }
}
