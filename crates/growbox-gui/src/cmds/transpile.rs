//! 提示词自转译命令(自我负责-输入侧,`设计/08-自我负责.md` 推论2 + `计划/提示词自转译.md`)。
//!
//! - `transpile_prompts`:用"消费该提示词的那个模型"把所有模型可见提示词逐条重写、保真校验后落覆盖层。
//!   成本高(每条一次重写 + 一次语义复核 LLM 调用)→ 前端按需触发、可中断、有进度;**不持锁跑 LLM**
//!   (先加锁快速收集,放锁后再循环调模型),避免长任务把全局状态锁占住卡死聊天/idle。
//! - `set_transpile_enabled`:仅翻开关(即时生效,不必重连)+ 落库。
//! - `get_transpile_status`:回显开关 + 覆盖条数 + 当前模型(前端面板用)。

use super::*;
use growbox_llm::{ChatMessage, ChatRequest};

/// 设提示词自转译开关(推论9/可关可还原)。即时生效(运行时取用层立刻按新值走覆盖或原文)+ 落库。
#[tauri::command]
pub async fn set_transpile_enabled(state: State<'_, SharedState>, enabled: bool) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.prompt_transpile = enabled;
    crate::transpile::set_enabled(enabled);
    st.save_settings();
    Ok(())
}

/// 回显自转译状态(Settings.tsx 面板加载时取):开关 + 覆盖条数 + 当前主/潜意识模型。
#[tauri::command]
pub async fn get_transpile_status(state: State<'_, SharedState>) -> Result<Value, String> {
    let st = state.lock().await;
    Ok(json!({
        "enabled": crate::transpile::is_enabled(),
        "override_count": crate::transpile::override_count(),
        "model": st.settings.model,
        "connected": st.connected,
        "concurrency": st.settings.transpile_concurrency,
    }))
}

/// 设转译并发数(「用当前模型重写」时同时在飞的请求数)。落库;下次重写即用。
#[tauri::command]
pub async fn set_transpile_concurrency(state: State<'_, SharedState>, concurrency: u32) -> Result<(), String> {
    let mut st = state.lock().await;
    st.settings.transpile_concurrency = concurrency.max(1);
    st.save_settings();
    Ok(())
}

/// 一条转译扫描单元在循环里需要的最小信息(脱离锁,避免持锁跑 LLM)。
struct Job {
    key: String,
    lang: &'static str,
    model: String,
    original: String,
}

