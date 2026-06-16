//! 持久化存储 —— 单文件嵌入式数据库(redb)。
//!
//! 取代原先散落的临时 JSON(settings.json / projects.json,以及只存在内存、
//! 重启即丢的 timeline / conclusions)。一个文件 `growbox.redb`:纯 Rust、ACID、
//! 无 C 依赖,随 Tauri 一起分发干净,具备商用级持久化。
//!
//! 三张表:
//! - `nodes`        —— 对话时间线节点(含向量 embedding + role),按 id 存。历史面板直接由此还原。
//! - `conclusions`  —— 经验/知识/理解结论,按 id 存。
//! - `kv`           —— 杂项结构化配置(settings / projects 等),按字符串键存 JSON。
//!
//! 向量说明:当前 embedding 为本地词法向量,单用户规模小,检索走暴力余弦
//! (见 `memory.rs`)。嵌入随节点一起落库;未来若向量规模上量,可在本层加 ANN
//! 索引,对外接口不变。值统一用 serde_json 编码(可读、可调试、无额外依赖)。

use std::path::Path;
use std::sync::{Arc, Mutex};

use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{de::DeserializeOwned, Serialize};

use crate::pointer::Pointer;
use crate::timeline::Node;
use growbox_core::Conclusion;

const NODES: TableDefinition<&str, &[u8]> = TableDefinition::new("nodes");
const CONCLUSIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("conclusions");
const KV: TableDefinition<&str, &[u8]> = TableDefinition::new("kv");
/// 指针网的边,键 `source\0target`,值 = 序列化 Pointer。
/// 磁盘原生:取一个节点的出边 = 按 `source\0` 前缀范围扫(B 树一次局部读),全图永不整体载入。
const EDGES: TableDefinition<&str, &[u8]> = TableDefinition::new("edges");
/// 强制跳转指针(阶段4「历史引用」),键 `source\0target`,值 = 序列化 target。
/// 与 EDGES 同布局(按 source 前缀读一个邻域),但语义不同:**位置键、无 topic 门、遍历到此必跳**——
/// 用户显式指认的历史位置。故单独成表、持久(用户断言的真相,不随热度衰减)。见 `设计/02` 五件套末行。
const JUMPS: TableDefinition<&str, &[u8]> = TableDefinition::new("jumps");

/// 写健康记账:write-through 写失败时累加。`None` last_error = 从未失败。
/// 铁律「严重异常不得静默」:写路径不返回 Result(write-through 散布全 crate),
/// 故失败funnel 进此处,由 GUI 的 health_json 每次轮询读出上浮红警(见 `异常告知.md`)。
#[derive(Default)]
struct WriteHealth {
    fail_count: u64,
    last_error: Option<String>,
}

/// 单文件数据库句柄。克隆共享同一底层 DB + 写健康记账(Arc),可在 Memory 与 AppState 间共用。
#[derive(Clone)]
pub struct Store {
    db: Arc<Database>,
    write_health: Arc<Mutex<WriteHealth>>,
}

