//! 状态/查询命令:工具卡片 + 运行状态 + 健康灯 + i18n 目录 + 提示词语言 + 安全开关 + 控制面板/记忆统计。

use super::*;

// ===================== 工具 / 状态 =====================

/// 工具开关面板列表。`ui_lang` 由前端传(界面语言 zh-CN/en/ja/zh-TW),按其本地化 label/description;
/// `name` 恒英文 key(前端 toggle 用)。切界面语言时前端重拉本命令即可刷新。
#[tauri::command]
pub async fn get_tools(state: State<'_, SharedState>, ui_lang: Option<String>) -> Result<Vec<Value>, String> {
    let ui_lang = ui_lang.unwrap_or_else(|| "zh-CN".to_string());
    let st = state.lock().await;
    Ok(st
        .registry
        .ui_cards(&ui_lang)
        .into_iter()
        .map(|c| json!({ "name": c.name, "label": c.label, "description": c.description, "enabled": true }))
        .collect())
}

#[tauri::command]
pub async fn set_tools(_state: State<'_, SharedState>) -> Result<Value, String> {
    Ok(Value::Null) // v1:工具集固定,不做禁用
}

/// 工具分类(给聊天里的图标区分用):哪些可调用名字是**工作流**、哪些是 **MCP** 外部工具。
/// 其余 = 内置工具。前端据此给不同 SVG 图标(工作流=分支流程 / MCP=外部包 / 内置=扳手)。
#[tauri::command]
pub async fn get_tool_kinds(state: State<'_, SharedState>) -> Result<Value, String> {
    let st = state.lock().await;
    let workflows: Vec<String> = st.registry.workflow_defs().into_iter().map(|d| d.name).collect();
    let mcp: Vec<String> = st.registry.mcp_hub().tool_names();
    Ok(json!({ "workflows": workflows, "mcp": mcp }))
}

#[tauri::command]
pub async fn get_status(
    state: State<'_, SharedState>,
    meter: State<'_, crate::context_meter::ContextMeter>,
) -> Result<Value, String> {
    // 实时上下文压力:主链最近请求实发的 prompt_tokens(独立原子,先读,不占 AppState 锁)。
    let ctx_prompt_tokens = meter.get();
    let st = state.lock().await;
    // P6 接真:指针边数 + 邻域缓存(平铺单 LFU)+ 染色覆盖率 / index_density / budget_pct。
    let (cache_used, cache_capacity, _hit, _evict) = st.memory.cache_stats();
    let pointer_count = st.memory.pointer_count();
    let total_nodes = st.memory.timeline().len();
    let (deep, light, none) = st.memory.stain_coverage();
    let pct = |n: usize| if total_nodes > 0 { n as f64 / total_nodes as f64 * 100.0 } else { 0.0 };
    // index_density = 已入第一层索引的向量数 / 总节点数([0,1]);全向量化时 ≈1。
    let index_density = if total_nodes > 0 {
        (st.memory.index_len() as f64 / total_nodes as f64).min(1.0)
    } else {
        0.0
    };
    Ok(json!({
        "connected": st.connected,
        "session_id": st.session_id,
        "model": st.settings.model,
        "budget_pct": st.memory.context_fill_pct() * 100.0,
        "fatigue": st.memory.fatigue(),
        // 记忆置换率 [0,1]:★工作区真实换入换出 churn★(2026-06-15 改挂真置换;此前错读 L2 邻域边缓存→被 L1 命中率绑死恒 0)。
        "replacement_rate": st.memory.context_replacement_rate(),
        // 实时上下文压力:最近请求实发上下文 token / 上下文窗口总量(可设);0 = 尚无请求。
        "ctx_prompt_tokens": ctx_prompt_tokens,
        "ctx_window_tokens": st.settings.context_window_tokens,
        "attention_span": 0,
        // 后台任务运行计数(状态栏 "shell k";对外只需计数,详情对内见 get_tasks/list_tasks)。
        "running_tasks": st.task_mgr.running_count(),
        "cache_used": cache_used, "cache_capacity": cache_capacity,
        // ★缓存队列(工作区=存放区=真·临时记忆)★:侧栏仪表改挂真置换(常驻/真假指针)。
        // 此前侧栏「邻域缓存」读 cache_used/capacity(L2 加速器,RAG 命中不下沉→恒 0/256,被 L1 绑死)。
        "queue_resident": st.memory.context_resident_len(),
        "queue_fake": st.memory.context_fake_pointers(),
        "queue_real": st.memory.context_real_pointers(),
        "l2_index_size": 0, "pointer_count": pointer_count,
        // 染色三色(无 Red 染色概念,red 恒 0):Deep=深绿确信 / Light=浅绿快扫 / None=灰未到。
        "coverage_deep_green_pct": pct(deep), "coverage_light_green_pct": pct(light),
        "coverage_red_pct": 0.0, "coverage_gray_pct": pct(none),
        "reverse_index_size": 0,
        "subconscious_wired": st.bridge.is_some(),
        "fragment_count": st.memory.fragment_count(),
        "index_density": index_density,
        "total_nodes": total_nodes,
        "health": health_json(&st),
    }))
}

