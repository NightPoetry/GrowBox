//! 工作流 —— GrowBox 底层执行机制(共享类型)。
//!
//! 实现 `设计文档/设计/07-工作流机制.md`:工作流 = 有向节点图,节点间**强制顺序流转**;
//! 每个节点 = `{引导词 + 可选工具子集 + 流转规则}`。把"该怎么做"从对 AI 的"建议"变成
//! 结构上的"约束"——节点的工具子集**物理锁死**选择空间,AI 想用错工具也选不到(推论1)。
//!
//! core 只放**数据结构 + 最小纯逻辑**(节点查找/流转判定);运行时存储/注册为动态工具/
//! 脊柱过滤都在上层 crate(见 `growbox-gui` 的 `workflow_store` 与 `agent`)。

use serde::{Deserialize, Serialize};

/// 流转目标为此值 = 退出工作流,回普通模式(全工具)。`finish` 是退出整条 Agent 循环;
/// `END` 只退出工作流本身、Agent 循环继续(见 07 推论6)。
pub const END_NODE: &str = "END";

/// 工作流作用域(三层,见 07 推论2)。P1 仅用 Global(内置出厂);Project/Artifact 的持久化在 P3。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowScope {
    /// 全局:系统内置的默认工作流,所有项目可用。
    Global,
    /// 项目:项目特定(如"打包工作流")。
    Project,
    /// 造物:某造物专属(如"五子棋对弈工作流"),随造物生灭。
    Artifact,
}

/// 节点流转条件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransitionOn {
    /// 本轮调用了某工具就流转(如"调了 artifact_command"→ 判胜负节点)。
    ToolCalled(String),
    /// 无条件流转:本节点这一轮结束即走(不依赖具体工具)。
    Always,
}

/// 一条流转边:满足 `on` 条件 → 去 `to` 节点(`to == END_NODE` 表示退出工作流回普通模式)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    pub on: TransitionOn,
    pub to: String,
}

/// 触发器(P2 用):某端口/事件 → 进入工作流某节点(如造物落子回调)。P1 仅占位、不消费。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WfTrigger {
    pub port: String,
    pub to: String,
}

/// 工作流节点 = 引导词 + 可选工具子集 + 流转规则。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// 节点 id(工作流内唯一)。
    pub id: String,
    /// 聚焦引导词:进入本节点时注入 system,告诉 AI 这一步该做什么。
    pub prompt: String,
    /// 本节点允许的工具子集(**物理锁死**选择空间)。脊柱过滤时另会无条件保留 finish/ask_user 防卡死。
    pub tools: Vec<String>,
    /// 流转规则。
    pub next: Vec<Transition>,
}

impl Node {
    /// 据本轮调用过的工具名,判定应流转到哪个节点。
    /// 优先匹配具体 `ToolCalled`(更精确),其次 `Always`;都不匹配 = 留在原节点(下轮继续)。
    pub fn transition_for(&self, called_tools: &[String]) -> Option<&str> {
        for t in &self.next {
            if let TransitionOn::ToolCalled(name) = &t.on {
                if called_tools.iter().any(|c| c == name) {
                    return Some(&t.to);
                }
            }
        }
        for t in &self.next {
            if matches!(t.on, TransitionOn::Always) {
                return Some(&t.to);
            }
        }
        None
    }
}

/// 工作流 = 有向节点图(强制顺序流转)。注册为动态工具后,LLM 调它即进入(工作流即动态工具)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    /// 工作流名 = 注册成的动态工具名(全局唯一)。
    pub name: String,
    /// 给 LLM 的动态工具描述(它据此决定何时调用进入)。
    pub description: String,
    pub scope: WorkflowScope,
    pub nodes: Vec<Node>,
    /// 入口节点 id。
    pub entry: String,
    /// 造物作用域绑定的画布 id(scope=Artifact 时设;其它作用域为 None)。
    /// 端口触发据此把某画布的交互回调映射到本工作流(见 `triggers` + 07 推论3)。
    #[serde(default)]
    pub canvas: Option<String>,
    /// 触发器(P2):端口/事件 → 进入工作流某节点。造物落子等交互回调即"端口"。
    #[serde(default)]
    pub triggers: Vec<WfTrigger>,
}

impl Workflow {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// 入口节点是否存在(注册前的最小校验)。
    pub fn entry_exists(&self) -> bool {
        self.node(&self.entry).is_some()
    }

    /// 据端口名找触发目标节点(P2 端口入口:一触发即进入该节点跑整套强制流转)。
    pub fn trigger_for(&self, port: &str) -> Option<&str> {
        self.triggers.iter().find(|t| t.port == port).map(|t| t.to.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf() -> Workflow {
        Workflow {
            name: "demo".into(),
            description: "d".into(),
            scope: WorkflowScope::Global,
            entry: "n1".into(),
            canvas: None,
            triggers: vec![WfTrigger { port: "tick".into(), to: "n2".into() }],
            nodes: vec![
                Node {
                    id: "n1".into(),
                    prompt: "第一步".into(),
                    tools: vec!["file_write".into()],
                    next: vec![Transition { on: TransitionOn::ToolCalled("file_write".into()), to: "n2".into() }],
                },
                Node {
                    id: "n2".into(),
                    prompt: "第二步".into(),
                    tools: vec!["finish".into()],
                    next: vec![Transition { on: TransitionOn::Always, to: END_NODE.into() }],
                },
            ],
        }
    }

    #[test]
    fn node_lookup_and_entry() {
        let w = wf();
        assert!(w.entry_exists());
        assert_eq!(w.node("n1").unwrap().prompt, "第一步");
        assert!(w.node("missing").is_none());
    }

    #[test]
    fn transition_tool_called_takes_priority() {
        let n = &wf().nodes[0];
        // 调了 file_write → 流转 n2。
        assert_eq!(n.transition_for(&["file_write".into()]), Some("n2"));
        // 没调匹配工具 → 留在原节点。
        assert_eq!(n.transition_for(&["file_read".into()]), None);
        assert_eq!(n.transition_for(&[]), None);
    }

    #[test]
    fn transition_always_fires_regardless() {
        let n = &wf().nodes[1];
        assert_eq!(n.transition_for(&[]), Some(END_NODE));
        assert_eq!(n.transition_for(&["anything".into()]), Some(END_NODE));
    }

    #[test]
    fn trigger_for_maps_port_to_node() {
        let w = wf();
        assert_eq!(w.trigger_for("tick"), Some("n2"));
        assert_eq!(w.trigger_for("unknown"), None);
    }

    #[test]
    fn workflow_roundtrips_json() {
        let w = wf();
        let s = serde_json::to_string(&w).unwrap();
        let back: Workflow = serde_json::from_str(&s).unwrap();
        assert_eq!(w, back);
    }
}
