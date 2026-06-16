//! 飞轮 —— 把每次实践沉淀成可复用、可演化的认知。
//!
//! 实现 `设计/04-飞轮学习.md` 推论1 五阶段闭环的前半:
//! - 收集 `collect`:每次操作的客观快照 → 经验级结论(不判断、不打标签、不推因果)。
//! - 提炼+压缩 `turn`:多条同类经验聚类 → 交 Reasoner 提炼出不变模式 → 知识级结论。
//!
//! 验证/泛化(需真跑实验)留给 app 的 Agent 循环 ④⑤,本 crate 只管认知压缩这一环。
//! 存原文/检索归 memory;这里只读 memory 的结论、写回更高压缩率的结论。

use std::collections::HashSet;

use async_trait::async_trait;
use growbox_core::{Conclusion, Confidence};
use growbox_memory::Memory;

/// 一次操作的客观快照(收集阶段的输入)。
///
/// 客观记录,不评判:`success` 是观察到的退出信号(命令退出码/有无报错),不是"好/坏"判断。
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// 做了什么操作。
    pub operation: String,
    /// 环境/前提(客观背景)。
    pub context: String,
    /// 观察到的结果(改了什么 / 输出 / 报错)。
    pub outcome: String,
    /// 客观成功信号(非主观判断)。
    pub success: bool,
}

impl Snapshot {
    pub fn new(operation: impl Into<String>, outcome: impl Into<String>, success: bool) -> Self {
        Snapshot { operation: operation.into(), context: String::new(), outcome: outcome.into(), success }
    }
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = context.into();
        self
    }
}

/// 提炼结果 —— 一簇同类经验里抽出的不变模式。
#[derive(Debug, Clone)]
pub struct Distillation {
    /// 不变的操作模式。
    pub operation: String,
    /// 不变的预期后果。
    pub expected: String,
    /// 反推出的最少前提。
    pub prerequisites: Vec<String>,
}

/// 一个被**提议**的 skill(设计/09 S3 = 飞轮自学:从反复出现的经验聚类里起草,待用户采纳)。
/// 结晶谱「经验 → Skill」的飞轮半:idle 看到同类经验反复成模式 → 提议把它沉淀成命名 playbook。
#[derive(Debug, Clone, PartialEq)]
pub struct ProposedSkill {
    /// kebab-case 名(采纳后即 skill 名)。
    pub name: String,
    /// 触发描述(一句话,何时用)。
    pub trigger: String,
    /// playbook 正文(markdown,带判断的步骤)。
    pub body: String,
}

/// 飞轮的"潜意识" —— 压缩阶段需要 LLM 抽象,但本 crate 不直接依赖 llm。
///
/// app 用真 LlmClient 实现;测试用 mock。与 memory 的 `Subconscious` 平行,各管一摊。
#[async_trait]
pub trait Reasoner: Send + Sync {
    /// 从一组同类经验里提炼不变模式。
    /// 返回 `None` = 这簇没有共同模式(暂作噪音,不压缩;留待 P2 探索)。
    async fn distill(&self, cluster: &[Conclusion]) -> Option<Distillation>;

    /// ★设计/09 S3★ 从一簇反复出现的经验里**提议**一个可复用 skill(结晶谱:经验 → Skill)。
    /// LLM 兼当质量闸:只在确有可泛化、值得命名沉淀的 playbook 时返回 `Some`;否则(太具体/噪音/
    /// 已是常识)返回 `None`,不提议。**默认 `None`**——mock / 无 LLM / 未实现该能力的 Reasoner
    /// 自动"不提议",零行为变更(idle 飞轮的提议是纯增量、可选环节)。
    async fn propose_skill(&self, _cluster: &[Conclusion]) -> Option<ProposedSkill> {
        None
    }
}

/// 飞轮引擎。无状态(状态在 memory),只持有聚类参数。
pub struct Flywheel {
    /// 聚类相似度阈值(Jaccard,0~1)。
    sim_threshold: f32,
}

impl Default for Flywheel {
    fn default() -> Self {
        Flywheel { sim_threshold: 0.3 }
    }
}

impl Flywheel {
    pub fn new() -> Self {
        Self::default()
    }

    /// 收集:客观快照 → 经验级结论(压缩率 0,信度中性)。
    ///
    /// 这是脊柱每次操作后必调的一步(见 `系统架构/05` 已知坑)。
    pub fn collect(&self, snapshot: Snapshot) -> Conclusion {
        let source = if snapshot.success { "采集:成功" } else { "采集:失败" };
        let mut c = Conclusion::experience(snapshot.operation, snapshot.outcome, source);
        if !snapshot.context.is_empty() {
            c = c.with_prerequisites(vec![snapshot.context]);
        }
        c
    }