#[tauri::command]
pub async fn get_health(state: State<'_, SharedState>) -> Result<Value, String> {
    Ok(health_json(&*state.lock().await))
}

#[tauri::command]
pub async fn get_translations(_lang: String) -> Result<Value, String> {
    Ok(json!({})) // 前端 i18n 自带,后端不再下发
}

/// 设置提示词语言(给 LLM 的:系统提示词 + 工具 schema description)。与界面语言彻底解耦
/// (用户决策 2026-06-02:给机器看的固定中/英二选一)。归一 zh*/其余→zh/en、存盘、按新语言
/// 重载系统提示词(工具 schema description 每回合现取,无需重载)。
#[tauri::command]
pub async fn set_prompt_lang(state: State<'_, SharedState>, lang: String) -> Result<(), String> {
    let mut st = state.lock().await;
    let norm = crate::tool_i18n::normalize_prompt_lang(&lang).to_string();
    st.settings.lang = norm.clone();
    if let Some(res_dir) = st.resource_dir.clone() {
        st.base_system_prompt = AppState::load_agent_prompt(&res_dir, &norm);
    } else {
        // 无资源目录(开发/无包):退回内置中文常量(仅 zh,en 走资源文件)。
        st.base_system_prompt = SYSTEM_PROMPT.to_string();
    }
    st.save_settings();
    Ok(())
}

/// 提示/告知 对外目录(显示半,设计:`感知告知-双受众.md` Phase 2)。按 ui_lang 渲染 human 模板下发,
/// 前端 `notify(code,params)` 据此就地 toast(单一源仍是 notices.i18n.json,镜像 get_tools 模式)。
/// 无需 State:catalog 是编译期内嵌的只读单一源。
#[tauri::command]
pub async fn get_notice_catalog(ui_lang: Option<String>) -> Result<Value, String> {
    let ui_lang = ui_lang.unwrap_or_else(|| "zh-CN".to_string());
    Ok(crate::notice_i18n::notices().catalog(&ui_lang))
}

/// 提示/告知 对内感知(感知半):前端来源的 UX 提示 fire-and-forget 回后端,
/// 按会话 prompt_lang 渲染 llm 文本交 `Memory::perceive` → LLM 下一回合可感知(自我感知原则)。
/// perceive=false 的纯 chrome 由 `perceive_notice` 内部按 catalog 标志直接跳过。
#[tauri::command]
pub async fn perceive_notice(
    state: State<'_, SharedState>,
    code: String,
    params: Value,
) -> Result<(), String> {
    let mut guard = state.lock().await;
    let st = &mut *guard;
    crate::notify::perceive_notice(&mut st.memory, &st.settings.lang, &code, &params);
    Ok(())
}

/// 打开外部 URL(系统浏览器)。只允许 http(s),用 OS 默认打开器(不在 webview 内导航,免把应用导航走)。
/// 给聊天里渲染的可点击超链接用(markdown.ts 渲染 + ChatArea 委托点击)。
#[tauri::command]
pub fn open_external_url(url: String) -> Result<(), String> {
    let u = url.trim().to_string();
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        return Err("只允许打开 http(s) 链接".into());
    }
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(&u);
        c
    };
    #[cfg(target_os = "linux")]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(&u);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", &u]);
        c
    };
    cmd.spawn().map(|_| ()).map_err(|e| format!("打开链接失败: {e}"))
}

/// 工具命令/路径显示:完整 or 截断(用户在设置里切换,统一管所有工具显示)。立即持久化,无需重连。
#[tauri::command]
pub async fn set_truncate_tool_display(state: State<'_, SharedState>, truncate: bool) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.truncate_tool_display = truncate;
    st.save_settings();
    Ok(())
}

/// 自动模式开关:false=手动(shell 逐条批准),true=自动(LLM 安全审核)。立即持久化,无需重连
/// (下一回合的 AgentConfig 即取新值)。
#[tauri::command]
pub async fn set_auto_mode(state: State<'_, SharedState>, auto: bool) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.auto_mode = auto;
    st.save_settings();
    Ok(())
}

