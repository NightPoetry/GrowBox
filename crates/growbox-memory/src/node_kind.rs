//! 受控节点 kind 表(记忆内核演进③)——时间线节点 / 内部事件的 kind 收敛为**受控取值**,
//! kind → 标签 **单一事实源**(catalog 模式,推广自 `notices.i18n.json` 的"存 code、用时按表恢复")。
//!
//! 自我感知原则的第三次泛化(接 `internal-state-perception` / `感知告知-双受众`):
//! AI 对自身及衍生的一切行为可感知 —— 对话(user/assistant)、系统(system)、内部状态(internal)、
//! **自身检索(mind_search)**、UI 操作(ui_event)、后台任务(task_event)。
//!
//! **非破坏**(铁律):role/kind 仍是自由 `String`,本表只是给受控取值一个**单一源标签**;
//! 未在表中的旧自由 kind 由 `label` 原样透传(渲染为自身),无迁移、无破坏。
//! **诚实**:就内联单 tag 省存储而言相对 content+384维 embedding 可忽略;真价值 = 受控词表(防散装)
//! + 含义单一源 + 用时恢复成 LLM 能懂的标签。

/// 用户消息。
pub const USER: &str = "user";
/// 助手消息。
pub const ASSISTANT: &str = "assistant";
/// 系统消息。
pub const SYSTEM: &str = "system";
/// 内部状态(失败/事件等自我感知;perceive 落时间线节点统一用此 role)。
pub const INTERNAL: &str = "internal";
/// 自身检索动作(下沉检索一等事件;自我感知第三次泛化)。
pub const MIND_SEARCH: &str = "mind_search";
/// 界面事件(面板开关等)。
pub const UI_EVENT: &str = "ui_event";
/// 后台任务衍生事件。
pub const TASK_EVENT: &str = "task_event";
/// 造物交互/结论(被造物层:AI 现造 UI 的点击/输入回传与永久结论)。
pub const ARTIFACT: &str = "artifact";
/// 项目级流程(二期:可复用的"在本项目做 X = 碰 A→B→C"配方;检索召回供 AI 照做或物化成工作流)。
/// 见 `设计文档/二期项目/设计原理/01-流程即一等公民.md`。
pub const PROCESS: &str = "process";
/// 代码诊断(二期 A2:语言服务器 publishDiagnostics → 感知层。编辑 .rs 后 rust-analyzer 报的
/// 编译错误/警告主动推入上下文,AI 改完即被告知哪行不过,不必跑全量构建)。
/// 见 `设计文档/二期项目/项目设计/03-LSP集成.md` M2 + [[internal-state-perception]]。
pub const LSP_DIAGNOSTIC: &str = "lsp_diagnostic";
/// 技能(第四原语:场景化知识/playbook。「某类场景怎么把事做好」的命名 playbook,带触发描述、
/// 进常驻清单可被 AI 主动挑、load_skill 取正文;与 process 同族两 kind,消费方式不同——
/// process 被动召回、skill 主动挑+召回兜底)。见 `设计文档/设计/09-Skill系统.md`。
pub const SKILL: &str = "skill";
/// 工具记忆(每工具每项目的"小本本":某工具在某"情况(关键因素)"下 可行/失败/不可行 的结论)。
/// 分发前会诊:已知不可行 + 高相似 → 反 K 一票否决重试;已知失败 → 软提醒。与 process/skill 同族。
/// 见 `设计文档/计划/工具记忆-不犯第二遍.md` + [[never-repeat-mistake-and-tool-memory]]。
pub const TOOL_MEMORY: &str = "tool_memory";
/// 自驱续跑(全自动模式下的"自动鞭策"种子:程序停下来后系统自动注入的"接下来做什么/要不要重构/
/// 怎么避免屎山"提示)。以 role=internal 落时间线 → 进记录、AI 可感知,但**不进对话历史**
/// (get_chat_history 只放 user/assistant);本 kind 只是给这条内部种子一个清晰的记录标签。
pub const SELF_DRIVE: &str = "self_drive";

/// 受控 kind 全集(防散装守卫 / 测试用 / 消费侧按标签筛)。
pub fn controlled() -> &'static [&'static str] {
    &[USER, ASSISTANT, SYSTEM, INTERNAL, MIND_SEARCH, UI_EVENT, TASK_EVENT, ARTIFACT, PROCESS, LSP_DIAGNOSTIC, SKILL, TOOL_MEMORY, SELF_DRIVE]
}

/// kind → 给 LLM/用户看的标签(**用时恢复**;prompt_lang zh/en 双语)。
/// 受控 kind 返回其单一源标签;未知(旧自由)kind 原样返回——非破坏透传。
pub fn label(kind: &str, prompt_lang: &str) -> String {
    let zh = prompt_lang.starts_with("zh");
    let s = match kind {
        MIND_SEARCH => if zh { "检索" } else { "memory search" },
        UI_EVENT => if zh { "界面事件" } else { "UI event" },
        TASK_EVENT => if zh { "后台任务" } else { "background task" },
        ARTIFACT => if zh { "造物" } else { "artifact" },
        PROCESS => if zh { "流程" } else { "process" },
        SKILL => if zh { "技能" } else { "skill" },
        TOOL_MEMORY => if zh { "工具记忆" } else { "tool memory" },
        SELF_DRIVE => if zh { "自驱续跑" } else { "self-drive" },
        LSP_DIAGNOSTIC => if zh { "代码诊断" } else { "diagnostic" },
        INTERNAL => if zh { "内部" } else { "internal" },
        USER => if zh { "用户" } else { "user" },
        ASSISTANT => if zh { "助手" } else { "assistant" },
        SYSTEM => if zh { "系统" } else { "system" },
        other => return other.to_string(), // 旧自由 kind 透传(非破坏)
    };
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controlled_kinds_have_bilingual_labels_distinct_from_code() {
        for &k in controlled() {
            let zh = label(k, "zh");
            let en = label(k, "en");
            assert!(!zh.is_empty() && !en.is_empty(), "{k} 双语标签非空");
            // 受控 kind 的标签应是"恢复后"的可读标签,不等于 code 本身(透传才返回 code)。
            assert_ne!(zh, k, "{k} 受控 → 渲染为标签而非 code");
        }
    }

    #[test]
    fn unknown_kind_passes_through() {
        // 旧自由 kind(如 agent 的 "LLM调用失败")原样透传,非破坏。
        assert_eq!(label("LLM调用失败", "zh"), "LLM调用失败");
        assert_eq!(label("某自由标签", "en"), "某自由标签");
    }
}
