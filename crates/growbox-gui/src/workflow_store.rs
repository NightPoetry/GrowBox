//! 工作流运行时存储 —— 注册表的"工作流即动态工具"那一半。
//!
//! 实现 `设计/07-工作流机制.md` 原则2(工作流即动态工具):
//! - `define_workflow` 执行器把一个工作流注册进这里(`define`);
//! - 注册表据此把每个工作流暴露成一个**动态工具**(`tool_defs`),LLM 调它即进入;
//! - 脊柱据当前节点向这里查工作流(`get`)做工具过滤 + 引导注入 + 强制流转。
//!
//! 与 `TaskManager` 同构:`Arc` 共享、内部用 `Mutex` 可变 —— `define_workflow` 执行器持一份克隆
//! 写入,`Registry` 持同一份克隆供脊柱读取。零新分发机制(复用唯一注册表 + 唯一分发路径)。
//!
//! ★P3 三层作用域持久化★(`设计/07` 推论2):`define` 据 `scope` 自动落库,切项目/启动时 `reload_scoped`
//! 重载——**全局**工作流出厂内置常驻;**项目**工作流落 redb(per-project kv 键,跨重启复用);
//! **造物**工作流落造物文件夹 `.growbox/artifacts/<canvas>/workflow.json`(随造物生灭,再进即在)。
//! 持久化是"存在即落、重载即回",中途态不持久(跨 run 续跑靠端口触发,见 P2)。无持久上下文(测试/未连接)
//! 则纯内存(within-session 可用)。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use growbox_core::{Node, ToolDef, Transition, TransitionOn, Workflow, WorkflowScope};
use growbox_memory::Store;
use parking_lot::Mutex;

/// 持久化目标(由 `state` 在切换项目时注入;无 = 纯内存)。`Store` 内含 Arc,克隆廉价。
#[derive(Default, Clone)]
struct PersistCtx {
    /// redb(项目工作流落此;无 store = 纯内存测试/未连接)。
    store: Option<Store>,
    /// 当前项目工作目录(造物工作流落 `.growbox/artifacts/<canvas>/`;扫描重载亦据此)。
    work_dir: Option<PathBuf>,
    /// 当前项目 id(项目工作流的 redb 键按它分桶,免串项目)。
    project_id: Option<String>,
}

/// 工作流存储(运行时)。
#[derive(Default)]
pub struct WorkflowStore {
    inner: Mutex<HashMap<String, Workflow>>,
    /// P3 持久化目标(切项目时注入)。
    persist: Mutex<PersistCtx>,
}

/// 项目工作流在 redb 的 kv 键(按 project_id 分桶,免不同项目串台)。
fn project_kv_key(project_id: &str) -> String {
    format!("wf_project::{project_id}")
}

impl WorkflowStore {
    /// 出厂:载入内置全局工作流(P1 验证 1 个)。
    pub fn with_builtins() -> Arc<Self> {
        let store = Arc::new(Self::default());
        for wf in builtin_global_workflows() {
            store.define(wf);
        }
        store
    }

    /// 注册/覆盖一个工作流(同名覆盖),并据作用域自动落库(P3)。
    /// Global 不落(出厂内置);Project 落 redb(整桶重写);Artifact 落造物文件夹。无持久上下文则纯内存。
    pub fn define(&self, wf: Workflow) {
        let scope = wf.scope;
        let canvas = wf.canvas.clone();
        self.inner.lock().insert(wf.name.clone(), wf);
        match scope {
            WorkflowScope::Global => {}
            WorkflowScope::Project => self.persist_project(),
            WorkflowScope::Artifact => {
                if let Some(cid) = canvas {
                    self.persist_artifact(&cid);
                }
            }
        }
    }

    /// 注入持久化目标(`state` 切换项目时调:store/work_dir/project_id 随之更新)。
    pub fn set_persist_context(&self, store: Option<Store>, work_dir: PathBuf, project_id: Option<String>) {
        *self.persist.lock() = PersistCtx { store, work_dir: Some(work_dir), project_id };
    }

