//! ★C1 实验:工具懒加载对 deepseek KV 缓存的真实影响★(真机直连,默认 #[ignore])。
//!
//! 验证 C1 的核心技术主张:**改 tools 数组会破坏 KV 缓存前缀;懒加载让 tools 恒定 → 缓存不破**。
//! 手段:同一(大)prompt,只变 tools 数组,直接读 deepseek `usage.prompt_cache_hit_tokens`(非流式)。
//! 用**真实 registry 的工具载荷**(不是合成),so 反映真实请求。
//!
//! 跑法(用户开服务器 API 后):
//!   DEEPSEEK_API_KEY=<key> cargo test -p growbox-gui --test live_lazy_cache -- --ignored --nocapture
//!
//! 读法:
//!   - "重复同一 tools" 两次:第二次 cache_hit 应大涨(缓存生效的基线)。
//!   - "懒关:换 tools(全量→节点子集)":若 cache_hit 相对基线**掉下来** = 改 tools 破坏缓存(C1 要修的病)。
//!   - "懒开:tools 恒定(核心不变)":cache_hit 应**保持高位** = C1 的修复有效。

use std::collections::HashSet;

use growbox_core::ToolDef;
use growbox_gui::registry::Registry;
use growbox_gui::tasks::TaskManager;
use serde_json::{json, Value};

const BASE: &str = "https://api.deepseek.com";
const MODEL: &str = "deepseek-v4-flash";

/// 把 ToolDef 列表序列化成 deepseek tools 字段(与 client.rs build_body 一致)。
fn tools_json(defs: &[ToolDef]) -> Vec<Value> {
    defs.iter()
        .map(|t| json!({
            "type": "function",
            "function": { "name": t.name, "description": t.description, "parameters": t.params }
        }))
        .collect()
}

/// 发一次**非流式**请求(任意 messages),返回 (prompt_tokens, cache_hit, cache_miss)。
async fn probe_msgs(http: &reqwest::Client, key: &str, messages: &[Value], tools: &[Value]) -> (i64, i64, i64) {
    let body = json!({
        "model": MODEL, "messages": messages, "tools": tools, "max_tokens": 8, "stream": false
    });
    let resp = http
        .post(format!("{BASE}/chat/completions"))
        .header("Authorization", format!("Bearer {key}"))
        .json(&body)
        .send()
        .await
        .expect("请求失败");
    let status = resp.status();
    let v: Value = resp.json().await.expect("解析响应失败");
    assert!(status.is_success(), "API 非 2xx: {status} {v}");
    let u = v.get("usage").cloned().unwrap_or_default();
    let g = |k: &str| u.get(k).and_then(|x| x.as_i64()).unwrap_or(-1);
    (g("prompt_tokens"), g("prompt_cache_hit_tokens"), g("prompt_cache_miss_tokens"))
}

/// 发一次(system + user),返回 (prompt_tokens, cache_hit, cache_miss)。
async fn probe(http: &reqwest::Client, key: &str, system: &str, user: &str, tools: &[Value]) -> (i64, i64, i64) {
    let messages = vec![json!({"role": "system", "content": system}), json!({"role": "user", "content": user})];
    probe_msgs(http, key, &messages, tools).await
}

