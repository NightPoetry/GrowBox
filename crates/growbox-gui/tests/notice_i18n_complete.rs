//! 提示文案完整性守卫(设计:`设计文档/感知告知-双受众.md`)。
//!
//! 遍历 `prompts/notices.i18n.json` 每个 code,断言:
//! - `human` 4 个 ui_lang 全非空(对外显示不退化成裸 key / 缺语言);
//! - `perceive != false` 则 `llm` 2 个 prompt_lang 全非空(对内感知不缺);
//! - `severity` ∈ {info,success,warn,error};`surface` ∈ {toast,health,silent}。
//!
//! 缺任一 → CI 红。维护单元 = 一条提示(一处增改),不是一条翻译 —— 防"散装维护两受众翻译"。

use serde_json::Value;

const NOTICES_JSON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../prompts/notices.i18n.json");
const UI_LANGS: [&str; 4] = ["zh-CN", "en", "ja", "zh-TW"];
const PROMPT_LANGS: [&str; 2] = ["zh", "en"];
const SEVERITIES: [&str; 4] = ["info", "success", "warn", "error"];
const SURFACES: [&str; 3] = ["toast", "health", "silent"];

fn load() -> Value {
    let raw = std::fs::read_to_string(NOTICES_JSON).expect("读不到 prompts/notices.i18n.json");
    serde_json::from_str(&raw).expect("notices.i18n.json 不是合法 JSON")
}

fn nonempty(v: Option<&Value>) -> bool {
    v.and_then(|v| v.as_str()).is_some_and(|s| !s.is_empty())
}

#[test]
fn every_notice_has_complete_i18n() {
    let json = load();
    let obj = json.as_object().expect("顶层应为对象");
    let mut missing: Vec<String> = Vec::new();

    for (code, entry) in obj {
        if code.starts_with('_') {
            continue; // _note 等元信息
        }
        let sev = entry.get("severity").and_then(|v| v.as_str()).unwrap_or("");
        if !SEVERITIES.contains(&sev) {
            missing.push(format!("{code}.severity 非法或缺失: '{sev}'"));
        }
        let surf = entry.get("surface").and_then(|v| v.as_str()).unwrap_or("");
        if !SURFACES.contains(&surf) {
            missing.push(format!("{code}.surface 非法或缺失: '{surf}'"));
        }
        // 对外:human 4 ui_lang 全齐
        for l in UI_LANGS {
            if !nonempty(entry.get("human").and_then(|f| f.get(l))) {
                missing.push(format!("{code}.human.{l}"));
            }
        }
        // 对内:perceive(默认 true)→ llm 2 prompt_lang 全齐
        let perceive = entry.get("perceive").and_then(|v| v.as_bool()).unwrap_or(true);
        if perceive {
            for l in PROMPT_LANGS {
                if !nonempty(entry.get("llm").and_then(|f| f.get(l))) {
                    missing.push(format!("{code}.llm.{l}(perceive=true 必须有对内渲染)"));
                }
            }
        }
    }

    assert!(
        missing.is_empty(),
        "notices.i18n.json 文案缺失(新增提示必须补齐全部受众/语言,防退化):\n{}",
        missing.join("\n")
    );
}
