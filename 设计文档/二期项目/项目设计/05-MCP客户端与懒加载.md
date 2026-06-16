# MCP 客户端 + 懒加载(收编生态 + 工具数无界治理)

> 战略级大工程。两件事:① 把 GrowBox 做成 MCP 客户端,**不逐个开发**地收编开放生态成百上千工具;
> ② 懒加载机制治理工具数(核心常驻、扩展按需物化),**顺带修工作流 v2 按节点换工具对缓存前缀的破坏**。
> 懒加载先做、不依赖 MCP;它也是 process 可执行档物化(`02`)的共用机制。

## 范围

只做:MCP 客户端桥 + 懒加载机制。不收编不在编码闭环上的生态工具(日历/CRM/设计稿,留用户按需连)。

## 方案

### 懒加载(先做,不依赖 MCP)

- **核心常驻 + 扩展只露名**:每次请求只注入稳定核心工具(`file_*` / `shell` / 工作流入口 / `finish` / `ask_user` 等)
  + 一份"deferred 工具名单"(几百字节)。
- **`tool_search` 执行器**:`tool_search{query}`(`select:` 名字 / 关键词 / 必含词)→ 返回匹配工具完整 schema,
  **append** 进上下文(不改前缀,保 KV byte-stable,接一期缓存铁证)→ 之后即可直接调。
- **顺带修缓存破坏**:工作流 v2 的"按节点换工具子集"(`registry.tools_for`,`registry.rs:149`)改成
  "核心常驻 + 节点工具按需露名",前缀不再随进出节点变。
- **触点**:`registry.rs:113/149`(`workflow_defs`/`tools_for`/`definitions` 加"允许名单"过滤,
  与 process 物化(`02`)、MCP 共用同一过滤)。

### MCP 客户端

- **传输层**:stdio / SSE / HTTP;握手 + `tools/list` 列工具 + `tools/call` 调用。
- **工具 → 执行器适配**:每个 MCP 工具用其 JSON schema 造一个动态 `Executor`(`definition` 来自 MCP schema),
  动态注册进 `registry` → 与内置工具走**相同分发路径**(一期公理"一切能力皆执行器",零特例)。
- **连接配置**:`.mcp.json` 式 `{name, transport, command|url, scope}`,落项目/全局作用域(复用工作流 P3 分桶)。
- **安全**(★):MCP 工具结果 = **外部不可信输入**(prompt injection 面)→ 必须过一期安全门、
  **不可直接用于风险动作的参数选择**;跨机/headless 下交互式鉴权 server 不可用(降级感知)。

## 接口草案

- `tool_search{ query }` → `<functions>` schema 块(append-only)。
- MCP 工具执行器:`name = <server>_<tool>`,`definition` 来自 MCP schema。
- 连接配置持久:复用 `WorkflowStore` 的 P3 分桶思路(redb / 作用域)。

## 数据流

```
AI 要发 Slack → tool_search("slack send") → 拉回 slack_send schema(append,前缀不破)
   → AI 调 slack_send → MCP 客户端 tools/call 转发 server → 结果过安全门
   → 经唯一脊柱回灌(感知 / 记忆 / 安全 一视同仁)
```

## 接原理

`设计原理/00-工具体系扩展`:推论2(MCP 工具 = 动态注册执行器)+ 推论3(懒加载保前缀 byte-stable)。
与 `01/02` 的 process 物化共用"允许名单"过滤。MCP 不可信输入接 `设计/03-安全审查`。

## 里程碑与风险

- **M1 懒加载**:`tool_search` + 名单注入 + append schema,先在现有工具/工作流/process 上落地(**不依赖 MCP**)+ 修工作流缓存破坏。
- **M2 MCP 单 server**:传输 + 工具→执行器适配 + 打通一个(优先 github / filesystem)。
- **M3 多 server**:连接配置持久化 + 安全门(MCP 结果当不可信输入)+ 浏览器/DB MCP(Playwright/Postgres)+ web 用现成 web MCP。
- **风险**:不可信输入(安全门必接);工具数膨胀后 `tool_search` 召回质量 → 复用记忆 RAG 给工具**描述**做语义检索(非纯名字匹配);跨机鉴权 server 不可用。

## MCP 失败分析笔记(选择期可见,2026-06-13 用户点的★重要★方向)

> 完整设计见 `计划/工具记忆-不犯第二遍.md` v2 节 + `用户决策/决策日志.md` 2026-06-13 条。此处只记 MCP 侧落点。

真机暴露:filesystem MCP 只授权 Desktop、够不到项目目录而失败 —— 不该只是"失败一次",应沉淀成笔记。机制 = 把工具记忆延伸到 MCP:
- MCP 调用失败 → LLM 分析**失败原因 + 使用前提** → 结晶为 `tool_memory` 笔记(复用 `note_tool_memory`/`consult_tool_memory`)。
- **选择期可见**:列 MCP 候选给 LLM 选时,描述旁并入该 MCP 的历次笔记 **+ 时间戳**(`mcp_get_status` / 工具清单渲染处挂)。LLM 据**内容 + 使用前提**判"这 MCP 适不适合当前任务"、据**时间戳**判"笔记是否过时"。
- 把会诊从"将调时拦"前移到"选择/列举时就知道",失败笔记随时间自然失效,基建不拖累模型。
