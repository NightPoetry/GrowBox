# 00 — deepseek-v4-flash 模型行为

> 实验日期:2026-05-30。API:`https://api.deepseek.com`(OpenAI 兼容)/ `.../anthropic`(Anthropic 兼容)。
> 可用模型:`deepseek-v4-flash`、`deepseek-v4-pro`。
> **核心发现:旧 AI 关于 flash 的核心结论是错的。flash 是推理模型,所谓"爱发空参"实为 token 被思维链吃光后的截断假象。**

---

## 一句话给后续开发

**flash 每次输出 = 先一段 `reasoning_content`(思维链)再 `content`/`tool_calls`。必须:① 单独处理 reasoning 字段;② max_tokens 给足并预留 reasoning 开销。否则工具调用会被截断成空参——这正是旧代码"空参保护"想治却治错的病。**

---

## 记录 1:flash 是推理模型(✗ 推翻旧认知)

- **待验**:flash 输出结构如何?
- **方法**:`max_tokens=20` 发"只回复两个字"。
- **观测**:返回含 `reasoning_content` 字段;`usage.completion_tokens_details.reasoning_tokens=19`;`content=""`;`finish_reason="length"`。
- **结论**:flash 先生成思维链(独立字段 `reasoning_content`),再生成正文。普通模型按"content 直出"的假设在此**不成立**。

## 记录 2:token 给足则正文正常

- **方法**:同问题,`max_tokens=800`。
- **观测**:`reasoning_tokens=50`,`content="你好！"`,`finish_reason="stop"`。
- **结论**:正文正常。token 预算必须覆盖 reasoning + 正文。

## 记录 3:工具调用完全正常,标准 OpenAI 格式(✗ 推翻"flash 不会调工具")

- **方法**:给 `file_read` 工具,`max_tokens=600`,问"读取 /tmp/config.json"。
- **观测**:`finish_reason="tool_calls"`;`tool_calls[0].function = {name:"file_read", arguments:"{\"path\": \"/tmp/config.json\"}"}`;参数完整正确。
- **结论**:模型工具调用能力正常,标准格式。旧 AI "flash 不会调工具/爱发空参"是误诊。

## 记录 4:并行工具调用支持

- **方法**:要求"同时读取 a/b/c 三个文件,一次性发起"。
- **观测**:一次返回 `tool_calls` 数组含 **3 个** `file_read`,各带正确 path。
- **结论**:支持一轮多工具并行。分发器要能处理一个 response 里多个 tool_call。

## 记录 5:流式中 reasoning 与 content 的顺序

- **方法**:`stream=true`,"用一句话介绍 Rust",`max_tokens=400`。
- **观测**:reasoning 28 个 chunk、content 26 个 chunk;出现顺序严格 **R→C**(先全部 reasoning,再全部 content),分属 delta 里不同字段(`delta.reasoning_content` vs `delta.content`)。
- **结论**:流式解析要分别累加两个字段;reasoning 阶段不要当正文显示(可作"思考中"指示)。沉默超时判定要把 reasoning chunk 也算作"有活动",否则会误判卡死。

## 记录 6:流式工具调用增量拼接

- **方法**:`stream=true` + 工具。
- **观测**:`tool_calls` 分多个 delta 增量到达(本次 19 个 delta);按 `index` 聚合,`function.name` 在首个 delta 给出,`function.arguments` 分片拼接;结束 `finish_reason="tool_calls"`。
- **结论**:流式分发器必须按 `tool_calls[].index` 累积 arguments 字符串,收齐再解析 JSON。

## 记录 7:截断退化 = "空参"假象的真身(✗ 彻底推翻"空参保护"的病因)

- **方法**:给工具,`max_tokens=30`(故意不够),问"读取 /tmp/config.json"。
- **观测**:截断有**两种表现**,取决于 token 在哪步耗尽:
  - 思维链写完、参数没写完 → `finish_reason="tool_calls"`,`tool_calls=[{function:{name:"file_read", arguments:""}}]`(**空 arguments**)。
  - 思维链都没写完就停 → `finish_reason="length"`,`tool_calls=None`,`content=""`。
- **结论**:旧 AI 看到的"空参 `{}`"= token 被 reasoning 吃光、参数还没开始生成就截断。**根治办法不是"空参保护",而是给足 token + 处理 reasoning**。两种表现都应判为"截断错误"(看 `finish_reason="length"` 或 空 arguments → 重试/加预算),而非"模型调用了空工具"。

---

## 对架构的硬约束(已实测背书)
1. **llm crate 必须解析 `reasoning_content`**(非流式 message 字段 + 流式 delta 字段),与 `content` 分开。
2. **max_tokens 默认值要大**,并为 reasoning 预留;不可沿用普通模型估算。
3. **流式工具调用按 `index` 增量拼接** arguments,收齐再解析。
4. **空 arguments 判定为截断**(检查 `finish_reason`/usage),触发重试或加预算,而非"空参友好提示"。
5. **沉默超时**把 reasoning chunk 计入"有活动"。
6. 待测:`deepseek-v4-pro` 是否同构;Anthropic 兼容端点(`/anthropic`)行为;长上下文/缓存命中(`prompt_cache_hit_tokens` 字段已观察到,可用于省钱)。
