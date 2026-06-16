//! 常驻 IdleWorker —— app 生命周期级"潜意识"后台任务:idle 时睡眠维护 + 飞轮压缩。
//!
//! 设计依据(`设计/04-飞轮学习` + `设计/02` 维护节 + `系统架构/00` 数据流 ⑤):
//! 经验的**采集**在前台每步做(异步不挡主线);**维护**(都要调潜意识 LLM)只在 **idle** 时做,
//! 优先级 **Agent > 睡眠 > 飞轮**——前台一活动,后台立刻让位。
//!
//! 两件 idle 工作(本轮顺序即优先级:睡眠先于飞轮):
//!   A. **睡眠维护**(P5):疲劳/有碎片债时,做梦还债(复查 mesh 跳过的中段)+ 少量推演预热网。
//!      每步经仲裁器取 `Sleep` 档。
//!   B. **飞轮压缩**:把积累的经验聚类压成知识。每簇经仲裁器取 `Flywheel` 档。
//!
//! 优先级如何落地(见 `arbiter.rs` 头注):前台 `run_chat` 全程持 AppState 锁 + Agent 档,故
//! 前台对后台天然互斥;后台两件事都只在 idle 跑,且每个潜意识 LLM 调用前各 acquire 一次仲裁器,
//! 故能在调用间隙让位给更高优先级(新来的前台 / 睡眠优先于飞轮)。每步之间回头看是否还 idle,
//! 前台一回来或收到取消就立刻收手,留待下个 idle 周期。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

use growbox_learn::{Flywheel, Reasoner};

use crate::arbiter::{Arbiter, Priority};
use crate::cmds::SharedState;

/// idle 行为旋钮快照(推论9 数值全可设;由 `Settings` 透传)。idle_loop 每拍从 `st.settings`
/// 重读,故用户在设置里改完下一拍即生效,无需重启 worker。默认见 core `Settings` 的 idle_* 字段:
/// 静默 8 分钟 = 正常人一段交流的打字上限,超过基本是真走开了——此时才动后台,不打断思考/打字的用户。
#[derive(Clone, Copy)]
struct IdleConfig {
    /// 静默达此时长才算真 idle(用户离开)。
    threshold: Duration,
    /// 巡检间隔:每隔多久看一眼是否进入 idle。
    tick: Duration,
    /// 触发睡眠维护的疲劳阈值(0~1;低于此且无碎片债则不睡)。
    fatigue_threshold: f64,
    /// 一次 idle 激活内睡眠步数上限(做梦/推演合计,防独占)。
    max_sleep_steps: usize,
    /// 一次 idle 激活内推演次数上限(推演生新碎片留给做梦还,需有界)。
    max_rehearsals: usize,
}

impl IdleConfig {
    fn from_settings(s: &growbox_core::Settings) -> Self {
        IdleConfig {
            threshold: Duration::from_secs(s.idle_threshold_secs.max(1) as u64),
            tick: Duration::from_secs(s.idle_tick_secs.max(1) as u64),
            fatigue_threshold: s.idle_fatigue_threshold as f64,
            max_sleep_steps: s.idle_max_sleep_steps as usize,
            max_rehearsals: s.idle_max_rehearsals as usize,
        }
    }
}

/// IdleWorker 句柄,可取消。
pub struct IdleWorkerHandle {
    cancel: CancellationToken,
    _join: tokio::task::JoinHandle<()>,
}

impl IdleWorkerHandle {
    /// 启动 IdleWorker。持有 AppState 共享锁 + 前台活动时间戳 + 仲裁器 + AppHandle(抛事件给前端)。
    pub fn spawn(
        state: SharedState,
        last_activity: Arc<AtomicI64>,
        arbiter: Arc<Arbiter>,
        app: AppHandle,
    ) -> Self {
        let cancel = CancellationToken::new();
        let cancel_inner = cancel.clone();
        let join = tokio::spawn(async move {
            idle_loop(state, last_activity, arbiter, app, cancel_inner).await;
        });
        IdleWorkerHandle { cancel, _join: join }
    }

