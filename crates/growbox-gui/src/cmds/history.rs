//! 历史/做梦/活的IDE命令:对话历史 + 引用上下文 + 会话列表 + 做梦睡眠小息 + 历史引用 + UI 操控回执。

use super::*;

#[tauri::command]
pub async fn get_chat_history(
    state: State<'_, SharedState>,
    before_ts: Option<String>,
    n: Option<usize>,
    _session_id: Option<String>,
) -> Result<Vec<Value>, String> {
    let st = state.lock().await;
    let limit = n.unwrap_or(30);
    let timeline = st.memory.timeline();
    let cutoff = before_ts.as_deref().unwrap_or("");
    // 软隔离 tag:有当前项目则只显示该项目 tag 的节点(方便看);无当前项目则不过滤。
    let current = st.current.as_deref();
    // 从新到旧遍历元信息(不触盘),命中 user|assistant 才惰性取 content,截止到 before_ts(不含)。
    let mut out: Vec<Value> = Vec::new();
    for meta in timeline.metas().iter().rev() {
        if out.len() >= limit { break; }
        let ts = meta.created_at.to_rfc3339();
        if !cutoff.is_empty() && ts.as_str() >= cutoff { continue; }
        if current.is_some() && meta.project_id.as_deref() != current { continue; }
        let content = timeline.content(&meta.id).unwrap_or_default();
        let role = effective_role(&meta.role, &content);
        if role != "user" && role != "assistant" { continue; }
        out.push(json!({ "role": role, "content": content, "ts": ts, "id": meta.id, "session_id": "timeline", "seq": 0 }));
    }
    out.reverse(); // 前端要旧→新
    Ok(out)
}

/// ★完整保真展示记录·存★:前端把"用户实际看到的"富消息(role/content[含内联工具卡]/thinking/meta/ts)
/// 整存,按当前项目落库。区别于时间线(=AI 记忆/检索源,只留 user/assistant 正文):这条记录是
/// "界面长什么样",重启原样还原(用户:用的时候什么样、下次启动还得什么样)。每回合结束前端调一次。
#[tauri::command]
pub async fn save_chat_transcript(state: State<'_, SharedState>, messages: Value) -> Result<(), String> {
    let st = state.lock().await;
    let pid = st.current.as_deref().unwrap_or("__global__");
    st.memory.save_transcript(pid, &messages.to_string());
    Ok(())
}

/// ★完整保真展示记录·取★:取当前项目保存的完整界面记录(富消息数组);无则 None →
/// 前端回退到 get_chat_history 的时间线派生(老项目/没存过的)。重启/切项目时前端先调它。
#[tauri::command]
pub async fn load_chat_transcript(state: State<'_, SharedState>) -> Result<Option<Value>, String> {
    let st = state.lock().await;
    let pid = st.current.as_deref().unwrap_or("__global__");
    Ok(st.memory.load_transcript(pid).and_then(|s| serde_json::from_str::<Value>(&s).ok()))
}

#[tauri::command]
pub async fn get_project_conversation_history(
    state: State<'_, SharedState>,
    before_ts: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<Value>, String> {
    let st = state.lock().await;
    let n = limit.unwrap_or(50);
    let timeline = st.memory.timeline();
    let cutoff = before_ts.as_deref().unwrap_or("");
    let current = st.current.as_deref(); // 软隔离 tag:有当前项目则只显示该项目节点。
    let mut out: Vec<Value> = Vec::new();
    // 从最新到最旧遍历元信息,只取 user/assistant 角色,截止到 before_ts;content 惰性取。
    for meta in timeline.metas().iter().rev() {
        if out.len() >= n { break; }
        let ts = meta.created_at.to_rfc3339();
        if !cutoff.is_empty() && ts.as_str() >= cutoff { continue; }
        if current.is_some() && meta.project_id.as_deref() != current { continue; }
        let content = timeline.content(&meta.id).unwrap_or_default();
        let role = effective_role(&meta.role, &content);
        if role != "user" && role != "assistant" { continue; }
        // session_id 用固定值(前端 HistoryDrawer 拖拽引用用),实际定位靠 ts。
        out.push(json!({ "role": role, "content": content, "ts": ts, "id": meta.id, "session_id": "timeline", "seq": 0 }));
    }
    out.reverse();
    Ok(out)
}

#[tauri::command]
pub async fn get_citation_context(
    state: State<'_, SharedState>,
    session_id: String,
    ts: String,
    radius: Option<usize>,
) -> Result<Value, String> {
    let st = state.lock().await;
    let r = radius.unwrap_or(5);
    let timeline = st.memory.timeline();
    let metas = timeline.metas();
    // 用 ts 在时间线里定位节点(元信息层,不触盘)。
    let pos = metas.iter().position(|m| m.created_at.to_rfc3339() == ts);
    let (cited, before, after) = match pos {
        None => (Value::Null, vec![], vec![]),
        Some(idx) => {
            let cited = meta_to_json(timeline, &metas[idx]);
            let start = idx.saturating_sub(r);
            let end = (idx + r + 1).min(metas.len());
            let before: Vec<_> = metas[start..idx].iter().map(|m| meta_to_json(timeline, m)).collect();
            let after: Vec<_> = metas[idx + 1..end].iter().map(|m| meta_to_json(timeline, m)).collect();
            (cited, before, after)
        }
    };
    Ok(json!({ "cited": cited, "before": before, "after": after, "session_id": session_id }))
}

/// 把节点元信息 + 惰性取的原文转成前端 ChatHistoryItem 格式。
fn meta_to_json(timeline: &growbox_memory::Timeline, m: &growbox_memory::NodeMeta) -> Value {
    let content = timeline.content(&m.id).unwrap_or_default();
    json!({ "role": effective_role(&m.role, &content), "content": content, "ts": m.created_at.to_rfc3339(), "id": m.id })
}

/// 旧节点没有 role 字段(默认 "system"),从内容前缀推断。
fn effective_role<'a>(role: &'a str, content: &str) -> &'a str {
    if role != "system" { return role; }
    // 旧格式兼容:根据 content 前缀推断。
    if content.starts_with("用户: ") { "user" }
    else if content.starts_with("助手: ") { "assistant" }
    else if content.starts_with("工具 ") { "tool" }
    else { "system" }
}

