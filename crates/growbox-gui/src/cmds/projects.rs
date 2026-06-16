//! 项目命令:增删切项目 + 目录授权 + 网络主机授权 + 原生目录选择器。

use super::*;

/// 用户在权限弹窗确认内网/本机访问后的持久化(决定脊柱 net 授权,见 PermissionDialog access="net"):
/// 落当前项目 `net_grants` + 当前沙箱当场生效。入参可是 URL 或裸主机名,返回实际授权的主机。
#[tauri::command]
pub async fn grant_net_host(state: State<'_, SharedState>, host: String) -> Result<String, String> {
    state.lock().await.grant_net_host(&host)
}

// ===================== 项目 =====================

#[derive(Deserialize)]
pub struct CreateProjectArgs {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub writable: Vec<String>,
    #[serde(default)]
    pub readonly: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[tauri::command]
pub async fn list_projects(state: State<'_, SharedState>) -> Result<Vec<Value>, String> {
    let st = state.lock().await;
    let (exp, kno, und) = conclusion_counts(&st);
    let cur = st.current.clone();
    Ok(st
        .projects
        .iter()
        .map(|p| {
            let is_cur = Some(&p.id) == cur.as_ref();
            json!({
                "id": p.id,
                "name": p.name,
                "archived": false,
                "writable": p.writable_roots,
                "readonly": p.readonly_roots,
                "experience_count": if is_cur { exp } else { 0 },
                "knowledge_count": if is_cur { kno } else { 0 },
                "understanding_count": if is_cur { und } else { 0 },
            })
        })
        .collect())
}

#[tauri::command]
pub async fn current_project(state: State<'_, SharedState>) -> Result<Option<String>, String> {
    Ok(state.lock().await.current.clone())
}

#[tauri::command]
pub async fn switch_project(state: State<'_, SharedState>, app: AppHandle, id: String) -> Result<Value, String> {
    let mut st = state.lock().await;
    if !st.switch_project(&id) {
        return Err(format!("项目不存在: {id}"));
    }
    let (name, work_dir) = st
        .current_project()
        .map(|p| (p.name.clone(), st.work_dir.display().to_string()))
        .unwrap_or_default();
    let _ = app.emit("project-switched", json!({ "id": id, "name": name, "work_dir": work_dir }));
    Ok(json!({ "ok": true, "id": id }))
}

#[tauri::command]
pub async fn create_project(state: State<'_, SharedState>, app: AppHandle, args: CreateProjectArgs) -> Result<String, String> {
    let mut st = state.lock().await;
    let writable = args.writable.iter().map(Into::into).collect();
    let readonly = args.readonly.iter().map(Into::into).collect();
    // 前端传来用户指定的 id;为空则后端自动生成
    let user_id = if args.id.is_empty() { None } else { Some(args.id.as_str()) };
    let id = st.create_project(user_id, args.name.clone(), writable, readonly);
    let work_dir = st.work_dir.display().to_string();
    let _ = app.emit("project-switched", json!({ "id": id, "name": args.name, "work_dir": work_dir }));
    Ok(id)
}

#[tauri::command]
pub async fn get_project_directories(state: State<'_, SharedState>, id: Option<String>) -> Result<Option<Value>, String> {
    let st = state.lock().await;
    let target = id.or_else(|| st.current.clone());
    let Some(target) = target else { return Ok(None) };
    Ok(st.projects.iter().find(|p| p.id == target).map(|p| {
        json!({
            "id": p.id,
            "name": p.name,
            "writable": p.writable_roots,
            "readonly": p.readonly_roots,
            "work_dir": st.work_dir.display().to_string(),
        })
    }))
}

#[derive(Deserialize)]
pub struct UpdateDirsArgs {
    pub id: String,
    #[serde(default)]
    pub writable: Vec<String>,
    #[serde(default)]
    pub readonly: Vec<String>,
}

#[tauri::command]
pub async fn update_project_directories(state: State<'_, SharedState>, args: UpdateDirsArgs) -> Result<Value, String> {
    let mut st = state.lock().await;
    let Some(p) = st.projects.iter_mut().find(|p| p.id == args.id) else {
        return Err(format!("项目不存在: {}", args.id));
    };
    p.writable_roots = args.writable.iter().map(Into::into).collect();
    p.readonly_roots = args.readonly.iter().map(Into::into).collect();
    st.save_projects();
    // 当前项目改了目录 → 重建沙箱。
    if st.current.as_deref() == Some(args.id.as_str()) {
        st.switch_project(&args.id);
    }
    Ok(json!({ "ok": true }))
}

/// macOS 原生目录选择器(无需 dialog 插件,走 osascript)。
#[tauri::command]
pub async fn pick_directory() -> Result<Option<String>, String> {
    let out = tokio::task::spawn_blocking(|| {
        std::process::Command::new("osascript")
            .arg("-e")
            .arg("POSIX path of (choose folder)")
            .output()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    if out.status.success() {
        let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok((!p.is_empty()).then_some(p))
    } else {
        Ok(None) // 用户取消
    }
}

/// 按压缩率把活跃结论分成经验/知识/理解三档计数。
fn conclusion_counts(st: &AppState) -> (usize, usize, usize) {
    let mut exp = 0;
    let mut kno = 0;
    let mut und = 0;
    for c in st.memory.conclusions().iter().filter(|c| c.is_active()) {
        if c.compression == 0.0 {
            exp += 1;
        } else if c.compression < 0.7 {
            kno += 1;
        } else {
            und += 1;
        }
    }
    (exp, kno, und)
}
