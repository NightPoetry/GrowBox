//! 造物文件夹 —— 每个造物自己的、可写的持久状态/记忆目录("AI 的眼睛")。
//!
//! 实现 `计划/造物交互-v2.md` §6 + 决策日志 2026-06-04「造物有自己的文件夹」:
//! - 路径约定:`<project>/.growbox/artifacts/<canvas_id>/`(相对项目根,沙箱内可写,免每次弹授权)。
//! - 封装造物的流记忆/持久状态(棋盘 board.json、对话历史等);AI 用文件工具读写 = 比"前端回传完整状态"
//!   更持久可靠的眼睛(关游戏/重启后还在)。LLM 自定生命周期(一局清 / 整造物存续)。
//! - **隔离主记忆**:造物文件夹的读写不采集成经验、不进时间线/RAG,不污染主记忆(脊柱据此跳过 ingest)。
//!
//! 真机 diag 印证:AI 本能想 `file_edit /root/.growbox/artifacts/main.html` 存造物,因路径在项目外被授权拦;
//! 根治 = 给它项目内的约定路径(本模块)+ 提示词告知。

use std::path::{Path, PathBuf};

/// 造物状态根(相对项目根):`.growbox/artifacts`。所有造物文件夹都在其下。
pub const ARTIFACTS_REL_ROOT: &str = ".growbox/artifacts";

/// GrowBox 内部状态根(相对项目根):`.growbox`。整目录纳入沙箱可写(覆盖造物文件夹 + 未来内部状态)。
pub const GROWBOX_REL_ROOT: &str = ".growbox";

/// 某造物的文件夹绝对路径:`<work_dir>/.growbox/artifacts/<canvas_id>`。
pub fn artifact_dir(work_dir: &Path, canvas_id: &str) -> PathBuf {
    work_dir.join(ARTIFACTS_REL_ROOT).join(sanitize(canvas_id))
}

/// GrowBox 内部状态根绝对路径(供沙箱纳入可写)。
pub fn growbox_root(work_dir: &Path) -> PathBuf {
    work_dir.join(GROWBOX_REL_ROOT)
}

/// 给 LLM 看的相对路径(提示词/回执里用):`.growbox/artifacts/<canvas_id>`。
pub fn artifact_rel(canvas_id: &str) -> String {
    format!("{ARTIFACTS_REL_ROOT}/{}", sanitize(canvas_id))
}

/// 一个文件操作的目标路径是否落在造物/内部状态目录下(隔离主记忆判据)。
/// `raw_path` 可能相对项目根或绝对;统一解析后判 `.growbox/` 前缀。
pub fn is_internal_state_path(work_dir: &Path, raw_path: &str) -> bool {
    let p = Path::new(raw_path);
    let abs = if p.is_absolute() { p.to_path_buf() } else { work_dir.join(p) };
    let root = growbox_root(work_dir);
    // 不做物理 canonicalize(目标可能尚不存在);用 lexical 归一前缀判断。
    lexical_starts_with(&abs, &root)
}

/// 防 canvas_id 里的 `/`、`..` 逃逸目录:只保留安全字符,空则回退 "main"。
fn sanitize(canvas_id: &str) -> String {
    let s: String = canvas_id
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if s.is_empty() { "main".to_string() } else { s }
}

/// 词法前缀判断(不碰磁盘):折叠 `.`,逐段比较;不解析符号链接(够用且不依赖路径存在)。
fn lexical_starts_with(path: &Path, base: &Path) -> bool {
    let norm = |p: &Path| -> Vec<std::ffi::OsString> {
        let mut out = Vec::new();
        for c in p.components() {
            use std::path::Component::*;
            match c {
                CurDir => {}
                ParentDir => {
                    out.pop();
                }
                other => out.push(other.as_os_str().to_os_string()),
            }
        }
        out
    };
    let (p, b) = (norm(path), norm(base));
    p.len() >= b.len() && p[..b.len()] == b[..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dir_and_rel_compose_from_canvas() {
        let wd = Path::new("/proj");
        assert_eq!(artifact_dir(wd, "gomoku"), PathBuf::from("/proj/.growbox/artifacts/gomoku"));
        assert_eq!(artifact_rel("gomoku"), ".growbox/artifacts/gomoku");
    }

    #[test]
    fn canvas_id_is_sanitized_against_escape() {
        let wd = Path::new("/proj");
        // 路径穿越/斜杠被打平,不会逃出 artifacts 根。
        assert_eq!(artifact_dir(wd, "../../etc"), PathBuf::from("/proj/.growbox/artifacts/______etc"));
        assert_eq!(artifact_dir(wd, ""), PathBuf::from("/proj/.growbox/artifacts/main"));
    }

    #[test]
    fn detects_internal_state_paths_relative_and_absolute() {
        let wd = Path::new("/proj");
        assert!(is_internal_state_path(wd, ".growbox/artifacts/gomoku/board.json"));
        assert!(is_internal_state_path(wd, "/proj/.growbox/artifacts/x/state.json"));
        // 项目内普通文件 → 不是内部状态(照常进主记忆)。
        assert!(!is_internal_state_path(wd, "src/main.rs"));
        assert!(!is_internal_state_path(wd, "/proj/README.md"));
        // 穿越想绕回 .growbox 仍判中(词法归一)。
        assert!(is_internal_state_path(wd, "src/../.growbox/artifacts/a/b.json"));
    }
}
