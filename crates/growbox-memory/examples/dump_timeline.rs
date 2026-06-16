//! 只读时间线诊断:列出所有表+条数,统计 nodes 角色分布 + 打印最近 N 条。
//! 用法:cargo run -p growbox-memory --example dump_timeline -- <redb 路径> [最近条数]

use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition, TableHandle};
use std::collections::BTreeMap;

const NODES: TableDefinition<&str, &[u8]> = TableDefinition::new("nodes");

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_timeline <redb path> [N]");
    let n_recent: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let db = Database::open(&path).expect("open redb");
    let rtx = db.begin_read().expect("read tx");

    println!("=== 所有表 ===");
    if let Ok(tables) = rtx.list_tables() {
        for th in tables {
            let name = th.name().to_string();
            if let Ok(t) = rtx.open_table::<&str, &[u8]>(TableDefinition::new(&name)) {
                println!("  表 {name:<16} 条数={}", t.len().unwrap_or(0));
            }
        }
    }

    let mut roles: BTreeMap<String, u32> = BTreeMap::new();
    let mut all: Vec<(String, String, String)> = Vec::new(); // (created_at, role, content)
    let mut embedded = 0u32;
    if let Ok(t) = rtx.open_table(NODES) {
        for item in t.iter().expect("iter nodes") {
            let (_k, v) = item.expect("node entry");
            let Ok(node) = serde_json::from_slice::<serde_json::Value>(v.value()) else { continue };
            let role = node.get("role").and_then(|r| r.as_str()).unwrap_or("?").to_string();
            let content = node.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
            let created = node.get("created_at").map(|c| c.to_string()).unwrap_or_default();
            let emb_len = node.get("embedding").and_then(|e| e.as_array()).map(|a| a.len()).unwrap_or(0);
            if emb_len > 0 { embedded += 1; }
            *roles.entry(role.clone()).or_default() += 1;
            all.push((created, role, content));
        }
    }
    println!("\n=== nodes 节点总数 {} (已嵌入 {embedded}) ===", all.len());
    for (r, c) in &roles {
        println!("  role={r:<14} {c}");
    }
    all.sort_by(|a, b| a.0.cmp(&b.0));
    // 模拟 8000 字符 ring 尾部:从最新往旧累加到 8000 字符,看实际进 ring 的条数/角色构成。
    {
        let mut used = 0usize;
        let mut ring: Vec<&(String, String, String)> = Vec::new();
        for n in all.iter().rev() {
            let len = n.2.len();
            if used + len > 8000 && !ring.is_empty() { break; }
            used += len;
            ring.push(n);
        }
        let mut ring_roles: BTreeMap<String, u32> = BTreeMap::new();
        for n in &ring { *ring_roles.entry(n.1.clone()).or_default() += 1; }
        println!("\n=== 8000字符 ring 尾部实际构成:{} 条,{used} 字符 ===", ring.len());
        for (r, c) in &ring_roles { println!("  ring role={r:<12} {c}"); }
    }
    println!("\n=== 最近 {n_recent} 条(时间升序)===");
    let start = all.len().saturating_sub(n_recent);
    for (ts, role, content) in &all[start..] {
        let preview: String = content.chars().take(150).collect();
        let preview = preview.replace('\n', " ⏎ ");
        println!("[{ts}] {role:<10} {preview}");
    }
}