#[tauri::command]
pub async fn list_sessions(state: State<'_, SharedState>) -> Result<Vec<Value>, String> {
    let st = state.lock().await;
    let total = st.memory.timeline().len();
    let sid = st.session_id.clone().unwrap_or_else(|| "timeline".into());
    // 单会话:timeline 本身就是唯一的数据源,没有"多个 session"的概念。
    Ok(vec![json!({ "session_id": sid, "size_bytes": total })])
}

/// 手动做梦/睡眠一轮(P5):做梦还碎片债 + 少量推演,经仲裁器取 Sleep 档。
/// 用户在面板按"做梦"时调;前台正在用 LLM(Agent 档)则排在其后。
#[tauri::command]
pub async fn dream_start(state: State<'_, SharedState>) -> Result<Value, String> {
    let t0 = std::time::Instant::now();
    // 先取仲裁器 Sleep 档(出借,跨锁存活)。
    let arbiter = { state.lock().await.arbiter.clone() };
    let _gate = arbiter.acquire_owned(crate::arbiter::Priority::Sleep).await;

    let mut st = state.lock().await;
    let Some(bridge) = st.bridge.clone() else {
        return Err("尚未连接 LLM".into());
    };
    let before = st.memory.fragment_count();
    // 有界睡眠:做梦还债 + 推演(max_cycles 兜住"推演生债→做梦还债"循环)。
    let report = st.memory.sleep(bridge.as_ref(), 32).await;
    Ok(json!({
        "session_id": st.session_id,
        "total_fragments": before,
        "processed": report.dreams,
        "discoveries": report.discoveries,
        "rehearsals": report.rehearsals,
        "remaining": report.fragments_remaining,
        "duration_ms": t0.elapsed().as_millis() as u64,
        "is_complete": report.fragments_remaining == 0,
    }))
}

#[tauri::command]
pub async fn dream_status(state: State<'_, SharedState>) -> Result<Value, String> {
    let st = state.lock().await;
    Ok(json!({
        "session_id": st.session_id,
        "total_fragments": st.memory.fragment_count(),
        "processed": st.memory.fragments_cleared(),
        "total_discoveries": 0,
        "fatigue": st.memory.fatigue(),
        "duration_ms": 0,
    }))
}

/// 小息(Nap,用户手动)——"擦黑板,不格式化硬盘":清当前对话工作集 + 三级缓存 + 碎片台账,
/// 保留长期记忆(时间线/结论/指针图/索引)。见 `Memory::nap`。
#[tauri::command]
pub async fn nap(state: State<'_, SharedState>) -> Result<(), String> {
    state.lock().await.memory.nap();
    Ok(())
}

