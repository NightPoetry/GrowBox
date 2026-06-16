//! 二级索引 —— 精确层"远处拉近"(`设计/02` 五件套之"二级索引 / 碎片"的索引那半;
//! 碎片那半已由 `fragments.rs` + 做梦落地)。
//!
//! 问题:一个热历史节点随对话推进会漂离当前前沿。它唯一的捷径(一级索引 = 磁盘 mesh 里
//! 那条 `source→target` 边)根在**旧 source** 上;未来查询的 RAG 入口未必落到那个旧 source,
//! 于是 mesh 导航够不着它,只能退化成线性扫。
//!
//! 解法(`设计/02` 案例):
//! - 某热节点在缓存"待满 K 个窗口"(漂移 ≥ K 窗口)→ 在**当前位置**建一条二级锚点指回它
//!   (粗粒度捷径,把远处的它拉近到前沿 ~1 跳)。
//! - 再待满 2K → 锚点粗化(改指一级索引而非原始位置),刷新到新的当前前沿,删旧二级。
//! - **永远只有(一级,二级)两层**:每个 target 至多一条二级锚点;一级始终是磁盘那条边。
//!   两锚点之间跳过的时间线区间 = 碎片(欠债,记进 `FragmentLedger`,做梦来还)。
//!
//! 与 `NeighborCache` / `FragmentLedger` 同性质:**有界 RAM 瞬态,可再生**(重启清零,
//! 下次检索再生成),真相在磁盘 mesh。容量满则淘汰最冷(LFU by heat)。

/// 一个窗口 = 多少个时间线节点(与精确层线性扫批量 `SCAN_BATCH` 同标度)。
pub const WINDOW: usize = 8;

/// 一条二级锚点:把一个热的远端节点拉近到当前前沿。
#[derive(Debug, Clone, PartialEq)]
pub struct Anchor {
    /// 远端热节点(原始位置)= 拉近的目标。
    pub target: String,
    /// 一级索引:target 的原始边 source(粗化后锚点"改指一级索引"即引用它)。
    pub l1_source: String,
    /// 上次建/刷锚点时的前沿长度(时间线节点数),用于度量后续漂移。
    pub anchored_at_len: usize,
    /// 复用热度(被拉近命中即 +1);容量满时淘汰最冷。
    pub heat: u32,
    /// 层级:1 = 指原始位置;2 = 已粗化(漂移 ≥ 2K,改指一级索引)。
    pub level: u8,
}

/// 有界二级索引(按 target 唯一;满则淘汰最冷)。
pub struct SecondaryIndex {
    anchors: Vec<Anchor>,
    capacity: usize,
    /// K:漂移达几个窗口才建二级锚点(2K 触发粗化)。
    k_windows: usize,
}

impl SecondaryIndex {
    pub fn new(capacity: usize, k_windows: usize) -> Self {
        SecondaryIndex {
            anchors: Vec::new(),
            capacity: capacity.max(1),
            k_windows: k_windows.max(1),
        }
    }

    /// 命中/复查一个远端 target 时调用,按漂移窗口数维护二级锚点(`设计/02` 案例的 K / 2K 规则)。
    /// - `windows_away` < K:还近,mesh 一级足够,不建二级(若已有则视作回到近端,撤掉)。
    /// - K ≤ `windows_away` < 2K:建/刷二级锚点(level 1,指原始位置),拉近到当前前沿。
    /// - `windows_away` ≥ 2K:粗化(level 2,改指一级索引),刷新到当前前沿。
    ///
    /// 同 target 已在账上 = 刷新(热度 +1、更新前沿与层级);否则新建(满则淘汰最冷)。
    pub fn register(&mut self, target: &str, l1_source: &str, windows_away: usize, current_len: usize) {
        if windows_away < self.k_windows {
            // 回到近端:撤掉旧二级(永远只在"远"时存在)。
            self.anchors.retain(|a| a.target != target);
            return;
        }
        let level: u8 = if windows_away >= 2 * self.k_windows { 2 } else { 1 };
        if let Some(a) = self.anchors.iter_mut().find(|a| a.target == target) {
            a.heat = a.heat.saturating_add(1);
            a.anchored_at_len = current_len; // 刷新到当前前沿(拉近)
            a.l1_source = l1_source.to_string();
            a.level = a.level.max(level); // 层级单调粗化,不回退
            return;
        }
        // 新建:满则先淘汰最冷(heat 最小)。
        if self.anchors.len() >= self.capacity {
            if let Some(pos) = self
                .anchors
                .iter()
                .enumerate()
                .min_by_key(|(_, a)| a.heat)
                .map(|(i, _)| i)
            {
                self.anchors.remove(pos);
            }
        }
        self.anchors.push(Anchor {
            target: target.to_string(),
            l1_source: l1_source.to_string(),
            anchored_at_len: current_len,
            heat: 1,
            level,
        });
    }