impl Store {
    /// 打开(或新建)数据库文件,并确保三张表存在(让后续只读事务不因表缺失而失败)。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let db = Database::create(path).map_err(|e| format!("打开数据库失败: {e}"))?;
        let wtx = db.begin_write().map_err(|e| e.to_string())?;
        {
            wtx.open_table(NODES).map_err(|e| e.to_string())?;
            wtx.open_table(CONCLUSIONS).map_err(|e| e.to_string())?;
            wtx.open_table(KV).map_err(|e| e.to_string())?;
            wtx.open_table(EDGES).map_err(|e| e.to_string())?;
            wtx.open_table(JUMPS).map_err(|e| e.to_string())?;
        }
        wtx.commit().map_err(|e| e.to_string())?;
        Ok(Store {
            db: Arc::new(db),
            write_health: Arc::new(Mutex::new(WriteHealth::default())),
        })
    }

    /// 写健康快照。`None` = 从未写失败;`Some((次数, 最近错误))` = 曾有写失败(数据可能未落盘)。
    /// GUI 每次 health 轮询读它,有失败即上浮 Fatal 红警(严重异常不静默)。
    pub fn write_fault(&self) -> Option<(u64, String)> {
        let h = self.write_health.lock().ok()?;
        if h.fail_count == 0 {
            None
        } else {
            Some((h.fail_count, h.last_error.clone().unwrap_or_default()))
        }
    }

    /// 记一次写失败(累加 + 存最近原因 + stderr)。返回 `()` 便于在错误分支 `return record(...)`。
    fn record_write_fail(&self, ctx: &str, err: impl std::fmt::Display) {
        let msg = format!("{ctx}: {err}");
        eprintln!("[store] 持久化写失败(数据可能未落盘): {msg}");
        if let Ok(mut h) = self.write_health.lock() {
            h.fail_count += 1;
            h.last_error = Some(msg);
        }
    }

    // ---- 节点 ----

    /// 写入/更新一个节点(write-through:摄入、补向量、染色后即时落库)。
    pub fn put_node(&self, node: &Node) {
        self.put_ser(NODES, &node.id, node, "节点");
    }

    /// 载入所有节点(调用方按 created_at 排序还原时间线)。
    /// 注:P3d 后开机用此做"读一遍建元信息 + 收向量喂索引,随即丢弃 content"的瞬时全读,
    /// content 不常驻 RAM;运行时按 id 走 `load_node` 惰性取。
    pub fn load_nodes(&self) -> Vec<Node> {
        self.load_all(NODES)
    }

    /// 点读一个节点(时间线惰性取内容用)。
    pub fn load_node(&self, id: &str) -> Option<Node> {
        let rtx = self.db.begin_read().ok()?;
        let t = rtx.open_table(NODES).ok()?;
        let g = t.get(id).ok()??;
        serde_json::from_slice(g.value()).ok()
    }

    /// 流式读出所有已向量化节点的 (id, 向量) —— 第一层索引重建用(向量不经时间线常驻)。
    pub fn load_node_vectors(&self) -> Vec<(String, Vec<f32>)> {
        let Ok(rtx) = self.db.begin_read() else { return Vec::new() };
        let Ok(t) = rtx.open_table(NODES) else { return Vec::new() };
        let mut out = Vec::new();
        if let Ok(iter) = t.iter() {
            for entry in iter.flatten() {
                if let Ok(n) = serde_json::from_slice::<Node>(entry.1.value()) {
                    if !n.embedding.is_empty() {
                        out.push((n.id, n.embedding));
                    }
                }
            }
        }
        out
    }

    // ---- 结论 ----

    /// 写入/更新一条结论(摄入、supersede 后即时落库)。
    pub fn put_conclusion(&self, c: &Conclusion) {
        self.put_ser(CONCLUSIONS, &c.id, c, "结论");
    }

    pub fn load_conclusions(&self) -> Vec<Conclusion> {
        self.load_all(CONCLUSIONS)
    }

    // ---- 指针边(磁盘原生图)----

    /// 写入/更新一条出边(write-through)。键 `source\0target`,值 = Pointer。
    pub fn put_edge(&self, source: &str, p: &Pointer) {
        self.put_ser(EDGES, &edge_key(source, &p.target), p, "指针边");
    }

    /// 取某节点的出边(一次按 `source\0` 前缀范围读,只碰这一个邻域)。
    pub fn neighbors(&self, source: &str) -> Vec<Pointer> {
        let Ok(rtx) = self.db.begin_read() else { return Vec::new() };
        let Ok(t) = rtx.open_table(EDGES) else { return Vec::new() };
        let lo = format!("{source}\0");
        let hi = format!("{source}\u{1}"); // source 不含 \0,故 [lo, hi) 恰好框住该 source 的全部边
        let mut out = Vec::new();
        if let Ok(iter) = t.range(lo.as_str()..hi.as_str()) {
            for entry in iter.flatten() {
                if let Ok(p) = serde_json::from_slice(entry.1.value()) {
                    out.push(p);
                }
            }
        }
        out
    }

    /// 点读一条边(link/bump 时查现有热度,免整邻域扫)。
    pub fn get_edge(&self, source: &str, target: &str) -> Option<Pointer> {
        let rtx = self.db.begin_read().ok()?;
        let t = rtx.open_table(EDGES).ok()?;
        let g = t.get(edge_key(source, target).as_str()).ok()??;
        serde_json::from_slice(g.value()).ok()
    }

    /// 删一条边(缓存淘汰 / 碎片清理用)。
    pub fn remove_edge(&self, source: &str, target: &str) {
        self.remove_key(EDGES, &edge_key(source, target), "删指针边");
    }

    /// 边总数(观测用;全表 len,不常调)。
    pub fn edge_count(&self) -> usize {
        let Ok(rtx) = self.db.begin_read() else { return 0 };
        let Ok(t) = rtx.open_table(EDGES) else { return 0 };
        t.len().map(|n| n as usize).unwrap_or(0)
    }

    /// 扫描带反 K 的边(供 sleep 复核反 K;限 `limit` 条命中,背景维护可接受 O(边数) 扫描)。
    /// 返回 (source, Pointer);从 `source\0target` 键还原 source。
    pub fn edges_with_negatives(&self, limit: usize) -> Vec<(String, Pointer)> {
        let mut out = Vec::new();
        if limit == 0 {
            return out;
        }
        let Ok(rtx) = self.db.begin_read() else { return out };
        let Ok(t) = rtx.open_table(EDGES) else { return out };
        if let Ok(iter) = t.iter() {
            for entry in iter.flatten() {
                if out.len() >= limit {
                    break;
                }
                if let Ok(p) = serde_json::from_slice::<Pointer>(entry.1.value()) {
                    if !p.negatives.is_empty() {
                        let key = entry.0.value();
                        let source = key.split('\0').next().unwrap_or("").to_string();
                        out.push((source, p));
                    }
                }
            }
        }
        out
    }

    // ---- 强制跳转指针(磁盘原生,位置键)----

    /// 写入一条强制跳转 source→target(write-through)。键 `source\0target`,值 = target。
    /// 幂等:同 (source,target) 覆盖。用户引用历史时建,持久不衰减。
    pub fn put_jump(&self, source: &str, target: &str) {
        self.put_ser(JUMPS, &edge_key(source, target), &target.to_string(), "强制跳转");
    }

    /// 取某位置(source)的全部强制跳转目标(按 `source\0` 前缀读一个邻域)。
    pub fn jumps(&self, source: &str) -> Vec<String> {
        let Ok(rtx) = self.db.begin_read() else { return Vec::new() };
        let Ok(t) = rtx.open_table(JUMPS) else { return Vec::new() };
        let lo = format!("{source}\0");
        let hi = format!("{source}\u{1}");
        let mut out = Vec::new();
        if let Ok(iter) = t.range(lo.as_str()..hi.as_str()) {
            for entry in iter.flatten() {
                if let Ok(tgt) = serde_json::from_slice::<String>(entry.1.value()) {
                    out.push(tgt);
                }
            }
        }
        out
    }

    /// 删一条强制跳转(目标节点失效/用户撤销引用时)。
    pub fn remove_jump(&self, source: &str, target: &str) {
        self.remove_key(JUMPS, &edge_key(source, target), "删强制跳转");
    }

    /// 强制跳转总数(观测用,面板 secondary_indexes)。
    pub fn jump_count(&self) -> usize {
        let Ok(rtx) = self.db.begin_read() else { return 0 };
        let Ok(t) = rtx.open_table(JUMPS) else { return 0 };
        t.len().map(|n| n as usize).unwrap_or(0)
    }

    // ---- 通用 KV(settings / projects 等) ----

    pub fn kv_put<T: Serialize>(&self, key: &str, value: &T) {
        self.put_ser(KV, key, value, &format!("配置[{key}]"));
    }

    /// 删一个 KV 键(旧结构迁移后清理用)。
    pub fn kv_remove(&self, key: &str) {
        self.remove_key(KV, key, &format!("删配置[{key}]"));
    }

    pub fn kv_get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let rtx = self.db.begin_read().ok()?;
        let table = rtx.open_table(KV).ok()?;
        let guard = table.get(key).ok()??;
        serde_json::from_slice(guard.value()).ok()
    }

    // ---- 内部 ----

    /// 序列化 + 落库,任一步失败都记账(不再静默吞)。
    fn put_ser<T: Serialize>(&self, def: TableDefinition<&str, &[u8]>, key: &str, value: &T, ctx: &str) {
        let bytes = match serde_json::to_vec(value) {
            Ok(b) => b,
            Err(e) => return self.record_write_fail(ctx, format!("序列化失败: {e}")),
        };
        self.put_bytes(def, key, &bytes, ctx);
    }

    /// 字节落库:开事务 / 开表 / insert / commit 任一失败都记账。失败不 commit(事务 drop 即 abort)。
    fn put_bytes(&self, def: TableDefinition<&str, &[u8]>, key: &str, bytes: &[u8], ctx: &str) {
        let wtx = match self.db.begin_write() {
            Ok(w) => w,
            Err(e) => return self.record_write_fail(ctx, format!("开启写事务失败: {e}")),
        };
        let mut ok = false;
        {
            match wtx.open_table(def) {
                Ok(mut t) => match t.insert(key, bytes) {
                    Ok(_) => ok = true,
                    Err(e) => self.record_write_fail(ctx, format!("insert 失败: {e}")),
                },
                Err(e) => self.record_write_fail(ctx, format!("打开表失败: {e}")),
            }
        }
        if !ok {
            return; // 写失败:不提交,事务 drop 即回滚
        }
        if let Err(e) = wtx.commit() {
            self.record_write_fail(ctx, format!("提交失败: {e}"));
        }
    }

    /// 删键:开事务 / 开表 / remove / commit 任一失败都记账。
    fn remove_key(&self, def: TableDefinition<&str, &[u8]>, key: &str, ctx: &str) {
        let wtx = match self.db.begin_write() {
            Ok(w) => w,
            Err(e) => return self.record_write_fail(ctx, format!("开启写事务失败: {e}")),
        };
        let mut ok = false;
        {
            match wtx.open_table(def) {
                Ok(mut t) => match t.remove(key) {
                    Ok(_) => ok = true,
                    Err(e) => self.record_write_fail(ctx, format!("remove 失败: {e}")),
                },
                Err(e) => self.record_write_fail(ctx, format!("打开表失败: {e}")),
            }
        }
        if !ok {
            return;
        }
        if let Err(e) = wtx.commit() {
            self.record_write_fail(ctx, format!("提交失败: {e}"));
        }
    }

    fn load_all<T: DeserializeOwned>(&self, def: TableDefinition<&str, &[u8]>) -> Vec<T> {
        let Ok(rtx) = self.db.begin_read() else { return Vec::new() };
        let Ok(t) = rtx.open_table(def) else { return Vec::new() };
        let mut out = Vec::new();
        if let Ok(iter) = t.iter() {
            for entry in iter.flatten() {
                if let Ok(v) = serde_json::from_slice(entry.1.value()) {
                    out.push(v);
                }
            }
        }
        out
    }
}

