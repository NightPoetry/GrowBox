//! OpenAI 兼容客户端(DeepSeek)。流式 chat,归一化为 StreamChunk。

use futures::StreamExt;
use tokio::sync::mpsc;

use crate::error::{LlmError, LlmResult};
use crate::stream::parse_sse_line;
use crate::types::{ChatRequest, Role, StreamChunk};

/// 单提供商客户端。多槽位路由(main/subconscious/embedder)由 app 组合多个 client。
pub struct LlmClient {
    http: reqwest::Client,
    api_base: String,
    api_key: String,
}

impl LlmClient {
    pub fn new(api_base: impl Into<String>, api_key: impl Into<String>) -> Self {
        LlmClient {
            http: reqwest::Client::new(),
            api_base: api_base.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
        }
    }

    /// 流式对话。返回一个 channel,逐个产出 StreamChunk。
    /// 网络/解析在后台 task 进行,不阻塞调用方。
    pub async fn chat_stream(&self, req: ChatRequest) -> LlmResult<mpsc::Receiver<LlmResult<StreamChunk>>> {
        let body = build_body(&req, true);
        let resp = self
            .http
            .post(format!("{}/chat/completions", self.api_base))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::Api { status, body });
        }

        let (tx, rx) = mpsc::channel::<LlmResult<StreamChunk>>(64);
        tokio::spawn(async move {
            let mut byte_stream = resp.bytes_stream();
            let mut buf = String::new();
            while let Some(chunk) = byte_stream.next().await {
                match chunk {
                    Ok(bytes) => {
                        buf.push_str(&String::from_utf8_lossy(&bytes));
                        // 按行切分,保留最后不完整的一行在 buf 里
                        while let Some(nl) = buf.find('\n') {
                            let line: String = buf.drain(..=nl).collect();
                            if let Some(chunks) = parse_sse_line(&line) {
                                for c in chunks {
                                    if tx.send(Ok(c)).await.is_err() {
                                        return; // 消费方已丢弃
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(LlmError::Http(e))).await;
                        return;
                    }
                }
            }
            // 处理可能残留的最后一行
            if !buf.trim().is_empty() {
                if let Some(chunks) = parse_sse_line(&buf) {
                    for c in chunks {
                        let _ = tx.send(Ok(c)).await;
                    }
                }
            }
        });

        Ok(rx)
    }

    /// 拉取可用模型列表(`GET {api_base}/models`,OpenAI 兼容),返回模型 id(按字母序)。
    /// 错误用结构化前缀,前端 `Settings.tsx` 按前缀给提示:
    /// `API_UNREACHABLE:`(连不上)/ `API_AUTH_FAILED:`(401/403,key 缺失或无效)/
    /// `API_BAD_RESPONSE:`(其它非 2xx 或非 OpenAI 格式)。
    pub async fn list_models(&self) -> Result<Vec<String>, String> {
        let resp = self
            .http
            .get(format!("{}/models", self.api_base))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .map_err(|_| "API_UNREACHABLE:".to_string())?;
        // 401/403 = 认证问题(key 空/错),不是"格式不符"——单独区分,否则提示误导用户去查 URL。
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(format!("API_AUTH_FAILED:{}", status.as_u16()));
        }
        if !status.is_success() {
            return Err(format!("API_BAD_RESPONSE:{}", status.as_u16()));
        }
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|_| "API_BAD_RESPONSE:".to_string())?;
        let arr = body
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or("API_BAD_RESPONSE:")?;
        let mut ids: Vec<String> = arr
            .iter()
            .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from))
            .collect();
        ids.sort();
        Ok(ids)
    }
}

fn build_body(req: &ChatRequest, stream: bool) -> serde_json::Value {
    let messages: Vec<serde_json::Value> = req
        .messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            let mut obj = serde_json::json!({ "role": role, "content": m.content });
            if !m.tool_calls.is_empty() {
                obj["tool_calls"] = serde_json::json!(m
                    .tool_calls
                    .iter()
                    .map(|tc| serde_json::json!({
                        "id": tc.id,
                        "type": "function",
                        "function": { "name": tc.name, "arguments": tc.arguments }
                    }))
                    .collect::<Vec<_>>());
            }
            if let Some(id) = &m.tool_call_id {
                obj["tool_call_id"] = serde_json::json!(id);
            }
            // thinking 模式硬约束:带 tool_calls 的中间 assistant 必须回传 reasoning_content,否则 400。
            if let Some(rc) = &m.reasoning_content {
                obj["reasoning_content"] = serde_json::json!(rc);
            }
            obj
        })
        .collect();

    let mut body = serde_json::json!({
        "model": req.model,
        "messages": messages,
        "temperature": req.temperature,
        "stream": stream,
    });
    // 流式时要末片用量(prompt_tokens)→ 面板"实时上下文压力"。是请求级参数、不进 messages 前缀,
    // 不影响 deepseek KV 前缀缓存(byte-stable prefix 只看 messages/tools)。
    if stream {
        body["stream_options"] = serde_json::json!({ "include_usage": true });
    }
    // 不设 max_tokens = 让模型自己决定何时停(实测 deepseek V4 端点无小默认截断,见交接报告 0-OPUS22)。
    if let Some(mt) = req.max_tokens {
        body["max_tokens"] = serde_json::json!(mt);
    }
    // 思考强度(deepseek V4):显式带上 reasoning_effort + thinking enabled。
    if let Some(eff) = &req.reasoning_effort {
        body["reasoning_effort"] = serde_json::json!(eff);
        body["thinking"] = serde_json::json!({ "type": "enabled" });
    }

    if !req.tools.is_empty() {
        body["tools"] = serde_json::json!(req
            .tools
            .iter()
            .map(|t| serde_json::json!({
                "type": "function",
                "function": { "name": t.name, "description": t.description, "parameters": t.params }
            }))
            .collect::<Vec<_>>());
    }

    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChatMessage;
    use growbox_core::ToolCall;

    #[test]
    fn reasoning_effort_emits_effort_and_thinking() {
        let req = ChatRequest::new("m", vec![ChatMessage::user("hi")]).with_reasoning_effort("max");
        let body = build_body(&req, false);
        assert_eq!(body["reasoning_effort"], "max");
        assert_eq!(body["thinking"]["type"], "enabled");
    }

    #[test]
    fn empty_reasoning_effort_is_omitted() {
        let req = ChatRequest::new("m", vec![ChatMessage::user("hi")]).with_reasoning_effort("");
        let body = build_body(&req, false);
        assert!(body.get("reasoning_effort").is_none(), "空串=不发");
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn reasoning_content_passed_back_only_when_present() {
        // ★thinking 模式硬约束:带 tool_calls 的中间 assistant 必须回传 reasoning_content,否则 deepseek V4 返回 400。
        let asst = ChatMessage {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![ToolCall { id: "c1".into(), name: "f".into(), arguments: "{}".into() }],
            tool_call_id: None,
            reasoning_content: Some("我的思考".into()),
        };
        let req = ChatRequest::new("m", vec![ChatMessage::user("hi"), asst]);
        let body = build_body(&req, false);
        let msgs = body["messages"].as_array().unwrap();
        assert!(msgs[0].get("reasoning_content").is_none(), "user 消息不带 reasoning_content");
        assert_eq!(msgs[1]["reasoning_content"], "我的思考", "工具调用 assistant 必须回传 reasoning_content");
    }
}
