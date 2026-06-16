//! open_debug_url —— 网页调试(Phase 2):把本地 web 应用的 URL 拉进可导航的调试 webview(注入套索)。
//!
//! AI 编排:用户说"调试网站"→ AI 读 package.json、用 shell 起 dev server(后台)、从输出抓 URL →
//! 调本工具开调试窗。家族二往返(`await_ack=true`):脊柱发出 → 前端 dispatchUiAction 调
//! `create_debug_webview` 建窗 + 注入套索运行时 → 回执验证态。窗里框选改源回传走本机 HTTP
//! (见 `web_debug.rs` / `web_debug_runtime.js`,不踩 Tauri 远程 IPC 雷区)。
//! 计划/网页调试窗-可视化框选改源.md Phase 2。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult, UiIntent};

pub struct OpenDebugUrl;

impl OpenDebugUrl {
    /// 校验 url 非空且为 http(s);合法返回规范化后的 url。
    fn validate(args: &serde_json::Value) -> Option<String> {
        let url = args.get("url").and_then(|v| v.as_str())?.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return None;
        }
        Some(url.to_string())
    }
}

#[async_trait::async_trait]
impl Executor for OpenDebugUrl {
    fn name(&self) -> &str {
        "open_debug_url"
    }

    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(),
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "" },
                },
                "required": ["url"]
            }),
        }
    }

    fn risk(&self) -> Risk {
        // 打开一个加载本地 URL 的调试窗,天然可逆、无本机写入。
        Risk::Safe
    }

    fn ui_intent(&self, args: &serde_json::Value) -> Option<UiIntent> {
        let url = Self::validate(args)?;
        Some(UiIntent::round_trip(
            "open_debug_url",
            serde_json::json!({ "url": url }),
        ))
    }

    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 正常路径走 ui_intent 往返;到此 = url 缺失/非 http(s)。
        ToolResult::fail(
            "open_debug_url 需要 http(s) 的 url(本地 dev server,如 http://localhost:3000)。",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_url_yields_round_trip_intent() {
        let exec = OpenDebugUrl;
        let intent = exec
            .ui_intent(&serde_json::json!({ "url": "http://localhost:3000" }))
            .expect("合法 URL 应产出意图");
        assert_eq!(intent.action, "open_debug_url");
        assert!(intent.await_ack, "open_debug_url 是家族二,必须等回执");
        assert_eq!(intent.prefill["url"], "http://localhost:3000");
    }

    #[test]
    fn non_http_or_missing_url_yields_none() {
        let exec = OpenDebugUrl;
        assert!(exec.ui_intent(&serde_json::json!({ "url": "" })).is_none());
        assert!(exec.ui_intent(&serde_json::json!({ "url": "file:///etc/passwd" })).is_none());
        assert!(exec.ui_intent(&serde_json::json!({})).is_none());
    }

    #[tokio::test]
    async fn execute_on_missing_url_fails() {
        let exec = OpenDebugUrl;
        let mut ctx = ExecCtx {
            args: serde_json::json!({}),
            work_dir: std::path::Path::new("."),
            limits: Default::default(),
            cancel: None,
        };
        let r = exec.execute(&mut ctx).await;
        assert!(!r.ok);
        assert!(r.content.contains("url"));
    }
}