    /// 重载当前作用域工作流(P3):清掉非全局(旧项目/造物的),从 redb 载本项目工作流 +
    /// 扫描造物文件夹载造物工作流。全局内置常驻不动。切项目/启动后调,使持久工作流跨重启复用。
    pub fn reload_scoped(&self) {
        let ctx = self.persist.lock().clone();
        // 清掉非全局(切项目时旧项目/造物工作流不该残留;全局内置保留)。
        self.inner.lock().retain(|_, wf| wf.scope == WorkflowScope::Global);
        // 项目工作流:从 redb 按 project_id 取整桶。
        if let (Some(store), Some(pid)) = (&ctx.store, &ctx.project_id) {
            if let Some(wfs) = store.kv_get::<Vec<Workflow>>(&project_kv_key(pid)) {
                let mut map = self.inner.lock();
                for wf in wfs {
                    map.insert(wf.name.clone(), wf);
                }
            }
        }
        // 造物工作流:扫描 `.growbox/artifacts/*/workflow.json`(再进造物即在)。
        if let Some(work_dir) = &ctx.work_dir {
            for wf in scan_artifact_workflows(work_dir) {
                self.inner.lock().insert(wf.name.clone(), wf);
            }
        }
    }

    /// 把当前所有 Project 作用域工作流整桶写回 redb(键按 project_id)。无 store/project_id 则跳过(纯内存)。
    fn persist_project(&self) {
        let ctx = self.persist.lock().clone();
        let (Some(store), Some(pid)) = (ctx.store, ctx.project_id) else { return };
        let wfs: Vec<Workflow> = self
            .inner
            .lock()
            .values()
            .filter(|w| w.scope == WorkflowScope::Project)
            .cloned()
            .collect();
        store.kv_put(&project_kv_key(&pid), &wfs);
    }

    /// 把某造物工作流写进它的造物文件夹 `.growbox/artifacts/<canvas>/workflow.json`。无 work_dir 则跳过。
    fn persist_artifact(&self, canvas_id: &str) {
        let ctx = self.persist.lock().clone();
        let Some(work_dir) = ctx.work_dir else { return };
        let Some(wf) = self
            .inner
            .lock()
            .values()
            .find(|w| w.scope == WorkflowScope::Artifact && w.canvas.as_deref() == Some(canvas_id))
            .cloned()
        else {
            return;
        };
        let dir = crate::artifact_fs::artifact_dir(&work_dir, canvas_id);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("造物工作流落盘失败(建目录 {}): {e}", dir.display());
            return;
        }
        match serde_json::to_vec_pretty(&wf) {
            Ok(bytes) => {
                if let Err(e) = std::fs::write(dir.join("workflow.json"), bytes) {
                    eprintln!("造物工作流落盘失败(写 {}): {e}", dir.display());
                }
            }
            Err(e) => eprintln!("造物工作流序列化失败: {e}"),
        }
    }

    /// 取一个工作流的克隆(脊柱读取用;工作流很小,克隆免持锁跨 await)。
    pub fn get(&self, name: &str) -> Option<Workflow> {
        self.inner.lock().get(name).cloned()
    }

    pub fn names(&self) -> Vec<String> {
        self.inner.lock().keys().cloned().collect()
    }

    /// 端口触发解析(P2):某画布来了交互回调(port),找绑定该画布、且有匹配 port 触发器的工作流,
    /// 返回 `(工作流名, 目标节点)`。脊柱据此从该节点进入跑整套强制流转(07 推论3 端口入口)。
    /// 单画布通常只绑一个造物工作流;取首个匹配。
    pub fn resolve_trigger(&self, canvas_id: &str, port: &str) -> Option<(String, String)> {
        let inner = self.inner.lock();
        for wf in inner.values() {
            if wf.canvas.as_deref() == Some(canvas_id) {
                if let Some(node) = wf.trigger_for(port) {
                    return Some((wf.name.clone(), node.to_string()));
                }
            }
        }
        None
    }

    /// 每个工作流暴露成一个动态工具定义(name = 工作流名;description = 工作流描述)。
    /// ★栈函数 v2★:参数 = 调用方"函数调用签名"(见 设计/07 v2 原则3)——调用方决定喂什么上下文、要带回什么、
    /// 怎么调(栈/直接)、循环上限。全部可选;最常见就是直接调用进入(全默认)。
    pub fn tool_defs(&self) -> Vec<ToolDef> {
        self.inner.lock().values().map(wf_tool_def).collect()
    }

    /// ★二期 C2 物化过滤(见 02-process-kind落地 M3 + 05-MCP M1)★:只暴露 **Global**(出厂内置入口流,
    /// 始终可用)+ 在 `allow` 名单里的工作流——被召回的"可执行流程"据 `wf:` 名物化进来(后续任务可栈调用)。
    /// 懒加载开时脊柱用它治理工作流数:工作流库可无界生长(项目/造物专属、未来更多结晶流程),但每轮
    /// 只暴露相关的几个,tools 字段恒定 → 缓存前缀稳。懒加载关时脊柱仍用全量 `tool_defs`(逐字不变)。
    pub fn tool_defs_scoped(&self, allow: &HashSet<String>) -> Vec<ToolDef> {
        self.inner
            .lock()
            .values()
            .filter(|wf| wf.scope == WorkflowScope::Global || allow.contains(&wf.name))
            .map(wf_tool_def)
            .collect()
    }
}

