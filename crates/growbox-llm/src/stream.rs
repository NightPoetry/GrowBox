//! SSE 流解析 —— 把 OpenAI 兼容的流式响应归一化为 `StreamChunk`。
//!
//! 关键(实测,见 `实验记录/00`):
//! - `delta.reasoning_content` 与 `delta.content` 分属不同字段。
//! - `delta.tool_calls[]` 分多个片段到达,按 `index` 累积 `arguments`。

use crate::types::StreamChunk;

/// 解析单行 SSE `data:` 负载,产出 0~N 个 StreamChunk。
///
/// 返回 `None` 表示该行无关(空行 / 非 data 行);`Some(vec![])` 表示有 data 但无可产出片段。
pub fn parse_sse_line(line: &str) -> Option<Vec<StreamChunk>> {
    let line = line.trim();
    let data = line.strip_prefix("data:")?.trim();
    if data == "[DONE]" {
        return Some(vec![]); // 终止由 finish_reason 负责,这里忽略
    }
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    let mut out = Vec::new();

    // 用量片(stream_options.include_usage):流末单独一片,choices 常为空,故在取 choice 之前处理,
    // 否则会被下面的 `?` 早退丢弃。prompt_tokens = 本次请求实际上下文 token(模型亲口算)。
    if let Some(pt) = v.get("usage").and_then(|u| u.get("prompt_tokens")).and_then(|x| x.as_u64()) {
        out.push(StreamChunk::Usage { prompt_tokens: pt as u32 });
    }

    let choice = match v.get("choices").and_then(|c| c.get(0)) {
        Some(c) => c,
        None => return Some(out), // 仅用量片 / 无 choice:产出已收集的(可能只含 Usage)。
    };

    if let Some(delta) = choice.get("delta") {
        if let Some(r) = delta.get("reasoning_content").and_then(|x| x.as_str()) {
            if !r.is_empty() {
                out.push(StreamChunk::Reasoning(r.to_string()));
            }
        }
        if let Some(c) = delta.get("content").and_then(|x| x.as_str()) {
            if !c.is_empty() {
                out.push(StreamChunk::Content(c.to_string()));
            }
        }
        if let Some(tcs) = delta.get("tool_calls").and_then(|x| x.as_array()) {
            for tc in tcs {
                let index = tc.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                let id = tc.get("id").and_then(|x| x.as_str()).map(String::from);
                let func = tc.get("function");
                let name = func
                    .and_then(|f| f.get("name"))
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty())
                    .map(String::from);
                let args_fragment = func
                    .and_then(|f| f.get("arguments"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(StreamChunk::ToolCallDelta { index, id, name, args_fragment });
            }
        }
    }

    if let Some(fr) = choice.get("finish_reason").and_then(|x| x.as_str()) {
        out.push(StreamChunk::Done { finish_reason: fr.to_string() });
    }

    Some(out)
}

/// 把一串 ToolCallDelta 聚合成完整工具调用(按 index)。
/// 供消费方收齐后调用。
#[derive(Default)]
pub struct ToolCallAccumulator {
    /// index -> (id, name, args 拼接)
    slots: std::collections::BTreeMap<u32, (String, String, String)>,
}

impl ToolCallAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, index: u32, id: Option<String>, name: Option<String>, args_fragment: &str) {
        let entry = self.slots.entry(index).or_default();
        if let Some(id) = id {
            entry.0 = id;
        }
        if let Some(name) = name {
            entry.1 = name;
        }
        entry.2.push_str(args_fragment);
    }

    /// 收齐后导出为 core::ToolCall 列表(按 index 升序)。
    ///
    /// ★id 完整性(修 Bug B:DeepSeek 400 "insufficient tool messages following tool_calls")★
    /// 实测 deepseek V4 推理模型并行 tool_call 偶发首片不带 id(或 id 落在丢失的分片里)→ slot 的 id 留空。
    /// 这条 tool_call 回传时序列化成 `"id": ""`(client.rs),assistant 声明了它、却没有任何 tool 消息能
    /// 用空 id 匹配上 → DeepSeek 拒整条请求。两道归一(都在此唯一关卡做,源头确定化,跨轮 byte-stable 不破缓存):
    ///   ① 丢弃完全空的幽灵 slot(args 续传分片落错 index 造出、却无 id 无 name 无实参)——它不是模型真发起的
    ///      调用,带回去只污染配对;整条丢掉则两侧都不存在、配对天然合法。
    ///   ② 余下 slot 若 id 为空,用 index 合成确定性 id(`gbtc_{index}`,前缀 deepseek 不用,避免与真 id 撞)。
    ///      同一 accumulator 既产 tool_call、其 id 又被脊柱复用去回填 tool_result → 两侧永远一致;
    ///      只依赖 index = 同一批流确定产出同一 id,不引入跨轮抖动(KV 前缀缓存安全)。
    pub fn finish(self) -> Vec<growbox_core::ToolCall> {
        self.slots
            .into_iter()
            .filter(|(_, (id, name, args))| {
                !(id.is_empty() && name.is_empty() && args.trim().is_empty())
            })
            .map(|(index, (mut id, name, arguments))| {
                if id.is_empty() {
                    id = format!("gbtc_{index}");
                }
                growbox_core::ToolCall { id, name, arguments }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reasoning_then_content() {
        let r = parse_sse_line(r#"data: {"choices":[{"delta":{"reasoning_content":"思考"}}]}"#);
        assert_eq!(r, Some(vec![StreamChunk::Reasoning("思考".into())]));
        let c = parse_sse_line(r#"data: {"choices":[{"delta":{"content":"答案"}}]}"#);
        assert_eq!(c, Some(vec![StreamChunk::Content("答案".into())]));
    }

    #[test]
    fn parses_finish_reason() {
        let r = parse_sse_line(r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#);
        assert_eq!(r, Some(vec![StreamChunk::Done { finish_reason: "stop".into() }]));
    }

    #[test]
    fn parses_usage_chunk_with_empty_choices() {
        // stream_options.include_usage 的末片:choices 为空,只有 usage。不能被早退丢弃。
        let r = parse_sse_line(r#"data: {"choices":[],"usage":{"prompt_tokens":12345,"completion_tokens":7}}"#);
        assert_eq!(r, Some(vec![StreamChunk::Usage { prompt_tokens: 12345 }]));
    }

    #[test]
    fn ignores_non_data_lines() {
        assert_eq!(parse_sse_line(""), None);
        assert_eq!(parse_sse_line(": keep-alive"), None);
    }

    #[test]
    fn done_marker_yields_empty() {
        assert_eq!(parse_sse_line("data: [DONE]"), Some(vec![]));
    }

    #[test]
    fn tool_call_delta_accumulates_by_index() {
        // 模拟流式分片:name 在首片,arguments 分多片(实测行为)。
        let mut acc = ToolCallAccumulator::new();
        acc.push(0, Some("call_1".into()), Some("file_read".into()), "{\"path\":");
        acc.push(0, None, None, "\"/tmp/a\"}");
        acc.push(1, Some("call_2".into()), Some("file_read".into()), "{\"path\":\"/tmp/b\"}");
        let calls = acc.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name, "file_read");
        assert_eq!(calls[0].arguments, "{\"path\":\"/tmp/a\"}");
        assert_eq!(calls[1].id, "call_2");
    }

    #[test]
    fn finish_backfills_missing_id_with_deterministic_index_id() {
        // ★Bug B 回归★:并行 tool_call 首片不带 id(实测 deepseek 偶发)→ id 留空。
        // finish() 必须用 index 兜底成非空 id,否则回传 `"id": ""` → DeepSeek 400。
        let mut acc = ToolCallAccumulator::new();
        // index 0 带 id,index 1 缺 id(只来了 name + args)。
        acc.push(0, Some("call_abc".into()), Some("file_read".into()), "{\"path\":\"/a\"}");
        acc.push(1, None, Some("shell".into()), "{\"command\":\"ls\"}");
        let calls = acc.finish();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "call_abc", "真 id 原样保留");
        assert_eq!(calls[1].id, "gbtc_1", "缺失 id 用 index 确定性兜底,非空");
        assert!(!calls[1].id.is_empty());
    }

    #[test]
    fn finish_drops_empty_phantom_slot() {
        // 幽灵 slot:args 续传分片落到一个从没收到 id/name 的 index(无实参)→ 不是真调用,丢弃。
        // 丢弃后两侧都不存在该调用,配对天然合法(不会出现声明了却没结果)。
        let mut acc = ToolCallAccumulator::new();
        acc.push(0, Some("call_1".into()), Some("file_read".into()), "{\"path\":\"/a\"}");
        acc.push(7, None, None, ""); // 纯幽灵
        let calls = acc.finish();
        assert_eq!(calls.len(), 1, "幽灵 slot 被丢弃");
        assert_eq!(calls[0].name, "file_read");
    }

    #[test]
    fn finish_keeps_named_call_with_empty_args() {
        // 空参 {}(token 被 reasoning 吃光截断)是真调用,必须带回(由截断重试单独处理),不能当幽灵丢。
        let mut acc = ToolCallAccumulator::new();
        acc.push(0, Some("call_1".into()), Some("file_read".into()), "");
        let calls = acc.finish();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].arguments, "");
    }

    #[test]
    fn parses_streaming_tool_call_fragment() {
        let r = parse_sse_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"file_read","arguments":"{\"p"}}]}}]}"#,
        );
        assert_eq!(
            r,
            Some(vec![StreamChunk::ToolCallDelta {
                index: 0,
                id: Some("c1".into()),
                name: Some("file_read".into()),
                args_fragment: "{\"p".into(),
            }])
        );
    }
}
