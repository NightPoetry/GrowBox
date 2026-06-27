//! 交互式终端会话 —— PTY 进程级注册表 + 人机共驾原语。
//!
//! 设计(对齐造物共驾模式,surface 从 HTML canvas 换成 PTY 终端):
//! - 一个会话 = 一个真 PTY 里跑的命令(如 `ssh user@host`)。
//! - 输出流:reader 线程把 PTY 输出① 实时回调出去(前端 xterm 显示)② 追加进有界缓冲(供 `pty_peek` 取尾 +
//!   唤醒 seed 取最近输出)。
//! - 输入:用户在 xterm 直接敲(经 `pty_input` 命令)/ AI 接管经 `pty_send` 工具 —— 两者都写同一个 writer。
//! - 事件点(Phase 2 唤醒用):输出静默或匹配关键模式时惊动 AI 来判断(连接成功/失败 → 提示用户重试 / 接管)。
//!
//! 本模块是纯引擎,**不依赖 Tauri**(输出经注入的回调外发),可独立单测。Tauri 层注入真 emit,
//! 测试注入收集器。会话存进程级单例注册表(对齐 transpile.rs 单例先例),AI 工具与命令按 id 操作。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use parking_lot::Mutex;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

/// 缓冲上限(字节):只留最近输出供 peek/唤醒,防 ssh/编译刷屏撑爆内存。
const BUFFER_CAP: usize = 64 * 1024;

/// 一个活的终端会话。
struct PtySession {
    /// 写入端:用户键入 / AI pty_send 都往这里写(同一终端,共驾)。
    writer: Box<dyn Write + Send>,
    /// 近期输出环形缓冲(有界,供 pty_peek 取尾 + 唤醒 seed)。
    buffer: Arc<Mutex<String>>,
    /// 主 PTY(持有保活;close 时连同 session 一起 drop = 关闭)。
    _master: Box<dyn portable_pty::MasterPty + Send>,
    /// 子进程(close 时 kill)。
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// 命令原文(展示/日志)。
    command: String,
    /// 进程是否已退出(reader 读到 EOF 置位)。
    exited: Arc<AtomicBool>,
    /// ★自适应轮询(P3「看着我输入纠错」)★:AI 设的"看一眼间隔"秒数;0=不轮询(纯事件驱动)。
    /// 前端每次 terminal_event 后读它排下一次强制唤醒;AI 据所见自调(近关注内容缩短、否则拉长、设 0 停)。
    watch_secs: Arc<AtomicU64>,
}

type Registry = Mutex<HashMap<String, PtySession>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_id() -> String {
    static N: AtomicU64 = AtomicU64::new(1);
    format!("term_{}", N.fetch_add(1, Ordering::Relaxed))
}

/// 一次输出回调:(session_id, chunk)。Tauri 层据此 emit 给前端 xterm;测试注入收集器。
pub type OutputSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// 开一个交互终端会话:在 PTY 里跑 `command`(经 sh -c),返回 session id。
/// `on_output` 每收到一段 PTY 输出就被调用(用于前端显示);同一段也进有界缓冲(peek 用)。
pub fn open(command: &str, work_dir: &std::path::Path, cols: u16, rows: u16, on_output: OutputSink) -> std::io::Result<String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let mut cmd = CommandBuilder::new("sh");
    cmd.arg("-c");
    cmd.arg(command);
    cmd.cwd(work_dir);
    let child = pair.slave.spawn_command(cmd).map_err(|e| std::io::Error::other(e.to_string()))?;
    // 关掉本进程持有的 slave 端:子进程退出后 master 才会读到 EOF。
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| std::io::Error::other(e.to_string()))?;
    let writer = pair.master.take_writer().map_err(|e| std::io::Error::other(e.to_string()))?;

    let id = next_id();
    let buffer = Arc::new(Mutex::new(String::new()));
    let exited = Arc::new(AtomicBool::new(false));

    // reader 线程:PTY 输出是阻塞 std::io::Read,用独立 OS 线程读,边读边回调 + 入缓冲。
    {
        let id = id.clone();
        let buffer = buffer.clone();
        let exited = exited.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,                       // EOF:进程结束
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]).to_string();
                        append_capped(&buffer, &chunk);
                        on_output(&id, &chunk);
                    }
                    Err(_) => break,
                }
            }
            exited.store(true, Ordering::Relaxed);
            on_output(&id, "\n[会话结束]\n");
        });
    }

    registry().lock().insert(
        id.clone(),
        PtySession {
            writer,
            buffer,
            _master: pair.master,
            child,
            command: command.to_string(),
            exited,
            watch_secs: Arc::new(AtomicU64::new(0)),
        },
    );
    Ok(id)
}

