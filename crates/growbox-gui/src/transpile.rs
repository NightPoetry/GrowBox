//! 提示词自转译(self-transpile prompts)—— 自我负责·输入侧(`设计/08-自我负责.md` 推论2)。
//!
//! 核心思想(decoder 自亲和):纯 decoder 模型最能执行"自己写出来的话"。所以把所有"喂给模型的提示词"
//! 用**消费该提示词的那个模型**按它自己的风格重写一遍,运行时优先用重写版。谁转译谁:
//! - 主模型可见(系统提示/工具说明/脚手架/自检)→ 由主模型转译;角色 `PromptRole::Main`。
//! - 潜意识可见(judge_relevant / judge_edge / distill)→ 由潜意识模型转译;角色 `PromptRole::Subconscious`。
//!   今天潜意识 == 主模型(同一个 LLM,见 `bridge.rs`),故二者解析到同一 model id、落同一个桶;将来真给
//!   潜意识加独立模型槽时,`model_for_role` 解析出不同 id → 覆盖层按 key 自动分桶,本模块一行不改。
//!
//! ## 设计要点(高内聚低耦合)
//! - **覆盖层 = 加法**:覆盖按 `(模型 id, 语言, 提示词键)` 分桶持久(redb kv);**原文永远是真理来源**。
//!   开关关 或 该桶无覆盖 → `tr()` 逐字返回原文 = 零行为变更。换模型/重转译干净,原文不丢。
//! - **唯一取用 chokepoint** = [`tr`]:所有提示词产出点在交给 LLM 前过这一道,返回覆盖或原文。
//! - **静态目录** = [`CATALOG`]:把散落在 `bridge.rs`/`render.rs` 的内联提示词收成具名条目(zh+en),
//!   既是转译扫描的来源,也让取用方按 key + 语言拿到(可能转译过的)文本。
//! - 全局状态(`enabled` + 两个 model id + 覆盖表)由 `connect` 经 [`configure`] 推入;在深层同步函数
//!   (registry/render/bridge)里直接 `tr()`,不必把状态层层穿参 —— 这是本特性唯一的进程级共享态。

use std::collections::HashMap;
use std::sync::OnceLock;
use parking_lot::RwLock;

use crate::tool_i18n::normalize_prompt_lang;

/// 覆盖层在 redb 里的 kv 键。值 = `HashMap<okey, 转译后文本>`(见 [`okey`])。
pub const OVERRIDES_KEY: &str = "prompt_transpile_overrides";

/// 消费某条提示词的"模型角色"。决定转译时用哪个模型、覆盖落哪个桶。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptRole {
    /// 主对话模型可见(系统提示、工具说明、脊柱脚手架、自检指令)。
    Main,
    /// 潜意识模型可见(检索判断 judge_relevant / 联想跳转 judge_edge / 飞轮压缩 distill)。
    Subconscious,
}

/// 静态提示词目录的一条:把内联提示词收成具名条目,zh/en 两份原文。
pub struct PromptEntry {
    /// 稳定键(覆盖分桶 + 取用按此 key)。不要改已上线的 key,否则旧覆盖失配。
    pub key: &'static str,
    pub role: PromptRole,
    /// 中文原文(prompt_lang=zh 用)。必须与接线前的硬编码逐字一致,保证默认中文用户零行为变更。
    pub zh: &'static str,
    /// 英文原文(prompt_lang=en 用)。
    pub en: &'static str,
}

/// 转译扫描单元(喂给「重写提示词」动作的一项)。静态条目 + 动态来源(系统提示/工具说明)拼成全表。
#[derive(Clone, Debug)]
pub struct Unit {
    pub key: String,
    pub role: PromptRole,
    /// 归一后的 prompt_lang:`"zh"` 或 `"en"`。
    pub lang: &'static str,
    pub original: String,
}

// ====== 静态提示词目录(内联提示词收编于此,zh+en)======
// zh 文本必须与接线前散落在 bridge.rs/render.rs 的硬编码**逐字一致**(默认中文用户零行为变更)。

