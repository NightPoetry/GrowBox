//! 执行器注册表 + 唯一分发路径。
//!
//! 实现 `系统架构/06-app.md` + 架构公理:一处登记,一条分发。根治旧代码"三套分发打架"。
//! dispatch 是所有能力调用的唯一入口,且**只此一处**过安全门(judge → risk_gate)。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use growbox_core::{Claim, ExecCtx, Executor, ToolCall, ToolDef, ToolLimits, ToolResult, UiIntent, Workflow};
use growbox_safety::{risk_gate, Operation, Sandbox, Verdict};

use crate::tasks::TaskManager;
use crate::tool_i18n::ToolI18n;
use crate::workflow_store::WorkflowStore;

/// 工作流节点内**始终保留**的兜底工具(防卡死,见 07 推论6):无论节点工具子集怎么收窄,
/// AI 永远能 finish(完成)/ ask_user(求助)/ workflow_return(返回上层)退出,不会被锁进死胡同。
/// `workflow_return` 是栈函数 v2 的结构化返回原语(见 07「加强版」原则4):任何工作流节点都能返回上层。
pub const WF_ALWAYS_AVAILABLE: [&str; 3] = ["finish", "ask_user", crate::executors::WORKFLOW_RETURN];

/// ★二期 C1★:**永不可 deferred** 的核心工具——始终在 tools 字段(否则懒加载会把"返回上层/完成/求助/
/// 搜工具/加载技能"自己也藏掉,陷死循环)。即便用户把它们写进 deferred 名单,注册表也强制保留。
/// `load_skill` 是渐进披露 Skill 的加载枢纽(同 tool_search 加载工具),藏掉它就没法用任何 skill。
pub const NEVER_DEFER: [&str; 5] = [
    "finish",
    "ask_user",
    crate::executors::WORKFLOW_RETURN,
    crate::executors::TOOL_SEARCH,
    crate::executors::LOAD_SKILL,
];

/// 给前端工具开关面板的一张工具卡片(name = 英文 key 稳定;label/description 按 ui_lang 本地化)。
pub struct ToolCard {
    pub name: String,
    pub label: String,
    pub description: String,
}

/// 一次分发的结果。
pub enum Dispatch {
    /// 已执行,带结果。
    Done(ToolResult),
    /// 终止类:已执行,且脊柱应据此收口本次任务(如 finish)。
    Terminal(ToolResult),
    /// 提问类:已执行,脊柱应让位暂停等用户回答(如 ask_user),停于 `AwaitingUser`。
    AwaitingUser(ToolResult),
    /// 交互类:请前端弹预填 UI(控制反转)。
    Intent(UiIntent),
    /// 越界:需用户授权(带原因 + 触发的资源声明,供授权后重放)。
    NeedAuth { reason: String, claim: Option<Claim> },
    /// 命中黑名单,硬拒绝(授权也不放行)。
    Denied { reason: String },
}

/// 注册表:工具名 → 执行器。
#[derive(Default)]
pub struct Registry {
    execs: HashMap<String, Box<dyn Executor>>,
    /// 工具文案多语言单一源(编译期内嵌,见 `tool_i18n`)。definitions/ui_cards 按语言取词。
    i18n: ToolI18n,
    /// 工具调用统计(面板 error_rate 接真用)。dispatch 是 `&self`,故用原子内部可变。
    tool_calls: AtomicU64,
    tool_fails: AtomicU64,
    /// 工具输出上限旋钮(推论9 可设);连接时由 `set_limits` 从 Settings 注入,dispatch 注进 ExecCtx。
    limits: ToolLimits,
    /// 工作流存储(工作流即动态工具,见 07):`define_workflow` 执行器持同一份克隆写入,此处供脊柱读取。
    workflows: Arc<WorkflowStore>,
    /// LSP 管理器(二期 A1/A2):与 `lsp` 执行器**共享同一个** Arc。执行器查代码时起/暖语言服务器并
    /// 暂存其 publishDiagnostics;脊柱(A2 诊断推感知层)经此读编辑后诊断 → perceive。见 03-LSP集成 M2。
    lsp: Arc<crate::lsp::LspManager>,
    /// ★二期 C1 懒加载总开关★:关 = 旧行为(全工具直接可调 + 工作流按节点收窄);开 = 核心常驻 +
    /// deferred 露名 + tool_search 按需加载 + tools 字段恒定(修缓存破坏)。连接时由 `set_lazy_tools` 注入。
    lazy_tools: bool,
    /// ★二期 C1 deferred 工具名单★:lazy 开时不进 tools 字段、只露名(经 tool_search 加载)。
    /// 已剔除 `NEVER_DEFER`(强制常驻)。连接时由 `set_deferred_tools` 从 Settings 注入。
    deferred: HashSet<String>,
    /// ★二期 D1 MCP 客户端★:与连接命令**共享同一个** Arc(内部 Mutex 可变)。已连 server 的工具
    /// 经此暴露成动态执行器,走**同一条** dispatch 路径(架构公理);懒加载开时 MCP 工具恒 deferred
    /// (经 tool_search 按需加载,不撑爆上下文 —— 见 05-MCP M1)。空(未连任何 server)时全程零影响。
    mcp: Arc<crate::mcp::McpHub>,
    /// Web 工具配置(搜索 provider/key + 超时;与 web_fetch/web_search 执行器**共享同一份**)。
    /// 连接/改设置时经 `set_web_config` 写入(推论9 数值全可设)。
    web: crate::executors::SharedWebConfig,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 用内置执行器初始化(空面板目录)。测试/无前端场景用;`ui_control` 目录为空(无可控面板)。
    pub fn with_builtins(task_mgr: Arc<TaskManager>) -> Self {
        Self::with_builtins_catalog(task_mgr, crate::ui::empty_catalog())
    }

    /// 用内置执行器初始化,并把前端声明的面板目录交给 `ui_control`(真实 app 用)。
    pub fn with_builtins_catalog(task_mgr: Arc<TaskManager>, ui_catalog: crate::ui::UiSurfaceCatalog) -> Self {
        // 工作流存储:出厂含内置全局工作流。一份克隆给 define_workflow 写入,一份留在注册表供脊柱读取。
        let workflows = WorkflowStore::with_builtins();
        // LSP 管理器(语言服务器懒起复用),给 lsp 执行器。二期 A1。
        // ★A2★ 注册表与执行器共享同一个 Arc:执行器暂存的诊断,脊柱经 `lsp_manager()` 读得到。
        let lsp_mgr = Arc::new(crate::lsp::LspManager::new());
        let mut r = Self::new();
        r.workflows = workflows.clone();
        r.lsp = lsp_mgr.clone();
        // web 配置共享同一份:注册表留一份(set_web_config 写),执行器各持克隆(执行时读)。
        let web_cfg = r.web.clone();
        for e in crate::executors::builtins(task_mgr, ui_catalog, workflows, lsp_mgr, web_cfg) {
            r.register(e);
        }
        r
    }

