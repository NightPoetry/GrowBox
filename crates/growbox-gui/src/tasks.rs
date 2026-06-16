//! 后台任务管理 —— "完成事件主触发 + 指数退避兜底巡检"的基石。
//!
//! 设计要点(见本会话设计讨论):
//! - 模型 spawn 的后台任务异步跑,结束即 `notify` 唤醒——这是"任务返回来触发自己"的信号源。
//! - 可探测是设计出来的:每个任务带一个 `DoneWhen` 完成判据(进程退出 / 文件出现 / 端口通 / 探针命令返回 0)。
//!   不退出的服务靠"等进程结束"永远等不到,必须给探针;退避巡检查的就是它。
//! - 卡死兜底:跑过 deadline 还没完成 → 判 `Failed`,避免无限等。
//! - 不跨重启:纯内存,app 关了任务即没了(语义干净,见设计决策)。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::Notify;

/// 完成判据 —— 怎么算"做完了"。spawn 时必带,逼模型先想清"我怎么知道它结束"。
#[derive(Clone, Debug, PartialEq)]
pub enum DoneWhen {
    /// 进程退出即完成(终止型命令:构建、测试、脚本)。退出码 0=Done,非 0=Failed。
    Exit,
    /// 工作目录下出现该(相对/绝对)路径 → 完成(适合"产物文件落地"型)。
    FileExists(PathBuf),
    /// 本机该端口可连 → 完成(适合"起服务并就绪"型,进程会一直跑)。
    PortOpen(u16),
    /// 探针命令返回 0 → 完成(最通用:curl 健康检查、grep 日志……)。
    Probe(String),
}

/// 任务状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Done,
    Failed,
}

/// 对外快照(供事件/列表/喂回模型)。
/// `label`=LLM 给的 tag(在干什么),`command`=原始 shell 命令——两者落库,服务"对自身及衍生物绝对感知"
/// (`设计/05-工具系统` 推论6;对外状态栏只露计数,对内 list_tasks 给全细节)。
#[derive(Clone, Debug)]
pub struct TaskRecord {
    pub id: String,
    pub label: String,
    /// 原始 shell 命令(落库,LLM 经 list_tasks 可见)。
    pub command: String,
    pub state: TaskState,
    /// 终止型任务的输出尾巴 / 失败原因。
    pub output: String,
    pub elapsed_ms: u128,
}

struct Entry {
    label: String,
    command: String,
    state: TaskState,
    output: String,
    started: Instant,
    finished: Option<Instant>,
}

impl Entry {
    fn record(&self, id: &str) -> TaskRecord {
        let end = self.finished.unwrap_or_else(Instant::now);
        TaskRecord {
            id: id.to_string(),
            label: self.label.clone(),
            command: self.command.clone(),
            state: self.state,
            output: self.output.clone(),
            elapsed_ms: end.duration_since(self.started).as_millis(),
        }
    }
}

/// 后台任务管理器。`Arc` 共享:既给脊柱/工具登记,也给常驻 supervisor 监听完成。
pub struct TaskManager {
    inner: Mutex<HashMap<String, Entry>>,
    /// 任意任务结束就 notify_waiters —— supervisor / wait_tasks 的主触发。
    notify: Notify,
    seq: AtomicUsize,
    /// wait_tasks 指数退避旋钮(ms;推论9 可设)。运行时由 `set_backoff` 从 Settings 注入,
    /// WaitTasks 执行器经 `Arc<TaskManager>` 读取(原子,无锁)。
    backoff_base_ms: AtomicU64,
    backoff_cap_ms: AtomicU64,
    /// 后台任务输出尾巴保留上限(字节;推论9 可设)。由 `set_output_cap` 从 Settings 注入。
    output_cap: AtomicUsize,
}

/// 探针轮询间隔(非 Exit 判据)。
const PROBE_EVERY: Duration = Duration::from_millis(500);
// 输出尾巴保留上限已暴露为可设(推论9):由 `TaskManager.output_cap`(原子)持有,从 Settings 注入。

impl TaskManager {
    pub fn new() -> Arc<Self> {
        Arc::new(TaskManager {
            inner: Mutex::new(HashMap::new()),
            notify: Notify::new(),
            seq: AtomicUsize::new(0),
            backoff_base_ms: AtomicU64::new(2_000),
            backoff_cap_ms: AtomicU64::new(60_000),
            output_cap: AtomicUsize::new(4096),
        })
    }