/// 主动自检指令模板(`render::self_verify_prompt`)。含占位符 `{summary}`,取用方替换。
const SELF_VERIFY_ZH: &str = "[收尾前验收 · 重要] 你即将给出本次任务的结论:\n\
         「{summary}」\n\n\
         正式收口前,把里面**每一条「我做了什么 / 现在是什么状态」的声称,挨条拿真实证据去核**,不许凭记忆。按声称类型选硬核办法:\n\
         - 说「改了/写了某文件」→ 重新读那个文件,确认那处改动真在里面(凭印象不算)。\n\
         - 说「备份了/创建了某东西」→ 真去确认那个文件/备份此刻存在(读它/列目录/ls);**找不到就是没做**。\n\
         - 说「服务在跑/能打开/访问正常」→ **现在**真去请求一次(curl 或查端口),别拿之前某条命令里一闪而过的结果当数——**那时活着不等于现在还活着**。\n\
         - 说「全部 200 / 都通过 / 都改了」→ 真去逐个请求/逐个确认,别一句「都好了」带过。\n\
         核完按结果处置:\n\
         - 证据对得上 → 保留。\n\
         - 只是说法含糊/不准 → 改成准确说法。\n\
         - **声称做了、可证据显示没做 → 现在就补做**(别用「未验证」蒙混过去),补完再核一遍。\n\
         - 实在做不到 → 老实说「这条没做到」以及为什么,绝不含糊带过、绝不把没做成的说成做成了。\n\
         核实并把该补的补完后,**再次调用 finish**,只给经得起证据检验的结论。宁可说「只完成了 A、B,C 没成功」,也不许虚报完成。";
const SELF_VERIFY_EN: &str = "[Pre-finish acceptance check - important] You are about to deliver the conclusion of this task:\n\
         \"{summary}\"\n\n\
         Before finishing, take **every claim of \"what I did / what the state now is\" and check it one by one against real evidence** - never from memory. Pick a hard method per claim type:\n\
         - Claimed \"changed/wrote a file\" -> re-read that file and confirm the change is actually in it (impressions don't count).\n\
         - Claimed \"backed up / created something\" -> actually confirm that file/backup exists right now (read it / list the dir / ls); **if you can't find it, it wasn't done**.\n\
         - Claimed \"service is running / opens / reachable\" -> request it **now** (curl or check the port); do not trust a fleeting result from an earlier command - **alive then does not mean alive now**.\n\
         - Claimed \"all 200 / all pass / all changed\" -> actually request/confirm each one; don't wave it away with \"all good\".\n\
         Then act on the result:\n\
         - Evidence matches -> keep it.\n\
         - Merely vague/imprecise wording -> rewrite it accurately.\n\
         - **Claimed done but evidence shows not done -> do it now** (do not paper over it with \"unverified\"), then re-check.\n\
         - Genuinely impossible -> honestly say \"this one I did not do\" and why; never gloss over it, never report undone work as done.\n\
         After verifying and completing what's missing, **call finish again** with a conclusion that withstands scrutiny. Better to say \"I only finished A and B; C failed\" than to falsely report completion.";

/// judge_relevant 系统提示(`bridge.rs`)。
const JUDGE_RELEVANT_ZH: &str = "你是检索判断器。判断哪些候选与查询相关,只输出相关候选编号(从0起)的 JSON 数组,例如 [0,2]。无相关项输出 []。不要解释。";
const JUDGE_RELEVANT_EN: &str = "You are a retrieval relevance judge. Decide which candidates are relevant to the query, and output only a JSON array of the relevant candidate indices (0-based), e.g. [0,2]. Output [] if none are relevant. Do not explain.";

/// judge_edge 系统提示(`bridge.rs`)。
const JUDGE_EDGE_ZH: &str = "你是联想跳转判断器。综合这条边历次命中/被拒的提问,判断当前提问是否值得跳到这条边通向的内容。只输出 JSON: {\"jump\":true} 或 {\"jump\":false}。不要解释。";
const JUDGE_EDGE_EN: &str = "You are an associative-jump judge. Considering this edge's past matched/rejected queries, decide whether the current query is worth jumping to the content this edge leads to. Output only JSON: {\"jump\":true} or {\"jump\":false}. Do not explain.";