/// 边键:`source\0target`。source/target 是 sha 派生的 node id,不含 `\0`,故分隔安全。
fn edge_key(source: &str, target: &str) -> String {
    format!("{source}\0{target}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pointer::Pointer;
    use crate::timeline::Node;
    use growbox_core::Conclusion;
    use tempfile::tempdir;

    /// 测试辅助:建一条指定 heat 的边(学习型 Pointer,单个正 K)。
    fn ptr(target: &str, topic: Vec<f32>, heat: u32) -> Pointer {
        let mut p = Pointer::from_topic(target, topic, 0);
        p.heat = heat;
        p
    }

    #[test]
    fn edges_prefix_scan_isolates_one_neighborhood() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path().join("t.redb")).unwrap();
        // a 有两条出边,b 有一条;按 source 前缀扫只取该 source 的邻域。
        s.put_edge("a", &ptr("x", vec![1.0], 1));
        s.put_edge("a", &ptr("y", vec![0.0], 2));
        s.put_edge("b", &ptr("z", vec![1.0], 1));

        let na = s.neighbors("a");
        assert_eq!(na.len(), 2, "只取 a 的邻域");
        assert!(na.iter().any(|p| p.target == "x" && p.heat == 1));
        assert!(na.iter().any(|p| p.target == "y" && p.heat == 2));
        assert_eq!(s.neighbors("b").len(), 1);
        assert!(s.neighbors("missing").is_empty());
        assert_eq!(s.edge_count(), 3);
    }

    #[test]
    fn put_edge_upserts_and_remove_deletes() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path().join("t.redb")).unwrap();
        s.put_edge("a", &ptr("x", vec![1.0], 1));
        s.put_edge("a", &ptr("x", vec![1.0], 5)); // 同键覆盖
        assert_eq!(s.neighbors("a").len(), 1);
        assert_eq!(s.neighbors("a")[0].heat, 5);
        s.remove_edge("a", "x");
        assert!(s.neighbors("a").is_empty());
    }

    #[test]
    fn edges_survive_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.redb");
        {
            let s = Store::open(&path).unwrap();
            s.put_edge("a", &ptr("x", vec![0.5], 3));
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.neighbors("a")[0].heat, 3);
    }

    #[test]
    fn nodes_and_conclusions_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.redb");
        {
            let s = Store::open(&path).unwrap();
            let mut n = Node::new("持久化的对话");
            n.embedding = vec![0.1, 0.2, 0.3];
            s.put_node(&n);
            s.put_conclusion(&Conclusion::experience("op", "exp", "src"));
        }
        // 重新打开:数据还在。
        let s = Store::open(&path).unwrap();
        let nodes = s.load_nodes();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].content, "持久化的对话");
        assert_eq!(nodes[0].embedding, vec![0.1, 0.2, 0.3]);
        assert_eq!(s.load_conclusions().len(), 1);
    }

    #[test]
    fn put_node_is_upsert_by_id() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path().join("t.redb")).unwrap();
        let mut n = Node::new("x");
        s.put_node(&n);
        n.hits = 5;
        s.put_node(&n); // 同 id 覆盖,不新增
        let nodes = s.load_nodes();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].hits, 5);
    }

    #[test]
    fn jumps_prefix_scan_and_persist() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.redb");
        {
            let s = Store::open(&path).unwrap();
            s.put_jump("here", "old1");
            s.put_jump("here", "old2");
            s.put_jump("there", "x");
            s.put_jump("here", "old1"); // 幂等覆盖
            let mut hs = s.jumps("here");
            hs.sort();
            assert_eq!(hs, vec!["old1", "old2"], "只取 here 的跳转,去重");
            assert_eq!(s.jumps("there"), vec!["x"]);
            assert!(s.jumps("missing").is_empty());
            assert_eq!(s.jump_count(), 3);
            s.remove_jump("here", "old1");
            assert_eq!(s.jumps("here"), vec!["old2"]);
        }
        // 持久:用户断言的历史引用重启后仍在。
        let s = Store::open(&path).unwrap();
        assert_eq!(s.jumps("here"), vec!["old2"]);
        assert_eq!(s.jump_count(), 2);
    }

    #[test]
    fn kv_roundtrip() {
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path().join("t.redb")).unwrap();
        assert!(s.kv_get::<Vec<String>>("missing").is_none());
        s.kv_put("list", &vec!["a".to_string(), "b".to_string()]);
        assert_eq!(s.kv_get::<Vec<String>>("list").unwrap(), vec!["a", "b"]);
    }

    #[test]
    fn successful_writes_report_no_write_fault() {
        // 正常写入不得误报写失败(否则 health 会假红警)。
        let dir = tempdir().unwrap();
        let s = Store::open(dir.path().join("t.redb")).unwrap();
        s.put_node(&Node::new("x"));
        s.put_conclusion(&Conclusion::experience("op", "exp", "src"));
        s.put_edge("a", &ptr("b", vec![1.0], 1));
        s.kv_put("k", &"v".to_string());
        s.remove_edge("a", "b");
        s.kv_remove("k");
        assert!(s.write_fault().is_none(), "成功路径不应记写失败");
    }
}