    /// 设置 wait_tasks 退避旋钮(连接时 / set_misc_config 从 Settings 注入;推论9 数值全可设)。
    pub fn set_backoff(&self, base_ms: u64, cap_ms: u64) {
        self.backoff_base_ms.store(base_ms.max(1), Ordering::Relaxed);
        self.backoff_cap_ms.store(cap_ms.max(1), Ordering::Relaxed);
    }
    /// 设置后台任务输出尾巴保留上限(字节;连接时从 Settings 注入;推论9 数值全可设)。
    pub fn set_output_cap(&self, cap: usize) {
        self.output_cap.store(cap.max(1), Ordering::Relaxed);
    }
    /// 输出尾巴上限(`cap` 截断时读)。
    pub fn output_cap(&self) -> usize {
        self.output_cap.load(Ordering::Relaxed)
    }
    /// 退避基数 ms(WaitTasks 读)。
    pub fn backoff_base_ms(&self) -> u64 {
        self.backoff_base_ms.load(Ordering::Relaxed)
    }
    /// 退避上限 ms(WaitTasks 读)。
    pub fn backoff_cap_ms(&self) -> u64 {
        self.backoff_cap_ms.load(Ordering::Relaxed)
    }

    /// 当前在跑的任务数。
    pub fn running_count(&self) -> usize {
        self.inner.lock().values().filter(|e| e.state == TaskState::Running).count()
    }

    /// 等待"任意任务结束"事件。无在跑任务时也会阻塞,直到下一次完成 notify。
    pub async fn wait_event(&self) {
        self.notify.notified().await;
    }

    /// 取走所有已结束(Done/Failed)的记录,留下 Running 的。供"完成后喂回模型"。
    pub fn drain_finished(&self) -> Vec<TaskRecord> {
        let mut g = self.inner.lock();
        let done: Vec<String> = g
            .iter()
            .filter(|(_, e)| e.state != TaskState::Running)
            .map(|(id, _)| id.clone())
            .collect();
        done.iter().map(|id| g.remove(id).unwrap().record(id)).collect()
    }

    /// 卡死兜底:把 Running 超过 deadline 的标记 Failed,返回被判死的。退避巡检时调。
    pub fn reap_hung(&self, deadline: Duration) -> Vec<TaskRecord> {
        let now = Instant::now();
        let mut g = self.inner.lock();
        let mut reaped = Vec::new();
        for (id, e) in g.iter_mut() {
            if e.state == TaskState::Running && now.duration_since(e.started) >= deadline {
                e.state = TaskState::Failed;
                e.finished = Some(now);
                e.output = format!("超时未返回(>{}s),判定卡死", deadline.as_secs());
                reaped.push(e.record(id));
            }
        }
        reaped
    }

    /// 当前全部任务快照(含 Running),供 list_tasks。
    pub fn snapshot(&self) -> Vec<TaskRecord> {
        self.inner.lock().iter().map(|(id, e)| e.record(id)).collect()
    }

    /// 起一个后台 shell 任务,非阻塞,立刻返回 id。完成由 `done_when` 判定。
    /// 调用方需保证 command 已过安全门(后台不绕过沙箱)。
    pub fn spawn_shell(self: &Arc<Self>, label: impl Into<String>, command: String, work_dir: PathBuf, done_when: DoneWhen) -> String {
        let id = format!("t{}", self.seq.fetch_add(1, Ordering::Relaxed) + 1);
        self.inner.lock().insert(
            id.clone(),
            Entry { label: label.into(), command: command.clone(), state: TaskState::Running, output: String::new(), started: Instant::now(), finished: None },
        );
        let mgr = Arc::clone(self);
        let tid = id.clone();
        tokio::spawn(async move {
            let (state, output) = run_task(&command, &work_dir, &done_when).await;
            mgr.finish(&tid, state, output);
        });
        id
    }

    fn finish(&self, id: &str, state: TaskState, output: String) {
        {
            let mut g = self.inner.lock();
            if let Some(e) = g.get_mut(id) {
                // 已被 reap_hung 判死的就别覆盖了。
                if e.state == TaskState::Running {
                    e.state = state;
                    e.finished = Some(Instant::now());
                    e.output = cap(output, self.output_cap());
                }
            }
        }
        self.notify.notify_waiters();
    }
}

fn cap(mut s: String, cap_bytes: usize) -> String {
    if s.len() > cap_bytes {
        let tail = s.split_off(s.len() - cap_bytes);
        return format!("…(截断)\n{tail}");
    }
    s
}