/// ★用当前模型重写提示词★:谁消费谁转译(主模型转 Main 项、潜意识模型转 Subconscious 项;
/// 今天二者同一个 model)。逐条 = 重写 → 机械保真(占位符/长度)→ 语义复核(问同模型有没有漏约束)
/// → 过则写覆盖;不过保留原文。覆盖按(模型,语言,键)分桶合并进既有表(别的模型旧覆盖保留),落 redb + 刷新内存。
#[tauri::command]
pub async fn transpile_prompts(app: AppHandle, state: State<'_, SharedState>) -> Result<Value, String> {
    // ---- 1) 加锁快速收集(不在锁内调 LLM)----
    let (driver, jobs, mut overrides, store, silence, model_label, concurrency) = {
        let st = state.lock().await;
        let driver = st.llm.clone().ok_or("尚未连接 LLM,请先在设置里连接")?;
        // 今天潜意识 == 主模型(同一个 model);将来拆独立潜意识模型时,这里按角色取各自 id。
        let main_model = st.settings.model.clone();
        let sub_model = st.settings.model.clone();
        if main_model.trim().is_empty() {
            return Err("未配置模型,无法转译".into());
        }
        // 合并基 = 版本库当前激活版的覆盖表(别的模型/语言的旧覆盖保留不动);重写只更新本模型本语言的键。
        let overrides = crate::transpile::overrides_snapshot();
        let store = st.store.clone();
        let model_label = main_model.clone();
        // 收集扫描单元:静态目录(self_verify + judge/distill)+ 工具 llm_desc + 系统提示(zh+en)。
        let mut units = crate::transpile::static_units();
        units.extend(st.registry.tool_desc_units());
        if let Some(rd) = st.resource_dir.clone() {
            for lang in ["zh", "en"] {
                let original = crate::state::AppState::load_agent_prompt(&rd, lang);
                if !original.trim().is_empty() {
                    units.push(crate::transpile::Unit {
                        key: "agent.system".to_string(),
                        role: crate::transpile::PromptRole::Main,
                        lang,
                        original,
                    });
                }
            }
        }
        // 解析每项消费模型,折叠成脱锁可用的 Job。
        let jobs: Vec<Job> = units
            .into_iter()
            .map(|u| {
                let model = match u.role {
                    crate::transpile::PromptRole::Main => main_model.clone(),
                    crate::transpile::PromptRole::Subconscious => sub_model.clone(),
                };
                Job { key: u.key, lang: u.lang, model, original: u.original }
            })
            .collect();
        // 转译输出可能比原文长,给足沉默超时(至少 60s)。
        let silence = (st.settings.complete_silence_secs.max(60)) as u64;
        let concurrency = st.settings.transpile_concurrency.max(1) as usize;
        (driver, jobs, overrides, store, silence, model_label, concurrency)
    };

    // ---- 2) 放锁后重写。★缓存暖机 + 并发★(见下)----
    use std::sync::atomic::Ordering::SeqCst;
    let total = jobs.len();
    // 先发一条"起手"进度(总数已知),免得前端在第一条完成前显示 0/0。
    let _ = app.emit(
        "transpile-progress",
        json!({ "done": 0, "total": total, "key": "", "lang": "", "finished": false }),
    );
    let mut written = 0usize;
    let mut skipped = 0usize;
    let done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    // ★Deepseek 缓存特性(用户 2026-06-08)★:每条转译的常量 system 前缀(rewrite_system / 语义复核指令)
    // 第一次发才被缓存住。若一上来就并发,头一批会在前缀尚未缓存时同时发出 → 各 miss 一次。
    // 故先**每个语言各取 1 条串行跑**(zh+en,把两份 system 前缀坐实进缓存),再放并发 → 余下请求全命中前缀。
    // 对无前缀缓存的模型(非 Deepseek):只是把 2 条放前面串行,无害(见 `计划/提示词自转译.md`)。
    let mut warmup: Vec<Job> = Vec::new();
    let mut rest: Vec<Job> = Vec::new();
    {
        let mut warmed: std::collections::HashSet<&'static str> = std::collections::HashSet::new();
        for job in jobs {
            if warmed.len() < 2 && warmed.insert(job.lang) {
                warmup.push(job);
            } else {
                rest.push(job);
            }
        }
    }
    // 暖机:串行(坐实 system 前缀缓存)。
    for job in &warmup {
        let r = process_job(driver.as_ref(), job, silence).await;
        let i = done.fetch_add(1, SeqCst) + 1;
        let _ = app.emit(
            "transpile-progress",
            json!({ "done": i, "total": total, "key": job.key, "lang": job.lang, "finished": false }),
        );
        match r {
            Some((k, v)) => { overrides.insert(k, v); written += 1; }
            None => skipped += 1,
        }
    }
    // 余下:并发(此时 system 前缀已暖,Deepseek 全命中)。
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut set = tokio::task::JoinSet::new();
    for job in rest {
        let sem = sem.clone();
        let driver = driver.clone();
        let app = app.clone();
        let done = done.clone();
        set.spawn(async move {
            let _permit = sem.acquire().await.ok()?; // 信号量关闭 → 放弃该条
            let r = process_job(driver.as_ref(), &job, silence).await;
            let i = done.fetch_add(1, SeqCst) + 1;
            let _ = app.emit(
                "transpile-progress",
                json!({ "done": i, "total": total, "key": job.key, "lang": job.lang, "finished": false }),
            );
            r
        });
    }
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Some((k, v))) => { overrides.insert(k, v); written += 1; }
            _ => skipped += 1,
        }
    }

    // ---- 3) 存成新版本(可后悔:不覆盖旧版,存进「历史提示词」并置为激活)+ 刷新内存覆盖表 ----
    let mut snapshot_name = String::new();
    if let Some(s) = &store {
        let meta = crate::transpile_store::add_version(s, &model_label, &overrides);
        snapshot_name = meta.name;
    }
    let nonempty = !overrides.is_empty();
    crate::transpile::set_overrides(overrides);
    crate::transpile::set_enabled(nonempty); // 有覆盖即生效(选区=默认/原文时才空=关)
    let _ = app.emit(
        "transpile-progress",
        json!({ "done": total, "total": total, "written": written, "skipped": skipped, "finished": true }),
    );
    Ok(json!({ "total": total, "written": written, "skipped": skipped, "snapshot": snapshot_name }))
}

