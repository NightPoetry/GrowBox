//! Memory 的单元测试(分层检索/指针学习/感知/维护),从 memory.rs 内联测试迁出。

use super::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Mock 潜意识:embed 按"是否含关键词"产出可区分向量;记录 judge 调用次数。
struct MockSub {
    keyword: String,
    judge_calls: Arc<AtomicUsize>,
}
#[async_trait]
impl Subconscious for MockSub {
    async fn embed(&self, text: &str) -> Vec<f32> {
        // 含关键词 → [1,0];否则 → [0,1]。query 也走同逻辑。
        if text.contains(&self.keyword) {
            vec![1.0, 0.0]
        } else {
            vec![0.0, 1.0]
        }
    }
    async fn judge_relevant(&self, query: &str, candidates: &[String]) -> Vec<usize> {
        self.judge_calls.fetch_add(1, Ordering::SeqCst);
        candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.contains(query))
            .map(|(i, _)| i)
            .collect()
    }
}

#[tokio::test]
async fn rag_hit_does_not_descend() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MockSub { keyword: "Rust".into(), judge_calls: calls.clone() };
    let mut m = Memory::new();
    m.ingest_conversation("Rust 是系统语言");
    m.ensure_embeddings(&sub).await;

    let (hits, layer) = m.retrieve("Rust", &sub).await;
    assert_eq!(layer, Layer::Rag, "RAG 应命中");
    assert_eq!(hits.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "RAG 命中不应下沉调用 judge");
}

#[tokio::test]
async fn descends_to_exact_when_rag_misses() {
    let calls = Arc::new(AtomicUsize::new(0));
    // query 关键词与库内容不相似 → RAG 未命中 → 下沉
    let sub = MockSub { keyword: "数据库".into(), judge_calls: calls.clone() };
    let mut m = Memory::new();
    m.ingest_conversation("配置数据库连接");
    m.ensure_embeddings(&sub).await;

    // query "连接" 不含关键词"数据库" → query 向量 [0,1],node 向量 [1,0] → cosine 0 → 下沉
    let (hits, layer) = m.retrieve("连接", &sub).await;
    assert_eq!(layer, Layer::Exact, "应下沉精确层");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "精确层应调一次 judge");
    assert_eq!(hits.len(), 1, "精确层按原文匹配应命中");
}

/// ★假指针端到端★:纯 RAG 命中经 assemble_context 进存放区 = 假指针(RagFake),
/// 且**不落二级锚、不进碎片**(假指针铁律,见 `用户决策/记忆架构-索引区与存放区.md`)。
#[tokio::test]
async fn rag_hit_pages_in_as_fake_pointer_without_sequence() {
    let sub = MockSub { keyword: "Rust".into(), judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    m.ingest_conversation("Rust 是系统语言");
    m.ensure_embeddings(&sub).await;

    let _ = m.assemble_context("Rust", &sub).await; // RAG 命中 → 假指针进缓存队列
    assert_eq!(m.context_fake_pointers(), 1, "RAG 命中 = 假指针进存放区");
    assert_eq!(m.context_real_pointers(), 0);
    assert_eq!(m.secondary_index_count(), 0, "假指针换入不落二级锚");
    assert_eq!(m.fragment_count(), 0, "假指针不进碎片系统");
}

/// L2(Exact)命中经 assemble_context 进存放区 = 真指针(Llm)。
#[tokio::test]
async fn exact_hit_pages_in_as_real_pointer() {
    let sub = MockSub { keyword: "数据库".into(), judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    m.ingest_conversation("配置数据库连接");
    m.ensure_embeddings(&sub).await;

    let _ = m.assemble_context("连接", &sub).await; // RAG 未命中 → 下沉 Exact → 真指针
    assert!(m.context_real_pointers() >= 1, "Exact 命中 = 真指针进存放区");
    assert_eq!(m.context_fake_pointers(), 0, "Exact 路径不产假指针");
}

/// 二期 process kind 建议档:`ingest_process` 写入 role=process,`retrieve_processes` 只召回 process 那条。
#[tokio::test]
async fn retrieve_processes_filters_to_process_kind() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MockSub { keyword: "Rust".into(), judge_calls: calls.clone() };
    let mut m = Memory::new();
    // 一条 process 建议档 + 一条普通对话,都含关键词"Rust"(同向量 → 都被 RAG 召回)。
    let pid = m.ingest_process("Rust 项目加设置碰 Settings/命令/前端/i18n");
    m.ingest_with_role("Rust 是系统语言", "user");
    m.ensure_embeddings(&sub).await;

    let procs = m.retrieve_processes("Rust", &sub).await;
    assert_eq!(procs.len(), 1, "只应召回 process kind 那条(普通对话被过滤)");
    assert_eq!(procs[0].source, pid, "召回的正是写入的 process 节点");
    assert!(procs[0].content.contains("加设置"), "内容是流程配方原文");
}

/// ★二期 B2 指针接通(越用越准)★:召回一条流程 → 记正 K(哨兵→流程边);
/// 抑制(被更正版取代,记反 K)→ 同族 query 再召回时被一票否决、不再浮现。
#[tokio::test]
async fn process_recall_reinforces_then_suppression_vetoes() {
    let sub = MockSub { keyword: "设置".into(), judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    let pid = m.ingest_process("加设置碰 13 处:Settings → 命令 → tauri-api → 四国 i18n");
    m.ensure_embeddings(&sub).await;

    // 首次召回:返回该流程,并在 哨兵 → 流程 边记正 K(召回=该 query 族在用)。
    let hits = m.retrieve_processes("设置", &sub).await;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source, pid);
    assert!(
        m.pointer_neighbors(PROCESS_RECALL_SOURCE).contains(&pid),
        "召回应在哨兵源记一条到该流程的正 K 边"
    );

    // 抑制(被更正版取代)→ 反 K(键=同族 query 向量)。
    let qv = sub.embed_query("设置").await;
    m.suppress_process_recall(&pid, &qv);

    // 再召回:同族 query 被反 K 一票否决 → 流程不再浮现(误召/过时的被压制)。
    let hits2 = m.retrieve_processes("设置", &sub).await;
    assert!(hits2.is_empty(), "被取代的流程,同族 query 应被反 K 压制、不再召回");
}

/// ★二期 B3 结晶(报告-纠正回路写入半)★:首次结晶新建 + 即时可召回;
/// 更正版(近重复)结晶 → 取代旧版,旧版被反 K 压制、只剩更正版召回(同一回路既建又修)。
#[tokio::test]
async fn crystallize_process_creates_then_supersedes_near_duplicate() {
    let sub = MockSub { keyword: "设置".into(), judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();

    // 1) 首次结晶:库里无近重复 → 新建,不取代;且即时嵌入 → 立刻可召回。
    let (id1, sup1) = m.crystallize_process("加设置碰 9 处:Settings → 命令 → 前端", &sub).await;
    assert!(sup1.is_none(), "首次结晶无近重复可取代");
    let hits = m.retrieve_processes("设置", &sub).await;
    assert_eq!(hits.len(), 1, "结晶后立刻可召回(即时嵌入)");
    assert_eq!(hits[0].source, id1);

    // 2) 更正版结晶(同族 → 近重复 cos≥阈)→ 取代 id1。
    let (id2, sup2) = m.crystallize_process("加设置碰 13 处(补四国 i18n 与 state 信号)", &sub).await;
    assert_eq!(sup2.as_deref(), Some(id1.as_str()), "近重复应取代旧版");
    assert_ne!(id1, id2, "更正版是新节点(append-only,旧版不删)");

    // 3) 再召回:旧版被反 K 压制,只剩更正版浮现。
    let ids: Vec<String> = m.retrieve_processes("设置", &sub).await.into_iter().map(|h| h.source).collect();
    assert!(ids.contains(&id2), "更正版应召回");
    assert!(!ids.contains(&id1), "被取代的旧版不再召回");
}

/// ★Skill S1(设计/09)★:crystallize_skill 新建即时可召回 + 同名取代旧版 + 反 K 压制;
/// learned_skill_body 按名取正文;learned_skill_listing 给出 (name, trigger)。与 process 同构、各用一源。
#[tokio::test]
async fn skill_crystallize_load_and_supersede() {
    let sub = MockSub { keyword: "调试".into(), judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();

    // 1) 首次结晶:新建,不取代;即时嵌入 → 立刻可语义召回(query 含关键词"调试")。
    let (id1, sup1) = m
        .crystallize_skill("web-debug-locate", "调试网页框选后反查源码时", "1. 看 data-source\n2. code_search", &sub)
        .await;
    assert!(sup1.is_none(), "首次结晶无近重复");
    let hits = m.retrieve_skills("调试", &sub).await;
    assert_eq!(hits.len(), 1, "结晶后立刻可召回");
    assert_eq!(hits[0].source, id1);
    // 正 K 边记在 skill 哨兵源(与 process 哨兵互不干扰)。
    assert!(m.pointer_neighbors(SKILL_RECALL_SOURCE).contains(&id1), "召回记正 K 到 skill 哨兵源");

    // 2) 按名取正文(已学优先)。
    let body = m.learned_skill_body("web-debug-locate").expect("按名取正文");
    assert!(body.contains("code_search"), "正文是 playbook 全文");
    // 清单给出 (name, trigger)。
    let listing = m.learned_skill_listing();
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0].0, "web-debug-locate");
    assert_eq!(listing[0].1, "调试网页框选后反查源码时");

    // 3) 同名更正版结晶 → 取代 id1,旧版反 K 压制,只剩更正版召回。
    let (id2, sup2) = m
        .crystallize_skill("web-debug-locate", "调试网页框选后反查源码时", "改进版:先装 dev-inspector 插件", &sub)
        .await;
    assert_eq!(sup2.as_deref(), Some(id1.as_str()), "同名应取代旧版");
    assert_ne!(id1, id2, "更正版是新节点(append-only)");
    let ids: Vec<String> = m.retrieve_skills("调试", &sub).await.into_iter().map(|h| h.source).collect();
    assert!(ids.contains(&id2) && !ids.contains(&id1), "只剩更正版召回,旧版被压制");
    // 取正文已是更正版。
    assert!(m.learned_skill_body("web-debug-locate").unwrap().contains("dev-inspector"));
}

/// ★工具记忆(计划/工具记忆-不犯第二遍)★:crystallize 即时可会诊;consult 返回最相似且最新一条;
/// 成本门 tool_memory_count;最新覆盖旧结论(关键因素变化自校正)。
#[tokio::test]
async fn tool_memory_crystallize_consult_and_latest_wins() {
    use crate::tool_memory_format::Verdict;
    let sub = MockSub { keyword: "项目".into(), judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    assert_eq!(m.tool_memory_count(), 0, "起步无工具记忆 → 成本门关(脊柱跳过会诊)");

    // 记一条:mcp_fs 访问项目内容不可行。即时嵌入 → 立刻可会诊。
    m.crystallize_tool_memory("mcp_fs", "访问当前项目内容", Verdict::Infeasible, "沙箱不含项目目录,够不到", &sub).await;
    assert_eq!(m.tool_memory_count(), 1);
    let (v, content, score) = m.consult_tool_memory("mcp_fs", "读当前项目内容", &sub).await.expect("应会诊到");
    assert_eq!(v, Verdict::Infeasible);
    assert!(content.contains("够不到"));
    assert!(score > 0.0, "相似度 > 0");

    // 别的工具无记忆 → None(不会误伤其它工具)。
    assert!(m.consult_tool_memory("shell", "随便", &sub).await.is_none());

    // 关键因素变化 → 记新结论(works);并列相似度下 created_at 最新者胜出(覆盖旧 infeasible)。
    m.crystallize_tool_memory("mcp_fs", "访问当前项目内容", Verdict::Works, "已把项目目录加进该 server 沙箱", &sub).await;
    let (v2, content2, _) = m.consult_tool_memory("mcp_fs", "访问当前项目内容", &sub).await.expect("仍会诊到");
    assert_eq!(v2, Verdict::Works, "最新结论覆盖旧的(自校正)");
    assert!(content2.contains("加进"));
}

/// 检索旋钮即时生效:把 RAG 命中阈抬到 >1(余弦最高 1.0,永不命中)→ 本会命中 RAG 的
/// query 也被迫下沉精确层。证明 RetrievalConfig 接通且 retrieve() 读的是旋钮而非常量。
#[tokio::test]
async fn retrieval_config_rag_threshold_forces_descent() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MockSub { keyword: "Rust".into(), judge_calls: calls.clone() };
    let mut m = Memory::new();
    m.ingest_conversation("Rust 是系统语言");
    m.ensure_embeddings(&sub).await;

    // 默认阈值(0.85)下:RAG 命中,不下沉。
    let (_, layer) = m.retrieve("Rust", &sub).await;
    assert_eq!(layer, Layer::Rag, "默认阈值下 RAG 命中");

    // 把命中阈抬到 1.01 → 同 query 被迫下沉精确层(旋钮即时生效)。
    m.set_retrieval_config(RetrievalConfig {
        rag_hit_threshold: 1.01,
        ..RetrievalConfig::default()
    });
    assert!((m.retrieval_config().rag_hit_threshold - 1.01).abs() < 1e-6, "旋钮已落");
    let (_, layer2) = m.retrieve("Rust", &sub).await;
    assert_eq!(layer2, Layer::Exact, "抬高 RAG 命中阈后被迫下沉(检索旋钮即时生效)");
}

#[tokio::test]
async fn assemble_context_recent_into_ring_ordered_with_timestamps() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MockSub { keyword: "Rust".into(), judge_calls: calls.clone() };
    let mut m = Memory::new();
    m.ingest_with_role("Rust 是系统语言", "user");
    m.ingest_with_role("好的,已了解", "assistant");
    m.ensure_embeddings(&sub).await;

    let blocks = m.assemble_context("Rust", &sub).await;
    assert!(!blocks.is_empty(), "应组装出上下文块");
    // ring 预算足够 → 最近节点都进 ring(被 ring 覆盖的不再重复进工作区)。
    assert!(blocks.iter().all(|b| b.region == Region::RecentRing));
    assert!(blocks.iter().any(|b| b.role == "user"));
    // 每块带完整时间戳;ring 按时间正序(旧→新)。
    let ts: Vec<_> = blocks.iter().map(|b| b.timestamp).collect();
    assert!(ts.windows(2).all(|w| w[0] <= w[1]), "ring 应按时间正序");
    // 两态:再次组装,块集合稳定、不重复膨胀。
    let blocks2 = m.assemble_context("Rust", &sub).await;
    assert_eq!(blocks.len(), blocks2.len(), "重复组装不应让上下文膨胀");
}

