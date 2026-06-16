# 09 · Skill 系统(场景化知识 = 第四原语)

> 来源:用户 2026-06-11 方向(见 `交接报告.md` 0-OPUS34 横幅 + `AI记忆快照/skill-system-direction.md`;起因 = 网页调试窗「DOM 反向定位源码」难点,见 `计划/网页调试窗-可视化框选改源.md`)。
> 状态:**设计稿,关键岔路已附拍板建议,待用户确认后实现**(大改前对设计铁律)。

## 定位:四原语谱

GrowBox 已有三个能力原语,Skill 补上缺的第四个:

| 原语 | 回答什么 | 形态 |
|---|---|---|
| 工具(执行器) | 能做什么 | 原子能力,少而通用(`05`) |
| 工作流 | 按既定步骤怎么做 | 确定性程序:强制顺序 + 节点收窄(`07`) |
| MCP | 外部生态有什么 | 收编的外部工具(二期 D) |
| **Skill** | **某类场景怎么把事做好** | **场景化知识 / playbook:AI 用判断施展,可指挥前三者** |

前三者都是**机制**,Skill 是**知识**。机制定义可能性,知识指导选择。

## 原则

1. **知识与机制分离**。「某类场景该怎么做好」是知识不是机制:知识进记忆,机制进执行器与工作流。把场景知识硬编码成专用执行器 = 堆特例、永远堆不完、违背架构公理(执行器要少而通用)。
2. **Skill 长在学习型记忆内核里**。Skill = 受控 kind 的记忆节点,天生继承全部记忆基建(嵌入检索、指针网正反 K、近重复坍缩、飞轮结晶、项目软隔离);可人工策划、可 AI 自学、**越用越准**。业界常见做法是把这类 playbook 写成磁盘静态文件(人写死、不学习);GrowBox 的 Skill 是活的——这是从自身记忆内核原理推出的形态,不是外来移植。

## 推论

### 推论 1:不为场景造执行器(← 原则 1)
凡发现自己要写「React 专用 X」「Vue 专用 X」「EJS 专用 X」三连,那是知识在敲门——写一个 Skill,让 AI 带着它用**通用工具**(shell / file / code_search)施展。
- **案例**:网页调试反向定位。React/Vue/EJS 各有定位法,做成 3 个执行器则第 4 个框架永远缺;做成 1 个 Skill,新框架只是 playbook 里多一条分支——AI 自己改 Skill 即可,不用发版。这与「表格渲染不做成工具(展示层不是执行器)」是同一判断的两面:机制面收紧,知识面放开。

### 推论 2:Skill 选路,工作流跑路;两者构成结晶谱(← 原则 1 + 2)
工作流锁死顺序(可靠),Skill 引导判断(灵活),互补不竞争:playbook 里可以含「调某工作流」「为该场景 define_workflow」;一个 Skill 用熟、步骤定型后,确定性部分可结晶为工作流,Skill 退为「何时用 + 判断要点」。与系统压缩谱同构的**结晶谱**:

```
经验 → process(被动召回的流程记忆) → Skill(命名 playbook,主动可挑) → 工作流(确定性机制)
```

越靠右越硬、越确定;飞轮把内容沿谱向右推。
- **案例**:打包这件事走完了整条谱——先是反复踩坑的经验(在 frontend 子目录跑 tauri build 不更新 .app),结晶为知识(CLAUDE.md 已知坑条目),最终锁死为机制(`07` 案例:打包工作流强制顺序)。

### 推论 3:载体 = `skill` kind 记忆节点,不建独立子系统(← 原则 2)
受控 kind 表(`node_kind.rs`)加一个 `skill` 值(非破坏,旧值天然合法);一个 Skill = 一个节点,正文 = **名称 + 触发描述(一句话,何时用)+ playbook 正文**。复用时间线 / 嵌入 / 索引 / `project_id` 软隔离,**不建独立 SkillStore**——process(二期 B,`learn_process`/`crystallize_process`/`retrieve_processes`)已验证这条路,Skill 与 process 是同族两个 kind。
- **案例**:作用域不需要新机制——全局 Skill = `project_id` 空;项目 Skill = 打当前项目 tag,检索时 `project_boost` 软偏好(已落地,fc30215)。对比工作流当年要专门做三层持久化(P3),Skill 的节点本来就在 redb,作用域是白拿的。