#[tokio::test]
#[ignore = "打真 API,需 DEEPSEEK_API_KEY + 服务器在线"]
async fn lazy_tools_cache_impact() {
    let key = std::env::var("DEEPSEEK_API_KEY").expect("需要环境变量 DEEPSEEK_API_KEY");
    let http = reqwest::Client::new();

    // 真实工具载荷:懒关全量 / 懒关节点子集 / 懒开核心常驻。
    let reg_full = Registry::with_builtins(TaskManager::new());
    let t_full = tools_json(&reg_full.tools_for("zh", None, &HashSet::new())); // 懒关·普通模式 = 全量工具
    let t_node = tools_json(&reg_full.tools_for("zh", Some(("create_artifact_workflow", "design")), &HashSet::new())); // 懒关·节点收窄子集

    let mut reg_lazy = Registry::with_builtins(TaskManager::new());
    reg_lazy.set_lazy_tools(true, growbox_core::Settings::default().deferred_tools);
    let t_core = tools_json(&reg_lazy.tools_for("zh", None, &HashSet::new())); // 懒开 = 核心常驻(恒定)

    // 足够大的固定 system(让前缀超过 deepseek 缓存阈值,确保可缓存)。
    let big = "你是 GrowBox 编码助手,工作在项目沙箱内,严格遵守安全门与项目约定。".repeat(80);
    let system = format!("{big}\n固定系统提示结束。");
    let user = "请用一句话确认你已就绪。";

    println!("\n========== C1 工具懒加载 · 缓存影响实验 ==========");
    println!("tools 规模:全量 {} 个 / 节点子集 {} 个 / 懒开核心 {} 个\n", t_full.len(), t_node.len(), t_core.len());

    let fmt = |label: &str, r: (i64, i64, i64)| {
        let (pt, hit, miss) = r;
        println!("[{label}] prompt_tokens={pt}  cache_hit={hit}  cache_miss={miss}  命中率={:.0}%", if pt > 0 { hit as f64 * 100.0 / pt as f64 } else { 0.0 });
        hit
    };

    // 0) 预热:先用全量 tools 打一次,把"system+全量tools"前缀写进缓存。
    let warm = probe(&http, &key, &system, user, &t_full).await;
    fmt("预热(全量 tools)", warm);

    // 1) 基线:重复同一全量 tools → 缓存应大涨(证明缓存确实生效、prompt 足够大可缓存)。
    let base = probe(&http, &key, &system, user, &t_full).await;
    let base_hit = fmt("基线·重复同一 tools", base);

    // 2) ★懒关·换 tools★:同 system,tools 换成节点子集(模拟进工作流节点)。若命中相对基线掉 = 改 tools 破缓存。
    let off = probe(&http, &key, &system, user, &t_node).await;
    let off_hit = fmt("懒关·换 tools(全量→节点子集)", off);

    // 3) ★懒开·tools 恒定★:再发一次核心常驻(先预热一次再测,与基线同样条件)。
    let _ = probe(&http, &key, &system, user, &t_core).await; // 预热核心前缀
    let on = probe(&http, &key, &system, user, &t_core).await;
    let on_hit = fmt("懒开·tools 恒定(核心常驻)", on);

    println!("\n---------- 结论 ----------");
    println!("改 tools 后命中相对基线变化:{} → {}（{}）", base_hit, off_hit,
        if off_hit < base_hit { "★掉了 = 改 tools 破坏缓存,C1 的病确实存在★" } else { "没掉 = 该端点 tools 不在缓存前缀/或阈值未达,需复核" });
    println!("懒开恒定 tools 命中:{}（应保持高位 = C1 修复有效)", on_hit);
    println!("==========================================\n");

    // 软断言:基线必须命中(否则 prompt 太小没进缓存,实验无效)。命中差异只打印不强断(端点行为为准)。
    assert!(base_hit > 0, "基线 cache_hit=0:prompt 可能未达缓存阈值,加大 system 重试;或端点不支持 prompt 缓存");
}

