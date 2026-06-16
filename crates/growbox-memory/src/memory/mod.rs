//! Memory —— 存与取的统一入口。分层检索:RAG → 精确层。
//!
//! 实现 `设计/02-记忆检索`:
//! - 原则1 分层下沉:先 RAG,够用且反馈好就停;否则下沉精确层。
//! - 原则2 精确层飞轮:时间线扫描 + 染色(避免重复 LLM 读)。
//!
//! 本版已落地:存储 / 分层下沉 / 精确层时间线扫描 / 染色加速。
//! 待加厚(精确层飞轮细节,见 `系统架构/04-memory.md`):指针网络、二级索引、碎片、缓存三级队列。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

use growbox_core::Conclusion;

use crate::cache::NeighborCache;
use crate::context::{ContextBlock, ContextWindow, Origin, Region};
use crate::fragments::FragmentLedger;
use crate::index::{ArroyIndex, HnswIndex, VectorIndex};
use crate::pointer::{Pointer, PointerNet};
use crate::secondary::{SecondaryIndex, WINDOW};
use crate::store::Store;
use crate::subconscious::Subconscious;
use crate::timeline::{Node, Stain, Timeline};

/// RAG 命中阈值:第一层余弦相似度 ≥ 此值 = "找到了",直接返回不下沉(无 judge 门,故是关键阈值)。
/// 按 e5 分布定:实测同义≈0.92、无关≈0.79(见 `计划/embedding-service.md`),取 0.85 卡在中间。
/// 对词法向量仍安全(无关≈0 远低于此)。真分布调优待 arroy 那波用更多数据校准。
const RAG_HIT_THRESHOLD: f32 = 0.85;
/// 第一层 RAG 从向量索引取的候选数(再按阈值过滤)。
const RAG_TOPK: usize = 8;
/// 内部状态瞬态环容量(渲染进上下文最末的"内部状态"块;超出丢最旧)。
const INTERNAL_EVENTS_CAP: usize = 32;
/// 造物交互瞬态环容量(被造物 UI 的点击/输入回传;独立于内部状态环,免互相挤占)。
const ARTIFACT_INTERACTIONS_CAP: usize = 64;

/// 一条 AI 感知到的内部状态事件(失败/异常等)。瞬态 RAM 环,同时落时间线可检索。
/// 见决策日志 2026-06-01「AI 必须能感知一切失败/内部状态」。
#[derive(Clone, Debug)]
struct InternalEvent {
    /// 单调递增序号(从不减):agent 循环据此 append-only 注入"自上次以来的新事件",
    /// 不再每轮重渲整块夹末尾 —— 后者会破坏 deepseek byte-stable prefix 缓存(2026-06-04 实测 hit 640→128)。
    seq: u64,
    at: growbox_core::Timestamp,
    kind: String,
    message: String,
}
/// 精确层每批给 LLM 判断的节点数(一个窗口)。
const SCAN_BATCH: usize = 8;
/// 精确层线性主干路本次最多往回读多少个节点(有界"全量扫描",设计/02:"翻到最早=全量"的成本上限)。
/// ★2026-06-15 核心修复★:旧实现只扫最近 SCAN_BATCH 个就停 + 扫完秒染 Deep,致未嵌入的新记忆
/// 在 idle 补嵌前被埋(置换率恒0/上下文割裂)。现在 L2 渐进扫从最新往回多批推进,直到攒够命中/扫满此预算/
/// 连续空批/扫到最早。数值全可设(推论9)。
const SCAN_MAX: usize = 256;
/// ★文档破碎阈(2026-06-17,修 dream-board 投喂文档检索盲区)★:入场 content 字符数 > 此值的节点
/// (典型 = 用户粘贴的整篇技术栈/约定文档)标 `needs_chunk`,idle 按句破成小块。1500:正常对话/回复
/// 几十~几百字远低于(不破),粘贴文档常 1000+ 字高于(破)。e5 嵌入窗约 512 token,超此长度单条向量
/// 必然稀释/截断 → RAG 对其窄问必漏,故破成小块各自成向量。数值全可设(推论9);0 = 关闭破碎。
const CHUNK_MIN_CHARS: usize = 1500;
/// 项目软偏好:检索命中若属当前项目,相似度乘 (1 + 此值) 再排序(软偏好,非硬过滤)。
/// 0 = 不偏好(纯按相似度);默认 0.5 = 本项目命中相似度 +50%。跨项目高相关仍可压过本项目低相关被召回。
const PROJECT_BOOST: f32 = 0.5;
/// 指针跟随阈值:query 的档A 加权余弦得分 ≥ 此值 → 走这条快车道(weight=1 时即原始 cosine 门)。
const POINTER_FOLLOW_THRESHOLD: f32 = 0.80;
/// 档A 余弦命中后是否仍走前沿 batch judge 确认一次(默认 true=精确;false=直接采纳省 LLM 调用)。
/// [可调,推论9]。仅档A(WeightedCosine)有意义;档B 本就是 LLM judge,此旋钮不参与。
/// 二级索引「远处拉近」的推测项不是余弦命中,无论此旋钮如何仍需 judge 确认。
const POINTER_FORCE_JUDGE: bool = true;
/// 反 K 一票否决阈值:query 与某反 K 的 cosine ≥ 此值 → 阻断该边跳转(近似重复才否决,省 judge)。
/// [可调,推论9;阶段5 暴露为旋钮]。取较高=只挡近乎相同的曾被拒 query,免误挡正常 query。
const NEG_BLOCK_THRESHOLD: f32 = 0.90;
/// sleep 复核反 K:反 K 的 last_used 早于"现在 - 此值"则老化移除,让被旧误判长期挡住的边重获 judge 机会。
/// 默认 14 天(ms);[可调,维护非 bounding]。
const NEG_REVIEW_MAX_AGE_MS: i64 = 14 * 24 * 3600 * 1000;
/// 每次 sleep 复核反 K 至多处理的边数(有界,背景维护可接受 O(边数) 扫描但只改这么多)。
const NEG_REVIEW_MAX_EDGES: usize = 64;
/// 进入指针网的"门":取 embedding 相似度最高的前 K 个节点作入口。
const ENTRY_K: usize = 3;
/// 入口最低相似度:低于此值不算门(query 与该节点几乎无关)。
const ENTRY_MIN_SIM: f32 = 0.30;
/// 旧版指针网整网 blob 的 KV 键(已弃用,仅 open 时一次性迁移到 EDGES 表后删除)。
const POINTER_KV_KEY: &str = "pointer_net";
/// ★二期 B3 结晶★:近重复检测的搜索 top-k。
const PROCESS_MERGE_TOPK: usize = 5;
/// ★二期 B3 结晶★:新流程配方与已有流程节点相似度 ≥ 此值 = 近重复 → 视为"更正版取代旧版"
/// (对旧版召回记反 K 压制;推论4 持续合并)。取较高 = 只对几乎同一件事的更正才取代,不误并不同流程。
const PROCESS_MERGE_THRESHOLD: f32 = 0.90;
/// ★二期 B2 流程指针接通★:流程"召回学习"用的合成源节点 id。流程不是从某真实节点跳达,而是按
/// query 族召回 → 用一个固定哨兵 source 复用持久化 mesh 边(EDGES 表):边 = 哨兵 → 流程节点,
/// 正 K = 该流程服务过的 query 族(召回即用)、反 K = 这族 query 用它判错/被更正版取代(召回时一票否决)。
/// 该 id 不在时间线/索引里,故永不作检索入口,与主网 node→node 边天然隔离。见 `二期项目/项目设计/02`。
pub(crate) const PROCESS_RECALL_SOURCE: &str = "__process_recall__";
/// ★Skill 召回学习哨兵源(设计/09 推论6)★:与 PROCESS_RECALL_SOURCE 同构——边 = 哨兵 → skill 节点,
/// 正 K = 该 skill 服务过的 query 族(语义召回即用)、反 K = 这族 query 用它判错/被更正版取代(召回时否决)。
/// 复用同一持久化 mesh 边机制,零新存储。技能与流程同族两 kind,各用一个哨兵源,互不干扰。
pub(crate) const SKILL_RECALL_SOURCE: &str = "__skill_recall__";

