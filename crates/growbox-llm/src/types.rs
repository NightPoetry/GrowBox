//! LLM 通信的请求/响应类型。
//!
//! 实现 `设计文档/系统架构/02-llm.md`。

use growbox_core::{ToolCall, ToolDef};
use serde::{Deserialize, Serialize};

/// 一条对话消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(default)]
    pub content: String,
    /// assistant 发起的工具调用(role=assistant 时)。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
    /// 工具结果回填时,对应的 tool_call id(role=tool 时)。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
    /// 思维链(role=assistant 时)。带 tool_calls 的中间 assistant 消息**原样回传** reasoning_content,
    /// 使该消息字节匹配模型生成时的 model-output 缓存单元 → 命中 deepseek KV 缓存(byte-stable prefix)。
    /// 实测不回传不会 400,但 assistant 段前缀会分叉、缓存 miss(成本×10);故回传(2026-06-04 定论)。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning_content: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::text(Role::System, content)
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::text(Role::User, content)
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::text(Role::Assistant, content)
    }
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        ChatMessage { role, content: content.into(), tool_calls: Vec::new(), tool_call_id: None, reasoning_content: None }
    }
    /// 工具执行结果回填。
    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        ChatMessage {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(call_id.into()),
            reasoning_content: None,
        }
    }
}

/// 一次对话请求。
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDef>,
    /// None = 不限制,让模型自己决定何时停止。
    pub max_tokens: Option<u32>,
    pub temperature: f32,
    /// 思考强度(deepseek V4):"high"(默认)/ "max"(agent 场景官方建议)。None = 不发,吃服务端默认。
    pub reasoning_effort: Option<String>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        ChatRequest {
            model: model.into(),
            messages,
            tools: Vec::new(),
            max_tokens: None,
            temperature: 0.7,
            reasoning_effort: None,
        }
    }
    pub fn with_tools(mut self, tools: Vec<ToolDef>) -> Self {
        self.tools = tools;
        self
    }
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n);
        self
    }
    /// 思考强度。空串视为不设(吃服务端默认)。
    pub fn with_reasoning_effort(mut self, effort: impl Into<String>) -> Self {
        let e = effort.into();
        self.reasoning_effort = if e.is_empty() { None } else { Some(e) };
        self
    }
}

/// 流式输出的一个片段。
///
/// flash 是推理模型:先 `Reasoning`,再 `Content`/`ToolCallDelta`(实测,见 `实验记录/00`)。
#[derive(Debug, Clone, PartialEq)]
pub enum StreamChunk {
    /// 思维链片段(独立于 content)。
    Reasoning(String),
    /// 正文片段。
    Content(String),
    /// 工具调用增量:按 index 累积 args_fragment,收齐再解析。
    ToolCallDelta {
        index: u32,
        id: Option<String>,
        name: Option<String>,
        args_fragment: String,
    },
    /// 结束:带 finish_reason("stop"/"length"/"tool_calls")。
    Done { finish_reason: String },
    /// 用量回报(开 `stream_options.include_usage` 后,流末单独一片,choices 为空)。
    /// `prompt_tokens` = 本次请求实际发出的上下文 token 数(模型亲口算,非本地估算)→ 面板"实时上下文压力"。
    Usage { prompt_tokens: u32 },
}