    /// 注入 Web 工具配置(连接/改设置时从 Settings 透传;推论9 数值全可设)。
    pub fn set_web_config(&self, cfg: crate::executors::WebConfig) {
        *self.web.write() = cfg;
    }

    /// 共享的 LSP 管理器(A2 诊断推感知层):脊柱据此在编辑 .rs 后拉已暖 rust-analyzer 的诊断。
    pub fn lsp_manager(&self) -> &crate::lsp::LspManager {
        &self.lsp
    }

    /// ★二期 D1★ 共享的 MCP 连接中心:连接/断开 server 的命令(D2)经此操作同一份 hub。
    pub fn mcp_hub(&self) -> Arc<crate::mcp::McpHub> {
        self.mcp.clone()
    }

    pub fn register(&mut self, exec: Box<dyn Executor>) {
        self.execs.insert(exec.name().to_string(), exec);
    }

    /// 注入工具输出上限旋钮(连接时从 Settings 透传;推论9 数值全可设)。dispatch 据此构造 ExecCtx。
    pub fn set_limits(&mut self, limits: ToolLimits) {
        self.limits = limits;
    }

    /// ★二期 C1★:注入懒加载总开关 + deferred 名单(连接时从 Settings 透传)。
    /// 名单里 `NEVER_DEFER` 的项被剔除(强制常驻);未知名(如还没注册的 MCP 工具)保留无害。
    pub fn set_lazy_tools(&mut self, enabled: bool, deferred: Vec<String>) {
        self.lazy_tools = enabled;
        self.deferred = deferred.into_iter().filter(|n| !NEVER_DEFER.contains(&n.as_str())).collect();
    }

    /// 懒加载是否启用(脊柱据此决定走核心常驻+露名+节点门控,还是旧行为)。
    pub fn lazy_enabled(&self) -> bool {
        self.lazy_tools
    }

    /// 某工具当前是否 deferred(懒加载开 且 非强制常驻 且 (在名单 或 是 MCP 工具))。
    /// ★D1★ MCP 工具在懒加载开时**恒 deferred**(收编生态后工具成百上千,绝不能全塞 tools 字段;
    /// 经 tool_search 按需加载即可)——这正是 C1 懒加载为 MCP 铺的路(05-MCP M1)。
    fn is_deferred(&self, name: &str) -> bool {
        self.lazy_tools
            && !NEVER_DEFER.contains(&name)
            && (self.deferred.contains(name) || self.mcp.is_tool(name))
    }

    /// 给 LLM 的工具定义清单。description 按 prompt_lang 从单一源注入(`name` 恒英文 key);
    /// `ui_control` 的 `{catalog}` 占位用其 `desc_dynamic()`(运行时面板目录)替换;
    /// 各参数的 description 也按 prompt_lang 覆盖进 schema。
    pub fn definitions(&self, prompt_lang: &str) -> Vec<ToolDef> {
        self.execs
            .values()
            .map(|e| {
                let name = e.name();
                let mut def = e.definition();
                // ★提示词自转译★:工具说明经唯一 chokepoint(开了取主模型重写版,否则原文);
                // {catalog} 动态替换在转译之后(保真校验保留该占位符)。
                let raw = self.i18n.llm_desc(name, prompt_lang);
                let mut desc = crate::transpile::tr(
                    &format!("tool.{name}.llm_desc"),
                    crate::transpile::PromptRole::Main,
                    prompt_lang,
                    &raw,
                );
                if let Some(dynamic) = e.desc_dynamic() {
                    desc = desc.replace("{catalog}", &dynamic);
                }
                def.description = desc;
                self.i18n.inject_param_descs(name, prompt_lang, &mut def.params);
                self.transpile_param_descs(name, prompt_lang, &mut def.params);
                def
            })
            .collect()
    }

    /// ★提示词自转译★:把已注入的各参数 description 过一道 tr(开自转译则用主模型重写版)。
    fn transpile_param_descs(&self, name: &str, prompt_lang: &str, params: &mut serde_json::Value) {
        let Some(props) = params.get_mut("properties").and_then(|p| p.as_object_mut()) else {
            return;
        };
        for (pname, prop) in props.iter_mut() {
            let Some(desc) = prop.get("description").and_then(|d| d.as_str()).map(String::from) else {
                continue;
            };
            let t = crate::transpile::tr(
                &format!("tool.{name}.param.{pname}"),
                crate::transpile::PromptRole::Main,
                prompt_lang,
                &desc,
            );
            if let Some(obj) = prop.as_object_mut() {
                obj.insert("description".into(), serde_json::Value::String(t));
            }
        }
    }

    /// ★提示词自转译★:列出所有内置工具的 llm_desc 转译单元(zh+en),供「重写提示词」动作扫描。
    /// 跳过无真翻译(llm_desc 兜底成工具名)的项;不含 MCP/工作流动态工具(MCP 描述来自外部 server、
    /// 工作流描述是用户/AI 数据,均不转译)。角色 Main(工具说明给主模型看)。
    pub fn tool_desc_units(&self) -> Vec<crate::transpile::Unit> {
        use crate::transpile::{PromptRole, Unit};
        let mut v = Vec::new();
        for e in self.execs.values() {
            let name = e.name();
            let key = format!("tool.{name}.llm_desc");
            for lang in ["zh", "en"] {
                let original = self.i18n.llm_desc(name, lang);
                if original != name {
                    // 无真翻译(兜底成工具名)的不转译。
                    v.push(Unit { key: key.clone(), role: PromptRole::Main, lang, original });
                }
                // 参数 description 也纳入转译。
                for (pname, desc) in self.i18n.param_descs(name, lang) {
                    v.push(Unit {
                        key: format!("tool.{name}.param.{pname}"),
                        role: PromptRole::Main,
                        lang,
                        original: desc,
                    });
                }
            }
        }
        v
    }

