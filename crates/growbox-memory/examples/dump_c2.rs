//! C2 诊断探针(只读):dump 出 redb 里的 process 节点(看 `wf:` 标记)+ 工作流桶(看是否注册)。
//! 用法:cargo run -p growbox-memory --example dump_c2 -- <redb 路径>

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};

const NODES: TableDefinition<&str, &[u8]> = TableDefinition::new("nodes");
const KV: TableDefinition<&str, &[u8]> = TableDefinition::new("kv");

#[derive(serde::Deserialize)]
struct NodeLite {
    #[serde(default)]
    id: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: String,
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_c2 <redb path>");
    let db = Database::open(&path).expect("open redb");
    let rtx = db.begin_read().expect("read tx");

    let (mut total, mut procs) = (0u32, 0u32);
    if let Ok(t) = rtx.open_table(NODES) {
        for item in t.iter().expect("iter nodes") {
            let (_k, v) = item.expect("node entry");
            total += 1;
            if let Ok(n) = serde_json::from_slice::<NodeLite>(v.value()) {
                if n.role == "process" {
                    procs += 1;
                    let has_wf = n.content.lines().any(|l| l.trim().starts_with("wf:"));
                    println!("PROCESS [{}] wf标记={}", n.id, if has_wf { "有" } else { "无" });
                    println!("  {}\n", n.content.replace('\n', "\n  "));
                }
            }
        }
    }
    println!("--- 节点总数 {total},process 节点 {procs} ---\n");

    let want = std::env::args().nth(2); // 可选:只详打这个工作流名
    if let Ok(t) = rtx.open_table(KV) {
        for item in t.iter().expect("iter kv") {
            let (k, v) = item.expect("kv entry");
            let key = k.value().to_string();
            if key.starts_with("wf_project") || key.to_lowercase().contains("workflow") {
                let wfs: Vec<serde_json::Value> = serde_json::from_slice(v.value()).unwrap_or_default();
                let names: Vec<String> = wfs
                    .iter()
                    .map(|w| format!("{}({})", w["name"].as_str().unwrap_or("?"), w["scope"].as_str().unwrap_or("?")))
                    .collect();
                println!("KV[{key}] 工作流: {names:?}");
                for w in &wfs {
                    let name = w["name"].as_str().unwrap_or("?");
                    if want.as_deref().map(|x| x != name).unwrap_or(false) {
                        continue;
                    }
                    println!("\n  >>> 工作流「{name}」入口={} 节点:", w["entry"].as_str().unwrap_or("?"));
                    if let Some(nodes) = w["nodes"].as_array() {
                        for n in nodes {
                            let id = n["id"].as_str().unwrap_or("?");
                            let tools: Vec<&str> =
                                n["tools"].as_array().map(|a| a.iter().filter_map(|t| t.as_str()).collect()).unwrap_or_default();
                            let prompt = n["prompt"].as_str().unwrap_or("");
                            println!("  - [{id}] tools={tools:?}");
                            println!("    prompt: {}", prompt.replace('\n', "\n    "));
                        }
                    }
                }
            }
        }
    }
}
