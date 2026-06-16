//! 内置种子 Skill 目录(设计/09 推论7「出厂种子」)。
//!
//! Skill = 第四原语 = 场景化知识/playbook(「某类场景怎么把事做好」)。运行期有两类来源:
//! ① **内置种子**(本模块,编译期常量):随系统出厂,作初始飞轮种子,任何项目可用、不入 redb;
//! ② **已学 skill**(`learn_skill` → `Memory` 的 skill kind 节点):可被飞轮自学结晶、语义召回、越用越准。
//!
//! 脊柱把两类**合并**成常驻清单(名称 + 触发描述)拼进系统提示;`load_skill` 按名取正文,
//! 也是两类都查(已学优先,内置兜底)。这与工作流(内置 Global + 持久化 Project/Artifact)同构:
//! 静态出厂目录不是"独立存储子系统"(设计/09 拒的是后者),与「skill 长在记忆内核」不冲突——
//! 内置只是默认,真正生长在已学节点里。
//!
//! ★存储结构(低耦合)★:每个种子的 playbook 正文是 `seeds/<name>.md` 独立文件(`include_str!` 编入),
//! 一 skill 一文件——便于无限扩充、各自可独立编辑/审阅,不把上百行字符串堆进本 .rs(防 god 文件)。
//! 新增种子 = 写一个 `seeds/<name>.md`(skill_format 格式)+ 在 `SEEDS` 加一行。
//!
//! ★为什么是知识不是执行器★:这些 playbook(反向定位/读前改后/造物设计/异步取消…)若硬编码成专用执行器
//! 就是堆特例、违架构公理。它们是"带着判断用通用工具(shell/file/code_search/render_artifact/auto-debug)
//! 施展"的知识。出厂集覆盖软件开发生命周期 + 本项目自己的血泪结晶(终止失效/造物重画失控/清 WebKit/
//! god 文件拆分/全自动调试),是飞轮哲学的兑现——把踩过的坑变成可复用、可被自学加厚的知识。
//! (参考业界 IDE 的 playbook 思路打磨,但立足 GrowBox 自身原则与工具,不照搬、不点名。)

/// 一个内置 skill 的静态条目。
pub struct SeedSkill {
    /// 唯一名(load_skill 按它取;kebab-case)。
    pub name: &'static str,
    /// 分类(海量库治理维度,设计/09 推论8):code/debug/ui/web/…。清单超量时按分类索引展示。
    pub category: &'static str,
    /// 触发描述(一句话,何时用——进常驻清单供 AI 主动挑)。
    pub trigger: &'static str,
    /// playbook 正文(markdown,AI 带着判断施展;由 seeds/<name>.md 编入)。
    pub body: &'static str,
}

/// 分类 → 中文域名(清单分类索引用;未知分类原样透传)。
pub fn category_label(cat: &str) -> &str {
    match cat {
        "code" => "代码编写",
        "debug" => "验证与调试",
        "ui" => "UI 设计",
        "web" => "网页调试",
        other => other,
    }
}