    /// 各工作流暴露成的动态工具定义(工作流即动态工具,见 07 原则2)。LLM 调同名工具即进入工作流。
    pub fn workflow_defs(&self) -> Vec<ToolDef> {
        self.workflows.tool_defs()
    }

    /// ★二期 C2 物化过滤★:Global 入口流 + `materialized` 名单(被召回的可执行流程物化进来)。
    /// 懒加载开时 `tools_for` 用它治理工作流数,使工作流库无界生长而 tools 字段恒定(缓存稳)。
    pub fn workflow_defs_scoped(&self, materialized: &HashSet<String>) -> Vec<ToolDef> {
        self.workflows.tool_defs_scoped(materialized)
    }

    /// 取一个已注册工作流(脊柱进入/流转时查)。None = 该名不是工作流。
    pub fn workflow(&self, name: &str) -> Option<Workflow> {
        self.workflows.get(name)
    }

    /// 端口触发解析(P2):某画布来了交互回调(port)→ 返回应进入的 `(工作流名, 节点id)`。
    /// 造物交互回调据此路由进工作流(见 07 推论3 端口入口 + `WorkflowStore::resolve_trigger`)。
    pub fn resolve_trigger(&self, canvas_id: &str, port: &str) -> Option<(String, String)> {
        self.workflows.resolve_trigger(canvas_id, port)
    }

    /// P3 持久化:注入工作流持久目标(store/work_dir/project_id)。`state` 切项目时调,随后 `reload_workflows`。
    pub fn set_workflow_context(
        &self,
        store: Option<growbox_memory::Store>,
        work_dir: std::path::PathBuf,
        project_id: Option<String>,
    ) {
        self.workflows.set_persist_context(store, work_dir, project_id);
    }

    /// P3 持久化:重载当前作用域工作流(清非全局 → 从 redb 载项目 + 扫造物文件夹载造物)。切项目/启动后调。
    pub fn reload_workflows(&self) {
        self.workflows.reload_scoped();
    }

    /// 工作流机制:据当前工作流节点算本轮该暴露给 LLM 的工具集(见 07 推论6)。
    /// - `current == None`(普通模式):全部真实工具 + 各工作流动态入口工具。
    /// - `current == Some((wf, node))`(工作流节点内):收窄到 `node.tools`,
    ///   并集 finish/ask_user 防卡死。把"劝 AI 别用错工具"变成"AI 根本选不到错的"(推论1)。
    ///
    /// 兜底:工作流/节点查不到时回退普通模式(不至于让脊柱拿不到任何工具)。
    ///
    /// `materialized`(★二期 C2★):本 run 被召回的"可执行流程"引用的工作流名(物化进 tools 字段供栈调用)。
    /// 懒加载开时,工作流暴露 = Global 入口流 + 这批物化的(治理工作流数、保前缀稳);关时忽略(全量暴露,逐字不变)。
    pub fn tools_for(
        &self,
        prompt_lang: &str,
        current: Option<(&str, &str)>,
        materialized: &HashSet<String>,
    ) -> Vec<ToolDef> {
        // ★C1 懒加载开★:tools 字段 = 核心常驻(非 deferred)+ 工作流入口,**恒定不随节点变**
        // (修缓存破坏)。deferred 工具只露名(`deferred_listing`)+ tool_search 按需加载;
        // 节点工具收窄改由脊柱"派发时门控"(`node_allowed_tools`)+ tool_search 按允许名单过滤实现。
        if self.lazy_tools {
            let mut core: Vec<ToolDef> =
                self.definitions(prompt_lang).into_iter().filter(|d| !self.is_deferred(&d.name)).collect();
            // ★C2★:工作流入口不再全量常驻,而是 Global 入口流 + 本 run 物化的可执行流程(治理 + 前缀稳)。
            core.extend(self.workflow_defs_scoped(materialized));
            return core;
        }
        // ★懒加载关(默认)= 旧行为,逐字不变★(无 MCP server 时 mcp.tool_defs() 为空,逐字不变)。
        let mut all = self.definitions(prompt_lang);
        all.extend(self.workflow_defs());
        all.extend(self.mcp.tool_defs()); // D1:已连 MCP 工具(懒加载关时直接可调)
        let Some((wf_name, node_id)) = current else {
            // 普通模式(不在工作流里):workflow_return 无意义(没有上层可返回)→ 隐藏,避免误调。
            all.retain(|d| d.name != crate::executors::WORKFLOW_RETURN);
            return all;
        };
        let Some(wf) = self.workflows.get(wf_name) else { return all };
        let Some(node) = wf.node(node_id).cloned() else { return all };
        all.into_iter()
            .filter(|d| {
                node.tools.iter().any(|t| t == &d.name) || WF_ALWAYS_AVAILABLE.contains(&d.name.as_str())
            })
            .collect()
    }

    /// ★C1★ 某工作流节点允许调用的工具全集(节点工具子集 ∪ 永远保留的兜底)。
    /// 懒加载开时脊柱用它做"派发时门控":节点内调了不在此集的工具 → 拒绝 + 引导(软锁);
    /// `search_tools` 也按它过滤 deferred 工具(节点外搜不到 = 硬锁)。节点查不到 → None(不门控)。
    pub fn node_allowed_tools(&self, wf_name: &str, node_id: &str) -> Option<HashSet<String>> {
        let node = self.workflows.get(wf_name)?.node(node_id).cloned()?;
        let mut set: HashSet<String> = node.tools.iter().cloned().collect();
        // NEVER_DEFER(含 finish/ask_user/workflow_return 防卡死 + tool_search 才能加载工具)始终允许。
        for t in NEVER_DEFER {
            set.insert(t.to_string());
        }
        Some(set)
    }

    /// ★C1 deferred 露名★:懒加载开时给 LLM 的"未直接加载的工具名单"(几百字节,只名字)。
    /// `allowlist=Some(节点允许集)` 时只露节点内的(硬锁);`None`=普通模式露全部 deferred。
    /// 无 deferred(或都被允许名单挡掉)返回 None。露名进**稳定前缀**(脊柱拼进系统提示,缓存稳)。
    pub fn deferred_listing(&self, allowlist: Option<&HashSet<String>>) -> Option<String> {
        if !self.lazy_tools {
            return None;
        }
        let mut names: Vec<String> = self
            .execs
            .values()
            .map(|e| e.name().to_string())
            .filter(|n| self.is_deferred(n))
            .filter(|n| allowlist.map(|a| a.contains(n)).unwrap_or(true))
            .collect();
        // ★D1★ MCP 工具名一并露出(它们恒 deferred,经 tool_search 加载 schema 再调)。
        for n in self.mcp.tool_names() {
            if self.is_deferred(&n) && allowlist.map(|a| a.contains(&n)).unwrap_or(true) {
                names.push(n);
            }
        }
        if names.is_empty() {
            return None;
        }
        names.sort_unstable();
        Some(format!(
            "[可用但未加载的工具] 以下工具暂未加载 schema,需要时先调 tool_search(按名字或关键词)拉回其用法再调用:\n{}",
            names.join(", ")
        ))
    }

