//! 脊柱用的纯渲染/描述 helper:把上下文块、工作流节点、UI 意图、越界目标渲染成给 LLM 或前端的文本。
//! 全是无副作用的小函数,从 `run_agent_loop` 抽出来单独成文件(`mod.rs` 经 `use render::*` 取回作用域)。

use growbox_core::{Claim, Node, UiIntent};
use growbox_memory::ContextBlock;

/// 渲染工作记忆区(P4):每区独特标记 + 区内角色说明 + 每块完整时间戳 + 明示"按时间戳判先后"。
/// 非线性区:摆放位置≠时间顺序,故提示词必须点明按时间戳判序(决策日志 2026-05-31)。
pub(super) fn render_working_region(blocks: &[ContextBlock], prompt_lang: &str) -> Option<String> {
    if blocks.is_empty() {
        return None;
    }
    // 区头说明块收进转译目录(开自转译则用主模型重写版,否则逐字原文);其后追加数据。
    let mut s = crate::transpile::catalog("render.working_header", prompt_lang);
    for b in blocks {
        s.push_str(&format!(
            "\n--- [时间 {} | 角色 {}] ---\n{}\n",
            b.timestamp.to_rfc3339(),
            b.role,
            b.content
        ));
    }
    s.push_str("\n========== 工作记忆区结束 ==========");
    Some(s)
}

/// ★回合内补检索渲染(P4 增量)★:任务进行到一半,据 AI 当下进展重新检索、**新**调入的长期记忆片段。
/// 与工作记忆区同为非线性区(按时间戳判序),但单独成块、明示"因当前进展补充检索"——让 AI 明白这是循环中途
/// 才浮现的相关记忆(如开始 SSH 才被想起的凭据),非开场就给的。空(无新命中)则不注入,不打扰。append-only。
pub(super) fn render_recall_supplement(blocks: &[ContextBlock], prompt_lang: &str) -> Option<String> {
    if blocks.is_empty() {
        return None;
    }
    let mut s = crate::transpile::catalog("render.recall_header", prompt_lang);
    for b in blocks {
        s.push_str(&format!(
            "\n--- [时间 {} | 角色 {}] ---\n{}\n",
            b.timestamp.to_rfc3339(),
            b.role,
            b.content
        ));
    }
    s.push_str("\n========== 补充记忆区结束 ==========");
    Some(s)
}

/// 渲染 8K 最近记忆 ring(P4):永远放最末、紧贴当前回合,着重特殊标记"这是最近记忆"。
/// 时间正序(旧→新);有意不为 prompt 缓存优化(只损这一小块)。
pub(super) fn render_recent_ring(blocks: &[ContextBlock], prompt_lang: &str) -> Option<String> {
    if blocks.is_empty() {
        return None;
    }
    let mut s = crate::transpile::catalog("render.recent_header", prompt_lang);
    for b in blocks {
        s.push_str(&format!(
            "[时间 {} | 角色 {}] {}\n",
            b.timestamp.to_rfc3339(),
            b.role,
            b.content
        ));
    }
    s.push_str("########## 最近记忆结束 ##########");
    Some(s)
}

/// 渲染相关项目流程(二期 B1·建议档):任务开始时把与当前任务相关的、本项目沉淀的可复用流程配方
/// 注入为"照做"块。流程是约定(改一个东西的全部涟漪面),非代码引用,通用解析找不到——见
/// `设计文档/二期项目/设计原理/01-流程即一等公民.md`。
pub(super) fn render_process_recipes(recipes: &[String], prompt_lang: &str) -> Option<String> {
    if recipes.is_empty() {
        return None;
    }
    let mut s = crate::transpile::catalog("render.recipes_header", prompt_lang);
    for r in recipes {
        s.push_str("\n- ");
        s.push_str(r);
        s.push('\n');
    }
    s.push_str("\n========== 相关项目流程结束 ==========");
    Some(s)
}

