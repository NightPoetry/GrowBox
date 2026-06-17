//! 真机端到端 —— 文档破碎化(`chunk_doc` 让 LLM 判破点)在**真 deepseek + 真 e5**下验证。
//!
//! 单测用词袋 mock 证逻辑(破碎前窄问被稀释下沉、破碎后 RAG 命中聚焦块);此处喂一篇**真实长文档**
//! (dream-board 技术栈约定,正是触发检索盲区的那类投喂),换真 LLM 判破点 + 真 e5 召回,
//! 闭环"真机下破碎质量与窄问召回都成立"。默认 #[ignore](不打真 API 不计费)。显式跑:
//!   DEEPSEEK_API_KEY=<key> cargo test -p growbox-gui --test live_chunk_doc -- --ignored --nocapture
//!
//! 模型解析同 live_precision_stage4:优先仓库内已暂存 e5,落空则 hf-hub 下载到临时目录。

use std::path::PathBuf;
use std::sync::Arc;

use growbox_gui::bridge::{LlmBridge, LlmDriver};
use growbox_llm::{LlmClient, LocalE5Embedder};
use growbox_memory::{Memory, RetrievalConfig};

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

/// 真实长文档(用户投喂的 dream-board 技术栈约定;多主题、含具体命名约定——正是被稀释成一条向量后
/// 窄问必漏的那类内容)。
const DOC: &str = "\
# dream-board 技术栈与约定\n\
服务端用 Express,入口 server.js,视图引擎 EJS。\n\
静态资源与所有路由都挂在子站前缀 basePath 下,basePath 固定为 /dream-board,根路径 / 不提供内容。\n\
模板用 express-ejs-layouts 中间件实现统一布局:依赖必须装、server.js 里 app.use(expressLayouts)、app.set('layout','layout')。\n\
布局文件 views/layout.ejs 持有 html 与 head,正文位置用 body 占位,CSS 链接写在 layout 的 head 里。\n\
数据库用 better-sqlite3,文件 data.db,db.js 封装,同步 API。\n\
站点配置集中在 server/config.js,导出 basePath、siteName、siteDesc、categories、statuses。\n\
主题色 token 统一叫 --db-accent,定义在 public/css/style.css 的 root 里,不要散落硬编码颜色。\n\
想法卡片的 class 统一叫 .idea-card,列表容器叫 .idea-feed。\n\
提交想法走 POST /dream-board/idea/new,成功后重定向回首页。";

/// ★真机破碎质量 + 窄问召回★:喂整篇文档 → 真 LLM 判破点破成小块 → 真 e5 召回窄问命中聚焦块。
#[tokio::test]
#[ignore = "打真 API + 真 e5,需 DEEPSEEK_API_KEY"]
async fn live_chunk_doc_splits_and_narrow_query_hits_focused_chunk() {
    let (mut memory, bridge) = setup();
    // 破碎阈设 400:让这篇 ~600 字文档触发破碎(真机默认 1500;此处只为让样例文档过闸)。
    memory.set_retrieval_config(RetrievalConfig { chunk_min_chars: 400, ..RetrievalConfig::default() });

    let parent = memory.ingest_with_role(DOC, "user");
    assert!(memory.timeline().meta(&parent).unwrap().needs_chunk, "长文档入场应标待破");
    let before_len = memory.timeline().len();

    // 真机破碎:split_sentences → 真 LLM chunk_doc 判破点 → 精确拼接成块。
    let n = memory.chunk_pending_batch(&bridge, 0).await;
    assert_eq!(n, 1, "破了 1 篇文档");
    assert!(memory.timeline().meta(&parent).unwrap().chunked, "父节点应标 chunked");

    // 收集破出的块(父节点之后新增的节点),打印供肉眼看切分质量。
    let chunk_ids: Vec<String> = memory.timeline().metas()[before_len..]
        .iter()
        .map(|m| m.id.clone())
        .collect();
    assert!(chunk_ids.len() >= 2, "应破成 ≥2 块(实际 {})", chunk_ids.len());
    eprintln!("\n========== 真 LLM 破出 {} 块 ==========", chunk_ids.len());
    let mut concat = String::new();
    for (i, id) in chunk_ids.iter().enumerate() {
        let c = memory.timeline().content(id).unwrap_or_default();
        eprintln!("--- 块 {} ({} 字) ---\n{}", i + 1, c.chars().count(), c.trim());
        concat.push_str(&c);
    }
    eprintln!("=======================================\n");

    // 零丢字:各块精确拼接还原原文(破点只在句末,不改写)。
    assert_eq!(concat, DOC, "各块拼接应精确还原原文(零丢字零改写)");
    // 没有哪一块就是整篇(确实破开了,不是退回大节点)。
    let doc_len = DOC.chars().count();
    for id in &chunk_ids {
        let len = memory.timeline().content(id).unwrap().chars().count();
        assert!(len < doc_len, "每块都应小于整篇({len} vs {doc_len})");
    }

    // 嵌入各块(父被跳过),真 e5 建索引。
    memory.ensure_embeddings(&bridge).await;

    // 三个窄问:真 e5 召回应命中**含该事实的聚焦块**,而非整篇/无关块(治旧版凭空答 --primary/sql.js)。
    for (q, fact) in [
        ("想法卡片的 CSS class 叫什么", "idea-card"),
        ("主题色 token 是什么", "db-accent"),
        ("这个项目用什么数据库", "better-sqlite3"),
    ] {
        let (hits, layer) = memory.retrieve(q, &bridge).await;
        eprintln!(
            "[窄问] 「{q}」-> {:?} 层,命中 {} 条;首条含「{fact}」= {}",
            layer,
            hits.len(),
            hits.first().map(|h| h.content.contains(fact)).unwrap_or(false)
        );
        assert!(
            hits.iter().any(|h| h.content.contains(fact)),
            "窄问「{q}」应召回含「{fact}」的块,实得 {hits:?}"
        );
        // 命中的不是整篇父节点(父已退出索引/检索)。
        assert!(
            !hits.iter().any(|h| h.source == parent),
            "已破父节点不该被召回(应是聚焦小块)"
        );
    }
    eprintln!("[真机] 破碎质量 + 三窄问聚焦召回 全部 OK");
}