/// 把一个工作流转成它暴露的动态工具定义(name = 工作流名;参数 = 栈函数 v2 调用签名)。
/// `tool_defs` / `tool_defs_scoped` 共用,签名单一源。
fn wf_tool_def(wf: &Workflow) -> ToolDef {
    ToolDef {
        name: wf.name.clone(),
        description: wf.description.clone(),
        params: serde_json::json!({
            "type": "object",
            "properties": {
                "input": {
                    "type": "string",
                    "description": "喂给被调工作流的输入(你当场撰写,可裁剪只给关键信息——最少充分信息原则,别灌全量噪音)。"
                },
                "return_spec": {
                    "type": "string",
                    "description": "你要它带回什么、什么格式(它完成后据此调 workflow_return)。默认它只回最少充分信息(错误/警告/结论)。"
                },
                "context_mode": {
                    "type": "string",
                    "enum": ["inherit", "isolated", "fork"],
                    "description": "上下文模式(子任务三选一,按它需不需要你的全部背景、要不要把过程留下来选):inherit=继承你当前全量上下文、分支工作留在你这里(默认,轻量续做);isolated=只看你给的 input + 系统提示、你的对话被隐藏、退出只回摘要(自洽的调查/审核子任务,裁噪音、可并行);fork=继承你当前全量上下文、但分支工作不回灌、退出只回摘要(需要你全部背景的繁重机械活,如大范围重构/反复编译,它什么都知道、只把结论交回、过程噪音不污染你)。"
                },
                "direct": {
                    "type": "boolean",
                    "description": "调用方式:false=栈调用(默认,压栈,它完成后返回到你这里继续);true=直接调用(尾调用,不压栈、栈深恒定,顺流而下/循环用,不返回到你)。"
                },
                "max_loops": {
                    "type": "integer",
                    "description": "栈调用时给被调分支的直接调用循环次数上限(-1=无限,默认 -1,慎用)。分支内若用直接调用循环处理,达此上限会被强制返回。"
                }
            }
        }),
    }
}

/// 扫描造物状态根 `.growbox/artifacts/*/workflow.json`,反序列化出每个造物的造物工作流(P3)。
/// 不存在/读不出/解析失败的逐个跳过(造物文件夹是可写状态目录,容忍脏数据,不致命)。
fn scan_artifact_workflows(work_dir: &std::path::Path) -> Vec<Workflow> {
    let root = work_dir.join(crate::artifact_fs::ARTIFACTS_REL_ROOT);
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&root) else { return out };
    for entry in entries.flatten() {
        let path = entry.path().join("workflow.json");
        if let Ok(bytes) = std::fs::read(&path) {
            match serde_json::from_slice::<Workflow>(&bytes) {
                Ok(wf) => out.push(wf),
                Err(e) => eprintln!("造物工作流读取失败({}): {e}", path.display()),
            }
        }
    }
    out
}