/// distill 系统提示(`bridge.rs`)。
const DISTILL_ZH: &str = "你是飞轮压缩器。从这组同类经验里提炼不变模式。\
            输出 JSON: {\"operation\":\"不变的操作模式\",\"expected\":\"不变的预期后果\",\"prerequisites\":[\"最少前提\"]}。\
            若它们其实没有共同模式,输出 {\"none\":true}。只输出 JSON。";
const DISTILL_EN: &str = "You are a flywheel compressor. Extract the invariant pattern from this group of similar experiences. \
            Output JSON: {\"operation\":\"invariant operation pattern\",\"expected\":\"invariant expected outcome\",\"prerequisites\":[\"minimal prerequisites\"]}. \
            If they share no common pattern, output {\"none\":true}. Output JSON only.";

/// propose_skill 系统提示(`bridge.rs`;设计/09 S3 飞轮自学提议)。LLM 兼当质量闸:多数簇应 none。
const PROPOSE_SKILL_ZH: &str = "你是经验结晶器。下面是一簇反复出现的同类经验。判断它们是否揭示了一个\
            **可复用、可泛化、值得命名沉淀**的做事 playbook(技能)。\
            **严格**:只有当它确实是一条今后遇到同类场景能照着做、且非显而易见常识的方法时才提议;\
            太具体(只对这一次)/ 噪音 / 已是常识 → 不提议。\
            提议则输出 JSON:{\"name\":\"kebab-case 短名\",\"trigger\":\"一句话:何时该用它\",\"body\":\"playbook 正文(markdown,带判断的步骤,简洁、指向具体动作)\"}。\
            不值得沉淀则输出 {\"none\":true}。只输出 JSON。";
const PROPOSE_SKILL_EN: &str = "You are an experience crystallizer. Below is a cluster of recurring similar experiences. \
            Decide whether they reveal a **reusable, generalizable, worth-naming** playbook (skill). \
            **Be strict**: propose only if it is a genuine method one could follow next time a similar situation arises AND is not obvious common sense; \
            too specific (one-off) / noise / already common sense -> do not propose. \
            If proposing, output JSON: {\"name\":\"kebab-case short name\",\"trigger\":\"one sentence: when to use it\",\"body\":\"playbook body (markdown, judgment-laden steps, concise, pointing at concrete actions)\"}. \
            If not worth crystallizing, output {\"none\":true}. Output JSON only.";
const CHUNK_DOC_ZH: &str = "你是文档破碎器。下面是一篇长文档**按句切好的编号原子句**(从0起)。\
            把它切成若干**语义连贯的块**:同一主题/同一条目的相邻句归一块,主题切换处另起一块。\
            只输出每个**新块的起始句编号**的 JSON 数组(升序,例如 [0,3,7];0 可省)。不要解释、不要改写任何句子。";
const CHUNK_DOC_EN: &str = "You are a document splitter. Below are the **numbered atomic sentences** (0-based) of a long document, already split by sentence. \
            Group them into **semantically coherent chunks**: adjacent sentences on the same topic/entry go together, start a new chunk where the topic switches. \
            Output only a JSON array of the **starting sentence index of each new chunk** (ascending, e.g. [0,3,7]; 0 may be omitted). Do not explain, do not rewrite any sentence.";

// --- 脊柱脚手架区头(render.rs);静态说明块,数据在其后追加。zh 与硬编码逐字一致。---
const RENDER_WORKING_ZH: &str =
    "========== 工作记忆区(WORKING MEMORY · 按相关性从长期记忆调入)==========\n\
     [本区说明] 下列是与当前任务相关、从长期记忆中调入的历史片段。\n\
     本区为非线性区:片段的先后位置不代表时间先后 —— 请严格按每块的「时间」字段判断发生顺序,\n\
     不要按它们在本区里的排列位置来推断先后。\n";
const RENDER_WORKING_EN: &str =
    "========== WORKING MEMORY (recalled from long-term memory by relevance) ==========\n\
     [About this region] Below are history fragments relevant to the current task, recalled from long-term memory.\n\
     This region is NON-LINEAR: a fragment's position does not imply chronological order -- judge the actual sequence strictly\n\
     by each block's \"time\" field, not by where it sits in this region.\n";
