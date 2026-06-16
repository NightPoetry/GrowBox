//! 分支日志(栈函数 v2 原则9 / 用户 2026-06-05 细化4)。
//!
//! 派生分支链**不与用户对话、不自动写主记忆**(主记忆只存摘要)——但其**全部调用信息原样存进
//! 项目级日志文件**(含调用时的工作流/节点上下文),随项目文件夹走,**环形覆盖**:到达上限即轮替
//! (`.log` → `.log.old`,新开),旧的被下一轮覆盖。默认上限 25G,`-1` = 无限制(可设)。
//!
//! 日志非关键路径:所有 IO 失败**静默**(不得因写日志失败而中断 Agent)。

use std::io::Write;
use std::path::{Path, PathBuf};

/// 分支日志相对项目根的路径(在造物/状态同族的 `.growbox/` 下)。
pub const BRANCH_LOG_REL: &str = ".growbox/branch.log";

/// 分支调用日志写入器:据 work_dir + 上限(GB)构造,`append` 原样追加并按需环形轮替。
pub struct BranchLog {
    path: PathBuf,
    /// None = 无限制(-1);Some(bytes) = 超此字节数即轮替。
    max_bytes: Option<u64>,
}

impl BranchLog {
    /// `max_gb` < 0 → 无限制;否则换算成字节上限。
    pub fn new(work_dir: &Path, max_gb: f64) -> Self {
        let max_bytes = if max_gb < 0.0 { None } else { Some((max_gb * 1e9) as u64) };
        Self { path: work_dir.join(BRANCH_LOG_REL), max_bytes }
    }

    /// 追加一条分支调用记录(原样,带 wf/node 上下文)。超上限则环形轮替。失败静默。
    pub fn append(&self, wf: &str, node: &str, kind: &str, data: &str) {
        let line = format!("wf={wf} node={node} {kind}: {data}\n");
        self.rotate_if_needed(line.len() as u64);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = f.write_all(line.as_bytes());
        }
    }

    /// 写前检查:若现有大小 + 本行将超上限 → 把当前日志轮替为 `.old`(环形覆盖,旧 .old 被替换)。
    fn rotate_if_needed(&self, incoming: u64) {
        let Some(max) = self.max_bytes else { return };
        if let Ok(meta) = std::fs::metadata(&self.path) {
            if meta.len().saturating_add(incoming) > max {
                let _ = std::fs::rename(&self.path, self.path.with_file_name("branch.log.old"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn appends_with_context_and_unlimited_when_negative() {
        let dir = tempdir().unwrap();
        let log = BranchLog::new(dir.path(), -1.0);
        assert!(log.max_bytes.is_none(), "-1 = 无限制");
        log.append("cmd_safety", "triage", "tool", "file_read(x) -> ok");
        let content = std::fs::read_to_string(dir.path().join(BRANCH_LOG_REL)).unwrap();
        assert!(content.contains("wf=cmd_safety") && content.contains("node=triage") && content.contains("file_read"));
    }

    #[test]
    fn rotates_when_exceeding_cap() {
        let dir = tempdir().unwrap();
        // 极小上限(50 字节)→ 写几条必触发轮替。
        let log = BranchLog::new(dir.path(), 50.0 / 1e9);
        for i in 0..6 {
            log.append("w", "n", "tool", &format!("step {i} 一些较长的内容填充字节"));
        }
        // 轮替产生了 .old(环形覆盖),当前 .log 仍在(新开)。
        assert!(dir.path().join(".growbox/branch.log.old").exists(), "超上限应轮替出 .old");
        assert!(dir.path().join(BRANCH_LOG_REL).exists(), "轮替后当前日志仍存在");
    }
}
