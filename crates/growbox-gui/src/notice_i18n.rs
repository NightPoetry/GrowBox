//! 提示/告知 文案多语言单一源加载器(设计:`设计文档/感知告知-双受众.md`)。
//!
//! 单一事实源 = `prompts/notices.i18n.json`,编译期 `include_str!` 内嵌(同 `tool_i18n`,绕开打包路径坑)。
//! 每条按 `code` 唯一,co-locate 两受众:
//! - `human` 给用户看(对外/显示),4 个 ui_lang(zh-CN/en/ja/zh-TW)。
//! - `llm` 给 LLM 感知(对内/perceive),2 个 prompt_lang(zh/en)。
//! - `severity`(info|success|warn|error)+ `surface`(toast|health|silent)+ `perceive`(默认 true)。
//!
//! 取词兜底:目标语言 → en → None(模板缺失由 `tests/notice_i18n_complete.rs` 在 CI 拦截)。
//! 占位符 `{key}` 在发出时以同一组运行时参数(`fill`)替换两受众。

use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::tool_i18n::normalize_prompt_lang;

/// 内嵌的提示文案源(编译期固化)。路径相对本文件 = 仓库根 `prompts/notices.i18n.json`。
const NOTICES_I18N_RAW: &str = include_str!("../../../prompts/notices.i18n.json");

/// 提示文案查表(只读)。
pub struct NoticeI18n {
    map: Value,
}

impl Default for NoticeI18n {
    fn default() -> Self {
        Self::load()
    }
}

impl NoticeI18n {
    /// 解析内嵌 JSON。失败 = 单一源写坏(出厂前防退化测试会拦),直接 panic 不静默(铁律:严重异常不静默)。
    pub fn load() -> Self {
        let map: Value = serde_json::from_str(NOTICES_I18N_RAW)
            .expect("prompts/notices.i18n.json 不是合法 JSON(提示文案单一源损坏)");
        NoticeI18n { map }
    }

    fn entry(&self, code: &str) -> Option<&Value> {
        self.map.get(code)
    }

    fn pick(&self, code: &str, field: &str, lang: &str) -> Option<&str> {
        self.map.get(code)?.get(field)?.get(lang)?.as_str()
    }

    /// 显示样式 / 健康分级:info|success|warn|error。缺省 info。
    pub fn severity(&self, code: &str) -> &str {
        self.entry(code)
            .and_then(|e| e.get("severity"))
            .and_then(|v| v.as_str())
            .unwrap_or("info")
    }

    /// 显示去向:toast(瞬态)|health(常驻指示灯)|silent(只感知不显示)。缺省 toast。
    pub fn surface(&self, code: &str) -> &str {
        self.entry(code)
            .and_then(|e| e.get("surface"))
            .and_then(|v| v.as_str())
            .unwrap_or("toast")
    }