/// 该角色的大节点是否该被破碎化。结构化 kind(流程/技能/工具记忆)**豁免**——它们靠整节点解析
/// (`retrieve_processes`/`retrieve_skills`/`consult_tool_memory` 解析节点头),破开会毁掉这些系统。
/// 普通内容(user/assistant/tool/internal/system)粘进整篇文档才需要破碎。
fn is_chunkable_role(role: &str) -> bool {
    !matches!(
        role,
        crate::node_kind::PROCESS | crate::node_kind::SKILL | crate::node_kind::TOOL_MEMORY
    )
}

/// 把一篇文档切成**原子句序列**,保证 `chunks.concat() == 原文`(零丢字、零改写——破点只在句末)。
/// 句末符:。!?！?；;\n(含 markdown 标题前的换行天然成界)。分隔符归到**前一句**末尾。
/// 末尾残段(无句末符结尾)也作一句。供 idle 破碎 pass:先切原子句,再交 LLM 判哪些另起一块。
pub(crate) fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        cur.push(ch);
        if matches!(ch, '。' | '!' | '?' | '！' | '？' | '；' | ';' | '\n') {
            // 已积累非纯空白才成句(连续换行不产生空句,纯空白仍留在 cur 里继续累积、并入下一句)。
            if !cur.trim().is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
    }
    // 末尾残段:非空即收(含纯空白尾,保 concat==原文;空 cur 不收)。
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 项目软隔离可见性:某节点(其 project_id)在当前项目 `cur` 下是否可见。
/// 同项目可见;全局(None tag)任何项目可见;有 tag 但当前无项目 → 不可见。工具记忆会诊/计数共用。
pub(crate) fn project_visible(node_project: &Option<String>, cur: Option<&str>) -> bool {
    match (node_project, cur) {
        (Some(p), Some(c)) => p == c,
        (None, _) => true,
        (Some(_), None) => false,
    }
}

/// 邻域缓存容量(磁盘图之上的 RAM 工作集,热 source 数)。
const NEIGHBOR_CACHE_CAP: usize = 256;
/// 碎片台账容量(mesh 跳转跳过的中段欠债,瞬态 RAM,见 `fragments.rs`)。
const FRAGMENT_LEDGER_CAP: usize = 512;
/// 二级索引容量(把热的远端节点拉近前沿的锚点数,瞬态 RAM,见 `secondary.rs`)。
const SECONDARY_INDEX_CAP: usize = 128;
/// 二级索引 K(漂移达几个窗口才建二级锚点;2K 触发粗化)。
const SECONDARY_K_WINDOWS: usize = 2;
/// 每次检索从二级索引"拉近"注入前沿的最热 target 数(有界,免每次 judge 过多)。
const SECONDARY_PULL_N: usize = 4;

/// 疲劳度权重(不看 CPU/内存,看三指标加权,见 `设计/02` 维护节)。和为 1。
const FATIGUE_W_HITRATE: f64 = 0.4; // 缓存命中率低
const FATIGUE_W_EVICT: f64 = 0.2; // 淘汰频繁
const FATIGUE_W_FRAGMENT: f64 = 0.4; // 碎片占比大

/// 一条检索结果。
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub content: String,
    pub source: String,
    pub score: f32,
}

/// 检索从哪一层得到(可观测,供元优化分析)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Rag,
    Exact,
}

/// 一次做梦(还一笔碎片债)的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DreamReport {
    /// 处理(还清)的碎片笔数(0 或 1)。
    pub processed: usize,
    /// 复查中段时新发现的相关节点数(补进索引的边数)。
    pub discoveries: usize,
    /// 处理后碎片台账是否已空(归零)。
    pub drained: bool,
}

/// 一次睡眠(做梦 + 推演 交替)的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SleepReport {
    /// 做梦还清的碎片笔数。
    pub dreams: usize,
    /// 推演(自问自答预演检索)的次数。
    pub rehearsals: usize,
    /// 累计新发现(补进索引的边数)。
    pub discoveries: usize,
    /// 睡眠结束时仍未还的碎片数。
    pub fragments_remaining: usize,
}

mod config;
mod maintenance;
mod perception;
mod retrieval;

#[cfg(test)]
mod tests;

