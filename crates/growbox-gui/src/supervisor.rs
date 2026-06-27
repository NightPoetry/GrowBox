//! 常驻 Supervisor —— app 生命周期级后台 tokio 任务。
//!
//! 实现交接报告 §5A 第 3 步:connect 时起 app 级后台任务,监听 TaskManager 完成事件,
//! 空闲时自动发起 agent_loop 回合(用合成消息告知模型后台任务完成了什么)。
//!
//! 核心矛盾(交接报告已述):Notify 没人监听就是打进虚空。Supervisor 让"完成"能唤醒循环。
//! 前台有回合在跑 → 共享 tokio Mutex 天然让位,等当前回合结束才轮到。

use std::sync::Arc;

use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

use crate::agent::{agent_loop, AgentConfig, AgentEvent, EventSink};
use crate::cmds::SharedState;
use crate::tasks::{TaskManager, TaskRecord, TaskState};

/// Supervisor 的句柄,可取消。
pub struct SupervisorHandle {
    cancel: CancellationToken,
    _join: tokio::task::JoinHandle<()>,
}

impl SupervisorHandle {
    /// 启动 Supervisor。持有 TaskManager + AppState 共享锁 + AppHandle(抛事件给前端)。
    pub fn spawn(task_mgr: Arc<TaskManager>, state: SharedState, app: AppHandle) -> Self {
        let cancel = CancellationToken::new();
        let cancel_inner = cancel.clone();
        let join = tokio::spawn(async move {
            supervisor_loop(task_mgr, state, app, cancel_inner).await;
        });
        SupervisorHandle { cancel, _join: join }
    }