    /// LLM 是否感知。缺省 true(自我感知原则:一切提示默认对内可见)。
    pub fn perceive_flag(&self, code: &str) -> bool {
        self.entry(code)
            .and_then(|e| e.get("perceive"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
    }

    /// 对外文案模板(按 ui_lang)。兜底:ui_lang → en → None。
    pub fn human(&self, code: &str, ui_lang: &str) -> Option<String> {
        self.pick(code, "human", ui_lang)
            .or_else(|| self.pick(code, "human", "en"))
            .map(str::to_string)
    }

    /// 对内文案模板(按 prompt_lang,内部归一 zh/en)。兜底:lang → en → None。
    pub fn llm(&self, code: &str, prompt_lang: &str) -> Option<String> {
        let lang = normalize_prompt_lang(prompt_lang);
        self.pick(code, "llm", lang)
            .or_else(|| self.pick(code, "llm", "en"))
            .map(str::to_string)
    }

    /// 给前端的对外目录(显示半):按 ui_lang 渲染 human 模板(占位符 `{x}` 保留,前端填参)。
    /// 每条 = {code, severity, surface, perceive, human}。前端缓存后由 `notify(code,params)` 就地渲染,
    /// 单一事实源仍是本 catalog(镜像 get_tools 的"后端下发本地化文案"模式)。`_note` 等元信息跳过。
    pub fn catalog(&self, ui_lang: &str) -> Value {
        let mut out = Vec::new();
        if let Some(obj) = self.map.as_object() {
            for code in obj.keys() {
                if code.starts_with('_') {
                    continue;
                }
                out.push(json!({
                    "code": code,
                    "severity": self.severity(code),
                    "surface": self.surface(code),
                    "perceive": self.perceive_flag(code),
                    "human": self.human(code, ui_lang).unwrap_or_default(),
                }));
            }
        }
        Value::Array(out)
    }
}

/// 全局只读单例(提示文案到处要用,免逐层穿参)。
pub fn notices() -> &'static NoticeI18n {
    static N: OnceLock<NoticeI18n> = OnceLock::new();
    N.get_or_init(NoticeI18n::load)
}

/// 把模板里的 `{key}` 用 params 对应值替换(两受众共用同一组 params)。
/// params 为 JSON 对象;字符串值直接填,其余按 JSON 文本填(数字等)。非对象则原样返回。
pub fn fill(template: &str, params: &Value) -> String {
    let Some(obj) = params.as_object() else {
        return template.to_string();
    };
    let mut out = template.to_string();
    for (k, v) in obj {
        let val = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out = out.replace(&format!("{{{k}}}"), &val);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn embedded_json_loads_and_picks() {
        let n = NoticeI18n::load();
        assert_eq!(n.severity("store.unavailable"), "error");
        assert_eq!(n.surface("store.unavailable"), "health");
        assert!(n.perceive_flag("store.unavailable"));
        assert!(n.human("store.unavailable", "zh-CN").unwrap().contains("持久化"));
        assert!(n.human("store.unavailable", "en").unwrap().contains("Storage"));
        assert!(n.llm("tool.failed", "zh").unwrap().contains("工具"));
        assert!(n.llm("tool.failed", "en").unwrap().contains("Tool"));
        // prompt_lang 归一:zh-CN/zh-TW → zh
        assert_eq!(n.llm("tool.failed", "zh-TW"), n.llm("tool.failed", "zh"));
    }

    #[test]
    fn unknown_code_defaults_and_none() {
        let n = NoticeI18n::load();
        assert_eq!(n.severity("nope.nope"), "info");
        assert_eq!(n.surface("nope.nope"), "toast");
        assert!(n.perceive_flag("nope.nope")); // 默认感知
        assert!(n.human("nope.nope", "en").is_none());
        assert!(n.llm("nope.nope", "zh").is_none());
    }

    #[test]
    fn catalog_renders_human_for_ui_lang_and_carries_flags() {
        let n = NoticeI18n::load();
        let cat = n.catalog("en");
        let arr = cat.as_array().expect("catalog 应为数组");
        // 元信息 _note 不入目录
        assert!(arr.iter().all(|e| e["code"].as_str() != Some("_note")));
        // 找一条已知提示,human 按 ui_lang 渲染、标志齐全
        let entry = arr
            .iter()
            .find(|e| e["code"] == "project.switched")
            .expect("应含 project.switched");
        assert_eq!(entry["severity"], "success");
        assert_eq!(entry["surface"], "toast");
        assert_eq!(entry["perceive"], true);
        assert_eq!(entry["human"], "Switched to {name}"); // en 模板,占位符保留
    }

    #[test]
    fn fill_substitutes_params_both_string_and_number() {
        assert_eq!(fill("已切换到 {name}", &json!({"name": "项目A"})), "已切换到 项目A");
        assert_eq!(fill("累计 {count} 次:{last}", &json!({"count": 3, "last": "盘满"})), "累计 3 次:盘满");
        // 无参数对象 → 原样
        assert_eq!(fill("纯文本", &json!(null)), "纯文本");
    }
}
