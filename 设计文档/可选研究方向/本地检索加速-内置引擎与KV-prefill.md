# 本地检索加速:内置推理引擎 + KV prefill

> 状态:**仅记录,未实现**(2026-06-10,Opus 与用户讨论后用户决定先不做)。
> 性质:内核记忆检索的可选加速层。讨论 + 调研已完成,实现挂起。

---

## 1. 用户原始诉求(原话要点)

- 现在记忆检索除了向量搜索,还有更具体的"LLM 搜索/判断";但**这套 LLM 判断依赖外部 URL**(实际是 judge 走潜意识槽 → 回退主模型 → api.deepseek.com,远程)。
- 想**单独加一个搜索用的开关,默认禁止**;开启后多出一个设置框,可以填**本地** endpoint / 模型。
- 核心加速设想:**把记忆 prefill 成 KV 缓存**复用。形如
  `"记忆是 [M],需要根据问题判断是否相关,用户的问题为:[Q]"` —— 前面这块(记忆)预先算成 KV,
  之后每次比较只在末尾续上问题,从而加速。明确认知:**这只能本地做,远端做不到。**
- 讨论后用户进一步定:
  - LM Studio 不行 → **内置引擎**。开启本地加速时多一个**必填项 = 模型文件**;
    上下文长度/温度等可设,但默认 **Auto**(按内容动态调:内容长就自动增大上下文窗口)。
  - 节奏:**一步到位(含 KV prefill),但不破坏原有体系** —— 远程判断那条路原样保留,
    只是"LLM 判断 + 开了本地加速"时切到另一套本地判断子系统。
  - 引擎选型:让我先**核实 candle 能否跑 Qwen3.6-27B 这种量级** → 调研结论见下,据此用户**暂缓**整个功能。

---

## 2. 为什么必须"内置引擎"(外部服务器的死穴)

- **"外部注入 KV 缓存" —— 任何走 OpenAI 兼容 API 的服务都做不到**,LM Studio 也不行。
  没法在外面算好 KV 再 POST 进去。