/// 内置种子 skill 全集(出厂 playbook)。新增 = 写 seeds/<name>.md + 在此加一行(零机制改动)。
/// 顺序 = 清单"内置优先"展示序:代码编写 → 验证/调试 → UI 设计 → 网页反向定位。
pub const SEEDS: &[SeedSkill] = &[
    SeedSkill { name: "read-before-write", category: "code", trigger: "动手改一处自己还没完全看懂的代码前(尤其跨文件/有调用方的改动)", body: include_str!("seeds/read-before-write.md") },
    SeedSkill { name: "disciplined-change", category: "code", trigger: "要落地一处代码改动时(决定改多大范围、怎么不引入新问题)", body: include_str!("seeds/disciplined-change.md") },
    SeedSkill { name: "review-your-diff", category: "code", trigger: "改完代码、收尾前自查这次 diff 有没有引入 bug 时", body: include_str!("seeds/review-your-diff.md") },
    SeedSkill { name: "write-tests-that-matter", category: "code", trigger: "为一处改动写测试、或判断现有测试够不够时", body: include_str!("seeds/write-tests-that-matter.md") },
    SeedSkill { name: "safe-refactor", category: "code", trigger: "要重构(拆大文件/挪模块/改结构)但必须保证行为不变时", body: include_str!("seeds/safe-refactor.md") },
    SeedSkill { name: "async-cancellation-discipline", category: "code", trigger: "写异步/长任务/流式循环,要保证能被「终止」+ 不挂死时", body: include_str!("seeds/async-cancellation-discipline.md") },
    SeedSkill { name: "plan-before-big-change", category: "code", trigger: "要做一个多步/跨文件的功能或较大改动,动手前要先定方案时", body: include_str!("seeds/plan-before-big-change.md") },
    SeedSkill { name: "orient-in-unfamiliar-code", category: "code", trigger: "进入一个不熟悉的代码库/模块,动手前要先建立全局认识时", body: include_str!("seeds/orient-in-unfamiliar-code.md") },
    SeedSkill { name: "name-and-shape-apis", category: "code", trigger: "设计一个函数/类型/模块的接口、或给东西命名时", body: include_str!("seeds/name-and-shape-apis.md") },
    SeedSkill { name: "optimize-by-measuring", category: "code", trigger: "觉得某处慢/想做性能优化时", body: include_str!("seeds/optimize-by-measuring.md") },
    SeedSkill { name: "commit-in-logical-units", category: "code", trigger: "把改动提交进版本控制时", body: include_str!("seeds/commit-in-logical-units.md") },
    SeedSkill { name: "verify-by-running", category: "debug", trigger: "改完一件事、要向用户汇报\"做好了\"之前", body: include_str!("seeds/verify-by-running.md") },
    SeedSkill { name: "investigate-before-fix", category: "debug", trigger: "遇到真机/运行期的 bug 或异常,要先定位根因而不是急着试改时", body: include_str!("seeds/investigate-before-fix.md") },
    SeedSkill { name: "self-test-with-auto-debug", category: "debug", trigger: "改了 UI/交互,要自己验证它真能用(不靠人当肉调试器)时", body: include_str!("seeds/self-test-with-auto-debug.md") },
    SeedSkill { name: "artifact-ui-craft", category: "ui", trigger: "用 render_artifact 现造一个 UI(棋盘/表单/图表/小工具)给用户交互时", body: include_str!("seeds/artifact-ui-craft.md") },
    SeedSkill { name: "ui-respects-attention", category: "ui", trigger: "设计/改动任何给用户看的界面(造物或 GrowBox 本体 UI)时", body: include_str!("seeds/ui-respects-attention.md") },
    SeedSkill { name: "solidjs-frontend-change", category: "ui", trigger: "动 GrowBox 自身前端(crates/growbox-gui/frontend,改组件/状态/样式)时", body: include_str!("seeds/solidjs-frontend-change.md") },
    SeedSkill { name: "reply-formatting", category: "ui", trigger: "给用户写回复、且输出里包含链接、表格、列表、代码等需要正确渲染的格式时", body: include_str!("seeds/reply-formatting.md") },
    SeedSkill { name: "web-debug-source-locate", category: "web", trigger: "网页调试窗框选元素后,需要把选中的 DOM 反向定位到本地源码再修改时", body: include_str!("seeds/web-debug-source-locate.md") },
    SeedSkill { name: "web-qa-self-feedback", category: "web", trigger: "要测自己做的网页功能对不对(按钮跳转/表单提交/各种交互),像真人测试员有计划地真操作再核对时", body: include_str!("seeds/web-qa-self-feedback.md") },
];

/// 按名取内置种子的正文(精确、大小写不敏感)。
pub fn seed_body(name: &str) -> Option<&'static str> {
    SEEDS.iter().find(|s| s.name.eq_ignore_ascii_case(name)).map(|s| s.body)
}

/// 内置种子名集合(小写)。用来判一个已学节点是否其实是"已物化的内置种子"(连接时 `ensure_seed_nodes`
/// 写入,名==种子名)——这类节点在清单/列表里归到**内置(按分类)**而非重复进「已学」。
fn seed_name_set() -> std::collections::BTreeSet<String> {
    SEEDS.iter().map(|s| s.name.to_ascii_lowercase()).collect()
}

