//! 提示词转译版本库("历史提示词")—— 让「用当前模型重写」可后悔。
//!
//! 每次重写 = 存一个**新版本**(覆盖表 gzip+base64 压缩成串),绝不覆盖旧版;UI 可列历史、加载任一版、改名、删除。
//! 「默认(原文)」是隐式版本(id=`default`,空覆盖=用原文),**永不可删、是兜底**。整库一条 redb 记录(原子)。
//!
//! 设计:核心逻辑是 [`Library`] 上的**纯方法**(可脱库单测);[`Store`] 只是 load→改→save 的薄包装。

use std::collections::HashMap;
use std::io::{Read, Write};

use base64::Engine;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use growbox_memory::Store;
use serde::{Deserialize, Serialize};

/// 整个版本库在 redb 的 kv 键。
const LIBRARY_KEY: &str = "transpile_library";
/// v1 单覆盖表键(`transpile.rs::OVERRIDES_KEY`),首次载入时迁移成一个版本。
const LEGACY_OVERRIDES_KEY: &str = crate::transpile::OVERRIDES_KEY;
/// 「默认(原文)」保留 id:激活它 = 空覆盖 = 用原文。
pub const DEFAULT_ID: &str = "default";

/// 一个版本的元信息(给 UI 列表)。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SnapshotMeta {
    pub id: String,
    /// 默认 `<模型名>-<序号>`,可自由重命名。
    pub name: String,
    pub model: String,
    /// 覆盖条数(列表显示;不必解压 blob 即可知)。
    pub count: usize,
    pub created_ms: i64,
}

#[derive(Serialize, Deserialize, Clone)]
struct StoredSnapshot {
    meta: SnapshotMeta,
    /// base64(gzip(json(HashMap<okey, 转译文本>)))。
    blob: String,
}

/// 版本库(整体一条 kv 记录)。`active` 为空或 `default` = 用原文。
#[derive(Serialize, Deserialize, Default)]
pub struct Library {
    snapshots: Vec<StoredSnapshot>,
    active: String,
    /// 单调序号,只增不减,用于默认命名(改名/删版都不回收,避免重名)。
    seq: u64,
}

/// 覆盖表 → 压缩串(gzip + base64)。
fn pack(map: &HashMap<String, String>) -> String {
    let json = serde_json::to_vec(map).unwrap_or_default();
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    let _ = enc.write_all(&json);
    let gz = enc.finish().unwrap_or_default();
    base64::engine::general_purpose::STANDARD.encode(gz)
}

/// 压缩串 → 覆盖表;任一步失败返回空表(安全:回落原文,不污染其它版本)。
fn unpack(blob: &str) -> HashMap<String, String> {
    let Ok(gz) = base64::engine::general_purpose::STANDARD.decode(blob) else {
        return HashMap::new();
    };
    let mut dec = GzDecoder::new(&gz[..]);
    let mut json = Vec::new();
    if dec.read_to_end(&mut json).is_err() {
        return HashMap::new();
    }
    serde_json::from_slice(&json).unwrap_or_default()
}

impl Library {
    fn find(&self, id: &str) -> Option<&StoredSnapshot> {
        self.snapshots.iter().find(|s| s.meta.id == id)
    }

    /// 当前激活版本的覆盖表(default/未知/空 → 空表 = 原文)。
    pub fn active_overrides(&self) -> HashMap<String, String> {
        if self.active.is_empty() || self.active == DEFAULT_ID {
            return HashMap::new();
        }
        self.find(&self.active).map(|s| unpack(&s.blob)).unwrap_or_default()
    }

    /// 列出所有真实版本(新→旧;不含隐式的 default,由 UI 自行置顶展示)。返回 `(metas, 当前激活 id)`。
    pub fn list(&self) -> (Vec<SnapshotMeta>, String) {
        let active = if self.active.is_empty() { DEFAULT_ID.to_string() } else { self.active.clone() };
        let metas = self.snapshots.iter().rev().map(|s| s.meta.clone()).collect();
        (metas, active)
    }

