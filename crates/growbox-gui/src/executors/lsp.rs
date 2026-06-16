//! lsp 执行器(二期 A1):把 LSP 代码智能机制(`crate::lsp`)接进唯一脊柱,供 AI 调用。
//! 机制本体在 `crate::lsp`(持久 async 客户端,已实测);本文件只做"意图↔协议"封装 + 结果裁剪。
//! 见 `设计文档/二期项目/项目设计/03-LSP集成.md`。

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult};
use serde_json::Value;

use crate::lsp::LspManager;

pub struct Lsp {
    mgr: Arc<LspManager>,
}

impl Lsp {
    pub fn new(mgr: Arc<LspManager>) -> Self {
        Self { mgr }
    }
}

#[async_trait]
impl Executor for Lsp {
    fn name(&self) -> &str {
        "lsp"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": ["hover", "definition", "references", "incoming_calls", "outgoing_calls"],
                        "description": "hover=取类型/文档 / definition=跳到定义 / references=查全部引用 / incoming_calls=谁调用了它(改前看影响面)/ outgoing_calls=它调用了谁"
                    },
                    "file_path": { "type": "string", "description": "目标文件(相对项目根或绝对路径)" },
                    "line": { "type": "integer", "description": "行号(1-based,与编辑器一致)" },
                    "character": { "type": "integer", "description": "列号(1-based)" }
                },
                "required": ["op", "file_path", "line", "character"]
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe // 只读代码智能(查类型/定义/引用),不改文件
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let op = ctx.args.get("op").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let file = ctx.args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let line = ctx.args.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let character = ctx.args.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        if file.is_empty() || line == 0 || character == 0 {
            return ToolResult::fail("lsp 需要 file_path + line + character(line/character 为 1-based,≥1)");
        }
        // 路径:相对则按项目根解析。
        let path = {
            let p = Path::new(file);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                ctx.work_dir.join(p)
            }
        };
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        // ★D3 分层降级 + 当前层自我感知(推论5)★:无对应语言服务器 → 明确告知"语义层不可用",
        // 引导退到结构层(code_outline / tree-sitter)或文本层(code_search),永不报死。
        let Some(kind) = crate::lsp::ServerKind::from_ext(ext) else {
            return ToolResult::fail(format!(
                "lsp:「{file}」(.{ext})无对应语言服务器,语义层(LSP)不可用。\
                 降级:用 code_outline 看文件结构(结构层/tree-sitter),或 code_search 按文本/正则定位(文本层)。"
            ));
        };
        if !path.is_file() {
            return ToolResult::fail(format!("lsp:文件不存在 {}", path.display()));
        }
        // 起/取对应语言服务器客户端 + didOpen(LSP 有状态:查文件前必须先同步内容)。
        // tsserver 缺失等返回的错误本身就含降级指引(见 LspManager::ensure_server)。
        let client = match self.mgr.client_for(ctx.work_dir, kind).await {
            Ok(c) => c,
            Err(e) => return ToolResult::fail(format!("lsp:{e}")),
        };
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        client.did_open(&path, &text, crate::lsp::language_id_of_ext(ext));
        let result = match op.as_str() {
            "hover" => client.hover(&path, line, character).await,
            "definition" => client.definition(&path, line, character).await,
            "references" => client.references(&path, line, character).await,
            "incoming_calls" | "outgoing_calls" => {
                return call_hierarchy(&client, &op, &path, line, character).await;
            }
            other => {
                return ToolResult::fail(format!(
                    "lsp:未知 op「{other}」(应为 hover/definition/references/incoming_calls/outgoing_calls)"
                ));
            }
        };
        match result {
            Ok(v) => ToolResult::ok(format_lsp_result(&op, &v)),
            Err(e) => ToolResult::fail(format!("lsp {op}:{e}")),
        }
    }
}

