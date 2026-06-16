//! 防退化:注册表里每个工具,工具文案单一源 `prompts/tools.i18n.json` 都必须有
//! 全部 4 个 ui_lang 的 label+ui_desc、zh/en 的 llm_desc,以及每个参数 zh/en 的说明。
//! 缺任一 → 测试失败(铁律:严重异常不静默)。
//!
//! 这是"不要每次改东西就退化成固定语言"的结构保证:新增工具若漏翻译,CI 直接红。

use growbox_gui::registry::Registry;
use growbox_gui::tasks::TaskManager;
use serde_json::Value;

const TOOLS_JSON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../prompts/tools.i18n.json");
const UI_LANGS: [&str; 4] = ["zh-CN", "en", "ja", "zh-TW"];
const PROMPT_LANGS: [&str; 2] = ["zh", "en"];

fn load_json() -> Value {
    let raw = std::fs::read_to_string(TOOLS_JSON)
        .unwrap_or_else(|e| panic!("tools.i18n.json 读不到({TOOLS_JSON}): {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("tools.i18n.json 不是合法 JSON: {e}"))
}

fn nonempty_str(v: Option<&Value>) -> bool {
    v.and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty())
}

#[test]
fn every_registered_tool_has_complete_i18n() {
    let json = load_json();
    let reg = Registry::with_builtins(TaskManager::new());
    let mut missing: Vec<String> = Vec::new();

    for name in reg.names() {
        let Some(entry) = json.get(&name) else {
            missing.push(format!("{name}: 整个条目缺失"));
            continue;
        };
        for l in UI_LANGS {
            for field in ["label", "ui_desc"] {
                if !nonempty_str(entry.get(field).and_then(|f| f.get(l))) {
                    missing.push(format!("{name}.{field}.{l}"));
                }
            }
        }
        for l in PROMPT_LANGS {
            if !nonempty_str(entry.get("llm_desc").and_then(|f| f.get(l))) {
                missing.push(format!("{name}.llm_desc.{l}"));
            }
        }
        // 参数说明也归提示词语言(否则 prompt_lang=en 时工具描述英文、参数说明中文,割裂)。
        if let Some(params) = entry.get("params").and_then(|p| p.as_object()) {
            for (pname, langs) in params {
                for l in PROMPT_LANGS {
                    if !nonempty_str(langs.get(l)) {
                        missing.push(format!("{name}.params.{pname}.{l}"));
                    }
                }
            }
        }
    }

    assert!(
        missing.is_empty(),
        "tools.i18n.json 文案缺失(新增工具必须补齐全部语言,防退化):\n{}",
        missing.join("\n")
    );
}

/// `{catalog}` 动态占位只应出现在 ui_control 的 llm_desc(唯一有运行时目录的工具)。
#[test]
fn catalog_placeholder_only_in_ui_control() {
    let json = load_json();
    for (name, entry) in json.as_object().expect("顶层应为对象") {
        if name.starts_with('_') {
            continue; // _note 等元字段
        }
        let has_ph = entry
            .get("llm_desc")
            .map(|d| d.to_string().contains("{catalog}"))
            .unwrap_or(false);
        if name == "ui_control" {
            assert!(has_ph, "ui_control 的 llm_desc 应含 {{catalog}} 占位");
        } else {
            assert!(!has_ph, "{name} 不该含 {{catalog}} 占位");
        }
    }
}