#[tokio::test]
async fn working_region_is_byte_stable_across_turns() {
    // P4 命根子:同 query 两次组装,工作记忆区的块序列(id+内容+时间戳+顺序)必须逐字一致,
    // 否则稳定前缀被打碎、prompt 缓存失效。用极小 ring 预算把检索命中逼进工作区来测它。
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MockSub { keyword: "Rust".into(), judge_calls: calls.clone() };
    let mut m = Memory::new();
    m.ingest_with_role("Rust 所有权 1", "user");
    m.ingest_with_role("Rust 生命周期 2", "user");
    m.ingest_with_role("Rust 借用检查 3", "user");
    m.ensure_embeddings(&sub).await;
    m.configure_context(50_000, 1); // ring=1 → 只最新进 ring,其余检索命中落工作区

    let key = |bs: &[ContextBlock]| -> Vec<(String, String, String)> {
        bs.iter()
            .filter(|b| b.region == Region::Working)
            .map(|b| (b.node_id.clone(), b.content.clone(), b.timestamp.to_rfc3339()))
            .collect()
    };
    let a = key(&m.assemble_context("Rust", &sub).await);
    let b = key(&m.assemble_context("Rust", &sub).await);
    assert!(!a.is_empty(), "应有工作区块(检索命中且未被 ring 覆盖)");
    assert_eq!(a, b, "同 query 两次组装,工作区必须逐字一致(否则破 prompt 缓存)");
}

/// 网状测试用的 mock:嵌入由 TAG 词决定(进图的门),相关性由 REL 标记决定
/// (谁是答案)——把"嵌入相似"和"是否相关"解耦,才能造出"入口 N 与目标 T 不同节点"
/// 的联想场景(mesh 的本质:N→T 边,经 N 召回 T)。
struct MeshSub {
    judge_calls: Arc<AtomicUsize>,
}
#[async_trait]
impl Subconscious for MeshSub {
    async fn embed(&self, text: &str) -> Vec<f32> {
        if text.contains("TAGA") {
            vec![1.0, 0.0, 0.0]
        } else if text.contains("TAGB") {
            vec![0.0, 1.0, 0.0]
        } else {
            vec![0.0, 0.0, 1.0]
        }
    }
    async fn judge_relevant(&self, _query: &str, candidates: &[String]) -> Vec<usize> {
        self.judge_calls.fetch_add(1, Ordering::SeqCst);
        candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.contains("REL"))
            .map(|(i, _)| i)
            .collect()
    }
}

/// 线性扫命中时,从入口节点向目标建一条边(网生长);Deep 染色。
#[tokio::test]
async fn linear_scan_grows_edge_from_entry() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MeshSub { judge_calls: calls.clone() };
    let mut m = Memory::new();
    let entry = m.ingest_conversation("TAGA 入口提问"); // 嵌入 [1,0,0],非 REL
    let target = m.ingest_conversation("REL 目标答案"); // 嵌入 [0,0,1],REL
    m.ensure_embeddings(&sub).await;

    // query 含 TAGA → 入口=entry 节点;线性扫命中 target(含 REL)→ 建边 entry→target。
    let hits = m.retrieve_exact("TAGA 找答案", &sub).await;
    assert!(hits.iter().any(|h| h.source == target), "应命中 target");
    assert_eq!(m.timeline().get(&target).unwrap().stain, Stain::Deep, "线性扫过染 Deep");
    assert_eq!(m.pointer_count(), 1, "应建一条 entry→target 边");
    assert!(!m.pointer_neighbors(&entry).is_empty(), "边挂在 entry 名下");
}

