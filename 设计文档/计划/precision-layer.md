# 精确层飞轮 实现计划

> 目标:把记忆第二层(精确检索)从"只会线性扫最近 8 个 + 染 Deep"加厚成 `设计/02` 的自加速飞轮——每次扫描的副产品沉淀成结构,越用越快。
> 真理来源:`设计/02-记忆检索`(原则2 + 五件套表)、`系统架构/04-memory`。

## 现状(起点)
- Timeline:节点带 embedding / hits / stain(`None`/`Light`/`Deep`,但 `Light` 从未被写)。
- 检索:RAG 暴力余弦(第一层)→ 下沉 `retrieve_exact`(只取最近 `SCAN_BATCH=8` 个非 Deep,judge 一批,全染 Deep)。
- 缺:指针、三级缓存、二级索引/碎片、强制跳转指针;做梦/睡眠/疲劳(maintain 空)。

## 五件套与本计划阶段的对应
| 设计五件套 | 阶段 |
|---|---|
| 指针(原子,source→target,走过的路) | 1 |
| 缓存(平铺单 LFU,按 heat 淘汰) | 2 |
| 染色(深绿/浅绿/无色) | 3(补 Light) |
| 二级索引 / 碎片(远处拉近 + 欠债) | 4 |
| 维护:做梦 / 睡眠 / 疲劳 + 潜意识仲裁器 | 5 |

## 阶段 1:指针网络(飞轮的原子)[已完成 Opus 2026-05-31]
落地:`pointer.rs`(Pointer/PointerNet)+ `retrieve_exact` 改两步(指针先行→线性兜底+沉淀)+ KV 持久化。memory crate 23 测试绿,全工作区 108 绿。
**一个有意识的取舍(待用户确认)**:指针的键用 **query 向量(topic,语义)** 而非 source 节点位置(空间)。理由:检索由提问驱动,"一个成功找到目标的过往提问"比"当时在时间线哪个位置"更能预测"下次相似提问要什么",也正好补 RAG(用目标原文向量比)漏掉的召回。设计表里"强制跳转指针(历史引用)"才是位置键的(遍历到此必跳),归阶段 4,与本阶段的语义键指针是两种东西。
另:走指针命中目标后 judge 一次确认(不盲信,符合 `设计/04`"结论=猜想,要验证"),代价 1 次 judge,仍远小于线性扫整批。顺带修了原实现一个潜在召回 bug:节点被染 Deep 后在重复 query 里会"消失"(线性扫过滤掉 Deep),现在经指针仍可召回。

**原理**:扫到相关 → 在"当前位置"建一条指向该历史节点的捷径;键 = 建指针时的 query 向量(一个成功找到目标的 query,是比目标原文更好的索引键)。下次相似 query 先走指针,直接跳到目标,省掉线性扫。这一步就让"扫描越用越快"成立。

- 新 `pointer.rs`:`Pointer { target: String, topic: Vec<f32>, heat: u32 }` + `PointerNet`(best_match by cosine、sediment 去重、bump 热度)。
- `retrieve_exact` 改:embed(query) → 先查指针(cosine(qv, topic) ≥ 阈值 → judge 目标 1 次 → 命中则 bump + 染 Light + 短路返回);未命中才线性扫,线性命中时 sediment 一条新指针。
- 持久化:PointerNet 走 `store.kv_put("pointer_net")` write-through(规模小,整体 blob;未来上量再拆表)。
- 测试:同类 query 第二次经指针命中,judge 调用数显著少于第一次线性扫;且能命中 RAG 漏掉的目标。

## 阶段 2:邻域缓存(平铺单 LFU)
- 磁盘指针图之上的有界 RAM 工作集:`NeighborCache`(HashMap,source→出边),按 heat 淘汰最冷,查找 O(1),超容淘汰;miss 回盘读。容量可在控制面板设置(默认 256,见 `00-交互层` 推论9)。
- 采集元指标:缓存命中率、淘汰次数(给阶段 5 的疲劳度用)。
- **退役"三级 1:2:4"(2026-06-02)**:早期借 CPU 缓存 L1/L2/L3 比喻写过"三级",但 HashMap 已 O(1)、真分层零收益,"最热级先查"退化为纯展示——属设计惯性。概念与代码统一为**平铺单 LFU**,面板只显 占用/容量/命中率/淘汰数。

