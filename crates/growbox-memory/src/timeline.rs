//! 对话时间线 —— 节点按时间排成一条线(磁盘原生 + 惰性内容,P3d)。
//!
//! 实现 `设计/02-记忆检索` + `计划/precision-layer.md` 的"磁盘原生"硬约束:
//! 持久 agent 记忆无界,**不可能全量进内存**。时间线因此不再开机把所有节点的
//! 原文 `content` / 向量 `embedding` 都 load 进 RAM。常驻内存的只有每节点的轻量元信息
//! `NodeMeta`(id / 时间 / 角色 / 热度 / 染色 / 向量版本 / 是否已向量化);重的 `content`
//! 按 id 惰性从 `Store` 取,前面挡一层有界热尾缓存 `NodeCache`(LRU)。
//! 向量本体留给第一层索引(`index.rs`,当前 HNSW 驻 RAM;待 HannoyIndex 落盘)与磁盘,
//! 元信息只记 `has_embedding` / `embedding_version`,不在此驻留向量。
//!
//! 无 `Store`(单元测试)时:`NodeCache` 无界、即权威全量(不淘汰),语义与旧内存版一致。

use std::cell::RefCell;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::store::Store;

/// 热尾缓存容量(有 Store 时常驻的全节点数上限;无 Store 时无界)。
const CACHE_CAP: usize = 512;

/// 染色 —— 标记一段区域被扫描的程度,加速全量扫描(避免重复 LLM 读)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Stain {
    /// 从没到过。
    #[default]
    None,
    /// 跟指针快扫过,可能有漏。
    Light,
    /// 仔细扫过,确信无漏。
    Deep,
}

/// 时间线上的一个节点(磁盘 / 缓存里的完整形态)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    /// 原文内容(对话/文件读取/shell 输出,同一格式)。惰性载入,不常驻 RAM。
    pub content: String,
    /// 角色: "user" | "assistant" | "tool" | "system"(默认,兼容旧数据)。
    #[serde(default = "default_role")]
    pub role: String,
    pub created_at: growbox_core::Timestamp,
    /// 第一层 RAG 用的向量(可空 = 未向量化)。惰性载入,不常驻 RAM。
    #[serde(default)]
    pub embedding: Vec<f32>,
    /// 该向量出自哪个向量空间(embedder version)。与当前 embedder 不符 → 需重嵌。
    /// 空 = 未向量化或旧数据(旧数据当作需重嵌)。
    #[serde(default)]
    pub embedding_version: String,
    /// 访问次数(热度,缓存与排序参考)。
    #[serde(default)]
    pub hits: u32,
    /// 染色状态。
    #[serde(default)]
    pub stain: Stain,
    /// 所属项目 tag(软隔离):None = 未分类(旧数据/全局)。**不硬分库**——
    /// 记忆仍是一整块,这个 tag 只用于:显示历史时按当前项目过滤(方便看),
    /// 检索时对本项目命中加权(软偏好)。跨项目高相关仍自然召回。
    #[serde(default)]
    pub project_id: Option<String>,
}

fn default_role() -> String { "system".into() }

impl Node {
    pub fn new(content: impl Into<String>) -> Self {
        Self::with_role(content, "system")
    }

    pub fn with_role(content: impl Into<String>, role: impl Into<String>) -> Self {
        let content = content.into();
        let created_at = growbox_core::now();
        Node {
            id: gen_id(&content, created_at),
            content,
            role: role.into(),
            created_at,
            embedding: Vec::new(),
            embedding_version: String::new(),
            hits: 0,
            stain: Stain::None,
            project_id: None,
        }
    }
}

/// 节点的轻量元信息 —— 常驻 RAM。重的 `content`/`embedding` 不在此(惰性取)。
#[derive(Debug, Clone)]
pub struct NodeMeta {
    pub id: String,
    pub created_at: growbox_core::Timestamp,
    pub role: String,
    pub hits: u32,
    pub stain: Stain,
    pub embedding_version: String,
    /// 是否已向量化(代替 embedding 本体做"待补向量"过滤,无需把向量驻 RAM)。
    pub has_embedding: bool,
    /// 所属项目 tag(软隔离;None=未分类)。常驻 meta 便于显示过滤/检索加权不触盘。
    pub project_id: Option<String>,
}

impl NodeMeta {
    fn from_node(n: &Node) -> Self {
        NodeMeta {
            id: n.id.clone(),
            created_at: n.created_at,
            role: n.role.clone(),
            hits: n.hits,
            stain: n.stain,
            embedding_version: n.embedding_version.clone(),
            project_id: n.project_id.clone(),
            has_embedding: !n.embedding.is_empty(),
        }
    }
}

