//! 网页调试(Phase 2)—— 把"正在跑的本地 web 应用"(URL/dev server)拉进可导航的 Tauri
//! WebviewWindow,注入套索运行时(框选→DOM 快照),框选结果经**导航到哨兵 scheme** 回传。
//!
//! 为什么回传不用 fetch / IPC:被调试页常自带 CSP(实测博客 helmet `default-src 'self'`),
//! `connect-src` 把跨源 fetch/XHR/WebSocket 全拦掉(fetch 报 `TypeError: Load failed`);Tauri 对
//! `WebviewUrl::External` 的 IPC 也不可靠(#15190)。而 **CSP 的 connect-src 管不着页面导航** ——
//! 故注入脚本把框选结果塞进 `gxlasso://x/?d=<JSON>` 触发一次导航,由 `on_navigation` 拦下、取出数据、
//! emit 给主窗前端(→ doSend 起改源回合,复用 Phase 1),并返回 false 取消导航(页面不动)。
//! 任何带 CSP 的被调试页都穿得过。计划/网页调试窗-可视化框选改源.md Phase 2。

use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// 注入被调试页面的套索运行时(initialization_script)。
const RUNTIME_JS: &str = include_str!("web_debug_runtime.js");

/// ★QA 自反馈调试★:`web_debug_drive` 的观察/扫描结果回传通道。被测页导航到 `gxlasso://act/?d=<JSON>`
/// 时,`on_navigation`(sync 回调)取出 d 兑现此 oneshot。用 std Mutex(`oneshot::send` 本身非阻塞、sync)。
static DRIVE_REPORT: OnceLock<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>> =
    OnceLock::new();

/// 打开/复用调试 webview,导航到 url,注入套索运行时。
/// 由前端 dispatchUiAction("open_debug_url") → invoke 调用(主窗本地调用,不踩远程 IPC 雷区)。
#[tauri::command]
pub async fn create_debug_webview(app: AppHandle, url: String) -> Result<(), String> {
    let url = url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("只允许 http(s) URL(本地 dev server,如 http://localhost:3000)".into());
    }
    let parsed = tauri::Url::parse(&url).map_err(|e| format!("URL 解析失败: {e}"))?;
    let label = "web-debug";
    if let Some(existing) = app.get_webview_window(label) {
        // 已有调试窗 → 直接导航(initialization_script 每次导航重跑,套索运行时随之重注入)。
        existing
            .navigate(parsed)
            .map_err(|e| format!("导航失败: {e}"))?;
        let _ = existing.set_focus();
        // 诊断:打开 Web Inspector(仅测试包/调试构建;正式 release 包编译期剔除)。
        #[cfg(any(debug_assertions, feature = "debug-endpoints"))]
        existing.open_devtools();
        return Ok(());
    }
    let app_for_nav = app.clone();
    let win = tauri::WebviewWindowBuilder::new(&app, label, tauri::WebviewUrl::External(parsed))
        .title("网页调试")
        .inner_size(1200.0, 820.0)
        .initialization_script(RUNTIME_JS)
        .devtools(cfg!(any(debug_assertions, feature = "debug-endpoints")))
        .on_navigation(move |u| {
            // ★哨兵回传通道★(CSP connect-src 管不着导航 → 任何带 CSP 的被测页都能回传;fetch 会被拦):
            //   gxlasso://x/   = 套索框选改源 → emit web-debug-edit 给主窗(Phase 2)。
            //   gxlasso://act/ = QA 自反馈观察/扫描结果 → 兑现 DRIVE_REPORT oneshot(供 web_debug_drive 返回)。
            if u.scheme() == "gxlasso" {
                let d = u.query_pairs().find(|(k, _)| k == "d").map(|(_, v)| v.into_owned());
                if u.host_str() == Some("act") {
                    if let (Some(d), Some(mu)) = (d, DRIVE_REPORT.get()) {
                        if let Ok(mut g) = mu.lock() {
                            if let Some(tx) = g.take() {
                                let _ = tx.send(d);
                            }
                        }
                    }
                } else if let Some(d) = d {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&d) {
                        let _ = app_for_nav.emit("web-debug-edit", v);
                    }
                }
                return false; // 取消导航:页面不动
            }
            true
        })
        .build()
        .map_err(|e| format!("创建调试窗失败: {e}"))?;
    // 诊断:打开 Web Inspector(仅测试包/调试构建;正式 release 包编译期剔除)。
    #[cfg(any(debug_assertions, feature = "debug-endpoints"))]
    win.open_devtools();
    Ok(())
}

