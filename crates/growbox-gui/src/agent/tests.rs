//! `render_working_region` / `render_recent_ring` 的渲染断言(从 agent.rs 内联测试迁出)。

use super::*;
use growbox_memory::ContextBlock;

fn blk(region: Region, role: &str, content: &str) -> ContextBlock {
    ContextBlock {
        region,
        node_id: format!("id-{content}"),
        role: role.into(),
        timestamp: growbox_core::now(),
        content: content.into(),
    }
}

#[test]
fn working_region_has_markers_and_timestamp_rule() {
    let blocks = vec![blk(Region::Working, "user", "旧的相关片段")];
    let s = render_working_region(&blocks, "zh").expect("应有工作区");
    assert!(s.contains("工作记忆区"), "缺区标记");
    assert!(s.contains("按每块的「时间」字段判断"), "缺'按时间戳判先后'铁律提示");
    assert!(s.contains("旧的相关片段"), "缺内容");
    assert!(s.contains("时间 "), "每块应带时间戳");
}

#[test]
fn recent_ring_marked_and_at_hand() {
    let blocks = vec![blk(Region::RecentRing, "assistant", "最近的话")];
    let s = render_recent_ring(&blocks, "zh").expect("应有 ring");
    assert!(s.contains("最近记忆"), "ring 应着重标记'最近记忆'");
    assert!(s.contains("最近的话"));
}

#[test]
fn empty_regions_render_none() {
    assert!(render_working_region(&[], "zh").is_none());
    assert!(render_recent_ring(&[], "zh").is_none());
}

// ★A2 诊断推感知层★:脊柱"本轮编辑了哪些 .rs"判据(edited_rust_path)。
#[test]
fn edited_rust_path_recognizes_successful_rs_edits_only() {
    let dir = tempfile::tempdir().unwrap();
    let wd = dir.path();
    let args = |p: &str| format!(r#"{{"path":"{p}","content":"x"}}"#);

    // file_write/file_edit 成功改 .rs → Some(绝对路径,相对按 work_dir 解析)
    let got = edited_rust_path("file_write", &args("src/a.rs"), true, wd).expect("成功改 .rs 应识别");
    assert_eq!(got, wd.join("src/a.rs"));
    assert!(edited_rust_path("file_edit", &args("src/b.rs"), true, wd).is_some());

    // 失败的编辑不算(没真改成)
    assert!(edited_rust_path("file_write", &args("src/a.rs"), false, wd).is_none(), "失败编辑不该拉诊断");
    // 非 .rs 不算(A1/A2 仅 Rust)
    assert!(edited_rust_path("file_write", &args("src/a.ts"), true, wd).is_none(), "非 .rs 不该识别");
    // 只读/列目录类不算
    assert!(edited_rust_path("file_read", &args("src/a.rs"), true, wd).is_none(), "file_read 不改文件");
    assert!(edited_rust_path("shell", &args("src/a.rs"), true, wd).is_none());
    // 造物文件夹(.growbox/)隔离:不推感知(主记忆隔离)
    assert!(
        edited_rust_path("file_write", &args(".growbox/artifacts/x/gen.rs"), true, wd).is_none(),
        "造物文件夹下的 .rs 不该推主记忆"
    );
    // 坏 JSON / 缺 path → None,不 panic
    assert!(edited_rust_path("file_write", "not json", true, wd).is_none());
}
