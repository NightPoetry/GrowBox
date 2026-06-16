//! create_project —— 交互类执行器(控制反转)。
//!
//! 实现 `设计/00-交互层`:AI 不直接建项目,而是请前端弹出预填好的创建 UI——
//! 能推断的字段(项目名)预填,不确定的(可读写目录)留空待用户裁决。
//! dispatch 见到 `ui_intent` 即弹 UI,不会调 `execute`。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult, UiIntent};

pub struct CreateProject;

#[async_trait::async_trait]
impl Executor for CreateProject {
    fn name(&self) -> &str {
        "create_project"
    }
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(), // 由注册表按 prompt_lang 从工具文案单一源注入

            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "推断出的项目名(可留给用户改)" },
                    "path": { "type": "string", "description": "用户指定的项目目录路径(可选,会预填到可写目录)" }
                }
            }),
        }
    }
    fn risk(&self) -> Risk {
        Risk::Safe
    }
    fn ui_intent(&self, args: &serde_json::Value) -> Option<UiIntent> {
        let mut prefill = serde_json::Map::new();
        if let Some(name) = args.get("name").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            prefill.insert("name".into(), serde_json::Value::String(name.to_string()));
        }
        // 用户指定了目录路径 → 预填为可写目录
        if let Some(path) = args.get("path").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            prefill.insert("writable".into(), serde_json::json!([path]));
        }
        // 家族一·阻塞裁决:弹出预填的新建项目表单 → 脊柱让位暂停,等用户确认/取消再重新驱动
        // (后续搭建依赖"项目已建+已切换"这个结果,不能假成功后绕过项目系统在错目录硬写)。
        Some(UiIntent::hand_off_gating("open_new_project", serde_json::Value::Object(prefill)))
    }
    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 正常路径不会走到这里(dispatch 见 ui_intent 即弹 UI)。
        ToolResult::ok("已请求打开新建项目面板")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefills_name_leaves_dirs_blank() {
        let intent = CreateProject.ui_intent(&serde_json::json!({"name":"个人博客"})).unwrap();
        assert_eq!(intent.action, "open_new_project");
        assert_eq!(intent.prefill.get("name").unwrap(), "个人博客");
        assert!(intent.prefill.get("writable_roots").is_none(), "不确定的字段应留空");
    }

    #[test]
    fn empty_name_yields_empty_prefill() {
        let intent = CreateProject.ui_intent(&serde_json::json!({})).unwrap();
        assert!(intent.prefill.as_object().unwrap().is_empty());
    }
}
