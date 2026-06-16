//! 项目与设置数据结构(从旧 project/settings crate 收进 core)。
//!
//! 只放数据结构;创建/切换/读写逻辑在 app。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 一个项目的配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub id: String,
    pub name: String,
    /// 可读写目录(AI 能读也能写)。
    pub writable_roots: Vec<PathBuf>,
    /// 只读目录(AI 能读、不能写)。
    pub readonly_roots: Vec<PathBuf>,
    /// 项目级安全授权(用户授权后追加,见 `设计/03-安全审查`)。
    #[serde(default)]
    pub grants: Vec<String>,
    /// 项目级已授权的内网/本机主机(web_fetch 等出站访问;小写、不含端口)。
    /// 与路径授权 `grants` 各自独立 —— 网络授权绝不放宽文件访问,反之亦然。
    #[serde(default)]
    pub net_grants: Vec<String>,
}

impl ProjectConfig {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        ProjectConfig {
            id: id.into(),
            name: name.into(),
            writable_roots: Vec::new(),
            readonly_roots: Vec::new(),
            grants: Vec::new(),
            net_grants: Vec::new(),
        }
    }
}

/// 全局/项目设置。安全相关字段遵循"只升不降"(见 `设计/03`),由 safety 层校验。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub api_base: String,
    #[serde(default)]
    pub api_key: String,
    pub model: String,
    /// 主模型最大输出 token(为 reasoning 预留,见 `实验记录/00`)。
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    /// Agent 循环最大轮数(每轮 = 一次 LLM 调用 + 工具结果回填)。
    /// 默认 1000(长任务够用);0 = 无限模式,只靠"无工具调用即完成"与空转/错误自然收口。
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    /// 提示词语言。
    #[serde(default = "default_lang")]
    pub lang: String,
    /// 嵌入(第一层 RAG)是否走远程槽位。false = 本地默认 embedder。
    #[serde(default)]
    pub embed_remote: bool,
    /// 远程嵌入服务 Base URL(OpenAI 兼容 `{base}/embeddings`;本地 Ollama/LM Studio 同此)。
    #[serde(default)]
    pub embed_api_base: String,
    /// 远程嵌入服务 API Key(与聊天 key 独立;DeepSeek 无嵌入端点)。
    #[serde(default)]
    pub embed_api_key: String,
    /// 远程嵌入模型名(如 text-embedding-3-small)。
    #[serde(default)]
    pub embed_model: String,
    /// ★独立潜意识模型槽(选填)★:judge_relevant/judge_edge/distill(及提示词自转译的 Subconscious 角色)
    /// 用的模型。**留空 = 复用主模型**(今天的行为);填了则潜意识走它自己的模型/端点/key,
    /// 转译「谁转译谁」按模型分桶自动各转各份(见 `transpile.rs`)。base/key 留空则回退主模型的。
    #[serde(default)]
    pub subconscious_model: String,
    #[serde(default)]
    pub subconscious_api_base: String,
    #[serde(default)]
    pub subconscious_api_key: String,
    /// P4 工作记忆区字符预算(≈ token)。0 = 用代码默认(随模型可调,大窗口模型设更大)。
    #[serde(default)]
    pub working_context_chars: u32,
    /// P4 "8K 最近记忆" ring 字符预算。0 = 用代码默认。
    #[serde(default)]
    pub recent_ring_chars: u32,
    /// 邻域缓存(精确层平铺单 LFU)容量条目数。0 = 用代码默认(256)。用户可在控制面板调
    /// (数值参数全可设,见 `设计/00-交互层` 推论9)。
    #[serde(default = "default_cache_cap")]
    pub neighbor_cache_cap: u32,
    /// 工具进度里命令/路径是否截断显示。false(默认)= 完整显示不省略;true = 过长则截断。
    /// 统一管所有工具上下文显示(shell 命令、文件路径等),由用户在设置里切换(用户决策 2026-06-02)。
    #[serde(default)]
    pub truncate_tool_display: bool,
    /// 自动模式:false(默认)= 手动,shell 命令逐条交用户批准;
    /// true = 自动,shell 经 LLM 安全审核员二次审,尽量全自动,碰隐私/个人文件夹才弹窗。
    /// 硬安全底线(危险命令 Deny、敏感密钥路径)两种模式都不可绕过。用户决策 2026-06-02。
    #[serde(default)]
    pub auto_mode: bool,
    /// ★danger 模式(为所欲为)★:全自动之上的最高放行档。true = 所有安全门一律放行(系统级操作/
    /// 敏感路径/危险命令/SSRF 全不拦),供无人值守自驱做"系统装 Python、全局 npm"等不卡授权。极高风险。
    /// ★`serde(skip)` 故意不持久★:每次启动默认 false,必须显式重新开启——防遗忘的危险模式随重启自启。
    /// 用户决策 2026-06-14。
    #[serde(skip)]
    pub danger_mode: bool,
    /// 用户配置的隐私文件夹绝对路径列表。命中(且未授权)时**必然弹窗**且着重强调"这是你设置的
    /// 隐私文件夹",需**二次确认**才加入当前项目读写。两种模式都不绕过。用户决策 2026-06-02。
    #[serde(default)]
    pub privacy_dirs: Vec<String>,
    /// 学习型指针匹配档:"weighted_cosine"(默认,廉价加权余弦)/ "llm_judge"(精确,读正负 K)。
    /// 见 `计划/指针-学习型边.md`;数值参数全可设(推论9)。
    #[serde(default = "default_pointer_match_mode")]
    pub pointer_match_mode: String,
    /// 指针跟随门:档A 加权余弦得分 ≥ 此值才走快车道(默认 0.80)。
    #[serde(default = "default_pointer_follow_threshold")]
    pub pointer_follow_threshold: f32,
    /// 反 K 一票否决阈值:query 与某反 K cosine ≥ 此值 → 阻断该边(默认 0.90,近似重复才否决)。
    #[serde(default = "default_pointer_neg_block_threshold")]
    pub pointer_neg_block_threshold: f32,
    /// 近似坍缩阈值:新 K 与已存 K cosine ≥ 此值 → weight+1 不新增(默认 0.93)。
    #[serde(default = "default_pointer_k_merge_threshold")]
    pub pointer_k_merge_threshold: f32,
    /// 档A 权重增益:`factor = 1 + gain·ln(weight)`(默认 0.30,被复用的 lane 更黏)。
    #[serde(default = "default_pointer_weight_gain")]
    pub pointer_weight_gain: f32,
    /// 单边正/负 K 数量上限:超界 LFU 淘汰最冷真实 K(默认 8,防膨胀第二道闸)。
    #[serde(default = "default_pointer_k_cap")]
    pub pointer_k_cap: u32,
    /// 档A 余弦命中后是否仍走前沿 judge 确认(默认 true=精确;false=命中即采纳省 LLM)。仅档A 有意义。
    #[serde(default = "default_pointer_force_judge")]
    pub pointer_force_judge: bool,
    /// 第一层 RAG 命中阈:首条余弦 ≥ 此值即返回不下沉(默认 0.85)。数值全可设(推论9)。
    #[serde(default = "default_retrieval_rag_hit_threshold")]
    pub retrieval_rag_hit_threshold: f32,
    /// 第一层 RAG 取回候选数(默认 8)。
    #[serde(default = "default_retrieval_rag_topk")]
    pub retrieval_rag_topk: u32,
    /// 精确层进图入口数:向量索引取 top-K 作下沉的门(默认 3)。
    #[serde(default = "default_retrieval_entry_k")]
    pub retrieval_entry_k: u32,
    /// 精确层入口最低相似度:低于此值的 top-K 不作入口(默认 0.30)。
    #[serde(default = "default_retrieval_entry_min_sim")]
    pub retrieval_entry_min_sim: f32,
    /// 精确层线性扫每批给 LLM judge 的节点数(默认 8)。
    #[serde(default = "default_retrieval_scan_batch")]
    pub retrieval_scan_batch: u32,
    /// 精确层线性主干路本次最多往回读多少个节点(有界"全量扫描"上限;默认 256)。
    /// 多批渐进扫直到攒够命中/扫满此预算/连续空批/扫到最早。数值全可设(推论9)。
    #[serde(default = "default_retrieval_scan_max")]
    pub retrieval_scan_max: u32,
    /// 项目软偏好系数:检索命中属当前项目 → 相似度乘 (1+此值) 再排序(默认 0.5;0=不偏好)。
    /// 软偏好非硬过滤——跨项目高相关仍可被召回。数值全可设(推论9)。
    #[serde(default = "default_retrieval_project_boost")]
    pub retrieval_project_boost: f32,
    /// 文档破碎阈:入场 content 字符数超过此值的大节点(粘贴的整篇文档)由 idle 按句破成小块,
    /// 各块独立向量 → 治长文窄问被稀释、RAG 必漏的盲区(默认 1500;0=关闭破碎)。数值全可设(推论9)。
    #[serde(default = "default_retrieval_chunk_min_chars")]
    pub retrieval_chunk_min_chars: u32,
    /// Agent 截断重试上限:工具调用被截成空参时最多翻倍 token 重试几次(默认 2)。数值全可设(推论9)。
    #[serde(default = "default_agent_max_token_retries")]
    pub agent_max_token_retries: u32,
    /// Agent 截断重试 token 上限:重试翻倍不超过此值(默认 32768)。
    #[serde(default = "default_agent_token_ceil")]
    pub agent_token_ceil: u32,
    /// Agent 流式沉默超时秒:任何 chunk(含 reasoning)都重置;真沉默超此值判超时(默认 90)。
    #[serde(default = "default_agent_silence_secs")]
    pub agent_silence_secs: u32,
    /// Agent 退化死循环上限:连续多少轮产出"近乎全等"才判高频重复、兜底收口(默认 2)。
    /// ★思考免死★:产出新内容(含 reasoning)永远不收口,只有真重复才算退化(用户原则 2026-06-03)。
    #[serde(default = "default_agent_max_stall")]
    pub agent_max_stall: u32,
    /// judge/distill(complete 路径)沉默超时秒:流卡住时多久收手降级,不卡死本回合(默认 60)。
    #[serde(default = "default_complete_silence_secs")]
    pub complete_silence_secs: u32,
    /// 思考强度(deepseek V4):"high" 或 "max"。默认 "max"(GrowBox 是 agent,官方建议 max)。数值全可设(推论9)。
    #[serde(default = "default_reasoning_effort")]
    pub reasoning_effort: String,
    /// idle 阈值秒:静默多久才算"真离开",开始后台睡眠/飞轮维护(默认 480=8 分钟)。数值全可设(推论9)。
    #[serde(default = "default_idle_threshold_secs")]
    pub idle_threshold_secs: u32,
    /// idle 巡检间隔秒:每隔多久看一眼是否进入 idle(默认 30)。
    #[serde(default = "default_idle_tick_secs")]
    pub idle_tick_secs: u32,
    /// 触发睡眠维护的疲劳阈值(0~1;低于此且无碎片债则不睡;默认 0.5)。
    #[serde(default = "default_idle_fatigue_threshold")]
    pub idle_fatigue_threshold: f32,
    /// 一次 idle 激活内睡眠步数上限(做梦/推演合计,防独占;默认 16)。
    #[serde(default = "default_idle_max_sleep_steps")]
    pub idle_max_sleep_steps: u32,
    /// 一次 idle 激活内推演次数上限(推演生新碎片留给做梦还,需有界;默认 4)。
    #[serde(default = "default_idle_max_rehearsals")]
    pub idle_max_rehearsals: u32,
    /// shell 批准弹窗等待超时秒:手动模式下等用户裁决多久,超时按拒绝(默认 300=5 分钟)。数值全可设(推论9)。
    #[serde(default = "default_shell_approval_timeout_secs")]
    pub shell_approval_timeout_secs: u32,
    /// UI 往返 ack 超时秒:活的 IDE 操作等前端回执多久,超时判未生效(默认 3)。
    #[serde(default = "default_ui_ack_timeout_secs")]
    pub ui_ack_timeout_secs: u32,
    /// 后台任务等待的退避基数毫秒:wait_tasks 指数退避起点(默认 2000)。
    #[serde(default = "default_task_backoff_base_ms")]
    pub task_backoff_base_ms: u32,
    /// 后台任务等待的退避上限毫秒:退避翻倍不超过此值(默认 60000)。
    #[serde(default = "default_task_backoff_cap_ms")]
    pub task_backoff_cap_ms: u32,
    /// file_read 单次读取字节上限(默认 204800=200KB)。数值全可设(推论9)。
    #[serde(default = "default_tool_max_read_bytes")]
    pub tool_max_read_bytes: u32,
    /// file_list 列出条目上限(默认 500)。
    #[serde(default = "default_tool_max_list_entries")]
    pub tool_max_list_entries: u32,
    /// shell 输出字节上限(默认 65536=64KB)。
    #[serde(default = "default_tool_max_output_bytes")]
    pub tool_max_output_bytes: u32,
    /// code_outline 大纲符号数上限(默认 400;超长文件防刷屏)。数值全可设(推论9)。
    #[serde(default = "default_tool_max_outline_symbols")]
    pub tool_max_outline_symbols: u32,
    /// 上下文窗口总量(token):面板"实时上下文压力"= 实发 prompt_tokens / 此值。随模型设(默认 256000=256k)。
    /// 数值全可设(推论9)。仅用于面板压力比的分母,不参与请求装配。
    #[serde(default = "default_context_window_tokens")]
    pub context_window_tokens: u32,
    /// shell 命令墙钟超时秒(默认 60;0 = 不限,慎用)。超时杀整进程组(含 `cmd &` 后台子进程)。
    /// 数值全可设(推论9)。防"命令永不返回致工具挂死 + 终止失效"(2026-06-09 真机 445s 挂死)。
    #[serde(default = "default_shell_timeout_secs")]
    pub shell_timeout_secs: u32,
    /// 后台任务输出尾巴保留字节(默认 4096)。
    #[serde(default = "default_task_output_cap")]
    pub task_output_cap: u32,
    /// 疲劳公式:缓存命中率低的权重(默认 0.4)。数值全可设(推论9)。
    #[serde(default = "default_fatigue_w_hitrate")]
    pub fatigue_w_hitrate: f32,
    /// 疲劳公式:淘汰频繁的权重(默认 0.2)。
    #[serde(default = "default_fatigue_w_evict")]
    pub fatigue_w_evict: f32,
    /// 疲劳公式:碎片占比大的权重(默认 0.4)。
    #[serde(default = "default_fatigue_w_fragment")]
    pub fatigue_w_fragment: f32,
    /// 瞬态:碎片台账容量(默认 512)。数值全可设(推论9)。
    #[serde(default = "default_transient_fragment_cap")]
    pub transient_fragment_ledger_cap: u32,
    /// 瞬态:二级索引容量(默认 128)。
    #[serde(default = "default_transient_secondary_cap")]
    pub transient_secondary_index_cap: u32,
    /// 瞬态:内部状态事件环容量(默认 32)。
    #[serde(default = "default_transient_internal_events_cap")]
    pub transient_internal_events_cap: u32,
    /// 瞬态:被造物(artifact)交互回传环容量(默认 64)。数值全可设(推论9)。
    #[serde(default = "default_transient_artifact_cap")]
    pub transient_artifact_interactions_cap: u32,
    /// 瞬态:sleep 复核反 K 的老化阈(天;默认 14)。连接时换算成毫秒。
    #[serde(default = "default_transient_neg_review_days")]
    pub transient_neg_review_max_age_days: u32,
    /// 瞬态:每次 sleep 复核反 K 至多扫的边数(默认 64)。
    #[serde(default = "default_transient_neg_review_edges")]
    pub transient_neg_review_max_edges: u32,
    /// 允许自动关机(自关机能力,见 `计划/自关机能力.md`)。默认 false:关机动作(关闭自己 / 系统关机)
    /// 每次都弹一次性授权;为 true 时用户已显式授予永久权 → 关机免弹窗、全自动。
    #[serde(default)]
    pub auto_shutdown_allowed: bool,
    /// 缓存预热(优化造物首次回复慢,见决策日志 2026-06-04)。默认 true:连接后预 prefill 系统提示词 +
    /// 工具定义吃满 deepseek KV 缓存,使首条造物回复也走缓存命中(不必等第二条才快)。可设开关。
    #[serde(default = "default_true")]
    pub cache_prewarm: bool,
    /// 分支日志上限(GB)。派生分支(栈函数 v2)的全部调用信息原样存项目级日志文件,环形覆盖;
    /// 到达此上限即轮替(旧的被覆盖)。默认 25;**-1 = 无限制**(慎用)。见 设计/07 v2 原则9。
    #[serde(default = "default_branch_log_max_gb")]
    pub branch_log_max_gb: f64,
    /// ★二期 C1 懒加载总开关★:开 = 工具懒加载(核心常驻 + 扩展工具 deferred 只露名,经 tool_search
    /// 按需拉回 schema 再调;tools 字段恒定 → 修工作流"按节点换工具"的 KV 缓存前缀破坏)。
    /// **默认 false = 今天的行为完全不变**(全工具直接可调、工作流照旧按节点收窄)。opt-in 真机评估后再定默认。
    #[serde(default)]
    pub lazy_tools: bool,
    /// ★二期 C1 deferred 工具名单(可设)★:`lazy_tools` 开时,这些工具不进 tools 字段、只露名。
    /// 默认 = 扩展/低频工具(常用 file_*/shell 留常驻 = 低延迟)。想更激进(更小前缀/更硬锁)就加更多;
    /// 想更直接就移出。tool_search/finish/ask_user/workflow_return 永不可 defer(始终常驻,注册表强制)。
    #[serde(default = "default_deferred_tools")]
    pub deferred_tools: Vec<String>,
    /// ★二期 D2 MCP server 连接配置(可设、持久)★:每条 = 一个 stdio MCP server(收编生态工具)。
    /// 应用时全量重连(断旧 + 连启用的);跨重启自动重连。MCP 工具结果按**外部不可信输入**处理
    /// (过一期安全门 + 来源标注,见 `05-MCP客户端与懒加载.md`)。默认空 = 不连任何 server、零影响。
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// ★主动自检(grounded verification)★:任务收尾前,AI 拿自己即将给出的工作汇报**重读相关文件/状态
    /// 逐条核对**(file_read/lsp/code_outline/code_search/shell),改正证据不支持的说法、标注无法验证的,
    /// 再正式收口。修"一次干多件事时过度声称/幻觉"。默认开;关掉省 token(见 self_verify_min_tools 阈值控成本)。
    #[serde(default = "default_true")]
    pub self_verify: bool,
    /// 自检触发阈值:本次任务工具调用数 ≥ 此值才触发自检(轻任务/纯问答不花这个钱)。默认 3,可设。
    #[serde(default = "default_self_verify_min_tools")]
    pub self_verify_min_tools: u32,
    /// ★提示词自转译(自我负责-输入侧,设计/08 推论2)★:用"消费该提示词的那个模型"把喂给模型的提示词
    /// (系统提示/工具说明/脚手架/judge·distill)按它自己的风格重写一遍(decoder 自亲和,模型最能执行自己写的话)。
    /// 默认关——开了才让运行时优先用转译覆盖层;覆盖产物由「重写提示词」动作生成、按(模型,语言,键)分桶持久,
    /// 原文永不丢。关 或 无覆盖 = 逐字用原文(零行为变更)。谁转译谁:主模型转主模型可见的,潜意识模型转自己那份
    /// (今天潜意识==主模型同一个,故同桶;将来拆独立潜意识模型时按 key 自动分桶,转译层不改)。见 计划/提示词自转译.md。
    #[serde(default)]
    pub prompt_transpile: bool,
    /// 提示词自转译并发数:「用当前模型重写」时同时在飞的 LLM 重写请求数(默认 4)。
    /// 大=墙钟更快但更吃配额/易撞限流;小=稳。每条转译彼此独立(冷请求),并发是主提速手段。
    #[serde(default = "default_transpile_concurrency")]
    pub transpile_concurrency: u32,
    /// ★Skill 系统总开关(设计/09)★:开 = 常驻清单拼进系统提示、可语义召回、load_skill 可用。
    /// 默认开。关 = 完全不暴露任何 skill(回到"无第四原语"的行为)。
    #[serde(default = "default_true")]
    pub skill_enabled: bool,
    /// Skill 常驻清单上限(内置优先 + 已学按新近补足到此数;被挤出者仍可语义召回/按名加载)。默认 24。
    #[serde(default = "default_skill_list_max")]
    pub skill_list_max: u32,
    /// Skill 自动加载阈值:语义召回相似度 ≥ 此值的 skill 直接注入正文(零 load_skill 调用、省 LLM
    /// 调用 + 加速);低于则只浮名。数值全可设。默认 0.88。
    #[serde(default = "default_skill_autoload_threshold")]
    pub skill_autoload_threshold: f32,
    /// 停用的 skill 名单(小写名):对内置种子与已学一视同仁;停用 = 不列清单/不召回/load 拒,
    /// 但不删数据、可随时重启(append-only 友好)。默认空。
    #[serde(default)]
    pub skill_disabled: Vec<String>,
    /// ★Web 工具(web_fetch/web_search)★ 搜索 provider:"" = 未配置(web_search 诚实失败并引导);
    /// "tavily" / "brave" / "searxng"。web_fetch 不依赖此项、开箱即用。
    #[serde(default)]
    pub web_search_provider: String,
    /// 搜索 provider 端点 Base URL:searxng 必填(自建实例,如 http://nas.lan:8888);
    /// tavily/brave 留空用官方端点(可覆盖成代理)。
    #[serde(default)]
    pub web_search_api_base: String,
    /// 搜索 provider API key(tavily/brave 必填;searxng 通常不需要)。
    #[serde(default)]
    pub web_search_api_key: String,
    /// web_search 默认返回条数(1~10)。数值全可设(推论9)。
    #[serde(default = "default_web_search_max_results")]
    pub web_search_max_results: u32,
    /// web_fetch/web_search 请求墙钟超时秒(默认 30;0 = 不限,慎用)。数值全可设(推论9)。
    #[serde(default = "default_web_timeout_secs")]
    pub web_timeout_secs: u32,
    /// ★工具记忆 + 不犯第二遍(计划/工具记忆-不犯第二遍)★ 总开关。开 = 分发前会诊「小本本」+
    /// note_tool_memory 记 + 本回合失败指纹守卫;关 = 全不做(行为同今天)。默认开。
    #[serde(default = "default_true")]
    pub tool_memory_enabled: bool,
    /// 工具记忆「不可行(infeasible)」硬否决相似度阈:当前情况与已记不可行情况余弦 ≥ 此值 →
    /// 反 K 一票否决重试。数值全可设(推论9)。
    /// ★默认 0.88(真机校准,0-OPUS37 续)★:本机默认嵌入器 = candle multilingual-e5-small,
    /// 其余弦"高地板"特性把所有短文本压在 0.83~0.93:无关同工具调用 ≤0.86、真相关/重复 0.884~0.934。
    /// 旧默认 0.85 会**误杀无关调用**(实测无关达 0.86);但 0.92 又太高 —— 真机实测一条"教科书级吻合"
    /// 的不可行记忆(seed「运行 docker 命令」vs 调用 `shell|{docker ps}`)相似度仅 **0.893**,0.92 会让它
    /// **永不触发**、整个否决形同虚设。0.88 落在无关(≤0.86)与相关(≥0.884)之间的窄缝:既挡重复又不误杀。
    /// 注:此缝很窄(e5-small 区分力弱),根治应让会诊比较 situation↔situation(都短文本)而非 call_sig↔整条
    /// 结构化全文(见 `计划/工具记忆-不犯第二遍.md`「会诊比较方式」待办)。数值全可设(推论9),用户可在高级 Tab 调。
    #[serde(default = "default_tool_memory_veto_threshold")]
    pub tool_memory_veto_threshold: f32,
    /// 工具记忆「失败(fails)」软提醒相似度阈:≥ 此值 → 执行前注入"曾在类似情况失败"的提醒(不阻断)。
    /// 数值全可设(推论9)。★默认 0.88(同上 e5 校准)★:旧默认 0.80 低于 e5 无关地板(~0.86)→
    /// 几乎每个同工具调用都软提醒(噪音);0.88 只在相当相似才提醒。
    #[serde(default = "default_tool_memory_warn_threshold")]
    pub tool_memory_warn_threshold: f32,
}

