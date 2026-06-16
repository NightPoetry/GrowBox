# memory(growbox-memory)—— 记忆系统(存取 + 分层检索 + 精确层飞轮)

> 本系统是"AI 记忆置换系统"(类 OS 虚拟内存)的落地处。thesis:**持久 agent 记忆无界,不可能全量进内存**——故正文与向量都磁盘原生,RAM 只留工作集。真理来源 `记忆置换系统-总纲.md` + `设计/02`。

## 模块与实际设计

### store.rs —— 持久化(redb 单文件)
- `redb` 单文件库 `growbox.redb`,五张表:`nodes`(节点,含向量+role)、`conclusions`(结论)、`kv`(settings/projects 等杂项)、`edges`(指针边)、`jumps`(强制跳转指针,阶段4)。
- `jumps` 与 `edges` 同布局(键 `source\0target`、按 source 前缀读),但语义=位置键、无 topic、持久不衰减(用户引用历史时钉);`put_jump/jumps/remove_jump/jump_count`。
- 边键 `source\0target`,`neighbors(source)` 按 `source\0` 前缀范围扫一个邻域(B 树局部读,全图不整载)。
- 点读:`load_node(id)`(P3d 惰性取正文);`load_node_vectors()`(流式 (id,向量) 喂第一层索引,不带正文)。
- write-through:摄入/补向量/染色后即时落库。

### timeline.rs —— 时间线(P3d 磁盘原生 + 惰性内容)
- RAM 常驻只有 **`NodeMeta`**(id/created_at/role/hits/stain/embedding_version/has_embedding)——轻。
- 正文 `content` 与 `embedding` **不常驻**:按 id 惰性从 Store 取,前面挡有界 LRU 热尾缓存 **`NodeCache`**(`CACHE_CAP=512`)。无 Store(测试)时缓存无界、即权威全量。
- `get(id)` 返回 owned `Node`(惰性装配:缓存/盘取 content+向量,叠加 RAM 内 meta 的权威字段);`metas()` / `meta(id)` / `content(id)` 供遍历与按需取。

### pointer.rs + edges —— 指针网(磁盘原生 mesh)
- 精确层飞轮的原子:节点→节点有向图(走过的路)。**网状非平铺**(靠少量入口进图、沿出边局部跳转,只 judge 前沿)。
- 真相在 `store` 的 EDGES 表(磁盘原生,按 source 邻域读);`PointerNet`(内存邻接)仅作无 store 测试兜底。边带 `topic`(建边时 query 向量,决定该不该走这条快车道)+ `heat`。

### cache.rs —— 邻域缓存(平铺单 LFU)
- `NeighborCache`:磁盘指针图之上的有界 RAM 工作集(HashMap,source→出边),LFU 按 heat 淘汰最冷,查找 O(1);命中率/淘汰指标喂 P5 疲劳度 + P6 面板(`cache_stats()` 返回 占用/容量/命中率/淘汰数)。
- **退役"三级 1:2:4 分层视图"(2026-06-02)**:CPU 缓存比喻带来的设计惯性,HashMap 已 O(1)、真分层零收益;概念与代码统一为平铺单缓存,容量可设(`00-交互层` 推论9,默认 256)。

### index.rs —— 第一层向量索引(可换引擎 seam)
- **`VectorIndex` trait**:`rebuild` / `search`(返回 (id, cosine 相似度) 降序) / `len`。换引擎不碰 `Memory::retrieve`。
- **`ArroyIndex`(运行时默认,有 Store 时)** —— arroy 0.6.4 + heed/LMDB,**磁盘原生 mmap,向量不全驻进程 RAM**。`open(dir)` 建 LMDB env(map_size 1 GiB);`rebuild`=clear+逐条 add_item+builder.build(当前全量重建);`search`=Reader.nns(k).by_vector,search_k 放大优先召回。arroy Cosine 距离 d=(1-cos)/2 → 还原 cos=1-2d(统一标度)。ItemId(u32)↔node id 用内存 `Vec<String>`。
- `HnswIndex`(instant-distance,内存 HNSW)= 无 Store/测试用;`BruteForceIndex` = 兜底/测试。