/// mesh 跳转:边建好后,经入口沿出边直达关联目标——即便 target 已 Deep 也召回,
/// 且不再线性重扫其余时间线(短路)。
#[tokio::test]
async fn mesh_hop_recalls_deep_target_without_rescanning() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MeshSub { judge_calls: calls.clone() };
    let mut m = Memory::new();
    let _entry = m.ingest_conversation("TAGA 入口提问");
    let target = m.ingest_conversation("REL 目标答案");
    m.ensure_embeddings(&sub).await;

    // 第一次:线性扫建边 entry→target(target 染 Deep)。
    m.retrieve_exact("TAGA 找答案", &sub).await;
    assert_eq!(m.timeline().get(&target).unwrap().stain, Stain::Deep);

    // 插入一个干扰节点:它在最近端,线性扫会碰到,但 mesh 跳转不该碰它。
    let distractor = m.ingest_conversation("TAGB 干扰无关");
    m.ensure_embeddings(&sub).await;

    // 第二次同类 query:经 entry 出边直达 target(Deep 仍召回),干扰节点不被扫。
    let hits = m.retrieve_exact("TAGA 再找", &sub).await;
    assert!(hits.iter().any(|h| h.source == target), "经 mesh 边应召回 Deep 的 target");
    assert_eq!(
        m.timeline().get(&distractor).unwrap().stain,
        Stain::None,
        "mesh 短路:干扰节点未被线性扫,仍 None"
    );
}

// ===================== 阶段2:学习型指针(正/负 K 在检索中回填) =====================

/// 嵌入由 TAGA 决定(进图的门);相关性由 query 是否含 "good" 控制——
/// 让**同一 target 在不同 query 下相关/不相关**,才能造出"边被跟随但 judge 拒"的负样本场景。
struct LearnSub {
    judge_calls: Arc<AtomicUsize>,
}
#[async_trait]
impl Subconscious for LearnSub {
    async fn embed(&self, text: &str) -> Vec<f32> {
        if text.contains("TAGA") {
            vec![1.0, 0.0, 0.0]
        } else {
            vec![0.0, 0.0, 1.0]
        }
    }
    async fn judge_relevant(&self, query: &str, candidates: &[String]) -> Vec<usize> {
        self.judge_calls.fetch_add(1, Ordering::SeqCst);
        if !query.contains("good") {
            return vec![]; // 本次 query 非 "good" → 全判不相关
        }
        candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.contains("REL"))
            .map(|(i, _)| i)
            .collect()
    }
}

/// 边复用(judge 受)→ 在边上累积一个正 K(去重 + heat+1),且带 query 原文。
#[tokio::test]
async fn mesh_reuse_records_positive_k() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = LearnSub { judge_calls: calls.clone() };
    let mut m = Memory::new();
    let entry = m.ingest_conversation("TAGA 入口提问"); // [1,0,0]
    let target = m.ingest_conversation("REL 目标答案"); // [0,0,1], REL
    m.ensure_embeddings(&sub).await;

    // 第一次:线性扫命中 target → 建边 entry→target(首个正 K,text="TAGA good build")。
    m.retrieve_exact("TAGA good build", &sub).await;
    let e1 = m.edges_of(&entry);
    let p1 = e1.iter().find(|p| p.target == target).expect("应建 entry→target 边");
    assert_eq!(p1.positives.len(), 1);
    assert_eq!(p1.positives[0].weight, 1, "建边一个正 K");
    assert!(!p1.positives[0].text.is_empty(), "正 K 带 query 原文");

    // 第二次同类 query:经边召回 target(judge 受)→ 复用 = 正 K 累积(近似坍缩 weight+1)。
    let hits = m.retrieve_exact("TAGA good again", &sub).await;
    assert!(hits.iter().any(|h| h.source == target), "经 mesh 边召回 target");
    let e2 = m.edges_of(&entry);
    let p2 = e2.iter().find(|p| p.target == target).unwrap();
    assert_eq!(p2.positives.len(), 1, "近似 query 坍缩到一个真实正 K");
    assert_eq!(p2.positives[0].weight, 2, "复用给正 K weight+1");
    assert_eq!(p2.heat, 2, "复用累 heat");
    assert!(p2.negatives.is_empty(), "受的 query 不产反 K");
}

/// 边被跟随但 judge 拒 → 在边上记一个反 K(此前这信息被丢弃 = 规格点名的关键缺口)。
#[tokio::test]
async fn judge_reject_records_negative_k() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = LearnSub { judge_calls: calls.clone() };
    let mut m = Memory::new();
    let entry = m.ingest_conversation("TAGA 入口提问"); // [1,0,0]
    let target = m.ingest_conversation("REL 目标答案"); // [0,0,1], REL
    m.ensure_embeddings(&sub).await;

    // 先用 "good" query 建边 entry→target。
    m.retrieve_exact("TAGA good build", &sub).await;
    assert!(m.edges_of(&entry).iter().any(|p| p.target == target), "应已建边");

    // 再用非 "good" query:边被跟随(topic [1,0,0] 匹配),但 judge 拒 target → 记反 K。
    m.retrieve_exact("TAGA bad probe", &sub).await;
    let e = m.edges_of(&entry);
    let p = e.iter().find(|p| p.target == target).unwrap();
    assert_eq!(p.negatives.len(), 1, "judge 拒回填一个反 K");
    assert!(!p.negatives[0].text.is_empty(), "反 K 带被拒 query 原文");
    // 反 K 不混入正 K;positives 仍只是建边那一个。
    assert_eq!(p.positives.len(), 1);
}

/// force_judge_on_cosine_hit=false:档A 余弦命中的边 target 直接采纳,跳过前沿 judge。
/// 对照 judge_reject_records_negative_k(force_judge=true 默认下,同样的非 good query 会被前沿 judge 拒)。
#[tokio::test]
async fn force_judge_off_accepts_cosine_hit_without_judge() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = LearnSub { judge_calls: calls.clone() };
    let mut m = Memory::new();
    let entry = m.ingest_conversation("TAGA 入口提问"); // [1,0,0]
    let target = m.ingest_conversation("REL 目标答案"); // [0,0,1], REL
    m.ensure_embeddings(&sub).await;

    // 建边(线性扫 judge 一次,query 含 good → 受)。
    m.retrieve_exact("TAGA good build", &sub).await;
    assert!(m.edges_of(&entry).iter().any(|p| p.target == target), "应已建边");

    // 关掉 force_judge(档A 默认模式)。
    m.set_pointer_config(PointerConfig {
        force_judge_on_cosine_hit: false,
        ..PointerConfig::default()
    });

    // 非 good query:档A 余弦命中边(topic [1,0,0])→ 直接采纳,不再调前沿 judge。
    let before = calls.load(Ordering::SeqCst);
    let hits = m.retrieve_exact("TAGA probe", &sub).await;
    assert!(
        hits.iter().any(|h| h.source == target),
        "force_judge=false:余弦命中即采纳,召回 target(force_judge=true 时此非 good query 会被拒)"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        before,
        "档A 命中跳过前沿 judge,无新 judge 调用"
    );
}

/// 阶段3 档A 反 K 一票否决:边被反 K 挡住 → mesh 不跳,连已 Deep 的 target 也召不回。
/// 带对照组(无反 K 时 mesh 能召回 Deep target),隔离出否决的因果。
#[tokio::test]
async fn neg_veto_blocks_mesh_recall_of_deep_target() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MeshSub { judge_calls: calls.clone() };
    let mut m = Memory::new();
    let entry = m.ingest_conversation("TAGA 入口提问"); // [1,0,0]
    let target = m.ingest_conversation("REL 目标答案"); // [0,0,1], REL
    m.ensure_embeddings(&sub).await;

    // 建边 entry→target(线性扫,target 染 Deep)。
    m.retrieve_exact("TAGA 建边", &sub).await;
    assert_eq!(m.timeline().get(&target).unwrap().stain, Stain::Deep);

    // 对照组:无反 K 时,mesh 跳转能召回已 Deep 的 target。
    let before = m.retrieve_exact("TAGA 再找", &sub).await;
    assert!(before.iter().any(|h| h.source == target), "无反 K:mesh 召回 Deep target");

    // 注入一个反 K,其向量 = TAGA 方向(= 后续 query 方向)。
    let qv = sub.embed("TAGA").await; // [1,0,0]
    m.record_negative_edge(&entry, &target, "曾误跳的 query", &qv);

    // 实验组:同类 TAGA query 被反 K 一票否决 → mesh 不跳;Deep target 线性扫也跳过 → 召不回。
    let after = m.retrieve_exact("TAGA 三找", &sub).await;
    assert!(!after.iter().any(|h| h.source == target), "反 K 否决:mesh 不召回,Deep target 落空");
}

/// 嵌入按 TAGA;judge_relevant 按 REL(建边用);judge_edge 按 query 是否含 "jump"(档B 跟随决策)。
struct EdgeJudgeSub {
    judge_calls: Arc<AtomicUsize>,
    edge_calls: Arc<AtomicUsize>,
}
#[async_trait]
impl Subconscious for EdgeJudgeSub {
    async fn embed(&self, text: &str) -> Vec<f32> {
        if text.contains("TAGA") {
            vec![1.0, 0.0, 0.0]
        } else {
            vec![0.0, 0.0, 1.0]
        }
    }
    async fn judge_relevant(&self, _query: &str, candidates: &[String]) -> Vec<usize> {
        self.judge_calls.fetch_add(1, Ordering::SeqCst);
        candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.contains("REL"))
            .map(|(i, _)| i)
            .collect()
    }
    async fn judge_edge(&self, query: &str, _pos: &[String], _neg: &[String], _target: &str) -> bool {
        self.edge_calls.fetch_add(1, Ordering::SeqCst);
        query.contains("jump") // 档B:此 mock 以 query 含 "jump" 代表 LLM 判"值得跳"
    }
}