fn default_tool_memory_veto_threshold() -> f32 {
    0.88
}

fn default_tool_memory_warn_threshold() -> f32 {
    0.88
}

fn default_web_search_max_results() -> u32 {
    5
}

fn default_web_timeout_secs() -> u32 {
    30
}

fn default_skill_list_max() -> u32 {
    24
}

fn default_skill_autoload_threshold() -> f32 {
    0.88
}

fn default_transpile_concurrency() -> u32 {
    4
}

fn default_self_verify_min_tools() -> u32 {
    3
}

/// 一个 MCP server 的连接配置(`.mcp.json` 式;持久进 Settings,见二期 D2)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerConfig {
    /// server 名(其工具全名前缀 `<name>_<tool>`,须唯一)。
    pub name: String,
    /// 可执行命令(stdio 传输;如 "npx" / "uvx" / 绝对路径)。
    pub command: String,
    /// 启动参数。
    #[serde(default)]
    pub args: Vec<String>,
    /// 额外环境变量(放 token 等;JSON 对象形态)。
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// 是否启用(false = 留着配置但不连)。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 传输方式:`"stdio"`(默认,spawn `command`)或 `"http"`(MCP Streamable HTTP,连 `url`)。
    #[serde(default = "default_mcp_transport")]
    pub transport: String,
    /// HTTP 传输的 endpoint URL(transport="http" 时用;stdio 时忽略)。
    #[serde(default)]
    pub url: String,
}