/// ★自驱续跑专用★:主动跑一次飞轮消化(把积压的逐步经验聚类→蒸馏成知识),不必等 8 分钟 idle。
/// 持续自驱时每个回合都 `touch_activity` → idle 永不触发 → 经验只采集不压缩、越堆越多;自驱循环每 N 轮
/// 调本命令补上消化(P2)。取 Flywheel 档(前台 Agent 档随时让位它);用与 `idle::digest_while_idle` 同一组
/// Flywheel 原语,只是不做 idle 让位 / 不附带 skill 提议(那是 idle 飞轮的事)。返回本次提炼出的知识条数。
#[tauri::command]
pub async fn run_digest_pass(state: State<'_, SharedState>) -> Result<usize, String> {
    use growbox_learn::Flywheel;
    // 第 1 拍:镜像(极短锁)。克隆活跃经验 + 聚类 + 克隆潜意识桥 + arbiter,随即放锁。
    let (clusters, bridge, arbiter) = {
        let st = state.lock().await;
        let Some(bridge) = st.bridge.clone() else { return Ok(0) };
        let experiences = Flywheel::active_experiences(&st.memory);
        if experiences.len() < 2 {
            return Ok(0);
        }
        (Flywheel::new().clusters_of(&experiences), bridge, st.arbiter.clone())
    };
    let fw = Flywheel::new();
    let mut produced = 0usize;
    for members in clusters {
        // 第 2 拍:蒸馏(无锁,取 Flywheel 档;慢的 LLM 在此,前台来了排它前面)。
        let distilled = {
            let _gate = arbiter.acquire(crate::arbiter::Priority::Flywheel).await;
            fw.distill_cluster(&members, bridge.as_ref()).await
        };
        let Some((knowledge, superseded)) = distilled else {
            continue; // 噪音簇:无共同模式,跳过。
        };
        // 第 3 拍:写回(极短锁)。
        {
            let mut st = state.lock().await;
            Flywheel::apply_distilled(&mut st.memory, knowledge, &superseded);
        }
        produced += 1;
    }
    Ok(produced)
}

/// 用户显式引用历史(阶段4「历史引用」)——在当前位置钉一条**强制跳转指针**指向那段历史。
/// `target` = 用户指认的历史节点 id;`from` 缺省=当前对话最近位置。返回实际钉下的 source 位置。
/// 之后检索导航的入口落到该位置即无条件召回 target(位置键,遍历到此必跳)。见 `Memory::pin_history_reference`。
#[tauri::command]
pub async fn reference_history(
    state: State<'_, SharedState>,
    target: String,
    from: Option<String>,
) -> Result<Option<String>, String> {
    let mut st = state.lock().await;
    Ok(st.memory.pin_history_reference(from.as_deref(), &target))
}

// ===================== 活的 IDE:UI 操控(推论 7)=====================

/// 前端 mount 时声明自己有哪些可被 LLM 操控的面板(单一真相在前端)。
/// 后端只持这份运行时副本,据此为 `ui_control` 生成 schema 并校验。见 `计划/活的IDE-UI执行器.md`。
#[tauri::command]
pub async fn register_ui_surfaces(state: State<'_, SharedState>, surfaces: Vec<UiSurface>) -> Result<(), String> {
    let st = state.lock().await;
    *st.ui_catalog.write() = surfaces;
    Ok(())
}

/// 前端落地一个家族二 UI 操作后回执(往返的另一半):按相关 id 把验证态投回等待的脊柱。
/// 走独立的 `UiAckRegistry` managed state,不触 AppState 锁(避免与持锁 await 的 run_chat 死锁)。
#[tauri::command]
pub fn ui_action_ack(
    registry: State<'_, UiAckRegistry>,
    id: String,
    applied: bool,
    ui_state: Option<Value>,
    note: Option<String>,
) {
    registry.deliver(&id, UiAck { applied, state: ui_state.unwrap_or(Value::Null), note });
}

/// 用户决定脊柱回执:前端弹窗(权限/shell)用户裁决后调用,按 id 把决定投回等待的脊柱。
/// 走独立的 `Decisions` managed state,不触 AppState 锁(与持锁 await 的 run_chat 不死锁)。
/// decision: "once"(允许这次) | "remember"(记住本项) | "trust_project"(信任整个项目) | "deny"(拒绝)。
#[tauri::command]
pub fn decision_ack(registry: State<'_, Decisions>, id: String, decision: String) {
    registry.deliver(&id, Decision::parse(&decision));
}

/// 前端上报某面板可见态变化(含用户手动开关)——感知闭合的另一半。
/// 更新后端缓存(`get_control_state` 据此上浮),真实翻转时 `perceive` 让 agent 看见用户的 UI 动作。
/// 首次上报(启动同步)不 perceive;只有从已知旧值真实翻转才记一条(去噪)。
#[tauri::command]
pub async fn ui_state_changed(state: State<'_, SharedState>, panel_id: String, open: bool) -> Result<(), String> {
    state.lock().await.note_ui_panel(&panel_id, open);
    Ok(())
}

#[tauri::command]
pub async fn confirm_app_exit() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn suggestion_response() -> Result<(), String> {
    Ok(())
}
