//! 邻域缓存 —— 磁盘指针图(`store.rs` EDGES)之上的有界 RAM 工作集。
//!
//! ★命名(2026-06-16 统一概念)★:这是**索引区的 L2 翻图加速器**(给"沿指针图导航"提速),
//! **不是记忆存放区、不是"记忆缓存"**——它空/满都不代表记忆有没有进场。真正的临时记忆存放区(缓存队列)
//! 是 `context.rs` 的 `ContextWindow`;面板"记忆缓存/缓存队列"指标读 ContextWindow,绝不读这里
//!（曾把本缓存当记忆缓存摆上面板=框架错误,见 `用户决策/记忆架构-索引区与存放区.md`)。
//!
//! 实现 `设计/02` 推论2"三级缓存"+ `计划/precision-layer.md` 阶段2:
//! 热 source 的出边邻域留内存,按 heat(LFU)淘汰最冷,miss 回盘读。内存占用 = 工作集,
//! 与总记忆量解耦。三级(1:2:4)是按 heat 的展示分层,喂 P5 疲劳度 / P6 面板;
//! 查找本身 O(1)(HashMap),"最热级先查"在此退化为分层视图,不影响正确性。

use std::collections::HashMap;

use crate::pointer::Pointer;

struct Entry {
    neighbors: Vec<Pointer>,
    heat: u32,
}

/// 有界邻域缓存(LFU 淘汰)。`source -> 出边`。
pub struct NeighborCache {
    map: HashMap<String, Entry>,
    capacity: usize,
    hits: u64,
    misses: u64,
    evictions: u64,
}

impl NeighborCache {
    pub fn new(capacity: usize) -> Self {
        NeighborCache { map: HashMap::new(), capacity: capacity.max(1), hits: 0, misses: 0, evictions: 0 }
    }

    /// 取缓存的邻域;命中则 heat+1。miss 返回 None(调用方回盘读后 `put`)。
    pub fn get(&mut self, source: &str) -> Option<Vec<Pointer>> {
        if let Some(e) = self.map.get_mut(source) {
            e.heat = e.heat.saturating_add(1);
            self.hits += 1;
            Some(e.neighbors.clone())
        } else {
            self.misses += 1;
            None
        }
    }

    /// 放入/刷新一个 source 的邻域(回盘读后调)。
    /// 新键插入前先淘汰已有最冷腾位——避免刚载入(正被使用)的条目自己成为最冷被立刻淘汰。
    pub fn put(&mut self, source: &str, neighbors: Vec<Pointer>) {
        if !self.map.contains_key(source) {
            while self.map.len() >= self.capacity {
                self.evict_coldest();
            }
        }
        self.map.insert(source.to_string(), Entry { neighbors, heat: 1 });
    }

    /// 失效一个 source(其边在盘上被改/删后,下次 miss 回盘重读最新)。
    pub fn invalidate(&mut self, source: &str) {
        self.map.remove(source);
    }

    fn evict_coldest(&mut self) {
        if let Some(k) = self.map.iter().min_by_key(|(_, e)| e.heat).map(|(k, _)| k.clone()) {
            self.map.remove(&k);
            self.evictions += 1;
        }
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn evictions(&self) -> u64 {
        self.evictions
    }
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 清空缓存条目(小息 Nap:擦掉热工作集,但磁盘图本身不动)。累计指标保留(观测连续性)。
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// 淘汰压力(0~1):累计淘汰相对容量的比值,churn 越凶越高。喂 P5 疲劳度。
    /// `evictions/(evictions+capacity)`——有界、单调、容量越大同样淘汰数压力越低。
    pub fn eviction_pressure(&self) -> f64 {
        let e = self.evictions as f64;
        e / (e + self.capacity as f64)
    }

    /// 总访问次数(hits+misses)。疲劳度判"有没有发生过检索活动"用(=0 时不算疲劳)。
    pub fn accesses(&self) -> u64 {
        self.hits + self.misses
    }

    /// 命中率(hits / (hits+misses));无访问记为 0。
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(target: &str) -> Pointer {
        Pointer::from_topic(target, vec![1.0], 0)
    }

    #[test]
    fn miss_then_hit_tracks_rate() {
        let mut c = NeighborCache::new(8);
        assert!(c.get("a").is_none()); // miss
        c.put("a", vec![p("x")]);
        assert_eq!(c.get("a").unwrap().len(), 1); // hit
        assert!((c.hit_rate() - 0.5).abs() < 1e-9); // 1 hit / 2 总
    }

    #[test]
    fn evicts_coldest_when_over_capacity() {
        let mut c = NeighborCache::new(2);
        c.put("a", vec![p("x")]);
        c.put("b", vec![p("y")]);
        // 多次命中 a、b 提热,c 进来时应淘汰最冷(从未命中的那个)。
        c.get("a");
        c.get("a");
        c.get("b");
        c.put("c", vec![p("z")]);
        assert_eq!(c.len(), 2, "超容淘汰");
        assert_eq!(c.evictions(), 1);
        assert!(c.get("c").is_some(), "新入的还在");
    }

    #[test]
    fn invalidate_forces_reload() {
        let mut c = NeighborCache::new(8);
        c.put("a", vec![p("x")]);
        c.invalidate("a");
        assert!(c.get("a").is_none(), "失效后变 miss,逼回盘重读");
    }
}
