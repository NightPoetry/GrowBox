//! learn_skill —— 把一个 Skill 结晶进记忆(第四原语,见 设计/09 推论7)。
//!
//! AI 在某类场景摸索出"该怎么把它做好"后,调它把 `{name, trigger, body}` 结晶成一个 skill 节点
//! (skill kind),进常驻清单可被主动挑、可被语义召回、越用越准。用户纠正后再调一次给更正版 →
//! 近重复/同名取代旧版(同一回路既建又修,与 learn_process 同构)。
//!
//! ★控制信号,不走 dispatch★:结晶要写主记忆(`Memory::crystallize_skill`,即时嵌入 + 近重复取代),
//! Memory 只在脊柱里可变,故脊柱在 dispatch 之前按工具名拦截。`execute` 仅兜底。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult};

/// 工具名常量(脊柱拦截用,单一源)。
pub const LEARN_SKILL: &str = "learn_skill";

pub struct LearnSkill;

#[async_trait::async_trait]
impl Executor for LearnSkill {
    fn name(&self) -> &str {
        LEARN_SKILL
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入
            params: serde_json::json!({
                "type": "object",
                "required": ["name", "trigger", "body"],
                "properties": {
                    "name": { "type": "string", "description": "" },
                    "trigger": { "type": "string", "description": "" },
                    "body": { "type": "string", "description": "" }
                }
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe // 只写自己的项目记忆(不碰文件/外部),安全
    }
    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 兜底:正常情况下脊柱已在 dispatch 前拦截(它需 &mut Memory + subconscious 嵌入)。
        ToolResult::ok("learn_skill 已记录(若未生效,请确认在对话主链中调用)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn definition_requires_name_trigger_body() {
        let def = LearnSkill.definition();
        assert_eq!(def.name, "learn_skill");
        let req = def.params["required"].as_array().unwrap();
        for k in ["name", "trigger", "body"] {
            assert!(req.iter().any(|v| v == k), "缺 required {k}");
        }
    }

    #[tokio::test]
    async fn fallback_execute_is_benign() {
        let mut ctx = ExecCtx {
            args: serde_json::json!({"name": "x", "trigger": "y", "body": "z"}),
            work_dir: Path::new("."),
            limits: Default::default(),
            cancel: None,
        };
        assert!(LearnSkill.execute(&mut ctx).await.ok);
    }
}