### 推论 4:双触发 = 常驻清单(主动挑)+ 语义召回(被动浮现)(← 原则 2 + `05` 渐进披露)
- **清单**:系统提示常驻一段 `[可用 Skill] 名称: 触发描述`(每 Skill 一行,byte-stable),AI 判断场景匹配时主动加载正文。与 C1 懒加载工具的 `deferred_listing` 同构(描述常驻、正文按需),缓存安全已被 C1 实验证实(`二期项目/实验记录-C1懒加载与缓存.md`)。
- **召回**:Skill 节点有嵌入,语义检索可召回——AI 没想到挑时,相关 Skill 自己浮上来,兜住清单挤出/描述没写好的长尾。
- **案例**:用户在调试窗框选博客标题提修改建议 → 清单行「web-debug-source-locate: 网页调试窗框选后把 DOM 反向定位到本地源码」命中场景 → AI 调 `load_skill` 取 playbook 再动手。

### 推论 5:正文按需加载,append-only 回灌(← `05` 渐进披露 + 缓存铁证)
`load_skill` 执行器按名取正文,以**工具结果消息**回灌(与 `tool_search` 的 schema 回灌同构)——绝不塞系统提示中段、绝不动 tools 数组 → KV 缓存前缀稳。lazy_tools 开时 `load_skill` 入 `NEVER_DEFER` 常驻(枢纽不能把自己藏掉,与 `tool_search` 同理)。
- **案例**:C1 实验铁证——tools 数组一变,缓存命中 8448 → 0(整条 prompt 重算);append-only 回灌零缓存损失。Skill 正文走同一条安全路径:代价只是一次 `load_skill` 往返延迟。

### 推论 6:越用越准 = 正反 K 学习(← 原则 2)
镜像 `PROCESS_RECALL_SOURCE` 模式:`SKILL_RECALL_SOURCE` 哨兵源,边 = 哨兵 → Skill 节点。召回被采用 → 正 K 强化(该 query 族下次更容易召回);被 judge 拒 / 用户纠正 → 反 K 压制(一票否决)。学习型指针的全部统计机制(加权余弦、LFU 淘汰、近重复坍缩)零成本继承。
- **案例**:「反向定位」Skill 被「框选改源」「定位到源码」「改的是哪个文件」等不同措辞召回并采用,各成正 K;某次被「定位 bug 根因」误召回、judge 拒 → 反 K,同族 query 不再误触。

### 推论 7:策划与自学共存,同一条结晶回路(← 原则 2 + `04` 飞轮)
- **出厂**:内置种子 Skill(第一个 = 网页调试反向定位),随系统发布,作初始飞轮种子。
- **即时**:AI 在某场景摸索成功后主动 `learn_skill` 结晶;用户纠正后重新结晶,近重复坍缩取代旧版(反 K 压旧)——与 `learn_process` 的「报告-纠正结晶」是同一条回路,既建又修。
- **后期(S3)**:idle 飞轮从重复出现的 process / 经验聚类中**提议**新 Skill(沿结晶谱右推)。
- **案例**:本次网页调试在 EJS 工程摸出「文字 + 结构 + 父链」三段定位法 → `learn_skill` 结晶;后续发现动态 `<%= %>` 三段法失灵、插件法才对 → 修正 playbook 重新结晶,新版取代旧版,反 K 压住过时版本。

### 推论 8:清单要治理,数值全可设(← 推论 4 + `00` 推论 9)
Skill 多了常驻清单本身吃 token:按指针热度 / 正 K 权重排序**取前 N**(N 可设),本项目 Skill 优先(`project_boost`),被挤出者仍可被语义召回(推论 4 兜底,不失联)。清单上限、召回阈值、排序权重全是旋钮,有默认、有 UI 设置路径。
- **案例**:30 个 Skill 全量清单约千余 token;取前 N=12 按热度,正在调博客的项目里 web-debug 类靠前;被挤出的「数据库迁移」Skill 在用户真问迁移时仍被语义召回浮现。

### 推论 9:Skill 是知识,不绕安全门(← `03` + `05` 推论 5)
加载 Skill 只是读知识进上下文;AI 据它做的每个动作仍是普通工具调用、仍过安全门。Skill 正文无执行语义,`learn_skill` 写记忆无副作用。
- **案例**:反向定位 playbook 写着「shell 装 dev-inspector 插件」→ 真执行时 shell 工具照常过安全审查(写 vite.config 在项目内 = 可逆放行;装全局包越界 = 弹授权),与 playbook 是否存在无关。

## ★海量库的发现机制(2026-06-12 用户定调,已落地)★

