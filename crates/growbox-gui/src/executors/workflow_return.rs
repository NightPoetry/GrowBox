//! workflow_return —— 栈函数工作流的结构化返回原语(见 设计/07「加强版:栈函数工作流 v2」原则4)。
//!
//! 被调工作流在收尾节点调它产出**结构化返回值**:脊柱据此**出栈**并把 `value` 作为一条可读消息
//! **回灌进父工作流上下文**(父下一轮即读到)——这就是"结构化调用必然有一个可被读取的属性"。
//!
//! ★控制信号,不走 dispatch★:`value` 的回灌 + 出栈需要脊柱的 `wf_stack`,故脊柱在 dispatch **之前**
//! 按工具名拦截(与"工作流入口工具"同款拦截),本执行器的 `execute` 仅作兜底(普通模式误调时给个温和结果)。
//! 注册它只为提供 ToolDef(schema)+ i18n 文案,使它能出现在工作流节点的工具清单里供 LLM 调用。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult};

/// 工具名常量(脊柱拦截 + 注册表过滤都用它,单一源)。
pub const WORKFLOW_RETURN: &str = "workflow_return";

pub struct WorkflowReturn;

#[async_trait::async_trait]
impl Executor for WorkflowReturn {
    fn name(&self) -> &str {
        WORKFLOW_RETURN
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入
            params: serde_json::json!({
                "type": "object",
                "required": ["value"],
                "properties": {
                    "value": { "type": "string", "description": "" },
                    "full": { "type": "boolean", "description": "" }
                }
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 兜底:正常情况下脊柱已在 dispatch 前拦截本工具(它需要 wf_stack 才能出栈+回灌)。
        // 走到这里 = 普通模式误调(没有工作流可返回)→ 温和提示,不报错也不终止。
        ToolResult::ok("workflow_return 仅在工作流内有效(当前不在工作流中,已忽略)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[tokio::test]
    async fn fallback_execute_is_benign_and_non_terminal() {
        let w = WorkflowReturn;
        assert!(!w.terminal(), "不是终止整条 Agent 循环的信号(只出一层栈)");
        let mut ctx =
            ExecCtx { args: serde_json::json!({"value": "x"}), work_dir: Path::new("."), limits: Default::default(), cancel: None };
        let r = w.execute(&mut ctx).await;
        assert!(r.ok, "兜底执行温和返回,不报错");
    }

    #[test]
    fn definition_requires_value_and_has_full_flag() {
        let def = WorkflowReturn.definition();
        assert_eq!(def.name, "workflow_return");
        assert_eq!(def.params["required"][0], "value");
        assert!(def.params["properties"].get("full").is_some(), "全量返回标志 full 应在 schema 里");
    }
}