    /// 拉近候选:最热的若干 target(检索时注入前沿,即便当前 RAG 入口够不着也能召回)。
    /// 按 heat 降序取前 `n` 个 target id。
    pub fn pull_targets(&self, n: usize) -> Vec<String> {
        let mut idx: Vec<&Anchor> = self.anchors.iter().collect();
        idx.sort_by_key(|a| std::cmp::Reverse(a.heat));
        idx.into_iter().take(n).map(|a| a.target.clone()).collect()
    }

    /// 命中一个被拉近的 target:热度 +1(检索经二级锚点召回后调)。
    pub fn bump(&mut self, target: &str) {
        if let Some(a) = self.anchors.iter_mut().find(|a| a.target == target) {
            a.heat = a.heat.saturating_add(1);
        }
    }

    /// 撤掉某 target 的二级锚点(目标失效 / 已被一级 mesh 直接覆盖)。
    pub fn drop_target(&mut self, target: &str) {
        self.anchors.retain(|a| a.target != target);
    }

    /// 当前二级锚点数(观测/面板 secondary_indexes.total)。
    pub fn len(&self) -> usize {
        self.anchors.len()
    }
    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }

    /// 清空(小息 Nap:擦黑板——二级索引是工作集,清掉下次再生成)。
    pub fn clear(&mut self) {
        self.anchors.clear();
    }

    /// 只读快照(测试/观测)。
    pub fn anchors(&self) -> &[Anchor] {
        &self.anchors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_target_builds_no_anchor() {
        let mut s = SecondaryIndex::new(8, 2); // K=2 窗口
        s.register("t", "src", 1, 100); // 漂移 1 窗口 < K=2 → 不建
        assert!(s.is_empty());
    }

    #[test]
    fn far_target_builds_level1_then_coarsens_at_2k() {
        let mut s = SecondaryIndex::new(8, 2); // K=2,2K=4
        s.register("t", "src", 2, 100); // K ≤ 2 < 2K → level 1
        assert_eq!(s.len(), 1);
        assert_eq!(s.anchors()[0].level, 1);
        assert_eq!(s.anchors()[0].anchored_at_len, 100);

        s.register("t", "src2", 4, 200); // ≥ 2K → 粗化 level 2,刷新前沿
        assert_eq!(s.len(), 1, "同 target 仍只一条(永远只两层)");
        assert_eq!(s.anchors()[0].level, 2, "改指一级索引");
        assert_eq!(s.anchors()[0].anchored_at_len, 200, "刷新到当前前沿(拉近)");
        assert_eq!(s.anchors()[0].heat, 2, "复用累热");
    }

    #[test]
    fn returning_near_drops_anchor() {
        let mut s = SecondaryIndex::new(8, 2);
        s.register("t", "src", 3, 100);
        assert_eq!(s.len(), 1);
        s.register("t", "src", 0, 110); // 又回到近端 → 撤掉
        assert!(s.is_empty());
    }

    #[test]
    fn capacity_evicts_coldest() {
        let mut s = SecondaryIndex::new(2, 1);
        s.register("a", "s", 5, 100);
        s.register("b", "s", 5, 100);
        s.bump("a");
        s.bump("a"); // a 更热
        s.register("c", "s", 5, 100); // 满 → 淘汰最冷(b)
        assert_eq!(s.len(), 2);
        assert!(s.anchors().iter().any(|x| x.target == "a"));
        assert!(s.anchors().iter().any(|x| x.target == "c"));
        assert!(!s.anchors().iter().any(|x| x.target == "b"), "最冷的 b 被淘汰");
    }

    #[test]
    fn pull_targets_hottest_first() {
        let mut s = SecondaryIndex::new(8, 1);
        s.register("a", "s", 5, 100);
        s.register("b", "s", 5, 100);
        s.bump("b");
        let pulled = s.pull_targets(1);
        assert_eq!(pulled, vec!["b"], "最热的先拉近");
    }
}