/// ★补充实验:真实多轮工作流任务里 C1 端到端省多少 token★
/// 模拟 6 轮工作流(对话 append-only 增长)+ 节点切换换工具。懒关:节点切换处换 tools →
/// 整段已缓存前缀全毁、重算;懒开:tools 恒定 → 只新增消息 miss、增长前缀持续命中。
/// 量累积 cache_hit / cache_miss(= 真账:每轮重算多少 token)+ 总耗时。
#[tokio::test]
#[ignore = "打真 API,需 DEEPSEEK_API_KEY + 服务器在线"]
async fn lazy_tools_cache_multiturn_workflow() {
    let key = std::env::var("DEEPSEEK_API_KEY").expect("需要 DEEPSEEK_API_KEY");
    let http = reqwest::Client::new();

    let reg = Registry::with_builtins(TaskManager::new());
    let full = tools_json(&reg.tools_for("zh", None, &HashSet::new()));
    let node = tools_json(&reg.tools_for("zh", Some(("create_artifact_workflow", "design")), &HashSet::new()));
    let mut reg_lazy = Registry::with_builtins(TaskManager::new());
    reg_lazy.set_lazy_tools(true, growbox_core::Settings::default().deferred_tools);
    let core = tools_json(&reg_lazy.tools_for("zh", None, &HashSet::new()));

    // 6 轮工具序列:懒关在 t2(进节点)/t4(回普通)/t5(再进节点)三处换 tools;懒开恒定。
    let off_seq: [&Vec<Value>; 6] = [&full, &node, &node, &full, &node, &full];
    let on_seq: [&Vec<Value>; 6] = [&core, &core, &core, &core, &core, &core];

    let big = "你是 GrowBox 编码助手,严格遵守安全门与项目约定,工作在项目沙箱内。".repeat(80);

    // 跑一条 6 轮会话:每轮发当前 messages 快照,再 append 一对(assistant+user)增长上下文。
    // run_tag 让两条会话的 system 前缀不同 → 缓存互不污染(否则懒开会命中懒关建的缓存)。
    async fn run_session(
        http: &reqwest::Client,
        key: &str,
        run_tag: &str,
        big: &str,
        seq: &[&Vec<Value>; 6],
    ) -> (i64, i64, u128) {
        let mut messages = vec![
            json!({"role": "system", "content": format!("[{run_tag}] {big}\n固定系统提示结束。")}),
            json!({"role": "user", "content": "帮我在本项目实现一个小功能,按工作流一步步来:先看代码,再改,再验证。"}),
        ];
        let (mut sum_hit, mut sum_miss) = (0i64, 0i64);
        let start = std::time::Instant::now();
        for (i, tools) in seq.iter().enumerate() {
            let (_pt, hit, miss) = probe_msgs(http, key, &messages, tools).await;
            sum_hit += hit.max(0);
            sum_miss += miss.max(0);
            // append 一轮"助手动手 + 工具结果"增长上下文(固定文本,保 byte-stable)。
            messages.push(json!({"role": "assistant", "content": format!("第{}步:我先用工具看一下当前情况,然后据此推进。", i + 1)}));
            messages.push(json!({"role": "user", "content": format!("第{}步工具结果:src/lib.rs 有 fn greet 定义在第1行,run 在第2行调用两次;无编译错误。继续下一步。", i + 1)}));
        }
        (sum_hit, sum_miss, start.elapsed().as_millis())
    }

    println!("\n========== C1 补充 · 多轮工作流端到端缓存实验(6 轮)==========");
    println!("懒关工具序列:full→node→node→full→node→full(3 处切换);懒开:core 恒定\n");

    let (off_hit, off_miss, off_ms) = run_session(&http, &key, "RUN-OFF", &big, &off_seq).await;
    let (on_hit, on_miss, on_ms) = run_session(&http, &key, "RUN-ON", &big, &on_seq).await;

    let rate = |h: i64, m: i64| if h + m > 0 { h as f64 * 100.0 / (h + m) as f64 } else { 0.0 };
    println!("[懒关] 累积 cache_hit={off_hit}  cache_miss={off_miss}  命中率={:.0}%  耗时={off_ms}ms", rate(off_hit, off_miss));
    println!("[懒开] 累积 cache_hit={on_hit}  cache_miss={on_miss}  命中率={:.0}%  耗时={on_ms}ms", rate(on_hit, on_miss));
    println!("\n---------- 结论 ----------");
    println!("懒开比懒关少重算(miss)token:{}（= C1 在这条 6 轮任务里省下的真账)", off_miss - on_miss);
    println!("（懒关每次节点切换把已缓存的增长前缀全毁重算;懒开 tools 恒定,增长前缀持续命中）");
    println!("==========================================\n");

    assert!(off_hit + off_miss > 0 && on_hit + on_miss > 0, "两条会话都应有 usage 数据");
}
