//! 配置旋钮命令(推论9 数值全可设):邻域缓存/指针/检索/瞬态/疲劳/Agent循环/idle/超时退避/工具输出上限。

use super::*;

/// 设置邻域缓存容量(控制面板可调,数值参数全可设——`设计/00-交互层` 推论9)。
/// 即时生效(重建缓存=清当前工作集,磁盘图不动)+ 落库,无需重连。
#[tauri::command]
pub async fn set_neighbor_cache_cap(state: State<'_, SharedState>, cap: u32) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.neighbor_cache_cap = cap;
    st.memory.set_cache_capacity(cap as usize);
    st.save_settings();
    Ok(())
}

/// 设置学习型指针旋钮(匹配档 + 4 阈值;`计划/指针-学习型边.md`,推论9 数值全可设)。
/// 即时生效(下次检索即用新值)+ 落库,无需重连。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn set_pointer_config(
    state: State<'_, SharedState>,
    match_mode: String,
    follow_threshold: f32,
    neg_block_threshold: f32,
    k_merge_threshold: f32,
    weight_gain: f32,
    k_cap: u32,
    force_judge: bool,
) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.pointer_match_mode = match_mode.clone();
    st.settings.pointer_follow_threshold = follow_threshold;
    st.settings.pointer_neg_block_threshold = neg_block_threshold;
    st.settings.pointer_k_merge_threshold = k_merge_threshold;
    st.settings.pointer_weight_gain = weight_gain;
    st.settings.pointer_k_cap = k_cap;
    st.settings.pointer_force_judge = force_judge;
    st.memory.set_pointer_config(growbox_memory::PointerConfig {
        match_mode: growbox_memory::PointerMatchMode::from_setting(&match_mode),
        follow_threshold,
        neg_block_threshold,
        k_merge_threshold,
        weight_gain,
        k_cap: k_cap as usize,
        force_judge_on_cosine_hit: force_judge,
    });
    st.save_settings();
    Ok(())
}

/// 设置检索行为旋钮(RAG 命中阈/候选数 + 精确层入口/批量;推论9 数值全可设)。
/// 即时生效(下次检索即用新值)+ 落库,无需重连。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn set_retrieval_config(
    state: State<'_, SharedState>,
    rag_hit_threshold: f32,
    rag_topk: u32,
    entry_k: u32,
    entry_min_sim: f32,
    scan_batch: u32,
    project_boost: f32,
) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.retrieval_rag_hit_threshold = rag_hit_threshold;
    st.settings.retrieval_rag_topk = rag_topk;
    st.settings.retrieval_entry_k = entry_k;
    st.settings.retrieval_entry_min_sim = entry_min_sim;
    st.settings.retrieval_scan_batch = scan_batch;
    st.settings.retrieval_project_boost = project_boost;
    // scan_max / chunk_min_chars 不在本命令参数里(避免破坏前端既有调用),沿用持久设置(默认 256 / 1500);
    // 需要时单独旋钮接线。先取进局部(避免与下方 &mut st.memory 借用冲突)。
    let scan_max = st.settings.retrieval_scan_max as usize;
    let chunk_min_chars = st.settings.retrieval_chunk_min_chars as usize;
    st.memory.set_retrieval_config(growbox_memory::RetrievalConfig {
        rag_hit_threshold,
        rag_topk: rag_topk as usize,
        entry_k: entry_k as usize,
        entry_min_sim,
        scan_batch: scan_batch as usize,
        scan_max,
        project_boost,
        chunk_min_chars,
    });
    st.save_settings();
    Ok(())
}

