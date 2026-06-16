//! tool_search —— 懒加载枢纽(二期 C1,见 项目设计/05-MCP客户端与懒加载 M1)。
//!
//! 工具数无界(尤其接 MCP 后几百上千)时,不可能全塞进每次请求的 tools 字段(撑爆上下文 + 破坏缓存)。
//! 机制:**核心工具常驻 + 扩展工具只露名**,要用某个露名工具时先 `tool_search{query}` 把它的完整 schema
//! 拉回上下文(append-only,不改前缀 → KV 缓存 byte-stable),之后即可直接调用。
//!
//! ★控制信号,不走 dispatch★:检索要读注册表的工具定义 + 当前工作流节点允许名单(脊柱才有),
//! 故脊柱在 dispatch 之前按工具名拦截(同 workflow_return / learn_process)。`execute` 仅兜底。
//! 它本身**永不 deferred、始终常驻**(否则没法用它来加载别的工具)——见 `registry::NEVER_DEFER`。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult};

/// 工具名常量(脊柱拦截 + NEVER_DEFER 都用它,单一源)。
pub const TOOL_SEARCH: &str = "tool_search";

pub struct ToolSearch;

#[async_trait::async_trait]
impl Executor for ToolSearch {
    fn name(&self) -> &str {
        TOOL_SEARCH
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入
            params: serde_json::json!({
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": { "type": "string", "description": "" }
                }
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe // 只读检索工具清单,不动任何资源
    }
    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 兜底:正常情况下脊柱已在 dispatch 前拦截(它需注册表 + 工作流节点允许名单)。
        ToolResult::ok("tool_search 需在对话主链中检索(当前未生效)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn definition_requires_query() {
        let def = ToolSearch.definition();
        assert_eq!(def.name, "tool_search");
        assert_eq!(def.params["required"][0], "query");
    }

    #[tokio::test]
    async fn fallback_execute_is_benign() {
        let mut ctx = ExecCtx {
            args: serde_json::json!({"query": "x"}),
            work_dir: Path::new("."),
            limits: Default::default(), cancel: None,
        };
        assert!(ToolSearch.execute(&mut ctx).await.ok);
    }
}
