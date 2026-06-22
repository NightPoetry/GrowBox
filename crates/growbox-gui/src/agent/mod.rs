//! Agent 循环 —— 全局唯一脊柱。
//!
//! 实现 `系统架构/06-app.md` 数据流:
//!   ①组装上下文(memory.retrieve + 系统提示)②调 LLM(reasoning/截断重试/沉默超时)
//!   ③执行器分发(唯一 Registry.dispatch,过 safety)④感知/纠错(失败结果回填,模型自纠,≤MAX_TURNS)
//!   ⑤学习(每步 flywheel.collect 一条经验入 memory)。
//!
//! 事件经 `EventSink` 抛出(Tauri 层转成前端事件;测试用收集器),故本脊柱不依赖 Tauri,可独立单测。
//!
//! 本模块按职责拆分:`types`(对外契约:事件/sink/配置/结果)· `render`(纯渲染 helper)·
//! `shell_gate`(shell 批准门)· `drive`(单次 LLM 流式驱动)。`mod.rs` 自身留循环脊柱
//! (`run_agent_loop` 及两个入口 `agent_loop`/`agent_loop_internal`)。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use growbox_core::{END_NODE, ToolResult};
use growbox_learn::{Flywheel, Reasoner, Snapshot};
use growbox_llm::{ChatMessage, ChatRequest, Role};
use growbox_memory::{Memory, Region, Subconscious};
use growbox_safety::Sandbox;

use crate::bridge::LlmDriver;
use crate::registry::{Dispatch, Registry};

mod drive;
mod render;
mod shell_gate;
mod types;

#[cfg(test)]
mod loop_tests;
#[cfg(test)]
mod tests;

// 对外契约类型(外部按 `agent::AgentEvent` 等路径引用,不变)。
pub use types::{AgentConfig, AgentEvent, AgentOutcome, EventSink, StopReason};

// 脊柱内部用的 helper(从子模块取回 `run_agent_loop` 作用域)。
use drive::drive_one;
use render::{
    claim_kind, claim_target, degenerate_fingerprint, describe_ui_intent, node_guidance,
    parse_process_spec, render_executable_processes, render_process_recipes, render_recent_ring,
    render_working_region, self_verify_prompt, verify_status_label,
};
use shell_gate::{shell_command_of, shell_gate};

// Agent 循环行为旋钮(截断重试/token上限/沉默超时/空转上限)已暴露为可设(推论9),
// 由 `AgentConfig` 透传(默认在 core `Settings`)。背景:实验记录/00——token 被 reasoning 吃光
// 会把工具调用截成空参,故需截断重试 + 给足 token;沉默超时把 reasoning chunk 算"有活动"。

/// 运行一轮完整对话(脊柱)。种子 = 用户消息:**必须执行**(裸文本→催续→必须 finish 才停)。
#[allow(clippy::too_many_arguments)]
pub async fn agent_loop(
    user_msg: &str,
    cfg: &AgentConfig,
    llm: &dyn LlmDriver,
    registry: &Registry,
    sandbox: &Sandbox,
    memory: &mut Memory,
    subconscious: &dyn Subconscious,
    reasoner: &dyn Reasoner,
    flywheel: &Flywheel,
    work_dir: &Path,
    sink: &dyn EventSink,
) -> AgentOutcome {
    run_agent_loop(
        user_msg, false, None, cfg, llm, registry, sandbox, memory, subconscious, reasoner, flywheel, work_dir,
        sink, false, false,
    )
    .await
}

/// 内部消息种子(面板裁决回流、感知层事件等,**非用户说的话**)。与用户消息的本质区别:
/// ① 以 system 角色入上下文、ingest "internal"(不当用户学/显示);② **AI 有权不执行**——
/// 调 LLM 后可仅当信息(不输出/不调工具),无 finish 义务,无工具调用即优雅结束、不催续。
/// 见用户决策 2026-06-02(内部消息 vs 用户消息)+ [[internal-state-perception]]。
///
/// `initial_wf`:端口触发种入(P2)。Some((工作流名, 节点id)) = 本回合从该工作流节点起手
///(工具收窄 + 引导注入),用于造物交互回调按 trigger 进入工作流(见 07 推论3)。None = 普通起手。
#[allow(clippy::too_many_arguments)]
pub async fn agent_loop_internal(
    seed: &str,
    initial_wf: Option<(String, String)>,
    cfg: &AgentConfig,
    llm: &dyn LlmDriver,
    registry: &Registry,
    sandbox: &Sandbox,
    memory: &mut Memory,
    subconscious: &dyn Subconscious,
    reasoner: &dyn Reasoner,
    flywheel: &Flywheel,
    work_dir: &Path,
    sink: &dyn EventSink,
    // 见 run_agent_loop 的 transient:观察/共驾瞬态轮不写主记忆。普通内部消息传 false。
    transient: bool,
    // ★自驱续跑★:true = 自动鞭策种子。完整种子只作本轮系统消息(AI 看得到),**不把全文落时间线**
    // (避免几百轮重复种子膨胀 timeline / 污染 recent-ring);记录由 run_chat 的精简 SELF_DRIVE 标记代劳。
    self_drive: bool,
) -> AgentOutcome {
    run_agent_loop(
        seed, true, initial_wf, cfg, llm, registry, sandbox, memory, subconscious, reasoner, flywheel, work_dir, sink,
        transient, self_drive,
    )
    .await
}

/// ★栈函数工作流帧(v2,见 设计/07「加强版:栈函数工作流」)★。
/// v1 的帧只是 `(工作流名, 节点)` 指针;v2 把它升级成带**上下文作用域**的函数帧:
/// 调用方进入时决定喂全量(inherit)还是裁剪(isolated)上下文,退出时按"最少充分信息原则"
/// 丢弃分支噪音、只回灌返回值(或 full 直通原始内容)。
#[derive(Clone)]
struct WfFrame {
    /// 工作流名。
    wf: String,
    /// 当前节点 id。
    node: String,
    /// 上下文隔离:true = 本帧只看 [系统提示 + 节点引导 + 调用方 input + 本帧工作消息],
    /// 父对话被隐藏(裁剪上下文);退出即丢弃本帧工作消息(最少充分信息原则)。false = 继承父全量。
    isolated: bool,
    /// fork 模式(context_mode=fork):看得到父全量上下文(同 inherit,不抬 context_floor),但退出时
    /// **截断分支工作**(同 isolated,只回摘要)。= 解耦"隐藏父"与"退出截断"两开关后的第三组合(继承全境 +
    /// 只回摘要),给"需父全部背景的繁重机械活"。与 `isolated` 互斥。见 设计/07-附录-子Agent三旋钮。
    fork: bool,
    /// 进入本帧时 `messages` 的长度:isolated 切片的"地板" + 退出截断的锚点(丢弃本帧之后追加的消息)。
    msg_base: usize,
    /// 进入前的 `context_floor`,退出时恢复(支持嵌套隔离)。
    prev_floor: usize,
    /// 是否"派生分支"(栈调用进入=true / 直接调用·主链续=false)。★记忆门控★(见 07 v2 原则7):
    /// 只有主智能体(直接调用链,is_branch=false)因高感知/涉用户交互被**全量索引**进主记忆;
    /// 分支流(is_branch=true)的工作转录**不自动写主记忆**(噪音隔离、退出即丢),其贡献=返回值(主智能体读到即记)。
    is_branch: bool,
    /// 本帧(栈调用进入时由主 LLM 设)允许的**直接调用循环次数上限**(-1=无限,慎用,见 07 v2 原则8)。
    /// 分支内用直接调用循环处理时,每次直接调用计 1;达上限则**强制返回**入口栈(防失控)。
    max_loops: i64,
    /// 本帧已用的直接调用循环次数(尾调用替换帧时继承+1)。
    loops_used: i64,
}