/// 设置瞬态容量旋钮(碎片/二级/内部环 cap + 反K复核天数/边数;推论9 数值全可设)。
/// 重建碎片台账/二级索引(瞬态可再生,清空可接受)+ 截断内部环 + 刷新反K复核参数。落库持久。
#[tauri::command]
pub async fn set_transient_caps(
    state: State<'_, SharedState>,
    fragment_ledger_cap: u32,
    secondary_index_cap: u32,
    internal_events_cap: u32,
    artifact_interactions_cap: u32,
    neg_review_max_age_days: u32,
    neg_review_max_edges: u32,
) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.transient_fragment_ledger_cap = fragment_ledger_cap.max(1);
    st.settings.transient_secondary_index_cap = secondary_index_cap.max(1);
    st.settings.transient_internal_events_cap = internal_events_cap.max(1);
    st.settings.transient_artifact_interactions_cap = artifact_interactions_cap.max(1);
    st.settings.transient_neg_review_max_age_days = neg_review_max_age_days.max(1);
    st.settings.transient_neg_review_max_edges = neg_review_max_edges.max(1);
    // 先取值成局部(经 MutexGuard 解引用读 settings),再写 memory,避免同时借 st。
    let caps = growbox_memory::TransientCapsConfig {
        fragment_ledger_cap: st.settings.transient_fragment_ledger_cap as usize,
        secondary_index_cap: st.settings.transient_secondary_index_cap as usize,
        internal_events_cap: st.settings.transient_internal_events_cap as usize,
        artifact_interactions_cap: st.settings.transient_artifact_interactions_cap as usize,
        neg_review_max_age_ms: st.settings.transient_neg_review_max_age_days as i64 * 86_400_000,
        neg_review_max_edges: st.settings.transient_neg_review_max_edges as usize,
    };
    st.memory.set_transient_caps(caps);
    st.save_settings();
    Ok(())
}

/// 设置疲劳公式权重旋钮(命中率低/淘汰/碎片三权重;推论9 数值全可设)。即时生效(下次 fatigue() 即用)+ 落库。
#[tauri::command]
pub async fn set_fatigue_config(
    state: State<'_, SharedState>,
    w_hitrate: f32,
    w_evict: f32,
    w_fragment: f32,
) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.fatigue_w_hitrate = w_hitrate;
    st.settings.fatigue_w_evict = w_evict;
    st.settings.fatigue_w_fragment = w_fragment;
    st.memory.set_fatigue_config(growbox_memory::FatigueConfig {
        w_hitrate: w_hitrate as f64,
        w_evict: w_evict as f64,
        w_fragment: w_fragment as f64,
    });
    st.save_settings();
    Ok(())
}

/// 设置 Agent 循环行为旋钮(截断重试/token上限/沉默超时/空转上限 + complete 沉默超时;推论9 数值全可设)。
/// 截断重试/token上限/沉默超时/空转 = 下次回合即生效(AgentConfig 每回合从 settings 重建);
/// complete 沉默超时落在 LlmBridge(连接时构建)→ 下次重连生效。落库持久。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn set_agent_config(
    state: State<'_, SharedState>,
    max_token_retries: u32,
    token_ceil: u32,
    silence_secs: u32,
    max_stall: u32,
    parallel_max: u32,
    complete_silence_secs: u32,
    reasoning_effort: String,
    self_verify: bool,
    self_verify_min_tools: u32,
    recall_in_loop: bool,
) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.agent_max_token_retries = max_token_retries;
    st.settings.agent_token_ceil = token_ceil;
    st.settings.agent_silence_secs = silence_secs;
    st.settings.agent_max_stall = max_stall;
    st.settings.parallel_max = parallel_max.max(1); // 至少 1(0 会让并发批永不前进)
    st.settings.complete_silence_secs = complete_silence_secs;
    // 思考强度:只认 high/max,其它(含空串)回退 high(默认,deepseek 官方默认)。
    st.settings.reasoning_effort = if reasoning_effort == "max" { "max".into() } else { "high".into() };
    // ★主动自检★开关 + 触发阈值(下回合对话即用新值;阈值 ≥1)。
    st.settings.self_verify = self_verify;
    st.settings.self_verify_min_tools = self_verify_min_tools.max(1);
    // ★回合内补检索★开关(下回合对话即用新值;AgentConfig 每回合从 settings 重建)。
    st.settings.recall_in_loop = recall_in_loop;
    st.save_settings();
    Ok(())
}