    /// 取消(断开连接 / app 退出时调)。
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

/// 距上次前台活动是否已静默够久(无锁,纯读时间戳)。阈值由当拍 `IdleConfig` 传入。
fn is_idle(last_activity: &AtomicI64, threshold: Duration) -> bool {
    let last = last_activity.load(Ordering::Relaxed);
    let now = growbox_core::now().timestamp_millis();
    now - last >= threshold.as_millis() as i64
}

/// IdleWorker 核心循环:定时巡检,真 idle 时先睡眠维护(还碎片债)再飞轮压缩。
async fn idle_loop(
    state: SharedState,
    last_activity: Arc<AtomicI64>,
    arbiter: Arc<Arbiter>,
    app: AppHandle,
    cancel: CancellationToken,
) {
    loop {
        // 每拍重读 idle 旋钮快照(运行时可在设置里改,下一拍即生效;短锁读 5 个值,后台低优先无碍)。
        let cfg = {
            let st = state.lock().await;
            IdleConfig::from_settings(&st.settings)
        };
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(cfg.tick) => {
                // 0a. 文档破碎化(排在补嵌之前,好让破出的小块紧接着被嵌):把入场标了 needs_chunk 的大文档
                //     按句破成小块。这是「修长文检索盲区」的生产入口——大节点单条向量稀释,破成小块各自向量才命中窄问。
                chunk_while_idle(&state, &last_activity, &arbiter, &cancel, cfg.tick).await;
                // 0b. 补嵌入(让语义检索可用):只要稍空闲(≥ 一个 tick)就增量补嵌,不必等 8 分钟维护阈。
                //    这是「修嵌入」的生产入口——此前 ensure_embeddings 只有测试在调,真机从不嵌入。
                embed_while_idle(&state, &last_activity, &arbiter, &cancel, cfg.tick).await;
                if is_idle(&last_activity, cfg.threshold) {
                    // A. 睡眠维护优先(Sleep 档),再 B. 飞轮压缩(Flywheel 档)。
                    sleep_while_idle(&state, &last_activity, &arbiter, &app, &cancel, &cfg).await;
                    digest_while_idle(&state, &last_activity, &arbiter, &app, &cancel, cfg.threshold).await;
                }
            }
        }
    }
}

/// 每批补嵌的节点数上限。有界 = 批间能让位前台/取消,避免一次嵌成百上千把一次 idle 卡死。
const EMBED_BATCH: usize = 32;

/// 每批破碎的文档数上限。破碎含 LLM 调用(判破点),故批更小;批间让位前台/取消。
const CHUNK_BATCH: usize = 4;

/// 文档破碎化(让长文检索可用):稍空闲就把入场标了 `needs_chunk` 的大文档按句破成小块,分批 +
/// 批间让位 + 可取消。取 `Flywheel` 档(优先级低于睡眠/前台)。排在 `embed_while_idle` 之前,
/// 好让破出的小块在同一段空闲里紧接着被嵌入、进索引。无待破 → 立即返回(零开销)。
async fn chunk_while_idle(
    state: &SharedState,
    last_activity: &AtomicI64,
    arbiter: &Arc<Arbiter>,
    cancel: &CancellationToken,
    min_idle: Duration,
) {
    if !is_idle(last_activity, min_idle) {
        return;
    }
    loop {
        if cancel.is_cancelled() || !is_idle(last_activity, min_idle) {
            break;
        }
        let _gate = arbiter.acquire(Priority::Flywheel).await;
        if cancel.is_cancelled() || !is_idle(last_activity, min_idle) {
            break;
        }
        let mut st = state.lock().await;
        let Some(bridge) = st.bridge.clone() else { break };
        let done = st.memory.chunk_pending_batch(bridge.as_ref(), CHUNK_BATCH).await;
        drop(st);
        if done == 0 {
            break; // 没有待破的了
        }
    }
}

