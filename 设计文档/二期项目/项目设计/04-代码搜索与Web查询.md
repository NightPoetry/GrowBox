# 代码搜索 + Web 查询

> 两个便宜的"看"类执行器。代码搜索是导航基础设施;Web 查询可先用现成 web MCP 顶替(见 `05`),急用再自建。

## 范围

只做:`code_search`(ripgrep 后端)+ `web_search` / `web_fetch`。不重做检索引擎;不做语义代码搜索(那是 LSP/tree-sitter 的活)。

## 方案

- **`code_search`**:ripgrep 后端,正则 + glob/类型过滤 + 输出模式(content / files / count + multiline)。
  受可写目录授权约束,结果接一期"工具输出上限"旋钮。与 `file_list` 划清职责(list = 列目录,search = 按内容/模式找)。
  提示词层引导"搜代码用它,别用 `shell grep`"(更快、结构化、权限对)。
- **`web_search` / `web_fetch`**:解陌生报错、查库/crate API、查文档;`web_fetch` 抓取转 markdown。
  **走一期安全门**:发出去即公开(provenance 判定,可能被缓存/索引);需网络 / API key。
  **可延后**:MCP 客户端就绪后用现成 web MCP 顶替,省自建。

## 接口草案

- `code_search{ pattern, glob?, type?, mode=content|files|count, multiline? }` → 结构化命中(file:line + 行内容)。
- `web_search{ query }` / `web_fetch{ url, prompt }`。

## 数据流

`AI 要定位某用法 → code_search{pattern, type:"rust"} → 命中 file:line 列表 → file_read 精读 → 改`。
`AI 撞陌生报错 → web_search{报错关键句} → web_fetch{命中链接} → 拿到解法 → 修`。

## 接原理

`设计原理/00-工具体系扩展`:推论1(代码搜索是"看得见代码"的物理基础之一)+ 推论4(web 查错 = 补"看"一环)。

## 里程碑与风险

- **M1**:`code_search`(ripgrep)content/files/count 三模式。
- **M2**:multiline + 类型过滤。
- **web**:延后到 `05` 的 MCP 就绪,用现成 web MCP;急用再自建 `web_fetch`/`web_search`。
- **风险**:大仓库结果上限(接一期旋钮);web 隐私(安全门 provenance);与 `file_list` 职责边界要在提示词写清。

## as-built(web 部分,2026-06-12 落地)

> 工具完备性对照后 Web 是唯一真缺口,按"急用自建"路线原生落地(不等 web MCP);`code_search` as-built 已久(二期 A3)。

- **执行器**:`growbox-gui/src/executors/web.rs` —— `WebFetch` + `WebSearch` + 共享 `WebConfig`(provider/api_base/api_key/max_results/timeout_secs,连接与改设置时经 `Registry::set_web_config` 热更;推论9 全可设,设置 → 连接 →「Web 搜索」)。两者皆 `Risk::Safe` + `Claim::Net(url)`。
- **安全门成为一类一等资源**:core 加 `Claim::Net(String)`,safety 加 `Operation::Net` + `judge_net` —— 非 http(s) 硬拒(file:///gopher 协议走私);**内网/本机(字面 IP 分级 + localhost/*.local/裸主机名)NeedAuth** 走决定脊柱阻塞 round-trip(调本地 dev server 属正当用法,授权即放行);公网放行。授权持久化 = `GrantScope::ThisProjectHost` + `ProjectConfig.net_grants`(与路径授权**互不放宽**,文件侧"信任本项目"也不打开网络);前端 PermissionDialog `access="net"` 专属文案 → `grant_net_host` 命令落库。
- **SSRF 纵深**(safety 单一真源 `host_is_private_literal`/`ip_is_private`,执行器复用):公网域名解析后复查(DNS rebinding)+ 解析结果 pin 进客户端(防判定/连接 TOCTOU);重定向手动逐跳(≤5),跨源跳内网即停并引导直接 fetch(走授权);**内网目标绕过一切代理**(真机实测 macOS 系统代理 Clash 会把 127.0.0.1 收走回 502,且内网 URL 发外部代理本身即泄漏;公网保留系统代理 = 用户靠它出网)。
- **web_fetch**:GET + HTML→纯文本(html2text,纯 Rust 守跨平台红线;raw=true 原文)+ 流式按"工具输出上限"旋钮截断 + 4xx/5xx 内容照给但标失败 + 二进制不展示;取消感知(150ms 轮询终止位)。
- **web_search**:tavily/brave/searxng 三适配(官方端点可覆盖;searxng 自建必填 base);未配置/缺 key **诚实失败 + 引导去设置**;命中 = 标题/URL/摘要 + 提示"读全文用 web_fetch"。
- **不可信标注**:两工具结果均加「不可信外部输入」前缀(与 MCP D2 同精神)——发出去即公开的 provenance 判定交给用户对 provider 的选择与授权。
- **懒加载**:默认进 `deferred_tools` 名单(露名 + tool_search 按需加载,KV 前缀稳)。
- **测试**:safety 7(judge_net/解析/分级/授权互不放宽)+ web.rs 15(本地 TcpListener 真 HTTP:正文化/截断/重定向拦内网/三 provider 解析/未配置引导)+ registry 2(唯一安全门 NeedAuth/硬拒)。
