//! Memory 的可调旋钮(推论9 数值全可设):指针匹配档 / 学习型指针 / 检索行为 / 疲劳权重 / 瞬态容量。
//! 纯数据 + Default(默认引用 mod.rs 的实测默认常量,经 `use super::*` 取得)。

use super::*;

/// 指针匹配档(`计划/指针-学习型边.md`):档A=廉价加权余弦(默认),档B=LLM 综合判断(精确,读正负 K 原文)。
/// [可调,推论9;阶段5 暴露为旋钮]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerMatchMode {
    #[default]
    WeightedCosine,
    LlmJudge,
}

impl PointerMatchMode {
    /// 从 Settings 字符串解析(未知值回退默认档A)。
    pub fn from_setting(s: &str) -> Self {
        match s {
            "llm_judge" => Self::LlmJudge,
            _ => Self::WeightedCosine,
        }
    }
    /// 转回 Settings 字符串。
    pub fn as_setting(self) -> &'static str {
        match self {
            Self::LlmJudge => "llm_judge",
            Self::WeightedCosine => "weighted_cosine",
        }
    }
}

/// 学习型指针的可调旋钮(推论9 数值全可设;阶段5 由 `Settings` 透传)。默认 = 各阶段实测默认常量。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerConfig {
    /// 匹配档:档A 加权余弦 / 档B LLM 综合判断。
    pub match_mode: PointerMatchMode,
    /// 跟随门:档A 加权余弦得分 ≥ 此值才走快车道。
    pub follow_threshold: f32,
    /// 反 K 一票否决阈值:query 与某反 K cosine ≥ 此值 → 阻断该边。
    pub neg_block_threshold: f32,
    /// 近似坍缩阈值:新 K 与已存 K cosine ≥ 此值 → weight+1 不新增。
    pub k_merge_threshold: f32,
    /// 档A 权重增益:`factor = 1 + gain·ln(weight)`。
    pub weight_gain: f32,
    /// 单边正/负 K 数量上限:超界 LFU 淘汰最冷真实 K(防膨胀第二道闸,纯统计)。
    pub k_cap: usize,
    /// 档A 余弦命中后是否仍走前沿 judge 确认(true=精确,false=命中即采纳省 LLM)。仅档A 有意义。
    pub force_judge_on_cosine_hit: bool,
}

impl Default for PointerConfig {
    fn default() -> Self {
        Self {
            match_mode: PointerMatchMode::WeightedCosine,
            follow_threshold: POINTER_FOLLOW_THRESHOLD,
            neg_block_threshold: NEG_BLOCK_THRESHOLD,
            k_merge_threshold: crate::pointer::K_MERGE_THRESHOLD,
            weight_gain: crate::pointer::POINTER_WEIGHT_GAIN,
            k_cap: crate::pointer::POINTER_K_CAP,
            force_judge_on_cosine_hit: POINTER_FORCE_JUDGE,
        }
    }
}

/// 检索行为的可调旋钮(推论9 数值全可设;由 `Settings` 透传)。默认 = 各检索常量。
/// 控制第一层 RAG(命中阈/候选数)与下沉精确层(进图入口/线性扫批量)的召回 vs 精度取舍。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetrievalConfig {
    /// 第一层 RAG 命中阈:首条余弦 ≥ 此值即"找到了"直接返回不下沉(无 judge 门,故关键)。调低=更爱用 RAG 浅层、少下沉。
    pub rag_hit_threshold: f32,
    /// 第一层 RAG 取回候选数(ANN top-K)。
    pub rag_topk: usize,
    /// 精确层进图入口数:向量索引取 top-K 节点作下沉的门。
    pub entry_k: usize,
    /// 精确层入口最低相似度:低于此值的 top-K 不作入口(免无关入口污染下沉)。
    pub entry_min_sim: f32,
    /// 精确层线性扫每批给 LLM judge 的节点数(一个窗口)。
    pub scan_batch: usize,
    /// 精确层线性主干路本次最多往回读多少个节点(有界"全量扫描"上限;多批渐进直到攒够/扫满/扫到最早)。
    pub scan_max: usize,
    /// 项目软偏好系数:命中属当前项目 → 相似度乘 (1+此值) 再排序。0=不偏好。软偏好非硬过滤。
    pub project_boost: f32,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            rag_hit_threshold: RAG_HIT_THRESHOLD,
            rag_topk: RAG_TOPK,
            entry_k: ENTRY_K,
            entry_min_sim: ENTRY_MIN_SIM,
            scan_batch: SCAN_BATCH,
            scan_max: SCAN_MAX,
            project_boost: PROJECT_BOOST,
        }
    }
}

