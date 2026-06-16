//! 调试 / E2E 端点 —— 仅在 `debug-endpoints` feature 下编译。
//!
//! 正式包(`cargo tauri build`)不带此 feature,本模块整体不编译:
//! 没有 127.0.0.1:19999 端口,也不注册 debug_eval / e2e_report 等 IPC。
//! 测试包(`cargo tauri build --features debug-endpoints`)才把这些端口装进去。
//!
//! 所有调试能力集中在本文件,不再散落于 cmds.rs / main.rs / lib.rs。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// 前端 eval 结果回报通道。debug_eval(IPC)与 HTTP `/eval` 共用:
/// 注入 JS 前放入 sender,前端执行完调 `e2e_report` 把结果送回。
pub static E2E_REPORT: std::sync::OnceLock<
    tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<String>>>,
> = std::sync::OnceLock::new();

// ===================== Tauri IPC 命令 =====================

/// 探活:前端确认后端调试通道在线。
#[tauri::command]
pub async fn get_debug_ping() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({ "ok": true }))
}

/// 占位:截屏 / 抓帧入口(测试构建里由外部脚本填充)。
#[tauri::command]
pub async fn debug_capture() -> Result<String, String> {
    Ok(String::new())
}

/// 前端把自测结果回报给后端(写临时文件 + 唤醒等待的 debug_eval/HTTP)。
#[tauri::command]
pub async fn receive_test_result() -> Result<(), String> {
    Ok(())
}

/// 前端 eval 结果汇报目标:写临时文件供外部脚本读,并经 oneshot 唤醒等待方。
#[tauri::command]
pub async fn e2e_report(result: String) -> Result<(), String> {
    let report_path = std::env::temp_dir().join("growbox_e2e_result.json");
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&result) {
        let _ = std::fs::write(&report_path, serde_json::to_string_pretty(&v).unwrap_or_default());
        eprintln!("[E2E] 结果已写入 {}", report_path.display());
    }
    if let Some(mu) = E2E_REPORT.get() {
        if let Some(tx) = mu.lock().await.take() {
            let _ = tx.send(result);
        }
    }
    Ok(())
}

/// 注入 JS 到 webview 并等前端回报结果(最多 25s)。供 IPC 调用方使用。
/// Promise-aware:函数体返回 Promise(如 __GROWBOX__.screenshot()/waitFor())会被 await 后再回报,
/// 同步返回值则原样直通——全自动调试模式(截图/等待条件)靠这个。
#[tauri::command]
pub async fn debug_eval(app: AppHandle, js: String) -> Result<String, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mu = E2E_REPORT.get_or_init(|| tokio::sync::Mutex::new(None));
    *mu.lock().await = Some(tx);

    let w = app.get_webview_window("main").ok_or("找不到 main 窗口")?;
    // body(函数体语句,含 return)经 JSON 字符串字面量喂 new Function:任意引号/反斜杠/换行
    // 都安全,且 body 语法错误走 catch 回报——绝不静默超时(真机踩过:转义把合法 JS 改坏,
    // eval 整段解析失败,e2e_report 永远不来)。
    let lit = serde_json::to_string(&js).map_err(|e| e.to_string())?;
    let wrapped = format!(
        "(function(){{ try {{ var __f = new Function({lit}); Promise.resolve(__f()).then(function(r){{ window.__TAURI__.core.invoke('e2e_report', {{ result: String(JSON.stringify(r)) }}); }}).catch(function(e){{ window.__TAURI__.core.invoke('e2e_report', {{ result: 'ERROR:' + String(e) }}); }}); }} catch(e) {{ window.__TAURI__.core.invoke('e2e_report', {{ result: 'ERROR:' + String(e) }}); }} }})()"
    );
    w.eval(&wrapped).map_err(|e| format!("eval 失败: {e}"))?;

    // 180s:容纳慢的后端直驱命令(debug_propose_skill 是真 LLM 调用、首个 e5 嵌入要加载模型)。
    let result = tokio::time::timeout(Duration::from_secs(180), rx)
        .await
        .map_err(|_| "超时:前端未在 180s 内回报结果".to_string())?
        .unwrap_or_else(|_| "null".into());
    Ok(result)
}

// ===================== HTTP 调试服务器(127.0.0.1:19999) =====================