## 阶段 3:染色三色完善
- 走指针的快扫染 `Light`(可能漏),线性细扫染 `Deep`(确信)。全量扫描跳 Deep、重查 Light。把 enum 里闲置的 `Light` 接上。

## 阶段 4:二级索引 + 碎片(欠债记账)[已完成:碎片=P5;二级索引+强制跳转=Opus 2026-06-01]
- 走指针跳过的中段 = 碎片 = 债,记录区间。[P5 已落:`fragments.rs` + 做梦消费]
- 缓存条目漂离当前 K 个窗口 → 建二级锚点拉回(只保留一级/二级两层,中间空白即碎片)。

### as-built(二级索引 + 强制跳转,Opus 2026-06-01)
精确层阶段4 的"索引那半"(碎片那半 P5 已落)。两件:

**强制跳转指针(历史引用,位置键)** —— `设计/02` 五件套末行。
- 磁盘原生:`store.rs` 新增 `JUMPS` 表(键 `source\0target`,值=target),与 EDGES 同布局(前缀读一个邻域),
  但语义=**位置键、无 topic 门、不随热度衰减、持久落库**(用户断言的真相)。`put_jump/jumps/remove_jump/jump_count`。
- `Memory::pin_history_reference(from, target)`:用户引用历史 → 在 from(缺省=当前位置)钉一条 source→target;
  拒绝自指/不存在目标。无 store 时内存 `HashMap` 兜底。`forced_jump_count()` 观测。
- `retrieve_exact` 步骤1.5:入口落到带强制跳转的位置 → **无条件**召回其目标(无需 judge、无 topic 门,
  "遍历到此必跳"),染 Light。无前沿但有强制命中时直接返回。
- gui:`reference_history` 命令(main.rs 注册);面板 `secondary_indexes.forced_jumps` 接真。

**二级索引(远处拉近)** —— `设计/02` 五件套"二级索引/碎片"的索引半。
- `secondary.rs` `SecondaryIndex`:有界 RAM 瞬态(同缓存/碎片性质,可再生,真相在磁盘 mesh),
  容量满淘汰最冷(LFU by heat)。`Anchor{target,l1_source,anchored_at_len,heat,level}`。
- K/2K 规则(`register(target,l1_source,windows_away,current_len)`,WINDOW=SCAN_BATCH=8,K=2):
  漂移 <K 不建(回近端自动撤锚);K≤漂移<2K 建 level1(指原始位置)拉到当前前沿;≥2K 粗化 level2
  (改指一级索引)并刷新前沿。同 target 至多一条锚点 = **永远只两层**。
- `retrieve_exact` 步骤2:topic 前沿之外,把 `pull_targets(N=4)` 最热远端 target 注入前沿——
  即便当前 RAG 入口的 topic 边够不着它(原始边根在漂远的旧 source),凭锚到前沿的二级锚点仍召回;
  命中即 `bump`、记碎片债(中段)、`register` 刷新。命中远端 mesh target 时也 register。
- `nap()` 清二级索引(工作集);强制跳转持久不清。面板 `secondary_indexes.total` 接真。
- 测试:memory 65(+10:secondary 单测5 / store jumps / 强制跳转召回+持久+拒绝 / 二级远处拉近端到端 / nap 清二级留强制)。
  端到端"远处拉近"用 2*WINDOW 填充把 target 推远、再用落在填充区的 query(入口无边)验证仍召回。

## 阶段 5:做梦 / 睡眠 / 疲劳 + 潜意识仲裁器 [已完成 Opus 2026-06-01]
- `maintain()`:做梦扫碎片(judge 间隙 → 补索引 → 清碎片);睡眠 = 做梦 + 推演 交替。
- 疲劳度 = 加权(缓存命中率低 + 淘汰频繁 + 碎片占比大),非 CPU/内存。
- **潜意识 LLM 仲裁器**(地基三锤之二,此处落地):带优先级 Agent > Sleep > 飞轮 的调度槽,做梦/演练也要检索(见 `补遗/做梦睡眠期也在检索.md`),不仲裁会和前台 agent race。

