//! 后台任务类执行器:spawn_task / wait_tasks / list_tasks。
//!
//! 这三个是 async 能力(等完成事件、退避巡检),正因执行器 trait 已是 async,
//! 它们就是一等执行器——经唯一注册表、唯一分发路径调用,安全门由 dispatch 统一把关
//! (spawn_task 用 `claim()` 声明它要跑的 shell 命令,dispatch 据此过沙箱)。
//! 不再在脊柱里按名拦截、也不再自己重写一遍安全门。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use growbox_core::{Claim, ExecCtx, Executor, Risk, ToolDef, ToolResult};

use crate::tasks::{DoneWhen, TaskManager, TaskRecord, TaskState};

// wait_tasks 指数退避基数/上限已暴露为可设(推论9):由 `TaskManager`(原子)持有,
// 运行时从 Settings 注入,WaitTasks 经 `self.task_mgr` 读取。默认 2 秒起 / 封顶 60 秒。

/// 解析 done_when 字符串为 DoneWhen 枚举。
pub(crate) fn parse_done_when(s: &str) -> Result<DoneWhen, String> {
    let s = s.trim();
    if s == "exit" {
        return Ok(DoneWhen::Exit);
    }
    if let Some(path) = s.strip_prefix("file:") {
        return Ok(DoneWhen::FileExists(path.trim().into()));
    }
    if let Some(port_str) = s.strip_prefix("port:") {
        let port: u16 = port_str.trim().parse().map_err(|_| format!("端口号无效: {port_str}"))?;
        return Ok(DoneWhen::PortOpen(port));
    }
    if let Some(cmd) = s.strip_prefix("probe:") {
        return Ok(DoneWhen::Probe(cmd.trim().into()));
    }
    Err(format!("未知的 done_when 格式: {s}(应为 exit / file:路径 / port:端口 / probe:命令)"))
}

fn format_finished(records: &[TaskRecord]) -> String {
    let mut lines = Vec::new();
    for r in records {
        let state = match r.state {
            TaskState::Running => unreachable!(),
            TaskState::Done => "完成",
            TaskState::Failed => "失败",
        };
        let output = if r.output.is_empty() {
            String::new()
        } else {
            // 截取前 500 字符防刷屏。
            let tail: String = r.output.chars().take(500).collect();
            format!("\n    输出: {tail}")
        };
        lines.push(format!("  {} [{}] {} | 命令: {} ({:.1}s){output}", r.id, state, r.label, r.command, r.elapsed_ms as f64 / 1000.0));
    }
    format!("后台任务结果:\n{}", lines.join("\n"))
}

// --- spawn_task ---

pub struct SpawnTask {
    task_mgr: Arc<TaskManager>,
}

impl SpawnTask {
    pub fn new(task_mgr: Arc<TaskManager>) -> Self {
        Self { task_mgr }
    }
}

#[async_trait]
impl Executor for SpawnTask {
    fn name(&self) -> &str {
        "spawn_task"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入

            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "要执行的 shell 命令" },
                    "label": { "type": "string", "description": "任务的简短描述" },
                    "done_when": { "type": "string", "description": "完成判据: \"exit\" | \"file:路径\" | \"port:端口\" | \"probe:命令\"" }
                },
                "required": ["command", "label", "done_when"]
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Reversible
    }
    /// 声明这次要跑的 shell 命令,交给 dispatch 的唯一安全门判定(后台不绕沙箱)。
    fn claim(&self, args: &serde_json::Value, _work_dir: &Path) -> Option<Claim> {
        args.get("command").and_then(|v| v.as_str()).map(|c| Claim::Shell(c.to_string()))
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let Some(command) = ctx.args.get("command").and_then(|v| v.as_str()) else {
            return ToolResult::fail("缺少参数 command");
        };
        let label = ctx.args.get("label").and_then(|v| v.as_str()).unwrap_or("未命名任务");
        let Some(done_when_str) = ctx.args.get("done_when").and_then(|v| v.as_str()) else {
            return ToolResult::fail("缺少参数 done_when");
        };
        let done_when = match parse_done_when(done_when_str) {
            Ok(d) => d,
            Err(e) => return ToolResult::fail(e),
        };
        // 安全门已由 dispatch 过(claim → judge → risk_gate);此处直接起任务。
        let id = self.task_mgr.spawn_shell(label, command.to_string(), ctx.work_dir.to_path_buf(), done_when);
        ToolResult::ok(format!("后台任务已启动: {id} (标签: {label})"))
    }
}