/// 内置全局工作流(出厂随系统;见 07 推论2 全局作用域)。
///
/// 通用最佳实践工作流随系统内置(不硬编码项目专属);项目/造物专属由 AI 经 define_workflow 自建。
/// ① **造物创建工作流**——强制顺序的典范(先想清结构再创建,再核对收尾)。
/// ② **命令安全审查工作流**(栈函数 v2 旗舰示范,用户原例)——执行命令前的可复用"子函数"。
fn builtin_global_workflows() -> Vec<Workflow> {
    vec![command_safety_workflow(), financial_action_gate_workflow(), investigate_workflow(), Workflow {
        name: "create_artifact_workflow".into(),
        description: "造物创建工作流:需要新建一个可交互的造物(UI/小工具/小游戏)时调用,\
                      按既定步骤(先想清结构与交互、再一次性创建、最后核对收尾)完成,避免边想边反复重画。"
            .into(),
        scope: WorkflowScope::Global,
        entry: "design".into(),
        canvas: None,
        triggers: vec![],
        nodes: vec![
            Node {
                id: "design".into(),
                prompt: "造物创建·第一步(构思):先想清这个造物要做什么——\
                         结构、用户怎么交互、需要保存哪些状态。需要时可用 file_read/file_list 查看已有资料。\
                         想清楚后,用 render_artifact 一次性创建完整自包含的造物(交互即时本地响应、\
                         在脚本里注册 window.gxOnCommand 接收你的指令)。"
                    .into(),
                tools: vec!["render_artifact".into(), "file_read".into(), "file_list".into()],
                next: vec![Transition {
                    on: TransitionOn::ToolCalled("render_artifact".into()),
                    to: "review".into(),
                }],
            },
            Node {
                id: "review".into(),
                prompt: "造物创建·第二步(核对收尾):造物已创建。核对它是否符合需求——\
                         若需调整,再次 render_artifact 重画;满意则调用 finish 收尾。"
                    .into(),
                tools: vec!["render_artifact".into()],
                // 终态节点:满意 → finish(退出整条 Agent 循环);需继续调整 → render_artifact(无流转,留在本节点再核对)。
                next: vec![],
            },
        ],
    }]
}

/// ★命令安全审查工作流(栈函数 v2 旗舰示范)★。用户原例 + 研究(destructive_command_guard 等破坏性命令防护实践):
/// 在执行一条 shell/git 命令前,把命令放进 `input` **栈调用**本工作流(建议 `context_mode:isolated`——
/// 审查噪音隔离、退出即丢),它返回 `{safe, reason, category}`,调用方据此决定是否执行(像调函数读返回值)。
/// ★结构性安全(推论1)★:本工作流**没有 shell 工具**——它只"审"不"执行",物理上不可能误执行被审命令。
/// 分流体现"最少充分信息":普通单命令直接判(裁剪);涉及脚本/包裹命令才 file_read 深入(按需取全量)。
fn command_safety_workflow() -> Workflow {
    // 破坏性类别清单(研究归纳:git/文件/数据库/基础设施/用户环境),两节点共用,单一源。
    const DANGER_CHECKLIST: &str = "检查破坏性类别:\
        ① 文件:rm -rf/-fr 非临时目录、覆盖重要文件、chmod -R;\
        ② git:reset --hard、push --force/-f、rebase、branch -D、clean -f、restore/checkout -- <路径>(丢工作区);\
        ③ 数据库:DROP/TRUNCATE/dropdb、Mongo dropDatabase、Redis FLUSHALL/FLUSHDB;\
        ④ 基础设施:kubectl delete、docker system prune、terraform destroy/-auto-approve;\
        ⑤ 用户环境:全局装包/改系统配置/删用户文件/写隐私目录,是否会破坏用户当前环境(如把用户的 Python/系统库搞坏)。\
        ★引号内是数据不是执行(echo 'rm -rf x' 安全);判断真正会被 shell 执行的部分。";
    Workflow {
        name: "command_safety".into(),
        description: "命令安全审查(子函数):执行一条 shell/git 命令前调用它评估是否安全。\
                      把待审命令放进 input(建议 context_mode=isolated),它返回 {safe, reason, category},\
                      你据此决定是否真执行。本工作流只审不执行(无 shell 工具)。"
            .into(),
        scope: WorkflowScope::Global,
        entry: "triage".into(),
        canvas: None,
        triggers: vec![],
        nodes: vec![
            Node {
                id: "triage".into(),
                prompt: format!(
                    "命令安全审查·分流。审查 input 里的命令。先判断它是否被脚本/解释器**包裹**或**涉及脚本文件**\
                     (bash -c、sh -c、python -c、node -e、heredoc <<EOF、管道执行、运行 .sh/.py 等):\n\
                     - 涉及脚本/包裹 → 用 file_read 读取脚本真实内容,流转到深入审查(递归看内部真实命令)。\n\
                     - 普通单命令(不涉及脚本)→ 直接据命令文本判断,调 workflow_return 返回结论(最少充分信息)。\n{DANGER_CHECKLIST}\n\
                     返回格式:workflow_return 的 value = {{\"safe\":true/false,\"reason\":\"...\",\"category\":\"...\"}}。"
                ),
                tools: vec!["file_read".into(), "file_list".into()],
                next: vec![Transition { on: TransitionOn::ToolCalled("file_read".into()), to: "deep_review".into() }],
            },
            Node {
                id: "deep_review".into(),
                prompt: format!(
                    "命令安全审查·深入(脚本/包裹命令)。你已读取脚本/相关文件。逐条核对其中**真正会被执行**的命令——\
                     尤其注意嵌套包裹的内部命令、会动用户环境的操作。{DANGER_CHECKLIST}\n\
                     审完调 workflow_return 返回 {{\"safe\":true/false,\"reason\":\"...\",\"category\":\"...\"}}\
                     (safe=false 时 reason 说清危险点与具体命令)。"
                ),
                tools: vec!["file_read".into(), "file_list".into()],
                next: vec![],
            },
        ],
    }
}