当经验库膨胀到成百上千条,**常驻平铺清单会撑爆上下文**。GrowBox 不靠"全塞进系统提示",而用自身的
**QKV 外置 + 压缩谱 + 渐进披露**三条原理把发现做成可 scale 的三层:

1. **常驻 = 分类索引 + 核心(O(分类) 非 O(skill))**:`skills::listing` 把 skill 按 `category`
   (code/debug/ui/web/…)分组拼进系统提示;**总数 ≤ `skill_list_max` 全列(名+触发),超出自动降级为
   「分类索引」**(每类只列名 + 条数,省触发)。常驻体量随分类数增长,千百个 skill 不撑爆。

2. **语义召回 = 长尾发现主路(接进脊柱,用户「K=带背景的问题→加载一整组」)**:每回合脊柱
   `memory.retrieve_skills(场景)` —— 走**指针网 QKV**(嵌入 + `SKILL_RECALL_SOURCE` 正反 K,
   Q=当前场景背景),返回**涌现的那一组**(对相似问题共同命中的 skill 自然成组,**动态聚类、无需手定
   bundle**)。正反 K 让聚类越用越准(judge 拒的反 K 否决、命中的正 K 强化)。

3. **高置信自动注入(省 LLM 调用 + 加速,用户定调)**:`render_recalled` 据 `Hit.score` 分流——
   **强匹配(≥ `skill_autoload_threshold`,默认 0.88)直接把整篇 playbook 正文注入上下文**
   (零 `load_skill` 调用 = 系统按索引路由、而非 LLM 逐个挑),至多 `AUTOLOAD_MAX_BODIES`=3 篇防撑爆;
   一般匹配只浮名+触发,AI 自行 `load_skill`。`skill_autoload_threshold` 数值全可设。

**当前分工**:**内置种子**=少而精的 curated 核心,**常驻清单每回合可见**(AI 直接 `load_skill`);
**已学 skill**(用户投喂的海量经验)=memory 节点,**走召回+自动注入**——scale 与省调用主要在这一侧,
海量库越大越靠它。

**★内置种子也物化成节点(0-OPUS37 已落地,设计/09 原"可加点"兑现)★**:连接(`AppState::new`)时
`skills::ensure_seed_nodes` 把每个内置种子幂等地写成 `skill` kind 节点(已存在同名则跳过,不覆盖用户的
`learn_skill` 同名版),嵌入由 idle `ensure_embeddings_batch` 统一补。**效果**:某场景强匹配某种子时整篇
playbook 自动注入(省一次 `load_skill`),消除"内置只浮名、已学才自动注入"的不对称。**仍保「内置始终
可见」**:常驻清单(`skills::listing`)仍由静态 `SEEDS` 按分类驱动,与是否已物化无关——做法是 listing /
`all_skills` 把"名==种子名"的已学节点**去重归回内置分类**(取最新 trigger 作覆盖版显示),只有非种子名的
真·已学才进「已学」组。这样物化前后清单输出一致(测试 `listing_keeps_seeds_under_categories_after_materialization`
锁死)。

**为何不建独立检索子系统**:全程复用记忆内核的"那一套"(嵌入 + 学习型指针正反 K + 项目软隔离),
skill 只是又一种受控 kind 的节点。投喂经验 → `learn_skill` 结晶成节点 → 自动嵌入、被召回、被聚类、
被自动注入 —— 这就是飞轮把"大量经验内化进庞大库且自管理"的兑现。

## 内置种子 Skill 集(出厂 playbook,提升开发成功率)

S1 出厂的种子构成一套「怎么把软件开发做好」的 playbook,覆盖开发生命周期(立足 GrowBox 自身工具
shell/file/code_search/lsp 与铁律,参考业界工程实践提炼,不点名不入 git):

| skill | 阶段 | 要点 |
|---|---|---|
| `read-before-write` | 理解 | 改前先读目标 + 上下游 + 不变式;小步改即时验 |
| `disciplined-change` | 改动 | 改现有文件优先;只做任务要求的;不过度工程/不加多余 error handling(只在系统边界校验)/不做兼容 hack/不留半成品;注释 why-only;大改前对设计 |
| `review-your-diff` | 审查 | 收尾前按高频 bug 类别逐 hunk 自审(条件写反/off-by-one/空解引用/漏 await·?/falsy-zero/复制粘贴错变量/catch 吞错/正则元字符);读整个被改函数;核对涟漪面;跑测试看诊断 |
| `investigate-before-fix` | 调试 | 全面勘探胜过单点试错;看一手证据;列候选根因逐个证伪;定位到具体行再改;改完验真 |
| `web-debug-source-locate` | 反向定位 | DOM→源码:先看 data-source 坐标→判框架→code_search 三段法;改完刷新复核 |