/// ★二期 C2★:把一条流程配方拆成 `(展示文本, 可选的可执行工作流名)`。
/// `wf: <名>` 单行是"可执行档"标记(见 `02-process-kind落地.md` M3)——从展示文本里剥掉(单独物化成可运行工作流)。
/// 无标记行 = 纯建议档,返回 `(配方原文, None)`。
pub(super) fn parse_process_spec(content: &str) -> (String, Option<String>) {
    let mut wf = None;
    let mut kept: Vec<&str> = Vec::new();
    for line in content.lines() {
        if let Some(rest) = line.trim().strip_prefix("wf:") {
            let name = rest.trim();
            if !name.is_empty() {
                wf = Some(name.to_string());
                continue; // 剥掉标记行,不进展示
            }
        }
        kept.push(line);
    }
    (kept.join("\n").trim().to_string(), wf)
}

/// 渲染可执行项目流程(二期 C2·可执行档):这些流程已沉淀为现成可运行的工作流,做这件事时**直接调用
/// 对应工作流(栈调用)**即按既定步骤可靠执行,优于手工逐步重做。每项 = `(展示文本, 工作流名)`。
pub(super) fn render_executable_processes(items: &[(String, String)], prompt_lang: &str) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let mut s = crate::transpile::catalog("render.executable_header", prompt_lang);
    for (text, wf) in items {
        s.push_str(&format!("\n- 任务:{text}\n  → 立即调用工作流(栈调用): `{wf}`(优先于手工逐步)\n"));
    }
    s.push_str("\n========== 可执行项目流程结束 ==========");
    Some(s)
}

/// ★主动自检指令(grounded verification)★:收尾前注入,让 AI 拿即将给出的工作汇报、**重读相关文件/真实状态
/// 逐条核对**(而非空想自省),改正证据不支持的说法、标注无法验证的,再正式 finish。
/// 强制"对证据复核"是关键——纯反思不涨准确率,重读真身才抓得住过度声称/幻觉。
pub(super) fn self_verify_prompt(summary: &str, prompt_lang: &str) -> String {
    // 模板收编进转译目录(`transpile::CATALOG` agent.self_verify,含占位符 {summary});
    // 开了自转译则用主模型重写过的版本,否则逐字原文。取回后把 {summary} 换成本次工作汇报。
    crate::transpile::catalog("agent.self_verify", prompt_lang).replace("{summary}", summary)
}

/// ★自检动效标签★:把自检阶段正在调用的工具译成"正在核查:xxx"(随核查对象变),给前端动态指示器。
pub(super) fn verify_status_label(name: &str, args: &str) -> String {
    let basename = |p: &str| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string();
    let path_arg = || {
        serde_json::from_str::<serde_json::Value>(args).ok().and_then(|v| {
            v.get("path")
                .or_else(|| v.get("file_path"))
                .and_then(|p| p.as_str())
                .map(basename)
        })
    };
    match name {
        "file_read" => format!("正在核查:读取 {}", path_arg().unwrap_or_else(|| "文件".into())),
        "file_write" | "file_edit" => format!("正在核查:复看 {}", path_arg().unwrap_or_else(|| "文件".into())),
        "code_outline" => format!("正在核查:结构 {}", path_arg().unwrap_or_else(|| "文件".into())),
        "file_list" => "正在核查:目录".into(),
        "code_search" => {
            let pat = serde_json::from_str::<serde_json::Value>(args)
                .ok()
                .and_then(|v| v.get("pattern").and_then(|p| p.as_str()).map(|s| s.chars().take(24).collect::<String>()));
            match pat {
                Some(p) if !p.is_empty() => format!("正在核查:搜索「{p}」"),
                _ => "正在核查:搜索".into(),
            }
        }
        "lsp" => "正在核查:语义".into(),
        "shell" => "正在核查:运行检查".into(),
        _ => format!("正在核查:{name}"),
    }
}