/// 补嵌入(让语义检索可用):稍空闲(≥ `min_idle`)就增量补嵌待向量化节点,分批 + 批间让位 + 可取消。
/// 取 `Flywheel` 档(优先级低于睡眠/前台)。**这是「修嵌入」的生产入口**:此前 `ensure_embeddings`
/// 只有单测在调,真机从不嵌入 → RAG 第一层向量索引长期为空、检索每次直接跌到精确层。
async fn embed_while_idle(
    state: &SharedState,
    last_activity: &AtomicI64,
    arbiter: &Arc<Arbiter>,
    cancel: &CancellationToken,
    min_idle: Duration,
) {
    if !is_idle(last_activity, min_idle) {
        return;
    }
    loop {
        if cancel.is_cancelled() || !is_idle(last_activity, min_idle) {
            break;
        }
        // 取 Flywheel 档:睡眠/前台一来就排到它前面。
        let _gate = arbiter.acquire(Priority::Flywheel).await;
        if cancel.is_cancelled() || !is_idle(last_activity, min_idle) {
            break;
        }
        let mut st = state.lock().await;
        let Some(bridge) = st.bridge.clone() else { break };
        let done = st.memory.ensure_embeddings_batch(bridge.as_ref(), EMBED_BATCH).await;
        drop(st);
        if done == 0 {
            break; // 没有待补的了
        }
    }
}

/// 睡眠维护(P5):疲劳或有碎片债时,逐步做梦还债 + 少量推演预热;每步取 `Sleep` 档、可被打断。
/// 做梦/推演是 `&mut Memory` 的 async(LLM 调用在内),故每步持 AppState 锁完成一次调用——与前台
/// 回合持锁同理,且只在 idle 跑、步与步之间放锁回看,前台一回来立刻收手。
async fn sleep_while_idle(
    state: &SharedState,
    last_activity: &AtomicI64,
    arbiter: &Arc<Arbiter>,
    app: &AppHandle,
    cancel: &CancellationToken,
    cfg: &IdleConfig,
) {
    // 先快速判要不要睡(疲劳够高 / 有碎片债)。
    let need_sleep = {
        let st = state.lock().await;
        if st.bridge.is_none() {
            return;
        }
        st.memory.fragment_count() > 0 || st.memory.fatigue() >= cfg.fatigue_threshold
    };
    if !need_sleep {
        return;
    }

    let mut dreams = 0usize;
    let mut discoveries = 0usize;
    let mut rehearsals = 0usize;
    for _ in 0..cfg.max_sleep_steps {
        if cancel.is_cancelled() || !is_idle(last_activity, cfg.threshold) {
            break;
        }
        // 取 Sleep 档:前台 Agent / 在飞的飞轮调用一结束就轮到睡眠;睡眠优先于飞轮。
        let _gate = arbiter.acquire(Priority::Sleep).await;
        // 让位复查(取到槽期间前台可能已回来)。
        if cancel.is_cancelled() || !is_idle(last_activity, cfg.threshold) {
            break;
        }
        let mut st = state.lock().await;
        let Some(bridge) = st.bridge.clone() else { break };
        if st.memory.fragment_count() > 0 {
            // 做梦:还一笔碎片债(复查中段有无遗漏)。
            let r = st.memory.dream_once(bridge.as_ref()).await;
            dreams += r.processed;
            discoveries += r.discoveries;
        } else if rehearsals < cfg.max_rehearsals {
            // 网已干净:推演一次(预热网、可能生新碎片留给下一步做梦)。
            if st.memory.rehearse_once(bridge.as_ref()).await {
                rehearsals += 1;
            } else {
                break; // 无可推演 → 睡眠自然结束。
            }
        } else {
            break; // 债已还清、推演到上限 → 收手。
        }
        // 锁与槽在此各自 drop(st 出作用域、_gate 出循环体),下一步重取,给前台让位空隙。
    }

    if dreams + rehearsals > 0 {
        let _ = app.emit(
            "memory-event",
            serde_json::json!({
                "type": "slept",
                "dreams": dreams,
                "discoveries": discoveries,
                "rehearsals": rehearsals,
            }),
        );
    }
}