### subconscious.rs —— 潜意识接口
- `cosine` 工具 + `Subconscious` trait(embed_query/embed_passage/judge_relevant/embedding_version),由 gui 的 bridge 接到真 LLM/Embedder;测试用 mock。

### memory.rs —— 统一入口 + 分层检索
- `Memory::new()`(纯内存,HnswIndex)/ `Memory::open(store, index_dir)`(磁盘:时间线惰性化 + ArroyIndex,打开失败回退 HNSW)。
- `retrieve`:第一层 RAG 走 `index.search`,`RAG_HIT_THRESHOLD=0.85`(按 e5 分布定)命中即返回;否则下沉。
- `retrieve_exact`:① 入口=index top-K(`ENTRY_K=3`,`ENTRY_MIN_SIM=0.30`)→ ② 沿出边 topic 匹配(`POINTER_FOLLOW_THRESHOLD=0.80`)跳前沿,judge 命中染 Light 短路 → ③ 线性扫近端非 Deep 窗口(`SCAN_BATCH=8`),命中建边让网生长、染 Deep。
- **P4 上下文组装**:持一个 `ContextWindow`(见 context.rs)。`assemble_context(query, sub)`=跑 `retrieve` → 命中按指针调入工作集(两态)+ 尾部 8K ring,产出 llm 无关的 `ContextBlock` 序列(工作区 + ring,各带完整时间戳)。`configure_context(working, ring)` 由 gui `connect()` 按设置应用(P4d)。

### context.rs —— 上下文组装层(P4,记忆置换"换入上下文")
- **llm 无关**:产出抽象 `ContextBlock { region(Working/RecentRing), node_id, role, timestamp, content }`,由 gui 套区标记转 ChatMessage。置换策略在此单测。
- **`ContextWindow`**(跨回合存活,放在 `Memory` 里):`resident: Vec<ResidentBlock>` 按调入顺序(append-only → 稳定前缀,命中 prompt 缓存)。
  - `page_in`:已在=只刷 heat 不挪位不重复(**两态铁律**);不在=append 末尾再按预算淘汰。
  - `enforce_budget`:超 `working_budget_chars` 淘汰最冷(heat 最小,并列淘汰最早),幸存者不重排。预算单位是 UTF-8 字节(token 保守上界,非精确 tokenizer)。
  - `working_blocks(exclude)`:输出工作区块,排除已被 ring 覆盖的 id(不重复拼)。
  - `set_budgets` / `suggest_working_chars(model_tokens)`(按模型窗口推算建议默认,留 35% 余量)= P4d 尺寸随模型/用户可调。
- 默认预算 `DEFAULT_WORKING_CHARS=48000` / `DEFAULT_RING_CHARS=8000`;`Settings.working_context_chars/recent_ring_chars`(0=默认)覆盖。
- 已知限制见 `计划/P4-上下文组装层.md`(预算分区独立非整窗 / 字节非精确 token / 中段淘汰破缓存 / supersede 残留)。

### fragments.rs + 维护方法 —— 精确层债务处理(P5,做梦/睡眠/疲劳)
- **`fragments.rs` `FragmentLedger`**:有界 RAM **瞬态**台账(FIFO 淘汰最旧、去重),记 mesh 跳转跳过的中段债 `Fragment{entry,target,query,topic}`。`retrieve_exact` 走快车道命中即记一笔。瞬态(重启清零)与缓存同性质——丢了下次检索再生成,无损正确性。
- **`Memory` 维护方法**(都 llm 无关、mock 可单测):
  - `fatigue()` → 0~1:`0.4·(1-命中率) + 0.2·淘汰压力 + 0.4·碎片占比`,无检索活动=0(新系统不累)。是"要不要睡"的启发式信号。
  - `dream_once(sub)` → `DreamReport{processed,discoveries,drained}`:取一笔债 → 复查 entry..target 中段非 Deep 节点 → judge 遗漏 → 相关的补 entry→节点 边(入索引)+ 染 Deep → 清债。
  - `rehearse_once(sub)` → bool:取近端节点原文当预演 query 跑精确层(预热网/建边/生新碎片)。
  - `sleep(sub,max_cycles)` → `SleepReport`:做梦+推演交替,碎片归零后推演无题/到上限即止(有界,防"推演生债→做梦还债"无限循环)。
  - `nap()`:擦黑板不格盘——清 ContextWindow + NeighborCache + FragmentLedger,留时间线/结论/EDGES/索引。