/// ★danger 模式(为所欲为)★开关:true = 所有安全门一律放行(系统级操作/敏感路径/危险命令/SSRF 全不拦)。
/// 极高风险,供无人值守自驱做"系统装 Python、全局 npm"等不卡授权。立即生效:同步 sandbox 标志(judge 据此放行)
/// + 下一回合 AgentConfig 取新值。**不持久**(Settings.danger_mode 是 serde(skip)),save_settings 不写盘 →
/// 重启默认关,必须显式重开,防遗忘的危险模式随重启自启。
#[tauri::command]
pub async fn set_danger_mode(state: State<'_, SharedState>, danger: bool) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.danger_mode = danger;
    st.sandbox.set_danger(danger); // 即时生效(即便此刻无回合在跑,也立刻反映到 judge)
    Ok(())
}

/// 读取用户配置的隐私文件夹列表(前端设置面板编辑用)。
#[tauri::command]
pub async fn get_privacy_dirs(state: State<'_, SharedState>) -> Result<Vec<String>, String> {
    Ok(state.lock().await.settings.privacy_dirs.clone())
}

/// 设置隐私文件夹列表(去空去重)。命中(且未授权)时必弹窗 + 二次确认。立即持久化,无需重连。
#[tauri::command]
pub async fn set_privacy_dirs(state: State<'_, SharedState>, dirs: Vec<String>) -> Result<(), String> {
    let mut st = state.lock().await;
    let mut seen = std::collections::HashSet::new();
    st.settings.privacy_dirs = dirs
        .into_iter()
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty() && seen.insert(d.clone()))
        .collect();
    st.save_settings();
    Ok(())
}

/// 用户授权放行本项目的 shell 系统路径引用(项目级 shell 信任,会话级)。
/// 关键:走这里**不污染目录列表**——只读探测(ls/which/command -v)引用 /usr 等触发的授权
/// 不再被错加成"可写目录"(用户决策 2026-06-02:只读探测不该被授权成可写)。
#[tauri::command]
pub async fn grant_shell_access(state: State<'_, SharedState>) -> Result<(), String> {
    let mut st = state.lock().await;
    st.sandbox.grant(growbox_safety::GrantScope::ThisProject);
    Ok(())
}

// ===================== v1 专属面板:安全空桩 =====================

#[tauri::command]
pub async fn get_control_state(state: State<'_, SharedState>) -> Result<Value, String> {
    let st = state.lock().await;
    // P6 接真:工具调用总数/失败/错误率(走唯一分发路径 Registry 的原子计数)。
    let (tool_total, tool_fail) = st.registry.tool_stats();
    let error_rate = if tool_total > 0 {
        tool_fail as f64 / tool_total as f64 * 100.0
    } else {
        0.0
    };
    // json_* / backup_count 仍为 0:无 JSON-mode 调用计量、无 file_write 快照备份机制(诚实置零,非谎报)。
    Ok(json!({
        "safety": { "backups_dir": "", "backup_count": 0, "note": "" },
        // 接真(去空桩):与 get_memory_stats 同源,控制面板"记忆网络"不再恒"暂无记忆数据"。
        "memory_stats": memory_stats_json(&st),
        "network_status": { "main_model": st.settings.model, "sub_model": Value::Null, "has_embedder": st.bridge.is_some() },
        "error_rate_pct": error_rate, "json_compliance_pct": 100.0,
        "tool_call_total": tool_total, "tool_call_fail": tool_fail,
        "json_call_total": 0, "json_call_valid": 0,
        // 活的 IDE:各面板真实可见态(前端 ui_state_changed 上报,含用户手动开关)。ack 缝至此闭合。
        "ui_panels": serde_json::to_value(&st.ui_panel_state).unwrap_or(Value::Null),
    }))
}

