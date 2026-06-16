//! 真机集成测试 —— 实际调用 deepseek-v4-flash。
//!
//! 默认 `#[ignore]`(CI/离线不跑)。手动运行:
//! ```bash
//! DEEPSEEK_API_KEY=sk-xxx cargo test -p growbox-llm --test live_deepseek -- --ignored --nocapture
//! ```
//! 验证 `实验记录/00` 的实测结论在我们的代码里真的成立。

use growbox_core::ToolDef;
use growbox_llm::{ChatMessage, ChatRequest, LlmClient, StreamChunk, ToolCallAccumulator};

fn client() -> Option<LlmClient> {
    let key = std::env::var("DEEPSEEK_API_KEY").ok()?;
    Some(LlmClient::new("https://api.deepseek.com", key))
}

#[tokio::test]
#[ignore = "需要真实 API key"]
async fn live_stream_has_reasoning_then_content() {
    let Some(c) = client() else {
        eprintln!("跳过:未设置 DEEPSEEK_API_KEY");
        return;
    };
    let req = ChatRequest::new(
        "deepseek-v4-flash",
        vec![ChatMessage::user("用一句话介绍 Rust")],
    )
    .with_max_tokens(400);

    let mut rx = c.chat_stream(req).await.expect("请求应成功");
    let (mut reasoning, mut content, mut finish) = (String::new(), String::new(), String::new());
    while let Some(chunk) = rx.recv().await {
        match chunk.expect("流片段不应报错") {
            StreamChunk::Reasoning(r) => reasoning.push_str(&r),
            StreamChunk::Content(c) => content.push_str(&c),
            StreamChunk::Done { finish_reason } => finish = finish_reason,
            _ => {}
        }
    }
    println!("reasoning={}字 content={:?} finish={}", reasoning.chars().count(), content, finish);
    // 实测结论:flash 是推理模型 → 有 reasoning;给足 token → 有正文、正常收尾。
    assert!(!reasoning.is_empty(), "flash 应产出 reasoning_content");
    assert!(!content.is_empty(), "给足 token 时应有正文");
    assert_eq!(finish, "stop");
}

#[tokio::test]
#[ignore = "需要真实 API key"]
async fn live_tool_call_works() {
    let Some(c) = client() else {
        eprintln!("跳过:未设置 DEEPSEEK_API_KEY");
        return;
    };
    let tool = ToolDef {
        name: "file_read".into(),
        description: "读取一个文件的内容".into(),
        params: serde_json::json!({
            "type":"object",
            "properties":{"path":{"type":"string","description":"文件绝对路径"}},
            "required":["path"]
        }),
    };
    let req = ChatRequest::new(
        "deepseek-v4-flash",
        vec![ChatMessage::user("请读取文件 /tmp/config.json 的内容")],
    )
    .with_tools(vec![tool])
    .with_max_tokens(600);

    let mut rx = c.chat_stream(req).await.expect("请求应成功");
    let mut acc = ToolCallAccumulator::new();
    let mut finish = String::new();
    while let Some(chunk) = rx.recv().await {
        match chunk.expect("流片段不应报错") {
            StreamChunk::ToolCallDelta { index, id, name, args_fragment } => {
                acc.push(index, id, name, &args_fragment);
            }
            StreamChunk::Done { finish_reason } => finish = finish_reason,
            _ => {}
        }
    }
    let calls = acc.finish();
    println!("finish={} calls={:?}", finish, calls);
    // 实测结论:flash 工具调用正常,标准格式,参数完整。
    assert_eq!(finish, "tool_calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "file_read");
    let args: serde_json::Value = serde_json::from_str(&calls[0].arguments).expect("args 应为合法 JSON");
    assert_eq!(args["path"], "/tmp/config.json");
}
