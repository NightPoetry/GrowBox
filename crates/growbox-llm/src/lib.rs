//! growbox-llm — 与 LLM 通信:路由、流式、工具/reasoning 解析。
//!
//! 实现 `设计文档/系统架构/02-llm.md`。
//! 只管"怎么调",不管"何时调/调来干啥"(那是 app 的 Agent 循环)。

mod client;
mod embed;
mod error;
#[cfg(feature = "local-embed")]
mod local_e5;
mod stream;
mod types;

pub use client::LlmClient;
pub use embed::{lexical_embed, EmbedKind, Embedder, LexicalEmbedder, RemoteEmbedder};
#[cfg(feature = "local-embed")]
pub use local_e5::LocalE5Embedder;
pub use error::{LlmError, LlmResult};
pub use stream::{parse_sse_line, ToolCallAccumulator};
pub use types::{ChatMessage, ChatRequest, Role, StreamChunk};