    /// 取仍然活跃、未被压缩过(压缩率 0)的经验 —— idle 消化的"镜像"输入。
    /// 克隆出来供 IdleWorker 无锁处理,前台可继续往 memory 写。
    pub fn active_experiences(mem: &Memory) -> Vec<Conclusion> {
        mem.conclusions().iter().filter(|c| c.is_active() && c.compression == 0.0).cloned().collect()
    }

    /// 把经验聚成"成模式的簇"(≥2 条),返回每簇的成员克隆。单条簇(不成模式)被过滤。
    pub fn clusters_of(&self, experiences: &[Conclusion]) -> Vec<Vec<Conclusion>> {
        cluster(experiences, self.sim_threshold)
            .into_iter()
            .filter(|idxs| idxs.len() >= 2)
            .map(|idxs| idxs.iter().map(|&i| experiences[i].clone()).collect())
            .collect()
    }

    /// 压缩一簇(★无锁单元★:只算,不碰 Memory)。交 Reasoner 抽不变模式,产出
    /// (新知识结论, 被它折叠的经验 id 列表)。`None` = 这簇无共同模式(噪音,留待 P2)。
    /// IdleWorker 在无锁状态下调它(慢的 LLM 在这里),再用极短的锁把结果写回。
    pub async fn distill_cluster(
        &self,
        members: &[Conclusion],
        reasoner: &dyn Reasoner,
    ) -> Option<(Conclusion, Vec<String>)> {
        if members.len() < 2 {
            return None;
        }
        let d = reasoner.distill(members).await?;
        // 压缩率随簇规模单调上升(压缩越多条,适用越广);信度算出来。
        let n = members.len() as u32;
        let compression = 1.0 - 1.0 / n as f32;
        let confidence = Confidence::Knowledge { supporting: n, contradicting: 0 };
        let source = members.iter().map(|c| c.id.as_str()).collect::<Vec<_>>().join(",");
        let knowledge = Conclusion::derived(d.operation, d.expected, source, compression, confidence)
            .with_prerequisites(d.prerequisites);
        let superseded = members.iter().map(|c| c.id.clone()).collect();
        Some((knowledge, superseded))
    }

    /// 写回一簇的压缩结果(★极短锁内调用★):append 新知识 + 幂等标记旧经验被取代。
    pub fn apply_distilled(mem: &mut Memory, knowledge: Conclusion, superseded: &[String]) {
        let new_id = knowledge.id.clone();
        mem.ingest_conclusion(knowledge);
        for id in superseded {
            mem.supersede(id, &new_id); // append-only,留进化史;幂等
        }
    }

    /// 转一轮(同步全压,持 &mut Memory 全程)——单测/兜底用。
    /// 运行时的"无锁 + 镜像"消化由 IdleWorker 用 `active_experiences`/`clusters_of`/
    /// `distill_cluster`/`apply_distilled` 编排(见 gui::idle)。
    ///
    /// 幂等:被折叠进知识的经验会标 `superseded_by`,下轮 `is_active` 过滤掉,不重复压缩。
    /// 返回本轮新产出的结论数。
    pub async fn turn(&self, mem: &mut Memory, reasoner: &dyn Reasoner) -> usize {
        let experiences = Self::active_experiences(mem);
        if experiences.len() < 2 {
            return 0;
        }
        let mut produced = 0;
        for members in self.clusters_of(&experiences) {
            if let Some((knowledge, superseded)) = self.distill_cluster(&members, reasoner).await {
                Self::apply_distilled(mem, knowledge, &superseded);
                produced += 1;
            }
        }
        produced
    }
}

/// 按操作/预期文本相似度贪心聚类。返回每簇的下标集合(含单元素簇)。
///
/// 纯函数,无 LLM,可独立单测。代表元用簇首,新元素与簇首相似度达阈值即入簇。
pub fn cluster(items: &[Conclusion], threshold: f32) -> Vec<Vec<usize>> {
    let tokens: Vec<HashSet<String>> = items
        .iter()
        .map(|c| tokenize(&format!("{} {}", c.operation, c.expected)))
        .collect();
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    'next: for i in 0..items.len() {
        for cl in clusters.iter_mut() {
            if jaccard(&tokens[i], &tokens[cl[0]]) >= threshold {
                cl.push(i);
                continue 'next;
            }
        }
        clusters.push(vec![i]);
    }
    clusters
}

/// 文本切词:ASCII 连续字母数字成词(长度≥2),CJK 单字成词。混合文本都能切。
fn tokenize(s: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut buf = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            buf.push(ch.to_ascii_lowercase());
            continue;
        }
        flush(&mut buf, &mut out);
        if is_cjk(ch) {
            out.insert(ch.to_string());
        }
    }
    flush(&mut buf, &mut out);
    out
}

fn flush(buf: &mut String, out: &mut HashSet<String>) {
    if buf.chars().count() >= 2 {
        out.insert(std::mem::take(buf));
    } else {
        buf.clear();
    }
}