    /// 加一个新版本并置为激活。`created_ms` 由调用方给(便于测试确定性)。
    pub fn add_version(&mut self, model: &str, map: &HashMap<String, String>, created_ms: i64) -> SnapshotMeta {
        self.seq += 1;
        let id = format!("v{}", self.seq);
        let meta = SnapshotMeta {
            id: id.clone(),
            name: format!("{model}-{}", self.seq),
            model: model.to_string(),
            count: map.len(),
            created_ms,
        };
        self.snapshots.push(StoredSnapshot { meta: meta.clone(), blob: pack(map) });
        self.active = id;
        meta
    }

    /// 激活某版本(default → 原文)。返回新激活版的覆盖表。未知 id 不改动。
    pub fn activate(&mut self, id: &str) -> HashMap<String, String> {
        if id == DEFAULT_ID {
            self.active = DEFAULT_ID.to_string();
            return HashMap::new();
        }
        if self.find(id).is_some() {
            self.active = id.to_string();
        }
        self.active_overrides()
    }

    /// 重命名一版(default 不可改;名空则忽略)。
    pub fn rename(&mut self, id: &str, name: &str) {
        let name = name.trim();
        if id == DEFAULT_ID || name.is_empty() {
            return;
        }
        if let Some(s) = self.snapshots.iter_mut().find(|s| s.meta.id == id) {
            s.meta.name = name.to_string();
        }
    }

    /// 删一版(default 拒删)。删的是激活版 → 激活回落 default。返回删后激活版的覆盖表。
    pub fn delete(&mut self, id: &str) -> HashMap<String, String> {
        if id == DEFAULT_ID {
            return self.active_overrides();
        }
        self.snapshots.retain(|s| s.meta.id != id);
        if self.active == id {
            self.active = DEFAULT_ID.to_string();
        }
        self.active_overrides()
    }
}

// ---- Store 薄包装:load → 改 → save ----

/// 载入版本库;无则建新(顺带迁移 v1 单覆盖表成一个版本)。
fn load(store: &Store) -> Library {
    if let Some(lib) = store.kv_get::<Library>(LIBRARY_KEY) {
        return lib;
    }
    let mut lib = Library { active: DEFAULT_ID.to_string(), ..Library::default() };
    // 迁移 v1:把旧的单覆盖表收进一个版本(model 未知留空)。
    if let Some(old) = store.kv_get::<HashMap<String, String>>(LEGACY_OVERRIDES_KEY) {
        if !old.is_empty() {
            lib.add_version("", &old, 0);
        }
    }
    lib
}

/// 当前激活版本的覆盖表(connect 时推入全局取用层)。
pub fn active_overrides(store: &Store) -> HashMap<String, String> {
    load(store).active_overrides()
}

/// 列出版本(新→旧)+ 当前激活 id。
pub fn list(store: &Store) -> (Vec<SnapshotMeta>, String) {
    load(store).list()
}

/// 加新版本(置为激活),持久化,返回其 meta。
pub fn add_version(store: &Store, model: &str, map: &HashMap<String, String>) -> SnapshotMeta {
    let mut lib = load(store);
    let meta = lib.add_version(model, map, growbox_core::now().timestamp_millis());
    store.kv_put(LIBRARY_KEY, &lib);
    meta
}

/// 激活某版本,持久化,返回其覆盖表。
pub fn activate(store: &Store, id: &str) -> HashMap<String, String> {
    let mut lib = load(store);
    let map = lib.activate(id);
    store.kv_put(LIBRARY_KEY, &lib);
    map
}

/// 重命名一版,持久化。
pub fn rename(store: &Store, id: &str, name: &str) {
    let mut lib = load(store);
    lib.rename(id, name);
    store.kv_put(LIBRARY_KEY, &lib);
}

