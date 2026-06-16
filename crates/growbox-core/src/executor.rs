//! 执行器 —— 一切能力的统一形态。
//!
//! 实现 `设计文档/系统架构/01-core.md` 与 `设计/05-工具系统.md`:
//! 每个能力 = 同一形态,经一个注册表、一条分发路径调用。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 一次调用要访问的资源声明 —— 供安全门按统一形态判定(见 `设计/03`)。
///
/// 注册表 dispatch 时问每个执行器"你这次动什么",据此走 safety 的单一判定路径;
/// core 不依赖 safety,只给出纯数据,由 app 映射到 `safety::Operation`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    Read(PathBuf),
    Write(PathBuf),
    Shell(String),
    /// 出站网络访问(完整 URL)。公网放行;内网/本机交还用户裁决;非 http(s) 拒(见 `设计/03`)。
    Net(String),
}

/// 风险/可逆等级 —— 决定安全门怎么处理(见 `设计/00` 推论4、`设计/03`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    /// 只读/无副作用,直接执行。
    Safe,
    /// 有副作用但可回滚,落在可写范围内直接执行。
    Reversible,
    /// 不可逆/高危,需把裁决交还用户。
    Irreversible,
}

/// 给 LLM 的工具定义(OpenAI function schema 形态)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema 形式的参数定义。
    pub params: serde_json::Value,
}

/// LLM 发起的一次工具调用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// 原始 JSON 字符串参数(可能为空 = 被截断,见 `实验记录/00`)。
    pub arguments: String,
}

/// 执行结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub ok: bool,
    pub content: String,
}

impl ToolResult {
    pub fn ok(content: impl Into<String>) -> Self {
        ToolResult { ok: true, content: content.into() }
    }
    pub fn fail(content: impl Into<String>) -> Self {
        ToolResult { ok: false, content: content.into() }
    }
}

/// 交互意图 —— 执行器不直接动手,而是请前端落地一个 UI 动作(见 `设计/00-交互层` 推论 7)。
///
/// 两个家族(本质不同,故行为不同,由 `await_ack` 区分):
/// - 家族一「交付用户裁决」(hand_off):弹出预填表单,发出即返回 —— "成功"= 已把表单呈现给用户,
///   裁决在带外(用户填了再定)。fire-and-forget 在此是**诚实**的。例:open_settings / create_project。
/// - 家族二「Agent 自己对 UI 动手」(round_trip):脊柱发出后**等前端回执**,返回验证过的状态 ——
///   不撒谎(否则会被飞轮当真结论学进记忆)。例:ui_control 开/关/切面板。
///
/// 家族一再细分(由 `gates` 区分):
/// - 普通 hand_off(gates=false):纯呈现,Agent 发出后继续干别的(如 open_settings、toast)。
/// - 阻塞裁决 hand_off(gates=true):后续工作依赖用户处理结果(如 create_project)——脊柱发出面板后
///   **本轮 run 让位暂停**(StopReason::AwaitingUser),交还控制权给用户;用户裁决(确认/取消)再经
///   前端注入新一轮重新驱动 Agent。修正"弹板即假成功+空转催续推着冲、感知不到取消"的缺陷
///   (用户决策 2026-06-02)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiIntent {
    /// 要落地的 UI 动作(如 "create_project" / "ui_control")。
    pub action: String,
    /// 预填/参数(键=字段名,值=值;不确定的字段不放进来 = 留空)。
    pub prefill: serde_json::Value,
    /// true = 家族二:脊柱发出后等前端回执,返回验证态;false = 家族一:发出即返回。
    pub await_ack: bool,
    /// 仅家族一有意义:true = 阻塞裁决类,脊柱发出后让位暂停等用户处理(后续依赖其结果)。
    pub gates: bool,
}

impl UiIntent {
    /// 家族一:弹预填表单交用户裁决,发出即返回(fire-and-forget 是诚实的"已呈现")。
    pub fn hand_off(action: impl Into<String>, prefill: serde_json::Value) -> Self {
        UiIntent { action: action.into(), prefill, await_ack: false, gates: false }
    }
    /// 家族一·阻塞裁决:弹板后脊柱让位暂停,等用户处理(确认/取消)再重新驱动。后续工作依赖其结果。
    pub fn hand_off_gating(action: impl Into<String>, prefill: serde_json::Value) -> Self {
        UiIntent { action: action.into(), prefill, await_ack: false, gates: true }
    }
    /// 家族二:Agent 自己对 UI 动手,脊柱发出后等前端回执,返回验证态(不撒谎)。
    pub fn round_trip(action: impl Into<String>, prefill: serde_json::Value) -> Self {
        UiIntent { action: action.into(), prefill, await_ack: true, gates: false }
    }
}