    /// ★C1 tool_search★:按 query 在 **deferred** 工具里检索,返回命中工具的完整定义文本(供 LLM 读后调用)。
    /// query 形态:`select:a,b`=精确取名;否则关键词(匹配工具名 + 描述,大小写不敏感)。
    /// `allowlist=Some(节点允许集)` 时只在节点内的 deferred 工具里搜(节点外搜不到=硬锁)。
    /// 已加载的核心工具不在搜索范围(它们本就在 tools 字段)。无命中给出明确提示(非失败)。
    pub fn search_tools(&self, query: &str, prompt_lang: &str, allowlist: Option<&HashSet<String>>) -> String {
        let q = query.trim();
        let selected: Option<Vec<String>> = q
            .strip_prefix("select:")
            .map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect());
        let ql = q.to_lowercase();
        let mut hits: Vec<ToolDef> = Vec::new();
        for e in self.execs.values() {
            let name = e.name();
            if !self.is_deferred(name) {
                continue; // 只搜 deferred(核心已加载)
            }
            if allowlist.map(|a| !a.contains(name)).unwrap_or(false) {
                continue; // 节点外:硬锁,搜不到
            }
            let desc = crate::transpile::tr(
                &format!("tool.{name}.llm_desc"),
                crate::transpile::PromptRole::Main,
                prompt_lang,
                &self.i18n.llm_desc(name, prompt_lang),
            );
            let matched = match &selected {
                Some(sel) => sel.iter().any(|s| s == name),
                None => name.to_lowercase().contains(&ql) || desc.to_lowercase().contains(&ql),
            };
            if matched {
                let mut def = e.definition();
                def.description = desc;
                self.i18n.inject_param_descs(name, prompt_lang, &mut def.params);
                self.transpile_param_descs(name, prompt_lang, &mut def.params);
                hits.push(def);
            }
        }
        // ★D1★ MCP 工具一并可搜(描述来自 server,非 i18n)。同样按 deferred + 节点允许名单过滤。
        for def in self.mcp.tool_defs() {
            let name = def.name.as_str();
            if !self.is_deferred(name) {
                continue;
            }
            if allowlist.map(|a| !a.contains(name)).unwrap_or(false) {
                continue;
            }
            let matched = match &selected {
                Some(sel) => sel.iter().any(|s| s == name),
                None => name.to_lowercase().contains(&ql) || def.description.to_lowercase().contains(&ql),
            };
            if matched {
                hits.push(def);
            }
        }
        if hits.is_empty() {
            return format!(
                "tool_search「{q}」:无匹配的可加载工具。可调 tool_search 用更宽的关键词,或确认该工具在当前可用范围内。"
            );
        }
        hits.sort_by(|a, b| a.name.cmp(&b.name));
        let mut out = format!("tool_search「{q}」命中 {} 个工具,以下是它们的用法(可直接调用):", hits.len());
        for d in &hits {
            out.push_str(&format!(
                "\n\n# {}\n{}\n参数(JSON Schema):{}",
                d.name,
                d.description,
                serde_json::to_string(&d.params).unwrap_or_default()
            ));
        }
        out
    }

    /// 给前端工具开关面板的本地化卡片(按 ui_lang)。`name` 恒英文 key(前端 toggle 用),
    /// `label`/`description` 本地化。
    pub fn ui_cards(&self, ui_lang: &str) -> Vec<ToolCard> {
        self.execs
            .values()
            .map(|e| {
                let name = e.name();
                ToolCard {
                    name: name.to_string(),
                    label: self.i18n.label(name, ui_lang),
                    description: self.i18n.ui_desc(name, ui_lang),
                }
            })
            .collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.execs.keys().cloned().collect()
    }

    /// 工具调用统计 `(总数, 失败数)`。面板 error_rate 用(P6 接真)。
    /// 计入"已尝试执行"的分发(Done/Terminal/AwaitingUser/Denied);Intent(UI 重定向)与 NeedAuth(待授权)不计。
    pub fn tool_stats(&self) -> (u64, u64) {
        (
            self.tool_calls.load(Ordering::Relaxed),
            self.tool_fails.load(Ordering::Relaxed),
        )
    }

    /// ★工具可见性闸(基础设施,见 设计/03 推论6 + 07 推论1)★:`name` 是否落在当前作用域的可用工具集内。
    /// `allowed = None`(无作用域 = 主智能体)→ 恒 true(全工具可用);
    /// `allowed = Some(集)`(工作流节点等受限作用域)→ name ∈ 集。
    /// 这是"只能用工具列表内的工具(防幻觉/防越权)"的**单一判据**——脊柱循环(派发前)与唯一执行闸门
    /// (`dispatch_inner`)共用,故每个带工具列表的作用域都按同一条规则收口。与懒加载等模式开关无关。
    pub fn tool_in_scope(name: &str, allowed: Option<&HashSet<String>>) -> bool {
        allowed.map(|a| a.contains(name)).unwrap_or(true)
    }

    /// 唯一分发路径:统计 + 转 `dispatch_inner`(真分发逻辑)。
    /// 分发(便捷版,不可中途取消)。测试/无前端路径用;生产脊柱用 `dispatch_with_cancel` 穿终止句柄。
    pub async fn dispatch(&self, call: &ToolCall, sandbox: &Sandbox, work_dir: &Path) -> Dispatch {
        self.dispatch_with_cancel(call, sandbox, work_dir, None).await
    }

    /// 分发(带回合级取消句柄)。`cancel` 穿进 `ExecCtx` → shell 等长命令在执行**中途**能响应「终止」,
    /// 不再只有 LLM 流式循环可取消(修"卡在工具执行里点不动结束")。
    pub async fn dispatch_with_cancel(
        &self,
        call: &ToolCall,
        sandbox: &Sandbox,
        work_dir: &Path,
        cancel: growbox_core::CancelFlag,
    ) -> Dispatch {
        self.dispatch_counted(call, sandbox, work_dir, cancel, None, false).await
    }

    /// 分发(带取消句柄 + 作用域可用工具集)。脊柱在**工作流节点内**用它派发真实工具:
    /// `allowed = Some(节点可用集)` → 唯一执行闸门按 `tool_in_scope` 校验(列表外即拒,防幻觉/越权,
    /// 见 设计/03 推论6 + 07 推论1);`allowed = None` 等价 `dispatch_with_cancel`(主智能体,全工具放行)。
    pub async fn dispatch_with_cancel_scoped(
        &self,
        call: &ToolCall,
        sandbox: &Sandbox,
        work_dir: &Path,
        cancel: growbox_core::CancelFlag,
        allowed: Option<&HashSet<String>>,
    ) -> Dispatch {
        self.dispatch_counted(call, sandbox, work_dir, cancel, allowed, false).await
    }

    /// 已授权重派发:用户经决定脊柱(`request_decision`)放行某 NeedAuth 动作后,带 `authorized=true`
    /// 重跑该工具 —— 把 NeedAuth 当 Allow 执行(**硬 Deny 仍拒**)。这样"授权后当场执行"无需改 sandbox
    /// 状态做一次性绕过(见 `agent::run_agent_loop` 的授权门 + `decision.rs`)。
    pub async fn dispatch_authorized(
        &self,
        call: &ToolCall,
        sandbox: &Sandbox,
        work_dir: &Path,
        cancel: growbox_core::CancelFlag,
    ) -> Dispatch {
        // 授权重派发:首次 scoped 派发已过工具可见性闸(否则首次即 Denied,到不了 NeedAuth)→ 此处传 None 免重校验。
        self.dispatch_counted(call, sandbox, work_dir, cancel, None, true).await
    }

    /// 统计包装:inner + 调用/失败计数。`allowed`(作用域可用集,见 `tool_in_scope`)与 `authorized`
    /// (授权重派发跳过 NeedAuth 门)一并透传给 inner。
    async fn dispatch_counted(
        &self,
        call: &ToolCall,
        sandbox: &Sandbox,
        work_dir: &Path,
        cancel: growbox_core::CancelFlag,
        allowed: Option<&HashSet<String>>,
        authorized: bool,
    ) -> Dispatch {
        let d = self.dispatch_inner(call, sandbox, work_dir, cancel, allowed, authorized).await;
        match &d {
            Dispatch::Done(r) | Dispatch::Terminal(r) | Dispatch::AwaitingUser(r) => {
                self.tool_calls.fetch_add(1, Ordering::Relaxed);
                if !r.ok {
                    self.tool_fails.fetch_add(1, Ordering::Relaxed);
                }
            }
            Dispatch::Denied { .. } => {
                self.tool_calls.fetch_add(1, Ordering::Relaxed);
                self.tool_fails.fetch_add(1, Ordering::Relaxed);
            }
            // Intent = UI 重定向、NeedAuth = 待授权:都不是"工具执行",不计入统计。
            Dispatch::Intent(_) | Dispatch::NeedAuth { .. } => {}
        }
        d
    }

    /// 真分发逻辑:解析参数 → 交互类弹 UI → 过安全门 → 执行。
    async fn dispatch_inner(
        &self,
        call: &ToolCall,
        sandbox: &Sandbox,
        work_dir: &Path,
        cancel: growbox_core::CancelFlag,
        allowed: Option<&HashSet<String>>,
        authorized: bool,
    ) -> Dispatch {
        // ★工具可见性闸(基础设施,见 设计/03 推论6 + 07 推论1)★:受限作用域(allowed=Some,如工作流节点)
        // 内只能调用其可用集里的工具——列表外的(哪怕已注册/本身安全/模型幻觉出来)在此唯一执行闸门即拒,
        // 与脊柱循环派发前的同一判据(`tool_in_scope`)互为表里;无作用域(None,主智能体)恒放行。
        // 硬拒(授权也不放行):这是"该作用域无此能力",非路径/风险类可授权事项。
        if !Self::tool_in_scope(&call.name, allowed) {
            return Dispatch::Denied {
                reason: format!(
                    "工具「{}」不在当前步骤的可用工具集内(防幻觉/越权:本步只能调用已暴露给你的工具)。",
                    call.name
                ),
            };
        }
        // ★D1 MCP★:内置 execs 没有的工具,若 hub 认领 → 造动态执行器,**走同一条** dispatch 路径
        // (架构公理:一切能力皆执行器)。`mcp_holder` 持有所有权,`exec` 借它;非 MCP 则为 None。
        let mcp_holder: Option<Box<dyn Executor>> = if self.execs.contains_key(&call.name) {
            None
        } else {
            self.mcp.executor_for(&call.name)
        };
        let exec: &dyn Executor = match self.execs.get(&call.name) {
            Some(e) => e.as_ref(),
            None => match &mcp_holder {
                Some(e) => e.as_ref(),
                None => return Dispatch::Done(ToolResult::fail(format!("未知工具: {}", call.name))),
            },
        };

        // 空参 = 多半被截断(实验记录/00);上层应已重试给足 token。到此仍解析失败 → 报错让模型重发。
        let args: serde_json::Value = if call.arguments.trim().is_empty() {
            serde_json::Value::Object(Default::default())
        } else {
            match serde_json::from_str(&call.arguments) {
                Ok(v) => v,
                Err(e) => return Dispatch::Done(ToolResult::fail(format!("参数 JSON 解析失败: {e}(原文: {})", call.arguments))),
            }
        };

        // 交互类:控制反转,不动手,弹预填 UI。
        if let Some(intent) = exec.ui_intent(&args) {
            return Dispatch::Intent(intent);
        }

        // 单一安全门:资源声明 → judge → 不可逆再确认。
        let claim = exec.claim(&args, work_dir);
        let path_verdict = match &claim {
            Some(Claim::Read(p)) => sandbox.judge(&Operation::Read(p)),
            Some(Claim::Write(p)) => sandbox.judge(&Operation::Write(p)),
            Some(Claim::Shell(c)) => sandbox.judge(&Operation::Shell(c)),
            Some(Claim::Net(u)) => sandbox.judge(&Operation::Net(u)),
            None => Verdict::Allow,
        };
        let verdict = risk_gate(exec.risk(), path_verdict);
        // 授权重派发:用户已就此 NeedAuth 放行 → 当 Allow 执行(硬 Deny 仍拒)。
        let verdict = match verdict {
            Verdict::NeedAuth { .. } if authorized => Verdict::Allow,
            other => other,
        };
        match verdict {
            Verdict::Allow => {
                let mut ctx = ExecCtx { args, work_dir, limits: self.limits, cancel };
                let result = exec.execute(&mut ctx).await;
                if exec.terminal() {
                    Dispatch::Terminal(result)
                } else if exec.awaits_user() {
                    Dispatch::AwaitingUser(result)
                } else {
                    Dispatch::Done(result)
                }
            }
            Verdict::NeedAuth { reason } => Dispatch::NeedAuth { reason, claim },
            Verdict::Deny { reason } => Dispatch::Denied { reason },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall { id: "c1".into(), name: name.into(), arguments: args.to_string() }
    }

    #[tokio::test]
    async fn dispatch_executes_allowed_read() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hi").unwrap();
        let reg = Registry::with_builtins(TaskManager::new());
        let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
        match reg.dispatch(&call("file_read", serde_json::json!({"path":"a.txt"})), &sb, dir.path()).await {
            Dispatch::Done(r) => {
                assert!(r.ok);
                assert_eq!(r.content, "hi");
            }
            _ => panic!("应直接执行"),
        }
    }

    #[tokio::test]
    async fn dispatch_write_outside_needs_auth() {
        let dir = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let reg = Registry::with_builtins(TaskManager::new());
        // 仅 dir 可写;往 outside 写 → 越界。
        let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
        let abs = outside.path().join("x.txt");
        let c = call("file_write", serde_json::json!({"path": abs.to_string_lossy(), "content":"y"}));
        match reg.dispatch(&c, &sb, dir.path()).await {
            Dispatch::NeedAuth { claim, .. } => {
                assert!(matches!(claim, Some(Claim::Write(_))));
            }
            _ => panic!("越界写应要授权"),
        }
    }

    #[tokio::test]
    async fn dispatch_dangerous_shell_denied() {
        let dir = tempdir().unwrap();
        let reg = Registry::with_builtins(TaskManager::new());
        let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
        let c = call("shell", serde_json::json!({"command":"sudo rm -rf /"}));
        assert!(matches!(reg.dispatch(&c, &sb, dir.path()).await, Dispatch::Denied { .. }));
    }

    /// 后台任务也走唯一安全门:危险命令的 spawn_task 同样被拒(不再有第二条绕过路径)。
    #[tokio::test]
    async fn dispatch_dangerous_spawn_task_denied() {
        let dir = tempdir().unwrap();
        let reg = Registry::with_builtins(TaskManager::new());
        let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
        let c = call("spawn_task", serde_json::json!({"command":"sudo rm -rf /","label":"x","done_when":"exit"}));
        assert!(matches!(reg.dispatch(&c, &sb, dir.path()).await, Dispatch::Denied { .. }));
    }

    #[tokio::test]
    async fn tool_stats_counts_calls_and_failures() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hi").unwrap();
        let reg = Registry::with_builtins(TaskManager::new());
        let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
        assert_eq!(reg.tool_stats(), (0, 0));
        // 成功读:total+1,fail 不变。
        let _ = reg.dispatch(&call("file_read", serde_json::json!({"path":"a.txt"})), &sb, dir.path()).await;
        assert_eq!(reg.tool_stats(), (1, 0));
        // 未知工具:Done(fail) → total+1 且 fail+1。
        let _ = reg.dispatch(&call("nope", serde_json::json!({})), &sb, dir.path()).await;
        assert_eq!(reg.tool_stats(), (2, 1));
        // 危险命令被拒:Denied → total+1 且 fail+1。
        let _ = reg.dispatch(&call("shell", serde_json::json!({"command":"sudo rm -rf /"})), &sb, dir.path()).await;
        assert_eq!(reg.tool_stats(), (3, 2));
        // 交互类(Intent)不计入统计。
        let _ = reg.dispatch(&call("create_project", serde_json::json!({"name":"x"})), &sb, dir.path()).await;
        assert_eq!(reg.tool_stats(), (3, 2), "Intent 不计入工具执行统计");
    }

    #[tokio::test]
    async fn dispatch_interactive_returns_intent() {
        let dir = tempdir().unwrap();
        let reg = Registry::with_builtins(TaskManager::new());
        let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
        let c = call("create_project", serde_json::json!({"name":"博客"}));
        match reg.dispatch(&c, &sb, dir.path()).await {
            Dispatch::Intent(i) => assert_eq!(i.action, "open_new_project"),
            _ => panic!("交互类应弹 UI"),
        }
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_fails_gracefully() {
        let dir = tempdir().unwrap();
        let reg = Registry::with_builtins(TaskManager::new());
        let sb = Sandbox::new(vec![], vec![]);
        match reg.dispatch(&call("nope", serde_json::json!({})), &sb, dir.path()).await {
            Dispatch::Done(r) => assert!(!r.ok),
            _ => panic!("未知工具应返回失败结果"),
        }
    }

    #[test]
    fn definitions_cover_builtins() {
        let reg = Registry::with_builtins(TaskManager::new());
        let names: Vec<String> = reg.definitions("zh").into_iter().map(|d| d.name).collect();
        // 文件/shell/交互类 + 后台任务三件套 + define_workflow + web 两件套,全在同一注册表、同一分发路径下。
        for expected in ["file_read", "file_write", "file_edit", "file_list", "shell", "create_project", "ui_control", "spawn_task", "wait_tasks", "list_tasks", "define_workflow", "web_fetch", "web_search"] {
            assert!(names.contains(&expected.to_string()), "缺执行器 {expected}");
        }
    }

    /// Web 工具走唯一安全门:内网/本机 URL → NeedAuth(带 Net 声明供授权后重放);非 http(s) → 硬拒。
    #[tokio::test]
    async fn dispatch_web_fetch_private_needs_auth_and_bad_scheme_denied() {
        let dir = tempdir().unwrap();
        let reg = Registry::with_builtins(TaskManager::new());
        let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
        let c = call("web_fetch", serde_json::json!({"url": "http://192.168.1.1/admin"}));
        match reg.dispatch(&c, &sb, dir.path()).await {
            Dispatch::NeedAuth { claim, .. } => assert!(matches!(claim, Some(Claim::Net(_)))),
            _ => panic!("内网 web_fetch 应交还用户授权"),
        }
        let c2 = call("web_fetch", serde_json::json!({"url": "file:///etc/passwd"}));
        assert!(matches!(reg.dispatch(&c2, &sb, dir.path()).await, Dispatch::Denied { .. }), "file:// 应硬拒");
    }

    /// 工作流机制(07 推论1/6):普通模式见全工具 + 工作流入口工具;节点内收窄到子集 + 兜底 finish/ask_user。
    #[test]
    fn tools_for_filters_to_node_subset() {
        let reg = Registry::with_builtins(TaskManager::new());
        // 普通模式:含真实工具,且含内置工作流的动态入口工具(工作流即动态工具)。
        let normal: Vec<String> = reg.tools_for("zh", None, &HashSet::new()).into_iter().map(|d| d.name).collect();
        assert!(normal.contains(&"shell".to_string()));
        assert!(normal.contains(&"create_artifact_workflow".to_string()), "工作流应作为动态工具出现");

        // 节点内:收窄到内置工作流 design 节点的工具子集(render_artifact/file_read/file_list)+ finish/ask_user。
        let scoped: Vec<String> =
            reg.tools_for("zh", Some(("create_artifact_workflow", "design")), &HashSet::new()).into_iter().map(|d| d.name).collect();
        assert!(scoped.contains(&"render_artifact".to_string()));
        assert!(scoped.contains(&"file_read".to_string()));
        assert!(scoped.contains(&"finish".to_string()), "finish 始终保留防卡死");
        assert!(scoped.contains(&"ask_user".to_string()), "ask_user 始终保留防卡死");
        // 物理锁死:节点外的工具选不到。
        assert!(!scoped.contains(&"shell".to_string()), "节点工具子集外的 shell 应被过滤掉");
        assert!(!scoped.contains(&"push_artifact_notice".to_string()));

        // 兜底:未知工作流/节点 → 回退普通模式(不至于无工具)。
        let fallback = reg.tools_for("zh", Some(("nope", "x")), &HashSet::new());
        assert!(fallback.iter().any(|d| d.name == "shell"));
    }

    /// ★C1 懒加载开★:tools 字段 = 核心常驻(无 deferred)+ 工作流入口,且**不随节点变**(缓存稳);
    /// deferred 工具只露名;tool_search 按名/关键词/select 搜到 schema;节点允许名单过滤(硬锁)。
    #[test]
    fn lazy_mode_stable_core_listing_and_search() {
        let mut reg = Registry::with_builtins(TaskManager::new());
        reg.set_lazy_tools(true, vec!["lsp".into(), "code_search".into(), "create_project".into(), "tool_search".into()]);
        assert!(reg.lazy_enabled());

        // tools 字段:常用工具常驻;deferred 不在;tool_search 被 NEVER_DEFER 强制常驻(即便写进名单)。
        let mut normal: Vec<String> = reg.tools_for("zh", None, &HashSet::new()).into_iter().map(|d| d.name).collect();
        normal.sort();
        assert!(normal.contains(&"file_write".to_string()) && normal.contains(&"shell".to_string()), "常用工具常驻");
        assert!(normal.contains(&"tool_search".to_string()), "tool_search 强制常驻(NEVER_DEFER)");
        assert!(!normal.contains(&"lsp".to_string()) && !normal.contains(&"code_search".to_string()), "deferred 不在 tools 字段");

        // ★缓存稳★:进入工作流节点,tools 字段恒定(不收窄)。
        let mut in_node: Vec<String> =
            reg.tools_for("zh", Some(("create_artifact_workflow", "design")), &HashSet::new()).into_iter().map(|d| d.name).collect();
        in_node.sort();
        assert_eq!(normal, in_node, "懒加载开:tools 字段不随进出节点变(前缀稳)");

        // deferred 露名:列出 deferred 名字。
        let listing = reg.deferred_listing(None).expect("有 deferred 露名");
        assert!(listing.contains("lsp") && listing.contains("code_search") && listing.contains("create_project"));
        assert!(!listing.contains("file_write"), "常驻工具不露名(它本就在 tools 字段)");

        // tool_search:按名 / select 搜到 deferred 工具 schema。
        let by_name = reg.search_tools("lsp", "zh", None);
        assert!(by_name.contains("lsp") && by_name.contains("hover"), "按名搜到 lsp schema: {by_name}");
        let by_select = reg.search_tools("select:code_search", "zh", None);
        assert!(by_select.contains("code_search") && by_select.contains("pattern"), "select 取到 code_search: {by_select}");

        // ★节点硬锁★:节点只允许 create_project → 搜 lsp 落空(节点外 deferred 搜不到)。
        let allow = reg.node_allowed_tools("create_artifact_workflow", "design");
        // design 节点不含 lsp;手造一个只含 create_project 的允许集验证过滤。
        let only_cp: std::collections::HashSet<String> = ["create_project".to_string()].into_iter().collect();
        let blocked = reg.search_tools("lsp", "zh", Some(&only_cp));
        assert!(!blocked.contains("hover"), "节点外的 deferred 工具搜不到(硬锁): {blocked}");
        let ok_cp = reg.search_tools("create_project", "zh", Some(&only_cp));
        assert!(ok_cp.contains("create_project"), "节点允许的 deferred 能搜到");
        // node_allowed_tools 含 NEVER_DEFER(tool_search/finish 等始终允许,否则锁死)。
        let allowed = allow.expect("design 节点存在");
        assert!(allowed.contains("tool_search") && allowed.contains("finish"), "节点允许集含 NEVER_DEFER");
    }

    /// ★C1 懒加载关(默认)★:`set_lazy_tools(false, ..)` → lazy_enabled 假;deferred_listing None;
    /// tools_for 完全是旧行为(此处只验开关与露名,旧行为由 tools_for_filters_to_node_subset 覆盖)。
    #[test]
    fn lazy_off_is_inert() {
        let mut reg = Registry::with_builtins(TaskManager::new());
        reg.set_lazy_tools(false, vec!["lsp".into()]);
        assert!(!reg.lazy_enabled());
        assert!(reg.deferred_listing(None).is_none(), "关时无露名");
        // 关时 tools 字段含全部工具(含 lsp),普通模式照旧。
        let normal: Vec<String> = reg.tools_for("zh", None, &HashSet::new()).into_iter().map(|d| d.name).collect();
        assert!(normal.contains(&"lsp".to_string()), "懒加载关:lsp 仍直接在 tools 字段");
    }

    /// ★二期 D1 MCP 接线(懒关)★:连一个 mock server 后,其工具作为动态工具暴露,并经**唯一 dispatch
    /// 路径**(架构公理)调通——MCP 工具与内置工具零特例同走安全门 + 执行。
    #[tokio::test]
    async fn dispatch_routes_mcp_tool_through_gate() {
        let dir = tempdir().unwrap();
        let reg = Registry::with_builtins(TaskManager::new());
        let names = reg.mcp_hub().connect_mock("demo").await.expect("连 mock MCP server");
        assert!(names.contains(&"demo_echo".to_string()));
        let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
        // 懒关:MCP 工具直接在 tools 字段(动态工具)。
        let exposed: Vec<String> = reg.tools_for("zh", None, &HashSet::new()).into_iter().map(|d| d.name).collect();
        assert!(exposed.contains(&"demo_echo".to_string()), "懒关:MCP 工具应作为动态工具暴露");
        // 经唯一 dispatch 调通(走 McpToolExecutor),且 D2 安全门标注外部不可信来源。
        match reg.dispatch(&call("demo_echo", serde_json::json!({"text": "hi"})), &sb, dir.path()).await {
            Dispatch::Done(r) => {
                assert!(r.ok && r.content.contains("echo: hi"), "MCP 调用应回显: {}", r.content);
                assert!(r.content.contains("不可信外部输入"), "MCP 结果应过 D2 安全门标注: {}", r.content);
            }
            _ => panic!("MCP 工具应经唯一 dispatch 执行"),
        }
        // 未知工具仍优雅失败(不 panic)。
        assert!(matches!(
            reg.dispatch(&call("demo_nope", serde_json::json!({})), &sb, dir.path()).await,
            Dispatch::Done(r) if !r.ok
        ));
    }

    /// ★二期 D1 × C1(懒开)★:MCP 工具恒 deferred —— 不进 tools 字段,只露名 + 可被 tool_search 搜到;
    /// 加载 schema 后仍经唯一 dispatch 调通。这正是懒加载为收编生态(成百上千工具)铺的路。
    #[tokio::test]
    async fn lazy_mode_defers_mcp_tools() {
        let dir = tempdir().unwrap();
        let mut reg = Registry::with_builtins(TaskManager::new());
        reg.mcp_hub().connect_mock("demo").await.expect("连 mock MCP server");
        reg.set_lazy_tools(true, vec![]); // 懒开,无内置 deferred 名单 —— MCP 仍恒 deferred
        let core: Vec<String> = reg.tools_for("zh", None, &HashSet::new()).into_iter().map(|d| d.name).collect();
        assert!(!core.contains(&"demo_echo".to_string()), "懒开:MCP 工具不进 tools 字段(恒 deferred)");
        let listing = reg.deferred_listing(None).expect("有 deferred 露名");
        assert!(listing.contains("demo_echo"), "MCP 工具应露名: {listing}");
        let found = reg.search_tools("echo", "zh", None);
        assert!(found.contains("demo_echo") && found.contains("回显"), "tool_search 应搜到 MCP 工具 schema: {found}");
        // 加载后仍经唯一 dispatch 调通。
        let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
        assert!(matches!(
            reg.dispatch(&call("demo_echo", serde_json::json!({"text": "x"})), &sb, dir.path()).await,
            Dispatch::Done(r) if r.ok
        ));
    }

    /// ★工具可见性闸(设计/03 推论6 + 07 推论1)★:受限作用域(allowed=Some)内,列表外的工具在唯一执行
    /// 闸门即拒(哪怕目标本在可写区 = 不是安全门拦,是可见性闸拦);列表内的正常执行;allowed=None(主智能体)
    /// 恒放行。这把"节点工具子集"从"不展示在菜单"升级成"调了也执行不了"——防幻觉/防绕过。
    #[tokio::test]
    async fn dispatch_scoped_rejects_out_of_scope_tool() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hi").unwrap();
        let reg = Registry::with_builtins(TaskManager::new());
        let sb = Sandbox::new(vec![dir.path().to_path_buf()], vec![]);
        // 作用域只允许 file_read(模拟"只读巡检"节点)。
        let allowed: HashSet<String> = ["file_read".to_string()].into_iter().collect();

        // 列表内:file_read 正常执行。
        let c_read = call("file_read", serde_json::json!({"path": "a.txt"}));
        match reg.dispatch_with_cancel_scoped(&c_read, &sb, dir.path(), None, Some(&allowed)).await {
            Dispatch::Done(r) => assert!(r.ok && r.content == "hi"),
            _ => panic!("列表内的 file_read 应正常执行"),
        }

        // 列表外:file_write 在唯一执行闸门被拒(目标本在可写区 → 排除安全门拦的混淆)。
        let c_write = call("file_write", serde_json::json!({"path": "a.txt", "content": "x"}));
        match reg.dispatch_with_cancel_scoped(&c_write, &sb, dir.path(), None, Some(&allowed)).await {
            Dispatch::Denied { reason } => assert!(reason.contains("可用工具集"), "拒因应点明可见性闸: {reason}"),
            _ => panic!("列表外的 file_write 应被可见性闸硬拒"),
        }
        // 真没执行:a.txt 仍是 hi。
        assert_eq!(std::fs::read_to_string(dir.path().join("a.txt")).unwrap(), "hi");

        // 无作用域(allowed=None = 主智能体):file_write 恢复放行(可见性闸不收窄)。
        match reg.dispatch_with_cancel_scoped(&c_write, &sb, dir.path(), None, None).await {
            Dispatch::Done(r) => assert!(r.ok, "主智能体无作用域,file_write 应放行"),
            _ => panic!("allowed=None 应放行全部工具"),
        }
    }

    /// `tool_in_scope` 单一判据:None 恒真(主智能体全工具);Some 仅列表内为真。
    #[test]
    fn tool_in_scope_judge() {
        let allowed: HashSet<String> = ["file_read".to_string(), "grep".to_string()].into_iter().collect();
        assert!(Registry::tool_in_scope("shell", None), "无作用域恒放行");
        assert!(Registry::tool_in_scope("file_read", Some(&allowed)));
        assert!(!Registry::tool_in_scope("shell", Some(&allowed)), "列表外应挡");
    }
}
