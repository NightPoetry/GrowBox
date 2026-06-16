//! code_search 执行器(二期 A3):按内容/模式在项目里找代码 —— "看得见代码"的导航基础设施。
//!
//! 后端 = `ignore`(ripgrep 自己的遍历库):**尊重 .gitignore/.ignore/hidden**(真实仓库不去搜
//! target/、node_modules/)+ 自带 glob/type 过滤;匹配用 `regex`(线性时间,无灾难回溯)。
//! 与 `file_list` 划清职责:list = 列一层目录,search = 按内容/正则跨树找。
//! 受唯一安全门只读约束(claim=Read(work_dir));输出接一期"工具输出上限"旋钮。
//! 见 `设计文档/二期项目/项目设计/04-代码搜索与Web查询.md`。

use std::path::Path;

use async_trait::async_trait;
use growbox_core::{Claim, ExecCtx, Executor, Risk, ToolDef, ToolResult};
use ignore::overrides::OverrideBuilder;
use ignore::types::TypesBuilder;
use ignore::WalkBuilder;
use regex::RegexBuilder;

pub struct CodeSearch;

/// 跳过超大文件(多半是生成物/数据,非代码;读进内存也太重)。
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// 全局命中上限(防大仓库正则把上下文撑爆;到此截断并诚实告知)。
const MAX_TOTAL_MATCHES: usize = 2000;
/// 单行回显字符上限(超长行如压缩 JS 截断)。
const MAX_LINE_LEN: usize = 300;

