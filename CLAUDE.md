# GrowBox -- AI 指导手册

> **来源/信任(2026-06-01 起由 Opus 接管维护)**:本文件顶部"接手前先读"与凡标注 `[Opus]` / 指向 `交接报告.md 0-OPUS4` / `设计文档/AI记忆快照/` 的内容,是**经实测验证的当前可信信息,直接用**。
> 下方未更新的旧"现状/构建"描述(测试数、阶段进度)出自 DeepSeek V4 / 早期会话,**可能过时,以 `交接报告.md` 顶部 0-OPUS4 横幅 + `设计文档/AI记忆快照/precision-layer-progress.md` 为准**。DeepSeek 时代的技术结论仍按铁律 3 需独立验证;Opus 已验证的结论不在此列。

> **本项目正在彻底重构中。** 接手前先读:
> 1. `交接报告.md` — 浓缩版(顶部 `0-OPUS4` 横幅 = 最新进度 + 续点 + 锁定决策)
> 2. `设计文档/AI记忆快照/` — **会话的记忆副本**(先读其 `MEMORY.md` 索引),含项目认识、用户决策、**保命协议(`lifeline-protocol.md`)** 等。**★该目录连同 `History/`、`用户决策/决策日志.md`、`交接报告.md` 现已收进加密保险箱 `private/docs.tar.gz.enc`(明文不入 git);先跑 `scripts/private-docs.sh unpack`(需仓库根 `.private-docs.key`)还原明文再读,详见 `private/README.md`。★**
> 3. 真理来源 = `设计文档/`(`记忆置换系统-总纲.md` / `用户决策/决策日志.md` / `跨平台迁移方案.md` / `打包设计.md` / `异常告知.md`)。旧代码已清理。
> 4. 旧 DeepSeek 完整记录(谨慎/需独立验证,已归档):`过时文档/DeepseekV4Pro交接文档/2026-05-30_第四轮完整交接.md`。
> 前端是 **SolidJS**(不是 React),改 UI 注意。
>
> **"保命"= 把当前状态固化进交接/记忆让新会话无缝接续**(详见 `设计文档/AI记忆快照/lifeline-protocol.md`):触发=用户说"开始保命" / 近 token 上限 / **停下来让用户做选择前**。

## 现状(2026-05-31 Opus 夯地基)

> ⚠️ 本节数字(测试数/阶段)停在 2026-05-31,**已过时**。当前真实状态以 `交接报告.md` 顶部 **0-OPUS4** 横幅为准(2026-06-01:嵌入/异常告知/指针磁盘原生 + 第一层 ANN 已落地,全工作区绿)。下文留作背景。

后端全部重写完成,新架构 6 个 crate **全部落地**(core/llm/safety/memory/learn/gui),**`cargo test --workspace` = 102 全绿**(`--lib` 101);`cargo tauri build --debug` 可出包。
**记忆持久化已落地**:redb 单文件库,settings/projects/对话时间线/结论全部 write-through。
**Agent 基本盘已落地**(Qwen):后台任务三工具 + 常驻 Supervisor + 飞轮 idle 学习闭环 + launch.sh 修复。详见 `设计文档/计划/agent-basics.md`。
**地基加固(Opus 2026-05-31)**:
- 前端 v1 清理收尾:audit 残留清完,tsc/build/cargo 三绿(Qwen 那轮只跑 Rust 测试漏了前端编译失败)。
- **`Executor::execute` 改 async**:后台任务三工具从"脊柱按名拦截 + 自己重写安全门"回归普通执行器,分发路径与安全门各收回唯一一处(架构公理复位)。agent.rs 967→719。详见 `系统架构/01-core` 已知坑。
- 三退出点用 `finalize()` 收口,学习行为一致。

**精确层飞轮加厚(Opus 2026-05-31,进行中)**:计划 `设计文档/计划/precision-layer.md` + `embedding-service.md`。阶段 1 指针网已落**内存版 mesh**(`growbox-memory/src/pointer.rs`,全工作区 109 绿)。设计评审锁定三件事(★见记忆 precision-layer-progress):①指针网状非平铺 ②磁盘原生非全驻内存(边按 source 落 redb,内存版是临时)③RAG 用真 embedding(本地默认 `multilingual-e5-small`/candle + 远程 OpenAI 兼容槽)。
**下一步(用户等额度刷新后做)**:先做 Embedding(脚手架+版本重嵌 → 远程实现 → candle 本地 e5-small → UI 嵌入槽 → 真机验证同义召回),再做指针磁盘化。

剩余:Embedding、精确层阶段 2-5(磁盘化/缓存/碎片/做梦睡眠疲劳+仲裁器)、健康监控接真数据、v1 面板接厚。

## 铁律(用户已明确)