/// 回显当前 Agent 循环旋钮(Settings.tsx 面板加载时取,数值全可设回显)。
#[tauri::command]
pub async fn get_agent_config(state: State<'_, SharedState>) -> Result<Value, String> {
    let s = &state.lock().await.settings;
    Ok(json!({
        "max_token_retries": s.agent_max_token_retries,
        "token_ceil": s.agent_token_ceil,
        "silence_secs": s.agent_silence_secs,
        "max_stall": s.agent_max_stall,
        "parallel_max": s.parallel_max,
        "complete_silence_secs": s.complete_silence_secs,
        "reasoning_effort": s.reasoning_effort,
        "self_verify": s.self_verify,
        "self_verify_min_tools": s.self_verify_min_tools,
        "recall_in_loop": s.recall_in_loop,
    }))
}

/// 设置 idle/做梦/睡眠旋钮(idle 阈值/巡检间隔/疲劳睡眠阈/睡眠步数/推演次数;推论9 数值全可设)。
/// IdleWorker 每拍从 settings 重读 → 下一拍即生效(无需重启)。落库持久。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn set_idle_config(
    state: State<'_, SharedState>,
    idle_threshold_secs: u32,
    idle_tick_secs: u32,
    idle_fatigue_threshold: f32,
    idle_max_sleep_steps: u32,
    idle_max_rehearsals: u32,
) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.idle_threshold_secs = idle_threshold_secs.max(1);
    st.settings.idle_tick_secs = idle_tick_secs.max(1);
    st.settings.idle_fatigue_threshold = idle_fatigue_threshold;
    st.settings.idle_max_sleep_steps = idle_max_sleep_steps;
    st.settings.idle_max_rehearsals = idle_max_rehearsals;
    st.save_settings();
    Ok(())
}

/// 回显当前 idle/睡眠旋钮(Settings.tsx 面板加载时取)。
#[tauri::command]
pub async fn get_idle_config(state: State<'_, SharedState>) -> Result<Value, String> {
    let s = &state.lock().await.settings;
    Ok(json!({
        "idle_threshold_secs": s.idle_threshold_secs,
        "idle_tick_secs": s.idle_tick_secs,
        "idle_fatigue_threshold": s.idle_fatigue_threshold,
        "idle_max_sleep_steps": s.idle_max_sleep_steps,
        "idle_max_rehearsals": s.idle_max_rehearsals,
    }))
}

/// 设置超时/退避旋钮(shell 批准超时 / UI ack 超时 / 后台任务退避基数·上限 + 分支日志上限;推论9 数值全可设)。
/// shell/UI 超时下回合即生效(TauriSink 每回合从 settings 重建);任务退避经 TaskManager 即时注入;
/// 分支日志上限下回合生效(AgentConfig 每回合从 settings 重建 BranchLog,见 agent/mod.rs)。落库持久。
#[tauri::command]
pub async fn set_misc_config(
    state: State<'_, SharedState>,
    shell_approval_timeout_secs: u32,
    ui_ack_timeout_secs: u32,
    task_backoff_base_ms: u32,
    task_backoff_cap_ms: u32,
    branch_log_max_gb: f64,
) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.shell_approval_timeout_secs = shell_approval_timeout_secs.max(1);
    st.settings.ui_ack_timeout_secs = ui_ack_timeout_secs.max(1);
    st.settings.task_backoff_base_ms = task_backoff_base_ms.max(1);
    st.settings.task_backoff_cap_ms = task_backoff_cap_ms.max(1);
    // 分支日志上限(GB):负数一律归一为 -1 = 无限制;非负原样(0 = 即写即覆盖,退化但合法)。
    st.settings.branch_log_max_gb = if branch_log_max_gb < 0.0 { -1.0 } else { branch_log_max_gb };
    st.task_mgr.set_backoff(task_backoff_base_ms as u64, task_backoff_cap_ms as u64);
    st.save_settings();
    Ok(())
}

