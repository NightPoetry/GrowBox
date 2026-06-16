//! shell 批准门:对"过了硬安全底线的普通命令"按模式(手动逐条批准 / 自动 LLM 审核)裁决。
//! 硬底线(危险命令 Deny / 敏感密钥 NeedAuth)归 `registry.dispatch`,不在此问;此处只处理 Allow 后的策略。

use std::path::Path;

use growbox_core::ToolResult;
use growbox_llm::{ChatMessage, ChatRequest};
use growbox_safety::{Operation, Sandbox, Verdict};

use crate::bridge::{complete, LlmDriver};
use crate::decision::DecisionKind;

use super::{AgentConfig, EventSink};

/// LLM 安全审核员的判定。
enum ShellVerdict {
    /// 放行(普通构建/测试/运行)。
    Safe,
    /// 触及隐私/个人文件夹,需用户授权某目录(最小权限)。
    NeedsPermission { path: String, access: String, reason: String },
    /// 破坏性/危险,应拦下。
    Dangerous { reason: String },
}

/// 从 shell 工具的 JSON 参数里取 `command` 字段(供批准门用)。非法/缺省返回 None。
pub(super) fn shell_command_of(args: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(args).ok()?;
    let cmd = v.get("command")?.as_str()?.trim();
    if cmd.is_empty() {
        None
    } else {
        Some(cmd.to_string())
    }
}

/// shell 批准门:过了硬底线的命令按模式裁决。返回 Some=不执行(把该结果回灌 LLM),None=放行。
pub(super) async fn shell_gate(
    cmd: &str,
    cfg: &AgentConfig,
    llm: &dyn LlmDriver,
    sandbox: &Sandbox,
    work_dir: &Path,
    sink: &dyn EventSink,
) -> Option<ToolResult> {
    // ★danger 模式(为所欲为)★:跳过一切批准门(LLM 审核 + 个人/隐私文件夹网),直接放行。
    // 此时 sandbox.judge 也已一律 Allow(见 Sandbox::set_danger),两处同由 Settings.danger_mode 驱动。
    if cfg.danger_mode {
        return None;
    }
    // 硬底线(危险命令 Deny / 敏感密钥 NeedAuth)归 dispatch:非 Allow 不进批准门。
    if !matches!(sandbox.judge(&Operation::Shell(cmd)), Verdict::Allow) {
        return None;
    }
    // 用户配置的隐私文件夹:命中且**未授权**则**必弹窗 + 二次确认**(privacy=true),先于模式裁决、
    // 不被自动模式或"信任本项目 shell"绕过(用户决策 2026-06-02)。已授权(在读写列表里)则放行。
    if let Some(dir) = crate::privacy::user_privacy_in_command(cmd, &cfg.privacy_dirs) {
        let granted = matches!(sandbox.judge(&Operation::Read(Path::new(&dir))), Verdict::Allow);
        if !granted {
            // 经决定脊柱**阻塞等用户裁决**(隐私文件夹在前端弹窗里二次确认);拒绝/超时才拦,放行则继续。
            let kind = DecisionKind::PathPermission {
                path: dir,
                reason: "命令触及你设置的隐私文件夹".into(),
                access: "read".into(), // 最小权限:先申请读
                privacy: true,
            };
            if !sink.request_decision(kind).await.allows() {
                return Some(ToolResult::fail("用户未授权访问隐私文件夹"));
            }
        }
    }
    if cfg.auto_mode {
        match shell_auto_verdict(llm, &cfg.model, cmd, work_dir).await {
            ShellVerdict::Safe => None,
            ShellVerdict::NeedsPermission { path, access, reason } => {
                // 最小权限:reviewer 给 read 就只申请 read。经决定脊柱阻塞问用户,放行则继续、拒绝才拦。
                let kind = DecisionKind::PathPermission { path, reason: reason.clone(), access, privacy: false };
                if sink.request_decision(kind).await.allows() {
                    None
                } else {
                    Some(ToolResult::fail(format!("用户未授权(触及隐私/个人文件夹): {reason}")))
                }
            }
            ShellVerdict::Dangerous { reason } => {
                Some(ToolResult::fail(format!("安全审核判定有风险,已拦下: {reason}")))
            }
        }
    } else {
        // 手动模式:逐条交用户裁决(已信任由脊柱免问 + 记忆)。
        let kind = DecisionKind::ShellApproval { command: cmd.to_string() };
        if sink.request_decision(kind).await.allows() {
            None
        } else {
            Some(ToolResult::fail("用户拒绝执行该命令"))
        }
    }
}

