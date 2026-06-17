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

/// 更长、更多事实点的"完整版"约定文档(14 个分散在 6 个小节里的具体事实)。
/// 整篇压成一条 e5 向量时,稀释更狠 —— 正好放大 before/after 的对照。
const LONG_DOC: &str = "\
# dream-board 项目约定(完整版)\n\
## 服务端\n\
入口是 server.js,Web 框架用 Express,视图引擎用 EJS。\n\
服务监听端口固定为 3000,本地访问 http://localhost:3000/dream-board/。\n\
所有静态资源与路由都挂在子站前缀 basePath 下,basePath 固定为 /dream-board,根路径 / 不提供任何内容。\n\
会话管理用 express-session 中间件,session 的 secret 从 .env 读取,不写死在代码里。\n\
## 模板与布局\n\
统一布局用 express-ejs-layouts 中间件实现:依赖必须装,server.js 里要 app.use(expressLayouts) 并 app.set('layout','layout')。\n\
布局文件是 views/layout.ejs,它持有 html 与 head 标签,正文用 body 占位,所有 CSS 链接都写在 layout 的 head 里。\n\
## 数据\n\
数据库用 better-sqlite3,数据文件是 data.db,所有读写经 db.js 封装,用的是同步 API。\n\
站点配置集中在 server/config.js,导出 basePath、siteName、siteDesc、categories、statuses 这些字段。\n\
## 样式约定\n\
主题色 token 统一叫 --db-accent,定义在 public/css/style.css 的 root 里,任何地方都不要散落硬编码颜色。\n\
想法卡片的 class 统一叫 .idea-card,卡片列表的容器叫 .idea-feed。\n\
## 路由\n\
提交一个新想法走 POST /dream-board/idea/new,成功后重定向回首页。\n\
给某个想法投票走 POST /dream-board/idea/:id/vote,一个用户对同一想法只能投一次。\n\
## 账号与部署\n\
后台管理员账号是 admin,初始密码 admin123,首次登录后应尽快修改。\n\
线上部署用 pm2 做进程管理,进程名叫 dream-board,用 pm2 reload 实现零停机更新。";

/// 10 个窄问 → 各自的事实关键词(每个落在文档不同小节里)。
const NARROW_QUERIES: &[(&str, &str)] = &[
    ("想法卡片的 CSS class 叫什么", "idea-card"),
    ("主题色 token 是什么", "db-accent"),
    ("这个项目用什么数据库", "better-sqlite3"),
    ("统一布局用什么中间件", "express-ejs-layouts"),
    ("站点配置集中在哪个文件", "config.js"),
    ("给想法投票走哪个路由", "vote"),
    ("后台管理员的初始密码", "admin123"),
    ("会话管理用什么库", "express-session"),
    ("服务监听哪个端口", "3000"),
    ("线上用什么做进程管理", "pm2"),
];

/// ★before/after 对照:证明确实"修对了"★
/// 同一篇文档、同一个真 LLM + 真 e5:
///   - 破碎前(关破碎,整篇一个节点):窄问只可能召回**整篇干草堆**(含事实但不聚焦)→ 0 个聚焦命中。
///   - 破碎后(开破碎,真 LLM 破点):窄问召回**聚焦小块**(含事实且远小于整篇)→ 全部聚焦命中。
/// 判据 served = 召回里存在"含该事实 且 长度 < 整篇一半"的块。整篇命中结构上不可能满足 → 破碎前必为 0。
#[tokio::test]
#[ignore = "打真 API + 真 e5,需 DEEPSEEK_API_KEY"]
async fn live_before_vs_after_fix_focused_recall() {
    let (mut broken, bridge) = setup();
    let doc_len = LONG_DOC.chars().count();
    // served = 召回里存在"含该事实 且 聚焦(长度 < 整篇一半)"的块。整篇命中长度=doc_len 不满足。
    let served = |hits: &[growbox_memory::Hit], fact: &str| {
        hits.iter()
            .any(|h| h.content.contains(fact) && h.content.chars().count() < doc_len / 2)
    };

    // —— 破碎前:关破碎(chunk_min_chars=0),整篇当一个节点嵌入 ——
    broken.set_retrieval_config(RetrievalConfig { chunk_min_chars: 0, ..RetrievalConfig::default() });
    let pb = broken.ingest_with_role(LONG_DOC, "user");
    assert!(!broken.timeline().meta(&pb).unwrap().needs_chunk, "破碎关 → 不标待破,整篇一个节点");
    broken.ensure_embeddings(&bridge).await;
    let mut broken_served = 0usize;
    eprintln!("\n========== 破碎前(整篇一条向量)==========");
    for (q, fact) in NARROW_QUERIES {
        let (hits, layer) = broken.retrieve(q, &bridge).await;
        let ok = served(&hits, fact);
        if ok {
            broken_served += 1;
        }
        let top_len = hits.first().map(|h| h.content.chars().count()).unwrap_or(0);
        eprintln!("「{q}」-> {:?} {}条, 首条{}字, 聚焦命中={}", layer, hits.len(), top_len, ok);
    }

    // —— 破碎后:开破碎(阈 400),真 LLM 破点 → 各块独立嵌入 ——
    let mut fixed = Memory::new();
    fixed.set_retrieval_config(RetrievalConfig { chunk_min_chars: 400, ..RetrievalConfig::default() });
    let pf = fixed.ingest_with_role(LONG_DOC, "user");
    let n = fixed.chunk_pending_batch(&bridge, 0).await;
    assert_eq!(n, 1, "破了 1 篇");
    assert!(fixed.timeline().meta(&pf).unwrap().chunked, "父节点已 chunked");
    fixed.ensure_embeddings(&bridge).await;
    let chunk_n = fixed.timeline().len() - 1; // 减去父节点
    let mut fixed_served = 0usize;
    eprintln!("\n========== 破碎后(真 LLM 破成 {chunk_n} 块)==========");
    for (q, fact) in NARROW_QUERIES {
        let (hits, layer) = fixed.retrieve(q, &bridge).await;
        let ok = served(&hits, fact);
        if ok {
            fixed_served += 1;
        }
        let top_len = hits.first().map(|h| h.content.chars().count()).unwrap_or(0);
        eprintln!("「{q}」-> {:?} {}条, 首条{}字, 聚焦命中={}", layer, hits.len(), top_len, ok);
    }

    let total = NARROW_QUERIES.len();
    eprintln!("\n=== 聚焦召回:破碎前 {broken_served}/{total}  破碎后 {fixed_served}/{total} ===\n");
    // 破碎前:整篇唯一节点,结构上不存在"聚焦小块"→ 必为 0。
    assert_eq!(broken_served, 0, "破碎前不可能有聚焦命中(只有整篇干草堆)");
    // 破碎后:每个窄问都该拿到聚焦块(留 1 个真机抖动余量)。
    assert!(
        fixed_served >= total - 1,
        "破碎后窄问应几乎全部聚焦命中(实得 {fixed_served}/{total})"
    );
}