/// 阶段4 档B:切到 LlmJudge 档后,边的跟随判定走 `judge_edge`(读正负 K 综合判断),
/// 而非档A 的加权余弦。judge_edge 判跳→召回;判不跳→连已 Deep 的 target 也召不回。
#[tokio::test]
async fn match_mode_b_drives_follow_via_judge_edge() {
    let jc = Arc::new(AtomicUsize::new(0));
    let ec = Arc::new(AtomicUsize::new(0));
    let sub = EdgeJudgeSub { judge_calls: jc, edge_calls: ec.clone() };
    let mut m = Memory::new();
    let _entry = m.ingest_conversation("TAGA 入口提问"); // [1,0,0]
    let target = m.ingest_conversation("REL 目标答案"); // [0,0,1], REL
    m.ensure_embeddings(&sub).await;

    // 建边(线性扫走 judge_relevant);此时无边可跟随,judge_edge 不被调用。
    m.retrieve_exact("TAGA build", &sub).await;
    assert_eq!(ec.load(Ordering::SeqCst), 0, "建边阶段(无边可随)不调 judge_edge");

    // 切档B。
    m.set_pointer_match_mode(PointerMatchMode::LlmJudge);

    // judge_edge 判跳(query 含 "jump")→ 经边召回 target。
    let yes = m.retrieve_exact("TAGA jump now", &sub).await;
    assert!(ec.load(Ordering::SeqCst) >= 1, "档B 的跟随判定走 judge_edge");
    assert!(yes.iter().any(|h| h.source == target), "judge_edge 判跳 → 经边召回 target");

    // judge_edge 判不跳(query 无 "jump")→ mesh 不跳;target 已 Deep,线性扫跳过 → 召不回。
    let before = ec.load(Ordering::SeqCst);
    let no = m.retrieve_exact("TAGA stay", &sub).await;
    assert!(ec.load(Ordering::SeqCst) > before, "仍走 judge_edge 决策");
    assert!(!no.iter().any(|h| h.source == target), "judge_edge 判不跳 → target 召不回");
}

/// 配套②:检索记成 mind_search 瞬态一等事件(ring-only,不落时间线节点),内部状态块经受控 kind 表渲染。
#[tokio::test]
async fn retrieve_perceives_mind_search_ring_only() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MockSub { keyword: "Rust".into(), judge_calls: calls.clone() };
    let mut m = Memory::new();
    m.ingest_conversation("Rust 是系统语言");
    m.ensure_embeddings(&sub).await;
    let before_events = m.internal_event_count();
    let before_nodes = m.timeline().len();

    let _ = m.retrieve("Rust", &sub).await;
    assert_eq!(m.internal_event_count(), before_events + 1, "检索记一条 mind_search 瞬态事件");
    assert_eq!(m.timeline().len(), before_nodes, "ring-only:不落时间线节点(上下文不随每回合检索增长)");

    // 内部状态块用受控 kind 表把 mind_search 渲染成可读标签「检索」。
    let block = m.render_internal_state("zh").unwrap();
    assert!(block.contains("检索"), "内部状态块经受控表渲染 mind_search 为「检索」");

    // 再检索一次:仍 ring-only,时间线节点数不变(印证稳定性)。
    let _ = m.retrieve("Rust", &sub).await;
    assert_eq!(m.timeline().len(), before_nodes, "多次检索时间线仍不增长");
}

/// 阶段6:sleep 复核反 K —— 老化掉旧反 K,让被一次旧误判长期挡住的边解封。
#[tokio::test]
async fn review_negatives_ages_out_stale_negative() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MeshSub { judge_calls: calls.clone() };
    let mut m = Memory::new();
    let entry = m.ingest_conversation("TAGA 入口提问");
    let target = m.ingest_conversation("REL 目标答案");
    m.ensure_embeddings(&sub).await;
    m.retrieve_exact("TAGA 建边", &sub).await; // 建边 entry→target

    // 注入一个反 K(last_used ≈ 真实 now)。
    let qv = sub.embed("TAGA").await;
    m.record_negative_edge(&entry, &target, "曾误跳的 query", &qv);
    assert!(
        m.edges_of(&entry).iter().any(|p| p.target == target && p.negatives.len() == 1),
        "已记一个反 K"
    );

    // 用远未来 now 复核(now 远大于真实 now ms;max_age 小)→ 旧反 K 被老化移除。
    let changed = m.review_negatives(10_000_000_000_000, 1000, 64);
    assert!(changed >= 1, "老化掉旧反 K");
    assert!(
        m.edges_of(&entry).iter().any(|p| p.target == target && p.negatives.is_empty()),
        "反 K 已清,边解封(下次正常重判)"
    );
}

/// 阶段5:PointerConfig 默认值锚定各阶段实测默认 + 匹配档字符串互转(Settings 透传用)。
#[test]
fn pointer_config_defaults_and_mode_roundtrip() {
    let c = PointerConfig::default();
    assert_eq!(c.match_mode, PointerMatchMode::WeightedCosine);
    assert!((c.follow_threshold - 0.80).abs() < 1e-6);
    assert!((c.neg_block_threshold - 0.90).abs() < 1e-6);
    assert!((c.k_merge_threshold - 0.93).abs() < 1e-6);
    assert!((c.weight_gain - 0.30).abs() < 1e-6);
    assert_eq!(c.k_cap, 8);
    assert!(c.force_judge_on_cosine_hit, "默认 force_judge=true(≈现状:档A 命中仍 judge 确认)");
    assert_eq!(PointerMatchMode::from_setting("llm_judge"), PointerMatchMode::LlmJudge);
    assert_eq!(PointerMatchMode::from_setting("weighted_cosine"), PointerMatchMode::WeightedCosine);
    assert_eq!(PointerMatchMode::from_setting("garbage"), PointerMatchMode::WeightedCosine, "未知值回退档A");
    assert_eq!(PointerMatchMode::LlmJudge.as_setting(), "llm_judge");
    assert_eq!(PointerMatchMode::WeightedCosine.as_setting(), "weighted_cosine");
}

// ===================== 阶段4:强制跳转指针 / 二级索引(远处拉近) =====================

/// 强制跳转(历史引用):用户在入口位置钉下一段历史 → 导航到入口即无条件召回它,
/// 即便那段历史与 query 既不向量相似、judge 也不认为相关(用户断言压过启发式)。
#[tokio::test]
async fn forced_jump_recalls_user_referenced_history() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MeshSub { judge_calls: calls.clone() };
    let mut m = Memory::new();
    let entry = m.ingest_conversation("TAGA 当前提问"); // [1,0,0]
    let history = m.ingest_conversation("TAGB 无关旧事"); // [0,1,0],不含 REL → judge 不认
    m.ensure_embeddings(&sub).await;

    // 不钉时:query 走 TAGA 入口,既无边也无强制跳转,history 与之无关 → 召不回。
    let before = m.retrieve_exact("TAGA 找", &sub).await;
    assert!(!before.iter().any(|h| h.source == history), "未钉前召不回无关历史");

    // 用户引用历史:在 entry 位置钉一条强制跳转 → history。
    let src = m.pin_history_reference(Some(&entry), &history);
    assert_eq!(src.as_deref(), Some(entry.as_str()));
    assert_eq!(m.forced_jump_count(), 1);

    // 再查:导航入口落到 entry → 必跳,无条件召回 history。
    let after = m.retrieve_exact("TAGA 再找", &sub).await;
    assert!(after.iter().any(|h| h.source == history), "钉后强制跳转应召回用户引用的历史");
    // 自指 / 不存在目标被拒。
    assert!(m.pin_history_reference(Some(&entry), &entry).is_none(), "自指无效");
    assert!(m.pin_history_reference(None, "不存在的节点").is_none(), "目标须在线");
}

/// 强制跳转持久:用户断言的历史引用应落库、重启后仍在(与衰减的语义边不同)。
#[tokio::test]
async fn forced_jumps_survive_reopen() {
    use crate::store::Store;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.redb");
    let sub = MeshSub { judge_calls: Arc::new(AtomicUsize::new(0)) };
    let history_id;
    let entry_id;
    {
        let mut m = Memory::open(Store::open(&path).unwrap(), dir.path());
        entry_id = m.ingest_conversation("TAGA 当前");
        history_id = m.ingest_conversation("TAGB 旧事");
        m.ensure_embeddings(&sub).await;
        m.pin_history_reference(Some(&entry_id), &history_id);
        assert_eq!(m.forced_jump_count(), 1);
    }
    let m2 = Memory::open(Store::open(&path).unwrap(), dir.path());
    assert_eq!(m2.forced_jump_count(), 1, "强制跳转应持久化并在重启后恢复");
    let after = {
        let mut m2 = m2;
        m2.retrieve_exact("TAGA 再来", &sub).await
    };
    assert!(after.iter().any(|h| h.source == history_id), "重启后强制跳转仍召回历史");
}

