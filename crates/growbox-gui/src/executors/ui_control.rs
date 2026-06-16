//! ui_control —— 活的 IDE 的家族二执行器(Agent 自己对 UI 动手)。
//!
//! 实现 `设计/00-交互层` 推论 7 + `计划/活的IDE-UI执行器.md` 支柱 A。
//! 一个参数化执行器 `ui_control(target, op)` 管所有面板的可见性,而非逐面板逐动作建执行器
//! (避组合爆炸,沿用 OpenSettings"一执行器 + 一标识"已验证模式)。
//!
//! - 目录(target 有哪些)由**前端声明**(`register_ui_surfaces`),本执行器只读那份运行时副本 →
//!   schema 动态生成、零跨语言重复(单一真相)。
//! - 合法调用经 `ui_intent` 返回**家族二往返意图**(`await_ack=true`):脊柱发出后等前端回执,
//!   返回验证态(不撒谎)。非法调用 `ui_intent` 返回 None → 落到 `execute` 报错列出合法值(经
//!   失败 → perceive 让 LLM 自纠)。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult, UiIntent};

use crate::ui::{UiSurfaceCatalog, UI_CONTROL_OPS};

pub struct UiControl {
    catalog: UiSurfaceCatalog,
}

impl UiControl {
    pub fn new(catalog: UiSurfaceCatalog) -> Self {
        UiControl { catalog }
    }

    /// 校验 (target, op) 是否在前端声明的目录里;合法返回 Some((target, op))。
    fn validate(&self, args: &serde_json::Value) -> Option<(String, String)> {
        let target = args.get("target").and_then(|v| v.as_str())?.trim();
        let op = args.get("op").and_then(|v| v.as_str())?.trim();
        if target.is_empty() || op.is_empty() {
            return None;
        }
        let cat = self.catalog.read();
        let surface = cat.iter().find(|s| s.id == target)?;
        if surface.ops.iter().any(|o| o == op) {
            Some((target.to_string(), op.to_string()))
        } else {
            None
        }
    }

