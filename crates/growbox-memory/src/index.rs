//! 第一层 RAG 向量索引 —— 可切换引擎的 ANN 接口。
//!
//! `VectorIndex` 是迁移 seam(见 `设计文档/跨平台迁移方案.md`):换引擎不动调用方。
//! - `ArroyIndex`(arroy + heed/LMDB,**磁盘原生 mmap**,向量不全驻进程 RAM)= 运行时默认引擎(有 Store 时)。
//!   选 arroy 而非 hannoy:同为 Meilisearch 团队、同 LMDB 家族、同样磁盘原生,但 arroy 更成熟
//!   (0.6.x,39 万下载,多年生产),版本干净;hannoy(0.1.x,更快)留作后续 drop-in 升级。见 `设计文档/跨平台迁移方案.md`。
//! - `HnswIndex`(instant-distance,纯 Rust HNSW,O(log N))= 无 Store(测试/纯内存)时的引擎,向量驻 RAM。
//! - `BruteForceIndex`(无依赖,O(N))= 参考/测试/兜底引擎。
//!
//! 向量约定:入索引的 embedding 已 L2 归一化(e5 与词法版都归一化),故 cosine = 点积,
//! 距离 = 1 - cosine。`search` 对外统一返回 (id, cosine 相似度) 降序。

use instant_distance::{Builder, HnswMap, Point, Search};

/// 第一层向量索引引擎。全量重建 + 近邻查询。
pub trait VectorIndex: Send {
    /// 用 (id, 向量) 全量重建(向量空间变 / 批量补向量后调)。
    fn rebuild(&mut self, items: Vec<(String, Vec<f32>)>);
    /// 查最近 k 个,返回 (id, cosine 相似度),按相似度降序。
    fn search(&self, query: &[f32], k: usize) -> Vec<(String, f32)>;
    /// 当前索引条目数。
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let mut s = 0.0;
    for i in 0..n {
        s += a[i] * b[i];
    }
    s
}

// ===================== HNSW(instant-distance,当前引擎) =====================

/// instant-distance 的点:归一化向量,距离 = 1 - cosine。
#[derive(Clone)]
struct Emb(Vec<f32>);

impl Point for Emb {
    fn distance(&self, other: &Self) -> f32 {
        1.0 - dot(&self.0, &other.0)
    }
}

/// 纯 Rust HNSW 索引(向量驻 RAM)。
#[derive(Default)]
pub struct HnswIndex {
    map: Option<HnswMap<Emb, String>>,
    len: usize,
}

impl HnswIndex {
    pub fn new() -> Self {
        Self::default()
    }
}

impl VectorIndex for HnswIndex {
    fn rebuild(&mut self, items: Vec<(String, Vec<f32>)>) {
        if items.is_empty() {
            self.map = None;
            self.len = 0;
            return;
        }
        let mut ids = Vec::with_capacity(items.len());
        let mut pts = Vec::with_capacity(items.len());
        for (id, v) in items {
            ids.push(id);
            pts.push(Emb(v));
        }
        self.len = ids.len();
        self.map = Some(Builder::default().build(pts, ids));
    }

    fn search(&self, query: &[f32], k: usize) -> Vec<(String, f32)> {
        let Some(map) = &self.map else { return Vec::new() };
        let mut search = Search::default();
        let q = Emb(query.to_vec());
        map.search(&q, &mut search)
            .take(k)
            .map(|item| (item.value.clone(), 1.0 - item.distance))
            .collect()
    }

    fn len(&self) -> usize {
        self.len
    }
}

// ===================== 暴力(无依赖,测试/兜底引擎) =====================

/// 线性扫描索引:确定、易测、无依赖。规模大时换 HNSW/arroy。
#[derive(Default)]
pub struct BruteForceIndex {
    items: Vec<(String, Vec<f32>)>,
}

impl BruteForceIndex {
    pub fn new() -> Self {
        Self::default()
    }
}

impl VectorIndex for BruteForceIndex {
    fn rebuild(&mut self, items: Vec<(String, Vec<f32>)>) {
        self.items = items;
    }

    fn search(&self, query: &[f32], k: usize) -> Vec<(String, f32)> {
        let mut scored: Vec<(String, f32)> =
            self.items.iter().map(|(id, v)| (id.clone(), dot(query, v))).collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored
    }

    fn len(&self) -> usize {
        self.items.len()
    }
}

// ===================== arroy(LMDB,磁盘原生,运行时默认) =====================

use std::num::NonZeroUsize;
use std::path::Path;

use arroy::distances::Cosine;
use arroy::{Database as ArroyDb, Reader, Writer};
use heed::{Env, EnvOpenOptions};
use rand::rngs::StdRng;
use rand::SeedableRng;

/// arroy 索引号(本库只用一个)。
const ARROY_INDEX: u16 = 0;
/// LMDB mmap 上限(虚拟地址空间,稀疏占用;单用户桌面 1 GiB 足够,约可装 ~70 万条 384 维)。
const ARROY_MAP_SIZE: usize = 1024 * 1024 * 1024;

/// 磁盘原生 ANN(arroy + LMDB)。向量落 LMDB,查询走 mmap,不把全部向量驻进程 RAM。
/// 满足"无界记忆不全驻内存"thesis 的向量这一半(content 那一半已由 P3d 惰性化)。
pub struct ArroyIndex {
    env: Env,
    db: ArroyDb<Cosine>,
    /// arroy 的 ItemId(u32)→ 我们的节点 id 字符串(只存 id,极轻)。
    ids: Vec<String>,
    dims: usize,
    len: usize,
}

