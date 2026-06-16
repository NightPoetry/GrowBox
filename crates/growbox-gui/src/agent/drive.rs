//! 单次 LLM 调用的流式驱动:reasoning/content 实时抛出,工具增量按 index 拼齐,归一化成 `DriveOutcome`。

use std::time::Duration;

use growbox_core::ToolCall;
use growbox_llm::{ChatRequest, StreamChunk, ToolCallAccumulator};

use crate::bridge::LlmDriver;

use super::{AgentEvent, EventSink};

/// 一次 LLM 调用的归一化结果。
pub(super) struct DriveOutcome {
    pub(super) content: String,
    pub(super) tool_calls: Vec<ToolCall>,
    /// 本轮思维链全文(thinking 模式)。两用:① 带 tool_calls 时必须回传给 API(否则 400);
    /// ② 循环判"这轮在思考"给思考免死 + 算退化重复指纹。
    pub(super) reasoning: String,
    /// finish_reason == "length" → 模型被截断,需加 token 重试。
    pub(super) truncated: bool,
}

/// 流式跑完一次请求:reasoning/content 实时抛出,工具增量按 index 拼齐。
///
/// `user_visible`:本轮思考/正文是否展示给用户(栈函数 v2 原则9:**分支链不与用户对话**,
/// 展示的思考只有主链)。false(在派生分支内)= 仍累积 reasoning/content 供脊柱用,但**不向用户 emit**
/// (分支复杂性内部包装;其原始细节另入分支日志,见 07 P-v2.4)。
pub(super) async fn drive_one(
    llm: &dyn LlmDriver,
    req: ChatRequest,
    sink: &dyn EventSink,
    silence_secs: u64,
    user_visible: bool,
) -> Result<DriveOutcome, String> {
    let mut rx = llm.chat_stream(req).await.map_err(|e| e.to_string())?;
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut acc = ToolCallAccumulator::new();
    let mut finish = String::new();

    // 取消轮询间隔:把"等下一个 chunk"切成 ≤1s 的小步,每步重查取消标志。
    // 这样即使流**真 stalled**(模型卡住、长时间无任何 chunk),点「终止」也能在 ≤1s 内生效——
    // 而不是卡在一整个 silence_secs 的 recv 等待里出不来(真机实测:pro 长思考 stalled 时点终止 200s+ 不停)。
    // 活跃流(chunk 持续到)仍每个 chunk 回环命中取消,行为不变。
    let poll = Duration::from_secs(1).min(Duration::from_secs(silence_secs.max(1)));
    let mut silent = Duration::ZERO;
    loop {
        // ★终止响应★:用户按「终止」后,流式途中(哪怕模型还在长思考、chunk 持续流)也立刻收口,
        // 不必等当前 LLM 调用整段跑完——否则长 reasoning 期间点终止要等几分钟才生效,体感"没用"。
        if sink.is_cancelled() {
            break;
        }
        // 沉默超时:任何 chunk(含 reasoning)都重置等待。按 poll 间隔分片等待,以便频繁重查取消。
        let chunk = match tokio::time::timeout(poll, rx.recv()).await {
            Err(_) => {
                // 本 poll 片无 chunk:累计静默,达 silence_secs 才真判超时;否则回环重查取消。
                silent += poll;
                if silent >= Duration::from_secs(silence_secs) {
                    return Err("LLM 响应沉默超时".into());
                }
                continue;
            }
            Ok(None) => break,
            Ok(Some(Err(e))) => return Err(e.to_string()),
            Ok(Some(Ok(c))) => {
                silent = Duration::ZERO; // 收到 chunk:静默计时归零(任何 chunk 含 reasoning 都重置)。
                c
            }
        };
        match chunk {
            StreamChunk::Reasoning(r) => {
                reasoning.push_str(&r);
                if user_visible {
                    sink.emit(AgentEvent::Reasoning(r)).await; // 分支内不向用户展示思考(只主链)。
                }
            }
            StreamChunk::Content(c) => {
                content.push_str(&c);
                if user_visible {
                    sink.emit(AgentEvent::Content(c)).await; // 分支内不与用户对话。
                }
            }
            StreamChunk::ToolCallDelta { index, id, name, args_fragment } => {
                acc.push(index, id, name, &args_fragment);
            }
            StreamChunk::Done { finish_reason } => finish = finish_reason,
            StreamChunk::Usage { prompt_tokens } => {
                // 实时上下文压力:只主链(分支上下文是隔离切片,不代表用户对话的真实占用)。
                if user_visible {
                    sink.note_context_tokens(prompt_tokens);
                }
            }
        }
    }

    let tool_calls = acc.finish();
    // ★配对不变式(Bug B 兜底)★:finish() 已确保每个 tool_call 有非空 id(空 id 回传会被 DeepSeek 400
    // 拒为 "insufficient tool messages following tool_calls")。这里只在 debug/测试断言,不在 release 改动
    // 数组(改动 = 破坏 byte-stable 前缀缓存),纯防回归。
    debug_assert!(
        tool_calls.iter().all(|tc| !tc.id.is_empty()),
        "tool_call id 不得为空(否则 tool_result 无法配对 → DeepSeek 400)"
    );
    // finish_reason == "length" 就是截断——不管是工具参残缺还是回复写一半,都应该加 token 重试。
    let truncated = finish == "length";
    Ok(DriveOutcome { content, tool_calls, reasoning, truncated })
}
