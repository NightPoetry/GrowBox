//! 指针网络 —— 精确层飞轮的原子:一张挂在时间线上的有向图(mesh)。
//!
//! `设计/02` 推论2 第一件套"指针(原子,单向 source→target,走过的路)"。
//! 边 = source 节点 → target 节点,表示"检索 source 那次发现 target 相关"的关联。
//! 时间线是主干路,指针是架在其上的快车道。
//!
//! ★LLM 指针 vs RAG 假指针(2026-06-16 统一概念)★:本网络里的边都是**真指针(LLM 指针)**——
//! L2 顺序读原文 + 指针图导航"走过的路"的产物,必然前面都被直接/间接扫过 → 有序列位置、有碎片回收
//!（`secondary.rs` 二级锚 + `fragments.rs` 做梦还债)。RAG(ANN 向量直接跳、无扫描路径)命中**不在此建边**
//!（没有真实 source→target 路径,硬建就是虚构边、坏了指针纯净性);它作为**假指针**只在存放区
//! `context.rs` 以 `Origin::RagFake` 标签存在(统一进缓存队列吃置换,换出不落序列、不进碎片)。
//! 见 `用户决策/记忆架构-索引区与存放区.md`。
//!
//! 关键:这是**网状**结构,不是平铺的一张大表。检索靠 RAG 找少量入口节点,
//! 再沿入口的出边局部跳转(联想),只 judge 前沿那几个目标——规模再大也只看
//! 局部邻域,不退化成"线性扫所有 topic"。边按 source 邻接存,正是为此。
//!
//! **学习型边(带正负 K)**:一条边不再只记单个 topic,而是累积
//! - 正 K(`positives`):历次成功命中的 query(各带权重),边越被复用越"万能";
//! - 反 K(`negatives`):judge 判不相关的 query(硬负样本),命中即规避误跳。
//!
//! K 永远是**真实出现过的 query、零主观、全过去统计**(设计铁律,见 `计划/指针-学习型边.md`):
//! 防膨胀只用纯统计(近似坍缩 + LFU),不合成簇心、不让 LLM 写父描述。
//! 阶段递进:本阶段(1)= 数据结构 + 旧边迁移 + 行为兼容;学习累积/匹配两档/有界在后续阶段。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::subconscious::cosine;

/// 近似坍缩阈值默认值:新 query 与某已存 K 的 cosine ≥ 此值 = 近似重复,
/// **给那个真实 K `weight+1` + 刷新 last_used,不新增 K**(防膨胀第一道闸,纯统计、不合成)。
/// 运行时由 `PointerConfig.k_merge_threshold` 传入(阶段5 可调,推论9);此常量是其默认。
pub(crate) const K_MERGE_THRESHOLD: f32 = 0.93;

/// 单边正/负 K 数量上限默认值:超界 LFU 淘汰最冷真实 K(weight 最低 + last_used 最旧)。
/// 防膨胀第二道闸(纯频率/近因统计,无合成);运行时由 `PointerConfig.k_cap` 传入(阶段6 可调,推论9)。
pub(crate) const POINTER_K_CAP: usize = 8;

/// 档A 加权余弦权重增益默认值:`factor = 1 + GAIN·ln(weight)`。
/// **用 ln(weight) 而非规格字面 ln(1+weight)**:后者对 weight-1(刚建、未复用的边)也放大(factor≈1.69),
/// 会把跟随门从 cosine 阈值普遍降到 ~0.47、过度放宽且暴涨 judge;改 ln(weight) 让 weight-1 的 factor=1
/// (完全保留基线 cosine 门),只让**真正被复用**的 lane 变黏。max(非求和)防单个高频 K 把边变磁铁。
/// 运行时由 `PointerConfig.weight_gain` 传入(阶段5 可调);此常量是其默认。
pub(crate) const POINTER_WEIGHT_GAIN: f32 = 0.3;