/// 删一版(default 拒删),持久化,返回删后激活版覆盖表。
pub fn delete(store: &Store, id: &str) -> HashMap<String, String> {
    let mut lib = load(store);
    let map = lib.delete(id);
    store.kv_put(LIBRARY_KEY, &lib);
    map
}

// ---- 导出 / 导入磁盘 .zip 文件("历史提示词"备份) ----

/// 纯函数:把(名/模型/覆盖表)打包成 .zip 字节(manifest.json + overrides.json,明文 JSON)。可单测。
fn zip_bytes(name: &str, model: &str, map: &HashMap<String, String>) -> Option<Vec<u8>> {
    let manifest = serde_json::json!({
        "kind": "growbox-transpile-snapshot",
        "version": 1,
        "name": name,
        "model": model,
        "count": map.len(),
    });
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("manifest.json", opts).ok()?;
        zip.write_all(serde_json::to_string_pretty(&manifest).ok()?.as_bytes()).ok()?;
        zip.start_file("overrides.json", opts).ok()?;
        zip.write_all(serde_json::to_vec_pretty(map).ok()?.as_slice()).ok()?;
        zip.finish().ok()?;
    }
    Some(cursor.into_inner())
}

/// 纯函数:从 .zip 字节读出 (name, model, 覆盖表)。缺 overrides.json/损坏 → None。可单测。
fn unzip(bytes: &[u8]) -> Option<(String, String, HashMap<String, String>)> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    let map: HashMap<String, String> = {
        let mut f = archive.by_name("overrides.json").ok()?;
        let mut s = String::new();
        f.read_to_string(&mut s).ok()?;
        serde_json::from_str(&s).ok()?
    };
    let (name, model) = archive
        .by_name("manifest.json")
        .ok()
        .and_then(|mut f| {
            let mut s = String::new();
            f.read_to_string(&mut s).ok()?;
            let v: serde_json::Value = serde_json::from_str(&s).ok()?;
            let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let model = v.get("model").and_then(|x| x.as_str()).unwrap_or("").to_string();
            Some((name, model))
        })
        .unwrap_or_default();
    Some((name, model, map))
}

/// 把一个版本(default → 空覆盖)打包成 .zip 字节。未知 id → None。
pub fn export_zip(store: &Store, id: &str) -> Option<Vec<u8>> {
    let lib = load(store);
    let (name, model, map) = if id == DEFAULT_ID {
        ("默认(原文)".to_string(), String::new(), HashMap::new())
    } else {
        let s = lib.find(id)?;
        (s.meta.name.clone(), s.meta.model.clone(), unpack(&s.blob))
    };
    zip_bytes(&name, &model, &map)
}