const RENDER_RECALL_ZH: &str =
    "========== 补充记忆(SUPPLEMENTARY RECALL · 据当前进展新检索调入)==========\n\
     [本区说明] 任务进行中,系统据你最近的思路/进展又检索了一次长期记忆,下列是**此刻新浮现**、\n\
     与当前进展相关的历史片段(开场未必给过 —— 例如你刚要动手才需要的某些信息)。\n\
     同为非线性区:请严格按每块「时间」字段判断先后,勿按它们在本区里的排列位置推断。\n";
const RENDER_RECALL_EN: &str =
    "========== SUPPLEMENTARY RECALL (re-retrieved by your current progress) ==========\n\
     [About this region] Mid-task, the system re-searched long-term memory using your recent reasoning/progress.\n\
     Below are history fragments that surfaced JUST NOW as relevant to where you are (not necessarily provided at the start --\n\
     e.g. info you only need now that you are about to act). NON-LINEAR: judge order strictly by each block's \"time\" field.\n";
const RENDER_RECENT_ZH: &str =
    "########## 最近记忆 · MOST RECENT(紧贴当前回合,按时间正序 旧→新)##########\n\
     [本区说明] 这是最近发生的对话/操作,时间上最贴近当前回合,按时间正序排列。\n";
const RENDER_RECENT_EN: &str =
    "########## MOST RECENT (closest to the current turn, chronological old->new) ##########\n\
     [About this region] These are the most recent conversations/actions, closest in time to the current turn, in chronological order.\n";
const RENDER_RECIPES_ZH: &str =
    "========== 相关项目流程(PROJECT RECIPES · 照做)==========\n\
     [本区说明] 下列是本项目沉淀的可复用流程,与当前任务相关。它们记录了\"在本项目做某事要碰哪些地方、\n\
     按什么顺序\"——这是项目约定、通用代码搜索找不到的涟漪面。动手前先看,据此把该改的地方一处不漏地做全;\n\
     若发现流程与当前代码不符(已过时),以当前代码为准并照实告知用户。\n";
const RENDER_RECIPES_EN: &str =
    "========== PROJECT RECIPES (follow them) ==========\n\
     [About this region] Below are reusable recipes distilled in this project, relevant to the current task. They record\n\
     \"which places to touch and in what order to do something in this project\" -- project conventions a generic code search won't find.\n\
     Read before acting, and make every required change with none missed; if a recipe conflicts with the current code (outdated),\n\
     follow the current code and tell the user honestly.\n";
const RENDER_EXEC_ZH: &str =
    "========== 可执行项目流程(EXECUTABLE · 优先直接运行)==========\n\
     [本区说明 · 重要] 下列项目流程已沉淀成**现成可运行的工作流**,且与当前任务相关。\n\
     ★若当前任务匹配其中某条,你应**首选直接调用它的工作流**(下方给出工作流名),由它按既定步骤可靠执行;\n\
     不要再手工 file_read/file_edit 逐步重做同一件事——那会丢掉这条流程沉淀下来的可靠性与正确顺序。★\n\
     仅当工作流明显不适用于当前情形时才回退手工。若发现流程已与当前代码不符(过时),以当前代码为准并照实告知用户。\n";
const RENDER_EXEC_EN: &str =
    "========== EXECUTABLE PROJECT RECIPES (prefer running directly) ==========\n\
     [About this region - important] The project recipes below have been distilled into **ready-to-run workflows** and are relevant to the current task.\n\
     * If the current task matches one, you should **prefer calling its workflow directly** (the workflow name is given below) to execute the established steps reliably;\n\
     do not redo the same thing step by step with file_read/file_edit -- that loses the reliability and correct ordering this recipe captured. *\n\
     Fall back to manual only when the workflow clearly does not fit. If a recipe conflicts with current code (outdated), follow current code and tell the user honestly.\n";