// 旋钮类型在 config 子模块,经此重导出保 `memory::PointerConfig` 等路径不变(lib.rs re-export 用)。
pub use config::{FatigueConfig, PointerConfig, PointerMatchMode, RetrievalConfig, SkillConfig, TransientCapsConfig};

pub struct Memory {
    timeline: Timeline,
    conclusions: Vec<Conclusion>,
    /// 精确层指针网络(无 store 时的内存兜底;有 store 时真相在 EDGES 表)。
    pointers: PointerNet,
    /// 磁盘指针图之上的 RAM 工作集(仅 store 路径用;`RefCell` 让只读访问器也能更新热度/淘汰)。
    cache: RefCell<NeighborCache>,
    /// 第一层 RAG 向量索引(可切换引擎,见 `index.rs`)。embedding 变更后重建。
    index: Box<dyn VectorIndex>,
    /// 持久化后端。`Some` = write-through 落库(运行时);`None` = 纯内存(单测)。
    store: Option<Store>,
    /// 上下文组装层(P4)的常驻工作集 —— 跨回合存活的"换入上下文"那一环(两态、预算淘汰)。
    context: ContextWindow,
    /// 精确层碎片台账(P5)——mesh 跳转跳过的中段欠债,做梦来还(瞬态 RAM)。
    fragments: FragmentLedger,
    /// 精确层二级索引(阶段4「远处拉近」)——把热的远端节点锚到当前前沿,瞬态 RAM 可再生。
    secondary: SecondaryIndex,
    /// 强制跳转指针(阶段4「历史引用」)无 store 时的内存兜底;有 store 时真相在 JUMPS 表。
    forced_jumps: HashMap<String, Vec<String>>,
    /// 内部状态瞬态环 —— AI 感知到的失败/内部事件(渲染进上下文最末块;同时已落时间线)。
    internal_events: VecDeque<InternalEvent>,
    /// 造物交互瞬态环 —— 被造物 UI 的点击/输入回传(渲进上下文;默认丢、不落时间线;独立环免挤占内部状态)。
    artifact_interactions: VecDeque<InternalEvent>,
    /// 内部/造物事件的单调序号发号器(append-only 注入用;从不减,即使 ring pop)。
    internal_seq: u64,
    artifact_seq: u64,
    /// 学习型指针可调旋钮(匹配档 + 4 阈值);阶段5 由 Settings 透传(推论9)。
    pointer_cfg: PointerConfig,
    /// 检索行为可调旋钮(RAG 命中阈/候选数 + 精确层入口/批量);由 Settings 透传(推论9)。
    retrieval_cfg: RetrievalConfig,
    /// 疲劳公式权重旋钮(命中率/淘汰/碎片三权重);由 Settings 透传(推论9)。
    fatigue_cfg: FatigueConfig,
    /// 瞬态容量旋钮(碎片/二级/内部环 cap + 反K复核参数);由 Settings 透传(推论9)。
    transient_caps: TransientCapsConfig,
    /// Skill 系统旋钮(总开关 + 清单上限 + 停用名单);由 Settings 透传(设计/09,推论8)。
    skill_cfg: SkillConfig,
    /// 当前项目 tag(软隔离):新 ingest 的节点盖此 tag;检索对本项目命中加权。
    /// None = 未指定(测试/未切项目)。由 gui `switch_project` 经 `set_current_project` 设。
    current_project: Option<String>,
}

impl Memory {
    /// 纯内存(测试用,无持久化)。
    pub fn new() -> Self {
        Memory {
            timeline: Timeline::new(),
            conclusions: Vec::new(),
            pointers: PointerNet::new(),
            cache: RefCell::new(NeighborCache::new(NEIGHBOR_CACHE_CAP)),
            index: Box::new(HnswIndex::new()),
            store: None,
            context: ContextWindow::default(),
            fragments: FragmentLedger::new(FRAGMENT_LEDGER_CAP),
            secondary: SecondaryIndex::new(SECONDARY_INDEX_CAP, SECONDARY_K_WINDOWS),
            forced_jumps: HashMap::new(),
            internal_events: VecDeque::new(),
            artifact_interactions: VecDeque::new(),
            internal_seq: 0,
            artifact_seq: 0,
            pointer_cfg: PointerConfig::default(),
            retrieval_cfg: RetrievalConfig::default(),
            fatigue_cfg: FatigueConfig::default(),
            transient_caps: TransientCapsConfig::default(),
            skill_cfg: SkillConfig::defaults(),
            current_project: None,
        }
    }

    /// 由持久化存储打开:载入已存的节点 / 结论 / 指针,之后所有写入 write-through 落库。
    /// 节点按 created_at 还原时间线顺序(redb 按 key 排序,需重排)。
    /// P3d:开机只把节点的轻量元信息建进时间线(content 不常驻 RAM),向量在这一遍读出
    /// 喂给第一层索引;运行时 content 按 id 惰性回盘取。
    /// P3e:第一层索引用磁盘原生 `ArroyIndex`(LMDB env 落在 `index_dir` 下);打开失败回退内存 HNSW。
    pub fn open(store: Store, index_dir: &Path) -> Self {
        let mut nodes = store.load_nodes();
        nodes.sort_by_key(|n| n.created_at);
        let mut timeline = Timeline::with_store(store.clone());
        for n in &nodes {
            timeline.restore(n);
        }
        drop(nodes); // content 不再常驻:用完即弃,运行时惰性回盘
        // 旧版本把整张指针网存成一个 KV blob;迁移到磁盘原生 EDGES 表后删除该 blob(无历史包袱)。
        if let Some(old) = store.kv_get::<HashMap<String, Vec<Pointer>>>(POINTER_KV_KEY) {
            for (source, edges) in &old {
                for p in edges {
                    store.put_edge(source, p);
                }
            }
            store.kv_remove(POINTER_KV_KEY);
        }
        // 第一层索引:磁盘原生 arroy(向量走 LMDB/mmap,不全驻 RAM);打开失败不致命,回退内存 HNSW。
        let index: Box<dyn VectorIndex> = match ArroyIndex::open(index_dir.join("vector-index")) {
            Ok(idx) => Box::new(idx),
            Err(e) => {
                eprintln!("向量索引(arroy)打开失败,本次回退内存 HNSW: {e}");
                Box::new(HnswIndex::new())
            }
        };
        // 磁盘原生:指针真相在 EDGES 表,按需读一个邻域;PointerNet 仅作无 store(测试)时的内存兜底。
        let mut m = Memory {
            timeline,
            conclusions: store.load_conclusions(),
            pointers: PointerNet::new(),
            cache: RefCell::new(NeighborCache::new(NEIGHBOR_CACHE_CAP)),
            index,
            store: Some(store),
            context: ContextWindow::default(),
            fragments: FragmentLedger::new(FRAGMENT_LEDGER_CAP),
            secondary: SecondaryIndex::new(SECONDARY_INDEX_CAP, SECONDARY_K_WINDOWS),
            forced_jumps: HashMap::new(),
            internal_events: VecDeque::new(),
            artifact_interactions: VecDeque::new(),
            internal_seq: 0,
            artifact_seq: 0,
            pointer_cfg: PointerConfig::default(),
            retrieval_cfg: RetrievalConfig::default(),
            fatigue_cfg: FatigueConfig::default(),
            transient_caps: TransientCapsConfig::default(),
            skill_cfg: SkillConfig::defaults(),
            current_project: None,
        };
        m.rebuild_index(); // 用已存的向量建第一层索引
        m
    }

