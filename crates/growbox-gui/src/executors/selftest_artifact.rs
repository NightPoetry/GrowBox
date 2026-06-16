//! selftest_artifact —— 造物自检关(被造物 Phase 4):AI 造完交互造物后,**finish 前先自测**各感知是否正常。
//!
//! 实现 `计划/被造物-自由展示与交互.md` 的「造物自检关」+ 用户决策 2026-06-03
//! (必须有自我调试功能,自己预计哪些可以感知,完成后发指令测试,保证各感知正常才能结束任务)。
//! 家族二往返:脊柱发出 → 前端令 iframe 的 gx 引导**枚举全部 `[data-gx-callback]`**(声明式回调让这步可自动枚举)
//! → 回报"造物当前声明了哪些回调感知点"(checklist)。AI 据此对预期:declared 与 intended 一致即各感知已接通;
//! 缺则修造物(补回调标识/接线)再测,全绿方可 finish。治"造完棋盘≠能感知落子"的造物版虚假成功。

use growbox_core::{ExecCtx, Executor, Risk, ToolDef, ToolResult, UiIntent};

pub struct SelftestArtifact;

#[async_trait::async_trait]
impl Executor for SelftestArtifact {
    fn name(&self) -> &str {
        "selftest_artifact"
    }

    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.name().into(),
            description: String::new(),
            params: serde_json::json!({
                "type": "object",
                "properties": {
                    "canvas_id": { "type": "string", "description": "" },
                },
                "required": []
            }),
        }
    }

    fn risk(&self) -> Risk {
        // 只读自检(枚举回调面、不改用户数据),零裁决。
        Risk::Safe
    }

    fn ui_intent(&self, args: &serde_json::Value) -> Option<UiIntent> {
        let canvas_id = args.get("canvas_id").and_then(|v| v.as_str()).unwrap_or("main").trim();
        let canvas_id = if canvas_id.is_empty() { "main" } else { canvas_id };
        // 总是往返(自检无需校验参数);前端枚举造物回调面后回执 checklist。
        Some(UiIntent::round_trip(
            "selftest_artifact",
            serde_json::json!({ "canvas_id": canvas_id }),
        ))
    }

    async fn execute(&self, _ctx: &mut ExecCtx) -> ToolResult {
        // 正常路径走 ui_intent 往返;到此 = 无前端(测试),诚实失败。
        ToolResult::fail("selftest_artifact 需要前端造物画布在场(经往返枚举回调面)。")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_yields_round_trip_with_default_canvas() {
        let exec = SelftestArtifact;
        let intent = exec.ui_intent(&serde_json::json!({})).expect("自检总是往返");
        assert_eq!(intent.action, "selftest_artifact");
        assert!(intent.await_ack, "自检是家族二,必须等前端回执 checklist");
        assert_eq!(intent.prefill["canvas_id"], "main");
    }

    #[test]
    fn custom_canvas_id_passes_through() {
        let exec = SelftestArtifact;
        let intent = exec.ui_intent(&serde_json::json!({ "canvas_id": "board" })).unwrap();
        assert_eq!(intent.prefill["canvas_id"], "board");
    }
}
