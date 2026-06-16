//! ask_user —— 向用户提问并让位暂停(澄清/选择类问题的"正门")。
//!
//! 与 finish 并列的循环出口:finish=任务完成(Completed);ask_user=需用户回答才能继续(AwaitingUser)。
//! 修真实 bug:agent 用**纯文字**问用户(无工具调用)会被"只说话=空转"逻辑判停滞 → 注入"继续或 finish"
//! 催续提示(还外泄到对话)+ 被推着又答一遍(重复)。让 agent **显式调 ask_user** 提问:这是工具调用,
//! 天然绕开空转分支;脊柱据 `awaits_user()` 干净结束本轮、把问题作为助手消息显示,等用户在聊天里回答驱动下一轮。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult};

pub struct AskUser;

#[async_trait::async_trait]
impl Executor for AskUser {
    fn name(&self) -> &str {
        "ask_user"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "要问用户的问题(需要澄清/在选项间选择才能继续时)" },
                    "options": { "type": "array", "items": { "type": "string" }, "description": "可选:供用户选择的选项列表" }
                },
                "required": ["question"]
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn awaits_user(&self) -> bool {
        true
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let question = ctx
            .args
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let opts: Vec<String> = ctx
            .args
            .get("options")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        // 问题(+ 选项)作为给用户看的内容回传;脊柱据 awaits_user 让位暂停。
        let mut out = if question.is_empty() { "(请补充说明)".to_string() } else { question };
        for (i, o) in opts.iter().enumerate() {
            if i == 0 {
                out.push('\n');
            }
            out.push_str(&format!("\n{}. {o}", i + 1));
        }
        ToolResult::ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn ask_user_awaits_and_formats_options() {
        let a = AskUser;
        assert!(a.awaits_user());
        assert!(!a.terminal());
        let mut ctx = ExecCtx {
            args: serde_json::json!({ "question": "用哪种方案?", "options": ["i18next", "查询参数"] }),
            work_dir: Path::new("."),
            limits: Default::default(), cancel: None,
        };
        let r = a.execute(&mut ctx).await;
        assert!(r.ok);
        assert!(r.content.contains("用哪种方案?"));
        assert!(r.content.contains("1. i18next") && r.content.contains("2. 查询参数"));
    }
}
