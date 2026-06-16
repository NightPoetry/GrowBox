//! 潜意识接口 —— memory 检索时需要 LLM 帮两件事,但不直接依赖 llm crate。
//!
//! app 用真 LlmClient 实现它;测试用 mock。这样 memory 可独立单测(不打真 API)。
//! 实现 `设计/02-记忆检索`:第一层 RAG 用 embed,第二层精确用 judge_relevant(读原文)。

use async_trait::async_trait;

#[async_trait]
pub trait Subconscious: Send + Sync {
    /// 第一层 RAG:把文本向量化(用途无关的核心实现)。
    async fn embed(&self, text: &str) -> Vec<f32>;

    /// 检索查询的向量化。默认回退到 `embed`;真实现(e5)会加 `query:` 前缀。
    async fn embed_query(&self, text: &str) -> Vec<f32> {
        self.embed(text).await
    }

    /// 入库文档的向量化。默认回退到 `embed`;真实现(e5)会加 `passage:` 前缀。
    async fn embed_passage(&self, text: &str) -> Vec<f32> {
        self.embed(text).await
    }

    /// 当前向量空间指纹。换 embedder → 此值变 → 旧向量失效需重嵌。
    /// 默认空串(mock 不关心版本);真实现返回 embedder 的 `version()`。
    fn embedding_version(&self) -> String {
        String::new()
    }

    /// 第二层精确:判断这批原文里哪些和查询相关(返回相关的索引)。
    async fn judge_relevant(&self, query: &str, candidates: &[String]) -> Vec<usize>;

    /// 档B 综合判断(学习型指针,精确档):综合这条边的历史——正 K(历次命中的提问原文,已带"命中N次"
    /// 标注)与反 K(judge 曾拒的提问原文)——与当前 query,判**是否值得跳到 target**。
    /// 判"边"而非逐 target,且利用负样本,比 `judge_relevant` 信息更全(见 `计划/指针-学习型边.md` 档B)。
    /// 默认回退:对 target 单条做 `judge_relevant`(= 逐 target 行为,mock/无 LLM 自动得合理默认);
    /// 真实现(LLM)读正负 K 原文综合一次判断。
    async fn judge_edge(
        &self,
        query: &str,
        positives: &[String],
        negatives: &[String],
        target: &str,
    ) -> bool {
        let _ = (positives, negatives);
        !self.judge_relevant(query, &[target.to_string()]).await.is_empty()
    }

    /// ★文档破碎化(让 LLM 自己判断从哪破开)★:给一篇大文档**已按句末符切好的原子句序列**,
    /// 判断哪些句子**另起一块**——返回这些"块首句"的下标(升序;下标 0 隐含为首块起点,可省)。
    /// 语义连贯的相邻句应归同一块。memory 侧据此把原子句**精确拼接**成各块(零改写、零丢字),
    /// 各块独立成节点、独立向量 → RAG 能命中文档里的窄问(如"想法卡片 class 叫什么")。
    /// 默认实现 = 按 `target_chars` 贪心分组(无 LLM 也得合理切分;mock / LLM 降级用);
    /// 真实现(`bridge.rs`,LLM)读语义判断破点,失败回退本贪心。
    async fn chunk_doc(&self, sentences: &[String], target_chars: usize) -> Vec<usize> {
        greedy_chunk_bounds(sentences, target_chars)
    }
}

/// 按目标字符数贪心分组:从头累积句子,**加上当前句会超过 `target_chars` 就在当前句另起一块**。
/// 返回各块首句下标(升序、含 0、严格递增)。`target_chars == 0` = 一句一块。空输入返回空。
/// 既是 `chunk_doc` 的默认实现,也是真 LLM 实现解析失败时的回退(语义无关但保证切开,不至于又退回大节点)。
pub fn greedy_chunk_bounds(sentences: &[String], target_chars: usize) -> Vec<usize> {
    if sentences.is_empty() {
        return Vec::new();
    }
    let mut bounds = vec![0usize];
    let mut acc = 0usize;
    for (i, s) in sentences.iter().enumerate() {
        let len = s.chars().count();
        if i > 0 && (target_chars == 0 || acc + len > target_chars) {
            bounds.push(i);
            acc = 0;
        }
        acc += len;
    }
    bounds
}

/// 余弦相似度(第一层 RAG 排序用)。
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_identical_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_is_zero() {
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    fn cosine_mismatch_len_is_zero() {
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn greedy_chunk_bounds_groups_by_target() {
        let s: Vec<String> = ["aaa", "bbb", "ccc", "ddd"].iter().map(|x| x.to_string()).collect();
        // target 6:aaa+bbb=6 不超,ccc 会超 → 块首 [0,2];块 = (aaa bbb)(ccc ddd)。
        assert_eq!(greedy_chunk_bounds(&s, 6), vec![0, 2]);
        // target 0:一句一块。
        assert_eq!(greedy_chunk_bounds(&s, 0), vec![0, 1, 2, 3]);
        // 大 target:不切,整篇一块。
        assert_eq!(greedy_chunk_bounds(&s, 9999), vec![0]);
        // 空输入。
        assert!(greedy_chunk_bounds(&[], 10).is_empty());
    }
}