/// 疲劳公式权重旋钮(推论9 数值全可设;由 `Settings` 透传)。`fatigue()` 三指标加权和:
/// `w_hitrate·命中率低 + w_evict·淘汰压力 + w_fragment·碎片占比`(clamp 0~1)。默认三权重和=1。
/// 配套 idle 的「疲劳睡眠阈」(`Settings.idle_fatigue_threshold`)——权重定怎么算累,阈值定累到多少才睡。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FatigueConfig {
    /// 缓存命中率低的权重(命中率越低越累)。
    pub w_hitrate: f64,
    /// 淘汰频繁的权重(工作集越挤越累)。
    pub w_evict: f64,
    /// 碎片占比大的权重(欠债越多越累)。
    pub w_fragment: f64,
}

impl Default for FatigueConfig {
    fn default() -> Self {
        Self {
            w_hitrate: FATIGUE_W_HITRATE,
            w_evict: FATIGUE_W_EVICT,
            w_fragment: FATIGUE_W_FRAGMENT,
        }
    }
}

/// 瞬态容量旋钮(推论9 数值全可设;由 `Settings` 透传)。这些是**可再生工作集**的有界上限
/// (真相在磁盘);调整经 `set_transient_caps` 在连接时 apply——重建碎片台账/二级索引(清空可接受,
/// 同 nap/缓存重建)、截断内部事件环、刷新反 K 复核参数。低价值旋钮(贯彻"一切默认可设")。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransientCapsConfig {
    /// 碎片台账容量(mesh 跳过中段的债,做梦来还)。
    pub fragment_ledger_cap: usize,
    /// 二级索引容量(「远处拉近」锚点)。
    pub secondary_index_cap: usize,
    /// 内部状态事件环容量(AI 感知的最近失败/事件,渲进上下文尾)。
    pub internal_events_cap: usize,
    /// 造物交互瞬态环容量(被造物 UI 回传;独立环,免挤占内部状态)。
    pub artifact_interactions_cap: usize,
    /// sleep 复核反 K 的老化阈(ms):反 K 早于"现在 - 此值"则移除,纠正历史误判。
    pub neg_review_max_age_ms: i64,
    /// sleep 每次复核反 K 至多扫的边数(有界背景维护)。
    pub neg_review_max_edges: usize,
}

impl Default for TransientCapsConfig {
    fn default() -> Self {
        Self {
            fragment_ledger_cap: FRAGMENT_LEDGER_CAP,
            secondary_index_cap: SECONDARY_INDEX_CAP,
            internal_events_cap: INTERNAL_EVENTS_CAP,
            artifact_interactions_cap: ARTIFACT_INTERACTIONS_CAP,
            neg_review_max_age_ms: NEG_REVIEW_MAX_AGE_MS,
            neg_review_max_edges: NEG_REVIEW_MAX_EDGES,
        }
    }
}

/// Skill 系统旋钮(设计/09 推论8 + 推论4;数值全可设,由 `Settings` 透传)。控制「常驻清单」治理 +
/// 哪些 skill 启用——全在设置可改(用户原则:所有能改的都能在设置控制)。`disabled` 按 skill 名
/// (小写)停用,对内置种子与已学 skill 一视同仁;停用 = 从常驻清单消失、不被语义召回、load_skill 不服务,
/// 但**不删数据、可随时重新启用**(append-only 友好,比破坏性删除更专业)。
#[derive(Debug, Clone, PartialEq)]
pub struct SkillConfig {
    /// 总开关:false = 完全不暴露 Skill(清单不拼、召回不出、load 拒)。默认开。
    pub enabled: bool,
    /// 常驻清单上限(内置优先 + 已学按新近补足到此数;被挤出者仍可语义召回/按名加载,推论4 兜底)。
    pub list_max: usize,
    /// ★自动加载阈值(设计/09 用户定调:高置信自动注入正文)★:语义召回的 skill,相似度 ≥ 此值
    /// 视为"强匹配"→ 脊柱**直接把整篇 playbook 正文注入上下文**(零 load_skill 调用 = 省 LLM 调用+加速);
    /// 低于此值只浮现名+触发(AI 自行决定要不要 load)。数值全可设。默认 0.88。
    pub autoload_threshold: f32,
    /// 停用的 skill 名(小写)。
    pub disabled: std::collections::HashSet<String>,
}

impl Default for SkillConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

impl SkillConfig {
    /// 出厂默认:开、清单上限 24、自动加载阈 0.88。
    pub fn defaults() -> Self {
        Self {
            enabled: true,
            list_max: 24,
            autoload_threshold: 0.88,
            disabled: std::collections::HashSet::new(),
        }
    }
    /// 某 skill 当前是否生效(总开关开 且 未被单独停用)。名按小写比对。
    pub fn is_active(&self, name: &str) -> bool {
        self.enabled && !self.disabled.contains(&name.to_ascii_lowercase())
    }
}