/// ★D3 调用层级★:prepareCallHierarchy 取首项 → incoming/outgoing → 裁成精简列表。
async fn call_hierarchy(
    client: &crate::lsp::LspClient,
    op: &str,
    path: &Path,
    line: u32,
    character: u32,
) -> ToolResult {
    let items = match client.prepare_call_hierarchy(path, line, character).await {
        Ok(v) => v,
        Err(e) => return ToolResult::fail(format!("lsp {op}:{e}")),
    };
    let Some(item) = items.as_array().and_then(|a| a.first()).cloned() else {
        return ToolResult::ok(format!("{op}:此位置不是可分析的调用点(光标需落在函数/方法名上)"));
    };
    let result = if op == "incoming_calls" {
        client.incoming_calls(item).await
    } else {
        client.outgoing_calls(item).await
    };
    match result {
        Ok(v) => ToolResult::ok(format_call_hierarchy(op, &v)),
        Err(e) => ToolResult::fail(format!("lsp {op}:{e}")),
    }
}

/// 调用层级结果裁剪:incoming 取 `from`、outgoing 取 `to`,每项 = 名字 + file:line。
fn format_call_hierarchy(op: &str, v: &Value) -> String {
    let key = if op == "incoming_calls" { "from" } else { "to" };
    let calls = v.as_array().cloned().unwrap_or_default();
    if calls.is_empty() {
        return format!("{op}:无结果");
    }
    let label = if op == "incoming_calls" { "调用方" } else { "被调用" };
    let mut s = format!("{op}({label})命中 {} 处:", calls.len());
    for c in &calls {
        let item = c.get(key).unwrap_or(c);
        let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("?");
        let uri = item.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let file = uri.strip_prefix("file://").unwrap_or(uri);
        let line = item.pointer("/range/start/line").and_then(|l| l.as_u64()).unwrap_or(0) + 1;
        s.push_str(&format!("\n{name}  {file}:{line}"));
    }
    s
}