/// 完整保真展示记录:按项目存富消息 JSON,重启(重开 Store)后原样取回;未知项目取回 None。
/// 这是"用的时候什么样、下次启动还得什么样"的持久化背书(区别于时间线/AI 记忆)。
#[test]
fn transcript_survives_reopen_and_is_per_project() {
    use crate::store::Store;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.redb");
    let rich = r#"[{"id":1,"role":"assistant","content":"改了 a.ejs","thinking":"想了想","meta":{"inputTokens":10,"outputTokens":5,"model":"x"},"ts":1}]"#;
    {
        let m = Memory::open(Store::open(&path).unwrap(), dir.path());
        m.save_transcript("projA", rich);
        assert_eq!(m.load_transcript("projA").as_deref(), Some(rich), "存进即取回(含思考/工具/meta原文)");
        assert_eq!(m.load_transcript("projB"), None, "未存过的项目取回 None → 调用方回退时间线");
    }
    // 重开 = 模拟重启:记录仍在、仍按项目隔离。
    let m2 = Memory::open(Store::open(&path).unwrap(), dir.path());
    assert_eq!(m2.load_transcript("projA").as_deref(), Some(rich), "重启后完整记录原样还原");
    assert_eq!(m2.load_transcript("projB"), None);
}

/// 二级索引「远处拉近」:一个热的远端 target,即便后续 query 的 RAG 入口已漂离、
/// 当前入口的 topic 边够不着它,也能凭锚到前沿的二级锚点被召回。
#[tokio::test]
async fn secondary_index_pulls_distant_hot_node() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MeshSub { judge_calls: calls.clone() };
    let mut m = Memory::new();
    let entry = m.ingest_conversation("TAGA 入口"); // pos0 [1,0,0]
    let target = m.ingest_conversation("TAGB REL 远端答案"); // pos1 [0,1,0] REL
    m.ensure_embeddings(&sub).await;

    // ① 近端时线性扫建边 entry→target(topic=[1,0,0])。
    m.retrieve_exact("TAGA 找答案", &sub).await;
    assert_eq!(m.pointer_count(), 1, "应建 entry→target 边");

    // ② 推进对话:塞 ≥2 窗口(2*WINDOW)的填充节点,把 target 推到远处。
    for k in 0..(2 * WINDOW + 2) {
        m.ingest_conversation(format!("decoy {k}")); // [0,0,1] 非 REL
    }
    m.ensure_embeddings(&sub).await;

    // ③ 同类 query 经 entry→target 边走 mesh 命中 target(远端)→ 注册二级锚点。
    let h3 = m.retrieve_exact("TAGA 再找", &sub).await;
    assert!(h3.iter().any(|h| h.source == target), "mesh 应召回远端 target");
    assert!(m.secondary_index_count() >= 1, "命中远端 target 应建二级锚点");

    // ④ 换一个落在填充区的 query:RAG 入口是 decoy(无边指向 target),
    //    单靠 mesh topic 边够不着 target;二级索引把它拉近前沿 → 仍召回。
    let h4 = m.retrieve_exact("decoy 别的事", &sub).await;
    assert!(
        h4.iter().any(|h| h.source == target),
        "入口漂离后,二级索引应把远端热 target 拉近召回"
    );
    let _ = entry;
}

/// 小息清二级索引(工作集)但保留强制跳转(长期、用户断言)。
#[tokio::test]
async fn nap_clears_secondary_keeps_forced_jumps() {
    let sub = MeshSub { judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    let entry = m.ingest_conversation("TAGA 入口");
    let target = m.ingest_conversation("TAGB REL 远端");
    m.ensure_embeddings(&sub).await;
    m.retrieve_exact("TAGA 找", &sub).await;
    for k in 0..(2 * WINDOW + 2) {
        m.ingest_conversation(format!("d {k}"));
    }
    m.ensure_embeddings(&sub).await;
    m.retrieve_exact("TAGA 再找", &sub).await; // 建二级锚点
    m.pin_history_reference(Some(&entry), &target); // 钉强制跳转
    assert!(m.secondary_index_count() >= 1);
    assert_eq!(m.forced_jump_count(), 1);

    m.nap();
    assert_eq!(m.secondary_index_count(), 0, "小息清二级索引(工作集)");
    assert_eq!(m.forced_jump_count(), 1, "强制跳转是长期记忆,小息保留");
}

#[tokio::test]
async fn pointers_survive_reopen() {
    use crate::store::Store;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.redb");
    let sub = MeshSub { judge_calls: Arc::new(AtomicUsize::new(0)) };
    {
        let mut m = Memory::open(Store::open(&path).unwrap(), dir.path());
        m.ingest_conversation("TAGA 入口提问");
        m.ingest_conversation("REL 目标答案");
        m.ensure_embeddings(&sub).await;
        m.retrieve_exact("TAGA 找答案", &sub).await; // 建边并落库
        assert_eq!(m.pointer_count(), 1);
    }
    // 重新打开:指针网应从库里恢复。
    let m2 = Memory::open(Store::open(&path).unwrap(), dir.path());
    assert_eq!(m2.pointer_count(), 1, "指针边应持久化并在重启后恢复");
}

/// 版本可变的 embedder:embed 产物随 version 改变,模拟"换 embedder = 向量空间变"。
struct VersionedSub {
    version: String,
    tag: f32, // 不同 version 给不同向量,验证重嵌确实换了值
}
#[async_trait]
impl Subconscious for VersionedSub {
    async fn embed(&self, _text: &str) -> Vec<f32> {
        vec![self.tag, 0.0]
    }
    fn embedding_version(&self) -> String {
        self.version.clone()
    }
    async fn judge_relevant(&self, _query: &str, _candidates: &[String]) -> Vec<usize> {
        Vec::new()
    }
}

#[tokio::test]
async fn reembeds_when_version_changes() {
    let mut m = Memory::new();
    let id = m.ingest_conversation("一条记忆");

    // 用 embedder A 向量化。
    let a = VersionedSub { version: "A".into(), tag: 1.0 };
    m.ensure_embeddings(&a).await;
    let n = m.timeline().get(&id).unwrap();
    assert_eq!(n.embedding, vec![1.0, 0.0]);
    assert_eq!(n.embedding_version, "A");

    // 同版本再跑:不应重嵌(version 相符)。把 tag 改了也不应生效。
    let a2 = VersionedSub { version: "A".into(), tag: 9.0 };
    m.ensure_embeddings(&a2).await;
    assert_eq!(m.timeline().get(&id).unwrap().embedding, vec![1.0, 0.0], "同版本不重嵌");

    // 换 embedder B(向量空间变)→ 整体重嵌,向量与版本都更新。
    let b = VersionedSub { version: "B".into(), tag: 2.0 };
    m.ensure_embeddings(&b).await;
    let n = m.timeline().get(&id).unwrap();
    assert_eq!(n.embedding, vec![2.0, 0.0], "换版本应重嵌出新向量");
    assert_eq!(n.embedding_version, "B");
}

#[tokio::test]
async fn conclusion_storage() {
    let mut m = Memory::new();
    m.ingest_conclusion(Conclusion::experience("op", "exp", "src"));
    assert_eq!(m.conclusions().len(), 1);
}

#[tokio::test]
async fn write_through_survives_reopen() {
    use crate::store::Store;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.redb");
    let sub = MockSub { keyword: "Rust".into(), judge_calls: Arc::new(AtomicUsize::new(0)) };
    {
        let mut m = Memory::open(Store::open(&path).unwrap(), dir.path());
        m.ingest_conversation("Rust 很好");
        m.ingest_conclusion(Conclusion::experience("op", "exp", "src"));
        m.ensure_embeddings(&sub).await; // 向量也应落库
    }
    // 重新打开:节点 + 结论 + 向量都还在,且时间线顺序还原(content 惰性回盘取)。
    let m2 = Memory::open(Store::open(&path).unwrap(), dir.path());
    assert_eq!(m2.timeline().len(), 1);
    let first = m2.timeline().metas()[0].id.clone();
    assert_eq!(m2.timeline().content(&first).unwrap(), "Rust 很好");
    assert!(!m2.timeline().get(&first).unwrap().embedding.is_empty(), "向量应已持久化");
    assert_eq!(m2.conclusions().len(), 1);
}

// ===================== P5:碎片 / 做梦 / 疲劳 / 睡眠 / 小息 =====================

/// 做梦专用 mock:嵌入按 TAG 词(TAGA/MID/其余),相关性 = 含 "REL"。
struct DreamSub {
    judge_calls: Arc<AtomicUsize>,
}
#[async_trait]
impl Subconscious for DreamSub {
    async fn embed(&self, text: &str) -> Vec<f32> {
        if text.contains("TAGA") {
            vec![1.0, 0.0, 0.0]
        } else if text.contains("MID") {
            vec![0.0, 1.0, 0.0]
        } else {
            vec![0.0, 0.0, 1.0]
        }
    }
    async fn judge_relevant(&self, _query: &str, candidates: &[String]) -> Vec<usize> {
        self.judge_calls.fetch_add(1, Ordering::SeqCst);
        candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.contains("REL"))
            .map(|(i, _)| i)
            .collect()
    }
}

