# process kind 落地(项目级流程的存储与检索)

> 把 `01-流程与检索架构.md` 落到 GrowBox 真实代码。**核心结论:几乎不需要新基础设施**——
> 写入复用 `ingest_with_role`,检索复用 `retrieve`(role 自动入索引),学习复用整套指针 API(target 换成流程 id),
> 可执行档复用工作流栈。代码触点见末表(全部已核对 file:line)。

## 范围

只定"流程作为记忆 kind 怎么存、怎么检索、可执行档怎么连工作流"的数据模型与代码触点。
不重做记忆/指针/工作流引擎(全复用一期)。

## 方案

**一条流程 = 一个 `role="process"` 的记忆节点**(检索单元)。分两档:
- **建议档**:`content` = 配方原文("在本项目做 X = 碰 A→B→C")。无工作流链接。
- **可执行档**:`content` = 简述 + 一行 `wf: <工作流名>`(执行体在 `WorkflowStore`,scope=project,P3 按项目落 redb)。

**存储**:`Memory::ingest_with_role(content, node_kind::PROCESS)`(`memory/mod.rs:456`);
embedding 由 `ensure_embeddings` 异步补(idle,`retrieval.rs:8`)。可执行档的工作流体经 `define_workflow` /
`WorkflowStore::define`(`workflow_store.rs:61`,P3 自动落 redb `wf_project::<id>`)。

**检索**:复用 `Memory::retrieve(query, sub) -> (Vec<Hit>, Layer)`(`retrieval.rs:34`,RAG→精确层)——
`role` 随 `Node` 自动入索引,**检索引擎无需改**。流程"越用越准"复用学习型指针 `PointerNet`(`pointer.rs`):
命中记正 K、judge 拒记反 K、近似坍缩 + LFU 防膨胀、反 K 一票否决——**指针 API 原样可用,无需扩**
(已核对:`target` 换成流程节点 id 即可)。给流程一套独立 `PointerConfig`(同记忆"一切数值可设")。

**消费(两档分流)**:
- 建议档命中 → 注入上下文给 AI **读、照做**(像召回一条结论)。
- 可执行档命中 → 取 `content` 的 `wf:` 名 → **物化**:`Registry::workflow_defs()`(`registry.rs:113`)
  按这批名字过滤,只把命中的工作流签名 **append** 进工具列表(不改前缀,保缓存)→ AI **栈调用**
  (`WfFrame` 机制已就位,`agent/mod.rs:410-723`)。

**结晶(写入)**:飞轮把"复发的报告-纠正序列"压成一条 process 节点;够稳够复杂 → 经 `define_workflow`
升格出工作流体、并在 process 节点 `content` 标 `wf:` 名。

## 数据模型(process 节点 content 约定)

```
建议档:
  role = process
  content = "【加数值设置】本项目加一个数值设置碰:core/project.rs Settings 字段 →
             cmds/config.rs set/get_misc_config → tauri-api.ts → settings/state.ts 信号 →
             Settings.tsx 回显 → AgentTab 控件 → i18n.ts + locales/{en,ja,zh-TW}.ts。
             顺序:后端 → 前端 → 四国 i18n。"

可执行档:
  role = process
  content = "【出测试包】重建前端 dist → 仓库根 cargo tauri build → 验证 .app 时间戳。\nwf: build_test_package"
  (执行体 build_test_package 在 WorkflowStore,scope=project,P3 落 redb wf_project::<id>)
```

> `wf:` 单行约定先用最简形态;若脆,M3 改成节点 content 存结构化 JSON 或单独 kv 映射(届时再定)。

## 代码触点(最小改动,已核对 file:line)

| 触点 | 文件:行 | 改法 |
|------|---------|------|
| 注册 kind | `node_kind.rs:14-33` | 加 `PROCESS="process"` const + `controlled()` 表 + `label()` 中英分支(非破坏) |
| 写入 | `memory/mod.rs:456` | 调用 `ingest_with_role(recipe, PROCESS)`,**函数无需改** |
| 检索 | `retrieval.rs:34` | **无需改**(role 自动索引);如要"只检索 process"可加可选 kind 过滤参数 |
| 指针复用 | `pointer.rs` / `mod.rs:337-377` | 新建一张 process `PointerNet`(或复用主网,target=流程 id);API 原样 |
| 物化过滤 | `registry.rs:113` `workflow_defs` | 加可选"允许名单"参数,据检索结果过滤(与懒加载/MCP 共用此过滤) |
| 升格 | `define_workflow.rs` / `workflow_store.rs:61` | 复用,无需改 |
| 结晶 | 飞轮(`learn` / IdleWorker) | 报告-纠正序列 → 写 process 节点;复发够多 → 升格工作流 |

## 接原理

兑现 `设计原理/01-流程即一等公民`:推论3(流程即记忆 kind,检索复用 RAG + 指针)+ 推论1(两档承载项目涟漪面)+
推论4(结晶/合并落飞轮 consolidation/prune)。复用 `设计/02-记忆检索`、`设计/07-工作流机制`。

## 里程碑与风险

- **M1 建议档(今天就能做,只依赖记忆系统)**:加 `PROCESS` kind + `ingest_with_role` 写入 + `retrieve` 召回注入。
- **M2 指针接通**:process 命中记正 K / judge 拒记反 K(复用 `retrieval.rs:242` 的边学习,或独立 process 网)→ 越用越准。
- **M3 可执行档**:`wf:` 约定 + `registry.workflow_defs` 按需物化过滤 + 升格路径(依赖懒加载 C1 的过滤机制)。
- **风险**:过时(靠正/反 K + 用前先验,同记忆陈旧校验);过度结晶(靠推论4"复发才结晶");`wf:` 约定脆(M3 转结构化)。
