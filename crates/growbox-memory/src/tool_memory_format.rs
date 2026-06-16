//! 工具记忆节点的内容格式(`计划/工具记忆-不犯第二遍.md`)——一条工具记忆 = 一个节点,内容是
//! 结构化 markdown:工具名 + 情况(关键因素)+ 结论(可行/失败/不可行)+ detail。格式由我们控制,
//! 解析头三项安全、不脆(同 `skill_format`)。
//!
//! ```text
//! # 工具记忆:<tool>
//! 情况:<situation 关键因素>
//! 结论:<infeasible|fails|works>
//!
//! <detail,任意 markdown>
//! ```

/// 工具记忆的结论(关键因素下的判定)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 已知不可行(根本走不通)——分发前会诊命中 + 高相似 → 反 K 一票否决重试。
    Infeasible,
    /// 失败(可能瞬态/依赖外部关键因素)——会诊命中 → 软提醒,不阻断。
    Fails,
    /// 可行(正向经验)——不阻不提(v1 仅作展示/将来正向提示)。
    Works,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Infeasible => "infeasible",
            Verdict::Fails => "fails",
            Verdict::Works => "works",
        }
    }
    /// 宽松解析(大小写/同义词);无法识别 → 默认 `Fails`(保守:当失败处理,只提醒不硬挡)。
    pub fn parse(s: &str) -> Verdict {
        match s.trim().to_ascii_lowercase().as_str() {
            "infeasible" | "impossible" | "不可行" | "做不到" | "走不通" => Verdict::Infeasible,
            "works" | "ok" | "success" | "可行" | "成功" => Verdict::Works,
            _ => Verdict::Fails,
        }
    }
}

const TOOL_PREFIX_ZH: &str = "# 工具记忆:";
const TOOL_PREFIX_EN: &str = "# Tool memory:";
const SITU_PREFIX_ZH: &str = "情况:";
const SITU_PREFIX_EN: &str = "Situation:";
const VERDICT_PREFIX_ZH: &str = "结论:";
const VERDICT_PREFIX_EN: &str = "Verdict:";

/// 格式化 (tool, situation, verdict, detail) 成一个工具记忆节点的内容。
pub fn format(tool: &str, situation: &str, verdict: Verdict, detail: &str) -> String {
    format!(
        "{TOOL_PREFIX_ZH}{}\n{SITU_PREFIX_ZH}{}\n{VERDICT_PREFIX_ZH}{}\n\n{}",
        tool.trim(),
        situation.trim(),
        verdict.as_str(),
        detail.trim()
    )
}

/// 从节点内容解析 (tool, situation, verdict)。解析不出(非本格式/旧数据)→ None(调用方跳过,非破坏)。
pub fn parse_head(content: &str) -> Option<(String, String, Verdict)> {
    let mut lines = content.lines();
    let first = lines.next()?.trim();
    let tool = strip_any(first, &[TOOL_PREFIX_ZH, TOOL_PREFIX_EN]).map(str::to_string)?;
    if tool.is_empty() {
        return None;
    }
    let mut situation = String::new();
    let mut verdict = Verdict::Fails;
    for l in lines {
        let l = l.trim();
        if let Some(s) = strip_any(l, &[SITU_PREFIX_ZH, SITU_PREFIX_EN]) {
            situation = s.to_string();
        } else if let Some(v) = strip_any(l, &[VERDICT_PREFIX_ZH, VERDICT_PREFIX_EN]) {
            verdict = Verdict::parse(v);
        }
        // 头解析够了就可以停在空行处;但为容错,扫到第一个空行即止。
        if l.is_empty() && !situation.is_empty() {
            break;
        }
    }
    Some((tool, situation, verdict))
}

fn strip_any<'a>(s: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes.iter().find_map(|p| s.strip_prefix(p)).map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_then_parse_roundtrips() {
        let c = format("mcp_fs_read", "访问当前项目目录内容", Verdict::Infeasible, "该 MCP server 沙箱不含项目目录,够不到");
        let (tool, situ, v) = parse_head(&c).expect("解析");
        assert_eq!(tool, "mcp_fs_read");
        assert_eq!(situ, "访问当前项目目录内容");
        assert_eq!(v, Verdict::Infeasible);
        assert!(c.contains("够不到"));
    }

    #[test]
    fn verdict_parse_is_lenient() {
        assert_eq!(Verdict::parse("INFEASIBLE"), Verdict::Infeasible);
        assert_eq!(Verdict::parse("不可行"), Verdict::Infeasible);
        assert_eq!(Verdict::parse("works"), Verdict::Works);
        assert_eq!(Verdict::parse("随便"), Verdict::Fails); // 兜底 = Fails(保守)
    }

    #[test]
    fn parse_non_tool_memory_returns_none() {
        assert!(parse_head("随便一段\n第二行").is_none());
    }

    #[test]
    fn english_prefixes_work() {
        let c = "# Tool memory:shell\nSituation:run docker\nVerdict:fails\n\nno docker daemon";
        let (t, s, v) = parse_head(c).unwrap();
        assert_eq!(t, "shell");
        assert_eq!(s, "run docker");
        assert_eq!(v, Verdict::Fails);
    }
}