/// 跑一个任务到它的完成判据满足。返回最终状态 + 输出。
async fn run_task(command: &str, work_dir: &PathBuf, done_when: &DoneWhen) -> (TaskState, String) {
    use tokio::process::Command;

    match done_when {
        // 终止型:等进程退出,退出码定成败。
        DoneWhen::Exit => {
            let out = Command::new("sh").arg("-c").arg(command).current_dir(work_dir).output().await;
            match out {
                Ok(o) => {
                    let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                    text.push_str(&String::from_utf8_lossy(&o.stderr));
                    if o.status.success() {
                        (TaskState::Done, text)
                    } else {
                        (TaskState::Failed, format!("退出码 {:?}\n{text}", o.status.code()))
                    }
                }
                Err(e) => (TaskState::Failed, format!("启动失败: {e}")),
            }
        }
        // 服务型:进程多半不退出,起它然后轮询探针判就绪。
        probe => {
            let mut child = match Command::new("sh").arg("-c").arg(command).current_dir(work_dir).spawn() {
                Ok(c) => c,
                Err(e) => return (TaskState::Failed, format!("启动失败: {e}")),
            };
            loop {
                if probe_ok(probe, work_dir).await {
                    return (TaskState::Done, format!("完成判据已满足: {probe:?}"));
                }
                // 进程在探针满足前就退了 → 多半是起服务失败。
                if let Ok(Some(status)) = child.try_wait() {
                    return (TaskState::Failed, format!("进程提前退出(状态 {status:?}),完成判据未满足"));
                }
                tokio::time::sleep(PROBE_EVERY).await;
            }
        }
    }
}

/// 探针是否满足。Exit 不会走到这里。
async fn probe_ok(done_when: &DoneWhen, work_dir: &PathBuf) -> bool {
    match done_when {
        DoneWhen::Exit => true,
        DoneWhen::FileExists(p) => {
            let path = if p.is_absolute() { p.clone() } else { work_dir.join(p) };
            path.exists()
        }
        DoneWhen::PortOpen(port) => {
            tokio::net::TcpStream::connect(("127.0.0.1", *port)).await.is_ok()
        }
        DoneWhen::Probe(cmd) => {
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .current_dir(work_dir)
                .output()
                .await
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn exit_task_succeeds_and_drains() {
        let mgr = TaskManager::new();
        let dir = tempdir().unwrap();
        let id = mgr.spawn_shell("echo", "echo hi".into(), dir.path().to_path_buf(), DoneWhen::Exit);
        assert_eq!(mgr.running_count(), 1);
        // 等完成事件。
        mgr.wait_event().await;
        let done = mgr.drain_finished();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].id, id);
        assert_eq!(done[0].state, TaskState::Done);
        assert!(done[0].output.contains("hi"));
        assert_eq!(mgr.running_count(), 0, "drain 后不再有在跑");
    }

    #[tokio::test]
    async fn exit_nonzero_is_failed() {
        let mgr = TaskManager::new();
        let dir = tempdir().unwrap();
        mgr.spawn_shell("false", "exit 3".into(), dir.path().to_path_buf(), DoneWhen::Exit);
        mgr.wait_event().await;
        let done = mgr.drain_finished();
        assert_eq!(done[0].state, TaskState::Failed);
        assert!(done[0].output.contains("3"));
    }

    #[tokio::test]
    async fn file_probe_completes_when_file_appears() {
        let mgr = TaskManager::new();
        let dir = tempdir().unwrap();
        // 命令本身立刻退出,但判据是 ready.txt 出现;命令负责造出它。
        mgr.spawn_shell(
            "make file",
            "sleep 0.2 && touch ready.txt && sleep 5".into(),
            dir.path().to_path_buf(),
            DoneWhen::FileExists("ready.txt".into()),
        );
        mgr.wait_event().await;
        let done = mgr.drain_finished();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].state, TaskState::Done);
    }

    #[tokio::test]
    async fn reap_hung_marks_overdue_failed() {
        let mgr = TaskManager::new();
        let dir = tempdir().unwrap();
        mgr.spawn_shell("slow", "sleep 30".into(), dir.path().to_path_buf(), DoneWhen::Exit);
        // deadline=0 → 立刻判死。
        let reaped = mgr.reap_hung(Duration::from_secs(0));
        assert_eq!(reaped.len(), 1);
        assert_eq!(reaped[0].state, TaskState::Failed);
        assert!(reaped[0].output.contains("卡死"));
        // 之后 drain 能取到这条死的。
        assert_eq!(mgr.drain_finished().len(), 1);
    }

    #[tokio::test]
    async fn drain_keeps_running_tasks() {
        let mgr = TaskManager::new();
        let dir = tempdir().unwrap();
        mgr.spawn_shell("long", "sleep 30".into(), dir.path().to_path_buf(), DoneWhen::Exit);
        assert!(mgr.drain_finished().is_empty(), "在跑的不该被 drain");
        assert_eq!(mgr.running_count(), 1);
    }
}