    /// 用所有已向量化节点全量重建第一层索引(开机 / 批量补向量 / 重嵌后调)。
    /// 向量从盘上(Store)或缓存(无 Store)取,不经时间线常驻。
    fn rebuild_index(&mut self) {
        let items = match &self.store {
            Some(s) => s.load_node_vectors(),
            None => self.timeline.cached_vectors(),
        };
        // 已破碎的父节点剔出索引:其内容已由各小块表示,父向量稀释,留着会与块重复/稀释命中
        //(dream-board 投喂文档盲区的另一半——父节点必须从 RAG 退场,只让小块上场)。
        let items: Vec<_> = items
            .into_iter()
            .filter(|(id, _)| self.timeline.meta(id).map(|m| !m.chunked).unwrap_or(true))
            .collect();
        self.index.rebuild(items);
    }

    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }
    pub fn conclusions(&self) -> &[Conclusion] {
        &self.conclusions
    }

    /// 把某节点的当前状态落库(write-through;无 store 时空操作)。
    fn persist_node(&self, id: &str) {
        if let Some(store) = &self.store {
            if let Some(n) = self.timeline.get(id) {
                store.put_node(&n);
            }
        }
    }

    /// 把某结论的当前状态落库(write-through;无 store 时空操作)。
    fn persist_conclusion(&self, id: &str) {
        if let Some(store) = &self.store {
            if let Some(c) = self.conclusions.iter().find(|c| c.id == id) {
                store.put_conclusion(c);
            }
        }
    }

    /// 取某节点出边。有 store:先查 RAM 工作集(缓存),miss 回盘读一个邻域并入缓存(磁盘原生 +
    /// 三级缓存,见 `cache.rs`);无 store:内存 PointerNet(测试兜底)。
    fn edges_of(&self, source: &str) -> Vec<Pointer> {
        match &self.store {
            Some(s) => {
                if let Some(hit) = self.cache.borrow_mut().get(source) {
                    return hit;
                }
                let fresh = s.neighbors(source);
                self.cache.borrow_mut().put(source, fresh.clone());
                fresh
            }
            None => self.pointers.neighbors(source).to_vec(),
        }
    }

    /// 建/复用一条边(键=query 原文+向量);走 `record_positive`(累积去重),
    /// write-through;失效该 source 缓存(下次回盘读最新)。
    fn link_edge(&mut self, source: &str, target: &str, text: &str, topic: Vec<f32>) {
        let now = growbox_core::now().timestamp_millis();
        let kmt = self.pointer_cfg.k_merge_threshold;
        let kcap = self.pointer_cfg.k_cap;
        match &self.store {
            Some(s) => {
                let mut p = match s.get_edge(source, target) {
                    Some(mut existing) => {
                        existing.record_positive(text.to_string(), topic, now, kmt);
                        existing
                    }
                    None => {
                        let mut p = Pointer::from_topic(target, topic, now);
                        if !text.is_empty() {
                            p.positives[0].text = text.to_string();
                        }
                        p
                    }
                };
                p.enforce_cap(kcap); // LFU 封顶(防膨胀第二道闸)
                s.put_edge(source, &p);
                self.cache.borrow_mut().invalidate(source);
            }
            None => {
                self.pointers.link(source, target, text, topic, now, kmt);
                self.pointers.enforce_cap_edge(source, target, kcap);
            }
        }
    }

    /// 检索命中:给一条**已存在**的边记一个正 K(累积去重 + heat+1),write-through。
    /// 仅作用于已存在的边(检索经它到达 target);不存在则忽略(不在此凭空建边)。
    fn record_positive_edge(&mut self, source: &str, target: &str, text: &str, qv: &[f32]) {
        let now = growbox_core::now().timestamp_millis();
        let kmt = self.pointer_cfg.k_merge_threshold;
        let kcap = self.pointer_cfg.k_cap;
        match &self.store {
            Some(s) => {
                if let Some(mut p) = s.get_edge(source, target) {
                    p.record_positive(text.to_string(), qv.to_vec(), now, kmt);
                    p.enforce_cap(kcap);
                    s.put_edge(source, &p);
                    self.cache.borrow_mut().invalidate(source);
                }
            }
            None => {
                self.pointers.record_positive(source, target, text, qv.to_vec(), now, kmt);
                self.pointers.enforce_cap_edge(source, target, kcap);
            }
        }
    }

    /// 检索 judge 拒:给一条**已存在**的边记一个反 K(硬负样本,不加 heat),write-through。
    /// 此前这信息被丢弃(规格点名的关键缺口);阶段3 档A 据此一票否决相似 query 的误跳。
    fn record_negative_edge(&mut self, source: &str, target: &str, text: &str, qv: &[f32]) {
        let now = growbox_core::now().timestamp_millis();
        let kmt = self.pointer_cfg.k_merge_threshold;
        let kcap = self.pointer_cfg.k_cap;
        match &self.store {
            Some(s) => {
                if let Some(mut p) = s.get_edge(source, target) {
                    p.record_negative(text.to_string(), qv.to_vec(), now, kmt);
                    p.enforce_cap(kcap);
                    s.put_edge(source, &p);
                    self.cache.borrow_mut().invalidate(source);
                }
            }
            None => {
                self.pointers.record_negative(source, target, text, qv.to_vec(), now, kmt);
                self.pointers.enforce_cap_edge(source, target, kcap);
            }
        }
    }

    // --- 二期 B2:流程召回学习(哨兵源 PROCESS_RECALL_SOURCE 复用持久化 mesh 边)---

    /// 这条 query 是否被某流程的召回边反 K 一票否决(同族 query 曾用它判错/它被更正版取代)。
    /// 命中即在召回时把该流程滤掉(越用越准:误召的被压制)。读单条边(store get_edge / 内存兜底)。
    fn process_recall_vetoes(&self, process_id: &str, qv: &[f32]) -> bool {
        let thr = self.pointer_cfg.neg_block_threshold;
        let edge = match &self.store {
            Some(s) => s.get_edge(PROCESS_RECALL_SOURCE, process_id),
            None => self
                .pointers
                .neighbors(PROCESS_RECALL_SOURCE)
                .iter()
                .find(|p| p.target == process_id)
                .cloned(),
        };
        edge.map(|p| p.neg_veto(qv, thr)).unwrap_or(false)
    }

    /// 流程被召回(=该 query 族在用它):在 哨兵 → 流程 边记正 K(累积 query 簇 + heat)。write-through。
    /// 复用 `link_edge`(创建/累积去重),故持久化、防膨胀(LFU 封顶)全继承。
    fn reinforce_process_recall(&mut self, process_id: &str, query: &str, qv: &[f32]) {
        self.link_edge(PROCESS_RECALL_SOURCE, process_id, query, qv.to_vec());
    }

    /// 抑制一个流程的召回(被更正版取代):在 哨兵 → 流程 边记反 K(键=取代它的新流程向量),
    /// 使同族 query 以后规避旧版(`process_recall_vetoes`)。边不存在则先建空壳。write-through。
    pub(crate) fn suppress_process_recall(&mut self, process_id: &str, qv: &[f32]) {
        let now = growbox_core::now().timestamp_millis();
        let kmt = self.pointer_cfg.k_merge_threshold;
        let kcap = self.pointer_cfg.k_cap;
        match &self.store {
            Some(s) => {
                let mut p = s
                    .get_edge(PROCESS_RECALL_SOURCE, process_id)
                    .unwrap_or_else(|| Pointer::empty(process_id));
                p.record_negative("(被更正版取代)".to_string(), qv.to_vec(), now, kmt);
                p.enforce_cap(kcap);
                s.put_edge(PROCESS_RECALL_SOURCE, &p);
                self.cache.borrow_mut().invalidate(PROCESS_RECALL_SOURCE);
            }
            None => {
                self.pointers.suppress(PROCESS_RECALL_SOURCE, process_id, "(被更正版取代)", qv.to_vec(), now, kmt);
                self.pointers.enforce_cap_edge(PROCESS_RECALL_SOURCE, process_id, kcap);
            }
        }
    }

    // --- Skill 召回学习(设计/09 推论6:哨兵源 SKILL_RECALL_SOURCE,与 process 同构,各用一源)---

    /// 这条 query 是否被某 skill 的召回边反 K 一票否决(同族 query 曾用它判错/它被更正版取代)。
    fn skill_recall_vetoes(&self, skill_id: &str, qv: &[f32]) -> bool {
        let thr = self.pointer_cfg.neg_block_threshold;
        let edge = match &self.store {
            Some(s) => s.get_edge(SKILL_RECALL_SOURCE, skill_id),
            None => self
                .pointers
                .neighbors(SKILL_RECALL_SOURCE)
                .iter()
                .find(|p| p.target == skill_id)
                .cloned(),
        };
        edge.map(|p| p.neg_veto(qv, thr)).unwrap_or(false)
    }

    /// skill 被语义召回(=该 query 族在用它):在 哨兵 → skill 边记正 K。write-through(复用 link_edge)。
    fn reinforce_skill_recall(&mut self, skill_id: &str, query: &str, qv: &[f32]) {
        self.link_edge(SKILL_RECALL_SOURCE, skill_id, query, qv.to_vec());
    }

    /// 抑制一个 skill 的召回(被更正版取代):在 哨兵 → skill 边记反 K(键=取代它的新 skill 向量)。
    pub(crate) fn suppress_skill_recall(&mut self, skill_id: &str, qv: &[f32]) {
        let now = growbox_core::now().timestamp_millis();
        let kmt = self.pointer_cfg.k_merge_threshold;
        let kcap = self.pointer_cfg.k_cap;
        match &self.store {
            Some(s) => {
                let mut p = s
                    .get_edge(SKILL_RECALL_SOURCE, skill_id)
                    .unwrap_or_else(|| Pointer::empty(skill_id));
                p.record_negative("(被更正版取代)".to_string(), qv.to_vec(), now, kmt);
                p.enforce_cap(kcap);
                s.put_edge(SKILL_RECALL_SOURCE, &p);
                self.cache.borrow_mut().invalidate(SKILL_RECALL_SOURCE);
            }
            None => {
                self.pointers.suppress(SKILL_RECALL_SOURCE, skill_id, "(被更正版取代)", qv.to_vec(), now, kmt);
                self.pointers.enforce_cap_edge(SKILL_RECALL_SOURCE, skill_id, kcap);
            }
        }
    }

    // --- Skill 节点读写(设计/09 推论3:skill kind 记忆节点,复用全部记忆基建)---

    /// 摄入一个 skill 节点。content = 结构化 playbook 文本(见 `skill_format`)。与任意节点同源
    /// (时间线 + 向量索引 + 学习型指针 + project_id 软隔离)。返回节点 id。
    pub fn ingest_skill(&mut self, content: impl Into<String>) -> String {
        self.ingest_with_role(content, crate::node_kind::SKILL)
    }

    /// 全部已学 skill 节点的 `(name, trigger, id)`(供常驻清单 + 主动挑)。按创建序;heat 排序治理是 S2。
    /// name/trigger 从节点 content 的结构化头解析(`skill_format::parse_head`),解析不出则跳过(非破坏)。
    pub fn learned_skill_listing(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for meta in self.timeline.metas() {
            if meta.role != crate::node_kind::SKILL {
                continue;
            }
            if let Some(node) = self.timeline.get(&meta.id) {
                if let Some((name, trigger)) = crate::skill_format::parse_head(&node.content) {
                    out.push((name, trigger, meta.id.clone()));
                }
            }
        }
        out
    }

    /// 按名取某 skill 节点的完整 playbook 正文(精确匹配 name,大小写不敏感)。无则 None。
    /// ★取**最新**版★:结晶取代是 append-only(旧版不删),timeline 按创建序 → 取最后一个同名匹配,
    /// 即最近结晶的更正版(否则会返回被取代的旧版)。
    pub fn learned_skill_body(&self, name: &str) -> Option<String> {
        let mut latest: Option<String> = None;
        for meta in self.timeline.metas() {
            if meta.role != crate::node_kind::SKILL {
                continue;
            }
            if let Some(node) = self.timeline.get(&meta.id) {
                if let Some((n, _)) = crate::skill_format::parse_head(&node.content) {
                    if n.eq_ignore_ascii_case(name) {
                        latest = Some(node.content);
                    }
                }
            }
        }
        latest
    }

    // --- 工具记忆节点(计划/工具记忆-不犯第二遍:每工具每项目小本本,与 skill/process 同族)---

    /// 摄入一条工具记忆节点。content = `tool_memory_format` 结构化文本。
    pub fn ingest_tool_memory(&mut self, content: impl Into<String>) -> String {
        self.ingest_with_role(content, crate::node_kind::TOOL_MEMORY)
    }

    /// 本项目工具记忆条数(成本门:为 0 则脊柱跳过分发前会诊 = 零开销;绝大多数情况如此)。
    /// 纯 meta 扫描不触盘;project 软隔离:同项目 + 全局(None tag)计入。
    pub fn tool_memory_count(&self) -> usize {
        let cur = self.current_project.as_deref();
        self.timeline
            .metas()
            .iter()
            .filter(|m| m.role == crate::node_kind::TOOL_MEMORY)
            .filter(|m| project_visible(&m.project_id, cur))
            .count()
    }

    // --- 强制跳转指针(阶段4「历史引用」:位置键,遍历到此必跳)---

    /// 取某位置(source)的全部强制跳转目标。有 store:JUMPS 表前缀读;无 store:内存兜底。
    fn forced_jumps_of(&self, source: &str) -> Vec<String> {
        match &self.store {
            Some(s) => s.jumps(source),
            None => self.forced_jumps.get(source).cloned().unwrap_or_default(),
        }
    }

    /// 用户显式引用历史 → 在某位置建一条**强制跳转指针**(`设计/02` 五件套末行)。
    /// `from=None` 用当前位置(时间线最近节点);target = 用户指认的那段历史节点 id。
    /// 与语义边不同:**位置键、无 topic 门、不随热度衰减、持久落库**(用户断言的真相)。
    /// 之后检索导航的入口落到 source,即无条件召回 target(retrieve_exact 步骤1.5)。
    pub fn pin_history_reference(&mut self, from: Option<&str>, target: &str) -> Option<String> {
        // 目标必须在线;否则忽略(不指向不存在的位置)。
        self.timeline.meta(target)?;
        let source = match from {
            Some(s) => s.to_string(),
            None => self.timeline.metas().last()?.id.clone(),
        };
        if source == target {
            return None; // 自指无意义
        }
        match &self.store {
            Some(s) => s.put_jump(&source, target),
            None => {
                let v = self.forced_jumps.entry(source.clone()).or_default();
                if !v.iter().any(|t| t == target) {
                    v.push(target.to_string());
                }
            }
        }
        Some(source)
    }

    /// 强制跳转指针总数(观测;面板 secondary_indexes)。
    pub fn forced_jump_count(&self) -> usize {
        match &self.store {
            Some(s) => s.jump_count(),
            None => self.forced_jumps.values().map(|v| v.len()).sum(),
        }
    }

    /// 当前二级索引锚点数(阶段4「远处拉近」;面板 secondary_indexes.total)。
    pub fn secondary_index_count(&self) -> usize {
        self.secondary.len()
    }

    /// 缓存观测(占用 / 容量 / 命中率 / 淘汰数),供 P5 疲劳度、P6 面板。平铺单 LFU,无分层。
    pub fn cache_stats(&self) -> (usize, usize, f64, u64) {
        let c = self.cache.borrow();
        (c.len(), c.capacity(), c.hit_rate(), c.evictions())
    }

    /// 记忆置换率 [0,1]:★工作区(置换系统的"物理内存")真实换入换出的 churn★(2026-06-15 改挂真置换)。
    /// = 工作区 `ContextWindow.replacement_rate()`(淘汰/换入,封顶 1)。每回合检索(**含 RAG 命中**)都经
    /// `assemble_context` 的 `page_in` 在这里 churn。**此前错读 L2 邻域边缓存的淘汰压力**——那只在 RAG 没命中、
    /// 下沉 L2 时才动 → RAG 命中越多越是 0,被 L1 命中率间接绑死(实现把"置换"窄化成了"L2 导航边缓存")。已纠正。
    /// 面板"记忆置换率" + 疲劳度 evict 项的真实来源。
    pub fn context_replacement_rate(&self) -> f64 {
        self.context.replacement_rate()
    }

    /// 工作区累计真实淘汰次数(面板「置换率 / 队列占用」hint;Nap 归零)。
    pub fn context_evictions(&self) -> u64 {
        self.context.evictions()
    }

    /// 工作区常驻条数(面板「缓存队列」hint)。
    pub fn context_resident_len(&self) -> usize {
        self.context.resident_len()
    }

    /// 存放区里**假指针(RAG/ANN 命中)**常驻数(面板:缓存队列里真/假指针占比)。
    /// 见 `用户决策/记忆架构-索引区与存放区.md`:RAG 指针作为假指针统一进队列/缓存,换出不落序列。
    pub fn context_fake_pointers(&self) -> usize {
        self.context.resident_fake_count()
    }

    /// 存放区里**真指针(L2/精确层命中)**常驻数。
    pub fn context_real_pointers(&self) -> usize {
        self.context.resident_real_count()
    }

    /// 当前指针边数(供元优化/测试观测)。
    pub fn pointer_count(&self) -> usize {
        match &self.store {
            Some(s) => s.edge_count(),
            None => self.pointers.edge_count(),
        }
    }

    /// 某节点的出边目标(供元优化/测试观测)。
    pub fn pointer_neighbors(&self, source: &str) -> Vec<String> {
        self.edges_of(source).iter().map(|p| p.target.clone()).collect()
    }

    // --- 摄入 ---

    /// 摄入一条对话/操作原文(同步,不阻塞;向量化延后)。
    pub fn ingest_conversation(&mut self, content: impl Into<String>) -> String {
        self.ingest_with_role(content, "system")
    }

    /// 摄入一条带角色的对话消息(role = "user"|"assistant"|"tool")。
    /// 供 agent_loop 和 chat history 公用,timeline 是唯一数据源。
    ///
    /// ★文档破碎化入场闸(2026-06-17)★:content 字符数超过破碎阈、且角色**非结构化 kind**
    /// (流程/技能/工具记忆靠整节点解析,豁免)→ 标 `needs_chunk`,留给 idle `chunk_pending_batch`
    /// 按句破成小块。同步只打标志(廉价、不卡回合);真正破碎(要 LLM 判破点)走 idle。
    pub fn ingest_with_role(&mut self, content: impl Into<String>, role: impl Into<String>) -> String {
        let content = content.into();
        let role = role.into();
        let mut node = Node::with_role(content, role);
        node.project_id = self.current_project.clone(); // 软隔离 tag:盖当前项目
        let gate = self.retrieval_cfg.chunk_min_chars;
        if gate > 0 && is_chunkable_role(&node.role) && node.content.chars().count() > gate {
            node.needs_chunk = true;
        }
        let id = self.timeline.push(node);
        self.persist_node(&id);
        id
    }

    /// 摄入一个**文档碎块**(idle 破碎 pass 产出):继承父节点 role + 项目 tag,**强制不再破碎**
    /// (块按构造已低于阈;即便是病态的"单个超长句"也接受为一块,绝不再标 `needs_chunk` 致死循环)。
    fn ingest_chunk(&mut self, content: impl Into<String>, role: impl Into<String>, project: Option<String>) -> String {
        let mut node = Node::with_role(content, role);
        node.project_id = project;
        node.needs_chunk = false;
        let id = self.timeline.push(node);
        self.persist_node(&id);
        id
    }

    /// 设置当前项目 tag(gui `switch_project` 调)。之后 ingest 的节点都盖这个 tag;
    /// 检索时对本项目命中加权(软偏好)。传 None = 不分类(测试/未切项目)。
    pub fn set_current_project(&mut self, project_id: Option<String>) {
        self.current_project = project_id;
    }

    /// 当前项目 tag(检索加权 / 显示过滤参考)。
    pub fn current_project(&self) -> Option<&str> {
        self.current_project.as_deref()
    }

    /// ★完整保真的展示记录★:把前端"用户实际看到的"富消息(含思考块/工具调用卡/token meta)
    /// 按项目整存进 KV,重启时原样还原界面。**与时间线(=AI 记忆/检索源)分离**:时间线只留
    /// user/assistant 正文供 RAG/召回,而这条记录留"界面长什么样"。键 = transcript:<project_id>。
    /// json = 前端富消息数组的 JSON 串。无 store(纯内存模式/测试)则静默跳过。
    pub fn save_transcript(&self, project_id: &str, json: &str) {
        if let Some(store) = &self.store {
            store.kv_put(&format!("transcript:{project_id}"), &json.to_string());
        }
    }

    /// 取某项目保存的完整展示记录(富消息数组 JSON 串);无则 None → 调用方回退时间线派生(老项目)。
    pub fn load_transcript(&self, project_id: &str) -> Option<String> {
        self.store.as_ref()?.kv_get::<String>(&format!("transcript:{project_id}"))
    }

    /// 摄入一条项目级流程(二期 process kind 建议档)。content = "在本项目做 X = 碰 A→B→C" 配方原文。
    /// 与任意节点同源(时间线 + 向量索引 + 学习型指针),用 `retrieve_processes` 按语义召回。
    pub fn ingest_process(&mut self, content: impl Into<String>) -> String {
        self.ingest_with_role(content, crate::node_kind::PROCESS)
    }

    // --- 观测访问器(面板 P6 接真用)---

    /// 第一层向量索引已装入的条目数(index_density 的分子)。
    pub fn index_len(&self) -> usize {
        self.index.len()
    }

    /// 上下文工作区填充率 [0,1](面板 budget_pct)。
    pub fn context_fill_pct(&self) -> f64 {
        self.context.fill_pct()
    }

    /// 染色覆盖计数 `(deep, light, none)`(总数用 `timeline().len()`)。
    /// Deep=仔细扫过确信无漏;Light=跟指针快扫过可能有漏;None=从没到过。无 Red 染色。
    pub fn stain_coverage(&self) -> (usize, usize, usize) {
        let (mut deep, mut light, mut none) = (0usize, 0usize, 0usize);
        for meta in self.timeline.metas() {
            match meta.stain {
                Stain::Deep => deep += 1,
                Stain::Light => light += 1,
                Stain::None => none += 1,
            }
        }
        (deep, light, none)
    }

    /// 摄入一条结论(经验/知识/理解同一模型)。
    pub fn ingest_conclusion(&mut self, c: Conclusion) {
        let id = c.id.clone();
        self.conclusions.push(c);
        self.persist_conclusion(&id);
    }

    /// 把一条结论标记为被更精确的新版本取代(append-only,不删,留进化史)。
    /// 飞轮把经验压成知识后调用,保证下一轮不重复压缩(见 `设计/04` 推论2)。
    pub fn supersede(&mut self, id: &str, by: &str) {
        if let Some(c) = self.conclusions.iter_mut().find(|c| c.id == id) {
            c.superseded_by = Some(by.to_string());
        }
        self.persist_conclusion(id);
    }

    /// 配置上下文组装层预算(P4d:随模型/用户设置)。任一为 0 = 保持当前默认。
    pub fn configure_context(&mut self, working_chars: usize, ring_chars: usize) {
        self.context.set_budgets(working_chars, ring_chars);
    }

    /// 设置整套指针旋钮(匹配档 + 4 阈值;`计划/指针-学习型边.md`)。阶段5 由 Settings 透传(推论9)。
    pub fn set_pointer_config(&mut self, cfg: PointerConfig) {
        self.pointer_cfg = cfg;
    }

    /// 仅设匹配档(便捷;整套见 `set_pointer_config`)。
    pub fn set_pointer_match_mode(&mut self, mode: PointerMatchMode) {
        self.pointer_cfg.match_mode = mode;
    }

    /// 当前指针旋钮(供控制面板回显;推论9 数值全可设)。
    pub fn pointer_config(&self) -> PointerConfig {
        self.pointer_cfg
    }

    /// 设置整套检索旋钮(RAG 命中阈/候选数 + 精确层入口/批量)。由 Settings 透传(推论9)。
    /// 即时生效(下次检索即用新值);纯参数无需重建索引/缓存。
    pub fn set_retrieval_config(&mut self, cfg: RetrievalConfig) {
        self.retrieval_cfg = cfg;
    }

    /// 当前检索旋钮(供控制面板回显;推论9 数值全可设)。
    pub fn retrieval_config(&self) -> RetrievalConfig {
        self.retrieval_cfg
    }

    /// 设置疲劳公式权重旋钮(命中率/淘汰/碎片;由 Settings 透传,推论9)。即时生效(下次 fatigue() 即用)。
    pub fn set_fatigue_config(&mut self, cfg: FatigueConfig) {
        self.fatigue_cfg = cfg;
    }

    /// 当前疲劳权重旋钮(供控制面板回显;推论9 数值全可设)。
    pub fn fatigue_config(&self) -> FatigueConfig {
        self.fatigue_cfg
    }

    /// 设置 Skill 系统旋钮(总开关 + 清单上限 + 停用名单);连接时由 Settings 注入(设计/09)。即时生效。
    pub fn set_skill_config(&mut self, cfg: SkillConfig) {
        self.skill_cfg = cfg;
    }

    /// 当前 Skill 旋钮(供设置 UI 回显)。
    pub fn skill_config(&self) -> &SkillConfig {
        &self.skill_cfg
    }

    /// 设置瞬态容量旋钮(碎片/二级/内部环 cap + 反K复核参数);连接时由 Settings 注入(推论9)。
    /// 重建碎片台账/二级索引(瞬态可再生,清空可接受,同 nap/缓存重建)、按新 cap 截断内部事件环。
    pub fn set_transient_caps(&mut self, cfg: TransientCapsConfig) {
        self.transient_caps = cfg;
        self.fragments = FragmentLedger::new(cfg.fragment_ledger_cap);
        self.secondary = SecondaryIndex::new(cfg.secondary_index_cap, SECONDARY_K_WINDOWS);
        while self.internal_events.len() > cfg.internal_events_cap {
            self.internal_events.pop_front();
        }
        while self.artifact_interactions.len() > cfg.artifact_interactions_cap {
            self.artifact_interactions.pop_front();
        }
    }

    /// 当前瞬态容量旋钮(供控制面板回显;推论9 数值全可设)。
    pub fn transient_caps(&self) -> TransientCapsConfig {
        self.transient_caps
    }

    /// 重设邻域缓存容量(用户在控制面板可调,数值参数全可设;见 `设计/00-交互层` 推论9)。
    /// 0 = 保持当前默认。重建缓存=清空当前工作集(同 nap 性质,磁盘指针图不动),累计指标归零。
    pub fn set_cache_capacity(&mut self, cap: usize) {
        if cap == 0 {
            return;
        }
        self.cache = RefCell::new(NeighborCache::new(cap));
    }

    /// 当前邻域缓存容量(供面板回显)。
    pub fn cache_capacity(&self) -> usize {
        self.cache.borrow().capacity()
    }

    /// 8K 最近 ring 的节点 id(时间序:旧→新),装到 ring 字符预算满为止。
    fn recent_ring_ids(&self) -> Vec<String> {
        let budget = self.context.ring_budget_chars();
        let metas = self.timeline.metas();
        let mut picked: Vec<String> = Vec::new();
        let mut used = 0usize;
        // 从最新往旧累加,够预算就停;最后翻成旧→新。
        for m in metas.iter().rev() {
            let len = self.timeline.content(&m.id).map(|c| c.len()).unwrap_or(0);
            if used + len > budget && !picked.is_empty() {
                break;
            }
            used += len;
            picked.push(m.id.clone());
        }
        picked.reverse();
        picked
    }

    // --- P5 维护:疲劳 / 做梦 / 睡眠 / 小息(`设计/02` 维护节 + `补遗/做梦睡眠期也在检索`) ---

    /// 当前未还碎片数(mesh 跳转跳过、待做梦复查的中段)。
    pub fn fragment_count(&self) -> usize {
        self.fragments.len()
    }

    /// 累计做梦还清的碎片数(观测)。
    pub fn fragments_cleared(&self) -> u64 {
        self.fragments.cleared()
    }

    /// 疲劳度(0~1)——不看 CPU/内存,看三指标加权:缓存命中率低 + 淘汰频繁 + 碎片占比大。
    /// 无任何检索活动时为 0(新系统不算累)。是"要不要睡"的启发式信号,非精确度量。
    pub fn fatigue(&self) -> f64 {
        let c = self.cache.borrow();
        let hit_term = if c.accesses() > 0 { 1.0 - c.hit_rate() } else { 0.0 };
        drop(c);
        // ★evict 项改读工作区真实置换 churn(2026-06-15)★(此前读 L2 邻域边缓存,RAG 命中时恒 0)。
        let evict_term = self.context.replacement_rate();
        let total = self.timeline.len().max(1) as f64;
        let frag_term = (self.fragment_count() as f64 / total).min(1.0);
        let w = self.fatigue_cfg;
        (w.w_hitrate * hit_term + w.w_evict * evict_term + w.w_fragment * frag_term).clamp(0.0, 1.0)
    }
}

impl Default for Memory {
    fn default() -> Self {
        Self::new()
    }
}
