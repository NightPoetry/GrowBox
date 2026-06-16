//! LSP 客户端(二期 A1):持久驱动语言服务器(rust-analyzer 等)做代码智能。
//! **机制与 UI 分离**:本模块是纯异步客户端,经集成测试直接验(不碰 GUI)。
//! 见 `设计文档/二期项目/项目设计/03-LSP集成.md`。
//!
//! 一条 `LspClient` = 一个常驻语言服务器连接(spawn 一次、复用):
//! - 写:请求/通知经 mpsc 串行送 writer 任务,Content-Length 分帧写 stdin。
//! - 读:reader 任务分帧读 stdout,响应按 id 路由到 oneshot,通知(诊断)暂存(A2 用)。
//! - 生命周期:initialize/initialized 握手;★查文件前必须先 didOpen 同步内容(LSP 有状态)★。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// ★A2 诊断轮询时序★(LSP 内部时序,同 REQUEST_TIMEOUT 为 const):编辑 .rs 后同步给已暖的
// rust-analyzer,等它重发该文件的诊断。仅在客户端已起且已索引(AI 先前用过 lsp)时调,故短预算够用。
/// 等"同步后首波诊断发布"的预算上限(暖客户端通常亚秒级;超时则按"暂无诊断"诚实告知)。
/// 脊柱(agent::perceive_rust_diagnostics)用,故 pub(crate)。
pub(crate) const DIAG_POLL_BUDGET: Duration = Duration::from_millis(2500);
/// 轮询步长。
const DIAG_POLL_STEP: Duration = Duration::from_millis(150);
/// 收到非空诊断后再等一小段,接住 flycheck/二次分析的后续诊断(仍有界)。
const DIAG_FLYCHECK_GRACE: Duration = Duration::from_millis(400);
/// 已收到"空诊断发布"后,持续为空超过此窗口 = 判定干净(避免干净文件干等满 budget;
/// 也是给 didOpen 重分析的迟到 error 波留的窗口——错误须在首次空发布后此窗口内到达)。
const DIAG_EMPTY_SETTLE: Duration = Duration::from_millis(900);

type Pending = Arc<Mutex<HashMap<i64, oneshot::Sender<Value>>>>;
type DiagMap = Arc<Mutex<HashMap<String, Vec<Value>>>>;

/// 一条持久语言服务器连接。
pub struct LspClient {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    next_id: AtomicI64,
    pending: Pending,
    /// uri -> diagnostics(A2 诊断推感知层用)。
    diagnostics: DiagMap,
    _child: Child, // 持有,drop 即杀进程
}

impl LspClient {
    /// 起一个语言服务器并完成 initialize 握手。`server_cmd` = 二进制路径;`root` = 工作区根。
    /// 便捷版(无额外参数,如 rust-analyzer)。
    pub async fn start(server_cmd: &str, root: &Path) -> Result<Self, String> {
        Self::start_with_args(server_cmd, &[], root).await
    }

    /// 起一个语言服务器(带启动参数,如 typescript-language-server 需 `--stdio`)并完成 initialize 握手。
    pub async fn start_with_args(server_cmd: &str, args: &[&str], root: &Path) -> Result<Self, String> {
        let mut child = Command::new(server_cmd)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("起语言服务器失败({server_cmd}): {e}"))?;

        let mut stdin = child.stdin.take().ok_or("LSP 无 stdin")?;
        let stdout = child.stdout.take().ok_or("LSP 无 stdout")?;

        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let diagnostics: DiagMap = Arc::new(Mutex::new(HashMap::new()));

        // writer 任务:串行把分帧字节写进 stdin。
        tokio::spawn(async move {
            while let Some(bytes) = rx.recv().await {
                if stdin.write_all(&bytes).await.is_err() {
                    break;
                }
                let _ = stdin.flush().await;
            }
        });