fn default_mcp_transport() -> String {
    "stdio".to_string()
}

fn default_true() -> bool {
    true
}

fn default_deferred_tools() -> Vec<String> {
    // code_search/code_outline 是"准高频"(找代码/看结构),挪出 deferred 常驻,省首次 tool_search 往返
    // (用户 2026-06-08;lsp 仍 deferred=语义层重、按需起)。
    [
        "lsp",
        // 交互式终端共驾工具:低频(只 ssh/REPL/装配等需要),长尾 deferred、用时 tool_search 拉。
        "pty_send",
        "pty_peek",
        "pty_close",
        "pty_watch",
        "learn_process",
        "note_tool_memory",
        "create_project",
        "open_settings",
        "ui_control",
        "render_artifact",
        "push_artifact_notice",
        "artifact_command",
        "selftest_artifact",
        "shutdown",
        "spawn_task",
        "wait_tasks",
        "list_tasks",
        // Web 两件套:偶发用(查报错/读文档),长尾 deferred、露名可见、用时 tool_search 拉。
        "web_fetch",
        "web_search",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn default_branch_log_max_gb() -> f64 {
    25.0
}

fn default_pointer_match_mode() -> String {
    "weighted_cosine".to_string()
}
fn default_pointer_follow_threshold() -> f32 {
    0.80
}
fn default_pointer_neg_block_threshold() -> f32 {
    0.90
}
fn default_pointer_k_merge_threshold() -> f32 {
    0.93
}
fn default_pointer_weight_gain() -> f32 {
    0.30
}
fn default_pointer_k_cap() -> u32 {
    8
}
fn default_pointer_force_judge() -> bool {
    true
}
fn default_retrieval_rag_hit_threshold() -> f32 {
    0.85
}
fn default_retrieval_rag_topk() -> u32 {
    8
}
fn default_retrieval_entry_k() -> u32 {
    3
}
fn default_retrieval_entry_min_sim() -> f32 {
    0.30
}
fn default_retrieval_scan_batch() -> u32 {
    8
}
fn default_retrieval_scan_max() -> u32 {
    256
}
fn default_retrieval_project_boost() -> f32 {
    0.5
}
fn default_retrieval_chunk_min_chars() -> u32 {
    1500
}
fn default_agent_max_token_retries() -> u32 {
    2
}
fn default_agent_token_ceil() -> u32 {
    32_768
}
fn default_agent_silence_secs() -> u32 {
    90
}
fn default_agent_max_stall() -> u32 {
    2
}
fn default_complete_silence_secs() -> u32 {
    60
}
fn default_reasoning_effort() -> String {
    // high = deepseek 官方默认,画 UI/交互/大多数任务快且够用(用户 2026-06-04 拍板:max 让画棋盘
    // 思考 234s/5万字太慢,交互没法用)。需要极致深推理(算法/架构)的复杂任务用户再手动调 max。
    "high".to_string()
}
fn default_idle_threshold_secs() -> u32 {
    8 * 60
}
fn default_idle_tick_secs() -> u32 {
    30
}
fn default_idle_fatigue_threshold() -> f32 {
    0.5
}
fn default_idle_max_sleep_steps() -> u32 {
    16
}
fn default_idle_max_rehearsals() -> u32 {
    4
}
fn default_shell_approval_timeout_secs() -> u32 {
    300
}
fn default_ui_ack_timeout_secs() -> u32 {
    3
}
fn default_task_backoff_base_ms() -> u32 {
    2_000
}
fn default_task_backoff_cap_ms() -> u32 {
    60_000
}
fn default_tool_max_read_bytes() -> u32 {
    200 * 1024
}
fn default_tool_max_list_entries() -> u32 {
    500
}
fn default_tool_max_output_bytes() -> u32 {
    64 * 1024
}
fn default_tool_max_outline_symbols() -> u32 {
    400
}
fn default_context_window_tokens() -> u32 {
    256_000
}
fn default_shell_timeout_secs() -> u32 {
    60
}
fn default_task_output_cap() -> u32 {
    4096
}
fn default_fatigue_w_hitrate() -> f32 {
    0.4
}
fn default_fatigue_w_evict() -> f32 {
    0.2
}
fn default_fatigue_w_fragment() -> f32 {
    0.4
}
fn default_transient_fragment_cap() -> u32 {
    512
}
fn default_transient_secondary_cap() -> u32 {
    128
}
fn default_transient_internal_events_cap() -> u32 {
    32
}
fn default_transient_artifact_cap() -> u32 {
    64
}
fn default_transient_neg_review_days() -> u32 {
    14
}
fn default_transient_neg_review_edges() -> u32 {
    64
}

fn default_max_tokens() -> u32 {
    0 // 0 = 不限制,让模型自己决定何时停止
}
fn default_max_turns() -> u32 {
    1000 // 长任务够用;0 = 无限
}
fn default_lang() -> String {
    "zh".to_string()
}
fn default_cache_cap() -> u32 {
    256
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            api_base: "https://api.deepseek.com".to_string(),
            api_key: String::new(),
            model: "deepseek-v4-flash".to_string(),
            max_tokens: default_max_tokens(),
            max_turns: default_max_turns(),
            lang: default_lang(),
            subconscious_model: String::new(),
            subconscious_api_base: String::new(),
            subconscious_api_key: String::new(),
            embed_remote: false,
            embed_api_base: String::new(),
            embed_api_key: String::new(),
            embed_model: String::new(),
            working_context_chars: 0, // 0 = 代码默认
            recent_ring_chars: 0,     // 0 = 代码默认
            neighbor_cache_cap: default_cache_cap(),
            truncate_tool_display: false, // 默认完整显示,不省略
            auto_mode: false,             // 默认手动:shell 逐条批准
            danger_mode: false,           // 默认关:danger(为所欲为)必须显式开启,且不持久
            privacy_dirs: Vec::new(),     // 默认无额外隐私文件夹
            pointer_match_mode: default_pointer_match_mode(),
            pointer_follow_threshold: default_pointer_follow_threshold(),
            pointer_neg_block_threshold: default_pointer_neg_block_threshold(),
            pointer_k_merge_threshold: default_pointer_k_merge_threshold(),
            pointer_weight_gain: default_pointer_weight_gain(),
            pointer_k_cap: default_pointer_k_cap(),
            pointer_force_judge: default_pointer_force_judge(),
            retrieval_rag_hit_threshold: default_retrieval_rag_hit_threshold(),
            retrieval_rag_topk: default_retrieval_rag_topk(),
            retrieval_entry_k: default_retrieval_entry_k(),
            retrieval_entry_min_sim: default_retrieval_entry_min_sim(),
            retrieval_scan_batch: default_retrieval_scan_batch(),
            retrieval_scan_max: default_retrieval_scan_max(),
            retrieval_project_boost: default_retrieval_project_boost(),
            retrieval_chunk_min_chars: default_retrieval_chunk_min_chars(),
            agent_max_token_retries: default_agent_max_token_retries(),
            agent_token_ceil: default_agent_token_ceil(),
            agent_silence_secs: default_agent_silence_secs(),
            agent_max_stall: default_agent_max_stall(),
            complete_silence_secs: default_complete_silence_secs(),
            reasoning_effort: default_reasoning_effort(),
            idle_threshold_secs: default_idle_threshold_secs(),
            idle_tick_secs: default_idle_tick_secs(),
            idle_fatigue_threshold: default_idle_fatigue_threshold(),
            idle_max_sleep_steps: default_idle_max_sleep_steps(),
            idle_max_rehearsals: default_idle_max_rehearsals(),
            shell_approval_timeout_secs: default_shell_approval_timeout_secs(),
            ui_ack_timeout_secs: default_ui_ack_timeout_secs(),
            task_backoff_base_ms: default_task_backoff_base_ms(),
            task_backoff_cap_ms: default_task_backoff_cap_ms(),
            tool_max_outline_symbols: default_tool_max_outline_symbols(),
            context_window_tokens: default_context_window_tokens(),
            shell_timeout_secs: default_shell_timeout_secs(),
            tool_max_read_bytes: default_tool_max_read_bytes(),
            tool_max_list_entries: default_tool_max_list_entries(),
            tool_max_output_bytes: default_tool_max_output_bytes(),
            task_output_cap: default_task_output_cap(),
            fatigue_w_hitrate: default_fatigue_w_hitrate(),
            fatigue_w_evict: default_fatigue_w_evict(),
            fatigue_w_fragment: default_fatigue_w_fragment(),
            transient_fragment_ledger_cap: default_transient_fragment_cap(),
            transient_secondary_index_cap: default_transient_secondary_cap(),
            transient_internal_events_cap: default_transient_internal_events_cap(),
            transient_artifact_interactions_cap: default_transient_artifact_cap(),
            transient_neg_review_max_age_days: default_transient_neg_review_days(),
            transient_neg_review_max_edges: default_transient_neg_review_edges(),
            auto_shutdown_allowed: false,
            cache_prewarm: true,
            branch_log_max_gb: default_branch_log_max_gb(),
            // 默认开懒加载:核心编码工具(读/改/shell/finish/tool_search 等 NEVER_DEFER + 内置)常驻,
            // 长尾(lsp/code_search/artifact/tasks…)deferred、用时 tool_search 按需加载。依据 = 本仓 C1 实验
            // (`二期项目/实验记录-C1懒加载与缓存.md`:tools 在 KV 缓存前缀最前,全量塞→换工具整条缓存全毁;
            // 懒开保 99% 缓存、多轮快 ~20%)+ 成熟 Agent 产品默认即"核心常驻 + 长尾延迟"。基本编码循环零 tool_search。
            lazy_tools: true,
            deferred_tools: default_deferred_tools(),
            mcp_servers: Vec::new(),
            self_verify: true,
            self_verify_min_tools: default_self_verify_min_tools(),
            prompt_transpile: false,
            transpile_concurrency: default_transpile_concurrency(),
            skill_enabled: true,
            skill_list_max: default_skill_list_max(),
            skill_autoload_threshold: default_skill_autoload_threshold(),
            skill_disabled: Vec::new(),
            web_search_provider: String::new(),
            web_search_api_base: String::new(),
            web_search_api_key: String::new(),
            web_search_max_results: default_web_search_max_results(),
            web_timeout_secs: default_web_timeout_secs(),
            tool_memory_enabled: true,
            tool_memory_veto_threshold: default_tool_memory_veto_threshold(),
            tool_memory_warn_threshold: default_tool_memory_warn_threshold(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_has_reasoning_budget() {
        // 实测:flash 是推理模型,默认 token 必须够大(见 实验记录/00)。
        let s = Settings::default();
        assert!(s.max_tokens == 0 || s.max_tokens >= 4096); // 0=不限制
        assert_eq!(s.model, "deepseek-v4-flash");
    }

    #[test]
    fn settings_roundtrip_json() {
        // config 结构体必须能反序列化(Deserialize 的真实用途)。
        let s = Settings::default();
        let j = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&j).unwrap();
        assert_eq!(back.model, s.model);
    }

    #[test]
    fn artifact_interactions_cap_defaults_and_survives_old_config() {
        // 默认 64(数值全可设·推论9):新旋钮有合理默认。
        assert_eq!(Settings::default().transient_artifact_interactions_cap, 64);
        // 向后兼容:旧版落库的 settings 没有该字段,反序列化应回退默认(#[serde(default)])。
        let mut v = serde_json::to_value(Settings::default()).unwrap();
        v.as_object_mut().unwrap().remove("transient_artifact_interactions_cap");
        let back: Settings = serde_json::from_value(v).unwrap();
        assert_eq!(back.transient_artifact_interactions_cap, 64);
    }

    #[test]
    fn project_starts_with_no_roots() {
        let p = ProjectConfig::new("p1", "个人博客");
        assert!(p.writable_roots.is_empty());
        assert_eq!(p.name, "个人博客");
    }
}