/// AI 改完源码后刷新调试 webview。EJS/Express 等**无 HMR** 的工程靠这个才看得到改动(下次请求才渲新内容,
/// 但页面不会自己刷);Vite 工程本就 HMR、多刷一下无害。eval 经 Tauri 原生注入,不受页面 CSP 限制。
/// ★cache-busting★:纯 location.reload() 在 WKWebView 可能命中缓存看不到改动 → 在 URL 换 _gxr 时间戳
/// 参数强制服务端重取(与 runtime.js 的「⟳刷新」按钮同策略;EJS/Express 忽略未知 query、SPA 无害)。
#[tauri::command]
pub async fn reload_debug_webview(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("web-debug") {
        let js = "try{var u=new URL(location.href);u.searchParams.set('_gxr',String(Date.now()));location.href=u.toString();}catch(e){location.reload();}";
        win.eval(js).map_err(|e| format!("刷新失败: {e}"))?;
    }
    Ok(())
}

/// ★QA 自反馈调试:真模拟操作 + 观察★(从"枚举回调"升级到"真点 / 真填 / 真提交 + 读结果")。
/// AI/主窗调:在调试 webview 里真做一个动作(`click`/`fill`/`submit`)或 `scan`(枚举交互点)/`observe`(读当前页),
/// 然后**读回结果**(url/title/本页报错)→ 据此判"按钮跳转对不对 / 表单提交后到没到对的页 / 有没有报错"。
/// 动作是 fire-and-forget(可能触发真导航)→ 等一拍让导航/DOM 反应,再在**导航后的当前页**单独 `observe()`
/// 经 `gxlasso://act` 哨兵回传(穿 CSP)。返回观察 JSON。是自反馈调试闭环的"手"(脑=skill,良心=金融闸工作流)。
#[tauri::command]
pub async fn web_debug_drive(
    app: AppHandle,
    op: String,
    selector: String,
    value: String,
) -> Result<serde_json::Value, String> {
    let win = app
        .get_webview_window("web-debug")
        .ok_or("没有调试窗(先用「调试网站」打开一个 URL)")?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let m = DRIVE_REPORT.get_or_init(|| std::sync::Mutex::new(None));
        *m.lock().map_err(|_| "drive 通道锁毒".to_string())? = Some(tx);
    }
    // selector/value 经 JSON 字面量注入,任意引号/反斜杠都安全。
    let sel = serde_json::to_string(&selector).unwrap_or_else(|_| "\"\"".into());
    let val = serde_json::to_string(&value).unwrap_or_else(|_| "\"\"".into());
    let action_js = match op.as_str() {
        "click" => format!("window.__gxqa&&window.__gxqa.click({sel});"),
        "fill" => format!("window.__gxqa&&window.__gxqa.fill({sel},{val});"),
        "submit" => format!("window.__gxqa&&window.__gxqa.submit({sel});"),
        "scan" => "window.__gxqa&&window.__gxqa.scan();".to_string(),
        "observe" => "window.__gxqa&&window.__gxqa.observe();".to_string(),
        other => return Err(format!("未知操作: {other}(支持 click/fill/submit/scan/observe)")),
    };
    win.eval(&action_js).map_err(|e| format!("eval 动作失败: {e}"))?;
    // click/fill/submit 是动作(可能触发导航)→ 等一拍,再在导航后的当前页 observe 回传;scan/observe 已自回传。
    if matches!(op.as_str(), "click" | "fill" | "submit") {
        tokio::time::sleep(Duration::from_millis(1200)).await;
        let _ = win.eval("window.__gxqa&&window.__gxqa.observe();");
    }
    let raw = tokio::time::timeout(Duration::from_secs(8), rx)
        .await
        .map_err(|_| "观察回传超时(8s)".to_string())?
        .map_err(|_| "观察回传通道断".to_string())?;
    serde_json::from_str::<serde_json::Value>(&raw).map_err(|e| format!("观察结果解析失败: {e}"))
}
