//! 清空记忆(干净起步):删除 redb 的记忆表(nodes/conclusions/edges/jumps),
//! **保留 kv 表(settings/projects 等配置)**。用于"旧记忆没用了,从头开始"。
//! ★必须在 GrowBox 退出时跑★(它持写锁,且内存态会回写覆盖)。
//! 用法:cargo run -p growbox-memory --example reset_memory -- <redb 路径>

use redb::{Database, TableDefinition};

const MEMORY_TABLES: [&str; 4] = ["nodes", "conclusions", "edges", "jumps"];

fn main() {
    let path = std::env::args().nth(1).expect("usage: reset_memory <redb path>");
    let db = Database::open(&path).expect("open redb(确认 GrowBox 已退出,否则被写锁占用)");
    let wtx = db.begin_write().expect("write tx");
    for name in MEMORY_TABLES {
        let def: TableDefinition<&str, &[u8]> = TableDefinition::new(name);
        match wtx.delete_table(def) {
            Ok(true) => println!("已删除表: {name}"),
            Ok(false) => println!("表不存在(跳过): {name}"),
            Err(e) => println!("删除表 {name} 失败: {e}"),
        }
    }
    wtx.commit().expect("commit");
    println!("完成。kv 表(settings/projects)保留未动。");
}