/// ★内置种子嵌入成节点(设计/09「日后可加」点)★:把内置种子物化成 `skill` kind 记忆节点,使其
/// **也享语义召回 + 高置信自动注入正文**(与已学 skill 一视同仁),不再只是常驻清单里的一行——
/// 当某场景强匹配某种子时,整篇 playbook 自动注入(省一次 `load_skill` 往返),消除"内置只浮名、
/// 已学才自动注入"的不对称(设计/09 第 90-93 行标记的可移除简化)。
///
/// 幂等:已存在同名 skill 节点(已物化 / 用户 `learn_skill` 同名覆盖版)则跳过——连接/重启重复调用安全,
/// 且不覆盖用户的覆盖版。嵌入不在此处做(连接要快):由 idle `ensure_embeddings_batch` 统一补,复用嵌入
/// 管线,补完即可召回。仍保「内置始终可见」:常驻清单仍由静态 `SEEDS` 驱动按分类展示(见 `listing` 的
/// 种子名去重),与是否已物化无关。
pub fn ensure_seed_nodes(memory: &mut growbox_memory::Memory) {
    use std::collections::HashSet;
    // 一遍取齐已有 skill 节点名(避免每个种子各扫一遍时间线)。
    let existing: HashSet<String> = memory
        .learned_skill_listing()
        .into_iter()
        .map(|(name, _, _)| name.to_ascii_lowercase())
        .collect();
    for s in SEEDS {
        if existing.contains(&s.name.to_ascii_lowercase()) {
            continue; // 已物化 / 已被同名学习覆盖
        }
        memory.ingest_skill(growbox_memory::skill_format::format(s.name, s.trigger, s.body));
    }
}

/// 一条 skill 的元信息(供设置 UI 列出 + 管理)。
pub struct SkillInfo {
    pub name: String,
    pub trigger: String,
    /// 分类(内置取 seed.category;已学暂归 "learned",S3 自动归类)。
    pub category: String,
    /// 来源:内置种子("builtin")或已学结晶("learned")。
    pub source: &'static str,
    /// 当前是否生效(总开关开 且 未被单独停用)。
    pub active: bool,
}

/// ★常驻清单(设计/09 推论4「清单为主」+ 推论8 海量库治理)★:把内置种子 + 已学 skill **按分类分组**
/// 拼进系统提示(稳定前缀,缓存安全;同 deferred_listing)。已学同名覆盖内置;停用的不列。
///
/// ★海量库 scale★:分类分组本身给结构;总数 ≤ list_max 时全列(名+触发);**超 list_max 时降级为
/// 「分类索引」**——每类只列名(省触发)+ 标注该类条数,正文/触发靠 `load_skill` 或语义召回(推论4)
/// 按需取。这样常驻体量随分类数(而非 skill 数)增长,千百个 skill 也不撑爆上下文。总开关关 → None。
pub fn listing(memory: &growbox_memory::Memory) -> Option<String> {
    let cfg = memory.skill_config();
    if !cfg.enabled {
        return None;
    }
    use std::collections::BTreeMap;
    let seeds = seed_name_set();
    // 一遍扫已学节点:**种子名** → 取最新 trigger(覆盖版,用于内置条目显示);**非种子名** → 收进
    // 「已学」(同名取最新、过滤停用)。把"已物化的内置种子"归回内置(按分类),不重复进「已学」,
    // 从而 listing 输出与种子是否已物化无关(保「内置始终可见」)。
    let mut seed_override: BTreeMap<String, String> = BTreeMap::new();
    let mut extra: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (name, trigger, _id) in memory.learned_skill_listing() {
        let key = name.to_ascii_lowercase();
        if seeds.contains(&key) {
            seed_override.insert(key, trigger); // 创建序 → 最后一个=最新版
        } else if cfg.is_active(&name) {
            extra.insert(key, (name, trigger));
        }
    }
    // 按分类收集 (label, Vec<(name, trigger)>);内置按 category,真·已学归入「已学」。保持 SEEDS 顺序。
    let mut groups: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut idx: BTreeMap<String, usize> = BTreeMap::new();
    let push = |label: String, name: String, trigger: String, groups: &mut Vec<(String, Vec<(String, String)>)>, idx: &mut BTreeMap<String, usize>| {
        let i = *idx.entry(label.clone()).or_insert_with(|| { groups.push((label, Vec::new())); groups.len() - 1 });
        groups[i].1.push((name, trigger));
    };
    let mut total = 0usize;
    for s in SEEDS {
        if !cfg.is_active(s.name) {
            continue; // 被停用
        }
        // 已物化/被覆盖版用最新 trigger;否则静态 trigger。无论是否物化,内置都按分类常驻显示。
        let trigger = seed_override
            .get(&s.name.to_ascii_lowercase())
            .cloned()
            .unwrap_or_else(|| s.trigger.to_string());
        push(category_label(s.category).to_string(), s.name.to_string(), trigger, &mut groups, &mut idx);
        total += 1;
    }
    for (name, trigger) in extra.values() {
        push("已学".to_string(), name.clone(), trigger.clone(), &mut groups, &mut idx);
        total += 1;
    }
    if total == 0 {
        return None;
    }
    // 超上限 → 分类索引模式(只列名,省触发);否则全列(名:触发)。
    let compact = cfg.list_max > 0 && total > cfg.list_max;
    let mut blocks = Vec::new();
    for (label, items) in &groups {
        if compact {
            let names: Vec<&str> = items.iter().map(|(n, _)| n.as_str()).collect();
            blocks.push(format!("【{label}·{}】{}", items.len(), names.join(" / ")));
        } else {
            let lines: Vec<String> = items.iter().map(|(n, t)| format!("  - {n}:{t}")).collect();
            blocks.push(format!("【{label}】\n{}", lines.join("\n")));
        }
    }
    let tail = if compact {
        "\n(清单较大已折叠为分类索引:看某 skill 的用法直接 load_skill(名称);拿不准就按场景描述、相关 skill 会被语义召回。)"
    } else {
        ""
    };
    Some(format!(
        "[可用 Skill] 按场景沉淀的 playbook(场景化知识),分类列出。遇到匹配场景时先 load_skill(名称) \
拉回完整步骤,再带着它用通用工具施展;没有完全匹配的就自行判断:\n{}{}",
        blocks.join("\n"),
        tail
    ))
}