/// ★金融操作授权闸(预编排工作流,B 软闸·栈函数)★。用户 2026-06-16 决策:金融安全不另起炉灶,
/// 做成一个**可调内置工作流**(同 `command_safety`)——AI 在提交"真实金融交易"前,把该操作放进 `input`
/// **栈调用**本工作流,它返回 `{authorized, reason}`,AI 据此决定提交不提交(像调函数读返回值;AI 服从结论 = 软闸的"拦")。
///
/// ★为什么不在派发层硬拦(用户问"硬拦你怎么判断是不是金融")★:派发层机械正则判不准"是不是金融"——
/// 中文/图标按钮会漏、"查看支付记录"这类会误拦、点击式付款连表单都不是。金融判断交给**人在首次授权时拍板**,
/// 不交给机器。安全偏置 = 拿不准就问(宁可多问,绝不少问),这对金融是正确方向。
/// 用户选 B 的理由"以后可以加更多东西":更多受控流程(删库闸/部署闸…)都按此模式加工作流,不碰派发层。
///
/// ★结构性安全(推论1)★:授权节点**没有任何提交/下单工具**(tools 为空,只剩恒可用的 ask_user/workflow_return)——
/// 物理上不可能在"问授权"时替用户提交。持久(批过记住)v1 走记忆召回(soft;拿不准重问 = 安全);
/// 硬性按项目落盘授权 + 决定脊柱模态弹窗是后续增量。
fn financial_action_gate_workflow() -> Workflow {
    Workflow {
        name: "financial_action_gate".into(),
        description: "金融操作授权闸(子函数):提交真实金融交易(下单/支付/转账/扣款)前调用它。\
                      把待做的金融操作放进 input(用默认上下文、别用 isolated,好让它看到本项目是否已授权),\
                      它查本项目授权历史、没有就首次问用户,返回 {authorized, reason};\
                      你据此决定是否真提交(authorized=false 就别提交、交回用户)。本工作流只问/只记/只返回,无提交工具。"
            .into(),
        scope: WorkflowScope::Global,
        entry: "authorize".into(),
        canvas: None,
        triggers: vec![],
        nodes: vec![Node {
            id: "authorize".into(),
            prompt: "金融操作授权检查。调用方把即将进行的真实金融操作(下单/支付/转账/扣款等)放在了 input 里。\n\
                ① 先看你当前上下文与记忆里,本项目以前是否已有'用户授权 GrowBox 自动执行金融操作'的结论。\n\
                ② 明确已授权 → 直接 workflow_return,value = {\"authorized\": true, \"reason\": \"本项目此前已获用户授权\"}。\n\
                ③ 没有授权历史 / 拿不准(安全偏置:拿不准就问,宁可多问绝不少问)→ 用 ask_user 问用户,说清:\
                这是哪个金融操作、本项目还没有自动金融授权历史、批准则本项目以后放行、不批则交回他手动测试。\n\
                   - 用户批准 → 在你的回复里明确写下一句'本项目已获用户授权:GrowBox 可自动执行金融操作'(供以后召回放行),\
                再 workflow_return value = {\"authorized\": true, \"reason\": \"用户首次授权,已记住本项目\"}。\n\
                   - 用户拒绝 / 要自己测 → workflow_return value = {\"authorized\": false, \"reason\": \"用户未授权,交回用户手动测试\"}。\n\
                本节点只能问(ask_user)或返回(workflow_return),没有任何提交/下单工具——物理上不可能在'问授权'时替用户提交。"
                .into(),
            // 空 tools:只剩恒可用的 ask_user/workflow_return(WF_ALWAYS_AVAILABLE)→ 结构性无提交能力。
            tools: vec![],
            next: vec![],
        }],
    }
}