这些是 S1 的出厂集;AI 用 `learn_skill` 结晶的 skill 与之同列、可覆盖同名。

## 第一个 Skill:网页调试反向定位(种子)

名称 `web-debug-source-locate`;触发描述「网页调试窗框选元素后,需要把选中 DOM 反向定位到本地源码再修改时」。playbook 大意(实现时打磨措辞):

1. **先看现成坐标**:选中元素及其祖先有无 `data-source` / `data-v-inspector` / `__source` 类源码坐标属性 → 有则直接用(文件:行),最准。
2. **判工程类型**(读 package.json / 配置文件):
   - Vite + React → shell 装 `react-dev-inspector` 或 `vite-plugin-dev-inspector`,改 vite.config 注入,重启 dev server → 渲染元素自带源码坐标,回到 1。
   - Vite + Vue → `vite-plugin-vue-inspector`,同路。
   - EJS / Express / 无插件生态 → 走 3。
3. **code_search 三段法**:先按选中文字精确搜 → 不中按类名 / id / 结构特征搜 → 再不中用父链锚点组合缩小;动态内容(`<%= %>` 等模板插值)别搜渲染后的值,搜其周边**静态锚点**,模板目录(views/)优先。
4. **改完源码** → `reload_debug_webview` 刷新 → 框选复核改对了没(自我负责,`08`)。

这正是 0-OPUS34 真机暴露的精度缺口(`code_search` 猜动态内容不准)的正解:精度问题是知识问题,不是再造一个执行器。

## 数据结构与接口(工程提要)

```
node_kind.rs    + pub const SKILL: &str = "skill"(受控表 + label 双语,非破坏)
Skill 节点正文(单节点,LLM 友好,markdown):
    名称 / 触发描述(一句话) / playbook 正文
执行器(均走唯一注册表 + 唯一分发路径):
    load_skill{name}                  取正文,append-only 工具结果回灌;入 NEVER_DEFER
    learn_skill{name, trigger, body}  脊柱拦截 → Memory::crystallize_skill
                                      (镜像 crystallize_process:近重复坍缩 + 反 K 取代旧版 + 即时嵌入)
记忆(growbox-memory):
    SKILL_RECALL_SOURCE 哨兵源(镜像 PROCESS_RECALL_SOURCE,正反 K 同一套边)
    retrieve_skills(query)(镜像 retrieve_processes:role==skill 过滤 + 反 K 否决 + 正 K 强化)
清单(注册表/脊柱):
    skill_listing() → 系统提示常驻段(名称 + 触发描述,热度排序取前 N;镜像 deferred_listing)
旋钮:skill_list_max(清单上限)/ skill_recall_threshold / 排序权重;project_boost 复用
```

## 实现阶段

- **S1(MVP)【★已落地 2026-06-12,commit `ce3468b`,真机验证通过★】**:`skill` kind + `load_skill` / `learn_skill` 执行器 + `skills::listing` 常驻清单(内置种子+已学合并)+ 内置种子 Skill + i18n 四语 + 单测。**真机自测**(deepseek-v4-pro,经全自动调试模式):发"调试网页框选想改源码、有技能就加载"→ AI 据常驻清单主动 `load_skill("web-debug-source-locate")` → playbook 进上下文 → 准确复述要点(data-source 捷径/判框架/code_search 三段法/`<%= %>` 模板坑/改完验证)。
  - **as-built 取舍**:S1 已含 `SKILL_RECALL_SOURCE` 正反 K + `retrieve_skills` 语义召回(本是 S2,因镜像 process 零成本,提前落地)。
  - **as-built 偏差(对设计的合理简化)**:内置种子是 `skills.rs` **编译期静态目录**(非 redb 节点),已学 skill 才是 memory 节点;脊柱合并两者(已学覆盖同名)。这与「不建独立 SkillStore」不冲突——静态出厂目录是默认(同内置工作流/notice catalog),真正生长在已学节点;省去 redb 种子化的 dedup/重嵌复杂度。**【0-OPUS37 已把种子也物化成节点】** 见上「内置种子也物化成节点」节:`ensure_seed_nodes` 连接时幂等写节点(静态目录仍是清单真相 + 物化副本供召回/自动注入),取舍升级为"二者兼得"。
  - **【0-OPUS37 兑现】内置种子也享语义召回 + 高置信自动注入**:见上「内置种子也物化成节点」节。
  - **修到的真 bug**:`learned_skill_body`/`id_by_name` 原返回首个同名节点;append-only 取代后旧版在前 → 必须取**最新**。
