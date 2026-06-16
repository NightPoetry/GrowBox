//! 真机端到端 —— 精确层阶段4(强制跳转指针 / 二级索引)在**真 e5 嵌入 + 真 deepseek judge**下验证。
//!
//! 单测用 mock 潜意识(确定性向量+判定)证逻辑;此处换真 e5 向量 + 真 LLM 相关性判断,
//! 闭环"真实嵌入/判断下这两件也成立"。默认 #[ignore](不打真 API 不计费)。显式跑:
//!   DEEPSEEK_API_KEY=<key> cargo test -p growbox-gui --test live_precision_stage4 -- --ignored --nocapture
//!
//! 模型解析:优先用仓库内已暂存的 e5(`crates/growbox-gui/models/`,full 包暂存处),
//! 落空则 hf-hub 下载到临时目录(本机 HF 缓存命中即快)。

use std::path::PathBuf;
use std::sync::Arc;

use growbox_gui::bridge::{LlmBridge, LlmDriver};
use growbox_llm::{LlmClient, LocalE5Embedder};
use growbox_memory::Memory;

/// 装配真 e5 + 真 deepseek 的潜意识桥 + 一个纯内存 Memory。
fn setup() -> (Memory, LlmBridge) {
    let key = std::env::var("DEEPSEEK_API_KEY").expect("需要环境变量 DEEPSEEK_API_KEY");
    let models_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models");
    let dl = std::env::temp_dir().join("growbox_live_e5");
    let embedder = Arc::new(LocalE5Embedder::new(vec![models_root], dl));
    let driver: Arc<dyn LlmDriver> = Arc::new(LlmClient::new("https://api.deepseek.com", key));
    let bridge = LlmBridge::new(driver, "deepseek-v4-flash", 4096, embedder, 60);
    (Memory::new(), bridge)
}

/// 强制跳转(历史引用):用户在入口位置钉一段历史 → 导航到入口即无条件召回它,
/// 即便那段历史与 query 既不被 e5 判相似、也不被真 judge 判相关。
#[tokio::test]
#[ignore = "打真 API + 真 e5,需 DEEPSEEK_API_KEY"]
async fn live_forced_jump_recalls_unrelated_history() {
    let (mut memory, bridge) = setup();
    let _ = memory.ingest_conversation("Rust 的所有权机制在编译期保证内存安全");
    let entry = memory.ingest_conversation("我们讨论了 Rust 的借用检查器是怎么工作的");
    let history = memory.ingest_conversation("上周末去公园看了樱花,天气特别好,还拍了不少照片");
    memory.ensure_embeddings(&bridge).await;

    let q = "Rust 借用和所有权的问题";
    // 钉之前:无关历史召不回(真 judge 不会把樱花判成与 Rust 相关)。
    let before = memory.retrieve_exact(q, &bridge).await;
    assert!(
        !before.iter().any(|h| h.source == history),
        "未钉前不应召回无关历史,实得 {before:?}"
    );

    // 用户引用历史:在 entry 位置钉强制跳转 → history。
    let src = memory.pin_history_reference(Some(&entry), &history);
    assert_eq!(src.as_deref(), Some(entry.as_str()), "应在 entry 位置钉下");

    // 钉后:导航入口落到 entry → 无条件召回那段无关历史(位置键,绕过 e5/judge)。
    let after = memory.retrieve_exact(q, &bridge).await;
    assert!(
        after.iter().any(|h| h.source == history),
        "钉后强制跳转应召回用户引用的历史(即便 e5/judge 都不认为相关),实得 {after:?}"
    );
    eprintln!("[强制跳转] 真 e5+真 judge 下,无关历史经位置键被强制召回 OK");
}

/// 二级索引(远处拉近):一个热的答案节点随对话推进漂离前沿(≥ K 窗口)后,
/// 命中它时应注册二级锚点,并仍能被召回。验真 e5 向量 + 真 judge 下这套机器跑得通。
///
/// 注:严格的"入口无边、纯靠二级锚点拉近召回"隔离场景由 mock 单测覆盖(真实嵌入下
/// 相似度与相关性强相关,难确定性地解耦);此处验"漂远的热节点被注册进二级索引且仍召回"。
#[tokio::test]
#[ignore = "打真 API + 真 e5,需 DEEPSEEK_API_KEY"]
async fn live_secondary_index_registers_distant_hot_node() {
    let (mut memory, bridge) = setup();
    let _ = memory.ingest_conversation("我们在排查系统整体性能");
    let _entry = memory.ingest_conversation("讨论如何加速数据库的查询速度");
    let t = memory.ingest_conversation(
        "最终方案:给 users 表的 email 列建了 B 树索引,查询从全表扫描变成索引查找,快了很多",
    );
    memory.ensure_embeddings(&bridge).await;

    // ① 真检索:命中数据库索引答案 T,从入口建一条边(网生长)。
    let h1 = memory.retrieve_exact("怎么加速数据库查询", &bridge).await;
    assert!(h1.iter().any(|x| x.source == t), "应命中数据库索引答案 T,实得 {h1:?}");
    assert!(memory.pointer_count() >= 1, "应建出至少一条边");

    // ② 推进对话:塞 ≥2*WINDOW 个无关主题节点,把 T 推到远处(漂移 ≥ K 窗口)。
    for k in 0..18 {
        memory.ingest_conversation(format!("今天午饭吃了第 {k} 样菜,味道还不错"));
    }
    memory.ensure_embeddings(&bridge).await;

    // ③ 同主题再查:T 已漂远,命中它时注册二级锚点(远处拉近的索引)。
    let h3 = memory.retrieve_exact("数据库查询加速的办法", &bridge).await;
    assert!(h3.iter().any(|x| x.source == t), "漂远后仍应召回热的 T,实得 {h3:?}");
    assert!(
        memory.secondary_index_count() >= 1,
        "命中漂远 ≥ K 窗口的热 T 应建二级锚点,实得 {}",
        memory.secondary_index_count()
    );
    eprintln!(
        "[二级索引] 真 e5+真 judge 下,T 漂离前沿(共 {} 节点)后被注册进二级索引并召回 OK",
        memory.timeline().len()
    );
}