/// ★并行调查员工作流(只读子代理)★。给临时勘探一个可调对象(见 `设计/07-附录-并行子代理`):
/// 把要查什么放进 `input`、`context_mode=isolated`;要并行就在同一回合调它多次(各喂不同 input)——
/// 脊柱把它们作为一次性子运行**并发**跑、各自只读勘探、各回一份摘要。
/// ★结构性安全(推论1)★:probe 节点只有只读工具(file_read/file_list/code_search)——子代理写不了、
/// 改不了、也派不出下一层(无工作流入口工具 → 无嵌套并行/fork-bomb)。
fn investigate_workflow() -> Workflow {
    Workflow {
        name: "investigate".into(),
        description: "并行调查员(只读子函数):把一块自洽的调查/勘探任务交给只读子代理。把要查什么放进 input、\
                      context_mode=isolated;要并行就在同一回合调它多次(各喂不同 input),它们会并发跑、各自只读勘探、\
                      各回一份摘要给你。它只读(file_read/file_list/code_search),写不了任何东西、也派不出别的子代理。"
            .into(),
        scope: WorkflowScope::Global,
        entry: "probe".into(),
        canvas: None,
        triggers: vec![],
        nodes: vec![Node {
            id: "probe".into(),
            prompt: "调查员·勘探(只读)。调用方把要调查的目标放在了 input 里。用只读工具(file_read/file_list/code_search)\
                     查清它,然后调 finish,summary = 你的结论(按 return_spec 的格式;只回最少充分信息——关键发现/结论,\
                     别把读到的全量原文搬回去)。你是只读子代理:写不了、改不了、也派不出别的子代理;查清就 finish 交回结论。"
                .into(),
            tools: vec!["file_read".into(), "file_list".into(), "code_search".into()],
            next: vec![], // 单节点:查清即 finish 退出(finish 恒可用,见 WF_ALWAYS_AVAILABLE)。
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_register_and_expose_as_tool() {
        let store = WorkflowStore::with_builtins();
        let names = store.names();
        assert!(names.contains(&"create_artifact_workflow".to_string()));
        // 暴露成动态工具(工作流即动态工具)。
        let defs = store.tool_defs();
        assert!(defs.iter().any(|d| d.name == "create_artifact_workflow" && !d.description.is_empty()));
        // 取出来结构完整,入口节点存在。
        let wf = store.get("create_artifact_workflow").unwrap();
        assert!(wf.entry_exists());
        // "构思"节点工具子集物理上不含 push_artifact_notice(推论1)。
        let design = wf.node("design").unwrap();
        assert!(!design.tools.iter().any(|t| t == "push_artifact_notice"));
    }

    /// ★命令安全审查工作流(栈函数 v2 旗舰)★:内置注册;结构性安全=所有节点都没有 shell(只审不执行);
    /// 分流节点据 file_read 流转到深入审查(涉及脚本才读全量=最少充分信息)。
    #[test]
    fn command_safety_workflow_is_builtin_and_cannot_execute() {
        let store = WorkflowStore::with_builtins();
        assert!(store.names().contains(&"command_safety".to_string()));
        let wf = store.get("command_safety").unwrap();
        assert!(wf.entry_exists());
        // ★推论1 结构性安全★:审查工作流的任何节点都不含 shell —— 物理上不可能误执行被审命令。
        for node in &wf.nodes {
            assert!(!node.tools.iter().any(|t| t == "shell"), "命令安全审查节点不应有 shell(只审不执行)");
        }
        // 分流 → 涉及脚本(file_read)→ 深入审查。
        let triage = wf.node("triage").unwrap();
        assert_eq!(triage.transition_for(&["file_read".into()]), Some("deep_review"));
        assert_eq!(triage.transition_for(&[]), None, "普通命令不流转(直接 workflow_return 返回)");
    }

    /// ★金融操作授权闸(B 软闸·栈函数)★:内置注册 + 暴露成 Global 入口工具;
    /// 结构性安全(推论1)=授权节点无任何提交/下单/网页操作工具(只剩恒可用的 ask_user/workflow_return)。
    #[test]
    fn financial_action_gate_is_builtin_and_has_no_submit_tools() {
        let store = WorkflowStore::with_builtins();
        assert!(store.names().contains(&"financial_action_gate".to_string()));
        let wf = store.get("financial_action_gate").unwrap();
        assert!(wf.entry_exists());
        // 暴露成 Global 入口工具(可栈调用),描述非空。
        let defs = store.tool_defs();
        assert!(defs.iter().any(|d| d.name == "financial_action_gate" && !d.description.is_empty()));
        // ★推论1 结构性安全★:授权节点不含任何提交类工具 —— 问授权时物理上不可能替用户提交。
        for node in &wf.nodes {
            for forbidden in ["web_debug_drive", "shell", "render_artifact", "submit", "file_write"] {
                assert!(!node.tools.iter().any(|t| t == forbidden), "授权节点不应有提交类工具 {forbidden}");
            }
        }
    }

    #[test]
    fn define_overwrites_same_name() {
        let store = WorkflowStore::default();
        let mk = |desc: &str| Workflow {
            name: "w".into(),
            description: desc.into(),
            scope: WorkflowScope::Global,
            entry: "a".into(),
            canvas: None,
            triggers: vec![],
            nodes: vec![Node { id: "a".into(), prompt: "p".into(), tools: vec![], next: vec![] }],
        };
        store.define(mk("first"));
        store.define(mk("second"));
        assert_eq!(store.get("w").unwrap().description, "second");
        assert_eq!(store.names().len(), 1);
    }

    #[test]
    fn resolve_trigger_matches_canvas_and_port() {
        use growbox_core::WfTrigger;
        let store = WorkflowStore::default();
        store.define(Workflow {
            name: "gomoku_play".into(),
            description: "五子棋".into(),
            scope: WorkflowScope::Artifact,
            entry: "ai_move".into(),
            canvas: Some("gomoku".into()),
            triggers: vec![WfTrigger { port: "place".into(), to: "ai_move".into() }],
            nodes: vec![Node { id: "ai_move".into(), prompt: "落子".into(), tools: vec!["artifact_command".into()], next: vec![] }],
        });
        // 画布 + 端口都对 → 进入 ai_move 节点。
        assert_eq!(store.resolve_trigger("gomoku", "place"), Some(("gomoku_play".into(), "ai_move".into())));
        // 画布对、端口不对 → 无。
        assert_eq!(store.resolve_trigger("gomoku", "hover"), None);
        // 画布不对 → 无(别的画布交互不串台)。
        assert_eq!(store.resolve_trigger("other", "place"), None);
    }

    fn project_wf(name: &str) -> Workflow {
        Workflow {
            name: name.into(),
            description: "打包".into(),
            scope: WorkflowScope::Project,
            entry: "a".into(),
            canvas: None,
            triggers: vec![],
            nodes: vec![Node { id: "a".into(), prompt: "p".into(), tools: vec!["shell".into()], next: vec![] }],
        }
    }

    fn artifact_wf(name: &str, canvas: &str) -> Workflow {
        use growbox_core::WfTrigger;
        Workflow {
            name: name.into(),
            description: "对弈".into(),
            scope: WorkflowScope::Artifact,
            entry: "ai_move".into(),
            canvas: Some(canvas.into()),
            triggers: vec![WfTrigger { port: "place".into(), to: "ai_move".into() }],
            nodes: vec![Node { id: "ai_move".into(), prompt: "落子".into(), tools: vec!["artifact_command".into()], next: vec![] }],
        }
    }

    // ===== P3 三层作用域持久化 =====

    #[test]
    fn project_workflow_persists_to_redb_and_reloads_per_project() {
        let dir = tempfile::tempdir().unwrap();
        let redb = Store::open(dir.path().join("t.redb")).unwrap();
        let s = WorkflowStore::with_builtins();
        s.set_persist_context(Some(redb.clone()), dir.path().to_path_buf(), Some("proj1".into()));
        s.define(project_wf("pack")); // 项目作用域 → 落 redb
        assert!(s.get("pack").is_some());

        // 模拟重启:全新 store 实例(同 redb、同项目)→ reload 从 redb 取回。
        let s2 = WorkflowStore::with_builtins();
        assert!(s2.get("pack").is_none(), "全新实例未 reload 前不该有");
        s2.set_persist_context(Some(redb.clone()), dir.path().to_path_buf(), Some("proj1".into()));
        s2.reload_scoped();
        assert!(s2.get("pack").is_some(), "项目工作流应从 redb 跨重启重载");
        assert!(s2.get("create_artifact_workflow").is_some(), "全局内置仍常驻");

        // 别的项目不该看到 proj1 的工作流(按 project_id 分桶)。
        let s3 = WorkflowStore::with_builtins();
        s3.set_persist_context(Some(redb.clone()), dir.path().to_path_buf(), Some("proj2".into()));
        s3.reload_scoped();
        assert!(s3.get("pack").is_none(), "项目工作流按 project_id 分桶,不串项目");
    }

    #[test]
    fn artifact_workflow_persists_to_folder_and_reloads_on_scan() {
        let dir = tempfile::tempdir().unwrap();
        let s = WorkflowStore::with_builtins();
        s.set_persist_context(None, dir.path().to_path_buf(), None); // 造物作用域只需 work_dir
        s.define(artifact_wf("gomoku_play", "gomoku"));
        let f = dir.path().join(".growbox/artifacts/gomoku/workflow.json");
        assert!(f.exists(), "造物工作流应落进造物文件夹 workflow.json");

        // 模拟再进造物/重启:全新实例扫描造物文件夹重载。
        let s2 = WorkflowStore::with_builtins();
        s2.set_persist_context(None, dir.path().to_path_buf(), None);
        s2.reload_scoped();
        assert!(s2.get("gomoku_play").is_some(), "造物工作流应从造物文件夹扫描重载");
        // 重载后端口触发仍可路由(随造物复活)。
        assert_eq!(s2.resolve_trigger("gomoku", "place"), Some(("gomoku_play".into(), "ai_move".into())));
    }

    #[test]
    fn reload_scoped_drops_non_global_keeps_builtins() {
        let s = WorkflowStore::with_builtins();
        s.define(project_wf("old")); // 无持久上下文 → 仅内存
        assert!(s.get("old").is_some());
        s.reload_scoped(); // 空上下文重载:清非全局,全局内置保留
        assert!(s.get("old").is_none(), "切项目重载应清掉旧项目/造物工作流");
        assert!(s.get("create_artifact_workflow").is_some(), "全局内置常驻不动");
    }

    /// ★二期 C2 物化过滤★:`tool_defs_scoped` 只暴露 Global 入口流 + `allow` 名单(被召回的可执行流程)。
    /// 项目/造物工作流不在 allow 里就不暴露(治理工作流数,懒加载下保前缀稳);全量 `tool_defs` 仍含全部。
    #[test]
    fn tool_defs_scoped_exposes_global_plus_allowlisted() {
        let s = WorkflowStore::with_builtins();
        s.define(project_wf("pack")); // 项目作用域,内存即可
        s.define(artifact_wf("gomoku_play", "gomoku")); // 造物作用域

        let names = |defs: Vec<ToolDef>| defs.into_iter().map(|d| d.name).collect::<HashSet<_>>();

        // 空 allow:只 Global 内置入口流(create_artifact_workflow / command_safety / financial_action_gate),项目/造物的都不暴露。
        let scoped = names(s.tool_defs_scoped(&HashSet::new()));
        assert!(
            scoped.contains("create_artifact_workflow")
                && scoped.contains("command_safety")
                && scoped.contains("financial_action_gate"),
            "Global 入口流始终暴露"
        );
        assert!(!scoped.contains("pack"), "未物化的项目工作流不暴露(C2 治理)");
        assert!(!scoped.contains("gomoku_play"), "未物化的造物工作流不暴露(经端口触发进入,不必占 tools 字段)");

        // 物化 pack(被召回的可执行流程):它进 tools 字段供栈调用,Global 仍在,gomoku_play 仍不在。
        let allow: HashSet<String> = ["pack".to_string()].into_iter().collect();
        let scoped2 = names(s.tool_defs_scoped(&allow));
        assert!(scoped2.contains("pack"), "物化的可执行流程应暴露供栈调用");
        assert!(scoped2.contains("create_artifact_workflow"), "Global 仍在");
        assert!(!scoped2.contains("gomoku_play"), "未物化的仍不暴露");

        // 全量 tool_defs 不受 allow 影响,含全部(懒加载关时的旧行为)。
        let all = names(s.tool_defs());
        assert!(all.contains("pack") && all.contains("gomoku_play") && all.contains("create_artifact_workflow"));
    }
}
