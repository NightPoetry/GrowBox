//! finish —— 终止执行器(结束 Agent 循环的唯一"正门")。
//!
//! 架构要点(见 `交接报告` 早停修复):脊柱不再因"这轮没有工具调用"就判完成。
//! 模型只有显式调用 finish,任务才算确认结束;否则裸文本会被提醒"继续或 finish"。
//! 被外部条件卡住(缺 key/缺授权/需用户决策)也调 finish,在 summary 里说清卡点。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult};

pub struct Finish;

#[async_trait::async_trait]
impl Executor for Finish {
    fn name(&self) -> &str {
        "finish"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入

            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "summary": { "type": "string", "description": "给用户的简洁总结(做了什么 / 卡在哪)" }
                }
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn terminal(&self) -> bool {
        true
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        // 总结回填为结果内容;空则交由脊柱沿用已累积的正文。
        let summary = ctx
            .args
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        ToolResult::ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn finish_is_terminal_and_echoes_summary() {
        let f = Finish;
        assert!(f.terminal());
        let mut ctx = ExecCtx { args: serde_json::json!({"summary": "博客建好了"}), work_dir: Path::new(".") , limits: Default::default(), cancel: None };
        let r = f.execute(&mut ctx).await;
        assert!(r.ok);
        assert_eq!(r.content, "博客建好了");
    }

    #[tokio::test]
    async fn finish_empty_summary_yields_empty() {
        let mut ctx = ExecCtx { args: serde_json::json!({}), work_dir: Path::new(".") , limits: Default::default(), cancel: None };
        assert_eq!(Finish.execute(&mut ctx).await.content, "");
    }
}