        // reader 任务:分帧读 stdout,响应路由 oneshot,通知暂存。
        {
            let pending = pending.clone();
            let diagnostics = diagnostics.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout);
                while let Ok(Some(msg)) = read_message(&mut reader).await {
                    route_message(msg, &pending, &diagnostics);
                }
            });
        }

        let client = LspClient {
            tx,
            next_id: AtomicI64::new(1),
            pending,
            diagnostics,
            _child: child,
        };

        // initialize 握手。
        let _init = client
            .request(
                "initialize",
                json!({
                    "processId": std::process::id(),
                    "rootUri": path_to_uri(root),
                    "capabilities": {
                        "textDocument": {
                            "hover": { "contentFormat": ["plaintext", "markdown"] },
                            "definition": {},
                            "references": {},
                            "callHierarchy": { "dynamicRegistration": false },
                        }
                    },
                }),
            )
            .await?;
        client.notify("initialized", json!({}));
        Ok(client)
    }

    fn alloc_id(&self) -> i64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// 发请求并等响应(带超时)。
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.alloc_id();
        let (otx, orx) = oneshot::channel();
        self.pending.lock().insert(id, otx);
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        match tokio::time::timeout(REQUEST_TIMEOUT, orx).await {
            Ok(Ok(resp)) => {
                if let Some(err) = resp.get("error") {
                    return Err(format!("LSP {method} 错误: {err}"));
                }
                Ok(resp.get("result").cloned().unwrap_or(Value::Null))
            }
            Ok(Err(_)) => Err(format!("LSP {method} 响应通道关闭")),
            Err(_) => {
                self.pending.lock().remove(&id);
                Err(format!("LSP {method} 超时"))
            }
        }
    }

    /// 发通知(无 id,不等回)。
    pub fn notify(&self, method: &str, params: Value) {
        let _ = self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn send(&self, payload: &Value) -> Result<(), String> {
        let body = serde_json::to_vec(payload).map_err(|e| e.to_string())?;
        let mut framed = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        framed.extend_from_slice(&body);
        self.tx.send(framed).map_err(|_| "LSP writer 已关闭".to_string())
    }

    /// 同步一个文件给服务器:★查它之前必须先 didOpen(LSP 有状态)★。
    pub fn did_open(&self, path: &Path, text: &str, language_id: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": path_to_uri(path), "languageId": language_id, "version": 1, "text": text
            }}),
        );
    }

    /// 关闭一个文档(让 RA 忘记其打开态)。A2 用:重诊断前先 close 再 open,使 RA 把它当全新文档
    /// **重新分析并重发布**诊断——否则对已打开、内容未变的文档再 didOpen,RA 可能不会重发布(实测坑)。
    pub fn did_close(&self, path: &Path) {
        self.notify("textDocument/didClose", json!({"textDocument": {"uri": path_to_uri(path)}}));
    }

    /// hover:取符号类型/文档。`line`/`character` 入参 1-based(协议内部转 0-based)。
    pub async fn hover(&self, path: &Path, line: u32, character: u32) -> Result<Value, String> {
        self.request("textDocument/hover", text_doc_position(path, line, character)).await
    }

    /// goToDefinition:符号定义位置。
    pub async fn definition(&self, path: &Path, line: u32, character: u32) -> Result<Value, String> {
        self.request("textDocument/definition", text_doc_position(path, line, character)).await
    }

    /// findReferences:符号全部引用(含声明)。
    pub async fn references(&self, path: &Path, line: u32, character: u32) -> Result<Value, String> {
        let mut p = text_doc_position(path, line, character);
        p["context"] = json!({ "includeDeclaration": true });
        self.request("textDocument/references", p).await
    }

    /// ★D3 调用层级★:prepareCallHierarchy 取该位置的调用层级项(后续 incoming/outgoing 用其首项)。
    pub async fn prepare_call_hierarchy(&self, path: &Path, line: u32, character: u32) -> Result<Value, String> {
        self.request("textDocument/prepareCallHierarchy", text_doc_position(path, line, character)).await
    }

    /// 谁调用了它(incoming):改一个函数前看影响面。`item` = prepareCallHierarchy 的某一项。
    pub async fn incoming_calls(&self, item: Value) -> Result<Value, String> {
        self.request("callHierarchy/incomingCalls", json!({ "item": item })).await
    }

    /// 它调用了谁(outgoing)。`item` = prepareCallHierarchy 的某一项。
    pub async fn outgoing_calls(&self, item: Value) -> Result<Value, String> {
        self.request("callHierarchy/outgoingCalls", json!({ "item": item })).await
    }

    /// 取某 uri 当前诊断(A2 用)。
    pub fn diagnostics_for(&self, path: &Path) -> Vec<Value> {
        self.diagnostics.lock().get(&path_to_uri(path)).cloned().unwrap_or_default()
    }

    /// ★A2 诊断推感知层核心★:把刚改过的文件重新同步给服务器,等它重发该文件诊断后返回。
    /// 时序:① 清掉该 uri 旧诊断(之后"出现"=同步后新发布,可与旧的区分)② didOpen 新内容触发
    /// 重分析 ③ 轮询到该 uri 再次有发布(空数组=干净也算发布)或预算耗尽 ④ 给 flycheck 一点宽限
    /// 再读最终结果。**仅对已暖客户端调**(暖=已索引,故 `budget` 短即可;`budget` 入参便于测试放宽)。
    pub async fn sync_and_diagnose(&self, path: &Path, text: &str, budget: Duration) -> Vec<Value> {
        let uri = path_to_uri(path);
        // close + open:强制 RA 当全新文档重分析+重发布(内容没变也重发布;只 did_open 会被去重而不重发)。
        self.did_close(path);
        self.diagnostics.lock().remove(&uri);
        self.did_open(path, text, "rust");
        let mut waited = Duration::ZERO;
        let mut first_empty_publish: Option<Duration> = None;
        loop {
            // 优先非空:错误一出就返回(再等一小段接住 flycheck/二次波次)。
            if !self.diagnostics_for(path).is_empty() {
                tokio::time::sleep(DIAG_FLYCHECK_GRACE).await;
                return self.diagnostics_for(path);
            }
            // 已发布但为空:可能干净(也可能 close 的清空波先到、error 波将至)。
            // 记首次空发布时刻;持续为空超过 settle 窗口 → 判定干净返回(不干等满 budget)。
            if self.diagnostics.lock().contains_key(&uri) {
                let t0 = *first_empty_publish.get_or_insert(waited);
                if waited.saturating_sub(t0) >= DIAG_EMPTY_SETTLE {
                    return Vec::new();
                }
            }
            if waited >= budget {
                return self.diagnostics_for(path); // 超时兜底
            }
            tokio::time::sleep(DIAG_POLL_STEP).await;
            waited += DIAG_POLL_STEP;
        }
    }
}

