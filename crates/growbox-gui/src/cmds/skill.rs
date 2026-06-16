//! Skill 系统的设置命令(设计/09 推论8)——把 Skill 纳入「所有能改的都能在设置控制」(用户原则)。
//!
//! 列出全部 skill(内置种子 + 已学,含来源/是否生效)、看正文、停用/启用(对内置与已学一视同仁,
//! 停用 = 不列清单/不召回/load 拒,但不删数据可随时重启)、总开关 + 常驻清单上限旋钮。
//! 全部即时生效 + 落库(同其他配置旋钮),无需重连。

use super::*;

/// 列出全部 skill(内置 + 已学;UI 用)。每条 = {name, trigger, source: builtin|learned, active}。
#[tauri::command]
pub async fn list_skills(state: State<'_, SharedState>) -> Result<Vec<Value>, String> {
    let st = state.lock().await;
    let out = crate::skills::all_skills(&st.memory)
        .into_iter()
        .map(|s| {
            json!({
                "name": s.name,
                "trigger": s.trigger,
                "category": s.category,
                "source": s.source,
                "active": s.active,
            })
        })
        .collect();
    Ok(out)
}

/// 取某 skill 的完整 playbook 正文(UI 展开时)。无则空串。
#[tauri::command]
pub async fn get_skill_body(state: State<'_, SharedState>, name: String) -> Result<String, String> {
    let st = state.lock().await;
    // load_body 会被 is_active 门控;UI 要看正文(含被停用的),故直接查已学 + 内置,不过滤。
    let body = st
        .memory
        .learned_skill_body(&name)
        .or_else(|| crate::skills::seed_body(&name).map(str::to_string))
        .unwrap_or_default();
    Ok(body)
}

/// 停用 / 启用某 skill(按名;对内置与已学一视同仁)。即时生效 + 落库。
#[tauri::command]
pub async fn set_skill_active(
    state: State<'_, SharedState>,
    name: String,
    active: bool,
) -> Result<(), String> {
    let mut st = state.lock().await;
    let key = name.to_ascii_lowercase();
    let mut set: std::collections::HashSet<String> =
        st.settings.skill_disabled.iter().map(|s| s.to_ascii_lowercase()).collect();
    if active {
        set.remove(&key);
    } else {
        set.insert(key);
    }
    st.settings.skill_disabled = set.into_iter().collect();
    apply_skill_config(&mut st);
    st.save_settings();
    Ok(())
}

/// 设置 Skill 总开关 + 常驻清单上限。即时生效 + 落库。
#[tauri::command]
pub async fn set_skill_config(
    state: State<'_, SharedState>,
    enabled: bool,
    list_max: u32,
    autoload_threshold: f32,
) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.skill_enabled = enabled;
    st.settings.skill_list_max = list_max;
    st.settings.skill_autoload_threshold = autoload_threshold;
    apply_skill_config(&mut st);
    st.save_settings();
    Ok(())
}

/// 回显 Skill 旋钮(设置 UI 打开时)。
#[tauri::command]
pub async fn get_skill_config(state: State<'_, SharedState>) -> Result<Value, String> {
    let st = state.lock().await;
    Ok(json!({
        "enabled": st.settings.skill_enabled,
        "list_max": st.settings.skill_list_max,
        "autoload_threshold": st.settings.skill_autoload_threshold,
    }))
}

// ===================== Skill 提议(S3 飞轮自学:idle 起草 → 用户采纳/丢弃) =====================

/// 列出待裁决的 skill 提议(设置 UI「技能提议」区)。每条 = {id,name,trigger,body,rationale,created_ms}。
#[tauri::command]
pub async fn list_skill_proposals(state: State<'_, SharedState>) -> Result<Vec<Value>, String> {
    let st = state.lock().await;
    let out = st
        .skill_proposals
        .pending
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "trigger": p.trigger,
                "body": p.body,
                "rationale": p.rationale,
                "created_ms": p.created_ms,
            })
        })
        .collect();
    Ok(out)
}

/// 采纳一条 skill 提议:经 `crystallize_skill` 结晶成真正的 skill 节点(即时嵌入可召回),并出队 + 落库。
/// 需已连接(crystallize 要潜意识桥做嵌入)。返回结晶后的 skill 名。
#[tauri::command]
pub async fn accept_skill_proposal(state: State<'_, SharedState>, id: String) -> Result<String, String> {
    let mut st = state.lock().await;
    let Some(bridge) = st.bridge.clone() else {
        return Err("需先连接(结晶 skill 要潜意识模型做嵌入)".into());
    };
    let Some(p) = st.skill_proposals.take(&id) else {
        return Err("提议不存在(可能已处理)".into());
    };
    // 结晶成 skill 节点(同名/近重复取代旧版;即时嵌入 → 立刻可召回/自动注入)。
    st.memory.crystallize_skill(&p.name, &p.trigger, &p.body, bridge.as_ref()).await;
    st.persist_skill_proposals();
    let name = p.name.clone();
    drop(st);
    Ok(name)
}

/// 丢弃一条 skill 提议:出队 + 记入"不再提"名单(防反复打扰)+ 落库。
#[tauri::command]
pub async fn reject_skill_proposal(state: State<'_, SharedState>, id: String) -> Result<(), String> {
    let mut st = state.lock().await;
    if st.skill_proposals.reject(&id).is_none() {
        return Err("提议不存在(可能已处理)".into());
    }
    st.persist_skill_proposals();
    Ok(())
}

/// 把 Settings 的 skill 旋钮推进 memory(即时生效)。
fn apply_skill_config(st: &mut AppState) {
    st.memory.set_skill_config(growbox_memory::SkillConfig {
        enabled: st.settings.skill_enabled,
        list_max: st.settings.skill_list_max as usize,
        autoload_threshold: st.settings.skill_autoload_threshold,
        disabled: st.settings.skill_disabled.iter().map(|s| s.to_ascii_lowercase()).collect(),
    });
}
