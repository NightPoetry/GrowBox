//! 连接/运行时命令:连接 LLM(探测+预热)+ 列模型 + 系统关机 + 会话重置。

use super::*;

// ===================== 连接 / 运行时 =====================

/// 应用版本号 -- 单一事实源 = workspace Cargo.toml(编译期 env! 注入)。
/// 前端显示一律从这里取;改版本只需改 Cargo.toml 一处(tauri.conf.json 已删硬编码 version,打包时继承 Cargo)。
#[tauri::command]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
pub async fn set_runtime_dir(state: State<'_, SharedState>, runtime_dir: String) -> Result<(), String> {
    if !runtime_dir.is_empty() {
        state.lock().await.set_runtime(runtime_dir.into());
    }
    Ok(())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn connect(
    state: State<'_, SharedState>,
    app: AppHandle,
    api_base: String,
    model: String,
    api_key: Option<String>,
    runtime_dir: String,
    max_turns: Option<u32>,
    max_tokens: Option<u32>,
    _supervisor_model: Option<String>,
    _supervisor_api_base: Option<String>,
    _supervisor_api_key: Option<String>,
    embed_remote: Option<bool>,
    embed_api_base: Option<String>,
    embed_api_key: Option<String>,
    embed_model: Option<String>,
    subconscious_model: Option<String>,
    subconscious_api_base: Option<String>,
    subconscious_api_key: Option<String>,
    working_context_chars: Option<u32>,
    recent_ring_chars: Option<u32>,
    neighbor_cache_cap: Option<u32>,
) -> Result<String, String> {
    let mut st = state.lock().await;
    if !runtime_dir.is_empty() {
        st.set_runtime(runtime_dir.into());
    }
    // 从资源文件加载系统提示词(失败则用内置 fallback);顺带记下资源目录(本地 e5 权重预置处)。
    if let Ok(res_dir) = app.path().resource_dir() {
        if st.base_system_prompt.is_empty() {
            let lang = st.settings.lang.clone();
            st.base_system_prompt = AppState::load_agent_prompt(&res_dir, &lang);
        }
        st.resource_dir = Some(res_dir);
    }
    if st.base_system_prompt.is_empty() {
        st.base_system_prompt = SYSTEM_PROMPT.to_string();
    }
    let mut s = st.settings.clone();
    s.api_base = api_base;
    s.model = model;
    // 非空才覆盖:空输入框不该清掉已落库的 key(否则每次连接都得重填)。
    if let Some(k) = api_key.filter(|k| !k.is_empty()) {
        s.api_key = k;
    }
    // 仍为空 → 回退环境变量 DEEPSEEK_API_KEY:补"GUI 连接从不读 env"的缺口(注入式/开发启动直接可连)。
    // app 本就把 api_key 持久化进 redb(设计内,state.rs 有 persist 测试),此回退不违反铁律。
    if s.api_key.is_empty() {
        if let Ok(k) = std::env::var("DEEPSEEK_API_KEY") {
            if !k.is_empty() {
                s.api_key = k;
            }
        }
    }
    // 循环轮数 / 输出 token:前端给了才覆盖,否则沿用已落库的值(0=无限/不限)。
    if let Some(mt) = max_turns {
        s.max_turns = mt;
    }
    if let Some(mt) = max_tokens {
        s.max_tokens = mt;
    }
    // 嵌入槽位:前端给了才覆盖,否则沿用已落库的值。
    if let Some(r) = embed_remote {
        s.embed_remote = r;
    }
    if let Some(b) = embed_api_base {
        s.embed_api_base = b;
    }
    if let Some(k) = embed_api_key {
        s.embed_api_key = k;
    }
    if let Some(m) = embed_model {
        s.embed_model = m;
    }
    // 独立潜意识模型槽:前端给了才覆盖(空字符串=清空=复用主模型,允许;故不过滤空)。
    if let Some(m) = subconscious_model {
        s.subconscious_model = m;
    }
    if let Some(b) = subconscious_api_base {
        s.subconscious_api_base = b;
    }
    if let Some(k) = subconscious_api_key {
        s.subconscious_api_key = k;
    }
    // 上下文预算(P4d):前端给了才覆盖,否则沿用已落库值(0=代码默认,随模型可调)。
    // 落进 settings 后由 state.connect → memory.configure_context 应用。
    if let Some(w) = working_context_chars {
        s.working_context_chars = w;
    }
    if let Some(r) = recent_ring_chars {
        s.recent_ring_chars = r;
    }
    if let Some(c) = neighbor_cache_cap {
        s.neighbor_cache_cap = c;
    }

    // 真探一次再算"连上"(修连接判断永远成功的 bug):端点不可达 / 模型不存在 → 失败。
    // 探测要发网络请求,先放掉 AppState 锁,别让 15s 网络往返占着全局锁。
    // 返回的结构化码(MODEL_NOT_FOUND/MODEL_NOT_LOADED/MODEL_ERROR/API_UNREACHABLE)
    // 直接透传给前端(Settings.tsx 按前缀匹配出对应提示),不要再包前缀。
    drop(st);
    probe_llm(&s.api_base, &s.api_key, &s.model).await?;

    // 探测通过,重新拿锁正式连接。
    let mut st = state.lock().await;
    let sid = st.connect(s);

    // 启动常驻 Supervisor(取消旧的,起新的)。
    if let Some(old) = st.supervisor.take() {
        old.cancel();
    }
    let state_arc: SharedState = (*state).clone();
    st.supervisor = Some(crate::supervisor::SupervisorHandle::spawn(
        st.task_mgr.clone(),
        state_arc,
        app.clone(),
    ));

    // 启动常驻 IdleWorker(取消旧的,起新的):idle 时把经验压成知识,见 `idle.rs`。
    if let Some(old) = st.idle_worker.take() {
        old.cancel();
    }
    let idle_state: SharedState = (*state).clone();
    st.idle_worker = Some(crate::idle::IdleWorkerHandle::spawn(
        idle_state,
        st.last_activity.clone(),
        st.arbiter.clone(),
        app.clone(),
    ));

    // 把启动期探到的严重异常(如持久化打不开)推给前端醒目告警(见 `异常告知.md`)。
    let _ = app.emit("health-alert", health_json(&st));

    // ⑦ 缓存预热(可设开关,默认开,见决策日志 2026-06-04):后台用一个最小请求把"系统提示词+工具"
    // 这段稳定前缀喂给模型预热 deepseek KV 缓存 → 首条真实回复(尤其造物)也命中缓存、不必等第二条才快。
    // 不阻塞 connect 返回(spawn 到后台)。
    if st.settings.cache_prewarm {
        if let Some(llm) = st.llm.clone() {
            let model = st.settings.model.clone();
            let lang = st.settings.lang.clone();
            let system_prompt = format!("{}\n\n{}", st.base_system_prompt, st.project_context());
            // 预热用 run 起手同款工具集(此刻无流程召回 → 物化集为空,与首轮一致)。
            let tools = st.registry.tools_for(&lang, None, &std::collections::HashSet::new());
            tokio::spawn(prewarm_cache(llm, model, system_prompt, tools));
        }
    }

    // ★二期 D2★ 持久 MCP server 跨重启自动重连(best-effort,不阻塞 connect 返回;失败仅记进状态)。
    let mcp_configs = st.settings.mcp_servers.clone();
    if !mcp_configs.is_empty() {
        let hub = st.registry.mcp_hub();
        tokio::spawn(async move {
            let statuses = super::mcp::reconnect_all(&hub, &mcp_configs).await;
            for s in &statuses {
                if s.get("connected").and_then(|c| c.as_bool()) != Some(true) && s.get("enabled").and_then(|e| e.as_bool()) == Some(true) {
                    eprintln!("[MCP] server 自动重连未成功: {s}");
                }
            }
        });
    }

    Ok(sid)
}

/// 缓存预热(⑦):后台用最小请求把"系统提示词 + 工具定义"稳定前缀喂给模型,预热其 KV 缓存。
/// 发出并拉完即弃(确保服务端完成 prefill → 缓存建立);max_tokens 极小,不浪费生成。
async fn prewarm_cache(
    llm: std::sync::Arc<dyn crate::bridge::LlmDriver>,
    model: String,
    system_prompt: String,
    tools: Vec<growbox_core::ToolDef>,
) {
    use growbox_llm::{ChatMessage, ChatRequest};
    let messages = vec![ChatMessage::system(system_prompt), ChatMessage::user("hi")];
    let req = ChatRequest::new(model, messages).with_tools(tools).with_max_tokens(16);
    if let Ok(mut rx) = llm.chat_stream(req).await {
        while rx.recv().await.is_some() {}
    }
}

/// 连接前真探一次:发一个最小流式请求,验证端点可达且模型存在。
///
/// 原 bug:`connect` 只建客户端就 `connected=true`,填任何 URL/不存在的模型都"连接成功"。
/// OpenAI 兼容服务器在**返回响应头**时就校验模型(不必等生成),所以这个探测很快:
/// - 模型不存在 / key 错 → 服务器 4xx → `chat_stream` 返回 `Api{status,body}` 错误;
/// - 端点拒绝 / 不可达 → `send()` 失败 → `Http` 错误;
/// - 端点接受 TCP 却不回 → 15s 超时兜底,不无限等。
///
/// 拿到 `Ok(rx)`(即 2xx 响应头)就够了,不去消费流(推理模型首 chunk 可能很久)。
///
/// 错误以**结构化码**返回(前端 `Settings.tsx` 按前缀给出对应中文提示):
/// `MODEL_NOT_LOADED:<model>` / `MODEL_NOT_FOUND:<model>` / `MODEL_ERROR:<detail>` / `API_UNREACHABLE:`。
async fn probe_llm(api_base: &str, api_key: &str, model: &str) -> Result<(), String> {
    use growbox_llm::{ChatMessage, ChatRequest, LlmClient, LlmError};
    let client = LlmClient::new(api_base.to_string(), api_key.to_string());
    let req = ChatRequest::new(model.to_string(), vec![ChatMessage::user("ping")]).with_max_tokens(1);
    match tokio::time::timeout(std::time::Duration::from_secs(15), client.chat_stream(req)).await {
        Ok(Ok(_rx)) => Ok(()),
        Ok(Err(LlmError::Api { status, body })) => {
            // 服务器收到请求但拒绝:多半是模型名问题(LM Studio 未加载常见 404)。
            let low = body.to_lowercase();
            if low.contains("not loaded") || low.contains("no models loaded") {
                Err(format!("MODEL_NOT_LOADED:{model}"))
            } else if status == 404 || low.contains("not found") || low.contains("does not exist") {
                Err(format!("MODEL_NOT_FOUND:{model}"))
            } else {
                Err(format!("MODEL_ERROR:{status}: {body}"))
            }
        }
        // 连接被拒/DNS/TLS 等传输层失败,或 15s 无响应 → 端点不可达。
        Ok(Err(LlmError::Http(_))) | Err(_) => Err("API_UNREACHABLE:".to_string()),
        Ok(Err(e)) => Err(format!("MODEL_ERROR:{e}")),
    }
}

#[tauri::command]
pub async fn list_models(api_base: String, api_key: Option<String>) -> Result<Vec<String>, String> {
    // 真发 `GET {api_base}/models`(此前写死 [flash, pro],无视 api_base = 谎报)。
    // 错误结构化码透传给前端(API_UNREACHABLE / API_BAD_RESPONSE)。
    let client = growbox_llm::LlmClient::new(api_base, api_key.unwrap_or_default());
    client.list_models().await
}

/// 真正执行关机(自关机能力,见 `计划/自关机能力.md`)。前端在用户一次性授权
/// (或 Settings.auto_shutdown_allowed 永久权)后调用 —— 授权裁决在前端确认窗,本命令只执行。
/// exit_self → app.exit(0);system_shutdown → detach 一个独立于 GrowBox 进程存活的倒计时 shutdown(§4)。
#[tauri::command]
pub async fn do_shutdown(app: AppHandle, action: String, delay_secs: u64) -> Result<(), String> {
    match action.as_str() {
        "exit_self" => {
            app.exit(0);
            Ok(())
        }
        "system_shutdown" => schedule_system_shutdown(delay_secs),
        other => Err(format!("未知关机动作: {other}")),
    }
}

/// detach 一个倒计时系统关机:独立于 GrowBox 进程存活(§4「先安排好身后事再走」)。
/// ★疫苗式优先(macOS)★:若已装 ShutdownHelper(签名小 app,走 System Events 自动化、免 root),
/// 用它——detached 启动、用 helper 自己的持久授权,跨 GrowBox 退出也成(见 helpers.rs)。
/// 否则回退老路 `shutdown -h`(需 root,可能失败,诚实回传)。
#[cfg(target_os = "macos")]
fn schedule_system_shutdown(delay_secs: u64) -> Result<(), String> {
    if crate::helpers::helper_exists("ShutdownHelper") {
        let d = delay_secs.to_string();
        return crate::helpers::launch_helper("ShutdownHelper", &["shutdown", &d]);
    }
    let minutes = delay_secs.div_ceil(60).max(1);
    let script = format!("nohup shutdown -h +{minutes} >/dev/null 2>&1 &");
    std::process::Command::new("sh")
        .arg("-c")
        .arg(&script)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("发起系统关机失败(无 helper 且 shutdown 需管理员权限): {e}"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn schedule_system_shutdown(delay_secs: u64) -> Result<(), String> {
    let minutes = delay_secs.div_ceil(60).max(1); // shutdown 以分钟计;至少 1 分钟
    // nohup + & 让关机命令脱离 GrowBox 进程独立存活(GrowBox 随后 exit_self 也不影响它)。
    let script = format!("nohup shutdown -h +{minutes} >/dev/null 2>&1 &");
    std::process::Command::new("sh")
        .arg("-c")
        .arg(&script)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("发起系统关机失败(可能需要管理员权限): {e}"))
}

/// ★疫苗式预接种 OS 授权★:spawn 对应 helper 做一次无害探针 → 触发系统授权弹窗(用户允许一次即永久)。
/// 例:kind="shutdown" → ShutdownHelper probe(读一次进程数,触发"控制 System Events"弹窗)。
#[tauri::command]
pub async fn vaccinate_permission(kind: String) -> Result<(), String> {
    match kind.as_str() {
        "shutdown" => crate::helpers::launch_helper("ShutdownHelper", &["probe"]),
        other => Err(format!("未知授权类型: {other}")),
    }
}

#[cfg(windows)]
fn schedule_system_shutdown(delay_secs: u64) -> Result<(), String> {
    std::process::Command::new("shutdown")
        .args(["/s", "/t", &delay_secs.to_string()])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("发起系统关机失败(可能需要管理员权限): {e}"))
}

/// 设置「允许自动关机」永久权(自关机能力):为 true 时关机动作免一次性授权窗、全自动。落库持久。
#[tauri::command]
pub async fn set_auto_shutdown_allowed(state: State<'_, SharedState>, allowed: bool) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.auto_shutdown_allowed = allowed;
    st.save_settings();
    Ok(())
}

/// 设置缓存预热开关(优化造物首次回复慢):连接后是否预 prefill 系统词+工具吃满 KV 缓存。落库持久。
#[tauri::command]
pub async fn set_cache_prewarm(state: State<'_, SharedState>, enabled: bool) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.cache_prewarm = enabled;
    st.save_settings();
    Ok(())
}

#[tauri::command]
pub async fn reset_chat_session(state: State<'_, SharedState>) -> Result<String, String> {
    let mut st = state.lock().await;
    let sid = format!("sess-{}", growbox_core::now().timestamp_millis());
    st.session_id = Some(sid.clone());
    Ok(sid)
}