- **S2(学习闭环增强)**:清单按指针热度/正 K 排序取前 N(治理旋钮)+ 设置 UI 只读查看 Skill 清单 + load 后采用反馈强化(load_skill 命中后据后续是否被 judge 拒来正/反 K)。
- **S3(飞轮自学)【★0-OPUS37 续 已落地★】**:idle 飞轮从经验聚类中**提议**新 Skill(结晶谱「经验 → Skill」右推)。as-built:
  - `Reasoner::propose_skill(cluster) -> Option<ProposedSkill>`(growbox-learn;**默认 None** = mock/无 LLM 不提议,零行为变更)。真实现 `bridge.rs` 镜像 `distill`:系统提示 `subconscious.propose_skill`(LLM **兼质量闸**:太具体/噪音/已是常识 → `{none:true}`;否则 `{name,trigger,body}`)。
  - **复用 digest 已有的经验聚类**(不新建聚类机制):`idle.rs::digest_while_idle` 对**足够大的簇**(`MIN_CLUSTER_FOR_SKILL=3`)在蒸馏知识后额外 `propose_skill`,**每次 idle 激活至多 1 条**(防膨胀)。
  - **提议存储 = capped kv 列表,非记忆节点**(`skill_proposals.rs::SkillProposalStore`,落 `skill_proposals` kv):提议是**待裁决建议、非知识**,不进节点 → 不污染召回/自动注入。采纳才经 `crystallize_skill` 成真 skill 节点。
  - **三道防膨胀**:① 队列容量上限 `MAX_PENDING=12`(满了不再新增)② 去重(已存在同名 skill / 队列已有 / 已拒)③ **丢弃即入"不再提"名单**(`rejected`)。+ 每激活 ≤1 + LLM 质量闸 = 五重。
  - **用户裁决**:设置 → 工具 →「技能」区「技能提议」子区列出待裁决项(名/触发/起草依据/展开看正文)+ **采纳**(`accept_skill_proposal` → crystallize_skill,即时嵌入可召回)/ **丢弃**(`reject_skill_proposal` → 出队 + 不再提)。命令 `list/accept/reject_skill_proposal`;前端 load-on-open(后端 emit `memory-event{skill-proposed}` 备实时刷新)。
  - **测试**:`skill_proposals` 存储(去重/拒/容量/json)+ `try_add_skill_proposal`(撞内置种子名/空/已拒去重 + 持久化)+ bridge `propose_skill` 解析与质量闸(none/残缺→不提议)。
  - **as-built 取舍(诚实)**:v1 提议**源 = 经验聚类**(复用现成机制);设计原文还提了 process 聚类,留作 v2(需新做 process 聚类)。LLM 起草**质量**需真机验收(本会话余额已充,可真机看)。
  - **未做**:Skill → 工作流结晶辅助(谱再右推一格)留后续。

## 设计岔路与拍板建议(待用户确认)

| 岔路 | 建议 | 理由 |
|---|---|---|
| 载体:记忆节点 vs 独立 SkillStore | **记忆节点 + `skill` kind** | 全部记忆基建白拿(嵌入/正反K/坍缩/软隔离);process 已验证;独立库 = 平行子系统违背简洁 |
| 触发:纯 RAG vs 纯清单 | **清单为主 + 召回兜底(结合)** | 清单 = 确定性主动挑(场景明确时必中);召回 = 长尾兜底;单用任一都有漏 |
| 要不要 `load_skill` 执行器 | **要** | 渐进披露的兑现点;正文不常驻才撑得起任意多 Skill;append-only 回灌缓存安全有 C1 铁证 |
| 策划 vs 自学边界 | **出厂种子 + AI 即时结晶(S1/S2);idle 自动提议放 S3** | 即时结晶复用 learn_process 回路成本最低;idle 提议要防膨胀,后置 |
| Skill 与 process 关系 | **同族两 kind,不合并** | process = 被动召回的流程记忆;Skill = 带触发描述、进常驻清单、可主动挑的命名 playbook;谱上相邻但消费方式不同 |