- **诚实记录**:做梦 discoveries>0 是**长程**现象——append-only + 近端线性扫(SCAN_BATCH=8)会把近端中段全染 Deep,只有越过"曾 Deep 又长出的 Light/None"长程区才有遗漏可捞;mesh 每次命中都记债(含空中段),做梦对空中段也正确清账(0 discoveries),台账有界不爆。**仲裁器在 gui 不在此**(它协调 gui 层 worker,见 gui 设计文档 + `arbiter.rs`)。

### secondary.rs + jumps —— 二级索引 + 强制跳转(精确层阶段4,Opus 2026-06-01)
- **强制跳转指针(历史引用,位置键)**:`Memory::pin_history_reference(from,target)` 在 from(缺省=当前位置)钉 source→target;`forced_jumps_of`/`forced_jump_count`。有 store 走 `JUMPS` 表(持久),无 store 内存 HashMap 兜底。`retrieve_exact` 步骤1.5:入口落到带跳转的位置 → **无条件**召回目标(无 judge、无 topic 门,遍历到此必跳),染 Light。
- **二级索引(远处拉近)**:`secondary.rs` `SecondaryIndex` —— 有界 RAM **瞬态**(同缓存/碎片性质,可再生),容量满淘汰最冷(LFU by heat)。K/2K 规则(`WINDOW=8`,`K=2`):漂移 <K 不建(回近端自动撤锚);K≤漂移<2K 建 level1(指原始位置);≥2K 粗化 level2(改指一级索引)并刷新到当前前沿;同 target 至多一条锚点=**永远只两层**。`retrieve_exact` 步骤2 把 `pull_targets(N=4)` 最热远端注入前沿——当前入口 topic 边够不着也凭锚到前沿的二级锚点召回;命中即 bump+记碎片债+register 刷新。`nap()` 清二级(工作集),强制跳转持久不清。

## 用的库
redb 4、**arroy 0.6.4**、**heed 0.22.1**、rand 0.8、instant-distance 0.6.1、sha2、serde/serde_json、async-trait。
**为什么 arroy 而非 hannoy**:同 Meilisearch 团队/同 LMDB 家族/同样磁盘原生,但 arroy 更成熟(0.6.x、39 万下载、多年生产、版本干净),hannoy 更快但仍 0.1.x;按"依赖须知名成熟"硬规矩选 arroy,seam 留着日后可换。见 `决策日志.md` 2026-06-01 + 记忆 `prefer-well-known-verified-deps`。

## 现状(P3 + P4 + P5 + 精确层阶段4 全完成)
memory crate **65 单测绿**(含 arroy 落盘/重开 + context 6 + 组装集成 2 + P5 碎片/做梦/疲劳/睡眠/nap 8 + 阶段4:secondary 单测5 / store jumps / 强制跳转召回·持久·拒绝 / 二级远处拉近端到端 / nap 清二级留强制)。正文(P3d)+ 向量(arroy)两半磁盘原生 → "无界不全驻 RAM"达成;P4 上下文组装层后端 + P5 维护 + **阶段4 二级索引(远处拉近)+ 强制跳转指针(历史引用)**全落地。精确层五件套(指针/染色/缓存/二级索引+碎片/维护)全齐。
**待优化**:`ArroyIndex::rebuild` 改增量 upsert/del(arroy 有 `add_item`/`del_item`),免每次开机全量重建(单用户规模当前可接受)。
**续点**:四级=打包就绪(图标集/e5 随包/版本号,需用户拍板);精确层无欠债。
**原文三层置换**(总纲)= 已由 P3d 惰性时间线(冷归档=在盘不在工作集)+ ContextWindow 工作集淘汰共同满足,无需另造存储层。
