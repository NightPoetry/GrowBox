//! define_workflow —— 定义并注册一个工作流(工作流即动态工具)。
//!
//! 实现 `设计/07-工作流机制.md` 原则2:LLM 用本执行器定义一个工作流,注册进 `WorkflowStore`;
//! 注册表随即把它暴露成一个**动态工具**(name = 工作流名),LLM 像调普通工具一样调用它即进入
//! 工作流,**复用唯一的工具分发路径,零新机制**。
//!
//! 它本身只是个普通(Safe)执行器:执行 = 解析定义、登记。强制顺序/工具过滤/引导注入由脊柱据
//! 当前节点施行(见 `agent.rs`)。
//!
//! ★对 LLM 友好的 JSON 形态★(与 core 内部枚举解耦,这里手工映射):
//! ```json
//! {
//!   "name": "工作流名(=注册成的工具名)",
//!   "description": "何时该调用进入(给 LLM 看)",
//!   "scope": "global|project|artifact",   // 可选,默认 global
//!   "canvas": "画布id",                    // 可选;scope=artifact 时绑定到某造物画布(端口触发用)
//!   "entry": "入口节点id",
//!   "nodes": [
//!     { "id": "n1", "prompt": "这一步该做什么",
//!       "tools": ["工具名", ...],          // 本步允许的工具子集(物理锁死选择空间)
//!       "next": [ { "to": "n2", "on_tool": "工具名" } ] }  // on_tool 省略=无条件流转;to="END"=退出工作流
//!   ],
//!   "triggers": [ { "port": "回调名", "to": "节点id" } ]   // 可选;造物交互回调(端口)→ 进入某节点跑整套
//! }
//! ```

use std::sync::Arc;

use growbox_core::{
    ExecCtx, Executor, Node, Risk, ToolDef, ToolResult, Transition, TransitionOn, WfTrigger,
    Workflow, WorkflowScope,
};
use serde_json::Value;

use crate::workflow_store::WorkflowStore;

pub struct DefineWorkflow {
    store: Arc<WorkflowStore>,
}