/// 从 .zip 字节导入成一个新版本(置激活)。`fallback_name` 在 manifest 缺名时用。
/// 返回新版本 meta;.zip 损坏/缺 overrides.json → None。
pub fn import_zip(store: &Store, bytes: &[u8], fallback_name: &str) -> Option<SnapshotMeta> {
    let (name, model, map) = unzip(bytes)?;
    let mut lib = load(store);
    let meta = lib.add_version(&model, &map, growbox_core::now().timestamp_millis());
    // 导入版用 manifest/文件名命名(否则 add_version 的 <model>-<seq> 默认名)。
    let chosen = if !name.trim().is_empty() {
        format!("{}(导入)", name.trim())
    } else if !fallback_name.trim().is_empty() {
        fallback_name.trim().to_string()
    } else {
        meta.name.clone()
    };
    lib.rename(&meta.id, &chosen);
    let meta = lib.find(&meta.id).map(|s| s.meta.clone()).unwrap_or(meta);
    store.kv_put(LIBRARY_KEY, &lib);
    Some(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn pack_unpack_roundtrip() {
        let map = m(&[("deepseek\u{1f}zh\u{1f}agent.system", "你是 GrowBox"), ("k2", "v2")]);
        let packed = pack(&map);
        assert!(!packed.is_empty());
        assert_eq!(unpack(&packed), map);
        // 坏串安全回落空表(不 panic)。
        assert!(unpack("not-valid-base64!!!").is_empty());
    }

    #[test]
    fn add_version_sets_active_and_serial_names() {
        let mut lib = Library::default();
        let a = lib.add_version("deepseek-v4-flash", &m(&[("k", "v1")]), 100);
        assert_eq!(a.name, "deepseek-v4-flash-1");
        assert_eq!(a.count, 1);
        let b = lib.add_version("deepseek-v4-flash", &m(&[("k", "v2"), ("k2", "x")]), 200);
        assert_eq!(b.name, "deepseek-v4-flash-2");
        // 激活最新版
        assert_eq!(lib.active_overrides(), m(&[("k", "v2"), ("k2", "x")]));
        // 列表新→旧
        let (metas, active) = lib.list();
        assert_eq!(metas.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), vec!["v2", "v1"]);
        assert_eq!(active, "v2");
    }

    #[test]
    fn activate_and_revert_to_default() {
        let mut lib = Library::default();
        lib.add_version("m", &m(&[("k", "old")]), 1);
        lib.add_version("m", &m(&[("k", "new")]), 2);
        // 后悔:激活回 v1
        assert_eq!(lib.activate("v1"), m(&[("k", "old")]));
        assert_eq!(lib.active_overrides(), m(&[("k", "old")]));
        // 激活 default = 原文(空)
        assert!(lib.activate(DEFAULT_ID).is_empty());
        assert!(lib.active_overrides().is_empty());
        let (_, active) = lib.list();
        assert_eq!(active, DEFAULT_ID);
    }

    #[test]
    fn rename_skips_default_and_empty() {
        let mut lib = Library::default();
        let a = lib.add_version("m", &m(&[("k", "v")]), 1);
        lib.rename(&a.id, "我的提示词");
        assert_eq!(lib.list().0[0].name, "我的提示词");
        lib.rename(&a.id, "   "); // 空名忽略
        assert_eq!(lib.list().0[0].name, "我的提示词");
        lib.rename(DEFAULT_ID, "x"); // default 不可改(无 panic、无效果)
    }

    #[test]
    fn delete_protects_default_and_falls_back() {
        let mut lib = Library::default();
        lib.add_version("m", &m(&[("k", "a")]), 1); // v1
        lib.add_version("m", &m(&[("k", "b")]), 2); // v2 active
        // 删非激活版 v1:不影响激活
        lib.delete("v1");
        assert_eq!(lib.active_overrides(), m(&[("k", "b")]));
        // 删激活版 v2:回落 default(原文)
        let after = lib.delete("v2");
        assert!(after.is_empty());
        assert_eq!(lib.list().1, DEFAULT_ID);
        // default 拒删(无 panic)
        lib.delete(DEFAULT_ID);
    }

    #[test]
    fn zip_roundtrip_preserves_overrides() {
        let map = m(&[("deepseek\u{1f}zh\u{1f}agent.system", "你是 GrowBox"), ("k2", "v2")]);
        let bytes = zip_bytes("我的提示词", "deepseek-v4-flash", &map).expect("打包");
        // 确实是 zip(PK 魔数)
        assert_eq!(&bytes[0..2], b"PK");
        let (name, model, back) = unzip(&bytes).expect("解包");
        assert_eq!(name, "我的提示词");
        assert_eq!(model, "deepseek-v4-flash");
        assert_eq!(back, map);
        // 坏字节安全 None(不 panic)
        assert!(unzip(b"not a zip").is_none());
    }

    #[test]
    fn seq_never_reused_after_delete() {
        let mut lib = Library::default();
        lib.add_version("m", &m(&[]), 1); // v1
        lib.delete("v1");
        let b = lib.add_version("m", &m(&[]), 2);
        assert_eq!(b.id, "v2", "序号只增不回收,避免与已删版重名");
    }
}
