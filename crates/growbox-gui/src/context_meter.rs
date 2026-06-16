//! 实时上下文压力计 —— 主链最近一次 LLM 请求实际发出的上下文 token 数。
//!
//! 来源 = 模型亲口回报的 `usage.prompt_tokens`(开 `stream_options.include_usage`,见 llm StreamChunk::Usage),
//! 非本地估算。面板"实时上下文压力"= 此值 / 上下文窗口总量(Settings.context_window_tokens,可设)。
//!
//! ★为何独立 managed state(不放 AppState)★:与 `ChatControl`/`UiAckRegistry` 同构 —— `run_chat` 全程持
//! AppState 锁跑脊柱,TauriSink 在持锁期间要写入本值,故走独立原子(不触 AppState 锁,无死锁)。
//! `get_status` 也取一份独立读出,免受回合锁阻塞。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

/// 最近一次主链请求的上下文 token 数(共享原子;0 = 尚无请求)。
#[derive(Default, Clone)]
pub struct ContextMeter {
    prompt_tokens: Arc<AtomicU32>,
}

impl ContextMeter {
    /// 记下本次请求的上下文 token(drive_one 收到 Usage 片、且主链时调)。
    pub fn set(&self, prompt_tokens: u32) {
        self.prompt_tokens.store(prompt_tokens, Ordering::Relaxed);
    }

    /// 读出最近一次值(get_status 回显;0 = 还没发过请求)。
    pub fn get(&self) -> u32 {
        self.prompt_tokens.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_then_get_roundtrips() {
        let m = ContextMeter::default();
        assert_eq!(m.get(), 0);
        m.set(12345);
        assert_eq!(m.get(), 12345);
    }

    #[test]
    fn clones_share_the_same_value() {
        let a = ContextMeter::default();
        let b = a.clone();
        a.set(999);
        assert_eq!(b.get(), 999, "克隆共享同一原子(managed state 取一份、sink 取一份)");
    }
}
