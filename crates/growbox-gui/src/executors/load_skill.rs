//! load_skill —— 取一个 Skill 的 playbook 正文(第四原语,见 设计/09 推论5)。
//!
//! Skill 的"描述常驻、正文按需"渐进披露:系统提示里只放每个 skill 的名称 + 触发描述(常驻清单),
//! AI 判断场景匹配时调 `load_skill{name}` 把完整 playbook 拉回上下文(append-only,不破缓存前缀),
//! 之后带着这份知识用通用工具施展。
//!
//! ★控制信号,不走 dispatch★:取正文要读 Memory(已学 skill 节点)+ 内置种子目录(脊柱才持有),
//! 故脊柱在 dispatch 之前按工具名拦截(同 tool_search / learn_process)。`execute` 仅兜底。
//! 它本身**永不 deferred、始终常驻**(否则没法用它加载 skill)——见 `registry::NEVER_DEFER`。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult};

/// 工具名常量(脊柱拦截 + NEVER_DEFER 都用它,单一源)。
pub const LOAD_SKILL: &str = "load_skill";

pub struct LoadSkill;

#[async_trait::async_trait]
impl Executor for LoadSkill {
    fn name(&self) -> &str {
        LOAD_SKILL
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入
            params: serde_json::json!({
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": { "type": "string", "description": "" }
                }
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe // 只读知识进上下文,不动任何资源(据它做的动作仍各自过安全门)
    }
    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 兜底:正常情况下脊柱已在 dispatch 前拦截(它需 Memory + 内置种子目录)。
        ToolResult::ok("load_skill 需在对话主链中加载(当前未生效)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn definition_requires_name() {
        let def = LoadSkill.definition();
        assert_eq!(def.name, "load_skill");
        assert_eq!(def.params["required"][0], "name");
    }

    #[tokio::test]
    async fn fallback_execute_is_benign() {
        let mut ctx = ExecCtx {
            args: serde_json::json!({"name": "x"}),
            work_dir: Path::new("."),
            limits: Default::default(),
            cancel: None,
        };
        assert!(LoadSkill.execute(&mut ctx).await.ok);
    }
}