/// 本轮进入(嵌套)工作流时,从入口工具调用参数解析出的"函数调用签名"(见 07 原则3)。
struct EnteredWf {
    wf: String,
    entry: String,
    /// 调用方喂给被调工作流的输入(LLM 当场撰写,可裁剪)。
    input: Option<String>,
    /// 调用方预留的返回契约:要带回什么、什么格式(workflow_return 据此产出)。
    return_spec: Option<String>,
    /// 上下文模式 isolated:true=隐藏父、只看 input(裁剪)/ false=继承父全量。
    isolated: bool,
    /// 上下文模式 fork:看得到父全量、但退出截断分支工作(只回摘要)。与 isolated 互斥(同一 context_mode 字符串)。
    fork: bool,
    /// 调用方式:false=栈调用(默认,压栈,被调返回到调用方);true=直接调用(尾调用,替换当前帧,
    /// 不压栈、栈深恒定、不返回到调用方——用于"顺流而下/循环"如主循环式工作流,避免栈溢出,见 07 v2 原则6)。
    direct: bool,
    /// 栈调用时主 LLM 给被调分支设的直接调用循环上限(-1=无限,慎用)。仅 direct=false 时有意义。
    max_loops: i64,
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_loop(
    user_msg: &str,
    seed_internal: bool,
    initial_wf: Option<(String, String)>,
    cfg: &AgentConfig,
    llm: &dyn LlmDriver,
    registry: &Registry,
    sandbox: &Sandbox,
    memory: &mut Memory,
    subconscious: &dyn Subconscious,
    reasoner: &dyn Reasoner,
    flywheel: &Flywheel,
    work_dir: &Path,
    sink: &dyn EventSink,
    // ★流式窗口记忆(transient)★:true = 本回合是"观察/共驾的瞬态轮"(如交互终端事件点唤醒),
    // seed/工具转录/收尾**不写主记忆/RAG/飞轮**,只进上下文+展示给用户(用户铁律:观察一般不入主记忆,
    // 除非值得关注的值)。失败仍 perceive(值得关注)。false = 正常持久回合。
    transient: bool,
    // ★自驱续跑★:true = 自动鞭策种子。完整种子作本轮系统消息让 AI 看到,但**不把全文 ingest 进时间线**
    // (P3:防几百轮重复种子膨胀 + recent-ring 被鞭策噪音挤占);记录交 run_chat 的精简 SELF_DRIVE 标记。
    self_drive: bool,
) -> AgentOutcome {
    // ① 组装上下文(P4 记忆置换系统,稳定→易变,命中 prompt 缓存):
    //    system prompt(最稳) → 工作记忆区(指针调入,两态) → 8K 最近 ring(最末) → 当前回合。
    //    置换策略在 memory(llm 无关);此处只套"每区独特标记 + 时间戳 + 按时间戳判序"的外壳。
    // ★C1 懒加载★:开关开时,把"未直接加载的 deferred 工具名单"(只露名)拼进系统提示(messages[0]=
    //  稳定前缀,缓存稳、隔离安全)。模型要用某个露名工具先 tool_search 拉回其 schema。关时返回 None,逐字不变。
    let mut system_prompt = match registry.deferred_listing(None) {
        Some(listing) => format!("{}\n\n{}", cfg.system_prompt, listing),
        None => cfg.system_prompt.clone(),
    };
    // ★Skill 常驻清单(设计/09 推论4)★:内置种子 + 已学 skill 的"名称+触发描述"拼进系统提示
    // (稳定前缀,缓存安全;同 deferred_listing)。AI 遇匹配场景调 load_skill 拉正文。无 skill 则不拼。
    if let Some(skill_listing) = crate::skills::listing(memory) {
        system_prompt = format!("{system_prompt}\n\n{skill_listing}");
    }
    let mut messages = vec![ChatMessage::system(system_prompt)];
    let blocks = memory.assemble_context(user_msg, subconscious).await;
    let (working, ring): (Vec<_>, Vec<_>) =
        blocks.into_iter().partition(|b| b.region == Region::Working);
    if let Some(msg) = render_working_region(&working, &cfg.prompt_lang) {
        messages.push(ChatMessage::system(msg));
    }
    if let Some(msg) = render_recent_ring(&ring, &cfg.prompt_lang) {
        messages.push(ChatMessage::system(msg));
    }
    // ★二期 C2 物化集★:本 run 被召回的"可执行流程"引用的工作流名。懒加载开时 `tools_for` 据此把这些
    // 工作流暴露进 tools 字段(供栈调用),其余工作流不占位 → 工作流库可无界生长而前缀稳(见 registry C2)。
    // 内部种子(端口触发/感知事件)不做任务级流程召回 → 保持空。
    let mut materialized: HashSet<String> = HashSet::new();
    // 用户消息以 user 入场;内部消息(面板裁决/感知事件)以 system 入场 + ingest "internal",
    // 不当用户说的话学/显示(内部消息 vs 用户消息,见用户决策 2026-06-02)。
    if seed_internal {
        messages.push(ChatMessage::system(user_msg));
        // transient(终端事件点等观察轮):seed 只进上下文、不落主记忆/时间线。
        // self_drive(自驱鞭策):完整种子不落时间线(P3 防膨胀),记录由 run_chat 的精简 SELF_DRIVE 标记代劳。
        if !transient && !self_drive {
            memory.ingest_with_role(user_msg, "internal");
        }
    } else {
        // ★项目级流程(二期 B1 建议档 + C2 可执行档)★:任务开始拉与当前任务相关的流程配方,
        // 注入上下文(放用户消息前)。流程 = 项目涟漪面约定(改一个东西要碰哪几处),通用解析找不到,
        // 见 `设计文档/二期项目/设计原理/01-流程即一等公民.md`。
        //   - 建议档(无 `wf:`)→ "照做"块,AI 读着手工做全。
        //   - 可执行档(`wf:` 引用的工作流仍存在)→ 物化该工作流(本 run 暴露供栈调用)+ "直接运行"块(C2)。
        let hits = memory.retrieve_processes(user_msg, subconscious).await;
        let mut advisory: Vec<String> = Vec::new();
        let mut executable: Vec<(String, String)> = Vec::new();
        for h in hits {
            let (text, wf) = parse_process_spec(&h.content);
            match wf {
                // 可执行档:wf: 引用的工作流存在 → 物化 + 注入"直接运行"块。
                Some(w) if registry.workflow(&w).is_some() => {
                    materialized.insert(w.clone());
                    executable.push((text, w));
                }
                // 无标记 / wf: 引用的工作流已不存在(被删/未建)→ 当建议档照做。
                _ => advisory.push(text),
            }
        }
        if let Some(msg) = render_process_recipes(&advisory, &cfg.prompt_lang) {
            messages.push(ChatMessage::system(msg));
        }
        if let Some(msg) = render_executable_processes(&executable, &cfg.prompt_lang) {
            messages.push(ChatMessage::system(msg));
        }
        // ★Skill 语义召回(设计/09 推论4「召回兜底」)★:按当前场景召回相关 skill(名+触发)注入——
        // 海量库长尾的发现主路。常驻清单(尤其折叠成分类索引后)装不下全部,靠这个把"此刻该用的"浮到眼前;
        // AI 再 load_skill 取正文。停用/总开关关的已在 retrieve_skills 内过滤。
        let skill_autoload = memory.skill_config().autoload_threshold;
        let skill_hits = memory.retrieve_skills(user_msg, subconscious).await;
        if let Some(msg) = crate::skills::render_recalled(&skill_hits, skill_autoload) {
            messages.push(ChatMessage::system(msg));
        }
        messages.push(ChatMessage::user(user_msg));
        memory.ingest_with_role(user_msg, "user");
    }

    // 工具清单**每轮**按当前工作流节点计算(见下文 turn_tools):普通模式 = 全集 + 工作流动态入口工具;
    // 工作流节点内 = 收窄到节点工具子集(物理锁死选择空间,见 设计/07 推论1/6)。
    let mut final_text = String::new();

    // ★工作流当前态(栈)★:空 = 普通模式(全工具);栈顶 = 当前所在工作流节点
    //(工具收窄 + 节点引导注入 + 强制顺序流转)。用**栈**支持嵌套(P3 推论4):某节点工具子集里含另一个
    //  工作流工具 → 调它即压栈进入嵌套工作流;嵌套 END 退出即出栈、回到父工作流节点(父据"调过该工作流"续流转)。
    // wf_stack 是 run-local;**跨 run 续跑靠端口触发**(P2):每次造物交互回调按 trigger 把 `initial_wf`
    // 种到对应节点,于是 run-local 的状态每回合都被端口"恢复"到正确节点(见 cmds::artifact_event)。
    let mut wf_stack: Vec<WfFrame> = Vec::new();
    // ★上下文地板(栈函数 v2 隔离)★:每轮发给 LLM 的请求 = messages[0..SYSTEM_PREFIX_LEN] ++ messages[context_floor..]。
    // isolated 帧把 floor 抬到自己的 msg_base → 父对话被隐藏(裁剪上下文,最少充分信息原则);退出复位。
    // 无 isolated 帧时 floor = SYSTEM_PREFIX_LEN(只 messages[0] 系统提示恒可见,其余=父全量上下文继承)。
    const SYSTEM_PREFIX_LEN: usize = 1;
    let mut context_floor = SYSTEM_PREFIX_LEN;
    // ★分支日志(v2 原则9)★:派生分支的调用信息原样存项目级日志(主记忆只摘要、展示不给用户,细节进日志不丢)。
    let branch_log = crate::branch_log::BranchLog::new(work_dir, cfg.branch_log_max_gb);
    if let Some((wf_name, node_id)) = initial_wf {
        // 端口触发种入:起手即注入初始节点引导(此后工具按该节点收窄)。节点失效则回退普通模式(空栈)。
        // 端口触发 = inherit(看得到造物交互上下文),非隔离。
        if let Some(node) = registry.workflow(&wf_name).and_then(|wf| wf.node(&node_id).cloned()) {
            let msg_base = messages.len();
            messages.push(ChatMessage::system(node_guidance(&wf_name, &node, &cfg.prompt_lang)));
            // 端口触发 = 主链续(直接调用语义),inherit、非分支。
            wf_stack.push(WfFrame {
                wf: wf_name,
                node: node_id,
                isolated: false,
                fork: false,
                msg_base,
                prev_floor: context_floor,
                is_branch: false,
                max_loops: -1,
                loops_used: 0,
            });
        }
    }

    // 0 = 无限模式;否则到 max_turns 收口。每轮 = 一次 LLM 调用 + 工具结果回填。
    let mut turn = 0usize;
    // ★思考免死★:不再因"没调工具"收口。只有连续 N 轮产出"近乎全等"=真高频重复死循环才收口。
    // 任何新产出(含 reasoning)都清零。工具调用也清零(在动手)。(用户原则 2026-06-03)
    let mut last_fingerprint: Option<String> = None;
    let mut same_count = 0usize;
    // ★append-only 内部状态注入(2026-06-04,修 byte-stable prefix 缓存)★:游标起点 = 进入时的发号器值,
    // 跳过已有事件(它们已在 assemble_context 检索 / seed 里)。turn loop 内新产生的内部/造物事件
    // **一次性追加进 messages 历史、永不重渲**,保持前缀字节稳定 → 命中 deepseek KV 缓存。
    // (旧做法"每轮把整块夹在请求末尾"破坏了 model-output 缓存单元,实测 hit 640→128,且致沉默超时 Bug B。)
    let mut internal_cursor = memory.internal_seq();
    let mut artifact_cursor = memory.artifact_seq();
    // ★失控硬防护(2026-06-04,防 CPU 345% 再现)★:本回合 render_artifact 累计次数上限。
    // AI 蒙眼困惑时会反复重画整个造物 → iframe 反复 reload → webview 狂转失控。超限即拦截、回灌引导
    // (用 artifact_command 增量更新,别重画)。正常创建/修改远低于此;纯硬兜底,不靠 AI 自觉。
    let mut render_count = 0usize;
    // 重画上限:超此次数则停下交还用户(不自动续重画)。用户 2026-06-04 真机定为 3(此前 8 仅拦截续轮)。
    const MAX_RENDER_PER_RUN: usize = 3;
    // ★主动自检(grounded verification)★:本次任务累计工具调用数(达阈值才值得自检);
    // self_verified=本次任务是否已自检过(至多一轮,防反复自我怀疑/死循环)。
    let mut total_tool_calls = 0usize;
    let mut self_verified = false;
    // ★不犯第二遍·本回合守卫(计划/工具记忆-不犯第二遍 C)★:本次任务里失败过的 (工具|参数) 指纹。
    // AI 原样重发同指纹 → 执行前注入"不犯第二遍"提醒(软,不硬阻——尊重 build/test 类工具的关键因素
    // 是外部状态、可能已被其它工具改变)。零 LLM/嵌入开销。
    let mut failed_call_sigs: std::collections::HashSet<String> = std::collections::HashSet::new();
    while cfg.max_turns == 0 || turn < cfg.max_turns as usize {
        // ★回合级取消检查点(造物交互 v2 §2)★:用户按「终止」→ 本轮优雅收口(不再调 LLM/动手)。
        // 任务未完成,不 finalize(那是完成的学习);已采集的逐步经验保留,只抛 Done 让前端停转。
        if sink.is_cancelled() {
            crate::notify::perceive_notice(memory, &cfg.prompt_lang, "chat.cancelled", &serde_json::json!({}));
            // 此前轮已展示过的回复也落库(凡展示过即落库),别让它在重载后消失。
            persist_visible_reply(memory, &final_text, transient);
            sink.emit(AgentEvent::Notice("已终止本回合".into())).await;
            sink.emit(AgentEvent::Done).await;
            return AgentOutcome { final_text, turns: turn, stopped: StopReason::Cancelled };
        }
        // append-only 注入:把自上次以来的新内部/造物事件追加进 messages(永久,不再每轮夹末尾)。
        if let Some((block, nc)) = memory.render_internal_since(&cfg.prompt_lang, internal_cursor) {
            messages.push(ChatMessage::system(block));
            internal_cursor = nc;
        }
        if let Some((block, nc)) = memory.render_artifact_since(&cfg.prompt_lang, artifact_cursor) {
            messages.push(ChatMessage::system(block));
            artifact_cursor = nc;
        }
        // ★工作流机制★:本轮暴露给 LLM 的工具集 = 按当前节点收窄(普通模式 = 全集 + 工作流入口工具)。
        // 节点内 prompt 已在进入/流转时作为 system 追加进 messages(append-only,缓存稳),此处只算工具集。
        let turn_tools = registry.tools_for(
            &cfg.prompt_lang,
            wf_stack.last().map(|f| (f.wf.as_str(), f.node.as_str())),
            &materialized,
        );
        // ★分支门控(栈函数 v2 原则7+9)★:在派生分支(栈调用帧)内 → ① 思考/正文不向用户展示(只主链对话)
        // ② 逐步工具转录不自动写主记忆。wf_stack 本轮内不变,故在调 LLM 前算一次,贯穿展示与记忆门控。
        let in_branch = wf_stack.last().is_some_and(|f| f.is_branch);
        // ② 调 LLM,带截断重试(token 不足把工具调用截成空参 → 加预算重试)。
        // 0 = 不限制,模型自己停。非零 = 有限额,截断时翻倍重试。
        let has_limit = cfg.max_tokens > 0;
        let mut budget = if has_limit { cfg.max_tokens } else { 0 };
        let mut retries = 0;
        // ★栈函数 v2 上下文切片★:isolated 帧把 context_floor 抬到其 msg_base → 本轮只发
        // [系统提示] ++ [本帧起的工作消息],父对话被隐藏(裁剪上下文)。inherit/普通模式 floor=1 → 发全量。
        // 仍是 byte-stable 前缀拼接(messages[0..1] 与 messages[floor..] 各自字节稳),隔离段自成稳定前缀,缓存按段命中。
        let request_messages: Vec<ChatMessage> = if context_floor > SYSTEM_PREFIX_LEN && context_floor <= messages.len() {
            let mut m = messages[..SYSTEM_PREFIX_LEN].to_vec();
            m.extend_from_slice(&messages[context_floor..]);
            m
        } else {
            messages.clone()
        };
        let outcome = loop {
            // messages 已是 byte-stable 前缀(内部状态已 append 进历史,不再夹带)。
            let mut req = ChatRequest::new(cfg.model.clone(), request_messages.clone())
                .with_tools(turn_tools.clone())
                .with_reasoning_effort(cfg.reasoning_effort.clone());
            if has_limit {
                req = req.with_max_tokens(budget);
            }
            match drive_one(llm, req, sink, cfg.silence_secs, !in_branch).await {
                Ok(o) => {
                    if o.truncated && has_limit && retries < cfg.max_token_retries && budget < cfg.token_ceil {
                        budget = (budget * 2).min(cfg.token_ceil);
                        retries += 1;
                        sink.emit(AgentEvent::Notice(format!("LLM 响应被截断,提高到 {budget} 重试")))
                            .await;
                        continue;
                    }
                    break o;
                }
                Err(e) => {
                    // 失败 AI 必须能感知:按 code 渲染 llm[prompt_lang] 落内部状态环 + 时间线(感知告知双受众)。
                    crate::notify::perceive_notice(
                        memory,
                        &cfg.prompt_lang,
                        "llm.call_failed",
                        &serde_json::json!({ "detail": e.as_str() }),
                    );
                    // 出错前若已展示过回复(往轮 final_text),也落库,别让它在重载后消失。
                    persist_visible_reply(memory, &final_text, transient);
                    sink.emit(AgentEvent::Done).await;
                    return AgentOutcome { final_text, turns: turn, stopped: StopReason::Error(e) };
                }
            }
        };

        // ★终止响应★:drive_one 流式途中被「终止」打断(或本轮 LLM 调用刚回来即发现已取消)→ 立刻优雅收口,
        // 不再处理工具/续轮。配合 drive.rs 的流中断检查,长思考期间点终止也秒级生效。
        if sink.is_cancelled() {
            crate::notify::perceive_notice(memory, &cfg.prompt_lang, "chat.cancelled", &serde_json::json!({}));
            // 已流式展示过的本轮正文也落库(凡展示过即落库);取消不 finalize(那是"完成"的学习)。
            if !outcome.content.is_empty() {
                final_text = outcome.content.clone();
            }
            persist_visible_reply(memory, &final_text, transient);
            sink.emit(AgentEvent::Notice("已终止本回合".into())).await;
            sink.emit(AgentEvent::Done).await;
            return AgentOutcome { final_text, turns: turn, stopped: StopReason::Cancelled };
        }

        // ★分支日志★:在派生分支内,本轮思考/正文不展示给用户(已门控),但原样存分支日志(细节不丢)。
        if in_branch {
            if let Some(f) = wf_stack.last() {
                if !outcome.reasoning.is_empty() {
                    branch_log.append(&f.wf, &f.node, "reasoning", &outcome.reasoning);
                }
                if !outcome.content.is_empty() {
                    branch_log.append(&f.wf, &f.node, "content", &outcome.content);
                }
            }
        }

        if !outcome.content.is_empty() {
            final_text = outcome.content.clone();
        }

        // ③ 无工具调用:模型只输出了思考/文字,没动手。★思考免死★(用户原则 2026-06-03):
        //    思考阶段物理上调不了工具,"没调工具"不能当卡住。只要在产出新内容就让它继续想/干;
        //    唯一收口条件 = 连续 max_stall 轮产出"近乎全等"(真高频重复死循环)。max_turns 仍是硬底线。
        if outcome.tool_calls.is_empty() {
            // 内部消息:AI 有权"仅当作信息"——调了 LLM 但选择不动手即合法终态,优雅结束不催续。
            if seed_internal && turn == 0 {
                // transient:观察轮的收尾不写主记忆、不跑飞轮 finalize(流式窗口记忆)。
                if !transient {
                    if !final_text.is_empty() {
                        memory.ingest_with_role(&final_text, "assistant");
                    }
                    finalize(flywheel, memory, reasoner, sink).await;
                }
                return AgentOutcome { final_text, turns: turn + 1, stopped: StopReason::Completed };
            }
            // 退化指纹 = reasoning + content 规范化(折叠空白,容忍排版差异 = "近乎全等")。
            let fp = degenerate_fingerprint(&outcome.reasoning, &outcome.content);
            if last_fingerprint.as_deref() == Some(fp.as_str()) {
                same_count += 1;
            } else {
                same_count = 1;
                last_fingerprint = Some(fp);
            }
            if same_count >= cfg.max_stall.max(2) {
                // 真·高频重复:同样的话连说 max_stall 轮 = 退化死循环。诚实告知 AI + 收口。
                crate::notify::perceive_notice(memory, &cfg.prompt_lang, "loop.degenerate", &serde_json::json!({}));
                sink.emit(AgentEvent::Notice("检测到回答高频重复,已收口".into())).await;
                if !transient {
                    memory.ingest_with_role(&final_text, "assistant");
                    finalize(flywheel, memory, reasoner, sink).await;
                }
                return AgentOutcome { final_text, turns: turn + 1, stopped: StopReason::Completed };
            }
            // 在产出新东西 = 在思考/推进:免死,接一句中性引导让它接着走(脚手架只进本轮上下文,不进 memory)。
            messages.push(ChatMessage::assistant(&outcome.content));
            messages.push(ChatMessage::user(
                "如果还在推进就继续(可调用工具动手);需要用户拍板就调用 ask_user;\
                 确认全部做完了再调用 finish。",
            ));
            turn += 1;
            continue;
        }

        // 有工具调用 = 在动手,清零退化重复跟踪。
        same_count = 0;
        last_fingerprint = None;
        // ★自检阈值★:累计本次任务的工具调用数(作"干了多少事"的代理;达阈值才触发收尾自检)。
        total_tool_calls += outcome.tool_calls.len();

        // 记下 assistant 的工具调用消息(供模型看到自己调了什么)。
        // ★回传 reasoning_content★(2026-06-04 定论):reasoning 是本条 assistant 生成的一部分,
        // 回传才能让此消息字节匹配 turn0 的 model-output 缓存单元 → 命中 deepseek KV 缓存(byte-stable prefix);
        // 不回传则 assistant 段前缀分叉、同样 miss。deepseek 文档亦要求"有 tool_call 须回传 + same prefix rule"。
        // 配套 append-only 内部状态注入,整条 messages 保持 byte-stable(Bug B 真因是 IS/AS 夹末尾破坏缓存,非回传本身)。
        messages.push(ChatMessage {
            role: Role::Assistant,
            content: outcome.content.clone(),
            tool_calls: outcome.tool_calls.clone(),
            tool_call_id: None,
            reasoning_content: if outcome.reasoning.is_empty() { None } else { Some(outcome.reasoning.clone()) },
        });

        // ③/④ 逐个分发,过唯一安全门(registry.dispatch 是唯一入口,含后台任务);结果回填,失败让模型自纠。
        // finish(终止类)是控制信号:命中即收口,不采集为经验、不回填(反正要退出)。
        let mut finished: Option<String> = None;
        let mut yielded: Option<String> = None;
        // ★工作流流转跟踪(本轮)★:entered_wf = 本轮调到了某工作流入口工具(进入它,带函数调用签名);
        // called_tools = 本轮实际派发过的真实工具名(供当前节点判定强制流转,见 07 原则1);
        // pending_return = 本轮调了 workflow_return(value, full),收尾时出栈一层并回灌返回值(栈函数 v2)。
        let mut entered_wf: Option<EnteredWf> = None;
        let mut pending_return: Option<(String, bool)> = None;
        let mut called_tools: Vec<String> = Vec::new();
        // ★A2 诊断推感知层★:本轮成功编辑过的 .rs 文件(非造物文件夹),循环末尾据此拉诊断 perceive。
        let mut edited_rs: Vec<PathBuf> = Vec::new();
        // ★工具可见性闸(设计/03 推论6 + 07 推论1)★:当前作用域(工作流节点)的可用工具集 —— 本轮 wf_stack
        // 不变,算一次贯穿整批工具。None = 不在任何节点(主智能体)= 全工具可用。脊柱侧(下方第一关)与唯一
        // 执行闸门(dispatch_with_cancel_scoped)共用同一判据 `tool_in_scope`。
        let node_allowed = wf_stack.last().and_then(|f| registry.node_allowed_tools(&f.wf, &f.node));
        for call in &outcome.tool_calls {
            // ★终止穿透到工具批次★:本轮可能有多个 tool_call,用户中途按「终止」→ 不再继续派发剩余工具,
            // 立刻收口(配合 shell 执行器内部的取消自杀:正在跑的那条命令也已被杀)。任务未完成,不 finalize。
            if sink.is_cancelled() {
                crate::notify::perceive_notice(memory, &cfg.prompt_lang, "chat.cancelled", &serde_json::json!({}));
                sink.emit(AgentEvent::Notice("已终止本回合".into())).await;
                sink.emit(AgentEvent::Done).await;
                return AgentOutcome { final_text, turns: turn, stopped: StopReason::Cancelled };
            }
            sink.emit(AgentEvent::ToolStart { name: call.name.clone(), args: call.arguments.clone() }).await;
            // ★自检动效★:已进入自检阶段(self_verified)时,每个工具调用驱动一次"正在核查:xxx"动态状态。
            if self_verified {
                sink.emit(AgentEvent::Status(verify_status_label(&call.name, &call.arguments))).await;
            }

            // ★工具可见性闸·脊柱侧(基础设施,设计/03 推论6 + 07 推论1)★:在工作流节点(受限作用域)内,
            // 本步只能调用节点可用集里的工具——**与懒加载无关、无条件生效**(旧版仅 lazy 开时把关,这里放开)。
            // 调了集外的 → 拒绝 + 引导(不执行、不进 dispatch)。这是覆盖**所有路由**(控制信号 + 真实工具)的
            // 第一关;唯一执行闸门 dispatch 按同一判据(`tool_in_scope`)再兜一层(防代码绕过脊柱循环)。
            // 主智能体(node_allowed=None)时不触发,全放行。
            if let Some(allowed) = node_allowed.as_ref() {
                if !Registry::tool_in_scope(&call.name, Some(allowed)) {
                    let mut names: Vec<&str> = allowed.iter().map(|s| s.as_str()).collect();
                    names.sort_unstable();
                    let wf_name = wf_stack.last().map(|f| f.wf.as_str()).unwrap_or("");
                    let msg = format!(
                        "当前工作流「{}」这一步不允许调用「{}」。本步可用:{}。如需其它能力,请按本步引导推进,或 finish / ask_user。",
                        wf_name, call.name, names.join(", ")
                    );
                    crate::notify::perceive_notice(
                        memory,
                        &cfg.prompt_lang,
                        "tool.failed",
                        &serde_json::json!({ "tool": call.name, "detail": msg }),
                    );
                    sink.emit(AgentEvent::ToolEnd { name: call.name.clone(), ok: false, content: msg.clone() }).await;
                    memory.ingest_with_role(format!("{} -> {}", call.name, msg), "tool");
                    messages.push(ChatMessage::tool_result(call.id.clone(), msg));
                    continue;
                }
            }

            // ★C1 tool_search★:懒加载枢纽(控制信号,不走 dispatch——需注册表 + 当前节点允许名单)。
            // 按 query 在 deferred 工具里检索,返回命中工具的完整 schema(append 进上下文,前缀不破),之后可直接调。
            // 节点内按允许名单过滤(节点外的 deferred 搜不到 = 硬锁)。
            if call.name == crate::executors::TOOL_SEARCH {
                let pv: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                let query = pv.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
                let allow = wf_stack.last().and_then(|f| registry.node_allowed_tools(&f.wf, &f.node));
                let result = if query.is_empty() {
                    "tool_search 需要 query(工具名 / 关键词 / select:名1,名2)".to_string()
                } else {
                    registry.search_tools(query, &cfg.prompt_lang, allow.as_ref())
                };
                sink.emit(AgentEvent::ToolEnd { name: call.name.clone(), ok: true, content: result.clone() }).await;
                memory.ingest_with_role(format!("tool_search({query}) -> 已返回工具 schema"), "tool");
                messages.push(ChatMessage::tool_result(call.id.clone(), result));
                called_tools.push(call.name.clone());
                continue;
            }

            // ★工作流即动态函数★:调到一个已注册工作流名 = 进入该工作流(控制信号,不走 dispatch)。
            // 复用唯一工具调用路径,零新机制(07 原则2/3)。从调用参数解析"函数调用签名"
            // (input/return_spec/context_mode/direct),收尾时据此压栈(栈调用)或替换帧(直接调用)。
            if let Some(wf) = registry.workflow(&call.name) {
                let entry = wf.entry.clone();
                let sig: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                let pick = |k: &str| sig.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
                let cmode = sig.get("context_mode").and_then(|v| v.as_str()).unwrap_or("");
                let isolated = cmode.eq_ignore_ascii_case("isolated");
                let fork = cmode.eq_ignore_ascii_case("fork"); // fork=看得到父(同 inherit)但退出截断(同 isolated);与 isolated 互斥
                let direct = sig.get("direct").and_then(|v| v.as_bool()).unwrap_or(false);
                let max_loops = sig.get("max_loops").and_then(|v| v.as_i64()).unwrap_or(-1);
                let ack = if wf.node(&entry).is_some() {
                    let mode = if direct { "直接调用(尾调用,主链续)" } else { "栈调用(完成后 workflow_return 返回上层)" };
                    format!("已进入工作流「{}」· {mode}(见下方该步引导)", wf.name)
                } else {
                    format!("已进入工作流「{}」,但入口节点「{}」不存在", wf.name, entry)
                };
                entered_wf = Some(EnteredWf {
                    wf: wf.name.clone(),
                    entry,
                    input: pick("input"),
                    return_spec: pick("return_spec"),
                    isolated,
                    fork,
                    direct,
                    max_loops,
                });
                sink.emit(AgentEvent::ToolEnd { name: call.name.clone(), ok: true, content: ack.clone() }).await;
                memory.ingest_with_role(format!("{} -> {}", call.name, ack), "tool");
                messages.push(ChatMessage::tool_result(call.id.clone(), ack));
                continue;
            }

            // ★栈函数返回 workflow_return(value, full)★:被调工作流的结构化返回(控制信号,不走 dispatch——
            // 需脊柱的 wf_stack 才能出栈+回灌)。收尾时出栈一层:full=false(默认)截断分支噪音、只回灌 value
            // (最少充分信息);full=true 不截断、原始工作上下文直通回父(零 LLM 搬运,慎用,污染主上下文)。
            if call.name == crate::executors::WORKFLOW_RETURN {
                let rv: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                let value = rv.get("value").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                let full = rv.get("full").and_then(|v| v.as_bool()).unwrap_or(false);
                let ack = if wf_stack.is_empty() {
                    "当前不在工作流中,workflow_return 已忽略".to_string()
                } else if full {
                    "已请求全量返回上层(原始内容直通)".to_string()
                } else {
                    "已返回上层(摘要)".to_string()
                };
                sink.emit(AgentEvent::ToolEnd { name: call.name.clone(), ok: true, content: ack.clone() }).await;
                memory.ingest_with_role(format!("workflow_return -> {ack}"), "tool");
                messages.push(ChatMessage::tool_result(call.id.clone(), ack));
                if !wf_stack.is_empty() {
                    pending_return = Some((value, full));
                }
                continue;
            }

            // ★二期 B3 结晶(报告-纠正回路写入半)★:learn_process 把一条复发的项目流程结晶进主记忆
            // (控制信号,不走 dispatch——需脊柱的 &mut Memory + subconscious 嵌入)。近重复取代旧版。
            // 门控:派生分支内不写主记忆(同记忆门控)→ 跳过结晶,只回执说明。
            if call.name == crate::executors::LEARN_PROCESS {
                let pv: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                let name = pv.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
                let recipe = pv.get("recipe").and_then(|v| v.as_str()).unwrap_or("").trim();
                let ack = if name.is_empty() || recipe.is_empty() {
                    "learn_process 需要 name + recipe(流程名 + 碰哪几处/什么顺序)".to_string()
                } else if in_branch {
                    "分支内不结晶流程(主记忆门控);如需沉淀请在对话主链中调用".to_string()
                } else {
                    let content = format!("【{name}】{recipe}");
                    let (_id, superseded) = memory.crystallize_process(content, subconscious).await;
                    match superseded {
                        Some(_) => format!("已更新项目流程「{name}」(取代了近重复的旧版,下次同类任务自动带上)"),
                        None => format!("已结晶项目流程「{name}」(下次同类任务自动召回照做)"),
                    }
                };
                sink.emit(AgentEvent::ToolEnd { name: call.name.clone(), ok: true, content: ack.clone() }).await;
                memory.ingest_with_role(format!("learn_process -> {ack}"), "tool");
                messages.push(ChatMessage::tool_result(call.id.clone(), ack));
                called_tools.push(call.name.clone());
                continue;
            }

            // ★Skill load_skill★:取一个 skill 的 playbook 正文(控制信号,不走 dispatch——需 Memory
            // 已学节点 + 内置种子目录)。已学优先、内置兜底;命中 = 正文 append 回上下文(前缀不破),
            // 未命中 = 列出可用名。这是渐进披露 Skill 的加载枢纽(设计/09 推论5)。
            if call.name == crate::executors::LOAD_SKILL {
                let pv: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                let name = pv.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
                let result = if name.is_empty() {
                    let names = crate::skills::available_names(memory).join(", ");
                    format!("load_skill 需要 name。可用 skill:{names}")
                } else if let Some(body) = crate::skills::load_body(memory, name) {
                    body
                } else {
                    let names = crate::skills::available_names(memory).join(", ");
                    format!("没有名为「{name}」的 skill。可用 skill:{names}")
                };
                sink.emit(AgentEvent::ToolEnd { name: call.name.clone(), ok: true, content: result.clone() }).await;
                memory.ingest_with_role(format!("load_skill({name}) -> 已加载 playbook"), "tool");
                messages.push(ChatMessage::tool_result(call.id.clone(), result));
                called_tools.push(call.name.clone());
                continue;
            }

            // ★Skill learn_skill★:把一个 skill(name/trigger/body)结晶进主记忆(控制信号,不走
            // dispatch——需 &mut Memory + subconscious 嵌入)。近重复/同名取代旧版。分支内不写主记忆(门控)。
            if call.name == crate::executors::LEARN_SKILL {
                let pv: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                let name = pv.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
                let trigger = pv.get("trigger").and_then(|v| v.as_str()).unwrap_or("").trim();
                let body = pv.get("body").and_then(|v| v.as_str()).unwrap_or("").trim();
                let ack = if name.is_empty() || trigger.is_empty() || body.is_empty() {
                    "learn_skill 需要 name + trigger(一句话:何时用)+ body(playbook 正文)".to_string()
                } else if in_branch {
                    "分支内不结晶 skill(主记忆门控);如需沉淀请在对话主链中调用".to_string()
                } else {
                    let (_id, superseded) = memory.crystallize_skill(name, trigger, body, subconscious).await;
                    match superseded {
                        Some(_) => format!("已更新 skill「{name}」(取代了同名/近重复的旧版,进常驻清单可被主动挑)"),
                        None => format!("已结晶 skill「{name}」(进常驻清单,下次匹配场景可 load_skill 取用)"),
                    }
                };
                sink.emit(AgentEvent::ToolEnd { name: call.name.clone(), ok: true, content: ack.clone() }).await;
                memory.ingest_with_role(format!("learn_skill -> {ack}"), "tool");
                messages.push(ChatMessage::tool_result(call.id.clone(), ack));
                called_tools.push(call.name.clone());
                continue;
            }

            // ★工具记忆 note_tool_memory★(计划/工具记忆-不犯第二遍 A):把一条工具经验结晶进主记忆
            // (控制信号,不走 dispatch——需 &mut Memory + subconscious 嵌入)。之后分发前会诊用它。
            if call.name == crate::executors::NOTE_TOOL_MEMORY {
                let pv: serde_json::Value = serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);
                let tool = pv.get("tool").and_then(|v| v.as_str()).unwrap_or("").trim();
                let situation = pv.get("situation").and_then(|v| v.as_str()).unwrap_or("").trim();
                let verdict = growbox_memory::tool_memory_format::Verdict::parse(
                    pv.get("verdict").and_then(|v| v.as_str()).unwrap_or(""),
                );
                let detail = pv.get("detail").and_then(|v| v.as_str()).unwrap_or("").trim();
                let ack = if tool.is_empty() || situation.is_empty() {
                    "note_tool_memory 需要 tool + situation(关键因素);verdict=infeasible/fails/works".to_string()
                } else if in_branch {
                    "分支内不写工具记忆(主记忆门控);如需记录请在对话主链中调用".to_string()
                } else {
                    memory.crystallize_tool_memory(tool, situation, verdict, detail, subconscious).await;
                    match verdict {
                        growbox_memory::tool_memory_format::Verdict::Infeasible => format!(
                            "已记:工具「{tool}」在此情况「{situation}」不可行。以后高度相似的调用会被拦下(不犯第二遍);关键因素若变了,再 note 一条新情况即可解除。"
                        ),
                        growbox_memory::tool_memory_format::Verdict::Works => {
                            format!("已记:工具「{tool}」在「{situation}」可行(覆盖此前同情况的旧结论)。")
                        }
                        growbox_memory::tool_memory_format::Verdict::Fails => format!(
                            "已记:工具「{tool}」在「{situation}」失败。以后相似调用前会提醒你(除非关键因素变了别原样重试)。"
                        ),
                    }
                };
                sink.emit(AgentEvent::ToolEnd { name: call.name.clone(), ok: true, content: ack.clone() }).await;
                memory.ingest_with_role(format!("note_tool_memory -> {ack}"), "tool");
                messages.push(ChatMessage::tool_result(call.id.clone(), ack));
                called_tools.push(call.name.clone());
                continue;
            }

            // shell 批准门。硬安全底线(危险命令 Deny / 敏感密钥 NeedAuth)归 dispatch 处理、不在此问;
            // 只对"过了硬底线的普通命令"按模式裁决:手动=逐条交用户;自动=LLM 安全审核员
            // 二次审 + 个人文件夹隐私网。返回 Some = 不执行、把结果回灌 LLM;None = 放行去 dispatch。
            if call.name == "shell" {
                if let Some(cmd) = shell_command_of(&call.arguments) {
                    if let Some(blocked) = shell_gate(&cmd, cfg, llm, sandbox, work_dir, sink).await {
                        crate::notify::perceive_notice(
                            memory,
                            &cfg.prompt_lang,
                            "tool.failed",
                            &serde_json::json!({ "tool": call.name, "detail": blocked.content }),
                        );
                        sink.emit(AgentEvent::ToolEnd { name: call.name.clone(), ok: false, content: blocked.content.clone() }).await;
                        let snap = Snapshot::new(format!("{}({})", call.name, call.arguments), blocked.content.clone(), false);
                        memory.ingest_conclusion(flywheel.collect(snap));
                        memory.ingest_with_role(format!("{} -> {}", call.name, blocked.content), "tool");
                        messages.push(ChatMessage::tool_result(call.id.clone(), blocked.content));
                        continue;
                    }
                }
            }

            // ★交互式终端(人机共驾 shell)★:interactive=true → 开 PTY 会话交前端 xterm + 用户/AI 共驾,
            // 不走 output() 捕获(那会在等输入时挂死)。会话长生命周期、独立本回合;开完即回执 session id,
            // 后续经事件点唤醒(terminal_event)+ pty_send/pty_peek/pty_close 共驾(见 pty.rs)。命令已过上面的
            // shell_gate 审核。控制信号,不走 dispatch(需脊柱的 sink 开会话)。
            if call.name == "shell" {
                let interactive = serde_json::from_str::<serde_json::Value>(&call.arguments)
                    .ok()
                    .and_then(|v| v.get("interactive").and_then(|b| b.as_bool()))
                    .unwrap_or(false);
                if interactive {
                    if let Some(cmd) = shell_command_of(&call.arguments) {
                        let result = match sink.open_terminal(&cmd, work_dir).await {
                            Some(id) => ToolResult::ok(format!(
                                "已开启交互终端会话「{id}」运行 `{cmd}`。用户可在终端直接输入;关键节点\
                                 (输出静默 / 匹配 password: / Permission denied / 提示符)会通知你 —— \
                                 届时 pty_peek 看屏、pty_send 接管、或提示用户;完成后 pty_close。"
                            )),
                            None => ToolResult::fail("无法开启交互终端(无前端或启动失败)。可改用非交互 shell。"),
                        };
                        if !result.ok {
                            crate::notify::perceive_notice(
                                memory,
                                &cfg.prompt_lang,
                                "tool.failed",
                                &serde_json::json!({ "tool": call.name, "detail": result.content }),
                            );
                        }
                        sink.emit(AgentEvent::ToolEnd { name: call.name.clone(), ok: result.ok, content: result.content.clone() }).await;
                        if in_branch {
                            if let Some(f) = wf_stack.last() {
                                branch_log.append(&f.wf, &f.node, "tool", &format!("shell(interactive) `{cmd}` -> {}", result.content));
                            }
                        } else {
                            let snap = Snapshot::new(format!("{}({})", call.name, call.arguments), result.content.clone(), result.ok);
                            memory.ingest_conclusion(flywheel.collect(snap));
                            memory.ingest_with_role(format!("{} -> {}", call.name, result.content), "tool");
                        }
                        called_tools.push(call.name.clone());
                        messages.push(ChatMessage::tool_result(call.id.clone(), result.content));
                        continue;
                    }
                }
            }

            // ★重画上限 → 停下告知用户(用户 2026-06-04 真机:收不到交互就别反复重绘)★:
            // 本回合 render_artifact 超 MAX_RENDER_PER_RUN 次 → **停下并告知用户,不再自动重画续轮**。
            // 反复重画通常是"AI 收不到用户操作 → 蒙眼 → 只能靠重画"的应激(真机 CPU 345% 失控真因);
            // 停下来交还用户比继续瞎画好——用户让重试时(下一条消息)再继续尝试。
            if call.name == "render_artifact" {
                render_count += 1;
                if render_count > MAX_RENDER_PER_RUN {
                    let msg = format!(
                        "我已重画造物 {render_count} 次(上限 {MAX_RENDER_PER_RUN})。看起来可能没收到你的操作(交互也许没接通),\
                         我先停在这里——继续靠重画帮不上忙。需要我继续/重试,直接告诉我就行。"
                    );
                    sink.emit(AgentEvent::ToolEnd {
                        name: call.name.clone(),
                        ok: false,
                        content: "已达本回合重画上限,停下交还用户".into(),
                    })
                    .await;
                    sink.emit(AgentEvent::Content(msg.clone())).await; // 用户可见:停下并说明
                    memory.ingest_with_role(&msg, "assistant");
                    finalize(flywheel, memory, reasoner, sink).await;
                    return AgentOutcome { final_text: msg, turns: turn + 1, stopped: StopReason::Completed };
                }
            }

            // ★工具记忆 + 不犯第二遍(计划/工具记忆-不犯第二遍 B+C)★:分发前查「小本本」。
            // 仅主链 + 总开关开 + 本项目确有该工具记忆(成本门,绝大多数情况 0 → 跳过、零开销)。
            // 先算本回合失败指纹(C);再做持久会诊(B);两者产出 pre_note(执行后前置进结果)或硬否决。
            let call_sig = format!("{}|{}", call.name, call.arguments);
            let mut pre_note: Option<String> = None;
            // C:本回合原样重发同一失败过的调用 → 软提醒(不阻断)。
            if cfg.tool_memory_enabled && !in_branch && failed_call_sigs.contains(&call_sig) {
                pre_note = Some(format!(
                    "[不犯第二遍] 本回合你已用相同参数调用过 `{}` 且失败。除非影响结果的关键因素变了(环境/前置已被其它操作改变),别原样重试——换做法,或说明哪个关键因素变了。反复同样失败 = 对'关键因素'判断错了,该换个关键因素看。",
                    call.name
                ));
            }
            // B:持久工具记忆会诊(已知不可行硬否决 / 已知失败软提醒)。
            let mut vetoed: Option<String> = None;
            if cfg.tool_memory_enabled && !in_branch && memory.tool_memory_count() > 0 {
                if let Some((verdict, lesson, score)) =
                    memory.consult_tool_memory(&call.name, &call_sig, subconscious).await
                {
                    use growbox_memory::tool_memory_format::Verdict;
                    match verdict {
                        Verdict::Infeasible if score >= cfg.tool_memory_veto_threshold => {
                            vetoed = Some(format!(
                                "[工具记忆·一票否决] 你曾在此项目记下:此操作在类似情况下不可行(相似度 {score:.2})。\n{lesson}\n\
                                 不犯第二遍 —— 除非影响结果的关键因素变了才重试;若确实变了,先 note_tool_memory 记一条新情况(覆盖旧结论)再调用。否则请换条路。",
                            ));
                        }
                        Verdict::Fails if score >= cfg.tool_memory_warn_threshold => {
                            pre_note = Some(format!(
                                "[工具记忆·提醒] 你曾记下此工具在类似情况失败(相似度 {score:.2}):{lesson}\n不犯第二遍 —— 除非关键因素变了别原样重试。",
                            ));
                        }
                        _ => {}
                    }
                }
            }
            // 硬否决:不执行,把教训当工具结果回灌(同 shell_gate 拦截路径)。
            if let Some(lesson) = vetoed {
                crate::notify::perceive_notice(
                    memory,
                    &cfg.prompt_lang,
                    "tool.failed",
                    &serde_json::json!({ "tool": call.name, "detail": "工具记忆一票否决(已知不可行)" }),
                );
                sink.emit(AgentEvent::ToolEnd { name: call.name.clone(), ok: false, content: lesson.clone() }).await;
                memory.ingest_with_role(format!("{}(被工具记忆否决)-> {lesson}", call.name), "tool");
                messages.push(ChatMessage::tool_result(call.id.clone(), lesson));
                called_tools.push(call.name.clone());
                continue;
            }

            // 穿取消句柄:让 shell 等长命令在执行中途响应「终止」(不只 LLM 流式循环可取消)。
            // ★工具可见性闸·闸门侧★:带本作用域可用集(node_allowed)→ 唯一执行闸门按 tool_in_scope 再校验一层
            // (与脊柱侧同判据,防绕过;主智能体 node_allowed=None 时等价旧 dispatch_with_cancel,全放行)。
            let mut dispatch = registry
                .dispatch_with_cancel_scoped(call, sandbox, work_dir, sink.cancel_flag(), node_allowed.as_ref())
                .await;

            // ★授权门(决定脊柱)★:NeedAuth(越界路径 / shell 引用敏感路径 / 不可逆确认)→ 经唯一
            // round-trip **阻塞等用户裁决**(不再 fire-and-forget 立刻失败)。放行 → 带 authorized 旁路
            // 重派发该工具(把 NeedAuth 当 Allow 执行,硬 Deny 仍拒),结果回到下方同一处理流;拒绝 → 置失败。
            // 授权的持久化(记住本文件夹 / 信任本项目 shell)由前端经既有 addPath/grantShellAccess 在回合后落地。
            if let Dispatch::NeedAuth { reason, claim } = &dispatch {
                let path = claim_target(claim);
                let privacy = crate::privacy::path_under_user_privacy(Path::new(&path), &cfg.privacy_dirs).is_some();
                let kind = crate::decision::DecisionKind::PathPermission {
                    path,
                    reason: reason.clone(),
                    access: claim_kind(claim).into(),
                    privacy,
                };
                dispatch = if sink.request_decision(kind).await.allows() {
                    registry.dispatch_authorized(call, sandbox, work_dir, sink.cancel_flag()).await
                } else {
                    Dispatch::Denied { reason: "用户未授权(拒绝/超时)".into() }
                };
            }

            // 终止类(finish):命中即收口,不回填、不采经验。
            if let Dispatch::Terminal(r) = dispatch {
                let summary = if r.content.trim().is_empty() { final_text.clone() } else { r.content.clone() };
                sink.emit(AgentEvent::ToolEnd { name: call.name.clone(), ok: r.ok, content: r.content }).await;
                finished = Some(summary);
                break;
            }

            // 提问类(ask_user):agent 显式向用户提问 → 把问题作为助手消息显示,本轮干净暂停等用户回答。
            // 调 ask_user 是工具调用,天然绕开"只说话=空转"催续分支:不催续、不重复、不外泄催续提示。
            // 用户在聊天里正常回答即驱动下一轮(前端无需特殊处理,问题走 Content 显示)。
            if let Dispatch::AwaitingUser(r) = dispatch {
                let question = if r.content.trim().is_empty() { "(需要你的回复)".to_string() } else { r.content.clone() };
                sink.emit(AgentEvent::Content(question.clone())).await;
                sink.emit(AgentEvent::ToolEnd {
                    name: call.name.clone(),
                    ok: true,
                    content: "已向用户提问,本轮在此暂停等待回答".into(),
                })
                .await;
                memory.ingest_with_role(&question, "assistant");
                final_text = question;
                yielded = Some("ask_user".into());
                break;
            }

            let result = match dispatch {
                Dispatch::Done(r) => r,
                Dispatch::Terminal(_) => unreachable!("Terminal 已在上面处理"),
                Dispatch::AwaitingUser(_) => unreachable!("AwaitingUser 已在上面处理"),
                Dispatch::Intent(intent) => {
                    if intent.await_ack {
                        // 家族二(Agent 自己对 UI 动手):往返等前端回执,返回**验证过的**状态,不撒谎。
                        // emit-with-id 在 ui_round_trip 内做,故这里不再额外 emit Intent(免重复)。
                        let ack = sink.ui_round_trip(&intent).await;
                        if ack.applied {
                            ToolResult::ok(format!(
                                "UI 已更新({}):{}",
                                describe_ui_intent(&intent),
                                ack.state
                            ))
                        } else {
                            ToolResult::fail(format!(
                                "UI 操作未生效({}):{}",
                                describe_ui_intent(&intent),
                                ack.note.unwrap_or_else(|| "前端未确认".into())
                            ))
                        }
                    } else if intent.gates {
                        // 家族一·阻塞裁决(如 create_project):弹板 → 让位暂停。
                        // 后续工作依赖用户处理结果,绝不假成功后继续/被空转催续推着冲。
                        // 标记 yield + 跳出循环,把控制权交还用户;裁决(确认/取消)由前端注入新一轮驱动。
                        let action = intent.action.clone();
                        sink.emit(AgentEvent::Intent(intent)).await;
                        sink.emit(AgentEvent::ToolEnd {
                            name: call.name.clone(),
                            ok: true,
                            content: format!("已打开「{action}」面板,等待用户处理(本轮在此暂停)"),
                        })
                        .await;
                        yielded = Some(action);
                        break;
                    } else {
                        // 家族一(纯呈现,不阻塞):弹预填表单,发出即返回(诚实的"已呈现")。
                        let action = intent.action.clone();
                        sink.emit(AgentEvent::Intent(intent)).await;
                        ToolResult::ok(format!("已请求前端打开「{action}」面板,等待用户操作"))
                    }
                }
                // NeedAuth 已在上面的「授权门」转化为放行重派发或 Denied,不会到这里。
                Dispatch::NeedAuth { .. } => unreachable!("NeedAuth 已在授权门处理"),
                Dispatch::Denied { reason } => ToolResult::fail(format!("操作被安全策略拒绝: {reason}")),
            };

            // ★工具记忆·软提醒前置(B-fails / C)★:把会诊/本回合守卫的提醒前置进结果(不改 ok)。
            let result = match pre_note {
                Some(note) => ToolResult { content: format!("{note}\n\n{}", result.content), ..result },
                None => result,
            };
            // ★不犯第二遍·记本回合失败指纹(C)★:失败 → 记下指纹,下次原样重发即提醒。
            if !result.ok {
                failed_call_sigs.insert(call_sig.clone());
            }

            // 工具非成功(执行失败/被拒/需授权)= AI 必须能感知的失败 → 内部状态环 + 时间线。
            if !result.ok {
                crate::notify::perceive_notice(
                    memory,
                    &cfg.prompt_lang,
                    "tool.failed",
                    &serde_json::json!({ "tool": call.name, "detail": result.content }),
                );
            }
            sink.emit(AgentEvent::ToolEnd { name: call.name.clone(), ok: result.ok, content: result.content.clone() })
                .await;

            // ⑤ 学习:每步采集一条客观经验入 memory(执行器吐经验,脊柱必调,见 系统架构/05)。
            //   ★造物文件夹隔离★:造物自己文件夹(.growbox/)的读写是造物的"流记忆/持久状态",不采集成经验、
            //   不进时间线/RAG,免污染主记忆(见 artifact_fs + 计划/造物交互-v2 §6)。本轮仍把结果回填 messages,
            //   AI 当下看得到(只是不进持久记忆)。
            // 记忆门控:派生分支内 → 逐步工具转录**不写主记忆**,改原样存分支日志(细节不丢);
            // 主链 → 照常采集进主记忆(造物文件夹隔离除外)。
            if in_branch {
                if let Some(f) = wf_stack.last() {
                    branch_log.append(&f.wf, &f.node, "tool", &format!("{}({}) -> {}", call.name, call.arguments, result.content));
                }
            } else if !transient && !is_internal_state_file_op(&call.name, &call.arguments, work_dir) {
                // transient(终端共驾观察轮):工具转录(pty_peek/pty_send 等)不落主记忆/飞轮,只进上下文+展示。
                let snap = Snapshot::new(format!("{}({})", call.name, call.arguments), result.content.clone(), result.ok);
                memory.ingest_conclusion(flywheel.collect(snap));
                memory.ingest_with_role(format!("{} -> {}", call.name, result.content), "tool");
            }

            // 工作流强制流转用:记下本轮派发过的真实工具(在工作流节点内据此判去哪个节点)。
            called_tools.push(call.name.clone());
            // ★A2★ 成功编辑过的 .rs(非造物文件夹)→ 收集,循环末尾拉诊断推感知层(result.ok 是 Copy,
            // 须在 result.content 被 move 进 messages 前读)。
            if let Some(p) = edited_rust_path(&call.name, &call.arguments, result.ok, work_dir) {
                edited_rs.push(p);
            }
            messages.push(ChatMessage::tool_result(call.id.clone(), result.content));
        }

        // ★A2 诊断推感知层★:本轮编辑过 .rs 且未收口/让位 → 若该工作区 rust-analyzer 已在跑
        // (AI 先前用过 lsp、已索引),重新同步文件 + 取诊断 → perceive(编辑期失败自我感知,见
        // 03-LSP集成 M2)。门控:① 派生分支内不写主记忆(in_branch)② 仅普通续轮(finish/让位轮不拉,
        // 省那次轮询延迟,反正要退出)。下回合经 render_internal_since 追加进上下文,AI 当回合可修。
        if finished.is_none() && yielded.is_none() && !in_branch && !edited_rs.is_empty() {
            edited_rs.sort();
            edited_rs.dedup();
            perceive_rust_diagnostics(registry.lsp_manager(), memory, &cfg.prompt_lang, work_dir, &edited_rs).await;
        }

        // ★栈函数工作流流转(07 v2)★:本轮结束据"进入/返回/调用了什么"更新工作流栈。
        // cascade_from = None → 刚进入新(子)工作流,不级联(下一轮跑新节点);
        // cascade_from = Some(pending_called) → 跑强制流转级联(含 END/达上限出栈、多层连环返回)。
        let cascade_from: Option<Vec<String>> = if let Some(ent) = entered_wf {
            // ★直接调用循环上限(v2 原则8)★:分支内用直接调用循环、超主 LLM 设的 max_loops → 强制返回入口栈(防失控)。
            let loop_blocked = ent.direct
                && wf_stack.last().map(|f| f.max_loops >= 0 && f.loops_used + 1 > f.max_loops).unwrap_or(false);
            if loop_blocked {
                let popped = wf_stack.pop().expect("loop_blocked 蕴含栈非空");
                if popped.isolated || popped.fork {
                    messages.truncate(popped.msg_base);
                }
                context_floor = popped.prev_floor;
                messages.push(ChatMessage::system(format!(
                    "[工作流「{}」已达最大循环次数 {},自动返回上层]",
                    popped.wf, popped.max_loops
                )));
                sink.emit(AgentEvent::Notice(format!("工作流「{}」达最大循环次数,已返回", popped.wf))).await;
                Some(vec![popped.wf]) // 让调用方据"调过该子工作流"续流转。
            } else {
                // ★进入(子)工作流★:开场 = 节点引导 + 调用方 input + 返回契约(一条 system,缓存稳)。
                // isolated 帧据此切片裁剪上下文;返回契约点醒"最少充分信息"默认。
                let opening = {
                    let node = registry.workflow(&ent.wf).and_then(|wf| wf.node(&ent.entry).cloned());
                    let mut s = match &node {
                        Some(n) => node_guidance(&ent.wf, n, &cfg.prompt_lang),
                        None => format!("工作流「{}」入口节点「{}」不存在", ent.wf, ent.entry),
                    };
                    if let Some(inp) = &ent.input {
                        s.push_str(&format!("\n\n[调用方输入]\n{inp}"));
                    }
                    if let Some(rs) = &ent.return_spec {
                        s.push_str(&format!(
                            "\n\n[调用方要求你返回(完成后调 workflow_return,value 按此格式;\
                             默认只回最少充分信息=错误/警告/结论,别把全量原始内容搬运回去)]\n{rs}"
                        ));
                    }
                    s
                };
                // 直接调用(尾调用):先优雅出栈当前帧再压新帧 = 替换栈顶,栈深不增(防主循环式工作流栈溢出);
                // 沿用被替换帧的链身份 + 循环计数+1 + 入口栈设的 max_loops。栈调用 = 新派生分支(从零计)。
                let mut inherited_branch = false;
                let mut next_loops = 0i64;
                let mut next_max = ent.max_loops;
                if ent.direct {
                    if let Some(popped) = wf_stack.pop() {
                        if popped.isolated || popped.fork {
                            messages.truncate(popped.msg_base);
                        }
                        context_floor = popped.prev_floor;
                        inherited_branch = popped.is_branch;
                        next_loops = popped.loops_used + 1;
                        next_max = popped.max_loops;
                    }
                }
                let msg_base = messages.len();
                messages.push(ChatMessage::system(opening));
                let prev_floor = context_floor;
                if ent.isolated {
                    context_floor = msg_base; // 隔离:抬高地板,父对话此后被隐藏(裁剪上下文)。
                }
                let is_branch = if ent.direct { inherited_branch } else { true };
                wf_stack.push(WfFrame {
                    wf: ent.wf,
                    node: ent.entry,
                    isolated: ent.isolated,
                    fork: ent.fork,
                    msg_base,
                    prev_floor,
                    is_branch,
                    max_loops: next_max,
                    loops_used: next_loops,
                });
                None // 刚进入,不级联(下一轮跑新节点)。
            }
        } else {
            // 起手 pending_called = 本轮真实工具;workflow_return 则先强制出栈当前帧(回灌返回值)再以"调过该工作流"喂父。
            let mut pending_called: Vec<String> = called_tools.clone();
            // ★workflow_return:被调工作流显式返回 → 出栈一层 + 回灌返回值(栈函数 v2 原则4)★。
            if let Some((value, full)) = pending_return {
                if let Some(frame) = wf_stack.pop() {
                    // 默认(full=false):isolated 截断分支工作消息(最少充分信息,退出即丢)。
                    // full=true:不截断,分支原始工作上下文直通回父(零 LLM 搬运;慎用,污染主上下文)。
                    if (frame.isolated || frame.fork) && !full {
                        messages.truncate(frame.msg_base);
                    }
                    context_floor = frame.prev_floor;
                    if !value.is_empty() {
                        messages.push(ChatMessage::system(format!("[工作流「{}」返回]\n{value}", frame.wf)));
                    } else if full {
                        messages.push(ChatMessage::system(format!("[工作流「{}」全量返回:见上方该分支原始输出]", frame.wf)));
                    }
                    sink.emit(AgentEvent::Notice(format!("工作流「{}」已返回", frame.wf))).await;
                    pending_called = vec![frame.wf];
                }
            }
            Some(pending_called)
        };

        // ★强制流转级联★:据 pending_called 判流转;END/容错/达上限出栈统一处理 isolated 截断 + 恢复 context_floor;
        // 出栈后以"出栈工作流名"喂父续判(级联,多层 END 连环返回)。刚进入新工作流(None)则跳过。
        if let Some(mut pending_called) = cascade_from {
            while let Some(frame) = wf_stack.last().cloned() {
                let Some(wf) = registry.workflow(&frame.wf) else { break };
                let Some(target) = wf.node(&frame.node).and_then(|n| n.transition_for(&pending_called)).map(str::to_string) else {
                    break; // 无匹配流转 → 留在原节点(工具仍按本节点收窄,引导已在历史)。
                };
                if target == END_NODE {
                    let popped = wf_stack.pop().expect("last 已确认存在");
                    if popped.isolated || popped.fork {
                        messages.truncate(popped.msg_base); // END 无显式返回值:isolated/fork 退出即丢分支噪音。
                    }
                    context_floor = popped.prev_floor;
                    sink.emit(AgentEvent::Notice(format!("工作流「{}」已完成", popped.wf))).await;
                    pending_called = vec![popped.wf]; // 返回父级:父节点据"调过该子工作流"再判(级联)。
                    continue;
                } else if let Some(next_node) = wf.node(&target) {
                    // 流转到新节点:把新节点引导作为 system 追加进历史(append-only,缓存稳),工具下轮收窄。
                    messages.push(ChatMessage::system(node_guidance(&frame.wf, next_node, &cfg.prompt_lang)));
                    if let Some(top) = wf_stack.last_mut() {
                        top.node = target;
                    }
                    break;
                } else {
                    let popped = wf_stack.pop().expect("last 已确认存在");
                    if popped.isolated || popped.fork {
                        messages.truncate(popped.msg_base);
                    }
                    context_floor = popped.prev_floor;
                    sink.emit(AgentEvent::Notice(format!("工作流「{}」目标节点「{target}」不存在,已退出", popped.wf))).await;
                    pending_called = vec![popped.wf];
                    continue;
                }
            }
        }

        if let Some(summary) = finished {
            // ★主动自检(grounded verification,用户 2026-06-08)★:任务做了实事(工具调用数 ≥ 阈值)、
            // 还没自检过、且在主链(非派生分支)→ 收尾前注入一次"拿你的工作汇报重读相关文件逐条核对、
            // 改正证据不支持的说法、标注无法验证的"指令,让 AI 重读真实状态后再 finish。grounded(强制重读)
            // 而非空想自省;至多一轮(self_verified 防反复自我怀疑)。关掉自检 / 轻任务(未达阈值)直接收口、不花这个钱。
            if cfg.self_verify && !self_verified && !in_branch && total_tool_calls >= cfg.self_verify_min_tools {
                self_verified = true;
                messages.push(ChatMessage::system(self_verify_prompt(&summary, &cfg.prompt_lang)));
                sink.emit(AgentEvent::Notice("收尾前自检:对照相关文件核对结论…".into())).await;
                sink.emit(AgentEvent::Status("正在核查结论…".into())).await; // 动态指示器起手
                turn += 1;
                continue;
            }
            final_text = summary;
            // transient(终端共驾观察轮):finish 收尾也不写主记忆/不跑飞轮(流式窗口记忆)。
            if !transient {
                memory.ingest_with_role(&final_text, "assistant");
                finalize(flywheel, memory, reasoner, sink).await;
            }
            return AgentOutcome { final_text, turns: turn + 1, stopped: StopReason::Completed };
        }

        // 让位暂停(打开了阻塞裁决类面板):干净收口,交还控制权给用户。
        // 任务未完成,不 finalize(那是完成/收尾的学习);只抛 Done 让前端停转。
        // 用户处理面板后,前端注入新一轮(确认→在新项目续做 / 取消→Agent 困惑并重弹或问)重新驱动。
        if let Some(action) = yielded {
            sink.emit(AgentEvent::Done).await;
            return AgentOutcome {
                final_text,
                turns: turn + 1,
                stopped: StopReason::AwaitingUser(action),
            };
        }

        turn += 1;
    }