/// 一条边的一个"K 样本" = 一次真实出现过的 query(正/负样本共用)。
/// 铁律:K = 真实过去 Q,零主观。近似坍缩时给 `weight+1`,不合成新向量。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct KSample {
    /// 当时的 query 原文(档B LLM 综合判断 + 可观测;旧边迁移时为空,新命中起补)。
    pub text: String,
    /// query 向量(档A 加权余弦档用)。
    pub vec: Vec<f32>,
    /// 命中/复用次数(近似坍缩在此累加,越多越响)。
    pub weight: u32,
    /// 最近使用时刻(epoch ms);LFU/近因淘汰、做梦合并用。旧边迁移置 0(最旧)。
    pub last_used_ms: i64,
}

impl KSample {
    /// 从一次 query 建一个权重 1 的正/负样本。
    pub fn new(text: impl Into<String>, vec: Vec<f32>, now_ms: i64) -> Self {
        Self { text: text.into(), vec, weight: 1, last_used_ms: now_ms }
    }
}

/// 一条出边(快车道),升级为**带正负 K 的学习型边**。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(from = "PointerRepr")]
pub struct Pointer {
    /// 指向的历史节点 id。
    pub target: String,
    /// 正 K:历次成功命中的 query(替代旧单 `topic`;累积学习)。
    pub positives: Vec<KSample>,
    /// 反 K:judge 判不相关的 query(硬负样本,命中即阻断跳转)。
    pub negatives: Vec<KSample>,
    /// 边总热度(≈ Σ positives.weight);平铺缓存按它淘汰。
    pub heat: u32,
}

/// 反序列化中转层:兼容**旧格式**(只有 `topic` 单向量)与**新格式**(positives/negatives)。
/// 旧边读出时把 `topic` 迁成一个正 K(text 空、weight=heat、last_used=0)。
/// 序列化永远走新格式(`Pointer` 自身的 derive),故迁移是单向、幂等、无需写库版本号。
#[derive(Deserialize)]
struct PointerRepr {
    target: String,
    #[serde(default)]
    positives: Vec<KSample>,
    #[serde(default)]
    negatives: Vec<KSample>,
    #[serde(default)]
    heat: u32,
    /// 旧格式字段:建边时的单个 query 向量。新格式无此字段。
    #[serde(default)]
    topic: Vec<f32>,
}

impl From<PointerRepr> for Pointer {
    fn from(r: PointerRepr) -> Self {
        let heat = r.heat.max(1);
        let mut positives = r.positives;
        // 旧边迁移:topic → 一个正 K(weight=heat 代表历次命中,last_used=0=最旧)。
        if positives.is_empty() && !r.topic.is_empty() {
            positives.push(KSample { text: String::new(), vec: r.topic, weight: heat, last_used_ms: 0 });
        }
        Pointer { target: r.target, positives, negatives: r.negatives, heat }
    }
}

impl Pointer {
    /// 空边(无正负 K,heat 0):用于"只想记反 K 抑制"却尚无该边时先建壳
    /// (二期 B2/B3:流程被更正版取代时,对旧版的召回边记反 K 压制——此前可能从没建过正 K 边)。
    pub fn empty(target: &str) -> Self {
        Pointer { target: target.to_string(), positives: Vec::new(), negatives: Vec::new(), heat: 0 }
    }

    /// 从单次 query 向量新建一条边(一个正 K)。
    pub fn from_topic(target: &str, topic: Vec<f32>, now_ms: i64) -> Self {
        Pointer {
            target: target.to_string(),
            positives: vec![KSample { text: String::new(), vec: topic, weight: 1, last_used_ms: now_ms }],
            negatives: Vec::new(),
            heat: 1,
        }
    }

    /// 代表性 topic 向量 = 最响(weight 最大)正 K 的向量。
    /// 阶段1 用它替换旧 `p.topic`,保持检索行为等价(单 topic 时即那一个 K);
    /// 阶段3 引入完整加权余弦(读全部正 K),届时此法仅作兜底。无正 K 返回空切片(cosine 对空返 0)。
    pub fn primary_topic(&self) -> &[f32] {
        self.positives
            .iter()
            .max_by_key(|k| k.weight)
            .map(|k| k.vec.as_slice())
            .unwrap_or(&[])
    }