/// 设置会话的自适应轮询间隔(秒;0=停)。AI 经 pty_watch 调。返回是否找到会话。
pub fn set_watch(id: &str, secs: u64) -> bool {
    let reg = registry().lock();
    match reg.get(id) {
        Some(s) => {
            s.watch_secs.store(secs, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// 读会话当前轮询间隔(秒;0=不轮询)。前端每次唤醒后据此排下一拍;会话不存在=0。
pub fn watch_interval(id: &str) -> u64 {
    registry().lock().get(id).map(|s| s.watch_secs.load(Ordering::Relaxed)).unwrap_or(0)
}

/// 往会话写入(用户键入 / AI 接管),data 原样送进 PTY stdin。返回是否找到会话。
pub fn send(id: &str, data: &str) -> bool {
    let mut reg = registry().lock();
    let Some(s) = reg.get_mut(id) else { return false };
    let _ = s.writer.write_all(data.as_bytes());
    let _ = s.writer.flush();
    true
}

/// 取会话近期输出尾部(最多 max 字节),供 AI 主动 peek 或唤醒 seed 取最近。
pub fn peek(id: &str, max: usize) -> Option<String> {
    let reg = registry().lock();
    let s = reg.get(id)?;
    let buf = s.buffer.lock();
    Some(tail(&buf, max))
}

/// 关闭会话:kill 子进程 + 从注册表移除(连同 master drop = 关 PTY)。返回是否找到。
pub fn close(id: &str) -> bool {
    let mut reg = registry().lock();
    match reg.remove(id) {
        Some(mut s) => {
            let _ = s.child.kill();
            true
        }
        None => false,
    }
}

/// 会话是否存在且进程未退出。
pub fn is_alive(id: &str) -> bool {
    let reg = registry().lock();
    reg.get(id).map(|s| !s.exited.load(Ordering::Relaxed)).unwrap_or(false)
}

/// 会话命令原文(展示用)。
pub fn command_of(id: &str) -> Option<String> {
    registry().lock().get(id).map(|s| s.command.clone())
}

fn append_capped(buffer: &Arc<Mutex<String>>, chunk: &str) {
    let mut b = buffer.lock();
    b.push_str(chunk);
    if b.len() > BUFFER_CAP {
        let cut = b.len() - BUFFER_CAP;
        // 按字符边界裁剪,避免切坏 UTF-8。
        let start = (cut..b.len()).find(|i| b.is_char_boundary(*i)).unwrap_or(b.len());
        *b = b[start..].to_string();
    }
}

fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let cut = s.len() - max;
    let start = (cut..s.len()).find(|i| s.is_char_boundary(*i)).unwrap_or(s.len());
    s[start..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn noop_sink() -> OutputSink {
        Arc::new(|_id: &str, _chunk: &str| {})
    }

    #[test]
    fn open_echo_streams_output_and_peek() {
        let collected = Arc::new(Mutex::new(String::new()));
        let c2 = collected.clone();
        let sink: OutputSink = Arc::new(move |_id, chunk| c2.lock().push_str(chunk));
        let dir = std::env::temp_dir();
        let id = open("echo hello-pty", &dir, 80, 24, sink).unwrap();
        // 给 reader 线程时间收输出 + 进程退出。
        std::thread::sleep(Duration::from_millis(400));
        let peeked = peek(&id, 4096).unwrap_or_default();
        assert!(peeked.contains("hello-pty"), "peek 应含输出,实得: {peeked:?}");
        assert!(collected.lock().contains("hello-pty"), "回调应收到输出");
        close(&id);
    }

    #[test]
    fn send_feeds_stdin_back_to_output() {
        // cat 把 stdin 原样回显到 stdout;send 的内容应出现在缓冲里。
        let id = open("cat", &std::env::temp_dir(), 80, 24, noop_sink()).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert!(send(&id, "ping-1234\n"));
        std::thread::sleep(Duration::from_millis(250));
        let peeked = peek(&id, 4096).unwrap_or_default();
        assert!(peeked.contains("ping-1234"), "cat 应回显 send 的输入,实得: {peeked:?}");
        close(&id);
    }

    #[test]
    fn close_removes_session() {
        let id = open("cat", &std::env::temp_dir(), 80, 24, noop_sink()).unwrap();
        assert!(peek(&id, 16).is_some());
        assert!(close(&id));
        assert!(peek(&id, 16).is_none(), "close 后会话应不存在");
        assert!(!send(&id, "x"), "close 后 send 应返回 false");
        assert!(!close(&id), "重复 close 返回 false");
    }

    #[test]
    fn tail_respects_char_boundary() {
        let s = "中文测试abc";
        let t = tail(s, 5); // 5 字节 = 不足以放下整段,从字符边界起
        assert!(s.ends_with(&t));
        assert!(std::str::from_utf8(t.as_bytes()).is_ok());
    }
}
