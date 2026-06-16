//! render_artifact —— 被造物(artifact)展示半:AI 现造一段沙箱 HTML 渲进"造物画布"(家族二往返,不撒谎)。
//!
//! 实现 `设计/05-工具系统` 推论 8 + `计划/被造物-自由展示与交互.md` Phase 1。
//! 与 `ui_control` 同为家族二(`await_ack=true`):脊柱发出后等前端回执,返回验证态。
//! 区别:无目录校验,只验 `html` 非空 —— 渲染目标是前端的造物画布(沙箱 iframe)。
//! 合法 → `ui_intent` 返回往返意图;非法(缺 html)→ None,落 `execute` 报错让 LLM 自纠。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult, UiIntent};

pub struct RenderArtifact;

impl RenderArtifact {
    /// 校验 html 非空;合法返回 (canvas_id, html)。canvas_id 缺省 "main"。
    fn validate(args: &serde_json::Value) -> Option<(String, String)> {
        let html = args.get("html").and_then(|v| v.as_str())?.trim();
        if html.is_empty() {
            return None;
        }
        let canvas_id = args.get("canvas_id").and_then(|v| v.as_str()).unwrap_or("main").trim();
        let canvas_id = if canvas_id.is_empty() { "main" } else { canvas_id };
        Some((canvas_id.to_string(), html.to_string()))
    }
}

#[async_trait::async_trait]
impl Executor for RenderArtifact {
    fn name(&self) -> &str {
        "render_artifact"
    }

    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            // 描述由注册表按 prompt_lang 从 tools.i18n.json 注入(参数描述同理)。
            description: String::new(),
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "html": { "type": "string", "description": "" },
                    "canvas_id": { "type": "string", "description": "" },
                    "chrome": { "type": "boolean", "description": "" },
                },
                "required": ["html"]
            }),
        }
    }

    fn risk(&self) -> Risk {
        // 渲进沙箱画布天然可逆、无本机/网络访问,零裁决(推论 4)。
        Risk::Safe
    }

    fn ui_intent(&self, args: &serde_json::Value) -> Option<UiIntent> {
        // 合法 → 家族二往返意图(脊柱发出后等前端回执);非法 → None,落 execute 报错。
        let (canvas_id, html) = Self::validate(args)?;
        // chrome:是否要顶部横栏(默认 true;LLM 造桌宠等可声明 false)。窗口框架 chrome=auto 时用。
        let chrome = args.get("chrome").and_then(|v| v.as_bool()).unwrap_or(true);
        Some(UiIntent::round_trip(
            "render_artifact",
            serde_json::json!({ "canvas_id": canvas_id, "html": html, "chrome": chrome }),
        ))
    }

    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 正常(合法)路径在 dispatch 见 ui_intent 即往返,不到这里。
        // 到此 = html 缺失/空:诚实报错,让 LLM 自纠。
        ToolResult::fail("render_artifact 需要非空的 html 参数(你现造的 UI 片段,可含 <style>)。")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_html_yields_round_trip_intent() {
        let exec = RenderArtifact;
        let intent = exec
            .ui_intent(&serde_json::json!({ "html": "<h1>hi</h1>" }))
            .expect("合法调用应产出意图");
        assert_eq!(intent.action, "render_artifact");
        assert!(intent.await_ack, "render_artifact 是家族二,必须等回执");
        assert_eq!(intent.prefill["html"], "<h1>hi</h1>");
        assert_eq!(intent.prefill["canvas_id"], "main", "缺省画布为 main");
    }

    #[test]
    fn custom_canvas_id_passes_through() {
        let exec = RenderArtifact;
        let intent = exec
            .ui_intent(&serde_json::json!({ "html": "<p>x</p>", "canvas_id": "board" }))
            .unwrap();
        assert_eq!(intent.prefill["canvas_id"], "board");
    }

    #[test]
    fn empty_or_missing_html_yields_none() {
        let exec = RenderArtifact;
        assert!(exec.ui_intent(&serde_json::json!({ "html": "" })).is_none());
        assert!(exec.ui_intent(&serde_json::json!({ "html": "   " })).is_none());
        assert!(exec.ui_intent(&serde_json::json!({})).is_none());
    }

    #[test]
    fn definition_requires_html_and_empty_description() {
        let exec = RenderArtifact;
        let def = exec.definition();
        assert_eq!(def.name, "render_artifact");
        assert!(def.description.is_empty(), "描述由注册表从 i18n 注入");
        let required = def.params["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r == "html"));
    }

    #[tokio::test]
    async fn execute_on_missing_html_fails() {
        let exec = RenderArtifact;
        let mut ctx = ExecCtx {
            args: serde_json::json!({}),
            work_dir: std::path::Path::new("."),
            limits: Default::default(), cancel: None,
        };
        let r = exec.execute(&mut ctx).await;
        assert!(!r.ok);
        assert!(r.content.contains("html"), "报错应提示需要 html");
    }
}
