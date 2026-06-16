//! learn_process —— 项目级流程的"报告-纠正回路"写入原语(二期 B3,见 设计原理/01 推论2)。
//!
//! AI 完成一件**复发的多步项目操作**后,调它把"做这件事要碰 A→B→C、什么顺序"结晶成一条
//! 项目级流程(process kind 建议档),下次同类任务自动召回照做(B1)、越用越准(B2)。
//! 用户纠正(漏了一处 / 顺序不对)时再调一次给更正版 → **同一回路既建又修**:近重复会取代旧版。
//!
//! ★控制信号,不走 dispatch★:结晶要写主记忆(`Memory::crystallize_process`,即时嵌入 + 近重复取代),
//! 而 Memory 只在脊柱(agent 循环)里可变,故脊柱在 dispatch **之前**按工具名拦截(同 workflow_return)。
//! 注册它只为提供 ToolDef(schema)+ i18n 文案,使它能出现在工具清单里供 LLM 调用;`execute` 仅兜底。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult};

/// 工具名常量(脊柱拦截用,单一源)。
pub const LEARN_PROCESS: &str = "learn_process";

pub struct LearnProcess;

#[async_trait::async_trait]
impl Executor for LearnProcess {
    fn name(&self) -> &str {
        LEARN_PROCESS
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入
            params: serde_json::json!({
                "type": "object",
                "required": ["name", "recipe"],
                "properties": {
                    "name": { "type": "string", "description": "" },
                    "recipe": { "type": "string", "description": "" }
                }
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe // 只写自己的项目记忆(不碰文件/外部),安全
    }
    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 兜底:正常情况下脊柱已在 dispatch 前拦截(它需 &mut Memory 才能结晶)。走到这里 = 异常路径。
        ToolResult::ok("learn_process 已记录(若未生效,请确认在对话主链中调用)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn definition_requires_name_and_recipe() {
        let def = LearnProcess.definition();
        assert_eq!(def.name, "learn_process");
        let req = def.params["required"].as_array().unwrap();
        assert!(req.iter().any(|v| v == "name") && req.iter().any(|v| v == "recipe"));
    }

    #[tokio::test]
    async fn fallback_execute_is_benign_and_non_terminal() {
        let lp = LearnProcess;
        assert!(!lp.terminal());
        let mut ctx = ExecCtx {
            args: serde_json::json!({"name": "x", "recipe": "y"}),
            work_dir: Path::new("."),
            limits: Default::default(), cancel: None,
        };
        assert!(lp.execute(&mut ctx).await.ok);
    }
}