    /// 给 LLM 的人话目录(definition 描述 + execute 报错共用)。
    fn catalog_hint(&self) -> String {
        let cat = self.catalog.read();
        if cat.is_empty() {
            return "(前端尚未声明任何可控面板)".into();
        }
        cat.iter()
            .map(|s| format!("{}({},可用 {})", s.id, s.label, s.ops.join("/")))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[async_trait::async_trait]
impl Executor for UiControl {
    fn name(&self) -> &str {
        "ui_control"
    }

    fn definition(&self) -> ToolDef {
        let cat = self.catalog.read();
        let targets: Vec<String> = cat.iter().map(|s| s.id.clone()).collect();
        drop(cat);
        ToolDef {
            name: self.name().into(),
            // 描述由注册表按 prompt_lang 从工具文案单一源注入;其中 llm_desc 的 `{catalog}`
            // 占位由下面的 desc_dynamic() 提供的运行时面板目录替换(动态部分与语言解耦)。
            description: String::new(),
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "面板标识", "enum": targets },
                    "op": { "type": "string", "description": "动作", "enum": UI_CONTROL_OPS },
                },
                "required": ["target", "op"]
            }),
        }
    }

    /// 把当前(前端声明的)面板目录交给注册表,替换 llm_desc 里的 `{catalog}` 占位。
    fn desc_dynamic(&self) -> Option<String> {
        Some(self.catalog_hint())
    }

    fn risk(&self) -> Risk {
        // 开关/切换面板天然可逆,零裁决(推论 4)。
        Risk::Safe
    }

    fn ui_intent(&self, args: &serde_json::Value) -> Option<UiIntent> {
        // 合法 → 家族二往返意图(脊柱发出后等前端回执);非法 → None,落 execute 报错。
        let (target, op) = self.validate(args)?;
        Some(UiIntent::round_trip(
            "ui_control",
            serde_json::json!({ "target": target, "op": op }),
        ))
    }

    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 正常(合法)路径在 dispatch 见 ui_intent 即往返,不到这里。
        // 到此 = 参数非法(未知 target / 不支持的 op / 缺参):诚实报错列出合法值,让 LLM 自纠。
        ToolResult::fail(format!(
            "ui_control 参数非法:target 必须是已声明的面板,op 必须是该面板支持的动作({})。可控面板:{}",
            UI_CONTROL_OPS.join("/"),
            self.catalog_hint()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{empty_catalog, UiSurface};

    fn catalog_with(surfaces: Vec<UiSurface>) -> UiSurfaceCatalog {
        let c = empty_catalog();
        *c.write() = surfaces;
        c
    }

    fn memory_surface() -> UiSurface {
        UiSurface {
            id: "memory".into(),
            label: "记忆可视化面板".into(),
            ops: vec!["open".into(), "close".into(), "toggle".into()],
        }
    }

    #[test]
    fn definition_reflects_declared_catalog() {
        let exec = UiControl::new(catalog_with(vec![memory_surface()]));
        // 描述本体改由注册表注入,definition() 的 description 留空;动态目录走 desc_dynamic()。
        assert!(exec.definition().description.is_empty());
        let dynamic = exec.desc_dynamic().expect("ui_control 有动态目录");
        assert!(dynamic.contains("memory"));
        assert!(dynamic.contains("记忆可视化面板"));
        // schema 的 target enum 仍含 memory(参数仍由执行器自报)。
        let targets = exec.definition().params["properties"]["target"]["enum"].as_array().unwrap().clone();
        assert!(targets.iter().any(|t| t == "memory"));
    }

    #[test]
    fn empty_catalog_yields_honest_dynamic() {
        let exec = UiControl::new(empty_catalog());
        assert!(exec.desc_dynamic().unwrap().contains("尚未声明"));
    }

    #[test]
    fn valid_call_yields_round_trip_intent() {
        let exec = UiControl::new(catalog_with(vec![memory_surface()]));
        let intent = exec
            .ui_intent(&serde_json::json!({ "target": "memory", "op": "close" }))
            .expect("合法调用应产出意图");
        assert_eq!(intent.action, "ui_control");
        assert!(intent.await_ack, "ui_control 是家族二,必须等回执");
        assert_eq!(intent.prefill["target"], "memory");
        assert_eq!(intent.prefill["op"], "close");
    }

    #[test]
    fn unknown_target_yields_none() {
        let exec = UiControl::new(catalog_with(vec![memory_surface()]));
        assert!(exec.ui_intent(&serde_json::json!({ "target": "nope", "op": "open" })).is_none());
    }

    #[test]
    fn unsupported_op_yields_none() {
        // memory 不支持 scroll(只有 open/close/toggle)。
        let exec = UiControl::new(catalog_with(vec![memory_surface()]));
        assert!(exec.ui_intent(&serde_json::json!({ "target": "memory", "op": "scroll" })).is_none());
    }

    #[test]
    fn missing_args_yield_none() {
        let exec = UiControl::new(catalog_with(vec![memory_surface()]));
        assert!(exec.ui_intent(&serde_json::json!({ "target": "memory" })).is_none());
        assert!(exec.ui_intent(&serde_json::json!({})).is_none());
    }

    #[tokio::test]
    async fn execute_on_invalid_reports_catalog() {
        // execute 仅在非法参数时被走到:应失败并列出合法面板。
        let exec = UiControl::new(catalog_with(vec![memory_surface()]));
        let mut ctx = ExecCtx { args: serde_json::json!({ "target": "nope", "op": "x" }), work_dir: std::path::Path::new("."), limits: Default::default(), cancel: None };
        let r = exec.execute(&mut ctx).await;
        assert!(!r.ok);
        assert!(r.content.contains("memory"), "报错应列出合法面板");
    }
}
