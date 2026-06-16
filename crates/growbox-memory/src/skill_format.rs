//! Skill 节点的内容格式(设计/09 推论3)——一个 skill = 一个记忆节点,内容是结构化 markdown:
//! 名称 + 触发描述(一句话,何时用)+ playbook 正文。我们**格式化**它(learn_skill 写入),
//! 也**解析**它的头两行(常驻清单 / 按名加载)——因为格式由我们控制,解析安全、不脆。
//!
//! 约定(LLM 友好、人可读、机可解析):
//! ```text
//! # 技能:<name>
//! 触发:<trigger 一句话>
//!
//! <playbook 正文,任意 markdown>
//! ```
//! parse_head 只读前两行抽 (name, trigger);正文是整段 content(含头,供 LLM 看全)。

const NAME_PREFIX_ZH: &str = "# 技能:";
const NAME_PREFIX_EN: &str = "# Skill:";
const WHEN_PREFIX_ZH: &str = "触发:";
const WHEN_PREFIX_EN: &str = "When:";

/// 把 (name, trigger, body) 格式化成一个 skill 节点的内容。
pub fn format(name: &str, trigger: &str, body: &str) -> String {
    format!("{NAME_PREFIX_ZH}{}\n{WHEN_PREFIX_ZH}{}\n\n{}", name.trim(), trigger.trim(), body.trim())
}

/// 从节点内容解析 (name, trigger)。解析不出(非 skill 格式 / 旧数据)→ None,调用方跳过(非破坏)。
/// 兼容中英前缀;name 行也容忍无前缀的纯 markdown 标题(`# xxx`)作兜底。
pub fn parse_head(content: &str) -> Option<(String, String)> {
    let mut lines = content.lines();
    let first = lines.next()?.trim();
    let name = strip_any(first, &[NAME_PREFIX_ZH, NAME_PREFIX_EN])
        .or_else(|| first.strip_prefix("# ").map(str::trim))
        .map(str::to_string)?;
    if name.is_empty() {
        return None;
    }
    // 触发行:取接下来第一条非空行,要求带触发前缀。
    let trigger = lines
        .map(str::trim)
        .find(|l| !l.is_empty())
        .and_then(|l| strip_any(l, &[WHEN_PREFIX_ZH, WHEN_PREFIX_EN]))
        .map(str::to_string)
        .unwrap_or_default();
    Some((name, trigger))
}

fn strip_any<'a>(s: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes.iter().find_map(|p| s.strip_prefix(p)).map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_then_parse_roundtrips() {
        let c = format("web-debug-source-locate", "框选元素后反查本地源码时", "1. 先看 data-source\n2. ...");
        let (name, trigger) = parse_head(&c).expect("解析 skill 头");
        assert_eq!(name, "web-debug-source-locate");
        assert_eq!(trigger, "框选元素后反查本地源码时");
        // 正文含头(LLM 看全),含 playbook
        assert!(c.contains("1. 先看 data-source"));
    }

    #[test]
    fn parse_non_skill_returns_none_or_empty_trigger() {
        // 完全不是 skill 格式
        assert!(parse_head("随便一段文本\n第二行").is_none() || parse_head("随便一段文本\n第二行").unwrap().1.is_empty());
        // 纯 markdown 标题兜底:有 name,trigger 空
        let (n, t) = parse_head("# 我的技能\n正文").unwrap();
        assert_eq!(n, "我的技能");
        assert!(t.is_empty());
    }

    #[test]
    fn english_prefixes_work() {
        let c = "# Skill:my-skill\nWhen:doing X\n\nbody";
        let (n, t) = parse_head(c).unwrap();
        assert_eq!(n, "my-skill");
        assert_eq!(t, "doing X");
    }
}
