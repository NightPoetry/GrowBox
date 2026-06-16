//! 工具文案多语言单一源加载器(见 `计划/luminous-dancing-prism`)。
//!
//! 文案唯一事实源 = `prompts/tools.i18n.json`,编译期 `include_str!` 内嵌(绕开 Tauri 打包路径坑;
//! 改文案改 JSON 重编译即可,不动 Rust 逻辑)。两套语言轴:
//! - `label`/`ui_desc` 给 UI,4 个 ui_lang(zh-CN/en/ja/zh-TW)。
//! - `llm_desc`/`params` 给 LLM,2 个 prompt_lang(zh/en)。
//!
//! 取词兜底链:目标语言 → en → 工具名(label/llm_desc)或空(ui_desc/params),运行期绝不崩;
//! 完整性由 `tests/tool_i18n_complete.rs` 在 CI 拦截(缺任一语言即红 —— 防"新增工具退化成固定语言")。

use serde_json::Value;

/// 内嵌的工具文案源(编译期固化)。路径相对本文件 = 仓库根 `prompts/tools.i18n.json`。
const TOOLS_I18N_RAW: &str = include_str!("../../../prompts/tools.i18n.json");

/// prompt_lang 归一到 zh / en。界面有四国语言,但发给 LLM 的提示词语言只分中/英
/// (用户决策:给机器看的固定中英二选一,与界面语言解耦)。
pub fn normalize_prompt_lang(lang: &str) -> &'static str {
    if lang.starts_with("zh") {
        "zh"
    } else {
        "en"
    }
}

/// 工具文案查表(只读,内部 serde_json::Value)。
pub struct ToolI18n {
    map: Value,
}

impl Default for ToolI18n {
    fn default() -> Self {
        Self::load()
    }
}

impl ToolI18n {
    /// 解析内嵌 JSON。解析失败 = 开发期把单一源写坏了(出厂前会被防退化测试拦下),
    /// 此处直接 panic 不静默(铁律:严重异常不静默)。
    pub fn load() -> Self {
        let map: Value = serde_json::from_str(TOOLS_I18N_RAW)
            .expect("prompts/tools.i18n.json 不是合法 JSON(工具文案单一源损坏)");
        ToolI18n { map }
    }

    fn pick(&self, name: &str, field: &str, lang: &str) -> Option<&str> {
        self.map.get(name)?.get(field)?.get(lang)?.as_str()
    }

    /// UI 工具显示名(按 ui_lang)。兜底:ui_lang → en → 工具名。
    pub fn label(&self, name: &str, ui_lang: &str) -> String {
        self.pick(name, "label", ui_lang)
            .or_else(|| self.pick(name, "label", "en"))
            .map(str::to_string)
            .unwrap_or_else(|| name.to_string())
    }

    /// UI 工具描述(按 ui_lang)。兜底:ui_lang → en → 空串。
    pub fn ui_desc(&self, name: &str, ui_lang: &str) -> String {
        self.pick(name, "ui_desc", ui_lang)
            .or_else(|| self.pick(name, "ui_desc", "en"))
            .unwrap_or("")
            .to_string()
    }

    /// 给 LLM 的工具描述(按 prompt_lang,内部归一 zh/en)。兜底:lang → en → 工具名(保证非空)。
    /// 调用方负责把 ui_control 的 `{catalog}` 占位替换为运行时面板目录。
    pub fn llm_desc(&self, name: &str, prompt_lang: &str) -> String {
        let lang = normalize_prompt_lang(prompt_lang);
        self.pick(name, "llm_desc", lang)
            .or_else(|| self.pick(name, "llm_desc", "en"))
            .map(str::to_string)
            .unwrap_or_else(|| name.to_string())
    }

    /// 列出某工具各参数的 (参数名, description)(按 prompt_lang,兜底 en)。提示词自转译扫描用。
    pub fn param_descs(&self, name: &str, prompt_lang: &str) -> Vec<(String, String)> {
        let lang = normalize_prompt_lang(prompt_lang);
        let Some(tool_params) = self
            .map
            .get(name)
            .and_then(|t| t.get("params"))
            .and_then(|p| p.as_object())
        else {
            return Vec::new();
        };
        tool_params
            .iter()
            .filter_map(|(param, langs)| {
                let d = langs.get(lang).or_else(|| langs.get("en")).and_then(|v| v.as_str())?;
                Some((param.clone(), d.to_string()))
            })
            .collect()
    }

    /// 把某工具各参数的 description 按 prompt_lang 注入进 schema 的 `properties`
    /// (只覆盖 description,保留 type/enum 等)。无对应翻译则保留 schema 原值。
    pub fn inject_param_descs(&self, name: &str, prompt_lang: &str, params: &mut Value) {
        let lang = normalize_prompt_lang(prompt_lang);
        let Some(tool_params) = self
            .map
            .get(name)
            .and_then(|t| t.get("params"))
            .and_then(|p| p.as_object())
        else {
            return;
        };
        let Some(props) = params.get_mut("properties").and_then(|p| p.as_object_mut()) else {
            return;
        };
        for (param, langs) in tool_params {
            let translated = langs
                .get(lang)
                .or_else(|| langs.get("en"))
                .and_then(|v| v.as_str());
            if let (Some(translated), Some(prop)) =
                (translated, props.get_mut(param).and_then(|p| p.as_object_mut()))
            {
                prop.insert("description".into(), Value::String(translated.to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_prompt_lang_maps_zh_variants() {
        assert_eq!(normalize_prompt_lang("zh"), "zh");
        assert_eq!(normalize_prompt_lang("zh-CN"), "zh");
        assert_eq!(normalize_prompt_lang("zh-TW"), "zh");
        assert_eq!(normalize_prompt_lang("en"), "en");
        assert_eq!(normalize_prompt_lang("ja"), "en"); // 非中文 → en
    }

    #[test]
    fn embedded_json_loads_and_picks() {
        let i = ToolI18n::load();
        assert_eq!(i.label("file_read", "en"), "Read File");
        assert_eq!(i.label("file_read", "zh-CN"), "读取文件");
        assert!(i.llm_desc("file_read", "zh").contains("读取项目内"));
        assert!(i.llm_desc("file_read", "en").contains("Read the text"));
        // ui_control 的 llm_desc 含 {catalog} 占位,等运行时替换。
        assert!(i.llm_desc("ui_control", "zh").contains("{catalog}"));
    }

    #[test]
    fn unknown_tool_falls_back_to_name() {
        let i = ToolI18n::load();
        assert_eq!(i.label("nope", "en"), "nope");
        assert_eq!(i.llm_desc("nope", "zh"), "nope");
        assert_eq!(i.ui_desc("nope", "en"), "");
    }

    #[test]
    fn inject_param_descs_overwrites_description_only() {
        let i = ToolI18n::load();
        let mut params = serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string", "description": "OLD" } },
            "required": ["path"]
        });
        i.inject_param_descs("file_read", "en", &mut params);
        assert_eq!(params["properties"]["path"]["description"], "File path (relative to project root)");
        assert_eq!(params["properties"]["path"]["type"], "string"); // 保留
    }
}