const NODE_GUIDANCE_ZH: &str =
    "[工作流「{wf}」· 进入节点「{node}」] {prompt}\n\
     (本步只有当前可用的工具;完成本步操作后会按既定流程自动进入下一步。需用户拍板可 ask_user,整件事做完可 finish。)";
const NODE_GUIDANCE_EN: &str =
    "[Workflow \"{wf}\" - entering node \"{node}\"] {prompt}\n\
     (This step exposes only the currently available tools; after you finish this step it auto-advances to the next per the workflow. Use ask_user when the user must decide, finish when the whole thing is done.)";

/// 全部静态条目。键稳定,勿改已上线 key。
pub const CATALOG: &[PromptEntry] = &[
    PromptEntry { key: "agent.self_verify", role: PromptRole::Main, zh: SELF_VERIFY_ZH, en: SELF_VERIFY_EN },
    PromptEntry { key: "subconscious.judge_relevant", role: PromptRole::Subconscious, zh: JUDGE_RELEVANT_ZH, en: JUDGE_RELEVANT_EN },
    PromptEntry { key: "subconscious.judge_edge", role: PromptRole::Subconscious, zh: JUDGE_EDGE_ZH, en: JUDGE_EDGE_EN },
    PromptEntry { key: "subconscious.distill", role: PromptRole::Subconscious, zh: DISTILL_ZH, en: DISTILL_EN },
    PromptEntry { key: "subconscious.propose_skill", role: PromptRole::Subconscious, zh: PROPOSE_SKILL_ZH, en: PROPOSE_SKILL_EN },
    PromptEntry { key: "subconscious.chunk_doc", role: PromptRole::Subconscious, zh: CHUNK_DOC_ZH, en: CHUNK_DOC_EN },
    PromptEntry { key: "render.working_header", role: PromptRole::Main, zh: RENDER_WORKING_ZH, en: RENDER_WORKING_EN },
    PromptEntry { key: "render.recall_header", role: PromptRole::Main, zh: RENDER_RECALL_ZH, en: RENDER_RECALL_EN },
    PromptEntry { key: "render.recent_header", role: PromptRole::Main, zh: RENDER_RECENT_ZH, en: RENDER_RECENT_EN },
    PromptEntry { key: "render.recipes_header", role: PromptRole::Main, zh: RENDER_RECIPES_ZH, en: RENDER_RECIPES_EN },
    PromptEntry { key: "render.executable_header", role: PromptRole::Main, zh: RENDER_EXEC_ZH, en: RENDER_EXEC_EN },
    PromptEntry { key: "render.node_guidance", role: PromptRole::Main, zh: NODE_GUIDANCE_ZH, en: NODE_GUIDANCE_EN },
];

// ====== 进程级状态(connect 推入)======

#[derive(Default)]
struct TranspileState {
    enabled: bool,
    main_model: String,
    sub_model: String,
    /// okey(model,lang,key) -> 转译后文本。
    overrides: HashMap<String, String>,
}

fn state() -> &'static RwLock<TranspileState> {
    static S: OnceLock<RwLock<TranspileState>> = OnceLock::new();
    S.get_or_init(|| RwLock::new(TranspileState::default()))
}

/// 覆盖表/查询的复合键:`model \u{1f} lang \u{1f} key`(单元分隔符,绝不出现在内容里)。
pub fn okey(model: &str, lang: &str, key: &str) -> String {
    format!("{model}\u{1f}{lang}\u{1f}{key}")
}

/// 连接时把当前开关 + 两个模型 id + 覆盖表整体推入(替换旧态)。
/// `sub_model`:今天传 == `main_model`(潜意识复用主模型);将来拆独立潜意识模型时传它自己的 id。
pub fn configure(enabled: bool, main_model: &str, sub_model: &str, overrides: HashMap<String, String>) {
    let mut st = state().write();
    st.enabled = enabled;
    st.main_model = main_model.to_string();
    st.sub_model = sub_model.to_string();
    st.overrides = overrides;
}

/// 仅翻开关(前端勾选即时生效,不必重连);模型与覆盖表保持不变。
pub fn set_enabled(enabled: bool) {
    state().write().enabled = enabled;
}