fn gen_id(content: &str, ts: growbox_core::Timestamp) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(content.as_bytes());
    h.update(ts.to_rfc3339().as_bytes());
    format!("node-{:x}", h.finalize())[..18].to_string()
}

// ===================== 热尾缓存(content/embedding 的有界 RAM 工作集) =====================

struct CacheSlot {
    node: Node,
    used: u64,
}

/// `content`/`embedding` 的有界 LRU 缓存。有 Store:miss 回盘读。
/// 无 Store(测试):容量无界,缓存即权威全量,永不淘汰。
struct NodeCache {
    map: HashMap<String, CacheSlot>,
    tick: u64,
    capacity: Option<usize>,
}

impl NodeCache {
    fn new(capacity: Option<usize>) -> Self {
        NodeCache { map: HashMap::new(), tick: 0, capacity }
    }

    fn bump(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    /// 命中则刷新 LRU 时戳并返回克隆;miss 返回 None(调用方回盘读后 `put`)。
    fn get(&mut self, id: &str) -> Option<Node> {
        let t = self.bump();
        let slot = self.map.get_mut(id)?;
        slot.used = t;
        Some(slot.node.clone())
    }

    /// 放入/刷新一个完整节点。新键插入前若超容先淘汰最久未用(LRU)。
    fn put(&mut self, node: Node) {
        let t = self.bump();
        if let Some(cap) = self.capacity {
            if !self.map.contains_key(&node.id) {
                while self.map.len() >= cap {
                    if !self.evict_lru() {
                        break;
                    }
                }
            }
        }
        self.map.insert(node.id.clone(), CacheSlot { node, used: t });
    }

    /// 更新已缓存节点的向量(向量化/重嵌;无 Store 时这是唯一权威副本)。
    fn set_embedding(&mut self, id: &str, embedding: Vec<f32>) {
        let t = self.bump();
        if let Some(slot) = self.map.get_mut(id) {
            slot.node.embedding = embedding;
            slot.used = t;
        }
    }

    fn evict_lru(&mut self) -> bool {
        if let Some(k) = self.map.iter().min_by_key(|(_, s)| s.used).map(|(k, _)| k.clone()) {
            self.map.remove(&k);
            true
        } else {
            false
        }
    }

    /// 当前缓存里所有已向量化节点的 (id, 向量) —— 无 Store 时供第一层索引重建。
    fn vectors(&self) -> Vec<(String, Vec<f32>)> {
        self.map
            .values()
            .filter(|s| !s.node.embedding.is_empty())
            .map(|s| (s.node.id.clone(), s.node.embedding.clone()))
            .collect()
    }
}

// ===================== 时间线 =====================

/// 时间线:有序的轻量元信息 + ID 索引 + 惰性内容(经 Store + 热尾缓存)。
pub struct Timeline {
    metas: Vec<NodeMeta>,
    index: HashMap<String, usize>,
    store: Option<Store>,
    cache: RefCell<NodeCache>,
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Timeline {
    /// 纯内存(无 Store):缓存无界、即权威全量。测试用。
    pub fn new() -> Self {
        Timeline {
            metas: Vec::new(),
            index: HashMap::new(),
            store: None,
            cache: RefCell::new(NodeCache::new(None)),
        }
    }

    /// 带 Store:内容惰性回盘读,缓存有界(热尾工作集)。
    pub fn with_store(store: Store) -> Self {
        Timeline {
            metas: Vec::new(),
            index: HashMap::new(),
            store: Some(store),
            cache: RefCell::new(NodeCache::new(Some(CACHE_CAP))),
        }
    }

    pub fn len(&self) -> usize {
        self.metas.len()
    }
    pub fn is_empty(&self) -> bool {
        self.metas.is_empty()
    }

    /// 追加一个节点(时间线只增不改)。内容入缓存(随即可读),元信息常驻。
    pub fn push(&mut self, node: Node) -> String {
        let id = node.id.clone();
        self.index.insert(id.clone(), self.metas.len());
        self.metas.push(NodeMeta::from_node(&node));
        self.cache.borrow_mut().put(node);
        id
    }

    /// 开机还原一个节点:只建元信息,内容/向量留在盘上(不占 RAM)。
    /// 仅 Store 路径用;调用方负责把向量喂给第一层索引。
    pub fn restore(&mut self, node: &Node) {
        self.index.insert(node.id.clone(), self.metas.len());
        self.metas.push(NodeMeta::from_node(node));
    }

    /// 有序的轻量元信息(创建顺序)。供扫描 / 历史面板等遍历,不触盘。
    pub fn metas(&self) -> &[NodeMeta] {
        &self.metas
    }

    /// 某节点的元信息(不触盘)。
    pub fn meta(&self, id: &str) -> Option<&NodeMeta> {
        self.index.get(id).and_then(|&i| self.metas.get(i))
    }

    /// 某节点在时间线上的序号(0 = 最早)。供二级索引度量"漂离前沿几个窗口"。
    pub fn position(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }

    /// 惰性取完整节点:先缓存,miss 回盘读并入缓存;元字段以 RAM 内 meta 为准(覆盖盘上可能滞后的值)。
    pub fn get(&self, id: &str) -> Option<Node> {
        let &i = self.index.get(id)?;
        let mut node = self.load_base(id)?;
        let m = &self.metas[i];
        node.role = m.role.clone();
        node.created_at = m.created_at;
        node.hits = m.hits;
        node.stain = m.stain;
        node.embedding_version = m.embedding_version.clone();
        Some(node)
    }

    /// 惰性取原文内容。
    pub fn content(&self, id: &str) -> Option<String> {
        self.load_base(id).map(|n| n.content)
    }

    /// 取节点的 content+embedding 原始形态(不叠加 meta)。缓存优先,miss 回盘读并入缓存。
    fn load_base(&self, id: &str) -> Option<Node> {
        if let Some(n) = self.cache.borrow_mut().get(id) {
            return Some(n);
        }
        match &self.store {
            Some(s) => {
                let n = s.load_node(id)?;
                self.cache.borrow_mut().put(n.clone());
                Some(n)
            }
            None => None,
        }
    }

    /// 命中一个节点:hits +1(热度)。
    pub fn touch(&mut self, id: &str) {
        if let Some(&i) = self.index.get(id) {
            self.metas[i].hits += 1;
        }
    }

    /// 给节点写入 embedding 及其向量空间版本(向量化补齐/重嵌)。
    /// 向量落在缓存(无 Store 时是唯一权威);Store 路径由 `Memory::persist_node` 写盘。
    pub fn set_embedding(&mut self, id: &str, embedding: Vec<f32>, version: impl Into<String>) {
        if let Some(&i) = self.index.get(id) {
            self.metas[i].embedding_version = version.into();
            self.metas[i].has_embedding = !embedding.is_empty();
            // 确保该节点在缓存里(Store 路径需先回盘读),再写入向量,使后续 get/持久化可见。
            self.load_base(id);
            self.cache.borrow_mut().set_embedding(id, embedding);
        }
    }

    /// 给一段节点染色。
    pub fn stain(&mut self, id: &str, stain: Stain) {
        if let Some(&i) = self.index.get(id) {
            self.metas[i].stain = stain;
        }
    }

    /// 缓存里所有已向量化节点的 (id, 向量) —— 无 Store 时供第一层索引重建。
    pub(crate) fn cached_vectors(&self) -> Vec<(String, Vec<f32>)> {
        self.cache.borrow().vectors()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_get_roundtrip() {
        let mut tl = Timeline::new();
        let id = tl.push(Node::new("第一条"));
        assert_eq!(tl.len(), 1);
        assert_eq!(tl.get(&id).unwrap().content, "第一条");
        assert_eq!(tl.content(&id).unwrap(), "第一条");
    }

    #[test]
    fn touch_increments_hits() {
        let mut tl = Timeline::new();
        let id = tl.push(Node::new("x"));
        tl.touch(&id);
        tl.touch(&id);
        assert_eq!(tl.meta(&id).unwrap().hits, 2);
        assert_eq!(tl.get(&id).unwrap().hits, 2);
    }

    #[test]
    fn stain_marks_region() {
        let mut tl = Timeline::new();
        let id = tl.push(Node::new("x"));
        assert_eq!(tl.meta(&id).unwrap().stain, Stain::None);
        tl.stain(&id, Stain::Deep);
        assert_eq!(tl.meta(&id).unwrap().stain, Stain::Deep);
        assert_eq!(tl.get(&id).unwrap().stain, Stain::Deep);
    }

    #[test]
    fn metas_in_order_recent_last() {
        let mut tl = Timeline::new();
        tl.push(Node::new("老"));
        tl.push(Node::new("新"));
        let last = tl.metas().last().unwrap();
        assert_eq!(tl.content(&last.id).unwrap(), "新");
    }

    #[test]
    fn set_embedding_visible_via_get_no_store() {
        let mut tl = Timeline::new();
        let id = tl.push(Node::new("v"));
        tl.set_embedding(&id, vec![0.1, 0.2], "ver1");
        assert_eq!(tl.get(&id).unwrap().embedding, vec![0.1, 0.2]);
        assert!(tl.meta(&id).unwrap().has_embedding);
        assert_eq!(tl.meta(&id).unwrap().embedding_version, "ver1");
        assert_eq!(tl.cached_vectors().len(), 1);
    }
}