/// 记忆统计单一来源(P4/P5 接真:指针边数 + 三级缓存 + 疲劳度三指标)。
/// get_memory_stats(MemoryViz)与 get_control_state(控制面板)同取此源,避免两面板割裂。
fn memory_stats_json(st: &AppState) -> Value {
    let (cache_used, cache_capacity, hit_rate, evictions) = st.memory.cache_stats();
    let fragment_count = st.memory.fragment_count();
    let total_nodes = st.memory.timeline().len();
    let fragment_ratio = if total_nodes > 0 {
        (fragment_count as f64 / total_nodes as f64).min(1.0)
    } else {
        0.0
    };
    json!({
        "total_nodes": total_nodes,
        "total_pointers": st.memory.pointer_count(),
        // 平铺单 LFU 缓存(退役三级 1:2:4):占用 / 容量 / 命中率 / 淘汰数。这是 L2 导航边缓存=加速层(幕后,非主表)。
        "cache": { "used": cache_used, "capacity": cache_capacity, "hit_rate": hit_rate, "total_evictions": evictions },
        // ★缓存队列(工作区 = 存放区 = 置换系统的"物理内存")★:面板「缓存队列」(原「队列占用/邻域缓存」)真实来源。
        // fill_pct=占用/预算(队列从空涨起,满了是常态)· resident=常驻块数 · evictions/replacement_rate=真置换 churn
        //（队列换出 = 记忆区换出,同一事件;Nap 归零)· fake/real_pointers=队列里假指针(RAG)/真指针(L2)各几条。
        "queue": { "fill_pct": st.memory.context_fill_pct(), "resident": st.memory.context_resident_len(), "evictions": st.memory.context_evictions(), "replacement_rate": st.memory.context_replacement_rate(), "fake_pointers": st.memory.context_fake_pointers(), "real_pointers": st.memory.context_real_pointers() },
        // eviction_rate 改读工作区真实淘汰次数(劳累度 hint 显示;此前恒 0)。
        "fatigue": { "cache_hit_rate": hit_rate, "eviction_rate": st.memory.context_evictions() as f64, "fragment_count": fragment_count, "fragment_ratio": fragment_ratio, "fatigue_value": st.memory.fatigue() },
        "secondary_indexes": { "total": st.memory.secondary_index_count(), "forced_jumps": st.memory.forced_jump_count(), "fragment_count": fragment_count, "cleared": st.memory.fragments_cleared() },
        // 学习型指针旋钮(控制面板回显 + 可设;推论9)。
        "pointer": {
            "match_mode": st.memory.pointer_config().match_mode.as_setting(),
            "follow_threshold": st.memory.pointer_config().follow_threshold,
            "neg_block_threshold": st.memory.pointer_config().neg_block_threshold,
            "k_merge_threshold": st.memory.pointer_config().k_merge_threshold,
            "weight_gain": st.memory.pointer_config().weight_gain,
            "k_cap": st.memory.pointer_config().k_cap,
            "force_judge": st.memory.pointer_config().force_judge_on_cosine_hit,
        },
        // 检索行为旋钮(控制面板回显 + 可设;推论9)。
        "retrieval": {
            "rag_hit_threshold": st.memory.retrieval_config().rag_hit_threshold,
            "rag_topk": st.memory.retrieval_config().rag_topk,
            "entry_k": st.memory.retrieval_config().entry_k,
            "entry_min_sim": st.memory.retrieval_config().entry_min_sim,
            "scan_batch": st.memory.retrieval_config().scan_batch,
            "project_boost": st.memory.retrieval_config().project_boost,
        },
        // 疲劳公式权重旋钮(控制面板回显 + 可设;推论9)。
        "fatigue_weights": {
            "w_hitrate": st.memory.fatigue_config().w_hitrate,
            "w_evict": st.memory.fatigue_config().w_evict,
            "w_fragment": st.memory.fatigue_config().w_fragment,
        },
        // 瞬态容量旋钮(控制面板回显 + 可设;推论9)。老化阈以天回显(后端存毫秒)。
        "transient": {
            "fragment_ledger_cap": st.memory.transient_caps().fragment_ledger_cap,
            "secondary_index_cap": st.memory.transient_caps().secondary_index_cap,
            "internal_events_cap": st.memory.transient_caps().internal_events_cap,
            "artifact_interactions_cap": st.memory.transient_caps().artifact_interactions_cap,
            "neg_review_max_age_days": st.memory.transient_caps().neg_review_max_age_ms / 86_400_000,
            "neg_review_max_edges": st.memory.transient_caps().neg_review_max_edges,
        },
    })
}

#[tauri::command]
pub async fn get_memory_stats(state: State<'_, SharedState>) -> Result<Value, String> {
    Ok(memory_stats_json(&*state.lock().await))
}

#[tauri::command]
pub async fn get_fatigue_level(state: State<'_, SharedState>) -> Result<f64, String> {
    Ok(state.lock().await.memory.fatigue())
}

/// 后台任务列表(供状态栏 "shell k" 点开看详情)。对外:label(tag)+ 原命令 + 状态;
/// 与 list_tasks(给 LLM 对内感知)同源 task_mgr.snapshot()。两受众,一份数据。
#[tauri::command]
pub async fn get_tasks(state: State<'_, SharedState>) -> Result<Value, String> {
    let st = state.lock().await;
    let tasks: Vec<Value> = st
        .task_mgr
        .snapshot()
        .iter()
        .map(|t| {
            let state_str = match t.state {
                crate::tasks::TaskState::Running => "running",
                crate::tasks::TaskState::Done => "done",
                crate::tasks::TaskState::Failed => "failed",
            };
            json!({ "id": t.id, "label": t.label, "command": t.command, "state": state_str, "elapsed_ms": t.elapsed_ms })
        })
        .collect();
    Ok(json!({ "running": st.task_mgr.running_count(), "tasks": tasks }))
}