/// 进入 idle 工作态:取一次镜像,逐簇消化;只要还 idle 就把这批簇连着清完,
/// 前台一回来(last_activity 前移)或收到取消就立刻让位。一次激活清完一批,
/// 不是"一周期一簇"——产出与一次性全压等价,只是摊成多次可被打断的短调用。
async fn digest_while_idle(
    state: &SharedState,
    last_activity: &AtomicI64,
    arbiter: &Arc<Arbiter>,
    app: &AppHandle,
    cancel: &CancellationToken,
    idle_threshold: Duration,
) {
    // 第 1 拍:取镜像(极短锁)。克隆活跃经验 + 聚类 + 克隆潜意识桥,随即放锁。
    let (clusters, bridge) = {
        let st = state.lock().await;
        let Some(bridge) = st.bridge.clone() else { return };
        let experiences = Flywheel::active_experiences(&st.memory);
        if experiences.len() < 2 {
            return;
        }
        // 聚类不调 LLM,放在锁内便宜;成簇(≥2)的成员克隆带出锁。
        let clusters = Flywheel::new().clusters_of(&experiences);
        (clusters, bridge)
    };
    if clusters.is_empty() {
        return;
    }

    let fw = Flywheel::new();
    let mut produced = 0usize;
    let mut proposed = false; // ★S3★ 每次 idle 激活至多提议 1 条 skill(防膨胀)。
    for members in clusters {
        // 簇与簇之间让位:前台回来 / 取消 → 收手,余下留待下个 idle 周期。
        if cancel.is_cancelled() || !is_idle(last_activity, idle_threshold) {
            break;
        }
        // 第 2 拍:想(无锁,但取 Flywheel 档)。慢的 LLM 调用在这里;睡眠/前台来了就在档上排到它前面。
        let distilled = {
            let _gate = arbiter.acquire(Priority::Flywheel).await;
            fw.distill_cluster(&members, bridge.as_ref()).await
        };
        let Some((knowledge, superseded)) = distilled else {
            continue; // 这簇无共同模式(噪音),跳过。
        };
        // 第 3 拍:写回(极短锁)。append 新知识 + 幂等标记旧经验被取代(write-through 落盘)。
        {
            let mut st = state.lock().await;
            Flywheel::apply_distilled(&mut st.memory, knowledge, &superseded);
        }
        produced += 1;

        // ★S3 飞轮自学(设计/09)★:足够大的簇(反复成模式)额外**提议**一个可复用 skill,沿结晶谱
        // 「经验 → Skill」右推。每次激活至多 1 条;LLM 兼质量闸(多数返回 None);去重/容量/已拒在
        // try_add_skill_proposal 内把关。提议待用户在设置里采纳(→ crystallize_skill)或丢弃。
        if !proposed && members.len() >= MIN_CLUSTER_FOR_SKILL {
            let room = { state.lock().await.skill_proposals.has_room() };
            if room {
                let drafted = {
                    let _gate = arbiter.acquire(Priority::Flywheel).await;
                    bridge.propose_skill(&members).await
                };
                if let Some(p) = drafted {
                    let rationale =
                        members.iter().take(3).map(|c| c.operation.clone()).collect::<Vec<_>>().join("; ");
                    let mut st = state.lock().await;
                    if st.try_add_skill_proposal(&p.name, &p.trigger, &p.body, &rationale) {
                        proposed = true;
                        let _ = app.emit(
                            "memory-event",
                            serde_json::json!({ "type": "skill-proposed", "name": p.name }),
                        );
                    }
                }
            }
        }
    }

    if produced > 0 {
        // 后台事件:前端可据此低调提示"已在后台提炼 N 条知识",忽略也无妨。
        let _ = app.emit(
            "memory-event",
            serde_json::json!({ "type": "digested", "count": produced }),
        );
    }
}

/// ★S3★ 触发 skill 提议的最小簇规模:簇里同类经验 ≥ 此数才考虑提议(小簇多半太具体,不值得成 skill)。
const MIN_CLUSTER_FOR_SKILL: usize = 3;