// --- wait_tasks ---

pub struct WaitTasks {
    task_mgr: Arc<TaskManager>,
}

impl WaitTasks {
    pub fn new(task_mgr: Arc<TaskManager>) -> Self {
        Self { task_mgr }
    }
}

#[async_trait]
impl Executor for WaitTasks {
    fn name(&self) -> &str {
        "wait_tasks"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入

            params: serde_json::json!({ "type": "object", "properties": {} }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        let task_mgr = &self.task_mgr;
        if task_mgr.running_count() == 0 {
            let finished = task_mgr.drain_finished();
            if finished.is_empty() {
                return ToolResult::ok("当前没有在跑的后台任务。");
            }
            return ToolResult::ok(format_finished(&finished));
        }

        // 指数退避循环:等完成事件 or 超时退避。基数/上限取自 TaskManager 旋钮(推论9 可设)。
        let backoff_cap = task_mgr.backoff_cap_ms();
        let mut backoff = task_mgr.backoff_base_ms();
        loop {
            tokio::select! {
                _ = task_mgr.wait_event() => {
                    let finished = task_mgr.drain_finished();
                    if !finished.is_empty() {
                        return ToolResult::ok(format_finished(&finished));
                    }
                    // notify 但无完成项(可能被 drain 走了),继续等。
                }
                _ = tokio::time::sleep(Duration::from_millis(backoff)) => {
                    // 退避超时:检查卡死任务。
                    let reaped = task_mgr.reap_hung(Duration::from_millis(backoff));
                    if !reaped.is_empty() {
                        return ToolResult::ok(format_finished(&reaped));
                    }
                    backoff = (backoff * 2).min(backoff_cap);
                    if task_mgr.running_count() == 0 {
                        let finished = task_mgr.drain_finished();
                        return ToolResult::ok(if finished.is_empty() {
                            "所有后台任务已完成。".into()
                        } else {
                            format_finished(&finished)
                        });
                    }
                }
            }
        }
    }
}

// --- list_tasks ---

pub struct ListTasks {
    task_mgr: Arc<TaskManager>,
}

impl ListTasks {
    pub fn new(task_mgr: Arc<TaskManager>) -> Self {
        Self { task_mgr }
    }
}

#[async_trait]
impl Executor for ListTasks {
    fn name(&self) -> &str {
        "list_tasks"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入

            params: serde_json::json!({ "type": "object", "properties": {} }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        let snapshot = self.task_mgr.snapshot();
        if snapshot.is_empty() {
            return ToolResult::ok("当前没有后台任务。");
        }
        let mut lines = Vec::new();
        for t in &snapshot {
            let state = match t.state {
                TaskState::Running => "运行中",
                TaskState::Done => "已完成",
                TaskState::Failed => "失败",
            };
            // tag(label)+ 原命令 + 状态 一并给 LLM:对自身衍生的后台任务绝对感知(设计 05 推论6)。
            lines.push(format!("  {} [{}] {} | 命令: {} ({:.1}s)", t.id, state, t.label, t.command, t.elapsed_ms as f64 / 1000.0));
        }
        ToolResult::ok(format!("后台任务列表:\n{}", lines.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_done_when_all_formats() {
        assert_eq!(parse_done_when("exit").unwrap(), DoneWhen::Exit);
        assert_eq!(parse_done_when("file:ready.txt").unwrap(), DoneWhen::FileExists("ready.txt".into()));
        assert_eq!(parse_done_when("port:8080").unwrap(), DoneWhen::PortOpen(8080));
        assert_eq!(
            parse_done_when("probe:curl -s http://localhost/health").unwrap(),
            DoneWhen::Probe("curl -s http://localhost/health".into())
        );
        assert!(parse_done_when("unknown").is_err());
        assert!(parse_done_when("port:abc").is_err());
    }

    #[test]
    fn spawn_task_claims_its_command() {
        let exec = SpawnTask::new(TaskManager::new());
        let claim = exec.claim(&serde_json::json!({"command":"cargo build"}), Path::new("/"));
        assert_eq!(claim, Some(Claim::Shell("cargo build".into())));
    }
}
