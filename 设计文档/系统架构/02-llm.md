# 02 — llm

## 职责
只管**与 LLM 通信**:路由(三槽位)、请求/流式、工具调用解析、reasoning 解析、方言(DLC)适配;不管何时调、调来干啥(那是 app 的 Agent 循环)。

## 接口
```rust
pub struct LlmRouter { /* main / subconscious / embedder 三槽位 */ }
impl LlmRouter {
    pub async fn chat_stream(&self, req: ChatRequest) -> Receiver<StreamChunk>;
    pub async fn embed(&self, text: &str) -> Vec<f32>;
}
pub enum StreamChunk {
    Reasoning(String),       // flash 思维链,单独字段(实测必须)
    Content(String),
    ToolCallDelta { index: u32, name: Option<String>, args_fragment: String },
    Done { finish_reason: String },
}
```

## Embedder 槽位(第一层 RAG 的真模型)
embed 不能用词法散列糊弄(只匹配字面词、抓不到语义),必须是真 embedding 模型。两种来源,可切换:
- **本地(默认)**:内嵌 `multilingual-e5-small`(~470MB,384 维,中英都稳),用 candle(纯 Rust CPU 后端,无原生库,各平台通吃)在本机算。离线、免费、记忆不出本机。注意 e5 要加 `query:`/`passage:` 前缀。详见 `计划/embedding-service.md`。
- **远程(可选槽位)**:OpenAI 兼容 `/v1/embeddings`,需 **Base URL + API Key + 模型名**三项(实测必填 `model`+`input`;Ollama 等本地服务同此接口,把 URL 指 localhost 即可)。
- 注意:**换 embedding 模型 = 向量空间变了,旧 node 向量全失效要重算**;模型带版本号,版本变则 `ensure_embeddings` 重嵌。
- DeepSeek(聊天 provider)无 embedding 端点,故不能复用聊天 key;embedder 是独立槽位。

## 依赖
→ 依赖:core、reqwest。 ← 被依赖:app(memory/learn 通过 app 传入的 client 使用)。

## 数据流
`ChatRequest → provider(OpenAI/Anthropic 兼容) → SSE 流 → 归一化为 StreamChunk`。
流式 tool_call 按 `index` 累积 `args_fragment` → 收齐再解析 JSON。

## 接原理
- `实验记录/00`:解析 `reasoning_content`、流式按 index 拼 args、空参判截断——全部实测背书。
- `设计/05` 推论2:空参 = 截断错误,由本层标注 `finish_reason`,app 决定重试。

## 已知坑(全部来自 `实验记录/00`,实测)
- flash 是推理模型:不解析 reasoning → 误判;不预留 token → 工具调用截断成空参。
- 流式 tool_calls 分多个 delta 到达,必须按 index 增量拼接。
- max_tokens 默认要大,且为 reasoning 预留。
