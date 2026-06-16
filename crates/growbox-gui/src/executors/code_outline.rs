//! code_outline 执行器(二期 D3 M4,第2层结构兜底)。
//!
//! 分层降级的"结构层"(`设计原理/00` 推论5):语义层(LSP)不可用时,用 tree-sitter 列文件的结构大纲
//! (顶层函数/类型/类/方法 + 行号),比纯文本搜索更结构化、又不依赖任何语言服务器进程(离线即用)。
//! 内置 grammar:Rust / Python / JS / TS(含 TSX)。无对应 grammar 的语言 → 诚实告知退到文本层(code_search)。
//! 机制在 `crate::outline`(纯函数,已单测);本文件只做路径解析 + 结果裁剪 + 降级提示。

use std::path::Path;

use async_trait::async_trait;
use growbox_core::{Claim, ExecCtx, Executor, Risk, ToolDef, ToolResult};

pub struct CodeOutline;

#[async_trait]
impl Executor for CodeOutline {
    fn name(&self) -> &str {
        "code_outline"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "目标文件(相对项目根或绝对路径)" }
                },
                "required": ["file_path"]
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe // 只读解析结构,不改文件
    }
    fn claim(&self, args: &serde_json::Value, work_dir: &Path) -> Option<Claim> {
        // 声明读目标文件(受只读沙箱约束);相对路径按项目根解析,与实际读取一致。
        let p = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        let path = Path::new(p);
        let abs = if path.is_absolute() { path.to_path_buf() } else { work_dir.join(path) };
        Some(Claim::Read(abs))
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let file = ctx.args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
        if file.is_empty() {
            return ToolResult::fail("code_outline 需要 file_path");
        }
        let path = {
            let p = Path::new(file);
            if p.is_absolute() { p.to_path_buf() } else { ctx.work_dir.join(p) }
        };
        if !path.is_file() {
            return ToolResult::fail(format!("code_outline:文件不存在 {}", path.display()));
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let source = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => return ToolResult::fail(format!("code_outline:读文件失败 {e}")),
        };
        // 符号数上限随设置(推论9 数值全可设;0 当默认 400)。
        let max_symbols = if ctx.limits.max_outline_symbols > 0 { ctx.limits.max_outline_symbols } else { 400 };
        // ★分层降级 + 当前层自我感知★:无内置 grammar → 退到文本层(code_search),明确告知 AI 当前层。
        let Some(symbols) = crate::outline::outline(&source, ext, max_symbols) else {
            return ToolResult::fail(format!(
                "code_outline:.{ext} 无内置结构 grammar(结构层不支持)。\
                 降级到文本层:用 code_search 按文本/正则在该文件定位。"
            ));
        };
        if symbols.is_empty() {
            return ToolResult::ok(format!("code_outline「{file}」(结构层):未发现顶层声明(可能是空文件或纯脚本)"));
        }
        let mut s = format!("code_outline「{file}」(结构层 · tree-sitter)命中 {} 个符号:", symbols.len());
        for sym in &symbols {
            s.push_str(&format!("\n{}:{}  {} {}", file, sym.line, sym.kind, sym.name));
        }
        if symbols.len() >= max_symbols {
            s.push_str(&format!("\n…(已截断到 {max_symbols} 个)"));
        }
        ToolResult::ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn outlines_a_rust_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("m.rs");
        std::fs::write(&f, "struct S;\nfn go() {}\n").unwrap();
        let mut ctx = ExecCtx {
            args: serde_json::json!({ "file_path": "m.rs" }),
            work_dir: dir.path(),
            limits: Default::default(), cancel: None,
        };
        let r = CodeOutline.execute(&mut ctx).await;
        assert!(r.ok && r.content.contains("struct S") && r.content.contains("fn go"), "rust 大纲: {}", r.content);
        assert!(r.content.contains("结构层"), "应标注当前层");
    }

    #[tokio::test]
    async fn degrades_on_unknown_ext() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.xyz");
        std::fs::write(&f, "blah blah").unwrap();
        let mut ctx = ExecCtx {
            args: serde_json::json!({ "file_path": "a.xyz" }),
            work_dir: dir.path(),
            limits: Default::default(), cancel: None,
        };
        let r = CodeOutline.execute(&mut ctx).await;
        assert!(!r.ok && r.content.contains("code_search"), "无 grammar 应降级指引文本层: {}", r.content);
    }

    #[tokio::test]
    async fn missing_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ExecCtx {
            args: serde_json::json!({ "file_path": "nope.rs" }),
            work_dir: dir.path(),
            limits: Default::default(), cancel: None,
        };
        assert!(!CodeOutline.execute(&mut ctx).await.ok);
    }
}