/// 强匹配自动注入正文的条数上限(防一次注入过多 playbook 撑爆上下文/缓存)。
const AUTOLOAD_MAX_BODIES: usize = 3;

/// ★语义召回 + 高置信自动加载(设计/09 推论4 + 用户定调「动态聚类·高置信注入正文」)★:
/// 按当前场景召回相关 skill(`retrieve_skills` 走指针网 QKV,Q=场景、正反K 学习,**涌现的那一组**)。
/// 据置信度(Hit.score)分流:
/// - **强匹配(score ≥ `autoload_threshold`)**:直接把整篇 playbook 正文注入上下文(零 load_skill
///   调用 = 省 LLM 调用 + 加速),至多 `AUTOLOAD_MAX_BODIES` 篇(防撑爆),按分降序取最强的。
/// - **一般匹配**:只浮现名+触发,AI 自行决定要不要 `load_skill`。
///
/// 停用/总开关关的已在 `Memory::retrieve_skills` 内过滤。无召回 → None。
pub fn render_recalled(hits: &[growbox_memory::Hit], autoload_threshold: f32) -> Option<String> {
    if hits.is_empty() {
        return None;
    }
    // 按分降序,强匹配优先拿正文额度。
    let mut ranked: Vec<&growbox_memory::Hit> = hits.iter().collect();
    ranked.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

    let mut bodies: Vec<String> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    for h in ranked {
        let Some((name, trigger)) = growbox_memory::skill_format::parse_head(&h.content) else { continue };
        if h.score >= autoload_threshold && bodies.len() < AUTOLOAD_MAX_BODIES {
            // 强匹配:整篇正文直通(h.content 即 playbook 全文)。
            bodies.push(format!("# {name}(已据场景自动加载)\n{}", h.content));
        } else {
            names.push(format!("  - {name}:{trigger}"));
        }
    }
    if bodies.is_empty() && names.is_empty() {
        return None;
    }
    let mut out = String::new();
    if !bodies.is_empty() {
        out.push_str("[已自动加载的 Skill] 据当前场景强匹配,以下 playbook 已为你直接载入,按它施展:\n\n");
        out.push_str(&bodies.join("\n\n---\n\n"));
    }
    if !names.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str("[相关 Skill] 这些可能也有用,需要就 load_skill(名称) 取完整步骤:\n");
        out.push_str(&names.join("\n"));
    }
    Some(out)
}

/// 按名取一个 skill 的完整 playbook 正文:**已学优先,内置种子兜底**(同 listing 的覆盖语义)。
/// 总开关关 / 该 skill 被停用 → None(回执提示)。无则 None。
pub fn load_body(memory: &growbox_memory::Memory, name: &str) -> Option<String> {
    let cfg = memory.skill_config();
    if !cfg.is_active(name) {
        return None;
    }
    memory
        .learned_skill_body(name)
        .or_else(|| seed_body(name).map(str::to_string))
}

