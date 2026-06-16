//! 交互式终端的 AI 共驾工具:pty_send(接管敲字)/ pty_peek(主动看屏)/ pty_close(收尾)。
//!
//! 配合 shell(interactive:true) 开的会话(见 pty.rs)。安全模型 = **用户在 xterm 全程可见 + 可随时关闭**:
//! 会话命令本身开会话时已过 shell_gate;pty_send 的键入是共驾的一部分,以"用户在看"为透明边界
//! (不再逐键过安全门——键入是流式/含控制字符,无法可靠解析)。这些工具直接操作 pty 进程级注册表。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult};

/// 默认 peek 返回的最大字节(够看清当前屏 + 不灌爆上下文)。
const DEFAULT_PEEK: usize = 4096;

fn session_id(args: &serde_json::Value) -> Option<String> {
    args.get("session_id").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// pty_send:往交互终端写入(AI 接管,如登录成功后自己敲后续命令)。
pub struct PtySend;

#[async_trait::async_trait]
impl Executor for PtySend {
    fn name(&self) -> &str {
        "pty_send"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(),
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "" },
                    "data": { "type": "string", "description": "" }
                },
                "required": ["session_id", "data"]
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Reversible
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let Some(id) = session_id(&ctx.args) else {
            return ToolResult::fail("pty_send 需要 session_id");
        };
        let Some(data) = ctx.args.get("data").and_then(|v| v.as_str()) else {
            return ToolResult::fail("pty_send 需要 data(要敲进终端的内容;命令记得带换行 \\n 才会执行)");
        };
        if crate::pty::send(&id, data) {
            ToolResult::ok(format!(
                "已向终端「{id}」输入(若是命令,确认带了换行才会执行)。可用 pty_peek 看反应。"
            ))
        } else {
            ToolResult::fail(format!("终端「{id}」不存在或已结束;无法输入。"))
        }
    }
}

/// pty_peek:主动读交互终端当前屏(C 混合:平时安静,AI 想看时拉一次)。
pub struct PtyPeek;

#[async_trait::async_trait]
impl Executor for PtyPeek {
    fn name(&self) -> &str {
        "pty_peek"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(),
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "" },
                    "max_bytes": { "type": "integer", "description": "" }
                },
                "required": ["session_id"]
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let Some(id) = session_id(&ctx.args) else {
            return ToolResult::fail("pty_peek 需要 session_id");
        };
        let max = ctx
            .args
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_PEEK);
        match crate::pty::peek(&id, max) {
            Some(screen) => {
                let alive = crate::pty::is_alive(&id);
                let state = if alive { "运行中" } else { "已结束" };
                ToolResult::ok(format!("[终端「{id}」· {state}· 当前屏尾]\n{screen}"))
            }
            None => ToolResult::fail(format!("终端「{id}」不存在或已结束。")),
        }
    }
}

/// pty_watch:进入/调整/退出"自适应轮询"看守模式(P3「看着我输入,有错误告诉我」)。
/// interval_secs>0 = 每隔这么久看一眼(即使屏没变也唤醒你);0 = 退出看守(回纯事件驱动)。
/// 你据所见自调间隔:接近你关注的内容/快出错时调小,平稳时调大,看完/不用了设 0。
pub struct PtyWatch;

#[async_trait::async_trait]
impl Executor for PtyWatch {
    fn name(&self) -> &str {
        "pty_watch"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(),
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "" },
                    "interval_secs": { "type": "integer", "description": "" }
                },
                "required": ["session_id", "interval_secs"]
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let Some(id) = session_id(&ctx.args) else {
            return ToolResult::fail("pty_watch 需要 session_id");
        };
        let secs = ctx.args.get("interval_secs").and_then(|v| v.as_u64()).unwrap_or(0);
        if !crate::pty::set_watch(&id, secs) {
            return ToolResult::fail(format!("终端「{id}」不存在或已结束。"));
        }
        if secs == 0 {
            ToolResult::ok(format!("已退出对终端「{id}」的看守(回到纯事件驱动:仅完成输入/输出时才通知你)。"))
        } else {
            ToolResult::ok(format!(
                "已进入看守:每约 {secs}s 看一眼终端「{id}」并通知你(即使屏没变)。据所见自调:\
                 接近关注内容/快出错就调小、平稳调大、看完设 0 退出。看守期不写主记忆(流式窗口)。"
            ))
        }
    }
}

/// pty_close:关闭交互终端会话(任务完成 / 不再需要时收尾)。
pub struct PtyClose;

#[async_trait::async_trait]
impl Executor for PtyClose {
    fn name(&self) -> &str {
        "pty_close"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(),
            params: serde_json::json!({
                "type": "object",
                "properties": { "session_id": { "type": "string", "description": "" } },
                "required": ["session_id"]
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Reversible
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let Some(id) = session_id(&ctx.args) else {
            return ToolResult::fail("pty_close 需要 session_id");
        };
        if crate::pty::close(&id) {
            ToolResult::ok(format!("已关闭终端「{id}」。"))
        } else {
            ToolResult::ok(format!("终端「{id}」已不存在(可能已结束),无需关闭。"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn ctx(v: serde_json::Value) -> ExecCtx<'static> {
        ExecCtx { args: v, work_dir: std::path::Path::new("."), limits: Default::default(), cancel: None }
    }

    #[tokio::test]
    async fn send_peek_close_roundtrip() {
        // 开一个 cat 会话(回显 stdin),用三工具走一遍。
        let id = crate::pty::open("cat", &std::env::temp_dir(), 80, 24, Arc::new(|_, _| {})).unwrap();
        std::thread::sleep(Duration::from_millis(120));

        let r = PtySend.execute(&mut ctx(serde_json::json!({"session_id": id, "data": "marco-42\n"}))).await;
        assert!(r.ok, "{}", r.content);
        std::thread::sleep(Duration::from_millis(200));

        let r = PtyPeek.execute(&mut ctx(serde_json::json!({"session_id": id}))).await;
        assert!(r.ok);
        assert!(r.content.contains("marco-42"), "peek 应见回显,实得: {}", r.content);

        let r = PtyClose.execute(&mut ctx(serde_json::json!({"session_id": id}))).await;
        assert!(r.ok);
        // 关后 peek 失败。
        let r = PtyPeek.execute(&mut ctx(serde_json::json!({"session_id": id}))).await;
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn missing_session_id_fails() {
        assert!(!PtySend.execute(&mut ctx(serde_json::json!({"data": "x"}))).await.ok);
        assert!(!PtyPeek.execute(&mut ctx(serde_json::json!({}))).await.ok);
        assert!(!PtyClose.execute(&mut ctx(serde_json::json!({}))).await.ok);
    }

    #[tokio::test]
    async fn send_to_unknown_session_fails() {
        let r = PtySend.execute(&mut ctx(serde_json::json!({"session_id": "term_nope", "data": "x"}))).await;
        assert!(!r.ok);
    }
}
