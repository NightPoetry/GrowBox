//! 碎片台账 —— 精确层欠债记账(`设计/02` 五件套之"二级索引 / 碎片")。
//!
//! 网状飞轮走快车道(mesh 跳转:入口 → 前沿目标)时,**跳过了入口与目标之间的一段中段**。
//! 这段没被 judge 过的中段 = 碎片 = 债:快扫可能漏了里面相关的节点。做梦(`Memory::dream_once`)
//! 就是来还这笔债——读中段间隙 → 潜意识 LLM 判有无遗漏 → 补边入索引 → 清碎片标记。
//!
//! 与 `NeighborCache` 同性质:**有界 RAM 工作台账,瞬态**(不落盘)。重启即清零——碎片只是
//! "待复查的优化欠条",不是真相;丢了无非下次检索再生成,无损正确性。容量满则丢最旧(FIFO)。

/// 一条碎片:一次 mesh 跳转跳过的中段。
#[derive(Debug, Clone)]
pub struct Fragment {
    /// 跳转入口节点 id(债的"近端")。
    pub entry: String,
    /// 跳到的目标节点 id(债的"远端");中段 = 时间线上 entry 与 target 之间。
    pub target: String,
    /// 触发这次跳转的查询原文(做梦复查中段时交潜意识 LLM 按它判相关)。
    pub query: String,
    /// 触发这次跳转的查询向量(做梦补边入索引时作边的键)。
    pub topic: Vec<f32>,
}

/// 有界碎片台账(FIFO 淘汰最旧)。
pub struct FragmentLedger {
    items: std::collections::VecDeque<Fragment>,
    capacity: usize,
    /// 累计还清的碎片数(做梦清掉的;观测/做梦报告用)。
    cleared: u64,
}

impl FragmentLedger {
    pub fn new(capacity: usize) -> Self {
        FragmentLedger {
            items: std::collections::VecDeque::new(),
            capacity: capacity.max(1),
            cleared: 0,
        }
    }

    /// 记一笔碎片债。同 (entry,target) 已在账上则不重复记(去重,免做梦重复复查同一段)。
    /// 满则丢最旧腾位。
    pub fn record(
        &mut self,
        entry: impl Into<String>,
        target: impl Into<String>,
        query: impl Into<String>,
        topic: Vec<f32>,
    ) {
        let entry = entry.into();
        let target = target.into();
        if self.items.iter().any(|f| f.entry == entry && f.target == target) {
            return;
        }
        while self.items.len() >= self.capacity {
            self.items.pop_front();
        }
        self.items.push_back(Fragment { entry, target, query: query.into(), topic });
    }

    /// 取出一笔最旧的碎片来还债(做梦逐笔处理)。空则 None。
    pub fn pop(&mut self) -> Option<Fragment> {
        self.items.pop_front()
    }

    /// 标记还清一笔(做梦处理完一笔后调,累加观测计数)。
    pub fn mark_cleared(&mut self) {
        self.cleared = self.cleared.saturating_add(1);
    }

    /// 当前未还碎片数。
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// 累计已还清碎片数。
    pub fn cleared(&self) -> u64 {
        self.cleared
    }

    /// 清空未还碎片(小息 Nap:擦黑板)。
    pub fn clear(&mut self) {
        self.items.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_dedups_same_interval() {
        let mut l = FragmentLedger::new(8);
        l.record("a", "z", "q", vec![1.0]);
        l.record("a", "z", "q", vec![1.0]); // 同段不重复
        l.record("a", "y", "q", vec![1.0]); // 不同段
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn fifo_evicts_oldest_when_full() {
        let mut l = FragmentLedger::new(2);
        l.record("a", "1", "q", vec![1.0]);
        l.record("a", "2", "q", vec![1.0]);
        l.record("a", "3", "q", vec![1.0]); // 满 → 丢最旧 (a,1)
        assert_eq!(l.len(), 2);
        // 最旧的应是 (a,2)。
        assert_eq!(l.pop().unwrap().target, "2");
    }

    #[test]
    fn pop_and_clear_tracking() {
        let mut l = FragmentLedger::new(8);
        l.record("a", "z", "q", vec![1.0]);
        assert!(l.pop().is_some());
        l.mark_cleared();
        assert_eq!(l.cleared(), 1);
        assert!(l.is_empty());
        l.record("b", "y", "q", vec![1.0]);
        l.clear();
        assert!(l.is_empty());
    }
}
