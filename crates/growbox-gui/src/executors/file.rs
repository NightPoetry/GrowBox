//! 文件类执行器:读 / 写 / 改 / 列目录。
//!
//! 安全由注册表的单一安全门把关(各执行器 `claim()` 声明动什么路径);
//! 这里只管干活,信任已过门。路径相对项目根 `work_dir` 解析。
//!
//! 同步 `std::fs` —— 旧代码"macOS 沙箱阻断同步 I/O"是 Tauri 主线程的坑;
//! 本架构所有执行都在 Agent 循环(async 命令的 tokio 任务)里跑,不碰 UI 主线程,
//! 故同步 I/O 安全。若日后真遇阻塞,在 dispatch 外层包 spawn_blocking 即可。

use std::path::{Path, PathBuf};

use growbox_core::{Claim, ExecCtx, Executor, Risk, ToolDef, ToolResult};

// 读文件回传上限 / 列目录条目上限已暴露为可设(推论9),经 `ExecCtx.limits` 注入(默认见 `ToolLimits`)。

/// 把工具参数里的相对路径解析为绝对路径(相对项目根)。
pub(crate) fn resolve(work_dir: &Path, raw: &str) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        work_dir.join(p)
    }
}

fn arg_str<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

// --- file_read ---

pub struct FileRead;

#[async_trait::async_trait]
impl Executor for FileRead {
    fn name(&self) -> &str {
        "file_read"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入

            params: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "文件路径(相对项目根)" } },
                "required": ["path"]
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn claim(&self, args: &serde_json::Value, work_dir: &Path) -> Option<Claim> {
        arg_str(args, "path").map(|p| Claim::Read(resolve(work_dir, p)))
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let Some(path) = arg_str(&ctx.args, "path") else {
            return ToolResult::fail("缺少参数 path");
        };
        let full = resolve(ctx.work_dir, path);
        match std::fs::read(&full) {
            Ok(bytes) => {
                let max_read = ctx.limits.max_read_bytes;
                let truncated = bytes.len() > max_read;
                let slice = &bytes[..bytes.len().min(max_read)];
                let mut text = String::from_utf8_lossy(slice).into_owned();
                if truncated {
                    text.push_str(&format!("\n...[已截断,文件共 {} 字节]", bytes.len()));
                }
                ToolResult::ok(text)
            }
            Err(e) => ToolResult::fail(format!("读取 {} 失败: {e}", full.display())),
        }
    }
}

// --- file_write ---

pub struct FileWrite;

#[async_trait::async_trait]
impl Executor for FileWrite {
    fn name(&self) -> &str {
        "file_write"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入

            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "文件路径(相对项目根)" },
                    "content": { "type": "string", "description": "要写入的完整内容" }
                },
                "required": ["path", "content"]
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Reversible
    }
    fn claim(&self, args: &serde_json::Value, work_dir: &Path) -> Option<Claim> {
        arg_str(args, "path").map(|p| Claim::Write(resolve(work_dir, p)))
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let (Some(path), Some(content)) = (arg_str(&ctx.args, "path"), arg_str(&ctx.args, "content")) else {
            return ToolResult::fail("缺少参数 path 或 content");
        };
        let full = resolve(ctx.work_dir, path);
        if let Some(parent) = full.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return ToolResult::fail(format!("建目录 {} 失败: {e}", parent.display()));
            }
        }
        match std::fs::write(&full, content) {
            Ok(()) => ToolResult::ok(format!("已写入 {} ({} 字节)", full.display(), content.len())),
            Err(e) => ToolResult::fail(format!("写入 {} 失败: {e}", full.display())),
        }
    }
}

// --- file_edit ---

pub struct FileEdit;