/// 端到端:mesh 跳转记一笔碎片债;做梦复查跳过的中段,发现被线性扫漏掉的相关节点,
/// 补一条入口→该节点的边(网长密),还清碎片。
///
/// 构造"被漏的中段节点":线性扫只取近端 SCAN_BATCH(8)个非 Deep 节点。把待漏节点 M 压到
/// 第 8 个之外(中间塞 8 个 decoy),建跨度边 E→T 时线性扫够不到 M,M 保持 None;
/// 之后同类 query 经 E→T 边走快车道直达 T(跳过 M)→ 记碎片;做梦复查中段才捞回 M。
#[tokio::test]
async fn mesh_hop_records_and_dream_discovers_missed_gap_node() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = DreamSub { judge_calls: calls.clone() };
    let mut m = Memory::new();
    let entry = m.ingest_conversation("TAGA 入口"); // pos0 [1,0,0]
    let missed = m.ingest_conversation("MID REL 被漏的中段答案"); // pos1 [0,1,0] 相关,将被线性扫漏
    for k in 0..8 {
        m.ingest_conversation(format!("decoy {k}")); // pos2..9 [0,0,1] 不相关,占满近端扫描窗口
    }
    let target = m.ingest_conversation("TGT REL 目标答案"); // pos10 [0,0,1] 相关,近端
    m.ensure_embeddings(&sub).await;

    // 本测试演示"线性扫窗口有界 → 漏掉远处 M → 靠做梦捞回"。把线性扫上限收到一批(8)复现该场景
    // (默认 256 会直接线性扫到 M,那是 2026-06-15 核心修复带来的更好路径,但不是本测试要验的 dream 机制)。
    m.set_retrieval_config(RetrievalConfig { scan_max: 8, ..RetrievalConfig::default() });

    // query1:无边 → 线性扫近端 8 个(T + 8 decoy 里的 7 个),够不到 M。命中 T → 建边 E→T;M 仍 None。
    let h1 = m.retrieve_exact("TAGA 找答案", &sub).await;
    assert!(h1.iter().any(|h| h.source == target), "线性扫应命中近端 target");
    assert_eq!(m.timeline().get(&missed).unwrap().stain, Stain::None, "M 在扫描窗口外,应仍 None");
    assert_eq!(m.fragment_count(), 0, "线性扫不记碎片");

    // query2:经 E→T 边走快车道直达 T,跳过中段 → 记一笔碎片债 (E,T)。
    let h2 = m.retrieve_exact("TAGA 再找", &sub).await;
    assert!(h2.iter().any(|h| h.source == target), "mesh 跳转应召回 target");
    assert_eq!(m.fragment_count(), 1, "mesh 跳过中段应记一笔碎片");

    // 做梦:复查 E..T 中段的非 Deep 节点(含 M),judge 出 M 相关 → 补边 E→M,还清碎片。
    let report = m.dream_once(&sub).await;
    assert_eq!(report.processed, 1);
    assert_eq!(report.discoveries, 1, "应捞回被漏的中段相关节点 M");
    assert!(report.drained, "唯一一笔碎片已还清");
    assert_eq!(m.fragment_count(), 0);
    assert!(m.pointer_neighbors(&entry).contains(&missed), "应补出 E→M 边");
    assert_eq!(m.timeline().get(&missed).unwrap().stain, Stain::Deep, "中段被仔细复查 = Deep");
    let _ = entry;
}