fn is_cjk(ch: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&ch)
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    let inter = a.intersection(b).count() as f32;
    let uni = a.union(b).count() as f32;
    if uni == 0.0 {
        0.0
    } else {
        inter / uni
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock Reasoner:把簇里第一条的操作作为模式,前提取各条上下文。
    struct MockReasoner;
    #[async_trait]
    impl Reasoner for MockReasoner {
        async fn distill(&self, cluster: &[Conclusion]) -> Option<Distillation> {
            let first = cluster.first()?;
            Some(Distillation {
                operation: format!("模式:{}", first.operation),
                expected: first.expected.clone(),
                prerequisites: vec!["最少前提".into()],
            })
        }
    }

    /// 永不提炼出模式的 Reasoner(模拟"全是噪音")。
    struct NullReasoner;
    #[async_trait]
    impl Reasoner for NullReasoner {
        async fn distill(&self, _cluster: &[Conclusion]) -> Option<Distillation> {
            None
        }
    }

    #[test]
    fn collect_makes_experience() {
        let fw = Flywheel::new();
        let c = fw.collect(Snapshot::new("加 JSON 要求", "输出变 JSON", true));
        assert_eq!(c.compression, 0.0);
        assert!(matches!(c.confidence, Confidence::Experience));
        assert!(c.is_active());
    }

    #[test]
    fn collect_keeps_context_as_premise() {
        let fw = Flywheel::new();
        let c = fw.collect(Snapshot::new("op", "out", false).with_context("flash 模型"));
        assert_eq!(c.prerequisites, vec!["flash 模型".to_string()]);
        assert_eq!(c.source, "采集:失败");
    }

    #[test]
    fn cluster_groups_similar_separates_distinct() {
        let items = vec![
            Conclusion::experience("flash token 不足", "工具调用截断成空参", "s"),
            Conclusion::experience("flash token 太小", "工具调用被截断", "s"),
            Conclusion::experience("配置 npm 镜像", "下载加速", "s"),
        ];
        let clusters = cluster(&items, 0.3);
        // 前两条同模式应在一簇,第三条独立。
        let sizes: Vec<usize> = {
            let mut s: Vec<usize> = clusters.iter().map(|c| c.len()).collect();
            s.sort_unstable();
            s
        };
        assert_eq!(sizes, vec![1, 2]);
    }

    #[tokio::test]
    async fn turn_distills_cluster_into_knowledge() {
        let fw = Flywheel::new();
        let mut mem = Memory::new();
        mem.ingest_conclusion(Conclusion::experience("flash token 不足", "工具调用截断成空参", "s"));
        mem.ingest_conclusion(Conclusion::experience("flash token 太小", "工具调用被截断", "s"));

        let produced = fw.turn(&mut mem, &MockReasoner).await;
        assert_eq!(produced, 1, "一个同类簇应压出一条知识");

        let knowledge: Vec<_> = mem.conclusions().iter().filter(|c| c.is_active() && c.compression > 0.0).collect();
        assert_eq!(knowledge.len(), 1);
        assert!(matches!(knowledge[0].confidence, Confidence::Knowledge { supporting: 2, .. }));
        // 原经验被取代(留史,但不再活跃)。
        let active_exp = mem.conclusions().iter().filter(|c| c.is_active() && c.compression == 0.0).count();
        assert_eq!(active_exp, 0, "被折叠的经验应标记取代");
    }

    #[tokio::test]
    async fn turn_is_idempotent() {
        let fw = Flywheel::new();
        let mut mem = Memory::new();
        mem.ingest_conclusion(Conclusion::experience("flash token 不足", "截断空参", "s"));
        mem.ingest_conclusion(Conclusion::experience("flash token 太小", "截断空参", "s"));

        assert_eq!(fw.turn(&mut mem, &MockReasoner).await, 1);
        assert_eq!(fw.turn(&mut mem, &MockReasoner).await, 0, "再转不应重复压缩");
    }

    #[tokio::test]
    async fn turn_skips_noise_cluster() {
        let fw = Flywheel::new();
        let mut mem = Memory::new();
        mem.ingest_conclusion(Conclusion::experience("flash token 不足", "截断", "s"));
        mem.ingest_conclusion(Conclusion::experience("flash token 太小", "截断", "s"));
        // Reasoner 判定无共同模式 → 不产出,经验保留活跃待 P2。
        assert_eq!(fw.turn(&mut mem, &NullReasoner).await, 0);
        let active_exp = mem.conclusions().iter().filter(|c| c.is_active() && c.compression == 0.0).count();
        assert_eq!(active_exp, 2, "噪音簇不折叠经验");
    }

    #[tokio::test]
    async fn turn_needs_two() {
        let fw = Flywheel::new();
        let mut mem = Memory::new();
        mem.ingest_conclusion(Conclusion::experience("孤立经验", "结果", "s"));
        assert_eq!(fw.turn(&mut mem, &MockReasoner).await, 0);
    }
}