/// 仅替换覆盖表(「重写提示词」动作跑完后刷新内存版);开关与模型 id 不变。
pub fn set_overrides(overrides: HashMap<String, String>) {
    state().write().overrides = overrides;
}

/// 取当前覆盖表快照(动作起手时合并进去 —— 按模型分桶,不同模型/语言的旧覆盖保留)。
pub fn overrides_snapshot() -> HashMap<String, String> {
    state().read().overrides.clone()
}

/// 角色解析到模型 id(覆盖分桶 + 用哪个模型转译都看它)。
pub fn model_for_role(role: PromptRole) -> String {
    let st = state().read();
    match role {
        PromptRole::Main => st.main_model.clone(),
        PromptRole::Subconscious => st.sub_model.clone(),
    }
}

/// 当前是否启用(前端回显 / 取用方快速短路)。
pub fn is_enabled() -> bool {
    state().read().enabled
}

/// 当前覆盖条数(前端回显:转译产物规模)。
pub fn override_count() -> usize {
    state().read().overrides.len()
}

/// ★唯一取用 chokepoint★:把一条提示词在交给 LLM 前过这一道。
/// 关 / 模型空 / 该桶无覆盖 → 逐字返回 `original`(零行为变更);否则返回该模型该语言的转译版。
pub fn tr(key: &str, role: PromptRole, prompt_lang: &str, original: &str) -> String {
    let lang = normalize_prompt_lang(prompt_lang);
    let st = state().read();
    if !st.enabled {
        return original.to_string();
    }
    let model = match role {
        PromptRole::Main => &st.main_model,
        PromptRole::Subconscious => &st.sub_model,
    };
    if model.is_empty() {
        return original.to_string();
    }
    st.overrides
        .get(&okey(model, lang, key))
        .cloned()
        .unwrap_or_else(|| original.to_string())
}

/// 按 key + 语言从静态目录取(可能转译过的)文本。角色由目录条目自带。
/// 未知 key = 开发期写错,panic 不静默(出厂前测试拦)。
pub fn catalog(key: &str, prompt_lang: &str) -> String {
    let lang = normalize_prompt_lang(prompt_lang);
    let entry = CATALOG
        .iter()
        .find(|e| e.key == key)
        .unwrap_or_else(|| panic!("未知的转译目录键: {key}"));
    let original = if lang == "en" { entry.en } else { entry.zh };
    tr(key, entry.role, lang, original)
}

/// 静态目录展开成扫描单元(zh + en 各一份)。「重写提示词」动作以此为起点,再并入系统提示/工具说明。
pub fn static_units() -> Vec<Unit> {
    let mut v = Vec::with_capacity(CATALOG.len() * 2);
    for e in CATALOG {
        v.push(Unit { key: e.key.to_string(), role: e.role, lang: "zh", original: e.zh.to_string() });
        v.push(Unit { key: e.key.to_string(), role: e.role, lang: "en", original: e.en.to_string() });
    }
    v
}

/// 「重写这段提示词」的**常量** system 指令(转译动作用)。要求:保约束、保占位符、只输出重写结果。
/// ★缓存★:做成常量 system(原文放 user)→ 这段前缀在所有转译调用间稳定命中,每条只算原文+输出的新 token。
pub fn rewrite_system(lang: &str) -> &'static str {
    if normalize_prompt_lang(lang) == "en" {
        "You will be given a prompt (in the user message) that will later be fed to you yourself. \
         Rewrite it in your own most natural, easiest-to-execute-precisely phrasing, so that reading it feels like reading your own words. \
         Strictly preserve: (1) every constraint, instruction and output-format requirement, losing nothing; \
         (2) keep all {placeholders} exactly as-is, do not translate or alter them; \
         (3) add no commentary, do not answer it - output only the rewritten prompt itself."
    } else {
        "你会在 user 消息里收到一段「将来要喂给你自己的提示词」。请用你自己最自然、最容易准确执行的说法把它重写一遍,\
         使你读它时像读自己写的话。严格保持:① 全部约束、指令、输出格式要求一字不少;\
         ② 所有 {xxx} 占位符原样保留、不要翻译或改写;③ 不要新增解释、不要回答它,只输出重写后的提示词本身。"
    }
}