    // 达到最大轮数:先把本回合最后一条已展示过的回复落库(凡展示过即落库),再收口学习一轮。
    persist_visible_reply(memory, &final_text, transient);
    if !transient {
        finalize(flywheel, memory, reasoner, sink).await;
    }
    AgentOutcome { final_text, turns: turn, stopped: StopReason::MaxTurns }
}

/// 把本回合最后一条**已流式展示给用户**的回复落库(role=assistant)。
/// ★修「AI 回复丢失」(2026-06-15 真机暴露)★:此前只有 finish 收口路径 ingest assistant;而跑满
/// max_turns(尤其 Supervisor 后台回合 max_turns=4 常如此)/ 用户终止 / LLM 出错收口时,final_text
/// 只经 sink 流式展示过、从未落时间线 → 重载/切项目后"好多 AI 回复没了",且 AI 检索不到自己干过什么
/// (上下文割裂)。不变式:凡作为 assistant 展示给用户的正文,必落时间线恰好一次(各终止点互斥 return,
/// 中途轮从不落库 → 不会重复)。transient(终端共驾观察轮)按设计不落主记忆;空文本不落。
fn persist_visible_reply(memory: &mut Memory, text: &str, transient: bool) {
    if !transient && !text.is_empty() {
        memory.ingest_with_role(text, "assistant");
    }
}

