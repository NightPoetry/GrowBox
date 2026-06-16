//! Memory 的 P5 维护面(设计/02 维护节 + 补遗):做梦还碎片债 / 推演 / 反K复核 / 睡眠 / 小息。

use super::*;

impl Memory {
    /// 做梦一笔:取一笔最旧碎片债,复查它跳过的中段——读间隙原文 → 潜意识 LLM 判有无遗漏 →
    /// 相关的补一条 入口→该节点 的边(入索引,下次走快车道直达)→ 清这笔碎片。
    /// 优先级由调用方保证(gui 经仲裁器 Sleep 档);此处只还一笔,可反复调至 `drained`。
    pub async fn dream_once(&mut self, sub: &dyn Subconscious) -> DreamReport {
        let Some(frag) = self.fragments.pop() else {
            return DreamReport { processed: 0, discoveries: 0, drained: true };
        };
        // 解析中段端点(独立作用域:metas 借用用完即放,后续可变借 timeline/fragments)。
        let bounds = {
            let metas = self.timeline.metas();
            let pe = metas.iter().position(|m| m.id == frag.entry);
            let pt = metas.iter().position(|m| m.id == frag.target);
            match (pe, pt) {
                (Some(a), Some(b)) => Some((a.min(b), a.max(b))),
                _ => None,
            }
        };
        let Some((lo, hi)) = bounds else {
            // 端点已不在线(append-only 下理论不会)→ 视作已还清。
            self.fragments.mark_cleared();
            return DreamReport { processed: 1, discoveries: 0, drained: self.fragments.is_empty() };
        };
        // 中段 = (lo, hi) 之间、尚未 Deep 扫过的节点(确信扫过的跳过)。
        let gap_ids: Vec<String> = {
            let metas = self.timeline.metas();
            if hi > lo + 1 {
                metas[lo + 1..hi]
                    .iter()
                    .filter(|m| m.stain != Stain::Deep)
                    .map(|m| m.id.clone())
                    .collect()
            } else {
                Vec::new()
            }
        };
        let gap: Vec<(String, String)> = gap_ids
            .into_iter()
            .filter_map(|id| self.timeline.content(&id).map(|c| (id, c)))
            .collect();
        if gap.is_empty() {
            self.fragments.mark_cleared();
            return DreamReport { processed: 1, discoveries: 0, drained: self.fragments.is_empty() };
        }
        let candidates: Vec<String> = gap.iter().map(|(_, c)| c.clone()).collect();
        let relevant = sub.judge_relevant(&frag.query, &candidates).await;
        let mut discoveries = 0;
        for (i, (id, _)) in gap.iter().enumerate() {
            self.timeline.stain(id, Stain::Deep); // 间隙被仔细复查过 = Deep(确信)
            if relevant.contains(&i) && &frag.entry != id {
                // 发现遗漏:补 入口→该节点 的边(键=原 query 原文+向量),网由此长密。
                self.link_edge(&frag.entry, id, &frag.query, frag.topic.clone());
                self.timeline.touch(id);
                discoveries += 1;
            }
            self.persist_node(id);
        }
        self.fragments.mark_cleared();
        DreamReport { processed: 1, discoveries, drained: self.fragments.is_empty() }
    }

    /// 推演一次:趁记忆网络干净时自问自答"用户若问 X,该检索到什么"——取近端一个节点的原文
    /// 当预演查询 X,跑一遍精确层(预热网/建边,顺带生成新碎片留给做梦)。返回是否真预演了一次。
    pub async fn rehearse_once(&mut self, sub: &dyn Subconscious) -> bool {
        let Some(id) = self.timeline.metas().last().map(|m| m.id.clone()) else {
            return false;
        };
        let Some(probe) = self.timeline.content(&id) else {
            return false;
        };
        if probe.trim().is_empty() {
            return false;
        }
        self.retrieve_exact(&probe, sub).await; // 预演走精确层,会建边、可能生成新碎片(好事)
        true
    }

    /// sleep 复核反 K(维护非 bounding,纯时间统计、无 LLM/无合成):老化过旧的反 K,
    /// 让被一次旧误判长期挡住的边重获 judge 机会(下次正常重判;仍不相关会再记反 K)。
    /// 有界(至多 `max_edges` 条)。返回改动边数。`now_ms`/`max_age_ms` 由调用方给(便于测试)。
    pub fn review_negatives(&mut self, now_ms: i64, max_age_ms: i64, max_edges: usize) -> usize {
        match &self.store {
            Some(s) => {
                let mut changed = 0;
                for (source, mut p) in s.edges_with_negatives(max_edges) {
                    if p.age_negatives(now_ms, max_age_ms) {
                        s.put_edge(&source, &p);
                        self.cache.borrow_mut().invalidate(&source);
                        changed += 1;
                    }
                }
                changed
            }
            None => self.pointers.age_all_negatives(now_ms, max_age_ms, max_edges),
        }
    }

    /// 睡眠:做梦 + 推演 交替,直到碎片归零后推演无题 / 到达 `max_cycles` 上限。
    /// gui 后台 worker 用 `dream_once`/`rehearse_once` 编排可取消版;此处是有界、可单测的
    /// 便捷闭环(`max_cycles` 兜住"推演生债 → 做梦还债"的循环,防无界)。
    pub async fn sleep(&mut self, sub: &dyn Subconscious, max_cycles: usize) -> SleepReport {
        let mut r = SleepReport::default();
        // 入睡先复核反 K:老化旧误判,纠正"一次误判永久封路"(阶段6 维护)。
        self.review_negatives(
            growbox_core::now().timestamp_millis(),
            self.transient_caps.neg_review_max_age_ms,
            self.transient_caps.neg_review_max_edges,
        );
        for _ in 0..max_cycles {
            if self.fragment_count() > 0 {
                let d = self.dream_once(sub).await;
                r.dreams += d.processed;
                r.discoveries += d.discoveries;
            } else if self.rehearse_once(sub).await {
                r.rehearsals += 1;
            } else {
                break; // 碎片归零且无可推演 → 睡眠自然结束。
            }
        }
        r.fragments_remaining = self.fragment_count();
        r
    }

    /// 小息(Nap,用户手动)——"擦黑板,不格式化硬盘":清当前对话工作集 + 三级缓存 + 碎片台账;
    /// 保留长期记忆(时间线 / 结论 / 磁盘指针图 / 第一层索引)。
    pub fn nap(&mut self) {
        self.context.clear();
        self.cache.borrow_mut().clear();
        self.fragments.clear();
        self.secondary.clear(); // 二级索引是工作集,擦掉下次再生成(强制跳转持久,不清)
    }
}