/// 工作流节点引导词:进入/流转到一个节点时,以 system 注入,告诉 AI 当前在哪个节点、这一步该做什么。
/// 配合工具收窄(本节点只暴露 node.tools + finish/ask_user),把"该怎么做"从建议变成约束(07 原则1)。
pub(super) fn node_guidance(wf_name: &str, node: &Node, prompt_lang: &str) -> String {
    // 节点引导模板收进转译目录(占位符 {wf}/{node}/{prompt});取回后填入运行时数据。
    crate::transpile::catalog("render.node_guidance", prompt_lang)
        .replace("{wf}", wf_name)
        .replace("{node}", &node.id)
        .replace("{prompt}", &node.prompt)
}

/// 家族二 UI 意图的可读描述(回填给 LLM 的工具结果用):如 "ui_control target=memory op=close"。
pub(super) fn describe_ui_intent(intent: &UiIntent) -> String {
    let target = intent.prefill.get("target").and_then(|v| v.as_str()).unwrap_or("?");
    let op = intent.prefill.get("op").and_then(|v| v.as_str()).unwrap_or("?");
    format!("{} target={target} op={op}", intent.action)
}

/// 越界资源的可读目标(给权限弹窗)。
pub(super) fn claim_target(claim: &Option<Claim>) -> String {
    match claim {
        Some(Claim::Read(p)) | Some(Claim::Write(p)) => p.display().to_string(),
        Some(Claim::Shell(c)) => c.clone(),
        Some(Claim::Net(u)) => u.clone(),
        None => String::new(),
    }
}

/// 授权请求的访问类型 —— 前端据此正确分流授权,避免把只读/shell 访问错加成"可写目录"
/// (用户决策 2026-06-02:只读探测不该被授权成可写)。shell 的 target 是命令字符串非路径;
/// net 的 target 是 URL(持久化按主机落 net_grants,见 grant_net_host)。
pub(super) fn claim_kind(claim: &Option<Claim>) -> &'static str {
    match claim {
        Some(Claim::Write(_)) => "write",
        Some(Claim::Read(_)) => "read",
        Some(Claim::Shell(_)) => "shell",
        Some(Claim::Net(_)) => "net",
        None => "",
    }
}

/// 退化重复指纹:reasoning + content 折叠所有空白(容忍排版/换行差异 = "近乎全等")。
/// 连续多轮指纹相同 = 模型在原地重复同样的话 = 真高频重复死循环(思考免死的唯一例外)。
pub(super) fn degenerate_fingerprint(reasoning: &str, content: &str) -> String {
    let combined = format!("{reasoning}\n{content}");
    combined.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod c2_tests {
    use super::*;

    #[test]
    fn parse_process_spec_extracts_wf_line() {
        // 有 wf: 标记 → 剥掉标记行,返回工作流名。
        let (text, wf) = parse_process_spec("【出测试包】重建前端 dist -> 仓库根 cargo tauri build\nwf: build_test_package");
        assert_eq!(wf.as_deref(), Some("build_test_package"));
        assert!(text.contains("重建前端") && !text.contains("wf:"), "标记行应被剥掉: {text}");

        // 无 wf: → 纯建议档。
        let (text2, wf2) = parse_process_spec("加设置碰 Settings -> 命令 -> 前端");
        assert!(wf2.is_none());
        assert_eq!(text2, "加设置碰 Settings -> 命令 -> 前端");

        // 空 wf: 名(畸形)→ 当普通行,不识别为可执行。
        let (_t, wf3) = parse_process_spec("step one\nwf:   ");
        assert!(wf3.is_none(), "空工作流名不算可执行档");
    }

    #[test]
    fn render_executable_processes_lists_call_to_action() {
        assert!(render_executable_processes(&[], "zh").is_none());
        let block = render_executable_processes(&[("出测试包".into(), "build_test_package".into())], "zh").unwrap();
        assert!(block.contains("build_test_package") && block.contains("调用工作流"));
    }
}