    /// 档A 加权余弦(廉价档):`max over 正K: cosine(qv, k.vec) × (1 + gain·ln(k.weight))`。
    /// 取**最响**的正 K(max 非求和 → 单个高频 K 不把边变磁铁);weight 让被复用越多的 lane 越易跟随
    /// (weight=1 factor=1=基线 cosine 门)。读全部正 K = 把单 topic 泛化为"查询簇",召回更全。
    /// `gain` 由 `PointerConfig.weight_gain` 传入(可调,推论9)。
    pub fn follow_score(&self, qv: &[f32], gain: f32) -> f32 {
        self.positives
            .iter()
            .map(|k| cosine(qv, &k.vec) * (1.0 + gain * (k.weight.max(1) as f32).ln()))
            .fold(0.0_f32, f32::max)
    }

    /// 反 K 一票否决:任一反 K 与 qv 的 cosine ≥ `neg_threshold` → 阻断跳转(这条 query 曾在此 lane 误跳)。
    /// 单个命中即否决(veto);宁漏不误召回 + 省 judge。被长期误挡由 sleep 复核纠正(阶段6)。
    pub fn neg_veto(&self, qv: &[f32], neg_threshold: f32) -> bool {
        self.negatives.iter().any(|k| cosine(qv, &k.vec) >= neg_threshold)
    }

    /// 记录一次成功命中的 query(正 K):heat+1,且把 query **累积去重**进 positives。
    /// 学习逻辑集中于此(连同 `record_negative`/`merge_sample`)。一个 target 通常对应一**族**提问,
    /// 累积历次成功 Q(各带权重)= 在线学这条边的"查询簇",比单个最新 Q 更稳、召回更全。
    pub fn record_positive(&mut self, text: String, vec: Vec<f32>, now_ms: i64, merge_threshold: f32) {
        self.heat += 1;
        Self::merge_sample(&mut self.positives, text, vec, now_ms, merge_threshold);
    }

    /// 记录一次 judge 拒绝的 query(反 K):累积去重进 negatives(**不**加 heat——拒绝非复用)。
    /// = 硬负样本挖掘:误跳一次就记住,阶段3 档A 据此一票否决相似 query 的误跳。
    pub fn record_negative(&mut self, text: String, vec: Vec<f32>, now_ms: i64, merge_threshold: f32) {
        Self::merge_sample(&mut self.negatives, text, vec, now_ms, merge_threshold);
    }

    /// 近似坍缩(纯统计、不合成):新样本与某已存 K cosine ≥ `merge_threshold` →
    /// 给那个**真实 K** weight+1 + 刷新 last_used(空 text 顺带回填);否则 push 一个新真实 K。
    /// 全程无平均、无合成向量、无 LLM —— K 永远是真实出现过的 Q(设计铁律)。`merge_threshold` 可调(推论9)。
    fn merge_sample(samples: &mut Vec<KSample>, text: String, vec: Vec<f32>, now_ms: i64, merge_threshold: f32) {
        let best = samples
            .iter()
            .enumerate()
            .map(|(i, k)| (i, cosine(&vec, &k.vec)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, c)) = best {
            if c >= merge_threshold {
                let k = &mut samples[i];
                k.weight += 1;
                k.last_used_ms = now_ms;
                if k.text.is_empty() && !text.is_empty() {
                    k.text = text; // 旧边/无文本 K 首次得到原文时回填
                }
                return;
            }
        }
        samples.push(KSample::new(text, vec, now_ms));
    }

    /// LFU 封顶(防膨胀第二道闸):正/负 K 超 `cap` → 淘汰最冷的**真实 K**
    /// (weight 最低 + last_used 最旧)。纯频率/近因统计,无平均、无合成。
    pub fn enforce_cap(&mut self, cap: usize) {
        Self::cap_samples(&mut self.positives, cap);
        Self::cap_samples(&mut self.negatives, cap);
    }

    /// 留最热的 `cap` 个真实 K:按 (weight, last_used) 降序排,截断尾部最冷者。
    /// 重排不影响语义(primary_topic/follow_score/neg_veto/merge 均与顺序无关)。
    fn cap_samples(samples: &mut Vec<KSample>, cap: usize) {
        if cap == 0 || samples.len() <= cap {
            return;
        }
        samples.sort_by(|a, b| {
            b.weight.cmp(&a.weight).then(b.last_used_ms.cmp(&a.last_used_ms))
        });
        samples.truncate(cap);
    }