1. 除前端交互设计外,**所有非 UI 结构全部重写**;旧代码不作实现依据。
2. CLI 已砍,只留 GUI。
3. **有疑问自己做实验验证**(有真 API key,见交接报告 §6);不继承旧 AI 结论(很多是错的)。
4. 用户要能跑的产品,不懂 Rust 细节--技术决策自己定 + 自测,别拿 trait/类型烦他。
5. 禁止 Emoji(文档/提示词皆是)。
6. 只在「要 key / 要授权 / 要看成果」时找用户;中途别频繁打断。
7. **永远不要动 `/Volumes/UserData/Projects/Helper/GrowBox_DeepseekV4Byopus/` 目录**--那是 Opus 4.8 构建的原始项目,只读参考,禁止任何写入/删除操作。

## 构建与测试

```bash
# 单 crate / 全工作区单元测试
cargo test -p growbox-core         # 也可 -p growbox-llm / -safety / -memory / -gui
cargo test --workspace             # 当前 108 全绿(含 e2e 编译;精确层阶段1后)

# 真机 API 测试(验证与 deepseek-v4-flash 的真实通信)
DEEPSEEK_API_KEY=<key> cargo test -p growbox-llm --test live_deepseek -- --ignored --nocapture
DEEPSEEK_API_KEY=<key> cargo test -p growbox-gui --test live_agent -- --ignored --nocapture

# 出包(两种,脚本一键,前后端开关已绑死一致):
scripts/build-official.sh          # 正式包:无调试桥、无 127.0.0.1:19999 端口、无 debug IPC
scripts/build-test.sh              # 测试包:带 window.__GROWBOX__ + :19999 + debug_eval/e2e_report
# 末尾可透传 cargo 参数,如 scripts/build-official.sh --debug
```

调试能力**集中在一处、按 feature 开关**(不再散落):
- 后端全在 `crates/growbox-gui/src/debug.rs`,由 cargo feature `debug-endpoints` 开关。
- 前端调试桥 `frontend/src/growbox-debug.ts` + App.tsx 钩子,由 Vite env `VITE_GROWBOX_DEBUG` 开关(正式构建被摇树删除)。
- 测试包跑 e2e:`cargo test -p growbox-gui --features debug-endpoints --test e2e_ui_debug`。

### 出包/启动 已知坑

- **★最容易踩(2026-06-03 Opus 又踩了)★ `cargo tauri build` 必须在仓库根(或 `crates/growbox-gui`)跑,绝不在 `crates/growbox-gui/frontend` 子目录** —— 否则报 `Couldn't recognize the current folder as a Tauri project`(它只在当前目录及子目录找 `tauri.conf.json`),而且 **.app 不会更新、你却以为更新了**(旧二进制还在、时间戳是旧的)。正确两步:① `cd crates/growbox-gui/frontend && npm run build` 重建前端 dist;② **回到仓库根** `cargo tauri build --debug --bundles app`。或直接用 `scripts/build-test.sh` / `build-official.sh`(目录与开关已绑死,免踩)。
- `tauri.conf.json` 的 `beforeBuildCommand` 是空 → `tauri build` **不会重建前端**。改了前端必须先手动 `cd crates/growbox-gui/frontend && npm run build`,否则旧 dist 被嵌进包。
- `scripts/launch.sh` 已修复(Qwen 2026-05-31):二进制名改 `growbox`,加前端重建步骤。

## 新架构(6 crate,单向依赖)

```
app(gui) -- 依赖全部 -- Agent 循环脊柱 + 执行器注册表 + Tauri + 前端   [已实现]
├── memory   分层检索(RAG→精确)+ 精确层飞轮          [已实现]
├── learn    飞轮:收集→聚类压缩 + 永久目标调度         [已实现]
├── safety   沙箱/路径分级/风险/三种授权                [已实现]
├── llm      LLM 通信:流式/reasoning/工具解析          [已实现]
└── core     共享类型:结论模型 + 执行器 trait(零依赖)  [已实现]
```

架构公理:**一切能力皆"执行器",Agent 循环是唯一脊柱**(一个注册表、一条分发路径)。

新增模块:`crates/growbox-gui/src/tasks.rs` -- TaskManager,后台任务生命周期管理(进行中特性,见交接报告 §5A)。

## 关键实测(deepseek-v4-flash,详见 `设计文档/实验记录/00`)

- flash 是**推理模型**:返回先 `reasoning_content` 再 `content`,流式里分属不同 delta 字段,顺序 R→C。
- 工具调用正常(标准 OpenAI 格式,支持并行);流式 tool_calls 按 `index` 增量拼 arguments。
- "空参 `{}`" = max_tokens 太小、token 被 reasoning 吃光导致截断 → 判截断重试 + 给足 token,**不是**"空参提示"。
- 沉默超时要把 reasoning chunk 算作"有活动"。

## 文档体系(`设计文档/`)

- `设计/`(原理层,原则→推论→案例):00 交互 / 01 能力 / 02 记忆 / 03 安全 / 04 飞轮 / 05 工具
- `系统架构/`(工程层,六段):00 总览 / 01 core … 06 app
- `实验记录/`:00 deepseek-v4-flash / 01 整条循环端到端
- `二期项目/`(内核之外的扩展层,自带两层:`设计原理/` 原理 + `项目设计/` 落地):工具体系扩展(LSP 代码智能 / 代码搜索 / Web / MCP 客户端+懒加载)