/// 1-based 行列 → LSP `textDocument` + `position` 参数(协议 0-based,故 -1)。
fn text_doc_position(path: &Path, line: u32, character: u32) -> Value {
    json!({
        "textDocument": { "uri": path_to_uri(path) },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
    })
}

/// 路径 → file URI(绝对路径;尽力 canonicalize)。
fn path_to_uri(path: &Path) -> String {
    let abs = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    format!("file://{}", abs.to_string_lossy())
}

/// 路由一条收到的消息:有 id 且无 method = 响应(路由 oneshot);否则通知(诊断暂存)。
fn route_message(msg: Value, pending: &Pending, diagnostics: &DiagMap) {
    if msg.get("method").is_none() {
        // 响应(server→client 的请求会同时带 id+method,这里只认无 method 的纯响应)。
        if let Some(id) = msg.get("id").and_then(|v| v.as_i64()) {
            if let Some(tx) = pending.lock().remove(&id) {
                let _ = tx.send(msg);
            }
        }
        return;
    }
    // 通知:暂存诊断,其余忽略(A1 不处理 server→client 请求)。
    if msg.get("method").and_then(|m| m.as_str()) == Some("textDocument/publishDiagnostics") {
        if let Some(params) = msg.get("params") {
            if let Some(uri) = params.get("uri").and_then(|u| u.as_str()) {
                let diags = params
                    .get("diagnostics")
                    .and_then(|d| d.as_array())
                    .cloned()
                    .unwrap_or_default();
                diagnostics.lock().insert(uri.to_string(), diags);
            }
        }
    }
}

