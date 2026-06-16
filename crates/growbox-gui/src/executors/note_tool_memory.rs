//! note_tool_memory —— 往工具的"小本本"记一条经验(计划/工具记忆-不犯第二遍 A)。
//!
//! AI 判定"某工具在某情况(关键因素)下 不可行/失败/可行"后调它结晶成一个 tool_memory 节点。
//! 之后**分发前会诊**会用它:不可行 + 高相似 → 反 K 一票否决重试;失败 → 软提醒。关键因素变了就再记
//! 一条(同情况新结论凭更新覆盖旧的,自校正)。
//!
//! ★控制信号,不走 dispatch★:要写主记忆(`Memory::crystallize_tool_memory`,即时嵌入),Memory 只在
//! 脊柱里可变,故脊柱在 dispatch 之前按工具名拦截(同 learn_skill/learn_process)。`execute` 仅兜底。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult};

/// 工具名常量(脊柱拦截用,单一源)。
pub const NOTE_TOOL_MEMORY: &str = "note_tool_memory";

pub struct NoteToolMemory;

#[async_trait::async_trait]
impl Executor for NoteToolMemory {
    fn name(&self) -> &str {
        NOTE_TOOL_MEMORY
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入
            params: serde_json::json!({
                "type": "object",
                "required": ["tool", "situation", "verdict"],
                "properties": {
                    "tool": { "type": "string", "description": "" },
                    "situation": { "type": "string", "description": "" },
                    "verdict": { "type": "string", "enum": ["infeasible", "fails", "works"], "description": "" },
                    "detail": { "type": "string", "description": "" }
                }
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe // 只写自己的项目记忆,安全
    }
    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 兜底:正常情况下脊柱已在 dispatch 前拦截(它需 &mut Memory + subconscious 嵌入)。
        ToolResult::ok("note_tool_memory 已记录(若未生效,请确认在对话主链中调用)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn definition_requires_tool_situation_verdict() {
        let def = NoteToolMemory.definition();
        assert_eq!(def.name, "note_tool_memory");
        let req = def.params["required"].as_array().unwrap();
        for k in ["tool", "situation", "verdict"] {
            assert!(req.iter().any(|v| v == k), "缺 required {k}");
        }
    }

    #[tokio::test]
    async fn fallback_execute_is_benign() {
        let mut ctx = ExecCtx {
            args: serde_json::json!({"tool": "x", "situation": "y", "verdict": "fails"}),
            work_dir: Path::new("."),
            limits: Default::default(),
            cancel: None,
        };
        assert!(NoteToolMemory.execute(&mut ctx).await.ok);
    }
}