/// 工具输出上限旋钮(推论9 数值全可设;由 `Settings` 透传,经 Registry → ExecCtx 注入)。
/// 默认 = 各执行器历来的写死常量;运行时可在控制面板/设置调。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolLimits {
    /// file_read 单次读取字节上限(默认 200KB)。
    pub max_read_bytes: usize,
    /// file_list 列出条目上限(默认 500)。
    pub max_list_entries: usize,
    /// shell 输出字节上限(默认 64KB)。
    pub max_output_bytes: usize,
    /// code_outline 大纲符号数上限(默认 400;超长文件防刷屏)。
    pub max_outline_symbols: usize,
    /// shell 命令墙钟超时秒(默认 60;0 = 不限,慎用)。超时杀整进程组、返回已收输出。
    /// 防"命令永不返回"(如前台起服务、或 `cmd &` 后台子进程攥住管道致捕获方等不到 EOF)。
    pub shell_timeout_secs: u64,
}

impl Default for ToolLimits {
    fn default() -> Self {
        ToolLimits {
            max_read_bytes: 200 * 1024,
            max_list_entries: 500,
            max_output_bytes: 64 * 1024,
            max_outline_symbols: 400,
            shell_timeout_secs: 60,
        }
    }
}

/// 回合级取消句柄 —— 用户按「终止」即置位的共享原子(无前端/后台路径为 None)。
/// 执行器(尤其 shell)据此在长命令执行**中途**响应终止,而非只在 LLM 流式循环里能取消。
/// core 零依赖,故用裸 std 原子(不引 tokio_util)。
pub type CancelFlag = Option<std::sync::Arc<std::sync::atomic::AtomicBool>>;

/// 执行上下文 —— 执行器运行时能拿到的环境。
pub struct ExecCtx<'a> {
    /// 解析后的参数。
    pub args: serde_json::Value,
    /// 当前工作目录(项目根)。
    pub work_dir: &'a std::path::Path,
    /// 工具输出上限(推论9 可设;默认见 `ToolLimits::default`)。
    pub limits: ToolLimits,
    /// 回合级取消句柄(用户「终止」置位)。None = 无前端/不可取消路径。见 `is_cancelled`。
    pub cancel: CancelFlag,
}

impl ExecCtx<'_> {
    /// 用户是否已请求终止本回合(执行器在长任务中途轮询)。无句柄 → 永不取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .map(|f| f.load(std::sync::atomic::Ordering::SeqCst))
            .unwrap_or(false)
    }
}

/// 执行器 trait —— 所有能力实现它。
///
/// `execute` 是 async:异步能力(后台任务、网络、做梦)也是一等执行器,
/// 经同一注册表、同一分发路径调用,不再绕开脊柱另起一套(架构公理)。
#[async_trait::async_trait]
pub trait Executor: Send + Sync {
    /// 工具名(全局唯一,注册表的键)。
    fn name(&self) -> &str;

    /// 给 LLM 的定义。`description` 由注册表按 prompt_lang 从工具文案单一源注入
    /// (本方法返回的 description 通常留空 = 占位);`name`/`params` 仍由各执行器自报。
    fn definition(&self) -> ToolDef;

    /// 动态描述补丁:返回需要拼进文案 `llm_desc` 里 `{catalog}` 占位符的运行时片段。
    /// 绝大多数执行器无动态部分(返回 None);`ui_control` 返回前端声明的当前面板目录。
    fn desc_dynamic(&self) -> Option<String> {
        None
    }

    /// 风险等级(交给安全门)。
    fn risk(&self) -> Risk;

    /// 交互类执行器返回预填 UI;默认无。
    fn ui_intent(&self, _args: &serde_json::Value) -> Option<UiIntent> {
        None
    }

    /// 终止类执行器:执行后脊柱应收口本次任务(如 finish)。默认否。
    /// 这是结束 Agent 循环的唯一"正门"——裸文本不再等于完成。
    fn terminal(&self) -> bool {
        false
    }

    /// 提问类执行器:执行后脊柱应**让位暂停**等用户回答(如 ask_user),停于 `AwaitingUser`。默认否。
    /// 与 `terminal` 并列的循环出口:terminal=任务完成(Completed);awaits_user=需用户回答才能继续。
    /// 解决"agent 用纯文字问用户被空转催续逻辑当停滞、催它继续/重复回答"——显式调它即干净暂停。
    fn awaits_user(&self) -> bool {
        false
    }

    /// 本次调用要访问的资源(供安全门判定);默认 None = 无需路径/命令判定。
    /// `work_dir` 用于把相对路径解析为绝对路径,使沙箱判定的目标与实际执行一致。
    fn claim(&self, _args: &serde_json::Value, _work_dir: &std::path::Path) -> Option<Claim> {
        None
    }

    /// 执行。
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy;
    #[async_trait::async_trait]
    impl Executor for Dummy {
        fn name(&self) -> &str {
            "dummy"
        }
        fn definition(&self) -> ToolDef {
            ToolDef {
                name: "dummy".into(),
                description: "test".into(),
                params: serde_json::json!({"type":"object","properties":{}}),
            }
        }
        fn risk(&self) -> Risk {
            Risk::Safe
        }
        async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
            ToolResult::ok("done")
        }
    }

    #[test]
    fn executor_basic() {
        let d = Dummy;
        assert_eq!(d.name(), "dummy");
        assert_eq!(d.risk(), Risk::Safe);
        assert!(d.ui_intent(&serde_json::json!({})).is_none());
    }

    #[test]
    fn tool_result_helpers() {
        assert!(ToolResult::ok("x").ok);
        assert!(!ToolResult::fail("y").ok);
    }
}