/// 分帧读一条 LSP 消息(Content-Length 头 + JSON 体)。EOF 返回 Ok(None)。
async fn read_message(reader: &mut BufReader<ChildStdout>) -> Result<Option<Value>, String> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(None); // EOF
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break; // 头结束
        }
        if let Some(v) = trimmed.strip_prefix("Content-Length:") {
            content_length = v.trim().parse().ok();
        }
    }
    let len = content_length.ok_or("LSP 消息缺 Content-Length")?;
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await.map_err(|e| e.to_string())?;
    let val: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    Ok(Some(val))
}

/// 语言服务器种类(决定起哪个二进制)。typescript-language-server 同时服务 ts/tsx/js/jsx。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServerKind {
    Rust,
    TypeScript,
}

impl ServerKind {
    /// 文件扩展名 → 语言服务器种类(D3 分层降级:None = 无对应 LSP,退 tree-sitter/文本)。
    pub fn from_ext(ext: &str) -> Option<ServerKind> {
        match ext {
            "rs" => Some(ServerKind::Rust),
            "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => Some(ServerKind::TypeScript),
            _ => None,
        }
    }
}

/// 文件扩展名 → LSP `didOpen` 的 languageId(tsx/jsx 需 react 变体,tsserver 据此区分)。
pub fn language_id_of_ext(ext: &str) -> &'static str {
    match ext {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        _ => "plaintext",
    }
}

/// 多工作区语言服务器管理:按 (根, 语言种类) 懒起、复用。注入给 `lsp` 执行器(Arc 共享)。
#[derive(Default)]
pub struct LspManager {
    clients: tokio::sync::Mutex<HashMap<(PathBuf, ServerKind), Arc<LspClient>>>,
}

