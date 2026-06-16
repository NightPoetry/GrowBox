# LSP 集成(语言级代码智能)

> 模块1,单点回报最高。`lsp` 执行器是**薄客户端**,多语言能力来自起对应语言服务器;
> 诊断接一期「内部状态感知层」主动推送——把"感知一切失败"从运行期延伸到编辑期。

## 范围

只做:LSP 客户端管理器 + `lsp` 执行器 + 诊断推感知层 + 分层降级。
不做:不写语言服务器(用现成 rust-analyzer / tsserver);不追求跨语言全覆盖(先 GrowBox 自身栈 Rust+TS,其它探测)。

## 方案

### 现成件 vs 模块职责(库不替你做的)

**现成(不自己写)**:
- 语言服务器 = **rust-analyzer / typescript-language-server**(起子进程,干真正的解析)。
- 客户端协议框架 = **`lsp-types`**(LSP 全部协议类型)+ **`async-lsp`**(异步 LSP 框架,基于 tower,
  客户端/服务器皆可建,跟随 lsp-types,MIT/Apache)。**不自己撸 JSON-RPC**。
- 参考实现 = **Helix 的 `helix-lsp`**(Rust 里最干净的 LSP 客户端,管服务器生命周期/诊断/重连),Zed/lapce 更重。

**模块真实工作量(以上库都不给的胶水)**:
1. **生命周期编排**:按文件类型起对的服务器、`initialize`/`initialized` 握手、
   **`didOpen`/`didChange` 把缓冲区状态同步给服务器**、等首次索引、崩溃重启、退出回收。
2. **工具封装(意图↔协议)**:LLM 的"查 file:line 的引用" ↔ LSP 的 position-based 请求;
   把 LSP 啰嗦的返回**裁成 LLM 能读的精简结果**;0-based↔1-based 转换。
3. **诊断→感知胶水**:`publishDiagnostics` → `perceive`(任何库都不给)。
4. **降级策略**:无服务器退 tree-sitter/文本(库不管,是模块的策略)。

> **提示词刻意薄**:`lsp` 工具描述只列操作 + 参数(file/line/character,1-based)+ "无服务器报错",
> **不教 LLM 解析代码**(那是服务器的事)。**价值在编排和服务器,不在提示词**——别误以为"接了库就完事",
> 真活是上面四条胶水。GrowBox 的 `lsp` 工具文案放 `prompts/tools.i18n.json`(四国),同款薄形态、自己的措辞。

### 模块构件(上面"真实工作量"的具体落点)

- **LSP 客户端管理器**:按文件类型懒起对应语言服务器子进程(rust-analyzer / typescript-language-server),
  基于 `async-lsp` 驱动,JSON-RPC over stdio,生命周期托管(懒起、复用、超时、退出回收)。
- **`lsp` 执行器**(接唯一脊柱,普通 `Executor`):封装操作
  goToDefinition / findReferences / hover / documentSymbol / workspaceSymbol / goToImplementation / incomingCalls / outgoingCalls。
- **诊断推感知层**:订阅 `textDocument/publishDiagnostics` → 经 `Memory::perceive` / `perceive_transient`
  (`memory/perception.rs:15/38`)把编译错误/警告主动推入上下文。AI 改完即被告知哪行不过,不必跑全量构建。
- **分层降级**(`设计原理/00` 推论5):无服务器 → 返回明确"无服务器,按文本模式" + AI 感知当前层 → 退代码搜索 / tree-sitter / 文本+LLM,**永不报死**。

## 接口草案

- **客户端框架**:`lsp-types`(协议类型)+ `async-lsp`(异步起/驱动服务器);不自己撸 JSON-RPC。
- 执行器:`lsp{ op, file_path, line, character, query? }` → 结构化结果(位置列表 / 类型文本 / 符号树)。
  **行列对外 1-based,与 `file_read` 行号对齐**(协议内部 0-based,转换处别错)。
- 服务器供给:`ext → server 命令` 映射。**解析顺序 = 环境变量覆盖 → 探测系统已装(尊重用户 rustup)→ 自动下载到自有目录**。
- **自动装配(终端用户零配置)**:语言服务器(如 rust-analyzer)是单文件二进制,可按平台(`uname -m` → aarch64/x86_64)从官方 release 自动下载、gunzip + chmod。**装进 GrowBox 自有数据目录**(`~/Library/Application Support/com.nightpoetry.growbox/lsp/<server>-<version>`,跨平台用 `dirs`),**不入系统 PATH、不污染用户环境、免 admin、版本可控、随卸载清干净**——同 VS Code/Zed,也同 GrowBox 已有的本地 e5 模型管理。下载须**校验和验证**(不跑未验证二进制)+ 进度提示(~14MB)。tsserver 依赖 node,更复杂(bundle node 或独立打包版,后议,见 D3)。
- 诊断:`perceive("lsp_diagnostic", "<file>:<line> <severity> <msg>")` → 瞬态环 + 可检索时间线(双路感知)。

## 数据流

```
AI 要改函数 → lsp{findReferences} 看 12 处调用点 → lsp{hover} 确认相邻类型 → file_edit 改
   → 语言服务器 publishDiagnostics → perceive 推 "src/x.rs:47 error: trait bound 不满足"
   → AI 当回合修(无需等 cargo build 几十秒)
```

## 接原理

`设计原理/00-工具体系扩展`:推论1(自建核心代码智能、诊断接感知层)+ 推论4(补"看"与"验"两环)+ 推论5(分层降级、感知当前层)。
诊断推送接 `内部状态感知.md` / `感知告知-双受众.md`。

## 里程碑与风险

- **M1**:rust-analyzer + hover / definition / references(GrowBox 自身开发即吃到回报)。
- **M2**:诊断推感知层(接内部状态感知)——编辑期失败自我感知。
- **M3**:tsserver(SolidJS/TS)+ 调用层级(incoming/outgoing,改函数前看影响面)。
- **M4**:tree-sitter 第2层(自定义语言 / 无服务器项目的结构兜底,见 `设计原理/00` 推论5)。
- **风险 / 已知坑**:
  - ★**LSP 有状态**★:查一个文件前**必须先 `didOpen`** 把内容同步给服务器、改动随 `didChange` 跟进,否则查不到或查到旧状态。这是最容易踩的坑——库给传输,同步时机得自己管。
  - **行列体系**:LSP 协议内部 0-based,工具对外 1-based 与 `file_read` 对齐——转换处别错。
  - **首次索引慢**(大仓库)→ 懒起 + 超时 + 进度感知(别在索引完成前误判"无结果")。
  - **打包语言服务器二进制**(体积/平台 → 内置仅自身栈 rust-analyzer/tsserver,其它探测系统已装)。
