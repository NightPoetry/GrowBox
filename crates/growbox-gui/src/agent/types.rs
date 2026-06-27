//! Agent 循环的公开类型:事件、事件汇 trait、一次对话的配置与结果。
//!
//! 这些是脊柱与上层(Tauri sink / 测试收集器)之间的契约,故单独成文件、由 `mod.rs`
//! 经 `pub use types::*` 重导出(外部仍按 `agent::AgentEvent` 等路径引用,不变)。

use growbox_core::UiIntent;

use crate::decision::{Decision, DecisionKind};
use crate::ui::UiAck;

/// 循环抛给上层的事件。
#[derive(Debug, Clone, PartialEq)]
pub enum AgentEvent {
    /// 思维链片段。
    Reasoning(String),
    /// 正文片段。
    Content(String),
    /// 系统提示(如截断重试通知)。
    Notice(String),
    /// ★瞬态状态★(不进聊天记录,只驱动一个动态指示器):如"正在核查:读取 a.txt"。
    /// 用于主动自检阶段的动效("正在核查 xxx",随核查对象变);前端显示一个动画 pill,Done 时清除。
    Status(String),
    /// 开始执行某工具。
    ToolStart { name: String, args: String },
    /// 工具执行完毕。
    ToolEnd { name: String, ok: bool, content: String },
    /// 交互类执行器请求前端弹预填 UI(控制反转)。
    Intent(UiIntent),
    /// 本次对话结束。
    Done,
}

/// 事件汇 —— 实现者把事件送达 UI / 测试收集器。
///
/// `emit` 是 fire-and-forget 叙述流;`ui_round_trip` 是家族二 UI 操作的 request/response:
/// 发出请求并**等前端回执**,返回验证态(不撒谎)。脊柱是前端通信的唯一中介,故往返也在此。
#[async_trait::async_trait]
pub trait EventSink: Send + Sync {
    async fn emit(&self, event: AgentEvent);

    /// 回合级取消检查(造物交互 v2 §2):脊柱每轮读;true = 用户已按「终止」,本轮优雅收口。
    /// 默认无前端 → 永不取消。TauriSink 读独立 `ChatControl` managed state(不走 AppState 锁)。
    fn is_cancelled(&self) -> bool {
        false
    }

    /// 家族二 UI 往返:请前端落地一个 UI 操作并回报结果态。默认无前端 → 未应用(诚实)。
    async fn ui_round_trip(&self, _intent: &UiIntent) -> UiAck {
        UiAck::unapplied("无前端(默认 sink)")
    }

    /// 用户决定脊柱:凡需用户裁决才能继续的动作(shell 审批 / 路径授权 / 隐私确认)都经此 round-trip——
    /// 发请求 → **阻塞等前端回执** → 返回裁决。实现方负责"已信任则免问"+ shell 记忆(见 `Decisions`)。
    /// 默认无前端(测试/非流式)→ 按类给安全默认:shell 放行一次(硬底线仍在 judge);
    /// 路径授权拒绝(没有用户在场就无法授权,安全侧)。
    async fn request_decision(&self, kind: DecisionKind) -> Decision {
        match kind {
            DecisionKind::ShellApproval { .. } => Decision::Once,
            DecisionKind::PathPermission { .. } => Decision::Deny,
        }
    }

    /// 本次 LLM 请求的实际上下文 token 数(模型亲口回报,见 StreamChunk::Usage)→ 面板"实时上下文压力"。
    /// 默认无前端 → 丢弃(测试/分支/后台不更新面板表)。TauriSink 写独立 `ContextMeter` managed 原子。
    fn note_context_tokens(&self, _prompt_tokens: u32) {}

    /// 回合级取消句柄,穿进 `ExecCtx.cancel` → 让长命令(shell)中途响应「终止」(不止 LLM 流式循环)。
    /// 默认无前端 → None(不可中途取消)。TauriSink 返回 `ChatControl.flag()`(与 `is_cancelled` 同源)。
    fn cancel_flag(&self) -> growbox_core::CancelFlag {
        None
    }

    /// 交互式终端(人机共驾 shell):开一个 PTY 会话跑 `command`,返回 session id;无前端 → None。
    /// TauriSink 实现:把会话输出经 `app.emit("terminal-output")` 直推前端 xterm(独立于 agent 回合,
    /// 贴合会话长生命周期),并 emit `terminal-open` 让前端自动挂载终端面板。脊柱据返回 id 回执给 LLM。
    async fn open_terminal(&self, _command: &str, _work_dir: &std::path::Path) -> Option<String> {
        None
    }
}