/// ★A2★ 本轮某次工具调用是否**成功编辑了一个非造物文件夹的 .rs 文件**;是则返回其绝对路径。
/// 只认 file_write/file_edit(file_list/file_read 不改文件)+ ok + 后缀 .rs + 非 `.growbox/` 造物文件夹。
fn edited_rust_path(name: &str, args: &str, ok: bool, work_dir: &Path) -> Option<PathBuf> {
    if !ok || !matches!(name, "file_write" | "file_edit") {
        return None;
    }
    let p = serde_json::from_str::<serde_json::Value>(args)
        .ok()?
        .get("path")?
        .as_str()?
        .to_string();
    if crate::artifact_fs::is_internal_state_path(work_dir, &p) {
        return None; // 造物自己文件夹:不推感知(主记忆隔离)
    }
    let path = {
        let pp = Path::new(&p);
        if pp.is_absolute() { pp.to_path_buf() } else { work_dir.join(pp) }
    };
    (path.extension().and_then(|e| e.to_str()) == Some("rs")).then_some(path)
}

/// ★A2 诊断推感知层★:编辑 .rs 后,若该工作区 rust-analyzer 已在跑(已暖、已索引),重新同步每个
/// 编辑过的文件 + 短轮询取其诊断 → perceive(双路:瞬态环 + 时间线,AI 当回合可见、可检索)。
/// 干净文件另发**瞬态**诚实说明(已检查、暂无;不落时间线免噪音,且点明 flycheck 可能仍在跑)。
/// **只对已暖客户端动作**:不因一次编辑就隐式起服务器/下载/冷索引(那会压上不可预期延迟)。
async fn perceive_rust_diagnostics(
    mgr: &crate::lsp::LspManager,
    memory: &mut Memory,
    prompt_lang: &str,
    work_dir: &Path,
    edited: &[PathBuf],
) {
    let Some(client) = mgr.existing_rust_client(work_dir).await else {
        return; // RA 未起(AI 尚未用过 lsp)→ 静默不感知,不隐式拉起
    };
    for path in edited {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let diags = client.sync_and_diagnose(path, &text, crate::lsp::DIAG_POLL_BUDGET).await;
        let rel = path.strip_prefix(work_dir).unwrap_or(path).display().to_string();
        match crate::lsp::summarize_diagnostics(&rel, &diags) {
            // 有 error/warning:双路感知(留痕、可检索),AI 据此当回合修。
            Some(msg) => memory.perceive(growbox_memory::node_kind::LSP_DIAGNOSTIC, msg),
            // 暂无:仅瞬态(不落时间线),诚实点明 flycheck 可能仍在后台跑,别当"绝对干净"。
            None => memory.perceive_transient(
                growbox_memory::node_kind::LSP_DIAGNOSTIC,
                if prompt_lang.starts_with("zh") {
                    format!("rust-analyzer 已检查「{rel}」,暂未报 error/warning(注:完整 flycheck 可能仍在后台跑)")
                } else {
                    format!("rust-analyzer checked \"{rel}\": no error/warning surfaced yet (note: full flycheck may still be running)")
                },
            ),
        }
    }
}

