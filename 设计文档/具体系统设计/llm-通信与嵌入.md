# llm(growbox-llm)—— LLM 通信 + 嵌入

两件事:和 LLM 说话(流式/推理/工具),把文本变向量(嵌入)。

## A. LLM 通信
- `client.rs` / `stream.rs` / `types.rs`:OpenAI 兼容协议,**流式**。
- 实测要点(deepseek-v4-flash,见 `实验记录/00`):是**推理模型**,流式先 `reasoning_content` 后 `content`(分属不同 delta 字段,顺序 R→C);工具调用标准 OpenAI 格式、支持并行,流式 `tool_calls` 按 `index` 增量拼 arguments;"空参 `{}`" = token 被 reasoning 吃光的截断,判截断重试 + 给足 token;沉默超时要把 reasoning chunk 算作"有活动"。
- `error.rs`:错误类型。

## B. 嵌入(Embedder)—— `embed.rs` + `local_e5.rs`
- **`Embedder` trait**:批量 `embed` + `version()` + `EmbedKind{Query,Passage}`。换实现的 seam。
- 三个实现:
  - `RemoteEmbedder` —— OpenAI 兼容远程(base/key/model 可配);通用模型不加 e5 前缀。
  - `LexicalEmbedder` —— 词法散列兜底(`--no-default-features` 或无 candle 时)。
  - **`LocalE5Embedder`(`local_e5.rs`,默认)** —— candle 跑本地 `multilingual-e5-small`(384 维)。真机:同义 cosine 0.9237、无关 0.7871。
- 换 embedder = 向量空间变 → `version` 变 → 旧向量整体重嵌(memory 侧据 `embedding_version` 触发)。

## 用的库
- 通信:reqwest(0.12,**rustls-tls**,无 OpenSSL)、tokio、tokio-stream、futures、thiserror、async-trait。
- 嵌入(feature `local-embed`,**默认开**):candle-{core,nn,transformers} **0.9**、tokenizers 0.21(**fancy-regex**,关 onig)、hf-hub 0.4(rustls)。

## 关键坑(都为守跨平台纯 Rust)
- **candle 钉 0.9**:0.10 的 candle-core 硬依赖带 onig(原生 oniguruma)的 tokenizers,破跨平台。
- tokenizers 关 onig 默认、走 fancy-regex 纯 Rust。
- e5 推理:attention_mask 传 **f32**(传 u32 运行时报错);`query:` / `passage:` 前缀按 `EmbedKind` 加。
- 模型解析顺序:resource_dir/models(带包预置)→ data_dir/models(下载缓存)→ hf-hub 下载。见 `打包设计.md`。

## 关键文件
`crates/growbox-llm/src/{client,stream,types,error,embed,local_e5}.rs`;feature 在 `Cargo.toml`(`default=["local-embed"]`)。

## 现状
11 单测绿(+1 ignored 真机)。真机嵌入已验证。
