//! 健康 / 异常告知 —— 严重影响主功能的异常集中记录 + 分级,供 UI 醒目告警。
//!
//! 实现 `设计文档/异常告知.md` 铁律:**严重异常禁止静默**。后端在异常处写入 Health,
//! 前端据 `worst()` 上色(绿/黄/橙/红)+ 致命级弹窗。问题解除即 `clear`,灯转绿。

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

/// 严重度。数值越大越严重;UI 取最高未解除级别上色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// 正常(绿)。
    Ok,
    /// 提示(黄):轻微 / 可恢复。
    Notice,
    /// 降级(橙):功能受损但可继续。
    Degraded,
    /// 致命(红):主功能不可用 / 有数据丢失风险。
    Fatal,
}

/// 一条健康问题。按 `code` 唯一(后写覆盖);清除即恢复。
/// `code` 是 `notices.i18n.json` 里 surface=health 的提示 code,`params` 是其占位符运行时值;
/// 显示文案由前端按 ui_lang 从 catalog 渲染(四国化,Phase 3),后端不再传死中文(双受众:对外显示半)。
#[derive(Debug, Clone, Serialize)]
pub struct Issue {
    pub code: String,
    pub severity: Severity,
    #[serde(default)]
    pub params: Value,
}

/// 健康状态:按 code 索引的问题集。空 = 全绿。
#[derive(Default)]
pub struct Health {
    issues: HashMap<String, Issue>,
}

impl Health {
    pub fn new() -> Self {
        Self::default()
    }

    /// 记一条问题(同 code 覆盖)。传 `Ok` 等价于清除该 code(问题解除)。
    /// `code` = notices.i18n.json 的 health 提示 code;`params` = 其占位符值(显示由前端按 ui_lang 渲染)。
    pub fn set(&mut self, code: impl Into<String>, severity: Severity, params: Value) {
        let code = code.into();
        if severity == Severity::Ok {
            self.issues.remove(&code);
        } else {
            self.issues.insert(code.clone(), Issue { code, severity, params });
        }
    }

    /// 解除某 code 的问题。
    pub fn clear(&mut self, code: &str) {
        self.issues.remove(code);
    }

    /// 最高未解除级别(无问题 = Ok)。
    pub fn worst(&self) -> Severity {
        self.issues.values().map(|i| i.severity).max().unwrap_or(Severity::Ok)
    }

    /// 快照(按严重度降序),给前端展示。
    pub fn snapshot(&self) -> Vec<Issue> {
        let mut v: Vec<Issue> = self.issues.values().cloned().collect();
        v.sort_by_key(|i| std::cmp::Reverse(i.severity));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_is_ok() {
        assert_eq!(Health::new().worst(), Severity::Ok);
    }

    #[test]
    fn worst_takes_highest() {
        let mut h = Health::new();
        h.set("a", Severity::Notice, json!({}));
        h.set("b", Severity::Fatal, json!({}));
        h.set("c", Severity::Degraded, json!({}));
        assert_eq!(h.worst(), Severity::Fatal);
        // 快照按严重度降序:Fatal 在前。
        assert_eq!(h.snapshot()[0].code, "b");
    }

    #[test]
    fn same_code_overwrites_and_ok_clears() {
        let mut h = Health::new();
        h.set("storage", Severity::Fatal, json!({ "detail": "坏了" }));
        assert_eq!(h.worst(), Severity::Fatal);
        h.set("storage", Severity::Ok, json!(null)); // 恢复
        assert_eq!(h.worst(), Severity::Ok);
        assert!(h.snapshot().is_empty());
    }
}