impl LspManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 解析某语言种类的服务器命令 + 启动参数。
    /// Rust:env → PATH → 自有缓存 → ★自动下载★(单文件二进制)。
    /// TypeScript:env `GROWBOX_LSP_TS_SERVER` → PATH 的 `typescript-language-server`(args `--stdio`);
    /// **不自动下载**(它依赖 node 运行时,bundle node 复杂,见 03-LSP集成 M3 "后议")——缺则诚实报错降级。
    async fn ensure_server(kind: ServerKind) -> Result<(String, Vec<&'static str>), String> {
        match kind {
            ServerKind::Rust => Self::ensure_rust_analyzer().await.map(|cmd| (cmd, vec![])),
            ServerKind::TypeScript => {
                if let Ok(p) = std::env::var("GROWBOX_LSP_TS_SERVER") {
                    if Path::new(&p).is_file() {
                        return Ok((p, vec!["--stdio"]));
                    }
                }
                // GrowBox 自有装配目录(由 install_tsserver 经 npm 装进自有数据目录,不污染系统)。
                let managed = ts_managed_binary();
                if managed.is_file() {
                    return Ok((managed.to_string_lossy().into_owned(), vec!["--stdio"]));
                }
                if let Some(p) = which("typescript-language-server") {
                    return Ok((p, vec!["--stdio"]));
                }
                Err("未找到 typescript-language-server(TS/JS 代码智能)。它依赖 node;\
                     可在设置里点「装配 TS/JS 语言服务器」(经 npm 装进 GrowBox 自有目录),\
                     或 `npm i -g typescript-language-server typescript`,或设 GROWBOX_LSP_TS_SERVER。\
                     在此之前 TS/JS 文件退化到 code_outline(结构)/ code_search(文本)层"
                    .to_string())
            }
        }
    }

    /// 取/起某 (根, 语言) 的客户端(懒起、复用)。
    pub async fn client_for(&self, root: &Path, kind: ServerKind) -> Result<Arc<LspClient>, String> {
        let key = (root.to_path_buf(), kind);
        let mut map = self.clients.lock().await;
        if let Some(c) = map.get(&key) {
            return Ok(c.clone());
        }
        let (cmd, args) = Self::ensure_server(kind).await?;
        let client = Arc::new(LspClient::start_with_args(&cmd, &args, root).await?);
        map.insert(key, client.clone());
        Ok(client)
    }

    /// 取**已起**的某 (根, 语言) 客户端(不起、不下载)。
    pub async fn existing_client(&self, root: &Path, kind: ServerKind) -> Option<Arc<LspClient>> {
        self.clients.lock().await.get(&(root.to_path_buf(), kind)).cloned()
    }

    /// typescript-language-server 是否可用(GrowBox 自有装配 或 系统 PATH 或 env)。
    pub fn ts_installed() -> bool {
        std::env::var("GROWBOX_LSP_TS_SERVER").ok().map(|p| Path::new(&p).is_file()).unwrap_or(false)
            || ts_managed_binary().is_file()
            || which("typescript-language-server").is_some()
    }

    /// 系统是否有 npm(决定能否自动装配)。
    pub fn npm_available() -> bool {
        which("npm").is_some()
    }

    /// ★tsserver 自动装配★:经 `npm` 把 typescript + typescript-language-server 装进 GrowBox 自有目录
    /// (`ts_managed_dir()`,不污染系统、随卸载清干净)。需系统已装 node/npm;缺则诚实报错。
    /// 返回装好的二进制路径。前端按钮触发。
    pub async fn install_tsserver() -> Result<String, String> {
        let npm = which("npm").ok_or(
            "未找到 npm。typescript-language-server 依赖 Node.js —— 请先装 Node(含 npm)再试。",
        )?;
        let dir = ts_managed_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("建装配目录失败: {e}"))?;
        let out = tokio::process::Command::new(&npm)
            .args([
                "install",
                "--prefix",
                &dir.to_string_lossy(),
                "--no-audit",
                "--no-fund",
                "--silent",
                "typescript",
                "typescript-language-server",
            ])
            .output()
            .await
            .map_err(|e| format!("启动 npm 失败: {e}"))?;
        if !out.status.success() {
            let err: String = String::from_utf8_lossy(&out.stderr).chars().take(600).collect();
            return Err(format!("npm install 失败:{err}"));
        }
        let bin = ts_managed_binary();
        if bin.is_file() {
            Ok(bin.to_string_lossy().into_owned())
        } else {
            Err("npm 装完但未找到 typescript-language-server 二进制(node_modules/.bin/)".into())
        }
    }

    /// 解析/装配 rust-analyzer(A1.5 自动装配):env 覆盖 → 系统 PATH(尊重用户 rustup)→
    /// GrowBox 自有缓存目录 → **自动按平台下载到自有目录**(不污染系统、免 admin、随卸载清干净)。
    async fn ensure_rust_analyzer() -> Result<String, String> {
        if let Ok(p) = std::env::var("GROWBOX_LSP_RUST_ANALYZER") {
            if Path::new(&p).is_file() {
                return Ok(p);
            }
        }
        if let Some(p) = which("rust-analyzer") {
            return Ok(p);
        }
        let cached = ra_cache_path();
        if cached.is_file() {
            return Ok(cached.to_string_lossy().into_owned());
        }
        download_rust_analyzer(&cached).await?;
        Ok(cached.to_string_lossy().into_owned())
    }

    /// 取**已起**的某工作区 rust 客户端(不起、不下载)。A2 诊断推感知层用:只在 rust-analyzer 已暖
    /// (AI 先前用过 lsp、已索引)时才在脊柱里拉编辑后诊断 —— 不因一次 .rs 编辑就隐式触发
    /// 起服务器/下载/冷索引(那会给编辑回合压上不可预期的延迟)。无则 None(静默不感知)。
    pub async fn existing_rust_client(&self, root: &Path) -> Option<Arc<LspClient>> {
        self.existing_client(root, ServerKind::Rust).await
    }

    /// 取/起某工作区根的 rust-analyzer 客户端(懒起、复用;首次自动装配)。
    pub async fn rust_client(&self, root: &Path) -> Result<Arc<LspClient>, String> {
        self.client_for(root, ServerKind::Rust).await
    }
}