/// 提取一段文本里的 `{占位符}` 集合(简单括号扫描,不嵌套)。转译保真校验用。
pub fn placeholders(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = s[i + 1..].find('}') {
                let inner = &s[i + 1..i + 1 + end];
                // 只收"像标识符"的占位符(字母/数字/下划线),避开 JSON 例子里的 {"jump":true}。
                if !inner.is_empty()
                    && inner.chars().all(|c| c.is_alphanumeric() || c == '_')
                {
                    let token = format!("{{{inner}}}");
                    if !out.contains(&token) {
                        out.push(token);
                    }
                }
                i += end + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// 转译保真校验(机械、确定性,可单测):
/// - 重写非空(去空白后);
/// - 原文里的每个 `{占位符}` 都在重写里出现(防丢占位符 → format/replace 失配);
/// - 长度在原文的 [0.25x, 4x] 之间(防截断/跑飞);
///
/// 不通过 = 弃用该重写、保留原文。语义级校验(是否漏约束)由调用方另起一次 LLM 复核,best-effort。
pub fn fidelity_ok(original: &str, rewrite: &str) -> bool {
    let r = rewrite.trim();
    if r.is_empty() {
        return false;
    }
    for ph in placeholders(original) {
        if !rewrite.contains(&ph) {
            return false;
        }
    }
    let (olen, rlen) = (original.chars().count().max(1), r.chars().count());
    let ratio = rlen as f64 / olen as f64;
    (0.25..=4.0).contains(&ratio)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全局态是进程级单例,cargo 默认多线程跑测试 → 必须串行化,否则一个测试的 `configure`
    /// 会在另一个测试 configure 与断言之间把全局态冲掉(竞态 → 偶发 FAILED)。用一把测试锁串起来;
    /// `unwrap_or_else(into_inner)` 容忍前一个测试 panic 留下的毒化锁。
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn fresh(enabled: bool, model: &str) {
        configure(enabled, model, model, HashMap::new());
    }

    #[test]
    fn disabled_returns_original_verbatim() {
        let _lk = lock();
        fresh(false, "m-disabled");
        let mut ov = HashMap::new();
        ov.insert(okey("m-disabled", "zh", "k"), "覆盖版".to_string());
        configure(false, "m-disabled", "m-disabled", ov);
        assert_eq!(tr("k", PromptRole::Main, "zh-CN", "原文"), "原文", "关掉时即便有覆盖也走原文");
    }

    #[test]
    fn enabled_hits_override_else_original() {
        let _lk = lock();
        let model = "m-hit";
        let mut ov = HashMap::new();
        ov.insert(okey(model, "zh", "kk"), "中文覆盖".to_string());
        ov.insert(okey(model, "en", "kk"), "EN override".to_string());
        configure(true, model, model, ov);
        assert_eq!(tr("kk", PromptRole::Main, "zh-CN", "原文"), "中文覆盖");
        assert_eq!(tr("kk", PromptRole::Main, "en", "orig"), "EN override");
        // 没覆盖的 key → 原文
        assert_eq!(tr("other", PromptRole::Main, "zh", "原样"), "原样");
    }

    #[test]
    fn empty_model_falls_back_to_original() {
        let _lk = lock();
        configure(true, "", "", HashMap::new());
        assert_eq!(tr("k", PromptRole::Main, "zh", "原"), "原");
    }

    #[test]
    fn role_resolves_to_distinct_models_when_split() {
        let _lk = lock();
        // 模拟将来潜意识拆出独立模型:主/潜意识各自命中各自桶。
        let mut ov = HashMap::new();
        ov.insert(okey("main-m", "zh", "x"), "主转译".to_string());
        ov.insert(okey("sub-m", "zh", "x"), "潜意识转译".to_string());
        configure(true, "main-m", "sub-m", ov);
        assert_eq!(tr("x", PromptRole::Main, "zh", "o"), "主转译");
        assert_eq!(tr("x", PromptRole::Subconscious, "zh", "o"), "潜意识转译");
    }

    #[test]
    fn role_same_model_shares_bucket_today() {
        let _lk = lock();
        // 今天潜意识==主模型:同一 model id,同一覆盖即被两个角色共享。
        let mut ov = HashMap::new();
        ov.insert(okey("same", "zh", "y"), "共享转译".to_string());
        configure(true, "same", "same", ov);
        assert_eq!(tr("y", PromptRole::Main, "zh", "o"), "共享转译");
        assert_eq!(tr("y", PromptRole::Subconscious, "zh", "o"), "共享转译");
    }

    #[test]
    fn catalog_picks_lang_and_keys_are_unique() {
        let _lk = lock();
        fresh(false, "m-cat"); // 关:catalog 返回原文,便于断言原文内容
        assert!(catalog("subconscious.judge_relevant", "zh").contains("检索判断器"));
        assert!(catalog("subconscious.judge_relevant", "en").contains("relevance judge"));
        assert!(catalog("agent.self_verify", "zh").contains("{summary}"));
        assert!(catalog("agent.self_verify", "en").contains("{summary}"));
        // key 唯一
        let mut seen = std::collections::HashSet::new();
        for e in CATALOG {
            assert!(seen.insert(e.key), "重复 catalog key: {}", e.key);
        }
    }

    #[test]
    fn catalog_zh_matches_legacy_hardcoded_byte_for_byte() {
        let _lk = lock();
        // 默认中文用户零行为变更的护栏:catalog zh 文本必须与接线前的硬编码逐字一致。
        fresh(false, "m-legacy");
        assert_eq!(
            catalog("subconscious.judge_relevant", "zh"),
            "你是检索判断器。判断哪些候选与查询相关,只输出相关候选编号(从0起)的 JSON 数组,例如 [0,2]。无相关项输出 []。不要解释。"
        );
        assert_eq!(
            catalog("subconscious.judge_edge", "zh"),
            "你是联想跳转判断器。综合这条边历次命中/被拒的提问,判断当前提问是否值得跳到这条边通向的内容。只输出 JSON: {\"jump\":true} 或 {\"jump\":false}。不要解释。"
        );
    }

    #[test]
    fn placeholders_extracts_identifier_braces_only() {
        assert_eq!(placeholders("hi {summary} and {wf_name}!"), vec!["{summary}", "{wf_name}"]);
        // JSON 例子里的 {"jump":true} 不算占位符(含引号/冒号)。
        assert!(placeholders("输出 {\"jump\":true} 或 {\"jump\":false}").is_empty());
        // 去重
        assert_eq!(placeholders("{a} {a} {b}"), vec!["{a}", "{b}"]);
    }

    #[test]
    fn fidelity_rejects_dropped_placeholder_and_bad_length() {
        let orig = "请核对「{summary}」并改正。";
        assert!(fidelity_ok(orig, "核对一下「{summary}」,有错就改。"), "保留占位符+合理长度 → 通过");
        assert!(!fidelity_ok(orig, "核对并改正。"), "丢了 {{summary}} → 拒");
        assert!(!fidelity_ok(orig, "   "), "空 → 拒");
        assert!(!fidelity_ok("短", &"长".repeat(100)), "长度跑飞 → 拒");
        // 无占位符的普通提示词,长度合理即可。
        assert!(fidelity_ok("你是检索判断器。输出 JSON 数组。", "你是相关性判断器,输出 JSON 数组,不解释。"));
    }

    #[test]
    fn static_units_covers_both_langs() {
        let units = static_units();
        assert_eq!(units.len(), CATALOG.len() * 2);
        assert!(units.iter().any(|u| u.key == "subconscious.distill" && u.lang == "en"));
        assert!(units.iter().any(|u| u.key == "subconscious.distill" && u.lang == "zh"));
    }

    #[test]
    fn rewrite_system_is_constant_and_mentions_placeholders() {
        // 常量 system(不含原文)→ 缓存友好;明示保留占位符。
        let zh = rewrite_system("zh");
        assert!(zh.contains("占位符") && zh.contains("只输出重写"));
        assert!(!zh.contains("{summary}"), "system 是常量,不嵌入具体原文");
        assert!(rewrite_system("en").contains("placeholders"));
    }
}