/// 列出「历史提示词」版本(新→旧)+ 当前激活 id(default = 原文)。
#[tauri::command]
pub async fn transpile_list_snapshots(state: State<'_, SharedState>) -> Result<Value, String> {
    let st = state.lock().await;
    let (metas, active) = st
        .store
        .as_ref()
        .map(crate::transpile_store::list)
        .unwrap_or_else(|| (Vec::new(), crate::transpile_store::DEFAULT_ID.to_string()));
    Ok(json!({ "active": active, "snapshots": metas }))
}

/// 激活某历史版本(id=default → 还原到原文)。刷新内存覆盖表,即时生效。
#[tauri::command]
pub async fn transpile_activate_snapshot(state: State<'_, SharedState>, id: String) -> Result<(), String> {
    let st = state.lock().await;
    if let Some(s) = &st.store {
        let map = crate::transpile_store::activate(s, &id);
        let nonempty = !map.is_empty();
        crate::transpile::set_overrides(map);
        crate::transpile::set_enabled(nonempty); // 选区=默认(原文)→ 空 → 关;选其它包 → 开
    }
    Ok(())
}

/// 重命名一个历史版本(default 不可改)。
#[tauri::command]
pub async fn transpile_rename_snapshot(state: State<'_, SharedState>, id: String, name: String) -> Result<(), String> {
    let st = state.lock().await;
    if let Some(s) = &st.store {
        crate::transpile_store::rename(s, &id, &name);
    }
    Ok(())
}

/// 删除一个历史版本(default 拒删;删激活版则回落 default=原文)。刷新内存覆盖表。
#[tauri::command]
pub async fn transpile_delete_snapshot(state: State<'_, SharedState>, id: String) -> Result<(), String> {
    let st = state.lock().await;
    if let Some(s) = &st.store {
        let map = crate::transpile_store::delete(s, &id);
        let nonempty = !map.is_empty();
        crate::transpile::set_overrides(map);
        crate::transpile::set_enabled(nonempty);
    }
    Ok(())
}

/// 导出一个历史版本成磁盘 .zip 文件(写到 `<数据目录>/transpile_exports/<名>.zip`),返回绝对路径。
#[tauri::command]
pub async fn transpile_export_snapshot(state: State<'_, SharedState>, id: String) -> Result<String, String> {
    let st = state.lock().await;
    let store = st.store.as_ref().ok_or("存储不可用")?;
    let bytes = crate::transpile_store::export_zip(store, &id).ok_or("该版本不存在或导出失败")?;
    // 文件名:取版本名,去掉非法字符;空则用 id。
    let (metas, _) = crate::transpile_store::list(store);
    let raw_name = if id == crate::transpile_store::DEFAULT_ID {
        "default".to_string()
    } else {
        metas.iter().find(|m| m.id == id).map(|m| m.name.clone()).unwrap_or_else(|| id.clone())
    };
    let safe: String = raw_name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let dir = st.data_dir.join("transpile_exports");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建导出目录失败: {e}"))?;
    let path = dir.join(format!("{safe}.zip"));
    std::fs::write(&path, &bytes).map_err(|e| format!("写文件失败: {e}"))?;
    Ok(path.display().to_string())
}