/// GrowBox 自有数据目录下的 rust-analyzer 缓存路径(不污染系统;同 redb 同族目录)。
fn ra_cache_path() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(std::env::temp_dir);
    base.join("com.nightpoetry.growbox").join("lsp").join("rust-analyzer")
}

/// GrowBox 自有 tsserver 装配目录(npm --prefix 装进这里;不污染系统)。
fn ts_managed_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(std::env::temp_dir);
    base.join("com.nightpoetry.growbox").join("lsp").join("ts-ls")
}

/// 装配后的 typescript-language-server 二进制路径(npm 装进 node_modules/.bin)。
fn ts_managed_binary() -> PathBuf {
    let exe = if cfg!(windows) {
        "typescript-language-server.cmd"
    } else {
        "typescript-language-server"
    };
    ts_managed_dir().join("node_modules").join(".bin").join(exe)
}

/// 当前平台的 rust-analyzer release 资产名(github releases 下载用)。
fn ra_asset_name() -> Result<&'static str, String> {
    let name = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "rust-analyzer-aarch64-apple-darwin"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "rust-analyzer-x86_64-apple-darwin"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "rust-analyzer-x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "rust-analyzer-aarch64-unknown-linux-gnu"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "rust-analyzer-x86_64-pc-windows-msvc"
    } else {
        return Err("当前平台无 rust-analyzer 预编译资产(请手动装并设 GROWBOX_LSP_RUST_ANALYZER)".into());
    };
    Ok(name)
}