/// 回显当前超时/退避旋钮(Settings.tsx 面板加载时取)。
#[tauri::command]
pub async fn get_misc_config(state: State<'_, SharedState>) -> Result<Value, String> {
    let s = &state.lock().await.settings;
    Ok(json!({
        "shell_approval_timeout_secs": s.shell_approval_timeout_secs,
        "ui_ack_timeout_secs": s.ui_ack_timeout_secs,
        "task_backoff_base_ms": s.task_backoff_base_ms,
        "task_backoff_cap_ms": s.task_backoff_cap_ms,
        "auto_shutdown_allowed": s.auto_shutdown_allowed,
        "cache_prewarm": s.cache_prewarm,
        "branch_log_max_gb": s.branch_log_max_gb,
        "lazy_tools": s.lazy_tools,
        "deferred_tools": s.deferred_tools,
    }))
}

/// ★二期 C1★:设置工具懒加载总开关 + deferred 名单(推论9/一切可设)。**即时生效**:更新 settings +
/// 重注入 registry(下回合 tools 装配即用新值),落库持久。关 = 旧行为;开 = 核心常驻 + tool_search 按需加载。
#[tauri::command]
pub async fn set_lazy_tools_config(
    state: State<'_, SharedState>,
    lazy_tools: bool,
    deferred_tools: Vec<String>,
) -> Result<(), String> {
    let mut st = state.lock().await;
    // 去空白/去空项,保留顺序去重(用户可能从文本框粘贴)。
    let mut seen = std::collections::HashSet::new();
    let cleaned: Vec<String> = deferred_tools
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && seen.insert(s.clone()))
        .collect();
    st.settings.lazy_tools = lazy_tools;
    st.settings.deferred_tools = cleaned.clone();
    st.registry.set_lazy_tools(lazy_tools, cleaned); // 即时生效(下回合 tools_for 即用新值)
    st.save_settings();
    Ok(())
}

/// 设置工具输出上限旋钮(file_read/file_list/shell 输出 + 后台任务输出尾巴;推论9 数值全可设)。
/// 经 Registry(ExecCtx 注入)+ TaskManager 即时生效(下次工具调用/任务即用新值)。落库持久。
#[tauri::command]
// Tauri 命令:各上限旋钮是独立标量参数(前端逐个传),参数较多属命令签名本质,不拆。
#[allow(clippy::too_many_arguments)]
pub async fn set_tool_limits(
    state: State<'_, SharedState>,
    max_read_bytes: u32,
    max_list_entries: u32,
    max_output_bytes: u32,
    max_outline_symbols: u32,
    task_output_cap: u32,
    context_window_tokens: u32,
    shell_timeout_secs: u32,
) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.tool_max_read_bytes = max_read_bytes.max(1);
    st.settings.tool_max_list_entries = max_list_entries.max(1);
    st.settings.tool_max_output_bytes = max_output_bytes.max(1);
    st.settings.tool_max_outline_symbols = max_outline_symbols.max(1);
    st.settings.task_output_cap = task_output_cap.max(1);
    // 上下文窗口总量(面板"实时上下文压力"分母);下限 1024 防除零/荒谬小值。
    st.settings.context_window_tokens = context_window_tokens.max(1024);
    // shell 命令墙钟超时秒(0=不限,慎用;不 clamp,允许用户显式关掉超时)。
    st.settings.shell_timeout_secs = shell_timeout_secs;
    // 先取值成局部(经 MutexGuard 解引用读 settings),再分别写 registry/task_mgr,避免同时借 st。
    let limits = growbox_core::ToolLimits {
        max_read_bytes: st.settings.tool_max_read_bytes as usize,
        max_list_entries: st.settings.tool_max_list_entries as usize,
        max_output_bytes: st.settings.tool_max_output_bytes as usize,
        max_outline_symbols: st.settings.tool_max_outline_symbols as usize,
        shell_timeout_secs: st.settings.shell_timeout_secs as u64,
    };
    let oc = st.settings.task_output_cap as usize;
    st.registry.set_limits(limits);
    st.task_mgr.set_output_cap(oc);
    st.save_settings();
    Ok(())
}