impl DefineWorkflow {
    pub fn new(store: Arc<WorkflowStore>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl Executor for DefineWorkflow {
    fn name(&self) -> &str {
        "define_workflow"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入
            params: serde_json::json!({
                "type": "object",
                "required": ["name", "description", "entry", "nodes"],
                "properties": {
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "scope": { "type": "string", "enum": ["global", "project", "artifact"] },
                    "canvas": { "type": "string" },
                    "triggers": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["port", "to"],
                            "properties": {
                                "port": { "type": "string" },
                                "to": { "type": "string" }
                            }
                        }
                    },
                    "entry": { "type": "string" },
                    "nodes": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["id", "prompt", "tools"],
                            "properties": {
                                "id": { "type": "string" },
                                "prompt": { "type": "string" },
                                "tools": { "type": "array", "items": { "type": "string" } },
                                "next": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "required": ["to"],
                                        "properties": {
                                            "to": { "type": "string" },
                                            "on_tool": { "type": "string" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        match parse_workflow(&ctx.args) {
            Ok(wf) => {
                let name = wf.name.clone();
                let node_count = wf.nodes.len();
                self.store.define(wf);
                ToolResult::ok(format!(
                    "已注册工作流「{name}」({node_count} 个节点)。它现在是一个工具,\
                     在需要按此既定流程做事时,直接调用同名工具即可进入。"
                ))
            }
            Err(e) => ToolResult::fail(format!("工作流定义无效: {e}")),
        }
    }
}

/// 解析 LLM 友好 JSON → core `Workflow`(校验入口存在 + 节点非空)。
fn parse_workflow(args: &Value) -> Result<Workflow, String> {
    let name = req_str(args, "name")?;
    let description = req_str(args, "description")?;
    let entry = req_str(args, "entry")?;
    let scope = match args.get("scope").and_then(|v| v.as_str()).unwrap_or("global") {
        "global" => WorkflowScope::Global,
        "project" => WorkflowScope::Project,
        "artifact" => WorkflowScope::Artifact,
        other => return Err(format!("scope 取值非法: {other}(应为 global/project/artifact)")),
    };

    let nodes_val = args
        .get("nodes")
        .and_then(|v| v.as_array())
        .ok_or("缺少 nodes 数组")?;
    if nodes_val.is_empty() {
        return Err("nodes 不能为空".into());
    }
    let mut nodes = Vec::with_capacity(nodes_val.len());
    for (i, nv) in nodes_val.iter().enumerate() {
        nodes.push(parse_node(nv).map_err(|e| format!("第 {} 个节点: {e}", i + 1))?);
    }

    let canvas = args.get("canvas").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(String::from);
    let triggers = match args.get("triggers").and_then(|t| t.as_array()) {
        Some(arr) => arr.iter().map(parse_trigger).collect::<Result<Vec<_>, _>>()?,
        None => vec![],
    };

    let wf = Workflow {
        name,
        description,
        scope,
        nodes,
        entry: entry.clone(),
        canvas,
        triggers,
    };
    if !wf.entry_exists() {
        return Err(format!("入口节点「{entry}」不在 nodes 里"));
    }
    // 触发器目标节点也须存在(否则端口触发会进到不存在的节点)。
    for t in &wf.triggers {
        if wf.node(&t.to).is_none() {
            return Err(format!("触发器 port「{}」指向的节点「{}」不在 nodes 里", t.port, t.to));
        }
    }
    Ok(wf)
}

fn parse_trigger(v: &Value) -> Result<WfTrigger, String> {
    Ok(WfTrigger { port: req_str(v, "port")?, to: req_str(v, "to")? })
}

fn parse_node(v: &Value) -> Result<Node, String> {
    let id = req_str(v, "id")?;
    let prompt = req_str(v, "prompt")?;
    let tools = v
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| arr.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let next = match v.get("next").and_then(|n| n.as_array()) {
        Some(arr) => arr.iter().map(parse_transition).collect::<Result<Vec<_>, _>>()?,
        None => vec![],
    };
    Ok(Node { id, prompt, tools, next })
}

fn parse_transition(v: &Value) -> Result<Transition, String> {
    let to = req_str(v, "to")?;
    let on = match v.get("on_tool").and_then(|t| t.as_str()) {
        Some(tool) if !tool.is_empty() => TransitionOn::ToolCalled(tool.to_string()),
        _ => TransitionOn::Always,
    };
    Ok(Transition { on, to })
}

fn req_str(v: &Value, key: &str) -> Result<String, String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or_else(|| format!("缺少字段 {key}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn ctx(args: Value) -> ExecCtx<'static> {
        ExecCtx { args, work_dir: Path::new("."), limits: Default::default(), cancel: None }
    }

    #[tokio::test]
    async fn defines_and_registers_workflow() {
        let store = Arc::new(WorkflowStore::default());
        let exec = DefineWorkflow::new(store.clone());
        let mut c = ctx(serde_json::json!({
            "name": "gomoku_play",
            "description": "五子棋对弈",
            "scope": "artifact",
            "canvas": "gomoku",
            "entry": "ai_move",
            "triggers": [ { "port": "place", "to": "ai_move" } ],
            "nodes": [
                { "id": "ai_move", "prompt": "轮到你落子", "tools": ["artifact_command"],
                  "next": [ { "to": "judge", "on_tool": "artifact_command" } ] },
                { "id": "judge", "prompt": "判胜负", "tools": ["artifact_command", "finish"] }
            ]
        }));
        let r = exec.execute(&mut c).await;
        assert!(r.ok, "{}", r.content);
        let wf = store.get("gomoku_play").expect("应已注册");
        assert_eq!(wf.scope, WorkflowScope::Artifact);
        assert_eq!(wf.entry, "ai_move");
        // 造物作用域绑定画布 + 端口触发解析。
        assert_eq!(wf.canvas.as_deref(), Some("gomoku"));
        assert_eq!(wf.trigger_for("place"), Some("ai_move"));
        // on_tool 映射成 ToolCalled。
        assert_eq!(
            wf.node("ai_move").unwrap().next[0].on,
            TransitionOn::ToolCalled("artifact_command".into())
        );
        // 缺 next 的节点 = 空流转(终态)。
        assert!(wf.node("judge").unwrap().next.is_empty());
    }

    #[tokio::test]
    async fn rejects_trigger_to_missing_node() {
        let store = Arc::new(WorkflowStore::default());
        let exec = DefineWorkflow::new(store.clone());
        let mut c = ctx(serde_json::json!({
            "name": "w", "description": "d", "entry": "a",
            "triggers": [ { "port": "p", "to": "ghost" } ],
            "nodes": [ { "id": "a", "prompt": "p", "tools": [] } ]
        }));
        let r = exec.execute(&mut c).await;
        assert!(!r.ok);
        assert!(store.get("w").is_none(), "触发器指向不存在节点应整体拒绝");
    }

    #[tokio::test]
    async fn on_tool_absent_means_always() {
        let store = Arc::new(WorkflowStore::default());
        let exec = DefineWorkflow::new(store.clone());
        let mut c = ctx(serde_json::json!({
            "name": "w", "description": "d", "entry": "a",
            "nodes": [ { "id": "a", "prompt": "p", "tools": [], "next": [ { "to": "END" } ] } ]
        }));
        assert!(exec.execute(&mut c).await.ok);
        assert_eq!(store.get("w").unwrap().node("a").unwrap().next[0].on, TransitionOn::Always);
    }

    #[tokio::test]
    async fn rejects_entry_not_in_nodes() {
        let store = Arc::new(WorkflowStore::default());
        let exec = DefineWorkflow::new(store.clone());
        let mut c = ctx(serde_json::json!({
            "name": "w", "description": "d", "entry": "nope",
            "nodes": [ { "id": "a", "prompt": "p", "tools": [] } ]
        }));
        let r = exec.execute(&mut c).await;
        assert!(!r.ok);
        assert!(store.get("w").is_none(), "无效定义不应注册");
    }

    #[tokio::test]
    async fn rejects_missing_fields() {
        let store = Arc::new(WorkflowStore::default());
        let exec = DefineWorkflow::new(store);
        // 缺 nodes
        let mut c = ctx(serde_json::json!({ "name": "w", "description": "d", "entry": "a" }));
        assert!(!exec.execute(&mut c).await.ok);
    }
}