/// 把 LSP 原始返回裁成给 LLM 的精简结果(只回要点,不回啰嗦的 range 细节)。
fn format_lsp_result(op: &str, v: &Value) -> String {
    match op {
        "hover" => {
            let text = v.pointer("/contents/value").and_then(|x| x.as_str()).unwrap_or("");
            if text.is_empty() {
                "hover:无信息(位置可能不在符号上,或索引未就绪)".into()
            } else {
                format!("hover:\n{text}")
            }
        }
        "definition" | "references" => {
            // 结果可能是 Location[](references/definition)或单个 Location(definition)。
            let locs: Vec<Value> = match v {
                Value::Array(a) => a.clone(),
                Value::Object(_) if v.get("uri").is_some() => vec![v.clone()],
                _ => Vec::new(),
            };
            if locs.is_empty() {
                return format!("{op}:无结果");
            }
            let mut s = format!("{op} 命中 {} 处:", locs.len());
            for loc in &locs {
                let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                let line = loc.pointer("/range/start/line").and_then(|l| l.as_u64()).unwrap_or(0) + 1;
                let file = uri.strip_prefix("file://").unwrap_or(uri);
                s.push_str(&format!("\n{file}:{line}"));
            }
            s
        }
        _ => v.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn format_hover_extracts_value() {
        let v = serde_json::json!({"contents": {"kind": "markdown", "value": "fn greet(name: &str)"}});
        assert!(format_lsp_result("hover", &v).contains("fn greet"));
    }

    #[test]
    fn format_references_lists_locations() {
        let v = serde_json::json!([
            {"uri": "file:///x/src/main.rs", "range": {"start": {"line": 0, "character": 3}}},
            {"uri": "file:///x/src/main.rs", "range": {"start": {"line": 1, "character": 20}}}
        ]);
        let s = format_lsp_result("references", &v);
        assert!(s.contains("命中 2 处") && s.contains("src/main.rs:1") && s.contains("src/main.rs:2"));
    }

    #[test]
    fn format_call_hierarchy_lists_callers() {
        let v = serde_json::json!([
            {"from": {"name": "main", "uri": "file:///x/src/main.rs", "range": {"start": {"line": 9, "character": 4}}}},
            {"from": {"name": "run", "uri": "file:///x/src/lib.rs", "range": {"start": {"line": 1, "character": 0}}}}
        ]);
        let s = format_call_hierarchy("incoming_calls", &v);
        assert!(s.contains("命中 2 处") && s.contains("main") && s.contains("src/main.rs:10"), "调用层级裁剪: {s}");
    }

    #[tokio::test]
    async fn missing_args_and_layered_degradation() {
        let lsp = Lsp::new(Arc::new(LspManager::new()));
        // 缺参(line=0)
        let mut ctx = ExecCtx {
            args: serde_json::json!({"op": "hover", "file_path": "a.rs", "line": 0, "character": 1}),
            work_dir: Path::new("."),
            limits: Default::default(), cancel: None,
        };
        assert!(!lsp.execute(&mut ctx).await.ok, "line=0 应拒");
        // ★D3 分层降级 + 当前层自我感知★:无对应语言服务器的扩展名 → 明确语义层不可用 + 指引结构/文本层。
        let mut ctx2 = ExecCtx {
            args: serde_json::json!({"op": "hover", "file_path": "a.xyz", "line": 1, "character": 1}),
            work_dir: Path::new("."),
            limits: Default::default(), cancel: None,
        };
        let r = lsp.execute(&mut ctx2).await;
        assert!(
            !r.ok && r.content.contains("code_outline") && r.content.contains("code_search"),
            "未知扩展应降级并指引结构/文本层: {}",
            r.content
        );
        // ★D3 tsserver★:.ts 被识别为受支持语言(路由 typescript-language-server),不再像 A1 那样"非 .rs 拒绝",
        // 也不走"无对应语言服务器"降级(tsserver 缺失则报 tsserver 相关诚实错误,见 LspManager::ensure_server)。
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.ts"), "export function f(){}\n").unwrap();
        let mut ctx3 = ExecCtx {
            args: serde_json::json!({"op": "hover", "file_path": "a.ts", "line": 1, "character": 17}),
            work_dir: dir.path(),
            limits: Default::default(), cancel: None,
        };
        let r3 = lsp.execute(&mut ctx3).await;
        assert!(!r3.content.contains("无对应语言服务器"), "TS 应被识别为受支持语言(路由 tsserver): {}", r3.content);
    }

    /// 端到端:lsp 执行器经 LspManager + 真 rust-analyzer 对临时 crate 做 hover。
    /// 需 `GROWBOX_LSP_RUST_ANALYZER` 指向真二进制;无则跳过(CI 不挂)。
    #[tokio::test]
    async fn executor_hover_end_to_end() {
        let ra_ok = std::env::var("GROWBOX_LSP_RUST_ANALYZER")
            .map(|p| Path::new(&p).is_file())
            .unwrap_or(false);
        if !ra_ok {
            eprintln!("skip: 未设 GROWBOX_LSP_RUST_ANALYZER(执行器端到端需真 rust-analyzer)");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"fix\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let src = "fn greet(name: &str) -> String { format!(\"hi {name}\") }\nfn main() { let _ = greet(\"x\"); }\n";
        std::fs::write(dir.path().join("src/main.rs"), src).unwrap();

        let lsp = Lsp::new(Arc::new(LspManager::new()));
        let mut content = String::new();
        for _ in 0..40 {
            let mut ctx = ExecCtx {
                args: serde_json::json!({"op": "hover", "file_path": "src/main.rs", "line": 2, "character": 21}),
                work_dir: dir.path(),
                limits: Default::default(), cancel: None,
            };
            let r = lsp.execute(&mut ctx).await;
            if r.ok && (r.content.contains("greet") || r.content.contains("fn ")) {
                content = r.content;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        eprintln!("[lsp 执行器端到端] {content}");
        assert!(content.contains("greet") || content.contains("fn "), "执行器 hover 应回签名,实得: {content}");
    }
}