impl ArroyIndex {
    /// 在 `dir` 下开/建一个 LMDB env(LMDB 用目录而非单文件)。
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, String> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).map_err(|e| format!("建向量索引目录失败: {e}"))?;
        let env = unsafe { EnvOpenOptions::new().map_size(ARROY_MAP_SIZE).open(dir) }
            .map_err(|e| format!("打开向量索引 LMDB 失败: {e}"))?;
        let mut wtxn = env.write_txn().map_err(|e| e.to_string())?;
        let db: ArroyDb<Cosine> = env.create_database(&mut wtxn, None).map_err(|e| e.to_string())?;
        wtxn.commit().map_err(|e| e.to_string())?;
        Ok(ArroyIndex { env, db, ids: Vec::new(), dims: 0, len: 0 })
    }
}

impl VectorIndex for ArroyIndex {
    fn rebuild(&mut self, items: Vec<(String, Vec<f32>)>) {
        let Ok(mut wtxn) = self.env.write_txn() else { return };
        let dims = items.first().map(|(_, v)| v.len()).unwrap_or(0);
        // 空集:清空并提交即可(维度未知,不建)。
        if items.is_empty() || dims == 0 {
            let writer = Writer::<Cosine>::new(self.db, ARROY_INDEX, 1);
            let _ = writer.clear(&mut wtxn);
            if wtxn.commit().is_ok() {
                self.ids.clear();
                self.dims = 0;
                self.len = 0;
            }
            return;
        }
        let writer = Writer::<Cosine>::new(self.db, ARROY_INDEX, dims);
        if writer.clear(&mut wtxn).is_err() {
            return;
        }
        let mut ids = Vec::with_capacity(items.len());
        for (id, vec) in &items {
            if vec.len() != dims {
                continue; // 同一 embedder 维度应一致;异常项跳过保持 item_id 与 ids 对齐
            }
            let item_id = ids.len() as u32; // item_id == ids 中的下标
            if writer.add_item(&mut wtxn, item_id, vec).is_err() {
                return;
            }
            ids.push(id.clone());
        }
        let mut rng = StdRng::seed_from_u64(42);
        if writer.builder(&mut rng).build(&mut wtxn).is_err() {
            return;
        }
        if wtxn.commit().is_err() {
            return;
        }
        self.ids = ids;
        self.dims = dims;
        self.len = self.ids.len();
    }

    fn search(&self, query: &[f32], k: usize) -> Vec<(String, f32)> {
        if self.len == 0 || query.len() != self.dims || k == 0 {
            return Vec::new();
        }
        let Ok(rtxn) = self.env.read_txn() else { return Vec::new() };
        let Ok(reader) = Reader::<Cosine>::open(&rtxn, ARROY_INDEX, self.db) else {
            return Vec::new();
        };
        // 单用户规模优先召回:把 search_k 放大,逼近精确(arroy 默认偏快、可能漏)。
        let mut qb = reader.nns(k);
        let sk = k.max(10).saturating_mul(reader.n_trees().max(1)).saturating_mul(15);
        if let Some(nz) = NonZeroUsize::new(sk) {
            qb.search_k(nz);
        }
        let Ok(hits) = qb.by_vector(&rtxn, query) else { return Vec::new() };
        // arroy Cosine 返回距离 d = (1 - cos)/2 → 还原 cosine 相似度 cos = 1 - 2d(与其余各层同一标度)。
        hits.into_iter()
            .filter_map(|(item, dist)| self.ids.get(item as usize).map(|id| (id.clone(), 1.0 - 2.0 * dist)))
            .collect()
    }

    fn len(&self) -> usize {
        self.len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(v: Vec<f32>) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n > 0.0 {
            v.iter().map(|x| x / n).collect()
        } else {
            v
        }
    }

    fn check_engine(mut idx: impl VectorIndex) {
        idx.rebuild(vec![
            ("a".into(), norm(vec![1.0, 0.0])),
            ("b".into(), norm(vec![0.0, 1.0])),
            ("c".into(), norm(vec![0.9, 0.1])),
        ]);
        assert_eq!(idx.len(), 3);
        let hits = idx.search(&norm(vec![1.0, 0.0]), 2);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "a", "最近邻应是 a");
        assert!(hits[0].1 >= hits[1].1, "结果按相似度降序");
        assert!(hits[0].1 > 0.99, "a 与查询同向,cosine≈1");
    }

    #[test]
    fn brute_force_finds_nearest() {
        check_engine(BruteForceIndex::new());
    }

    #[test]
    fn hnsw_finds_nearest() {
        check_engine(HnswIndex::new());
    }

    #[test]
    fn arroy_finds_nearest_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        check_engine(ArroyIndex::open(dir.path()).unwrap());
    }

    #[test]
    fn arroy_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut idx = ArroyIndex::open(dir.path()).unwrap();
            idx.rebuild(vec![("a".into(), norm(vec![1.0, 0.0])), ("b".into(), norm(vec![0.0, 1.0]))]);
            assert_eq!(idx.len(), 2);
        }
        // 重开同目录:LMDB 数据还在盘上,但 ids 映射在内存(需重建恢复)。
        // 这里验证重开后 rebuild 能在已有 env 上正常清+建。
        let mut idx = ArroyIndex::open(dir.path()).unwrap();
        idx.rebuild(vec![("a".into(), norm(vec![1.0, 0.0]))]);
        let hits = idx.search(&norm(vec![1.0, 0.0]), 1);
        assert_eq!(hits[0].0, "a");
    }

    #[test]
    fn empty_index_returns_nothing() {
        let idx = HnswIndex::new();
        assert!(idx.is_empty());
        assert!(idx.search(&[1.0, 0.0], 5).is_empty());
    }
}