/// 起后台线程监听 127.0.0.1:19999,接受 `curl POST /eval`(JS body)/ `GET /health`。
/// 仅本模块编译时(即带 feature 时)被 main.rs 调起。
pub fn start_server(app_handle: AppHandle) {
    std::thread::spawn(move || {
        let listener = match TcpListener::bind("127.0.0.1:19999") {
            Ok(l) => {
                eprintln!("[debug] http://127.0.0.1:19999");
                l
            }
            Err(_) => return,
        };
        for stream in listener.incoming() {
            let app = app_handle.clone();
            std::thread::spawn(move || {
                if let Ok(mut s) = stream {
                    s.set_read_timeout(Some(Duration::from_secs(10))).ok();
                    serve(&mut s, &app);
                }
            });
        }
    });
}

fn serve(stream: &mut std::net::TcpStream, app: &AppHandle) {
    let mut r = BufReader::new(&*stream);
    let mut req = String::new();
    if r.read_line(&mut req).is_err() {
        return;
    }

    let mut body_len = 0usize;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).is_err() {
            return;
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
            body_len = v.trim().parse().unwrap_or(0);
        }
    }

    let mut body = vec![0u8; body_len];
    if body_len > 0 {
        let _ = r.read_exact(&mut body);
    }
    let js = String::from_utf8_lossy(&body).to_string();

    let (code, resp) = if req.starts_with("POST /eval") {
        match run_js(&js, app) {
            Ok(r) => ("200 OK", r),
            Err(e) => ("500 ERROR", format!("{{\"error\":\"{e}\"}}")),
        }
    } else if req.starts_with("GET /health") {
        ("200 OK", r#"{"status":"ok"}"#.into())
    } else {
        ("200 OK", "POST /eval  <- JS body\nGET /health".into())
    };

    let _ = write!(
        stream,
        "HTTP/1.1 {code}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n{resp}",
        resp.len()
    );
}

// ===================== 后端直驱调试命令(测试 4 块新能力,不靠 LLM 轮盘) =====================
//
// 这些命令让外部(curl /eval 里 invoke,或前端)能**确定性**地驱动新落地的后端逻辑,在真机环境
// (真 Clash 系统代理 / 真默认嵌入器 / 真安全门)里逐路径验证,免去"靠 LLM 恰好调对工具"的不确定。
// 全部仅 debug-endpoints feature 编译,正式包不含。

/// 把一个工具经**真分发脊柱**(安全门 + 执行器)跑一遍,返回结构化结果。
/// 用途:确定性测 web_fetch / web_search / SSRF / 本机授权 / 系统代理 no_proxy 修复,不用等 LLM 调。
/// `authorized=true` 走 `dispatch_authorized`(把 NeedAuth 当 Allow,模拟用户已授权后的重放)。
#[tauri::command]
pub async fn debug_dispatch(
    state: tauri::State<'_, crate::cmds::SharedState>,
    tool: String,
    args: String,
    authorized: bool,
) -> Result<serde_json::Value, String> {
    let st = state.lock().await;
    let call = growbox_core::ToolCall { id: "debug".to_string(), name: tool, arguments: args };
    let dispatch = if authorized {
        st.registry.dispatch_authorized(&call, &st.sandbox, st.work_dir.as_path(), None).await
    } else {
        st.registry.dispatch_with_cancel(&call, &st.sandbox, st.work_dir.as_path(), None).await
    };
    use crate::registry::Dispatch;
    let v = match dispatch {
        Dispatch::Done(r) => serde_json::json!({ "kind": "done", "ok": r.ok, "content": r.content }),
        Dispatch::Terminal(r) => serde_json::json!({ "kind": "terminal", "ok": r.ok, "content": r.content }),
        Dispatch::AwaitingUser(r) => serde_json::json!({ "kind": "awaiting_user", "ok": r.ok, "content": r.content }),
        Dispatch::Intent(i) => serde_json::json!({ "kind": "intent", "action": i.action }),
        Dispatch::NeedAuth { reason, claim } => {
            serde_json::json!({ "kind": "need_auth", "reason": reason, "claim": format!("{claim:?}") })
        }
        Dispatch::Denied { reason } => serde_json::json!({ "kind": "denied", "reason": reason }),
    };
    Ok(v)
}

/// 直接调"分发前会诊"(`consult_tool_memory`),返回 verdict + 真实相似度 + 命中记忆全文。
/// 用途:在真默认嵌入器下**校准** veto/warn 阈值——0.85 余弦在 LexicalEmbedder 下是否够得着?
#[tauri::command]
pub async fn debug_consult_tool_memory(
    state: tauri::State<'_, crate::cmds::SharedState>,
    tool: String,
    situation: String,
) -> Result<serde_json::Value, String> {
    let mut st = state.lock().await;
    let bridge = st.bridge.clone().ok_or("未连接(无 bridge,先 connect)")?;
    let count = st.memory.tool_memory_count();
    let res = st.memory.consult_tool_memory(&tool, &situation, bridge.as_ref()).await;
    Ok(match res {
        Some((v, content, score)) => serde_json::json!({
            "found": true, "verdict": v.as_str(), "score": score, "content": content, "tool_memory_count": count
        }),
        None => serde_json::json!({ "found": false, "tool_memory_count": count }),
    })
}