    /// 取消 Supervisor(断开连接 / app 退出时调)。
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

/// Supervisor 事件抛给前端的 Sink(和 TauriSink 类似,但独立,避免 cmds 的依赖循环)。
struct SupervisorSink {
    app: AppHandle,
}

#[async_trait::async_trait]
impl EventSink for SupervisorSink {
    async fn emit(&self, event: AgentEvent) {
        match event {
            AgentEvent::Content(s) => {
                let _ = self.app.emit("chat-chunk", serde_json::json!({ "delta": s, "done": false, "kind": "supervisor" }));
            }
            AgentEvent::Notice(s) => {
                let _ = self.app.emit("chat-chunk", serde_json::json!({ "delta": format!("\n> {s}\n"), "done": false, "kind": "supervisor" }));
            }
            AgentEvent::ToolStart { name, args: _ } => {
                let _ = self.app.emit("chat-chunk", serde_json::json!({ "delta": format!("\n> [Supervisor] 正在执行 {name}...\n"), "done": false, "kind": "supervisor" }));
            }
            AgentEvent::ToolEnd { name, ok, .. } => {
                let status = if ok { "完成" } else { "失败" };
                let _ = self.app.emit("chat-chunk", serde_json::json!({ "delta": format!("\n> [Supervisor] {name} {status}\n"), "done": false, "kind": "supervisor" }));
            }
            _ => {}
        }
    }
}

/// Supervisor 核心循环:等任务完成 → 合成消息 → 跑 agent_loop。
async fn supervisor_loop(
    task_mgr: Arc<TaskManager>,
    state: SharedState,
    app: AppHandle,
    cancel: CancellationToken,
) {
    loop {
        // 等"任意任务完成"事件,或收到取消信号。
        tokio::select! {
            _ = cancel.cancelled() => {
                break;
            }
            _ = task_mgr.wait_event() => {
                // 取走已完成的任务。
                let finished = task_mgr.drain_finished();
                if finished.is_empty() {
                    continue; // 可能是 reap_hung 触发的 notify,但已 drain 过了。
                }

                // 对外显示(2×2 第四格):每个完成/失败的后台任务弹一条 toast(前端按 ui_lang 渲染)。
                // 放在连接判断之前 → 不管连没连上都让用户看到结果。对内感知由下面的 Supervisor 回合负责。
                for r in &finished {
                    let code = if r.state == TaskState::Done { "task.completed" } else { "task.failed" };
                    crate::notify::emit_notice(&app, code, serde_json::json!({ "label": r.label }));
                }

                // 合成消息:告诉模型哪些后台任务完成了。
                let msg = synthesize_completion_message(&finished);

                // 通知前端。
                let _ = app.emit("supervisor-event", serde_json::json!({
                    "type": "task_completed",
                    "message": &msg,
                    "task_count": finished.len(),
                }));

                // 抢锁跑 agent_loop。前台有回合在跑时,这里会等(共享 Mutex 天然让位)。
                let mut guard = state.lock().await;
                let st = &mut *guard;

                // 前置条件:必须已连接 LLM。
                let Some(llm) = st.llm.clone() else { continue; };
                let Some(bridge) = st.bridge.clone() else { continue; };

                let full_prompt = format!("{}\n\n{}", st.base_system_prompt, st.project_context());
                let cfg = AgentConfig {
                    model: st.settings.model.clone(),
                    max_tokens: st.settings.max_tokens,
                    max_turns: st.settings.max_turns.min(4), // Supervisor 回合限制轮数,防失控。
                    parallel_max: st.settings.parallel_max as usize,
                    system_prompt: full_prompt,
                    prompt_lang: st.settings.lang.clone(),
                    auto_mode: st.settings.auto_mode,
                    danger_mode: st.settings.danger_mode, // 跟随设置(danger 是全局模式)
                    privacy_dirs: st.settings.privacy_dirs.clone(),
                    max_token_retries: st.settings.agent_max_token_retries as usize,
                    token_ceil: st.settings.agent_token_ceil,
                    silence_secs: st.settings.agent_silence_secs as u64,
                    max_stall: st.settings.agent_max_stall as usize,
                    reasoning_effort: st.settings.reasoning_effort.clone(),
                    branch_log_max_gb: st.settings.branch_log_max_gb,
                    self_verify: false, // Supervisor 后台回合:不自检(省 token,且非用户面结论)
                    self_verify_min_tools: st.settings.self_verify_min_tools as usize,
                    recall_in_loop: st.settings.recall_in_loop, // 后台回合也按用户设置补检索(轮数已限 4,成本有界)
                    // 工具记忆:后台 Supervisor 回合也按用户设置会诊(同样别犯第二遍)。
                    tool_memory_enabled: st.settings.tool_memory_enabled,
                    tool_memory_veto_threshold: st.settings.tool_memory_veto_threshold,
                    tool_memory_warn_threshold: st.settings.tool_memory_warn_threshold,
                };
                st.sandbox.set_danger(cfg.danger_mode); // 与 cfg 同步(judge 据此放行)
                let work_dir = st.work_dir.clone();
                let sink = SupervisorSink { app: app.clone() };

                // 跑 agent_loop(Supervisor 回合:模型看到后台任务完成了什么,决定后续)。
                let _outcome = agent_loop(
                    &msg,
                    &cfg,
                    llm.as_ref(),
                    &st.registry,
                    &st.sandbox,
                    &mut st.memory,
                    bridge.as_ref(),
                    bridge.as_ref(),
                    &st.flywheel,
                    &work_dir,
                    &sink,
                )
                .await;

                // Supervisor 回合结束,通知前端。
                let _ = app.emit("supervisor-event", serde_json::json!({
                    "type": "round_complete",
                    "task_count": finished.len(),
                }));
            }
        }
    }
}

/// 把已完成的任务列表合成一条给模型的消息。
fn synthesize_completion_message(records: &[TaskRecord]) -> String {
    let mut lines: Vec<String> = vec!["以下后台任务已完成,请查看结果并决定后续操作:".into()];
    for r in records {
        let state = match r.state {
            TaskState::Done => "成功",
            TaskState::Failed => "失败",
            TaskState::Running => unreachable!(),
        };
        let output = if r.output.is_empty() {
            String::new()
        } else {
            let tail: String = r.output.chars().take(300).collect();
            format!("\n  输出: {tail}")
        };
        lines.push(format!("  - 任务 {} [{}] {} ({:.1}s){}", r.id, state, r.label, r.elapsed_ms as f64 / 1000.0, output));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesize_message_format() {
        let records = vec![
            TaskRecord {
                id: "t1".into(),
                label: "构建项目".into(),
                command: "cargo build".into(),
                state: TaskState::Done,
                output: "Build successful".into(),
                elapsed_ms: 5200,
            },
            TaskRecord {
                id: "t2".into(),
                label: "跑测试".into(),
                command: "cargo test".into(),
                state: TaskState::Failed,
                output: "2 tests failed".into(),
                elapsed_ms: 3100,
            },
        ];
        let msg = synthesize_completion_message(&records);
        assert!(msg.contains("t1"));
        assert!(msg.contains("构建项目"));
        assert!(msg.contains("成功"));
        assert!(msg.contains("t2"));
        assert!(msg.contains("失败"));
        assert!(msg.contains("5.2s"));
    }
}
