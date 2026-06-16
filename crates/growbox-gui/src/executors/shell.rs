//! shell 执行器 —— 在项目根跑命令,捕获输出。
//!
//! 危险命令/越界路径由 safety 的 `judge_shell` 在 dispatch 时拦下;这里只负责执行已放行的命令。
//!
//! ★为何 tokio + 进程组 + 超时 + 取消(2026-06-09 修真机 445s 挂死 + 终止失效)★:
//! 旧版用阻塞 `std::process::Command::output()`,它把 stdout/stderr 读到 EOF 才返回。命令里若有
//! `cmd &`(起常驻服务),那个后台子进程**继承并攥住管道写端** → EOF 永不到 → `.output()` 永久阻塞
//! (实测 445s 还在等);且阻塞调用塞在 async 里、取消标志只在 LLM 流式循环查,故"卡在工具执行里
//! 点不动终止"。现在:① tokio 异步 spawn,独立进程组(`process_group(0)`);② 并发排流(防管道写满
//! 反压阻住命令);③ 墙钟超时(可设)+ 回合级取消(`ExecCtx.cancel`)与命令完成三者竞速;④ 任一出口
//! 都 `killpg(-pgid, SIGKILL)` 杀整组(含 `&` 后台子进程)→ 既不挂死也不留僵尸,管道随之 EOF、收回
//! 已产出输出。持久服务请走后台任务 / 交互终端(PTY);shell 工具语义 = 跑一条会结束的命令并取其输出。

use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use growbox_core::{Claim, ExecCtx, Executor, Risk, ToolDef, ToolResult};
use tokio::io::AsyncReadExt;

pub struct Shell;

/// 命令的收场方式。
enum Outcome {
    /// 正常退出(带退出码)。
    Done(i32),
    /// 被信号终止(无退出码)。
    Signalled,
    /// 墙钟超时(秒)被我们杀。
    TimedOut(u64),
    /// 用户「终止」被我们杀。
    Cancelled,
}

/// 读尽一个管道但只留前 `cap` 字节(超额仍继续读,以免写端反压阻住命令)。返回 (内容, 是否截断)。
async fn drain_capped<R: AsyncReadExt + Unpin>(mut r: R, cap: usize) -> (Vec<u8>, bool) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if buf.len() < cap {
                    let take = (cap - buf.len()).min(n);
                    buf.extend_from_slice(&chunk[..take]);
                    if take < n {
                        truncated = true;
                    }
                } else {
                    truncated = true;
                }
            }
        }
    }
    (buf, truncated)
}

/// 杀整进程组(`pid` = 进程组首领 = pgid)。SIGKILL 不可捕获,确保 `cmd &` 起的后台子进程也一并死。
/// 组已空(命令已自行退尽)时返回 ESRCH,无副作用。
#[cfg(unix)]
fn kill_group(pid: u32) {
    // SAFETY: 仅传整数给 killpg,无内存交互;失败(ESRCH 等)被忽略,无副作用。
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
}