/// 造物文件夹隔离判据:是不是"对造物自己文件夹(.growbox/)的文件操作"。
/// 是 → 脊柱跳过主记忆采集(造物的流记忆/持久状态不污染主时间线/RAG,见 artifact_fs)。
fn is_internal_state_file_op(name: &str, args: &str, work_dir: &Path) -> bool {
    if !matches!(name, "file_read" | "file_write" | "file_edit" | "file_list") {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(args)
        .ok()
        .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(String::from))
        .map(|p| crate::artifact_fs::is_internal_state_path(work_dir, &p))
        .unwrap_or(false)
}

/// 收口:只抛 Done。三个退出点(空转兜底 / finish / MaxTurns)共用。
///
/// ★设计★:经验的**提炼/压缩**(idle 学习)按 `设计/04`+`系统架构/00` 是 **idle 时**做、
/// 且"收集异步不挡主线"——故**不再在前台回合里压缩**(那会卡住回合返回:前端等
/// `send_chat` 命令返回才停转、且 `run_chat` 全程持状态锁)。压缩移交独立的 `IdleWorker`
/// (见 `gui::idle`):静默 8 分钟后,对**镜像**(克隆的经验)无锁处理,只在写回时极短持锁,
/// 不影响前台继续写入。优先级 Agent > 飞轮(新问题一来即让位)。
/// 参数 `_flywheel/_memory/_reasoner` 现已不在前台用(保留签名,免改所有调用点)。
async fn finalize(_flywheel: &Flywheel, _memory: &mut Memory, _reasoner: &dyn Reasoner, sink: &dyn EventSink) {
    sink.emit(AgentEvent::Done).await;
}