#[tokio::test]
async fn dream_on_empty_ledger_is_noop() {
    let sub = DreamSub { judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    let r = m.dream_once(&sub).await;
    assert_eq!(r.processed, 0);
    assert!(r.drained);
}

#[tokio::test]
async fn fatigue_zero_when_fresh_rises_with_debt() {
    // 全新系统:没发生过检索活动、无碎片 → 不累。
    let fresh = Memory::new();
    assert_eq!(fresh.fatigue(), 0.0, "新系统疲劳为 0");

    // 攒出一笔碎片债后疲劳应上升(碎片占比贡献)。
    let sub = DreamSub { judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    m.ingest_conversation("TAGA 入口");
    m.ingest_conversation("TGT REL 目标");
    m.ensure_embeddings(&sub).await;
    m.retrieve_exact("TAGA 找", &sub).await; // 建边
    m.retrieve_exact("TAGA 再找", &sub).await; // mesh 跳 → 记碎片
    assert!(m.fragment_count() >= 1);
    assert!(m.fatigue() > 0.0, "有碎片债应有疲劳");
}

/// 疲劳权重可设:三权重全置 0 → 同一有债场景疲劳归 0,证明 fatigue() 读旋钮而非常量。
#[tokio::test]
async fn fatigue_weights_are_configurable() {
    let sub = DreamSub { judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    m.ingest_conversation("TAGA 入口");
    m.ingest_conversation("TGT REL 目标");
    m.ensure_embeddings(&sub).await;
    m.retrieve_exact("TAGA 找", &sub).await;
    m.retrieve_exact("TAGA 再找", &sub).await;
    assert!(m.fragment_count() >= 1);
    assert!(m.fatigue() > 0.0, "默认权重下有碎片债应有疲劳");

    m.set_fatigue_config(FatigueConfig { w_hitrate: 0.0, w_evict: 0.0, w_fragment: 0.0 });
    assert_eq!(m.fatigue_config().w_fragment, 0.0, "旋钮已落");
    assert_eq!(m.fatigue(), 0.0, "权重全 0 → 疲劳 0(旋钮即时生效)");
}

#[tokio::test]
async fn nap_clears_working_set_keeps_long_term() {
    let sub = DreamSub { judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    m.ingest_conversation("TAGA 入口");
    m.ingest_conversation("TGT REL 目标");
    m.ingest_conclusion(Conclusion::experience("op", "exp", "s"));
    m.ensure_embeddings(&sub).await;
    m.retrieve_exact("TAGA 找", &sub).await;
    m.retrieve_exact("TAGA 再找", &sub).await; // 记碎片
    m.assemble_context("TAGA", &sub).await; // 工作集有内容
    let nodes = m.timeline().len();
    let ptrs = m.pointer_count();
    let concl = m.conclusions().len();

    m.nap(); // 擦黑板

    assert_eq!(m.fragment_count(), 0, "小息清碎片台账");
    assert_eq!(m.timeline().len(), nodes, "时间线(长期记忆)保留");
    assert_eq!(m.pointer_count(), ptrs, "磁盘指针图保留");
    assert_eq!(m.conclusions().len(), concl, "结论保留");
}

#[tokio::test]
async fn sleep_dreams_to_drain_fragments() {
    let sub = DreamSub { judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    m.ingest_conversation("TAGA 入口");
    m.ingest_conversation("TGT REL 目标");
    m.ensure_embeddings(&sub).await;
    m.retrieve_exact("TAGA 找", &sub).await;
    m.retrieve_exact("TAGA 再找", &sub).await; // 记一笔碎片
    assert!(m.fragment_count() >= 1);
    let r = m.sleep(&sub, 5).await;
    assert!(r.dreams >= 1, "睡眠期应至少做梦还一笔碎片");
}

#[tokio::test]
async fn timeline_order_restored_by_created_at() {
    use crate::store::Store;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.redb");
    {
        let mut m = Memory::open(Store::open(&path).unwrap(), dir.path());
        m.ingest_conversation("最早");
        std::thread::sleep(std::time::Duration::from_millis(2));
        m.ingest_conversation("最新");
    }
    let m2 = Memory::open(Store::open(&path).unwrap(), dir.path());
    // metas 末尾 = 最近:应是"最新"(content 惰性回盘取)。
    let last = m2.timeline().metas().last().unwrap().id.clone();
    assert_eq!(m2.timeline().content(&last).unwrap(), "最新");
}

#[test]
fn perceive_rings_renders_and_persists_to_timeline() {
    let mut m = Memory::new();
    assert!(m.render_internal_state("zh").is_none(), "无事件不渲染");
    m.perceive("LLM调用失败", "响应沉默超时");
    m.perceive("工具失败", "shell: 退出码 1");
    assert_eq!(m.internal_event_count(), 2);
    let block = m.render_internal_state("zh").expect("有事件应渲染");
    assert!(block.contains("内部状态"), "特殊句式标明内部状态");
    assert!(block.contains("响应沉默超时") && block.contains("shell: 退出码 1"));
    assert!(block.contains("时间 "), "每条带时间戳");
    // AI 所感知的一切都能被索引:感知也进时间线(可检索)。
    assert_eq!(m.timeline().len(), 2);
}

#[test]
fn render_internal_since_is_append_only_by_cursor() {
    // ★append-only 缓存修复★:游标只取新事件,已注入的不再重渲(byte-stable prefix)。
    let mut m = Memory::new();
    let start = m.internal_seq();
    assert_eq!(start, 0);
    assert!(m.render_internal_since("zh", start).is_none(), "无新事件返回 None");
    m.perceive_transient(crate::node_kind::MIND_SEARCH, "检索:五子棋 命中3");
    let (block, cur) = m.render_internal_since("zh", start).expect("有新事件应渲染");
    assert!(block.contains("五子棋 命中3"));
    assert_eq!(cur, 1, "新游标 = 下一个待分配 seq");
    // 用新游标再取 → 无新事件(不重复注入已 append 的)。
    assert!(m.render_internal_since("zh", cur).is_none(), "已注入的不再重渲(append-only)");
    // 再来一条 → 只取这条新的(旧的不重出)。
    m.perceive_transient("工具失败", "shell 退出码 1");
    let (block2, cur2) = m.render_internal_since("zh", cur).expect("新事件");
    assert!(block2.contains("shell 退出码 1"), "只含新事件");
    assert!(!block2.contains("五子棋 命中3"), "旧事件不重复注入");
    assert_eq!(cur2, 2);
}

#[test]
fn render_artifact_since_append_only() {
    let mut m = Memory::new();
    let s = m.artifact_seq();
    m.perceive_artifact("main", "move", "(3,4)");
    let (block, cur) = m.render_artifact_since("zh", s).expect("有交互");
    assert!(block.contains("(3,4)"));
    assert!(m.render_artifact_since("zh", cur).is_none(), "append-only:已注入不重渲");
}

#[test]
fn internal_events_ring_is_bounded_but_timeline_keeps_all() {
    let mut m = Memory::new();
    for i in 0..(INTERNAL_EVENTS_CAP + 10) {
        m.perceive("测试", format!("事件 {i}"));
    }
    assert_eq!(m.internal_event_count(), INTERNAL_EVENTS_CAP, "瞬态环有界");
    assert_eq!(m.timeline().len(), INTERNAL_EVENTS_CAP + 10, "时间线全留,历史不丢");
}

/// 瞬态容量可设:把内部事件环 cap 调小,环按新 cap 有界(证明读旋钮而非常量)。
#[test]
fn transient_caps_shrink_internal_ring() {
    let mut m = Memory::new();
    m.set_transient_caps(TransientCapsConfig { internal_events_cap: 3, ..Default::default() });
    for i in 0..10 {
        m.perceive("测试", format!("事件 {i}"));
    }
    assert_eq!(m.transient_caps().internal_events_cap, 3, "旋钮已落");
    assert_eq!(m.internal_event_count(), 3, "内部事件环按旋钮 cap 有界");
}

/// 造物交互(流2):入独立造物瞬态环 —— 不落时间线、不进内部状态环(默认丢);
/// render_artifact_state 渲染;cap 可设且有界。
#[test]
fn perceive_artifact_ring_only_and_bounded() {
    let mut m = Memory::new();
    assert!(m.render_artifact_state("zh").is_none(), "无交互不渲染");
    m.perceive_artifact("main", "落子", "(3,4)");
    m.perceive_artifact("main", "提交", "{form}");
    assert_eq!(m.artifact_interaction_count(), 2);
    // 独立环:造物交互不进内部状态环、不落时间线。
    assert_eq!(m.internal_event_count(), 0, "造物交互不进内部状态环");
    assert_eq!(m.timeline().len(), 0, "造物交互默认不落时间线");
    let block = m.render_artifact_state("zh").expect("有交互应渲染");
    assert!(block.contains("造物交互"), "块头标明造物交互");
    assert!(block.contains("落子") && block.contains("(3,4)"));
    assert!(block.contains("造物"), "kind 渲染为造物标签");
    // cap 可设:调小到 1,环按新 cap 有界。
    m.set_transient_caps(TransientCapsConfig { artifact_interactions_cap: 1, ..Default::default() });
    assert_eq!(m.artifact_interaction_count(), 1, "造物环按旋钮 cap 截断");
    m.perceive_artifact("main", "再点", "x");
    assert_eq!(m.artifact_interaction_count(), 1, "持续有界");
}

/// ★造物回传值:宽松兜底 + 可感知截断(用户原则 2026-06-04:上报量 LLM 自己决定 + 自我感知)★
/// 正常游戏态(< 16384)原样进、AI 看到完整;超兜底才截断,且渲染里**如实告知 AI 被截断**(它据此精简)。
#[test]
fn perceive_artifact_value_huge_truncated_and_perceivable() {
    let mut m = Memory::new();
    // 正常棋盘态(几百字)原样、无截断标记。
    let board = format!("{{\"board\":[{}]}}", "0,".repeat(225));
    assert!(board.chars().count() < 16384);
    m.perceive_artifact("g", "move", &board);
    let block = m.render_artifact_state("zh").expect("有交互");
    assert!(block.contains("\"board\""), "正常状态原样可见");
    assert!(!block.contains("被截断"), "未超限不应有截断标记");
    // 超兜底(>16384)→ 截断 + 可感知标记(告知原始长度 + 怎么办)。
    m.perceive_artifact("g", "move", &"x".repeat(20000));
    let block2 = m.render_artifact_state("zh").expect("有交互");
    assert!(block2.contains("被截断"), "超限应如实告知 AI 被截断(自我感知原则)");
    assert!(block2.contains("20000"), "告知原始长度,AI 才知道丢了多少");
    assert!(block2.chars().filter(|c| *c == 'x').count() <= 16384, "确实截到上限(没塞完整 20000)");
}

/// 造物结论(主动留):与默认丢的 perceive_artifact 不同 —— 永久落时间线、可检索、kind=artifact。
#[test]
fn perceive_artifact_conclusion_persists_to_timeline() {
    let mut m = Memory::new();
    m.perceive_artifact_conclusion("和用户约定毕业前一起看樱花");
    // 入内部状态瞬态环 + 落时间线(永久),不进默认丢的造物交互环。
    assert_eq!(m.internal_event_count(), 1, "结论入内部状态环");
    assert_eq!(m.artifact_interaction_count(), 0, "结论不进默认丢的造物交互环");
    assert_eq!(m.timeline().len(), 1, "结论永久落时间线(可检索)");
    let block = m.render_internal_state("zh").unwrap();
    assert!(block.contains("樱花") && block.contains("造物"));
}

// ============================================================================
// 大面积压测(2026-06-15 L2 核心修复)
// 验证:新记忆即时可检索靠 L2 渐进扫(不靠嵌入)/ 染色分级护住未嵌入记忆 /
//       有嵌入节点染 Deep 加速 / scan_max 有界 / ring=64K 默认。
// ============================================================================

/// 压测①:被一堆无关记忆埋住的"契约"旧记忆,未嵌入,仍能被 L2 渐进扫捞到(复现 dream-board 病根)。
/// AI 某回合立契约("用 express-ejs-layouts"),之后写一堆文件把它埋了,idle 没跑(未嵌入)。
/// 旧实现只扫最近 8 个 → 契约被埋检索不到 → 漏接;新实现渐进扫到它。
#[tokio::test]
async fn stress_buried_contract_retrievable_via_l2_without_embedding() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MockSub { keyword: "ZZZ_无此词".into(), judge_calls: calls.clone() };
    let mut m = Memory::new();
    let contract = m.ingest_conversation("契约 express-ejs-layouts 必须在 server.js 接上 app.use");
    for k in 0..60 {
        m.ingest_conversation(format!("写了视图文件 view_{k}.ejs"));
    }
    // 不嵌入(连续工作、idle 没跑)→ 索引空 → RAG 必空 → 下沉 L2 线性主干。
    let (hits, layer) = m.retrieve("express-ejs-layouts", &sub).await;
    assert_eq!(layer, Layer::Exact, "未嵌入 → RAG 空 → 下沉 L2");
    assert!(
        hits.iter().any(|h| h.source == contract),
        "L2 渐进扫应捞到被 60 条无关记忆埋住的契约(旧 8 格窗口必漏)"
    );
}

/// 压测②:染色分级——未嵌入节点被一次不相关扫描后只染 Light(非 Deep),后续别的 query 仍能线性扫到。
/// 旧病根:扫过秒染 Deep → 下次 filter(!=Deep) 永久跳过 → 未嵌入新记忆被埋。
#[tokio::test]
async fn stress_unembedded_stays_scannable_after_irrelevant_scan() {
    let sub = MockSub { keyword: "ZZZ".into(), judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    let answer = m.ingest_conversation("数据库连接池大小设为 20");
    for k in 0..5 {
        m.ingest_conversation(format!("无关闲聊 {k}"));
    }
    // query1:与 answer 不相关的检索,会线性扫到 answer 但判不相关。旧实现此处染 Deep(从此藏起)。
    let _ = m.retrieve("天气怎么样", &sub).await;
    assert_ne!(
        m.timeline().get(&answer).unwrap().stain,
        Stain::Deep,
        "未嵌入节点扫过应是 Light 不是 Deep(保持线性可扫)"
    );
    // query2:相关检索,应仍能线性扫到 answer(旧实现这里因 Deep 被跳过而漏)。
    let (hits, layer) = m.retrieve("连接池", &sub).await;
    assert_eq!(layer, Layer::Exact);
    assert!(
        hits.iter().any(|h| h.source == answer),
        "未嵌入记忆被不相关扫描过后,仍可被后续相关 query 检索到"
    );
}

/// 压测③:有嵌入的节点被线性扫过 → 染 Deep(索引这条退路在,可安全跳过加速);染色加速仍生效。
#[tokio::test]
async fn stress_embedded_node_dyed_deep_after_linear_scan() {
    let sub = MockSub { keyword: "唯一标记XQ".into(), judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    let n = m.ingest_conversation("含 唯一标记XQ 的内容");
    m.ensure_embeddings(&sub).await; // 嵌入 → has_embedding=true
    // query 不含 keyword → 向量 [0,1] vs node [1,0] → cosine 0 → RAG 不命中 → 下沉精确层走线性扫。
    let _ = m.retrieve("别的话题", &sub).await;
    assert_eq!(
        m.timeline().get(&n).unwrap().stain,
        Stain::Deep,
        "有嵌入的节点线性扫过染 Deep(有索引退路,可安全跳过)"
    );
}

/// 压测④:大规模(1500 节点)连续工作未嵌入。一次完全不匹配的检索被 scan_max 限住,judge 有界、
/// 不扫遍 1500;而最近写的相关节点仍可检索到(基本假设)。
#[tokio::test]
async fn stress_large_scale_scan_bounded_and_recent_reachable() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MockSub { keyword: "ZZZ".into(), judge_calls: calls.clone() };
    let mut m = Memory::new();
    for k in 0..1500 {
        m.ingest_conversation(format!("日志条目 {k}"));
    }
    // 完全不匹配:L2 扫满 scan_max(256)即停,judge ≤ ceil(256/8)=32,绝不扫遍 1500。
    let (hits, _) = m.retrieve("绝不存在的词AAA", &sub).await;
    assert!(hits.is_empty(), "无匹配应空");
    let jc = calls.load(Ordering::SeqCst);
    assert!(jc <= 33, "scan_max 应把 judge 调用限在 ~32(实际 {jc}),不扫遍 1500 节点");
    // 最近写一条相关的:在最近端,渐进扫第一批就命中。
    let recent = m.ingest_conversation("最近的关键结论 KEY_RECENT_9931");
    let (hits2, _) = m.retrieve("KEY_RECENT_9931", &sub).await;
    assert!(
        hits2.iter().any(|h| h.source == recent),
        "最近写的相关记忆必可检索(刚产生=刚可检索的基本假设)"
    );
}

/// 压测⑤:连续多轮检索 + 染色累积——有嵌入节点扫过转 Deep,下轮线性扫被 filter 跳过(加速,judge 递减);
/// 系统在反复检索下不发散、近端始终可达。
#[tokio::test]
async fn stress_repeated_retrieval_dye_accelerates_embedded() {
    let calls = Arc::new(AtomicUsize::new(0));
    let sub = MockSub { keyword: "ZZZ".into(), judge_calls: calls.clone() };
    let mut m = Memory::new();
    for k in 0..40 {
        m.ingest_conversation(format!("普通条目 {k}"));
    }
    m.ensure_embeddings(&sub).await; // 全部嵌入 → 线性扫过即可安全 Deep
    // 第一轮不匹配检索:扫近端一批批,把扫到的(有嵌入)染 Deep。
    let _ = m.retrieve("不匹配BBB", &sub).await;
    let first = calls.load(Ordering::SeqCst);
    // 第二轮同样不匹配:上一轮染 Deep 的被 filter 跳过 → 这轮 judge 调用应不增多于上轮(染色加速)。
    let _ = m.retrieve("不匹配CCC", &sub).await;
    let second = calls.load(Ordering::SeqCst) - first;
    assert!(second <= first, "染色应让重复检索的 judge 调用不增反减(实际 第一轮{first}/第二轮{second})");
}

/// 压测⑥:ring 默认预算 = 64K(2026-06-15 用户决策:最近原文常驻窗扩大,减小"近期契约滑出 ring → 割裂")。
#[test]
fn ring_default_is_64k() {
    assert_eq!(crate::DEFAULT_RING_CHARS, 64_000, "ring 默认应为 64K");
}

// ============================================================================
// 文档破碎化(2026-06-17,修 dream-board 投喂文档检索盲区)
// 验证:大文档入场标待破 / idle 破成小块 / 父节点退出索引 / 破碎后窄问由 RAG 第一层命中(稀释解除)/
//       结构化 kind 豁免破碎。
// ============================================================================

/// 词袋嵌入 mock:向量 = 各词在文本里出现次数(over 固定小词表)。
/// 用途:整篇文档含**所有**词 → 向量被稀释,窄问余弦低于命中阈(复现病根);
/// 单块只含一个词 → 向量聚焦,窄问余弦=1 命中(证明破碎治本)。judge_relevant 按子串(精确层兜底)。
struct BagEmbed {
    judge_calls: Arc<AtomicUsize>,
}
const BAG_VOCAB: &[&str] = &["idea-card", "db-accent", "better-sqlite3", "express"];
#[async_trait]
impl Subconscious for BagEmbed {
    async fn embed(&self, text: &str) -> Vec<f32> {
        let lower = text.to_lowercase();
        BAG_VOCAB.iter().map(|t| lower.matches(t).count() as f32).collect()
    }
    async fn judge_relevant(&self, query: &str, candidates: &[String]) -> Vec<usize> {
        self.judge_calls.fetch_add(1, Ordering::SeqCst);
        candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.contains(query))
            .map(|(i, _)| i)
            .collect()
    }
    // chunk_doc 用默认实现(按 target 贪心),无需 LLM。
}

/// ★dream-board 回归★:整篇技术栈文档破碎前窄问被稀释,破碎后由 RAG 第一层直接命中对应块。
#[tokio::test]
async fn chunking_fixes_rag_dilution_for_fed_document() {
    let sub = BagEmbed { judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    // 设小破碎阈,让这篇短"文档"也触发破碎(真机默认 1500;块目标≈阈,按行破)。
    m.set_retrieval_config(RetrievalConfig { chunk_min_chars: 30, ..RetrievalConfig::default() });
    let doc = "想法卡片的 class 统一叫 idea-card。\n主题色 token 叫 db-accent。\n\
               数据库用 better-sqlite3 存数据。\n服务端用 express 框架。";
    let parent = m.ingest_with_role(doc, "user");
    assert!(m.timeline().meta(&parent).unwrap().needs_chunk, "大文档入场即标待破");

    // 破碎前:待破节点不嵌(等破成块)→ 索引空 → 窄问只能下沉精确层(整篇当一个节点,RAG 无从聚焦)。
    m.ensure_embeddings(&sub).await;
    let (_pre, pre_layer) = m.retrieve("idea-card", &sub).await;
    assert_eq!(pre_layer, Layer::Exact, "破碎前窄问无法走 RAG(大节点未嵌/稀释),只能下沉");

    // 破碎:1 篇文档 → 多块,父节点标 chunked。
    let n = m.chunk_pending_batch(&sub, 0).await;
    assert_eq!(n, 1, "破了 1 篇文档");
    assert!(m.timeline().meta(&parent).unwrap().chunked, "父节点已标 chunked");
    assert!(!m.timeline().meta(&parent).unwrap().needs_chunk);
    assert!(m.timeline().len() >= 5, "父 + 至少 4 块(实际 {})", m.timeline().len());

    // 嵌入各块(父被跳过),建索引。
    m.ensure_embeddings(&sub).await;

    // 破碎后:窄问"idea-card"由 RAG 第一层直接命中含 .idea-card 的那块(稀释解除)。
    let (hits, layer) = m.retrieve("idea-card", &sub).await;
    assert_eq!(layer, Layer::Rag, "破碎后窄问由 RAG 第一层命中(不再被稀释逼下沉)");
    assert!(
        hits.iter().any(|h| h.content.contains("idea-card") && h.content.contains("class")),
        "命中含 .idea-card class 的那块"
    );
    // 已破父节点退出索引 → 绝不作为命中返回(只让小块上场)。
    assert!(!hits.iter().any(|h| h.source == parent), "已破父节点不再被检索命中");
    // 另一窄问:数据库 → better-sqlite3 那块(对照旧版凭空答 sql.js)。
    let (hits2, _) = m.retrieve("better-sqlite3", &sub).await;
    assert!(
        hits2.iter().any(|h| h.content.contains("better-sqlite3")),
        "命中 better-sqlite3 那块(治好旧版幻觉)"
    );
}

/// split_sentences:拼接精确还原原文(零丢字),在句末符/换行处断开。
#[test]
fn split_sentences_preserves_content_and_breaks_on_enders() {
    let doc = "第一句。第二句！第三句?\n第四行没句号";
    let s = split_sentences(doc);
    assert_eq!(s.concat(), doc, "拼接还原原文,零丢字零改写");
    assert!(s.len() >= 4, "句末符(。!?)与换行处断开(实际 {} 段)", s.len());
    assert!(s[0].ends_with('。'));
}

/// 结构化 kind(流程/技能/工具记忆)豁免破碎——它们靠整节点解析,破开会毁掉对应系统。
#[tokio::test]
async fn structured_kinds_are_exempt_from_chunking() {
    let sub = BagEmbed { judge_calls: Arc::new(AtomicUsize::new(0)) };
    let mut m = Memory::new();
    m.set_retrieval_config(RetrievalConfig { chunk_min_chars: 10, ..RetrievalConfig::default() });
    let long = "很长很长的配方正文需要整块解析".repeat(3); // 远超 10 字符阈
    let pid = m.ingest_process(long.clone());
    assert!(!m.timeline().meta(&pid).unwrap().needs_chunk, "流程节点豁免:不标待破");
    // 同样长度的普通 user 内容则标待破(对照,证明阈值确实触发)。
    let uid = m.ingest_with_role(long, "user");
    assert!(m.timeline().meta(&uid).unwrap().needs_chunk, "普通大节点标待破");
    let _ = m.chunk_pending_batch(&sub, 0).await;
    assert!(!m.timeline().meta(&pid).unwrap().chunked, "流程节点始终不被破碎");
}