#[async_trait]
impl Executor for CodeSearch {
    fn name(&self) -> &str {
        "code_search"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "正则(Rust regex 语法;不区分大小写用内联 (?i))" },
                    "glob": { "type": "string", "description": "可选 glob 过滤,如 *.rs 或 src/**/*.rs(只搜匹配的)" },
                    "type": { "type": "string", "description": "可选文件类型过滤,如 rust/ts/js/py/md/toml(ignore 内置类型名)" },
                    "mode": { "type": "string", "enum": ["content", "files", "count"], "description": "content=行内容(默认)/ files=只列命中文件 / count=每文件命中数" },
                    "multiline": { "type": "boolean", "description": "true=正则可跨行匹配(. 含换行);默认 false=逐行" }
                },
                "required": ["pattern"]
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe // 只读:按内容找代码,不改文件
    }
    fn claim(&self, _args: &serde_json::Value, work_dir: &Path) -> Option<Claim> {
        Some(Claim::Read(work_dir.to_path_buf())) // 搜项目根,受只读沙箱约束
    }
    async fn execute(&self, ctx: &mut ExecCtx) -> ToolResult {
        let pattern = match ctx.args.get("pattern").and_then(|v| v.as_str()) {
            Some(p) if !p.is_empty() => p,
            _ => return ToolResult::fail("code_search 需要非空 pattern(正则)"),
        };
        let glob = ctx.args.get("glob").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
        let type_filter = ctx.args.get("type").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
        let mode = ctx.args.get("mode").and_then(|v| v.as_str()).unwrap_or("content");
        if !matches!(mode, "content" | "files" | "count") {
            return ToolResult::fail(format!("code_search 未知 mode「{mode}」(应为 content/files/count)"));
        }
        let multiline = ctx.args.get("multiline").and_then(|v| v.as_bool()).unwrap_or(false);

        // 编译正则(线性时间,无灾难回溯)。multiline → . 可匹配换行,让模式跨行。
        let re = match RegexBuilder::new(pattern).dot_matches_new_line(multiline).build() {
            Ok(re) => re,
            Err(e) => return ToolResult::fail(format!("code_search 正则非法: {e}")),
        };

        let root = ctx.work_dir;
        let mut wb = WalkBuilder::new(root);
        // require_git(false):无 .git 的项目也尊重 .gitignore/.ignore(不是每个项目都 git init 了);
        // hidden(true) 顺带跳过 .git/ 等隐藏目录。
        wb.hidden(true).git_ignore(true).require_git(false).parents(true);
        if let Some(t) = type_filter {
            let mut tb = TypesBuilder::new();
            tb.add_defaults();
            tb.select(t);
            match tb.build() {
                Ok(types) => {
                    wb.types(types);
                }
                Err(e) => {
                    return ToolResult::fail(format!(
                        "code_search 未知 type「{t}」: {e}(常见 rust/ts/js/py/md/toml/json)"
                    ))
                }
            }
        }
        if let Some(g) = glob {
            let mut ob = OverrideBuilder::new(root);
            if let Err(e) = ob.add(g) {
                return ToolResult::fail(format!("code_search glob 非法「{g}」: {e}"));
            }
            match ob.build() {
                Ok(ov) => {
                    wb.overrides(ov);
                }
                Err(e) => return ToolResult::fail(format!("code_search glob 构建失败: {e}")),
            }
        }

        let max_bytes = ctx.limits.max_output_bytes;
        let mut content_lines: Vec<String> = Vec::new(); // content/files 模式逐条
        let mut counts: Vec<(String, usize)> = Vec::new(); // count 模式 (rel, n)
        let mut out_bytes = 0usize;
        let mut total_matches = 0usize;
        let mut files_hit = 0usize;
        let mut truncated = false;

        'walk: for result in wb.build() {
            let Ok(entry) = result else { continue };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            if entry.metadata().map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(false) {
                continue; // 超大文件跳过(生成物/数据)
            }
            let path = entry.path();
            let Ok(text) = std::fs::read_to_string(path) else { continue }; // 二进制/非 UTF8 跳过
            let rel = path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned();

            let mut file_hits = 0usize;
            if multiline {
                for m in re.find_iter(&text) {
                    file_hits += 1;
                    total_matches += 1;
                    if mode == "content" {
                        let line_no = text[..m.start()].bytes().filter(|&b| b == b'\n').count() + 1;
                        let line = format!("{rel}:{line_no}: {}", first_line_trunc(m.as_str()));
                        out_bytes += line.len() + 1;
                        content_lines.push(line);
                    }
                    if total_matches >= MAX_TOTAL_MATCHES || out_bytes >= max_bytes {
                        truncated = true;
                        break;
                    }
                }
            } else {
                for (i, raw) in text.lines().enumerate() {
                    if re.is_match(raw) {
                        file_hits += 1;
                        total_matches += 1;
                        if mode == "content" {
                            let line = format!("{rel}:{}: {}", i + 1, trunc_line(raw));
                            out_bytes += line.len() + 1;
                            content_lines.push(line);
                        }
                        if total_matches >= MAX_TOTAL_MATCHES || out_bytes >= max_bytes {
                            truncated = true;
                            break;
                        }
                    }
                }
            }

            if file_hits > 0 {
                files_hit += 1;
                match mode {
                    "files" => {
                        out_bytes += rel.len() + 1;
                        content_lines.push(rel);
                    }
                    "count" => counts.push((rel, file_hits)),
                    _ => {}
                }
            }
            if truncated || out_bytes >= max_bytes {
                truncated = true;
                break 'walk;
            }
        }

        if total_matches == 0 {
            let scope = describe_scope(glob, type_filter);
            return ToolResult::ok(format!("code_search:无命中(pattern=「{pattern}」{scope})"));
        }

        let body = match mode {
            "count" => {
                counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                let lines: Vec<String> = counts.iter().map(|(r, n)| format!("{r}: {n}")).collect();
                lines.join("\n")
            }
            _ => content_lines.join("\n"),
        };
        let header = format!(
            "code_search 命中 {total_matches} 处,跨 {files_hit} 文件(mode={mode}):"
        );
        let mut out = format!("{header}\n{body}");
        if truncated {
            out.push_str(&format!(
                "\n…[已截断:命中过多(超 {MAX_TOTAL_MATCHES} 处或输出上限);请用更精确的 pattern/glob/type 收窄]"
            ));
        }
        ToolResult::ok(out)
    }
}

/// 单行回显:去尾空白 + 超长截断(按字符,防多字节切半)。
fn trunc_line(s: &str) -> String {
    let s = s.trim_end();
    if s.chars().count() > MAX_LINE_LEN {
        let kept: String = s.chars().take(MAX_LINE_LEN).collect();
        format!("{kept}…")
    } else {
        s.to_string()
    }
}

/// 取一段(可能多行)匹配的首行并截断(multiline 模式回显用)。
fn first_line_trunc(s: &str) -> String {
    trunc_line(s.lines().next().unwrap_or(""))
}

