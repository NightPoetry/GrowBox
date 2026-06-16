//! 个人文件夹识别 —— 自动模式的隐私安全网(不全靠 LLM 审核)。
//!
//! 用户决策 2026-06-02:自动模式尽量全自动,但"通过目录判断可能是用户的个人文件夹"时必须弹窗
//! 询问授权(最小权限,先申请读)。这是基于目录的硬判断,即便 LLM 审核漏判也兜得住。

use std::path::Path;
use std::path::PathBuf;

/// 用户的个人文件夹(家目录 + Documents/Desktop/Downloads/Pictures/Movies/Music)。
/// 跨平台经 `dirs`。项目的可写/只读目录是已授权工作区,不在此列(由 sandbox 管)。
pub fn personal_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = dirs::home_dir() {
        for sub in ["Documents", "Desktop", "Downloads", "Pictures", "Movies", "Music"] {
            v.push(home.join(sub));
        }
        v.push(home); // 家目录本身(放最后,更具体的子目录优先匹配)
    }
    v
}

/// 命令串是否引用了某个**越界的**个人文件夹(返回命中的目录)。
///
/// 子串前缀匹配(命令是字符串,无法精确解析路径):命中个人文件夹即视为触及隐私。
/// 但**跳过工作区所在的那个个人目录**——若项目本身就建在 ~/Documents/xxx 下,访问 ~/Documents
/// 属授权工作区范围,不算越界(交 LLM 审核判细节),避免误报刷屏。
pub fn personal_path_in_command(command: &str, work_dir: &Path) -> Option<String> {
    for dir in personal_dirs() {
        let s = dir.to_string_lossy();
        if s.is_empty() || !command.contains(s.as_ref()) {
            continue;
        }
        // 工作区就在这个个人目录下 → 属已授权工作区,不算越界隐私。
        if work_dir.starts_with(&dir) {
            continue;
        }
        return Some(s.into_owned());
    }
    None
}

/// 命令串是否引用了某个**用户配置的隐私文件夹**(子串匹配,返回命中目录)。
/// 与个人文件夹网不同:用户隐私文件夹**不跳过工作区**——用户明确要"遇到必然询问"。
pub fn user_privacy_in_command(command: &str, privacy_dirs: &[String]) -> Option<String> {
    for dir in privacy_dirs {
        let d = dir.trim();
        if !d.is_empty() && command.contains(d) {
            return Some(d.to_string());
        }
    }
    None
}

/// 路径是否落在某个用户隐私文件夹下(file 操作的 claim 路径用)。返回命中目录。
pub fn path_under_user_privacy(path: &Path, privacy_dirs: &[String]) -> Option<String> {
    for dir in privacy_dirs {
        let d = dir.trim();
        if !d.is_empty() && path.starts_with(d) {
            return Some(d.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detects_personal_dir_outside_workspace() {
        let Some(home) = dirs::home_dir() else { return };
        let desktop = home.join("Desktop");
        let cmd = format!("cat {}/secret.txt", desktop.display());
        // 工作区在 /tmp/proj(不在 Desktop 下)→ 命中 Desktop。
        assert_eq!(
            personal_path_in_command(&cmd, &PathBuf::from("/tmp/proj")),
            Some(desktop.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn skips_when_workspace_under_that_personal_dir() {
        let Some(home) = dirs::home_dir() else { return };
        let proj = home.join("Documents").join("myproj");
        let cmd = format!("ls {}", proj.display());
        // 项目就在 ~/Documents/myproj → 访问 ~/Documents 属工作区,不报。
        assert_eq!(personal_path_in_command(&cmd, &proj), None);
    }

    #[test]
    fn none_for_ordinary_command() {
        assert_eq!(personal_path_in_command("npm run build", &PathBuf::from("/tmp/proj")), None);
    }
}