/// 一次对话配置。
pub struct AgentConfig {
    pub model: String,
    pub max_tokens: u32,
    /// 循环最大轮数;0 = 无限(长任务不被硬墙截断,见 Settings.max_turns)。
    pub max_turns: u32,
    /// ★并行子代理并发上限★:一回合发出多个 `context_mode=isolated` 调查员调用时,最多几个同时在飞
    /// (其余排队跑完)。默认 4;1 = 退化为顺序、结果不丢。见 `设计/07-附录-并行子代理`。
    pub parallel_max: usize,
    pub system_prompt: String,
    /// 提示词语言(zh/en):决定发给 LLM 的工具 schema description 取哪种文案(与界面语言解耦)。
    pub prompt_lang: String,
    /// 自动模式:false=手动(shell 逐条批准),true=自动(LLM 审核)。决定 shell 批准门策略。
    pub auto_mode: bool,
    /// ★danger 模式(为所欲为)★:全自动之上的最高放行档。true = shell_gate 全跳过 + sandbox.judge 一律放行
    /// (系统级操作/敏感路径/危险命令/SSRF 全不拦),供无人值守自驱做系统装包等不卡授权。极高风险、会话级。
    pub danger_mode: bool,
    /// 用户配置的隐私文件夹绝对路径:命中(且未授权)必弹窗 + 二次确认,两模式都不绕过。
    pub privacy_dirs: Vec<String>,
    /// 截断重试上限:工具调用被截成空参时,最多翻倍 token 重试几次(默认 2,推论9 可设)。
    pub max_token_retries: usize,
    /// 截断重试的 token 上限:重试翻倍不超过此值(默认 32768)。
    pub token_ceil: u32,
    /// 流式沉默超时秒:任何 chunk(含 reasoning)都重置;真沉默超此值判超时(默认 90)。
    pub silence_secs: u64,
    /// 退化死循环上限:连续多少轮产出"近乎全等"才判高频重复收口(默认 2)。
    /// ★思考免死★:产出新内容(含 reasoning)永远不收口,只有真重复才退化(用户原则 2026-06-03)。
    pub max_stall: usize,
    /// 思考强度(deepseek V4):"high"/"max",空串=不发(吃服务端默认)。默认 "max"。
    pub reasoning_effort: String,
    /// 分支日志上限(GB,-1=无限):派生分支调用信息原样存项目日志文件,环形覆盖(见 Settings.branch_log_max_gb)。
    pub branch_log_max_gb: f64,
    /// ★主动自检★:收尾前 AI 拿工作汇报重读相关文件核对、改正/标注后再 finish(grounded verification)。
    /// 默认开;关掉省 token。仅主链、且本次工具调用数 ≥ `self_verify_min_tools` 时触发,且每次任务至多一轮。
    pub self_verify: bool,
    /// 自检触发阈值:本次任务工具调用数 ≥ 此值才自检(轻任务不花钱)。
    pub self_verify_min_tools: usize,
    /// ★回合内补检索(用户决策:回合内重跑检索)★:开场 `assemble_context` 只按进场用户消息检索一次;
    /// 任务跑到一半 AI 才需要的信息(如开始 SSH 才要的凭据)开场未必召回。开 = 每轮顶端用"AI 上一轮的
    /// 思路+进展"作新查询再检索(复用 RAG→精确两层),新命中增量注入(append-only、去重、无新命中不打扰)。
    /// 仅主链(非派生分支)生效。默认开;关 = 行为同今天(只开场检索一次)。
    pub recall_in_loop: bool,
    /// ★工具记忆 + 不犯第二遍(计划/工具记忆-不犯第二遍)★ 总开关:开 = 分发前会诊「小本本」+
    /// 本回合失败指纹守卫。关 = 全不做(行为同今天)。
    pub tool_memory_enabled: bool,
    /// 工具记忆「不可行」硬否决相似度阈(默认 0.85):当前情况与已记 infeasible 余弦 ≥ 此 → 否决重试。
    pub tool_memory_veto_threshold: f32,
    /// 工具记忆「失败」软提醒相似度阈(默认 0.80):≥ 此 → 执行前注入"曾在类似情况失败"提醒(不阻断)。
    pub tool_memory_warn_threshold: f32,
}

/// 循环为何停下。
#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    Completed,
    MaxTurns,
    Error(String),
    /// 让位暂停:打开了阻塞裁决类面板(gated hand_off,如 create_project),
    /// 本轮交还控制权给用户;用户处理后由前端注入新一轮重新驱动。携带面板 action。
    AwaitingUser(String),
    /// 用户主动终止(造物交互 v2 §2):按「终止」叫停当前回合,脊柱在下一检查点优雅收口。
    Cancelled,
}

/// 一次对话的结果。
#[derive(Debug, Clone)]
pub struct AgentOutcome {
    pub final_text: String,
    pub turns: usize,
    pub stopped: StopReason,
}