/// 设置 Web 工具配置(web_search provider/端点/key + 条数;web_fetch/web_search 超时;推论9 全可设)。
/// 经 Registry 共享配置即时生效(下次调用即用新值)。落库持久。
#[tauri::command]
pub async fn set_web_config(
    state: State<'_, SharedState>,
    provider: String,
    api_base: String,
    api_key: String,
    max_results: u32,
    timeout_secs: u32,
) -> Result<(), String> {
    let provider = provider.trim().to_ascii_lowercase();
    if !matches!(provider.as_str(), "" | "duckduckgo" | "ddg" | "tavily" | "brave" | "searxng") {
        return Err(format!(
            "未知搜索 provider「{provider}」(支持 duckduckgo/tavily/brave/searxng;空=用免 key 的 DuckDuckGo)"
        ));
    }
    let mut st = state.lock().await;
    st.settings.web_search_provider = provider;
    st.settings.web_search_api_base = api_base.trim().to_string();
    st.settings.web_search_api_key = api_key.trim().to_string();
    st.settings.web_search_max_results = max_results.clamp(1, 10);
    st.settings.web_timeout_secs = timeout_secs; // 0 = 不限,慎用(同 shell_timeout 语义)
    let cfg = st.web_config_from_settings();
    st.registry.set_web_config(cfg);
    st.save_settings();
    Ok(())
}

/// 回显 Web 工具配置(设置面板加载时取)。key 原样回显(本地单机应用,与 api_key 等同策略)。
#[tauri::command]
pub async fn get_web_config(state: State<'_, SharedState>) -> Result<Value, String> {
    let s = &state.lock().await.settings;
    Ok(json!({
        "provider": s.web_search_provider,
        "api_base": s.web_search_api_base,
        "api_key": s.web_search_api_key,
        "max_results": s.web_search_max_results,
        "timeout_secs": s.web_timeout_secs,
    }))
}

/// 设置工具记忆 + 不犯第二遍旋钮(总开关 + 两个相似度阈;计划/工具记忆-不犯第二遍)。即时生效 + 落库。
#[tauri::command]
pub async fn set_tool_memory_config(
    state: State<'_, SharedState>,
    enabled: bool,
    veto_threshold: f32,
    warn_threshold: f32,
) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.tool_memory_enabled = enabled;
    st.settings.tool_memory_veto_threshold = veto_threshold.clamp(0.0, 1.0);
    st.settings.tool_memory_warn_threshold = warn_threshold.clamp(0.0, 1.0);
    st.save_settings();
    Ok(())
}

/// 回显工具记忆旋钮(设置面板加载时)。
#[tauri::command]
pub async fn get_tool_memory_config(state: State<'_, SharedState>) -> Result<Value, String> {
    let s = &state.lock().await.settings;
    Ok(json!({
        "enabled": s.tool_memory_enabled,
        "veto_threshold": s.tool_memory_veto_threshold,
        "warn_threshold": s.tool_memory_warn_threshold,
    }))
}

/// ★tsserver 自动装配★:经 npm 把 typescript-language-server 装进 GrowBox 自有目录(需系统已装 node/npm)。
/// 长任务(下载 npm 包),装好返回二进制路径。前端按钮触发。
#[tauri::command]
pub async fn install_tsserver() -> Result<String, String> {
    crate::lsp::LspManager::install_tsserver().await
}

/// 回显 TS/JS 语言服务器状态(已装?有 npm?)。设置面板据此显示「装配」按钮。
#[tauri::command]
pub async fn tsserver_status() -> Result<Value, String> {
    Ok(json!({
        "installed": crate::lsp::LspManager::ts_installed(),
        "npm": crate::lsp::LspManager::npm_available(),
    }))
}

/// 回显当前工具输出上限旋钮(Settings.tsx 面板加载时取)。
#[tauri::command]
pub async fn get_tool_limits(state: State<'_, SharedState>) -> Result<Value, String> {
    let s = &state.lock().await.settings;
    Ok(json!({
        "max_read_bytes": s.tool_max_read_bytes,
        "max_list_entries": s.tool_max_list_entries,
        "max_output_bytes": s.tool_max_output_bytes,
        "max_outline_symbols": s.tool_max_outline_symbols,
        "task_output_cap": s.task_output_cap,
        "context_window_tokens": s.context_window_tokens,
        "shell_timeout_secs": s.shell_timeout_secs,
    }))
}