/// 确定性地结晶一条工具记忆节点(即时嵌入)。用途:为 veto 测试预置"已知不可行"小本本,
/// 免去靠 LLM 恰好调 note_tool_memory。`verdict` 宽松解析(infeasible/fails/works/中文同义词)。
#[tauri::command]
pub async fn debug_seed_tool_memory(
    state: tauri::State<'_, crate::cmds::SharedState>,
    tool: String,
    situation: String,
    verdict: String,
    detail: String,
) -> Result<String, String> {
    let mut st = state.lock().await;
    let bridge = st.bridge.clone().ok_or("未连接(无 bridge,先 connect)")?;
    let v = growbox_memory::tool_memory_format::Verdict::parse(&verdict);
    let id = st
        .memory
        .crystallize_tool_memory(&tool, &situation, v, &detail, bridge.as_ref())
        .await;
    Ok(id)
}

/// 用一簇合成经验直接调真 LLM 起草 skill(`Reasoner::propose_skill`),返回草稿或 `{proposed:false}`。
/// 用途:验证 S3 飞轮自学的**起草质量**(LLM 是否从反复经验里抽出可复用 playbook / 噪音是否返回 none),
/// 免去先攒够 ≥3 真经验簇再等 8 分钟 idle。
#[tauri::command]
pub async fn debug_propose_skill(
    state: tauri::State<'_, crate::cmds::SharedState>,
    ops: Vec<String>,
) -> Result<serde_json::Value, String> {
    use growbox_learn::Reasoner;
    let bridge = {
        let st = state.lock().await;
        st.bridge.clone().ok_or("未连接(无 bridge,先 connect)")?
    };
    let fw = growbox_learn::Flywheel::new();
    let cluster: Vec<growbox_core::Conclusion> = ops
        .iter()
        .map(|op| fw.collect(growbox_learn::Snapshot::new(op.clone(), "成功", true)))
        .collect();
    match bridge.propose_skill(&cluster).await {
        Some(p) => Ok(serde_json::json!({
            "proposed": true, "name": p.name, "trigger": p.trigger, "body": p.body
        })),
        None => Ok(serde_json::json!({ "proposed": false })),
    }
}

fn run_js(js: &str, app: &AppHandle) -> Result<String, String> {
    let w = app.get_webview_window("main").ok_or("no main window")?;

    let report_lock = E2E_REPORT.get_or_init(|| tokio::sync::Mutex::new(None));
    let (tx, rx) = tokio::sync::oneshot::channel();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async { *report_lock.lock().await = Some(tx); });

    // Promise-aware(同 debug_eval);body 是"表达式",经 JSON 字面量 + new Function 注入:
    // 任意引号/反斜杠/多行都安全,语法错误走 catch 回报,绝不静默超时。旧的手工转义
    // ('→\\' 等)是 body 曾嵌在引号串里的遗物,对"代码位"插入反而制造语法错误,已废除。
    let lit = serde_json::to_string(js).map_err(|e| e.to_string())?;
    let wrapped = format!(
        "(function(){{ try {{ var __f = new Function('return (' + {lit} + '\\n);'); Promise.resolve(__f()).then(function(r){{ window.__TAURI__.core.invoke('e2e_report',{{result:String(JSON.stringify(r))}}); }}).catch(function(e){{ window.__TAURI__.core.invoke('e2e_report',{{result:'ERROR:'+String(e)}}); }}); }} catch(e) {{ window.__TAURI__.core.invoke('e2e_report',{{result:'ERROR:'+String(e)}}); }} }})()"
    );
    w.eval(&wrapped).map_err(|e| format!("eval: {e}"))?;

    let result = rt.block_on(async {
        // 180s:容纳慢的后端直驱命令(debug_propose_skill 真 LLM 调用 / 首个 e5 嵌入加载模型)。
        match tokio::time::timeout(Duration::from_secs(180), rx).await {
            Err(_) => Err("超时(180s)".to_string()),
            Ok(Err(_)) => Ok("null".to_string()),
            Ok(Ok(v)) => Ok(v),
        }
    })?;
    Ok(result)
}