### as-built(实际落地,Opus 2026-06-01)
- **碎片**:`growbox-memory/src/fragments.rs` `FragmentLedger`(有界 RAM 瞬态,去重 FIFO);`retrieve_exact` mesh 跳转直达 target 时把跳过中段记一笔债 {entry,target,query,topic}。瞬态(重启清零)=与缓存同性质,丢了无非下次再生成,无损正确性。
- **疲劳**:`Memory::fatigue()` = 0.4·(1-命中率) + 0.2·淘汰压力 + 0.4·碎片占比,无检索活动时为 0。
- **做梦**:`Memory::dream_once(sub)` 取一笔债 → 复查 entry..target 中段非 Deep 节点 → judge 遗漏 → 相关的补 entry→节点 边 + 染 Deep → 清债。`DreamReport`。
- **推演**:`Memory::rehearse_once(sub)` 取近端节点原文当预演 query 跑精确层(预热网/建边/生新碎片)。
- **睡眠**:`Memory::sleep(sub,max_cycles)` 做梦+推演交替,碎片归零后推演无题/到上限即止。`SleepReport`。gui `idle.rs` 用 dream_once/rehearse_once 编排可取消版(疲劳≥0.5或有债才睡,逐步让位)。
- **小息(Nap)**:`Memory::nap()` 清工作集+三级缓存+碎片台账,留长期记忆;gui `nap` 命令。
- **仲裁器**:`growbox-gui/src/arbiter.rs`(不在 memory:它协调 gui 层 worker)——容量1优先级互斥闸 Agent>Sleep>Flywheel,取消安全。`run_chat` 取 Agent 档整回合,`idle.rs` 睡眠取 Sleep、飞轮取 Flywheel。补遗硬前置件已满足。
- **仍欠**:本阶段做了"碎片记账 + 做梦消费碎片";**阶段4 的二级索引/远处拉近 + 强制跳转指针(位置键)未做**,留作精确层后续欠债。做梦 discoveries 主要是长程现象(见 `AI记忆快照/precision-layer-progress.md` 诚实记录)。

## 存储:磁盘原生(贯穿全程的硬约束,用户 2026-05-31 指出)
持久 agent 记忆无界增长,**不可能全量进内存**。指针网必须磁盘原生:
- **边按 source 落 redb**:键 `source_id\0target_id`。取"节点 N 的出边"= 一次 `source_id\0` 前缀范围扫(B-树一次局部读),不碰其他边,全图永不整体载入。
- **从当前位置往外导航**:精确层入口=对话当前位置(永远热),读其出边→跳目标→读目标出边……每跳几次按键读,只 page-in 实际走过的邻域。不需要全局索引找入口(那是第一层 RAG/ANN 的事,另一条线)。
- **三级缓存(阶段2)= 磁盘图之上的 RAM 工作集**:热 source 邻域留内存,按 heat 淘汰,miss 回盘读。所以"缓存"和"指针落盘"是一回事,阶段1存储与阶段2应合并做。内存占用=工作集,与总记忆量解耦。
- 时间线本身也要从"开机全量 load 进 Vec"改成按 id 惰性取 + 热尾缓存(牵动 RAG 暴力余弦与历史面板,工作量大,可单独一步)。
- **当前实现是内存版 mesh(`HashMap` 邻接 + `Vec<Node>` 全驻)**:导航逻辑(入口→出边→前沿)正确、可留作缓存层逻辑,但背后要换成 redb 边表 + 按需读邻域 + 有界缓存。

## 顺序与原则
严格按 1→5(下层是上层地基:无指针则缓存/二级索引无物可缓存/锚定)。每阶段 `cargo test -p growbox-memory` 全绿后再进下一阶段。有疑问用 mock Subconscious 做实验验证,不照搬文档措辞。