/// 无命中提示里描述本次过滤范围。
fn describe_scope(glob: Option<&str>, type_filter: Option<&str>) -> String {
    match (glob, type_filter) {
        (Some(g), Some(t)) => format!(", glob={g}, type={t}"),
        (Some(g), None) => format!(", glob={g}"),
        (None, Some(t)) => format!(", type={t}"),
        (None, None) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn run(work_dir: &Path, args: serde_json::Value) -> ToolResult {
        let mut ctx = ExecCtx { args, work_dir, limits: Default::default(), cancel: None };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(CodeSearch.execute(&mut ctx))
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/main.rs"), "fn main() {\n    greet();\n    greet();\n}\nfn greet() {}\n").unwrap();
        std::fs::write(dir.path().join("src/util.rs"), "pub fn helper() {}\n// greet 注释\n").unwrap();
        std::fs::write(dir.path().join("readme.md"), "调用 greet 的文档\n").unwrap();
        // .gitignore + 被忽略的生成目录:不该被搜到
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/gen.rs"), "fn greet() {} // generated\n").unwrap();
        dir
    }

    #[test]
    fn content_mode_lists_file_line_text() {
        let dir = fixture();
        let r = run(dir.path(), serde_json::json!({"pattern": "greet"}));
        assert!(r.ok, "{}", r.content);
        // main.rs 两处调用 + 定义 + util 注释 + readme;都应出现 file:line
        assert!(r.content.contains("src/main.rs:2: "), "应含 file:line: {}", r.content);
        assert!(r.content.contains("src/main.rs:3: "));
        assert!(r.content.contains("命中"), "应有命中汇总");
        // ★尊重 .gitignore★:target/ 被忽略,不该出现
        assert!(!r.content.contains("target/"), "gitignore 的 target/ 不该被搜到: {}", r.content);
        assert!(!r.content.contains("generated"));
    }

    #[test]
    fn files_mode_lists_unique_files_only() {
        let dir = fixture();
        let r = run(dir.path(), serde_json::json!({"pattern": "greet", "mode": "files"}));
        assert!(r.ok, "{}", r.content);
        assert!(r.content.contains("src/main.rs"));
        assert!(r.content.contains("src/util.rs"));
        assert!(r.content.contains("readme.md"));
        // files 模式不带行内容(只文件名)
        assert!(!r.content.contains("greet();"), "files 模式不该有行内容");
    }

    #[test]
    fn count_mode_reports_per_file_counts() {
        let dir = fixture();
        let r = run(dir.path(), serde_json::json!({"pattern": "greet", "mode": "count"}));
        assert!(r.ok, "{}", r.content);
        // main.rs: 3(两次调用 + 定义),按数量降序在前
        assert!(r.content.contains("src/main.rs: 3"), "main.rs 应 3 次: {}", r.content);
    }

    #[test]
    fn type_filter_restricts_to_rust() {
        let dir = fixture();
        let r = run(dir.path(), serde_json::json!({"pattern": "greet", "type": "rust", "mode": "files"}));
        assert!(r.ok, "{}", r.content);
        assert!(r.content.contains("src/main.rs"));
        assert!(!r.content.contains("readme.md"), "type=rust 应排除 .md: {}", r.content);
    }

    #[test]
    fn glob_filter_restricts_paths() {
        let dir = fixture();
        let r = run(dir.path(), serde_json::json!({"pattern": "greet", "glob": "*.md", "mode": "files"}));
        assert!(r.ok, "{}", r.content);
        assert!(r.content.contains("readme.md"));
        assert!(!r.content.contains("main.rs"), "glob *.md 应只剩 md: {}", r.content);
    }

    #[test]
    fn no_match_is_ok_with_clear_message() {
        let dir = fixture();
        let r = run(dir.path(), serde_json::json!({"pattern": "zzz_nonexistent_zzz"}));
        assert!(r.ok, "无命中应是 ok(不是失败): {}", r.content);
        assert!(r.content.contains("无命中"));
    }

    #[test]
    fn invalid_regex_fails_clearly() {
        let dir = fixture();
        let r = run(dir.path(), serde_json::json!({"pattern": "(unclosed"}));
        assert!(!r.ok);
        assert!(r.content.contains("正则非法"), "{}", r.content);
    }

    #[test]
    fn empty_pattern_fails() {
        let dir = fixture();
        assert!(!run(dir.path(), serde_json::json!({"pattern": ""})).ok);
        assert!(!run(dir.path(), serde_json::json!({})).ok);
    }

    #[test]
    fn multiline_matches_across_lines() {
        let dir = fixture();
        // main.rs 里 "main() {\n    greet" 跨行;非 multiline 匹配不到,multiline 能
        let single = run(dir.path(), serde_json::json!({"pattern": r"main\(\) \{\s+greet"}));
        assert!(single.content.contains("无命中"), "逐行模式不该跨行匹配: {}", single.content);
        let multi = run(dir.path(), serde_json::json!({"pattern": r"main\(\) \{\s+greet", "multiline": true}));
        assert!(multi.ok && multi.content.contains("src/main.rs:1: "), "multiline 应跨行命中并报起始行: {}", multi.content);
    }
}