/// 全部可用 skill 名(已学 + 内置,去重,排除停用),供 load_skill 未命中时回执"你可以加载这些"。
pub fn available_names(memory: &growbox_memory::Memory) -> Vec<String> {
    use std::collections::BTreeSet;
    let cfg = memory.skill_config();
    let mut set: BTreeSet<String> = SEEDS
        .iter()
        .filter(|s| cfg.is_active(s.name))
        .map(|s| s.name.to_string())
        .collect();
    for (name, _t, _id) in memory.learned_skill_listing() {
        if cfg.is_active(&name) {
            set.insert(name);
        }
    }
    set.into_iter().collect()
}

/// ★设置 UI 用★:全部 skill 的元信息(内置 + 已学,去重;同名以已学为准),含来源 + 是否生效。
/// 不受总开关/停用过滤(UI 要列出全部、含被停用的,好让用户重新启用)。按 内置在前、名字序。
pub fn all_skills(memory: &growbox_memory::Memory) -> Vec<SkillInfo> {
    use std::collections::BTreeMap;
    let cfg = memory.skill_config();
    let seeds = seed_name_set();
    // 种子名 → 最新 trigger(覆盖版,显示在内置条目上);非种子名 → 真·已学条目(同名去重取最新)。
    // 已物化的内置种子归到内置(source=builtin),不重复成一条 learned。
    let mut seed_override: BTreeMap<String, String> = BTreeMap::new();
    let mut extra: BTreeMap<String, (String, String)> = BTreeMap::new();
    for (name, trigger, _id) in memory.learned_skill_listing() {
        let key = name.to_ascii_lowercase();
        if seeds.contains(&key) {
            seed_override.insert(key, trigger);
        } else {
            extra.insert(key, (name, trigger)); // 不过滤停用:UI 要列全(含停用的,好让用户重启)
        }
    }
    let mut out: Vec<SkillInfo> = Vec::new();
    for s in SEEDS {
        let trigger = seed_override
            .get(&s.name.to_ascii_lowercase())
            .cloned()
            .unwrap_or_else(|| s.trigger.to_string());
        out.push(SkillInfo {
            name: s.name.to_string(),
            trigger,
            category: s.category.to_string(),
            source: "builtin",
            active: cfg.is_active(s.name),
        });
    }
    for (name, trigger) in extra.values() {
        out.push(SkillInfo {
            name: name.clone(),
            trigger: trigger.clone(),
            category: "learned".to_string(),
            source: "learned",
            active: cfg.is_active(name),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_are_well_formed_and_parseable() {
        assert!(!SEEDS.is_empty());
        let mut seen = std::collections::HashSet::new();
        for s in SEEDS {
            assert!(!s.name.is_empty() && !s.trigger.is_empty() && !s.body.is_empty());
            assert!(seen.insert(s.name.to_ascii_lowercase()), "种子名重复:{}", s.name);
            // 正文必须能被 skill_format 解析出头(脊柱清单/加载依赖它),且与条目 name/trigger 一致。
            let (name, trigger) = growbox_memory::skill_format::parse_head(s.body)
                .unwrap_or_else(|| panic!("种子 {} 正文头不可解析", s.name));
            assert_eq!(name, s.name, "种子 {} 正文头 name 不一致", s.name);
            assert_eq!(trigger, s.trigger, "种子 {} 正文头 trigger 不一致", s.name);
        }
    }

    #[test]
    fn seed_body_lookup() {
        assert!(seed_body("web-debug-source-locate").is_some());
        assert!(seed_body("WEB-DEBUG-SOURCE-LOCATE").is_some()); // 大小写不敏感
        assert!(seed_body("artifact-ui-craft").is_some()); // 新增 UI 簇
        assert!(seed_body("nope").is_none());
    }

    // --- 内置种子嵌入成节点(0-OPUS37):物化 + 幂等 + 清单去重 + 召回 ---

    use async_trait::async_trait;
    use growbox_memory::{Memory, Subconscious};

    /// Mock 潜意识:含关键词 → [1,0],否则 → [0,1];judge 按子串。够测召回(强匹配 cosine=1≥阈)。
    struct KwSub {
        kw: String,
    }
    #[async_trait]
    impl Subconscious for KwSub {
        async fn embed(&self, text: &str) -> Vec<f32> {
            if text.contains(&self.kw) {
                vec![1.0, 0.0]
            } else {
                vec![0.0, 1.0]
            }
        }
        async fn judge_relevant(&self, query: &str, candidates: &[String]) -> Vec<usize> {
            candidates.iter().enumerate().filter(|(_, c)| c.contains(query)).map(|(i, _)| i).collect()
        }
    }

    #[test]
    fn ensure_seed_nodes_materializes_and_is_idempotent() {
        let mut m = Memory::new();
        assert_eq!(m.learned_skill_listing().len(), 0, "起步无 skill 节点");
        ensure_seed_nodes(&mut m);
        let n1 = m.learned_skill_listing().len();
        assert_eq!(n1, SEEDS.len(), "应把每个种子物化成一个节点");
        // 再调一次:已存在则跳过,不重复写。
        ensure_seed_nodes(&mut m);
        assert_eq!(m.learned_skill_listing().len(), n1, "幂等:重复调用不新增");
        // 物化后 load_body 仍取得正文(已学优先,内容==种子)。
        assert!(load_body(&m, "read-before-write").is_some());
    }

    #[test]
    fn listing_keeps_seeds_under_categories_after_materialization() {
        // 未物化时的清单(基线)。
        let mut m = Memory::new();
        let before = listing(&m).expect("有内置种子");
        assert!(before.contains("【代码编写】") && before.contains("read-before-write"));
        assert!(!before.contains("【已学】"), "无已学时不应有「已学」组");
        // 物化后:清单**不变**(种子仍按分类显示,不被甩进「已学」)。
        ensure_seed_nodes(&mut m);
        let after = listing(&m).expect("仍有清单");
        assert_eq!(after, before, "种子物化成节点后,常驻清单输出应保持一致(内置始终可见)");
    }

    #[test]
    fn genuine_learned_skill_shows_in_learned_section_only() {
        let mut m = Memory::new();
        ensure_seed_nodes(&mut m);
        // 学一个**非种子名**的 skill → 进「已学」,种子仍按分类。
        m.ingest_skill(growbox_memory::skill_format::format("deploy-blue-green", "做蓝绿部署时", "1. 起新栈\n2. 切流量"));
        let out = listing(&m).expect("有清单");
        assert!(out.contains("【已学】") && out.contains("deploy-blue-green"), "新学的非种子 skill 进「已学」: {out}");
        assert!(out.contains("【代码编写】") && out.contains("read-before-write"), "种子仍按分类: {out}");
        // 该名只出现一次(不在内置分类里重复)。
        assert_eq!(out.matches("deploy-blue-green").count(), 1, "已学 skill 不应重复出现");
        // all_skills:种子记 builtin、新学记 learned,种子不因物化而重复成 learned。
        let infos = all_skills(&m);
        let builtin = infos.iter().filter(|i| i.source == "builtin").count();
        assert_eq!(builtin, SEEDS.len(), "每个种子恰一条 builtin(物化不产生重复 learned)");
        assert_eq!(infos.iter().filter(|i| i.name == "deploy-blue-green").count(), 1);
    }

    #[tokio::test]
    async fn materialized_seed_is_semantically_recallable() {
        // 物化 + 嵌入(mock e5)后,种子可被 retrieve_skills 召回(此前只有已学 skill 能)。
        let mut m = Memory::new();
        ensure_seed_nodes(&mut m);
        // web-debug 种子的触发/正文含"框选";用它当关键词,查询也含"框选"→ 强匹配。
        let kw = "框选";
        assert!(seed_body("web-debug-source-locate").unwrap().contains(kw), "种子正文应含关键词(测试前提)");
        let sub = KwSub { kw: kw.to_string() };
        m.ensure_embeddings(&sub).await; // 补向量(idle 在真机做这步)
        let hits = m.retrieve_skills("网页框选后改源码", &sub).await;
        assert!(!hits.is_empty(), "物化的种子应能被语义召回");
        assert!(
            hits.iter().any(|h| h.content.contains("web-debug-source-locate")),
            "应召回到 web-debug 种子: {:?}",
            hits.iter().map(|h| h.content.lines().next().unwrap_or("")).collect::<Vec<_>>()
        );
    }
}