/// 自动下载 rust-analyzer 到 `dest`(github 官方 release HTTPS;gunzip + chmod)。
/// 信任锚 = 官方 rust-lang github 的 HTTPS(TLS 认证源);per-version 校验和固化是后续加强
/// (sha2 已是工作区依赖,见 03 自动装配)。
async fn download_rust_analyzer(dest: &Path) -> Result<(), String> {
    let asset = ra_asset_name()?;
    let url =
        format!("https://github.com/rust-lang/rust-analyzer/releases/latest/download/{asset}.gz");
    let resp = reqwest::get(&url).await.map_err(|e| format!("下载 rust-analyzer 失败: {e}"))?;
    let resp = resp.error_for_status().map_err(|e| format!("下载 rust-analyzer HTTP 错误: {e}"))?;
    let gz_bytes = resp.bytes().await.map_err(|e| format!("读取下载流失败: {e}"))?;
    // gunzip(.gz → 二进制)。
    let mut decoder = flate2::read::GzDecoder::new(&gz_bytes[..]);
    let mut bin = Vec::new();
    std::io::copy(&mut decoder, &mut bin).map_err(|e| format!("解压 rust-analyzer 失败: {e}"))?;
    if bin.is_empty() {
        return Err("rust-analyzer 解压结果为空".into());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建目录失败: {e}"))?;
    }
    std::fs::write(dest, &bin).map_err(|e| format!("写 rust-analyzer 失败: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(dest).map_err(|e| e.to_string())?.permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(dest, perm).map_err(|e| format!("chmod 失败: {e}"))?;
    }
    Ok(())
}

/// ★A2★ 把 rust-analyzer 诊断裁成给 AI **感知**的精简文本。只取 error/warning(severity 1/2),
/// 跳过 information/hint(3/4)减噪;行/列转 1-based 与编辑器 + `file_read` 对齐。
/// 无可感知诊断返回 `None`(不产生"无错"噪音 —— 干净由上层另发瞬态诚实说明)。
/// 纯函数,不依赖服务器,可单测。
pub fn summarize_diagnostics(rel_path: &str, diags: &[Value]) -> Option<String> {
    const MAX_LINES: usize = 20;
    let mut lines: Vec<String> = Vec::new();
    for d in diags {
        let sev = d.get("severity").and_then(|s| s.as_u64()).unwrap_or(1);
        if sev > 2 {
            continue; // 3=information / 4=hint:不主动推感知(AI 可主动用 lsp 看)
        }
        let label = if sev == 1 { "error" } else { "warning" };
        let line = d.pointer("/range/start/line").and_then(|l| l.as_u64()).unwrap_or(0) + 1;
        let col = d.pointer("/range/start/character").and_then(|c| c.as_u64()).unwrap_or(0) + 1;
        let msg = d.get("message").and_then(|m| m.as_str()).unwrap_or("").replace('\n', " ");
        lines.push(format!("  {rel_path}:{line}:{col} {label}: {msg}"));
    }
    if lines.is_empty() {
        return None;
    }
    let total = lines.len();
    let mut s = format!("rust-analyzer 报告「{rel_path}」有 {total} 条诊断(error/warning):");
    for l in lines.iter().take(MAX_LINES) {
        s.push('\n');
        s.push_str(l);
    }
    if total > MAX_LINES {
        s.push_str(&format!("\n  …(另有 {} 条未列出)", total - MAX_LINES));
    }
    Some(s)
}

/// 在 PATH 里找一个可执行文件,返回绝对路径。
fn which(name: &str) -> Option<String> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let full = dir.join(name);
            if full.is_file() {
                Some(full.to_string_lossy().into_owned())
            } else {
                None
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 定位测试用 rust-analyzer:环境变量 → 仓库 `.tooling/ra` → 系统 PATH。
    /// 都没有 → None(CI 无 RA 时测试跳过、不挂)。
    fn test_ra() -> Option<String> {
        if let Ok(p) = std::env::var("GROWBOX_LSP_RUST_ANALYZER") {
            if Path::new(&p).is_file() {
                return Some(p);
            }
        }
        let local = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.tooling/ra");
        if Path::new(local).is_file() {
            return Some(local.to_string());
        }
        which("rust-analyzer")
    }

    /// 起真实 rust-analyzer,对临时 cargo 工程做 hover / definition / references。
    /// ★机制验证(不碰 UI)★:有 rust-analyzer 才跑,否则跳过。
    #[tokio::test]
    async fn lsp_hover_definition_references_on_fixture() {
        let Some(ra) = test_ra() else {
            eprintln!("skip: 无 rust-analyzer(设 GROWBOX_LSP_RUST_ANALYZER 或放 .tooling/ra)");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"fix\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        // greet 定义在第 1 行;第 2 行 main 调用它(列对齐 1-based)。
        let src = "fn greet(name: &str) -> String { format!(\"hi {name}\") }\nfn main() { let _ = greet(\"x\"); }\n";
        let main_rs = dir.path().join("src/main.rs");
        let mut f = std::fs::File::create(&main_rs).unwrap();
        f.write_all(src.as_bytes()).unwrap();
        drop(f);

        let client = LspClient::start(&ra, dir.path()).await.expect("起 rust-analyzer");
        client.did_open(&main_rs, src, "rust");

        // hover "greet" 调用处(第 2 行,"greet(" 的 g 在第 21 列)。索引未就绪会回 null,重试等索引。
        let (hline, hcol) = (2u32, 21u32);
        let mut hover = Value::Null;
        for _ in 0..40 {
            hover = client.hover(&main_rs, hline, hcol).await.unwrap_or(Value::Null);
            if !hover.is_null() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let hover_text = serde_json::to_string(&hover).unwrap_or_default();
        eprintln!("[LSP机制验证] hover = {}", &hover_text[..hover_text.len().min(200)]);
        assert!(
            hover_text.contains("greet") || hover_text.contains("fn"),
            "hover 应含符号/签名信息,实得: {hover_text}"
        );

        // definition:从调用处跳到定义(应落在 main.rs 第 1 行)。
        let def = client.definition(&main_rs, hline, hcol).await.unwrap();
        let def_text = serde_json::to_string(&def).unwrap_or_default();
        eprintln!("[LSP机制验证] definition = {}", &def_text[..def_text.len().min(200)]);
        assert!(def_text.contains("main.rs"), "definition 应指向 main.rs,实得: {def_text}");

        // references:greet 至少 2 处(定义 + 调用)。
        let refs = client.references(&main_rs, hline, hcol).await.unwrap();
        let count = refs.as_array().map(|a| a.len()).unwrap_or(0);
        eprintln!("[LSP机制验证] references 命中 {count} 处");
        assert!(count >= 2, "references 应 ≥2(定义+调用),实得 {count}: {refs}");
    }

    /// A2 纯函数:诊断裁剪只取 error/warning、1-based、无错返回 None。
    #[test]
    fn summarize_diagnostics_filters_and_formats() {
        // 无诊断 → None
        assert!(summarize_diagnostics("src/x.rs", &[]).is_none());
        // 只有 hint(severity 4)→ None(不推感知)
        let only_hint = vec![serde_json::json!({
            "severity": 4, "message": "consider importing", "range": {"start": {"line": 0, "character": 0}}
        })];
        assert!(summarize_diagnostics("src/x.rs", &only_hint).is_none());
        // error(1)+warning(2)+hint(4):取前两个,行列 1-based,hint 被滤
        let mixed = vec![
            serde_json::json!({"severity": 1, "message": "mismatched types", "range": {"start": {"line": 11, "character": 4}}}),
            serde_json::json!({"severity": 2, "message": "unused variable: x", "range": {"start": {"line": 19, "character": 0}}}),
            serde_json::json!({"severity": 4, "message": "hint", "range": {"start": {"line": 1, "character": 0}}}),
        ];
        let s = summarize_diagnostics("src/foo.rs", &mixed).expect("有 error/warning");
        assert!(s.contains("有 2 条诊断"), "应只计 error/warning(滤掉 hint): {s}");
        assert!(s.contains("src/foo.rs:12:5 error: mismatched types"), "行列应 1-based: {s}");
        assert!(s.contains("src/foo.rs:20:1 warning: unused variable: x"), "warning 应列出: {s}");
        assert!(!s.contains("hint"), "hint 不该出现: {s}");
    }

    /// A2 端到端(机制层,不碰 UI):对一个**有错**的 .rs,sync_and_diagnose 应取回 rust-analyzer
    /// 的诊断,且 summarize 能裁出该错误。需真 rust-analyzer(同上,无则跳过)。
    #[tokio::test]
    async fn sync_and_diagnose_surfaces_real_error() {
        let Some(ra) = test_ra() else {
            eprintln!("skip: 无 rust-analyzer(设 GROWBOX_LSP_RUST_ANALYZER 或放 .tooling/ra)");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"fix\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        // 引用一个不存在的符号 → rust-analyzer 原生诊断稳定报"cannot find value"。
        let src = "fn main() {\n    let _ = no_such_symbol_xyz;\n}\n";
        let main_rs = dir.path().join("src/main.rs");
        std::fs::write(&main_rs, src).unwrap();

        let client = LspClient::start(&ra, dir.path()).await.expect("起 rust-analyzer");
        // 冷起 + 首次索引可能慢:重试调 sync_and_diagnose(每次清+重开会持续催 RA),直到拿到诊断。
        let mut summary = None;
        for _ in 0..40 {
            let diags = client.sync_and_diagnose(&main_rs, src, Duration::from_millis(800)).await;
            if let Some(s) = summarize_diagnostics("src/main.rs", &diags) {
                summary = Some(s);
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let summary = summary.expect("应取回该文件的真实诊断");
        eprintln!("[A2 诊断推感知层] {summary}");
        assert!(summary.contains("error"), "应含 error: {summary}");
        assert!(
            summary.contains("no_such_symbol_xyz") || summary.to_lowercase().contains("cannot find"),
            "诊断应提到未解析符号: {summary}"
        );
    }

    /// A1.5 自动装配:真下载 rust-analyzer 到临时目录并验证可运行(~14MB,#[ignore] 显式跑)。
    #[tokio::test]
    #[ignore]
    async fn download_rust_analyzer_works() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("ra");
        download_rust_analyzer(&dest).await.expect("自动下载 rust-analyzer");
        assert!(dest.is_file(), "下载产物应存在");
        let out = std::process::Command::new(&dest)
            .arg("--version")
            .output()
            .expect("跑 --version");
        let ver = String::from_utf8_lossy(&out.stdout);
        eprintln!("[A1.5 自动装配] 下载并运行: {}", ver.trim());
        assert!(ver.contains("rust-analyzer"), "下载的应是 rust-analyzer,实得: {ver}");
    }
}