#[async_trait::async_trait]
impl Executor for FileEdit {
    fn name(&self) -> &str {
        "file_edit"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入

            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "文件路径(相对项目根)" },
                    "old": { "type": "string", "description": "要被替换的原文(需唯一可定位)" },
                    "new": { "type": "string", "description": "替换后的新文" }
                },
                "required": ["path", "old", "new"]
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Reversible
    }
    fn claim(&self, args: &serde_json::Value, work_dir: &Path) -> Option<Claim> {
        arg_str(args, "path").map(|p| Claim::Write(resolve(work_dir, p)))
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let (Some(path), Some(old), Some(new)) =
            (arg_str(&ctx.args, "path"), arg_str(&ctx.args, "old"), arg_str(&ctx.args, "new"))
        else {
            return ToolResult::fail("缺少参数 path / old / new");
        };
        let full = resolve(ctx.work_dir, path);
        let content = match std::fs::read_to_string(&full) {
            Ok(c) => c,
            Err(e) => return ToolResult::fail(format!("读取 {} 失败: {e}", full.display())),
        };
        let Some(pos) = content.find(old) else {
            return ToolResult::fail(format!("未在 {} 中找到要替换的 old 文本", full.display()));
        };
        let mut edited = String::with_capacity(content.len() - old.len() + new.len());
        edited.push_str(&content[..pos]);
        edited.push_str(new);
        edited.push_str(&content[pos + old.len()..]);
        match std::fs::write(&full, &edited) {
            Ok(()) => ToolResult::ok(format!("已替换 {} 中 1 处", full.display())),
            Err(e) => ToolResult::fail(format!("写回 {} 失败: {e}", full.display())),
        }
    }
}

// --- file_list ---

pub struct FileList;

#[async_trait::async_trait]
impl Executor for FileList {
    fn name(&self) -> &str {
        "file_list"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入

            params: serde_json::json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "目录路径(相对项目根,缺省为根)" } }
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn claim(&self, args: &serde_json::Value, work_dir: &Path) -> Option<Claim> {
        let p = arg_str(args, "path").unwrap_or(".");
        Some(Claim::Read(resolve(work_dir, p)))
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let path = arg_str(&ctx.args, "path").unwrap_or(".");
        let full = resolve(ctx.work_dir, path);
        let rd = match std::fs::read_dir(&full) {
            Ok(rd) => rd,
            Err(e) => return ToolResult::fail(format!("读取目录 {} 失败: {e}", full.display())),
        };
        let mut lines = Vec::new();
        for entry in rd.flatten().take(ctx.limits.max_list_entries) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            lines.push(if is_dir { format!("{name}/") } else { name });
        }
        lines.sort();
        ToolResult::ok(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn run(exec: &dyn Executor, work_dir: &Path, args: serde_json::Value) -> ToolResult {
        let mut ctx = ExecCtx { args, work_dir, limits: Default::default(), cancel: None };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(exec.execute(&mut ctx))
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tempdir().unwrap();
        let w = run(&FileWrite, dir.path(), serde_json::json!({"path":"a/b.txt","content":"你好"}));
        assert!(w.ok, "{}", w.content);
        let r = run(&FileRead, dir.path(), serde_json::json!({"path":"a/b.txt"}));
        assert!(r.ok);
        assert_eq!(r.content, "你好");
    }

    #[test]
    fn edit_replaces_first_occurrence() {
        let dir = tempdir().unwrap();
        run(&FileWrite, dir.path(), serde_json::json!({"path":"f.txt","content":"foo bar foo"}));
        let e = run(&FileEdit, dir.path(), serde_json::json!({"path":"f.txt","old":"foo","new":"X"}));
        assert!(e.ok, "{}", e.content);
        let r = run(&FileRead, dir.path(), serde_json::json!({"path":"f.txt"}));
        assert_eq!(r.content, "X bar foo");
    }

    #[test]
    fn edit_missing_old_fails() {
        let dir = tempdir().unwrap();
        run(&FileWrite, dir.path(), serde_json::json!({"path":"f.txt","content":"hello"}));
        let e = run(&FileEdit, dir.path(), serde_json::json!({"path":"f.txt","old":"nope","new":"x"}));
        assert!(!e.ok);
    }

    #[test]
    fn list_shows_entries() {
        let dir = tempdir().unwrap();
        run(&FileWrite, dir.path(), serde_json::json!({"path":"one.txt","content":"1"}));
        run(&FileWrite, dir.path(), serde_json::json!({"path":"sub/two.txt","content":"2"}));
        let l = run(&FileList, dir.path(), serde_json::json!({}));
        assert!(l.ok);
        assert!(l.content.contains("one.txt"));
        assert!(l.content.contains("sub/"));
    }

    #[test]
    fn claim_resolves_against_work_dir() {
        let dir = tempdir().unwrap();
        let claim = FileRead.claim(&serde_json::json!({"path":"x.txt"}), dir.path());
        assert_eq!(claim, Some(Claim::Read(dir.path().join("x.txt"))));
    }
}