#[async_trait::async_trait]
impl Executor for Shell {
    fn name(&self) -> &str {
        "shell"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入

            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "要执行的命令" },
                    "interactive": { "type": "boolean", "description": "" }
                },
                "required": ["command"]
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Reversible
    }
    fn claim(&self, args: &serde_json::Value, _work_dir: &Path) -> Option<Claim> {
        args.get("command").and_then(|v| v.as_str()).map(|c| Claim::Shell(c.to_string()))
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let Some(command) = ctx.args.get("command").and_then(|v| v.as_str()) else {
            return ToolResult::fail("缺少参数 command");
        };
        let command = command.to_string();
        let max_output = ctx.limits.max_output_bytes;
        let timeout_secs = ctx.limits.shell_timeout_secs;
        let cancel = ctx.cancel.clone();

        // 防御:cwd 不存在 → spawn 直接 os error 2("无法启动命令: No such file or directory")。
        // 能建就建,绝不把"目录没建好"这种基建问题甩给模型去绕(真机暴露,见 create_project/switch_project)。
        if !ctx.work_dir.exists() {
            let _ = std::fs::create_dir_all(ctx.work_dir);
        }

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&command)
            .current_dir(ctx.work_dir)
            .stdin(Stdio::null()) // 不继承 stdin:防命令读输入时永久等待(又一类挂死)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // 独立进程组 → 超时/取消时能一锅端整组(含 `&` 后台子进程)。
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            // 命令起不来(spawn 失败)才是真正的工具执行失败。
            Err(e) => return ToolResult::fail(format!("无法启动命令: {e}")),
        };
        // 进程组首领 pid(= pgid);wait 回收后 id() 变 None,故先取。
        let pid = child.id();
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        // 并发排流:即时清空管道缓冲,命令产再多输出也不会因写端反压而卡住。
        let out_task = tokio::spawn(drain_capped(stdout, max_output));
        let err_task = tokio::spawn(drain_capped(stderr, max_output));

        // 命令完成 / 墙钟超时 / 用户取消 三者竞速(100ms 轮询取消标志与截止时刻)。
        let deadline = (timeout_secs > 0).then(|| Instant::now() + Duration::from_secs(timeout_secs));
        let outcome = {
            let wait = child.wait();
            tokio::pin!(wait);
            loop {
                tokio::select! {
                    r = &mut wait => break match r {
                        Ok(s) => s.code().map(Outcome::Done).unwrap_or(Outcome::Signalled),
                        Err(_) => Outcome::Signalled,
                    },
                    _ = tokio::time::sleep(Duration::from_millis(100)) => {
                        if cancel.as_ref().is_some_and(|f| f.load(Ordering::SeqCst)) {
                            break Outcome::Cancelled;
                        }
                        if let Some(d) = deadline {
                            if Instant::now() >= d {
                                break Outcome::TimedOut(timeout_secs);
                            }
                        }
                    }
                }
            }
        };

        // ★任一出口都杀整组★:清掉 `cmd &` 留下的后台子进程 + 让排流任务的管道 EOF(否则它们读不完)。
        // 命令已自行退尽时这是无害空操作(组已空 → ESRCH)。
        if let Some(p) = pid {
            #[cfg(unix)]
            kill_group(p);
            #[cfg(not(unix))]
            let _ = p;
        }
        #[cfg(not(unix))]
        let _ = child.start_kill();
        // 超时/取消是在 wait 完成前 break 的 → 补一次 wait 回收僵尸(unix 杀组后很快返回)。
        if matches!(outcome, Outcome::TimedOut(_) | Outcome::Cancelled) {
            let _ = child.wait().await;
        }
        let (out, out_tr) = out_task.await.unwrap_or_default();
        let (err, err_tr) = err_task.await.unwrap_or_default();

        // 组装输出体(stdout + [stderr] + 截断标注)。
        let mut body = String::new();
        body.push_str(&String::from_utf8_lossy(&out));
        if !err.is_empty() {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str("[stderr] ");
            body.push_str(&String::from_utf8_lossy(&err));
        }
        if out_tr || err_tr {
            body.push_str("\n...[输出已截断]");
        }

        match outcome {
            // ★退出码语义(2026-06-08)★:命令"跑通了"即 ok=true,退出码当**数据**写进 content 交主 LLM 判
            //(它有任务意图,如自己写的 `grep -c` 清楚 0 个匹配=断言通过),比机械"非零即失败"准。只有
            // spawn 失败(上面 Err 分支)/ 超时 / 用户取消才是工具层面的失败。
            Outcome::Done(code) => {
                let header = format!("退出码 {code}\n");
                let content = if code == 0 {
                    format!("{header}{body}")
                } else {
                    format!(
                        "{header}{body}\n[退出码非零:若属预期(如 grep 无匹配 / diff 有差异 / 断言为假)则正常,\
                         请据上方输出自行判断是否成功;若是真错误,据此纠正]"
                    )
                };
                ToolResult::ok(content)
            }
            Outcome::Signalled => ToolResult::ok(format!("退出码 -(命令被信号终止)\n{body}")),
            Outcome::TimedOut(secs) => ToolResult::fail(format!(
                "命令超时({secs}s)已被终止(连同其后台子进程)。常驻服务请用后台任务 / 交互终端,\
                 别用 shell 干等;或把后台进程输出重定向到文件并 disown。\n{body}"
            )),
            Outcome::Cancelled => {
                ToolResult::fail(format!("用户终止:命令已被杀(连同其后台子进程)。\n{body}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use growbox_core::ToolLimits;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use tempfile::tempdir;

    fn ctx_with<'a>(
        cmd: &str,
        dir: &'a Path,
        limits: ToolLimits,
        cancel: growbox_core::CancelFlag,
    ) -> ExecCtx<'a> {
        ExecCtx { args: serde_json::json!({ "command": cmd }), work_dir: dir, limits, cancel }
    }

    #[tokio::test]
    async fn echo_runs_in_workdir() {
        let dir = tempdir().unwrap();
        let mut ctx = ctx_with("echo hi", dir.path(), Default::default(), None);
        let r = Shell.execute(&mut ctx).await;
        assert!(r.ok, "{}", r.content);
        assert!(r.content.contains("hi"));
        assert!(r.content.contains("退出码 0"));
    }

    #[tokio::test]
    async fn nonzero_exit_is_ok_with_neutral_hint() {
        // ★退出码语义★:命令跑通即 ok=true(非零是数据交主 LLM 判),不机械判失败。
        let dir = tempdir().unwrap();
        let mut ctx = ctx_with("exit 3", dir.path(), Default::default(), None);
        let r = Shell.execute(&mut ctx).await;
        assert!(r.ok, "命令成功执行(虽非零退出)→ ok=true");
        assert!(r.content.contains("退出码 3"));
        assert!(r.content.contains("退出码非零"));
    }

    #[tokio::test]
    async fn grep_no_match_not_treated_as_failure() {
        let dir = tempdir().unwrap();
        let mut ctx =
            ctx_with("echo ok | grep -c 'NONEXISTENT'", dir.path(), Default::default(), None);
        let r = Shell.execute(&mut ctx).await;
        assert!(r.ok, "grep 无匹配(退出 1)= 断言通过,不得判失败");
        assert!(r.content.contains("退出码 1"));
    }

    #[tokio::test]
    async fn stderr_is_captured() {
        let dir = tempdir().unwrap();
        let mut ctx = ctx_with("echo oops 1>&2", dir.path(), Default::default(), None);
        let r = Shell.execute(&mut ctx).await;
        assert!(r.ok);
        assert!(r.content.contains("[stderr]") && r.content.contains("oops"));
    }

    #[tokio::test]
    async fn backgrounded_server_does_not_hang() {
        // ★核心回归(真机 445s 挂死)★:命令里起后台常驻进程(攥住管道),不得让工具卡死。
        // sh 跑完 echo 即退,我们杀整组干掉那个 sleep,管道 EOF → 立刻返回 "hi"。
        let dir = tempdir().unwrap();
        let mut ctx = ctx_with("sleep 30 & echo hi", dir.path(), Default::default(), None);
        let r = tokio::time::timeout(Duration::from_secs(5), Shell.execute(&mut ctx))
            .await
            .expect("后台子进程不得令 shell 工具挂死(应秒回,而非等 sleep 30)");
        assert!(r.ok, "{}", r.content);
        assert!(r.content.contains("hi"));
    }

    #[tokio::test]
    async fn foreground_hang_times_out_and_is_killed() {
        // 前台永不返回的命令(如起服务忘了 &)→ 到墙钟超时被杀整组,fail + 提示,而非无限等。
        let dir = tempdir().unwrap();
        let limits = ToolLimits { shell_timeout_secs: 1, ..Default::default() };
        let mut ctx = ctx_with("sleep 30", dir.path(), limits, None);
        let r = tokio::time::timeout(Duration::from_secs(5), Shell.execute(&mut ctx))
            .await
            .expect("超时应在 ~1s 生效,不得真等 30s");
        assert!(!r.ok, "超时被杀 = 工具失败");
        assert!(r.content.contains("超时"));
    }

    #[tokio::test]
    async fn cancel_kills_running_command() {
        // ★修"卡在工具里点不动终止"★:取消标志已置位 → 长命令被中途杀掉、立刻 fail 收口。
        let dir = tempdir().unwrap();
        let flag = Arc::new(AtomicBool::new(true)); // 预置=已请求终止
        let mut ctx = ctx_with("sleep 30", dir.path(), Default::default(), Some(flag));
        let r = tokio::time::timeout(Duration::from_secs(5), Shell.execute(&mut ctx))
            .await
            .expect("已取消的长命令应被中途杀掉而非等满 30s");
        assert!(!r.ok);
        assert!(r.content.contains("终止"));
    }

    #[tokio::test]
    async fn spawn_runs_true_ok() {
        // sh 总在,故以 `true` 验 ok 路径(spawn 失败极难稳定构造,语义见 execute 内注释:Err 分支才 fail)。
        let dir = tempdir().unwrap();
        let mut ctx = ctx_with("true", dir.path(), Default::default(), None);
        let r = Shell.execute(&mut ctx).await;
        assert!(r.ok);
    }

    #[test]
    fn claim_is_shell() {
        let c = Shell.claim(&serde_json::json!({"command":"ls"}), Path::new("/"));
        assert_eq!(c, Some(Claim::Shell("ls".into())));
    }
}
