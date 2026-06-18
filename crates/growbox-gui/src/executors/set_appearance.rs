//! set_appearance —— LLM 直接切换界面外观(暗/亮/跟随系统)。
//!
//! 用户铁律:一切可设置的设置都应能被 LLM 操控。外观纯属无害的展示偏好(非安全/风险裁决),
//! 故与 open_settings(只把用户带到设置项、改不改由用户定)不同:这里 LLM **直接落地**。
//! 镜像 artifact_command 的家族二往返:脊柱发出 UI 意图 → 前端应用并持久化 → 回执验证态(不撒谎)。
//! 主题真相源在前端 localStorage(同 truncate_tool_display 一类纯 UI 偏好),后端不另存。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult, UiIntent};

pub struct SetAppearance;

impl SetAppearance {
    /// 校验 theme 是受支持的三档之一,规整为小写。非法 → None(执行器据此报错)。
    fn validate(args: &serde_json::Value) -> Option<String> {
        let theme = args.get("theme").and_then(|v| v.as_str())?.trim().to_lowercase();
        match theme.as_str() {
            "dark" | "light" | "auto" => Some(theme),
            _ => None,
        }
    }
}

#[async_trait::async_trait]
impl Executor for SetAppearance {
    fn name(&self) -> &str {
        "set_appearance"
    }

    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            // 描述由注册表按 prompt_lang 从 tools.i18n.json 注入。
            description: String::new(),
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "theme": {
                        "type": "string",
                        "enum": ["dark", "light", "auto"],
                        "description": ""
                    }
                },
                "required": ["theme"]
            }),
        }
    }

    fn risk(&self) -> Risk {
        // 纯展示偏好,无本机/网络/敏感操作,零裁决。
        Risk::Safe
    }

    fn ui_intent(&self, args: &serde_json::Value) -> Option<UiIntent> {
        let theme = Self::validate(args)?;
        Some(UiIntent::round_trip(
            "set_appearance",
            serde_json::json!({ "theme": theme }),
        ))
    }

    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 正常路径走 ui_intent;到这里 = theme 非法或缺失。
        ToolResult::fail("set_appearance 需要 theme 取值 dark / light / auto 之一。")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_theme_yields_round_trip() {
        for t in ["dark", "light", "auto"] {
            let intent = SetAppearance
                .ui_intent(&serde_json::json!({ "theme": t }))
                .expect("合法 theme 应产出往返意图");
            assert_eq!(intent.action, "set_appearance");
            assert!(intent.await_ack, "家族二,等前端回执");
            assert_eq!(intent.prefill["theme"], t);
        }
    }

    #[test]
    fn theme_is_normalized_lowercase() {
        let intent = SetAppearance
            .ui_intent(&serde_json::json!({ "theme": "  LIGHT " }))
            .expect("应规整大小写与空白");
        assert_eq!(intent.prefill["theme"], "light");
    }

    #[test]
    fn invalid_or_missing_theme_yields_none() {
        assert!(SetAppearance.ui_intent(&serde_json::json!({ "theme": "blue" })).is_none());
        assert!(SetAppearance.ui_intent(&serde_json::json!({})).is_none());
    }

    #[tokio::test]
    async fn execute_without_intent_fails() {
        let mut ctx = ExecCtx { args: serde_json::json!({}), work_dir: std::path::Path::new("."), limits: Default::default(), cancel: None };
        let r = SetAppearance.execute(&mut ctx).await;
        assert!(!r.ok);
    }
}
