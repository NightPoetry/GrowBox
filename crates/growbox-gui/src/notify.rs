//! 告知原语(设计:`设计文档/感知告知-双受众.md`)。
//!
//! 双受众:**对内 perceive(prompt_lang)** + **对外显示(ui_lang)**。
//! Phase 1 先落"对内"半:把后端事件按 `code` 渲染成 `llm` 文本,交 `Memory::perceive`,
//! kind 用 code(语言中立稳定标识,LLM 跨回合可一致识别事件类型)。
//! "对外"半(toast/事件 emit)与前端迁移见 Phase 2/3。

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use growbox_memory::Memory;

use crate::notice_i18n::{fill, notices};

/// 让 LLM 感知一条提示(对内)。
/// 按 `perceive` 标志决定是否感知(默认 true);取 `llm[prompt_lang]` 模板填 params 后交 `Memory::perceive`。
/// 模板缺失则退化为用 code 本身当文本(绝不静默丢事件)。
pub fn perceive_notice(memory: &mut Memory, prompt_lang: &str, code: &str, params: &Value) {
    let n = notices();
    if !n.perceive_flag(code) {
        return;
    }
    let template = n.llm(code, prompt_lang).unwrap_or_else(|| code.to_string());
    memory.perceive(code, fill(&template, params));
}

/// 让用户看到一条提示(对外显示半,设计 2×2 第四格【后端产生 × 对外显示】)。
/// 后端来源的提示发 `"notice"` 事件 { code, params };前端监听后按 ui_lang 从同一 catalog 渲染 toast。
/// 不在此渲染文案(前端持完整 catalog 就地按界面语言渲),后端只传语言中立的 code + params。
/// 对内感知由调用方另行决定(perceive_notice,或如后台任务由 Supervisor 回合负责),两腿解耦。
pub fn emit_notice(app: &AppHandle, code: &str, params: Value) {
    let _ = app.emit("notice", json!({ "code": code, "params": params }));
}