/// 从前端上传的 .zip(base64 编码字节)导入成一个新版本(置激活)。刷新内存覆盖表。
#[tauri::command]
pub async fn transpile_import_snapshot(
    state: State<'_, SharedState>,
    name: String,
    data_b64: String,
) -> Result<Value, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_b64.trim())
        .map_err(|e| format!("base64 解码失败: {e}"))?;
    let st = state.lock().await;
    let store = st.store.as_ref().ok_or("存储不可用")?;
    let meta = crate::transpile_store::import_zip(store, &bytes, &name).ok_or(".zip 无效或缺 overrides.json")?;
    // 导入即激活 → 刷新全局取用层。
    let map = crate::transpile_store::activate(store, &meta.id);
    let nonempty = !map.is_empty();
    crate::transpile::set_overrides(map);
    crate::transpile::set_enabled(nonempty);
    Ok(json!({ "id": meta.id, "name": meta.name, "count": meta.count }))
}

/// 处理一条转译单元:重写(★缓存友好★ system=常量指令 + user=原文)→ 机械保真 + 语义复核;
/// 全过返回 `(okey, 重写文本)`,否则 None(保留原文)。暖机串行与并发都用它。
async fn process_job(
    driver: &dyn crate::bridge::LlmDriver,
    job: &Job,
    silence: u64,
) -> Option<(String, String)> {
    let req = ChatRequest::new(
        job.model.clone(),
        vec![
            ChatMessage::system(crate::transpile::rewrite_system(job.lang)),
            ChatMessage::user(job.original.clone()),
        ],
    );
    let rewrite = crate::bridge::complete(driver, req, silence).await.ok()?.trim().to_string();
    if crate::transpile::fidelity_ok(&job.original, &rewrite)
        && semantic_preserved(driver, &job.model, &job.original, &rewrite, job.lang, silence).await
    {
        Some((crate::transpile::okey(&job.model, job.lang, &job.key), rewrite))
    } else {
        None
    }
}

/// 语义保真复核:问同一模型「改写是否完整保留原文的全部约束/格式/占位符,既不漏也不增」。
/// 只回 是/否;调不通(infra 失败)按放行处理(机械门已守占位符/长度,不因网络抖动白丢一条)。
async fn semantic_preserved(
    driver: &dyn crate::bridge::LlmDriver,
    model: &str,
    original: &str,
    rewrite: &str,
    lang: &str,
    silence: u64,
) -> bool {
    // ★缓存友好★:判定指令做成常量 system,变化的原文/改写放 user。
    let sys = if crate::tool_i18n::normalize_prompt_lang(lang) == "en" {
        "You are given an original prompt and its rewrite (in the user message). Does the rewrite preserve ALL constraints, \
         instructions, output-format requirements and {placeholders} of the original - dropping nothing essential and adding \
         no new requirement? Answer with only one word: yes or no."
    } else {
        "user 消息里给你一段原始提示词和它的改写。改写是否完整保留了原文的全部约束、指令、输出格式要求和 {占位符}\
         ——既没漏掉要紧的、也没新增要求?只回一个字:是 或 否。"
    };
    let user = format!("原文:\n\"\"\"\n{original}\n\"\"\"\n\n改写:\n\"\"\"\n{rewrite}\n\"\"\"");
    let req = ChatRequest::new(model.to_string(), vec![ChatMessage::system(sys), ChatMessage::user(user)]);
    match crate::bridge::complete(driver, req, silence).await {
        Ok(ans) => {
            let a = ans.trim().to_lowercase();
            // 命中否定 → 不保真;否则(含明确肯定 / 模糊)放行。
            !(a.contains("no") || a.contains('否') || a.contains("not preserve"))
        }
        Err(_) => true, // infra 失败:放行(机械门已守)
    }
}