    /// sleep 复核反 K(维护非 bounding):移除 `last_used_ms` 早于 `now - max_age_ms` 的反 K,
    /// 让"被一次旧误判长期挡住"的边重新获得 judge 机会(下次正常重判;仍不相关会再记)。
    /// 纯统计(按时间老化),无 LLM、无合成。返回是否有移除。
    pub fn age_negatives(&mut self, now_ms: i64, max_age_ms: i64) -> bool {
        let before = self.negatives.len();
        let cutoff = now_ms - max_age_ms;
        self.negatives.retain(|k| k.last_used_ms >= cutoff);
        self.negatives.len() != before
    }
}

/// 指针网络:按 source 节点邻接存边(node -> 出边表)。
/// 磁盘原生后这是无 store(测试)时的内存兜底,真相在 redb EDGES 表。
#[derive(Default)]
pub struct PointerNet {
    adj: HashMap<String, Vec<Pointer>>,
}

impl PointerNet {
    pub fn new() -> Self {
        Self::default()
    }

    /// 边总数(供元优化/测试观测)。
    pub fn edge_count(&self) -> usize {
        self.adj.values().map(|v| v.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.adj.values().all(|v| v.is_empty())
    }

    /// source 节点的出边(无则空)。
    pub fn neighbors(&self, source: &str) -> &[Pointer] {
        self.adj.get(source).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// 建一条关联边 source→target(text/topic=本次 query 原文+向量)。
    /// 已有同 (source,target) 则走 `record_positive`(累积去重,不重复堆边);新边带首个正 K。
    pub fn link(&mut self, source: &str, target: &str, text: &str, topic: Vec<f32>, now_ms: i64, merge_threshold: f32) {
        let edges = self.adj.entry(source.to_string()).or_default();
        if let Some(p) = edges.iter_mut().find(|p| p.target == target) {
            p.record_positive(text.to_string(), topic, now_ms, merge_threshold);
        } else {
            let mut p = Pointer::from_topic(target, topic, now_ms);
            if !text.is_empty() {
                p.positives[0].text = text.to_string();
            }
            edges.push(p);
        }
    }

    /// 复用一条已存在的边:记一个正 K(去重 + heat+1)。供检索命中回填(memory 层调度)。
    pub fn record_positive(&mut self, source: &str, target: &str, text: &str, vec: Vec<f32>, now_ms: i64, merge_threshold: f32) {
        if let Some(edges) = self.adj.get_mut(source) {
            if let Some(p) = edges.iter_mut().find(|p| p.target == target) {
                p.record_positive(text.to_string(), vec, now_ms, merge_threshold);
            }
        }
    }

    /// judge 拒一条已存在的边:记一个反 K(去重,不加 heat)。
    pub fn record_negative(&mut self, source: &str, target: &str, text: &str, vec: Vec<f32>, now_ms: i64, merge_threshold: f32) {
        if let Some(edges) = self.adj.get_mut(source) {
            if let Some(p) = edges.iter_mut().find(|p| p.target == target) {
                p.record_negative(text.to_string(), vec, now_ms, merge_threshold);
            }
        }
    }

    /// 抑制一条边:记一个反 K(去重);边不存在则先建空壳再记。
    /// 二期 B2/B3:流程被更正版取代 → 对旧版召回边记反 K,使同族 query 以后规避旧版(此前可能无该边)。
    pub fn suppress(&mut self, source: &str, target: &str, text: &str, vec: Vec<f32>, now_ms: i64, merge_threshold: f32) {
        let edges = self.adj.entry(source.to_string()).or_default();
        let p = match edges.iter_mut().position(|p| p.target == target) {
            Some(i) => &mut edges[i],
            None => {
                edges.push(Pointer::empty(target));
                edges.last_mut().expect("just pushed")
            }
        };
        p.record_negative(text.to_string(), vec, now_ms, merge_threshold);
    }

    /// 对一条已存在的边执行 LFU 封顶(memory 层在 record 后调度,内存兜底路径)。
    pub fn enforce_cap_edge(&mut self, source: &str, target: &str, cap: usize) {
        if let Some(edges) = self.adj.get_mut(source) {
            if let Some(p) = edges.iter_mut().find(|p| p.target == target) {
                p.enforce_cap(cap);
            }
        }
    }

    /// sleep 复核反 K(内存兜底路径):老化各边的反 K,至多改动 `max_edges` 条,返回改动边数。
    pub fn age_all_negatives(&mut self, now_ms: i64, max_age_ms: i64, max_edges: usize) -> usize {
        let mut changed = 0;
        for edges in self.adj.values_mut() {
            for p in edges.iter_mut() {
                if changed >= max_edges {
                    return changed;
                }
                if !p.negatives.is_empty() && p.age_negatives(now_ms, max_age_ms) {
                    changed += 1;
                }
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subconscious::cosine;

    #[test]
    fn link_creates_edge() {
        let mut net = PointerNet::new();
        net.link("a", "b", "q1", vec![1.0, 0.0], 0, K_MERGE_THRESHOLD);
        let ns = net.neighbors("a");
        assert_eq!(ns.len(), 1);
        assert_eq!(ns[0].target, "b");
        assert_eq!(ns[0].heat, 1);
        assert_eq!(ns[0].positives.len(), 1, "新边一个正 K");
        assert_eq!(ns[0].positives[0].text, "q1", "新边带 query 原文");
        assert!(ns[0].negatives.is_empty());
        assert!(net.neighbors("nope").is_empty());
    }

    #[test]
    fn link_dedups_by_source_target() {
        let mut net = PointerNet::new();
        net.link("a", "b", "q", vec![1.0, 0.0], 0, K_MERGE_THRESHOLD);
        net.link("a", "b", "q", vec![0.9, 0.1], 1, K_MERGE_THRESHOLD); // 近似 Q(cos≈0.99)→ 坍缩
        assert_eq!(net.edge_count(), 1, "同 (source,target) 去重");
        assert_eq!(net.neighbors("a")[0].heat, 2, "重复建边累热度");
        assert_eq!(net.neighbors("a")[0].positives.len(), 1, "近似 Q 坍缩到一个正 K");
    }

    #[test]
    fn suppress_creates_negative_only_edge_then_vetoes() {
        // 二期 B2/B3:此前无该边,suppress 先建空壳再记反 K → 同向 query 被一票否决,且无正 K(不会被跟随)。
        let mut net = PointerNet::new();
        net.suppress("__src__", "proc1", "(被取代)", vec![1.0, 0.0], 0, K_MERGE_THRESHOLD);
        let ns = net.neighbors("__src__");
        assert_eq!(ns.len(), 1, "建出一条边");
        assert_eq!(ns[0].target, "proc1");
        assert!(ns[0].positives.is_empty(), "只有反 K、无正 K(空壳 + 反 K)");
        assert_eq!(ns[0].negatives.len(), 1);
        assert_eq!(ns[0].heat, 0, "反 K 不加 heat");
        assert!(ns[0].neg_veto(&[1.0, 0.0], 0.90), "同向 query 被否决");
        assert!(!ns[0].neg_veto(&[0.0, 1.0], 0.90), "正交 query 不否决");
        // 已存在的边再 suppress → 累积反 K(不重复建边)。
        net.suppress("__src__", "proc1", "(再次)", vec![0.0, 1.0], 1, K_MERGE_THRESHOLD);
        assert_eq!(net.edge_count(), 1, "同 (source,target) 不重复建边");
        assert_eq!(net.neighbors("__src__")[0].negatives.len(), 2, "正交反 K 各占一个");
    }

    #[test]
    fn different_targets_are_separate_edges() {
        let mut net = PointerNet::new();
        net.link("a", "b", "", vec![1.0, 0.0], 0, K_MERGE_THRESHOLD);
        net.link("a", "c", "", vec![0.0, 1.0], 0, K_MERGE_THRESHOLD);
        assert_eq!(net.neighbors("a").len(), 2, "a 的两条出边");
        assert_eq!(net.edge_count(), 2);
    }

    #[test]
    fn positives_accumulate_distinct_queries() {
        // 同边的不同查询模式(正交向量,cos=0 < 阈值)各占一个真实正 K(查询簇)。
        let mut p = Pointer::from_topic("t", vec![1.0, 0.0], 0);
        p.record_positive("q-orthogonal".into(), vec![0.0, 1.0], 1, K_MERGE_THRESHOLD);
        assert_eq!(p.positives.len(), 2, "不同查询模式各占一 K");
        assert_eq!(p.heat, 2);
        // 近似重复 → 坍缩到已存 K,weight+1,不新增。
        p.record_positive("q-near".into(), vec![0.0, 1.0], 2, K_MERGE_THRESHOLD);
        assert_eq!(p.positives.len(), 2, "近似 Q 不新增");
        let k = p.positives.iter().find(|k| k.vec == vec![0.0, 1.0]).unwrap();
        assert_eq!(k.weight, 2, "近似坍缩给真实 K weight+1");
    }

    #[test]
    fn negatives_recorded_and_dedup_no_heat() {
        let mut p = Pointer::from_topic("t", vec![1.0, 0.0], 0);
        let heat_before = p.heat;
        p.record_negative("bad-q".into(), vec![0.0, 1.0], 1, K_MERGE_THRESHOLD);
        p.record_negative("bad-q2".into(), vec![0.0, 1.0], 2, K_MERGE_THRESHOLD); // 近似 → 坍缩
        assert_eq!(p.negatives.len(), 1, "反 K 近似坍缩");
        assert_eq!(p.negatives[0].weight, 2);
        assert_eq!(p.heat, heat_before, "记反 K 不加 heat(拒绝非复用)");
        assert!(p.positives.iter().all(|k| k.vec != vec![0.0, 1.0]), "反 K 不混入正 K");
    }

    #[test]
    fn migrated_edge_backfills_text_on_next_hit() {
        // 旧边迁移来的正 K text 为空;新命中(近似 Q)坍缩时回填原文。
        let old = r#"{"target":"b","topic":[1.0,0.0],"heat":2}"#;
        let mut p: Pointer = serde_json::from_str(old).unwrap();
        assert_eq!(p.positives[0].text, "");
        p.record_positive("real query text".into(), vec![1.0, 0.0], 5, K_MERGE_THRESHOLD);
        assert_eq!(p.positives.len(), 1, "近似坍缩到迁移来的 K");
        assert_eq!(p.positives[0].text, "real query text", "空 text 回填");
    }

    #[test]
    fn follow_score_weight1_equals_raw_cosine() {
        // weight-1(刚建、未复用)factor=1 → follow_score == 裸 cosine(完全保留基线门)。
        let p = Pointer::from_topic("t", vec![1.0, 0.0], 0);
        assert!((p.follow_score(&[1.0, 0.0], POINTER_WEIGHT_GAIN) - 1.0).abs() < 1e-6);
        assert!(p.follow_score(&[0.0, 1.0], POINTER_WEIGHT_GAIN).abs() < 1e-6);
    }

    #[test]
    fn follow_score_takes_max_and_weight_boosts_reused_lane() {
        let mut p = Pointer::from_topic("t", vec![1.0, 0.0], 0); // K1 [1,0] weight1
        // 反复命中 [0,1] 方向 → 坍缩成热 K2 weight 高。
        for _ in 0..9 {
            p.record_positive("q".into(), vec![0.0, 1.0], 1, K_MERGE_THRESHOLD);
        }
        let k2 = p.positives.iter().find(|k| k.vec == vec![0.0, 1.0]).unwrap();
        assert!(k2.weight >= 9, "热 K 累积 weight");
        // 取 max:qv=[0,1] 命中热 K → 加权后 > 1.0(被复用的 lane 更黏);qv=[1,0] 命中冷 K → =1.0。
        assert!(p.follow_score(&[0.0, 1.0], POINTER_WEIGHT_GAIN) > 1.0, "热 lane 加权抬分");
        assert!((p.follow_score(&[1.0, 0.0], POINTER_WEIGHT_GAIN) - 1.0).abs() < 1e-6, "冷 K(weight1)仍裸 cosine");
    }

    #[test]
    fn neg_veto_blocks_only_similar_rejected() {
        let mut p = Pointer::from_topic("t", vec![1.0, 0.0], 0);
        p.record_negative("bad".into(), vec![0.0, 1.0], 1, K_MERGE_THRESHOLD);
        assert!(p.neg_veto(&[0.0, 1.0], 0.90), "与反 K 同向 → 一票否决");
        assert!(!p.neg_veto(&[1.0, 0.0], 0.90), "与反 K 正交 → 不否决");
    }

    #[test]
    fn enforce_cap_evicts_coldest_real_k() {
        let mut p = Pointer::from_topic("t", vec![1.0, 0.0], 0);
        // 三个不同查询模式的真实正 K:weight/last_used 各异。
        p.positives = vec![
            KSample { text: "hot".into(), vec: vec![1.0, 0.0, 0.0], weight: 5, last_used_ms: 100 },
            KSample { text: "cold".into(), vec: vec![0.0, 1.0, 0.0], weight: 1, last_used_ms: 50 },
            KSample { text: "warm".into(), vec: vec![0.0, 0.0, 1.0], weight: 3, last_used_ms: 200 },
        ];
        p.enforce_cap(2);
        assert_eq!(p.positives.len(), 2, "封顶到 2");
        assert!(p.positives.iter().any(|k| k.text == "hot"), "最热(w5)留");
        assert!(p.positives.iter().any(|k| k.text == "warm"), "次热(w3)留");
        assert!(!p.positives.iter().any(|k| k.text == "cold"), "最冷(w1)淘汰——纯频率统计,无合成");
    }

    #[test]
    fn age_negatives_removes_old_keeps_recent() {
        let mut p = Pointer::from_topic("t", vec![1.0, 0.0], 0);
        p.negatives = vec![
            KSample { text: "old".into(), vec: vec![0.0, 1.0], weight: 1, last_used_ms: 100 },
            KSample { text: "new".into(), vec: vec![1.0, 0.0], weight: 1, last_used_ms: 1000 },
        ];
        // now=1100, max_age=500 → cutoff=600;old(100)<600 移除,new(1000)≥600 留。
        assert!(p.age_negatives(1100, 500));
        assert_eq!(p.negatives.len(), 1);
        assert_eq!(p.negatives[0].text, "new", "旧误判老化,近的反 K 仍守");
        assert!(!p.age_negatives(1100, 500), "无更旧的 → 不再变");
    }

    #[test]
    fn primary_topic_guides_which_lane() {
        // a 有两条出边,primary_topic 不同;调用方靠 cosine 选该走哪条。
        let mut net = PointerNet::new();
        net.link("a", "b", "", vec![1.0, 0.0], 0, K_MERGE_THRESHOLD);
        net.link("a", "c", "", vec![0.0, 1.0], 0, K_MERGE_THRESHOLD);
        let qv = [1.0, 0.0];
        let taken: Vec<&str> = net
            .neighbors("a")
            .iter()
            .filter(|p| cosine(&qv, p.primary_topic()) >= 0.8)
            .map(|p| p.target.as_str())
            .collect();
        assert_eq!(taken, vec!["b"]);
    }

    #[test]
    fn old_format_migrates_topic_to_positive() {
        // 旧格式 JSON(只有 topic / heat,无 positives/negatives)读出 → 迁成一个正 K。
        let old = r#"{"target":"b","topic":[1.0,0.0],"heat":3}"#;
        let p: Pointer = serde_json::from_str(old).unwrap();
        assert_eq!(p.target, "b");
        assert_eq!(p.heat, 3);
        assert_eq!(p.positives.len(), 1, "topic 迁成一个正 K");
        assert_eq!(p.positives[0].vec, vec![1.0, 0.0]);
        assert_eq!(p.positives[0].weight, 3, "weight=heat 代表历次命中");
        assert_eq!(p.positives[0].text, "", "旧边无原文");
        assert!(p.negatives.is_empty());
        // primary_topic 仍指向迁移来的向量,检索行为等价。
        assert_eq!(p.primary_topic(), &[1.0, 0.0]);
    }

    #[test]
    fn new_format_roundtrips_without_topic() {
        // 新格式序列化→反序列化幂等,且不再含 topic 字段。
        let mut p = Pointer::from_topic("b", vec![0.5, 0.5], 100);
        p.record_positive("q".into(), vec![0.2, 0.8], 200, K_MERGE_THRESHOLD);
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("topic"), "新格式不写 topic 字段");
        let back: Pointer = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p, "新格式幂等");
    }
}