/// 自动模式:LLM 安全审核员 + Rust 个人文件夹隐私网(不全靠 LLM)。
async fn shell_auto_verdict(llm: &dyn LlmDriver, model: &str, cmd: &str, work_dir: &Path) -> ShellVerdict {
    let audit = audit_shell_command(llm, model, cmd, work_dir).await;
    // Rust 安全网:LLM 说 safe 但命令触及越界个人文件夹 → 仍要授权(默认先申请读,最小权限)。
    if matches!(audit, ShellVerdict::Safe) {
        if let Some(path) = crate::privacy::personal_path_in_command(cmd, work_dir) {
            return ShellVerdict::NeedsPermission {
                path,
                access: "read".into(),
                reason: "命令触及你的个人文件夹".into(),
            };
        }
    }
    audit
}

/// 调 LLM 审核一条 shell 命令。失败/解析不出 → 降级为 Safe(硬底线 + Rust 隐私网仍兜底)。
async fn audit_shell_command(llm: &dyn LlmDriver, model: &str, cmd: &str, work_dir: &Path) -> ShellVerdict {
    let home = dirs::home_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let sys = "你是 shell 命令安全审核员。审核 AI 要执行的命令并判定:\
        ① 是否危害用户数据安全(删除/覆盖/清空重要数据等破坏性操作);\
        ② 是否触及用户隐私/个人文件夹(家目录下 Documents/Desktop/Downloads/Pictures 等,或读取敏感信息),\
        但工作目录内的读写属正常工作不算;\
        ③ 普通构建/测试/运行程序(node/npm/cargo/git/python 等)属安全。\
        只输出 JSON 不要解释:\
        {\"verdict\":\"safe|needs_permission|dangerous\",\"path\":\"涉及目录(needs_permission 时给绝对路径,否则空)\",\"access\":\"read|write(命令对该目录是读还是写,最小权限)\",\"reason\":\"一句话原因\"}";
    let user = format!("家目录: {home}\n工作目录: {}\n命令: {cmd}", work_dir.display());
    let req = ChatRequest::new(model.to_string(), vec![ChatMessage::system(sys), ChatMessage::user(user)]);
    // shell 安全审核是 best-effort(失败即降级 Safe);用固定 60s 沉默超时即可,不必做成旋钮。
    let Ok(out) = complete(llm, req, 60).await else {
        return ShellVerdict::Safe; // 审核失败:降级放行(硬底线 + 隐私网仍在)
    };
    let Some(json) = extract_json_str(&out) else {
        return ShellVerdict::Safe;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) else {
        return ShellVerdict::Safe;
    };
    let reason = v.get("reason").and_then(|x| x.as_str()).unwrap_or("").to_string();
    match v.get("verdict").and_then(|x| x.as_str()).unwrap_or("safe") {
        "dangerous" => ShellVerdict::Dangerous { reason },
        "needs_permission" => {
            let path = v.get("path").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            if path.is_empty() {
                return ShellVerdict::Safe; // 没给路径无法授权,降级(Rust 网会兜)
            }
            let access = match v.get("access").and_then(|x| x.as_str()) {
                Some("write") => "write",
                _ => "read", // 默认最小权限:读
            };
            ShellVerdict::NeedsPermission { path, access: access.into(), reason }
        }
        _ => ShellVerdict::Safe,
    }
}

/// 从可能含围栏/前后说明的 LLM 输出里抠第一段 JSON(对象/数组)。与 bridge 的同名逻辑一致(此处本地小拷贝)。
fn extract_json_str(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{' || b == b'[')?;
    let open = bytes[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let mut depth = 0;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(s[start..=i].to_string());
            }
        }
    }
    None
}
