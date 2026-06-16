# Embedding(第一层 RAG 的真模型)实现计划

> 背景:当前 `embed` 是本地词法散列(`bridge.rs::local_embed`,FNV 词袋),只抓字面词重叠、无语义——RAG 实质是模糊关键词匹配。要做真语义 RAG,必须换真 embedding 模型。
> 决策(用户拍板 2026-05-31):**本地内嵌小模型为默认 + 可选远程槽位**。

## 形态:Embedder 是可切换的槽位
`Subconscious::embed` 已是 trait,换实现是干净一刀。做成两路实现:

### 本地(默认)
- 内嵌一个小 embedding 模型,用 **candle**(纯 Rust,无 ONNX 原生库,随 Tauri 分发干净)在本机算。
- 离线、免费、记忆不出本机——契合本地持久 agent。
- **模型(已定 2026-05-31):`multilingual-e5-small`**(~470MB,384 维,中英混合稳)。架构是 BERT(基于 Multilingual-MiniLM-L12-H384),candle 的 bert 模块可跑。
- **跨平台(用户硬要求)**:candle CPU 后端纯 Rust、无原生依赖,macOS/Windows/Linux 通吃;不开 GPU/accelerate 特性以免平台差异。这正是不用 fastembed/ort 的原因(ONNX Runtime 要按平台带原生库)。
- **e5 必须加前缀**(实测坑,不加召回会差):查询用 `"query: "` 前缀,入库文档用 `"passage: "` 前缀。embed 实现里按用途区分两种前缀。
- 模型文件随包分发或首次运行下载到 data_dir(二选一,见"待定")。

### 远程(可选槽位)
- OpenAI 兼容 `POST {base}/embeddings`,body `{model, input}`,header `Authorization: Bearer {key}`,返回 `{data:[{embedding:[...]}]}`。
- 配置三项:**Base URL + API Key + 模型名**(实测 `model`+`input` 必填)。
- 本地服务(Ollama / LM Studio)同此接口,URL 指 `http://localhost:11434/v1` 即可——所以"想要别的本地模型"也走这条,不必改 app。
- DeepSeek 无 embedding 端点,故 embedder 与聊天 provider 是独立槽位,不复用聊天 key。

## UI(连接/设置页)
仿现有"Supervisor 模型"那组,加一组"嵌入(Embedding)":
- 一个开关:本地(默认)/ 远程。
- 远程时显示 Base URL + Key + 模型名三个输入。
- 本地时无需配置(或仅显示当前内嵌模型名 + 版本)。

## 关键牵连
- **换模型 = 向量空间变,旧 node 向量全失效**。Node 存当前 embedding 的"模型版本标记";版本不符 → `ensure_embeddings` 重嵌。本地↔远程切换、或换模型,都触发重嵌。
- 维度不固定(bge-small-zh 512 / e5-small 384 / OpenAI 1536)。`cosine` 已对长度不等返回 0,但重嵌前混用会让旧向量算分为 0——所以切换时必须全量重嵌,不能半量混用。
- 与磁盘化(见 `precision-layer.md` 的存储讨论)同盘:embedding 跟节点一起落 redb,本地模型按需算、随节点入库。

## 实施顺序(建议)
1. [完成 2026-05-31] 抽 `Embedder` trait + 远程实现(OpenAI 兼容)+ 版本标记 + 重嵌 + query/passage 前缀打通 + connect 透传。
2. [完成 2026-06-01] candle 本地 e5(`local_e5.rs`)+ feature `local-embed` 默认 + `build_embedder` 默认走它。**真机验证通过**:同义不同词 cosine=0.9237、无关=0.7871(词法版做不到)。
3. [待做] UI 配置(连接/设置页加嵌入槽,仿 supervisor)= 路线 P2。
4. [完成 2026-06-01] 真机验证同义召回(见上 candle live test `live_e5_synonym_recall`)。

## ★ e5 落地后发现的待办(记入路线 P3 RAG 重做)
- **RAG 阈值要按 e5 分布重标**:`memory.rs::RAG_HIT_THRESHOLD=0.80` 是为词法向量(无关≈0)定的;e5 无关项 cosine≈0.79、相关≈0.92,0.80 会把几乎所有东西当命中。建议上调到 ~0.85(对词法仍安全,无关≈0;对 e5 能分开 0.79/0.92)。换 ANN(arroy)时一并校准。
- candle 关键实测(`local_e5.rs`):pin candle **0.9**(0.10 的 candle-core 硬依赖带 onig 原生库的 tokenizers,破跨平台红线);tokenizers 关 onig 默认走 fancy-regex 纯 Rust;forward 传 **f32** attention_mask(传 u32 运行时 dtype 报错);均值池化用 mask + L2 归一化;e5 前缀 query:/passage:。

## 阶段 1 落地实况(Opus 2026-05-31)
- `growbox-llm/src/embed.rs`:`Embedder` trait(批量 `embed(&[String], EmbedKind) -> Vec<Vec<f32>>` + `version()`)、`EmbedKind{Query,Passage}`、`RemoteEmbedder`(OpenAI 兼容 `{base}/embeddings`,按 index 还原顺序)、`LexicalEmbedder`(原 gui 词法版搬来,作当前默认)。纯函数 `build_embed_body`/`parse_embed_response` 单测。
- `memory::Subconscious`:加 `embed_query`/`embed_passage`(默认回退 `embed`,表达"默认查询/文档同向量化";mock 零改动)+ `embedding_version`(默认空)。retrieve→embed_query,ensure_embeddings→embed_passage。
- `Node` 加 `embedding_version`;`set_embedding(id, vec, version)`;`ensure_embeddings` 版本不符即整体重嵌(换 embedder→向量空间变→旧向量失效)。测试 `reembeds_when_version_changes`。
- `LlmBridge` 持 `Arc<dyn Embedder>`;`Settings` 加 `embed_remote/embed_api_base/embed_api_key/embed_model`;`state::build_embedder` 按设置选远程/词法;`connect` 命令透传四参数。
- 全工作区单测 113 绿。**注意**:远程通用模型(text-embedding-3)不加 e5 前缀,故 RemoteEmbedder 忽略 EmbedKind;前缀逻辑留给阶段 2 的 candle e5 实现按 kind 落地。