- LM Studio 具体核实结论:
  - **不暴露** llama.cpp 原生的 `/slots/{id}/save` 与 `/restore`(那是 raw `llama-server` 的端点,
    LM Studio 的 REST API v0 是另一套受限表面)。所以经 LM Studio 没有 KV 注入入口。
  - 它有**自动前缀缓存**(底层 llama.cpp `cache_prompt: true` 默认开),但限制致命:
    ① 只保最近一条前缀热着、idle 还会丢(bug #1861);**无法把 N 条记忆 KV 同时焊住**;
    ② **用户的"记忆在前、问题在尾"排版方向恰好反了** —— 1 个 Q 对 N 条记忆、记忆在前 →
       每条请求前缀都不同 → 自动缓存命中≈0;要吃自动缓存得"问题在前",但那样只缓存住短 Q、
       真正贵的长记忆每次仍全算(省的是便宜那头);
    ③ 部分架构静默关闭复用(Qwen3.5-MoE / 滑窗 / Mamba-SSM;Mac MLX 对混合架构前缀缓存基本是坏的)。
- **raw `llama-server`**(llama.cpp):确有 `/slots/save|restore`(`--slot-save-path`),restore 接近瞬时,
  是"prefill 全部记忆"的真身 —— 但要离开 LM Studio、绑定 llama.cpp 二进制、每条记忆一个 slot、N 次串行调用,
  且 mmproj/视觉模型下被禁用。
- **内置引擎 = KV 缓存归我们 Rust 自己握** → 用户的 prefill 设想从此真正可行(不是绕路,是拿到底层把手)。
  对 `[系统指令][记忆 M]` 跑一次前向 → 拿 KV 张量快照 → 存住(内存 LRU 或落盘);
  任意问题 Q 来,克隆该快照、只解码 `[Q] 判断相关吗`、读结果 → **记忆 M 在其生命周期里只算一次**。
  Auto 上下文/温度也顺手:自己 tokenize 真实内容 → n_ctx 取刚好容下的下一档(设上限);
  判断类任务温度本就该 ≈0(要确定的 yes/no)。

---

## 3. 引擎与模型调研硬结论(2026-06-10)

### candle 现状(项目已在树)
- `candle-core/nn/transformers 0.9` 已是 workspace 依赖(`local-embed` feature 后),给本地嵌入用;
  但 e5-small 的 candle 实现其实**还没真正接完**(当前默认仍是词法散列 `LexicalEmbedder`)。
- Cargo.toml 特意为保**全程纯 Rust** 固定 candle 0.9(避开 onig 那条 C 原生链)。
- **没有任何 llama.cpp 绑定。**
- candle 真实瓶颈 = **架构覆盖,不是模型体积**:有 RAM 能跑到 ~30B 量化(慢),
  前提是该架构已被实现(Llama/Mistral/Gemma/Phi/Qwen2/Qwen3-MoE 都有)。
- 更顺手的纯 Rust 选项:**mistral.rs**(基于 candle 的纯 Rust 推理引擎,量化/Metal/采样/多架构都包好,
  保住"全程纯 Rust")—— 但同样不会有全新架构。

### Qwen3.6-27B 能不能跑 → 现在不行,且恰是最不该选的那类
1. **架构没接**:Qwen3.6-27B 是 2026-04-22 发的全新 dense 模型,用
   **Gated DeltaNet(线性注意力)+ Gated Attention 混合架构**(64 层里每 4 层 3 层是线性注意力)+ MTP 投机解码。
   candle 几乎肯定还没接;要跑得自己实现 DeltaNet 层 + MTP,工程量巨大。
2. **它的架构正好废掉 KV prefill**:前缀 KV 缓存复用只对**纯全注意力**模型成立;一旦有
   滑窗/Mamba-SSM/线性注意力层就静默退化为全量重算。Qwen3.6 有 3/4 的层是线性注意力 ——
   **它没有你要 prefill 的那种 KV 缓存**(线性注意力带的是递归状态,另一套机器,也没人在 candle 实现)。
3. **就算能跑也慢**:27B dense Q4 ≈ 7 tok/s 基线(48GB M4 Pro),要 ~17GB 内存;
   一个本该**加速**检索的判断器用 27B 本地跑,大概率比直接调远程 DeepSeek 还慢。

### 关键结论 = KV prefill 想要的是"小全注意力模型"
- 判断"记忆跟问题相不相关"是简单任务,不需要 27B 旗舰;整套架构(向量粗筛 → 小 LLM 判被收窄的 frontier)
  本就为廉价判断设计。
- 把判断槽换成**小型全注意力模型**(Qwen2.5-1.5B/3B、Gemma-2B、Phi、Llama-3.2-3B):
  ① candle/mistral.rs 已支持;② 有真 KV 缓存 → prefill/快照复用**真能用**;③ 本地快(几十~上百 tok/s)、离线免 key。
- "大模型质量"该交给**远程 DeepSeek**(那条路原样保留);"本地加速判断"的本质前提就是**小而快**。
- **悬而未决(用户暂缓时未拍)**:本地判断模型量级。我推荐 1.5B~4B 小全注意力;用户当时选择"先记录,不做"。

---

## 4. 若将来要做:架构接入点(已核到的代码位置)

- **判断逻辑(切本地的落点)**:`crates/growbox-memory/src/memory/retrieval.rs`
  - 档 B LLM 综合判断 `sub.judge_edge(query, pos_k, neg_k, target)`(retrieval.rs:262,一条边一次)
  - 前沿/线性兜底批判 `sub.judge_relevant(query, candidates)`(retrieval.rs:304/313/393,多条一次)
  - 这两个走 `crates/growbox-gui/src/bridge.rs` 的 `Subconscious` 实现(`complete()` 收流式、非流式语义)。
  - "开了本地加速 → 切另一套判断子系统"应在这里按开关分流(不动远程那支)。
- **独立 LLM 槽的现成范式**:`crates/growbox-gui/src/state.rs` 的 `connect()` 构建潜意识 driver
  (独立 endpoint/key + 回退主模型)—— 照抄一个"本地判断引擎槽"。
- **配置字段**:`crates/growbox-core/src/project.rs` 的 `Settings`(嵌入槽 `embed_*` / 潜意识槽 `subconscious_*` 同款,
  新增本地判断引擎槽:模型文件路径 + Auto 上下文/温度开关 + 总开关默认关)。
- **前端设置框**:`crates/growbox-gui/frontend/src/components/settings/ConnectionTab.tsx`
  (与"潜意识模型"段同级新增"本地检索加速"块);四语文案。
- **嵌入引擎前例**:`crates/growbox-llm/src/embed.rs`(`LexicalEmbedder`/`RemoteEmbedder`,
  candle 落地的既有模式可参照)。
- **已定约束**:一步到位含 KV prefill;**不破坏原体系**(远程判断一字不动,本地是并行新增、开关 gate)。

---

## 5. 来源(调研链接)

- LM Studio / 前缀缓存限制:<https://medium.com/@michael.hannecke/llm-prompt-caching-what-you-should-know-2665d76d3d8d>
- LM Studio KV 复用/idle 丢弃 bug:<https://github.com/lmstudio-ai/lmstudio-bug-tracker/issues/1563> · <https://github.com/lmstudio-ai/lmstudio-bug-tracker/issues/1861>
- llama.cpp slot save/restore:<https://github.com/ggml-org/llama.cpp/discussions/13606> · <https://lmstudio.ai/docs/developer/rest/endpoints>
- Qwen3.6-27B(dense / Gated DeltaNet + MTP):<https://qwen.ai/blog?id=qwen3.6-27b> · <https://www.marktechpost.com/2026/04/22/alibaba-qwen-team-releases-qwen3-6-27b-a-dense-open-weight-model-outperforming-397b-moe-on-agentic-coding-benchmarks/>
- 27B 本地 tok/s 实测:<https://vinoth12940.github.io/blog/articles/genai-20260519-local-mtp-speculative-decoding/>
- candle / mistral.rs:<https://github.com/huggingface/candle> · <https://docs.rs/candle-transformers/>
