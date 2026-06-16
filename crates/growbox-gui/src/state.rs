//! 应用状态 —— 把各 crate 的能力组装成一个可共享的运行时。
//!
//! 不依赖 Tauri(Tauri 层把它放进 managed state、用 Mutex 包起来)。
//! 设置与项目落盘到 data_dir;记忆 v1 暂留内存(持久化是后续飞轮项,见交接报告)。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use growbox_core::{ProjectConfig, Settings};
use growbox_learn::{Flywheel, Scheduler};
use growbox_llm::{Embedder, LlmClient, RemoteEmbedder};
use growbox_memory::{Memory, Store};
use growbox_safety::Sandbox;

use crate::bridge::{LlmBridge, LlmDriver};
use crate::health::{Health, Severity};
use crate::tasks::TaskManager;

/// 全局运行时状态。
pub struct AppState {
    pub settings: Settings,
    pub registry: crate::registry::Registry,
    pub memory: Memory,
    pub sandbox: Sandbox,
    pub flywheel: Flywheel,
    pub scheduler: Scheduler,
    pub projects: Vec<ProjectConfig>,
    pub current: Option<String>,
    pub work_dir: PathBuf,
    /// 主模型驱动(连接后就绪)。
    pub llm: Option<Arc<dyn LlmDriver>>,
    /// 潜意识桥(检索判断 + 飞轮压缩;连接后就绪)。
    pub bridge: Option<Arc<LlmBridge>>,
    pub connected: bool,
    pub session_id: Option<String>,
    pub data_dir: PathBuf,
    /// Tauri 资源目录(带模型包的 e5 权重预置处;connect 时由 cmds 注入)。
    pub resource_dir: Option<PathBuf>,
    /// 单文件持久化(settings/projects/记忆)。`None` 仅在 DB 打不开时降级为纯内存。
    pub store: Option<Store>,
    /// 系统提示词(从资源文件加载,连接时设置)。
    pub base_system_prompt: String,
    /// 后台任务管理器(纯内存,Arc 共享给 Supervisor)。
    pub task_mgr: std::sync::Arc<TaskManager>,
    /// 常驻 Supervisor(connect 时启动,断开时取消)。
    pub supervisor: Option<crate::supervisor::SupervisorHandle>,
    /// 前台最近活动时间戳(epoch millis)。IdleWorker 无锁读它判断是否进入 idle(前台优先)。
    /// 前台回合起止各 touch 一次;独立 Arc,使后台不必抢 AppState 锁就能判 idle。
    pub last_activity: Arc<AtomicI64>,
    /// 常驻 IdleWorker(connect 时启动,断开时取消)。idle 时把经验压成知识,见 `idle.rs`。
    pub idle_worker: Option<crate::idle::IdleWorkerHandle>,
    /// 健康/异常告知状态(严重异常不静默,见 `异常告知.md`)。
    pub health: Health,
    /// 潜意识 LLM 仲裁器(P5)——共用一个潜意识 LLM 的优先级调度槽(Agent > Sleep > 飞轮)。
    /// 前台回合 / 睡眠 worker / 飞轮 idle 压缩在调潜意识 LLM 前各 acquire 一档,见 `arbiter.rs`。
    pub arbiter: Arc<crate::arbiter::Arbiter>,
    /// 已让 AI 感知过的持久化写失败累计数(去重:只在 write_fault 计数增长时 perceive 一次)。
    pub perceived_write_faults: u64,
    /// 活的 IDE 面板目录(前端 `register_ui_surfaces` 声明,`ui_control` 持同一克隆读它)。
    /// 单一真相在前端;后端只持运行时副本(见 `ui.rs`)。
    pub ui_catalog: crate::ui::UiSurfaceCatalog,
    /// 活的 IDE 面板可见态缓存(前端 `ui_state_changed` 上报,含用户手动开关)。
    /// `get_control_state` 据此上浮真实可见态;真实变化时 perceive 让 agent 看见(感知无盲区)。
    pub ui_panel_state: std::collections::HashMap<String, bool>,
    /// ★Skill 提议(设计/09 S3)★:idle 飞轮起草的待裁决 skill 提议 + 已拒名单。落 redb kv,非记忆节点。
    pub skill_proposals: crate::skill_proposals::SkillProposalStore,
}

impl AppState {
    /// 从资源目录加载系统提示词文件。
    pub fn load_prompt(resource_dir: &Path, name: &str) -> String {
        let path = resource_dir.join("prompts").join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|_| String::from("你是 GrowBox,一个会自我进化的 AI 编程助手。用工具完成任务,用中文回复。"))
    }

    /// 按提示词语言加载 agent 系统提示词(`system.zh.md` / `system.en.md`)。
    /// 系统提示词受提示词语言(zh/en)控制、不受界面语言控制(用户决策:除系统提示词外一切归 UI 多语言)。
    /// 兜底链:目标语言文件 → 旧单份 `system.md` → `load_prompt` 的内置默认串。
    pub fn load_agent_prompt(resource_dir: &Path, lang: &str) -> String {
        let file = if crate::tool_i18n::normalize_prompt_lang(lang) == "en" {
            "agent/system.en.md"
        } else {
            "agent/system.zh.md"
        };
        let path = resource_dir.join("prompts").join(file);
        if let Ok(s) = std::fs::read_to_string(&path) {
            if !s.trim().is_empty() {
                return s;
            }
        }
        Self::load_prompt(resource_dir, "agent/system.md")
    }

    /// 构建当前项目上下文(注入到系统提示词后面,给 LLM 自我感知)。
    pub fn project_context(&self) -> String {
        let proj_name = self
            .current_project()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "无".into());
        let writable: Vec<String> = self
            .current_project()
            .map(|p| p.writable_roots.iter().map(|p| p.display().to_string()).collect())
            .unwrap_or_default();
        let readonly: Vec<String> = self
            .current_project()
            .map(|p| p.readonly_roots.iter().map(|p| p.display().to_string()).collect())
            .unwrap_or_default();

        let w = if writable.is_empty() { "无".into() } else { writable.join(", ") };
        let r = if readonly.is_empty() { "无".into() } else { readonly.join(", ") };

        format!(
            "=== 当前项目状态(自我感知) ===\n\
             项目名称: {proj_name}\n\
             工作目录: {work_dir}\n\
             可写目录: {w}\n\
             只读目录: {r}\n\
             记忆条数: {n}\n\
             以上是你的完整执行环境。用户说\"新建xx\"时,若已有活跃项目则直接在当前目录动手;\
             若用户明确指定不同目录才弹 create_project。",
            proj_name = proj_name,
            work_dir = self.work_dir.display(),
            w = w,
            r = r,
            n = self.memory.conclusions().iter().filter(|c| c.is_active()).count(),
        )
    }

    /// 从 data_dir 打开单文件数据库,载入设置/项目/记忆(没有则用默认)。
    pub fn new(data_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&data_dir);
        // 持久化打不开 = 严重异常,不静默(见 `异常告知.md`):降级为纯内存运行但记下 Fatal,
        // 由 UI 红色告警(记忆仅在内存,退出即丢)。
        let (store, store_err) = match Store::open(data_dir.join("growbox.redb")) {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e.to_string())),
        };
        if let Some(s) = &store {
            migrate_legacy_json(s, &data_dir); // 旧 JSON 一次性迁入 DB
        }
        let memory = match &store {
            Some(s) => Memory::open(s.clone(), &data_dir), // 载入已存的时间线/结论(向量索引 LMDB 落 data_dir),后续 write-through
            None => Memory::new(),
        };
        // 后台任务执行器(spawn/wait/list)要和这个 task_mgr 共享状态,故先建 task_mgr 再建注册表。
        let task_mgr = TaskManager::new();
        // 活的 IDE 面板目录:此处建空目录,ui_control 与 AppState 共享同一 Arc;
        // 前端 mount 时 register_ui_surfaces 填充(单一真相在前端)。
        let ui_catalog = crate::ui::empty_catalog();
        let mut health = Health::new();
        let mut memory = memory;
        if let Some(e) = store_err {
            // code 对齐 catalog(store.unavailable,surface=health):显示文案由前端按 ui_lang 渲染(四国化)。
            health.set(
                "store.unavailable",
                Severity::Fatal,
                serde_json::json!({ "detail": e.to_string() }),
            );
            // 失败 AI 也必须能感知(不止用户的红警灯):按 code 渲染 llm 落内部状态 + 时间线。
            // 构造期 settings 尚未 reload,prompt_lang 用默认 zh(store 打不开属罕见边界;health 显示侧 Phase 3 并入 catalog)。
            crate::notify::perceive_notice(
                &mut memory,
                "zh",
                "store.unavailable",
                &serde_json::json!({ "detail": e.to_string() }),
            );
        }
        // 内置种子 skill 物化成节点(设计/09 可加点):让其也享语义召回 + 高置信自动注入,而不仅是
        // 常驻清单一行。幂等(已存在同名则跳过);嵌入由 idle 统一补。在载入历史之后、用前置 None project
        // tag(种子是全局 skill)写入。
        crate::skills::ensure_seed_nodes(&mut memory);
        // Skill 提议存储(S3):从 kv 载回上次未消化的提议 + 已拒名单(无则空)。
        let skill_proposals = store
            .as_ref()
            .and_then(|s| s.kv_get::<crate::skill_proposals::SkillProposalStore>("skill_proposals"))
            .unwrap_or_default();
        let mut st = AppState {
            settings: Settings::default(),
            registry: crate::registry::Registry::with_builtins_catalog(task_mgr.clone(), ui_catalog.clone()),
            memory,
            sandbox: Sandbox::new(vec![], vec![]),
            flywheel: Flywheel::new(),
            scheduler: Scheduler::with_seeds(),
            projects: Vec::new(),
            current: None,
            work_dir: data_dir.clone(),
            llm: None,
            bridge: None,
            connected: false,
            session_id: None,
            store,
            data_dir: data_dir.clone(),
            resource_dir: None,
            base_system_prompt: String::new(),
            task_mgr,
            supervisor: None,
            last_activity: Arc::new(AtomicI64::new(growbox_core::now().timestamp_millis())),
            idle_worker: None,
            health,
            arbiter: Arc::new(crate::arbiter::Arbiter::new()),
            perceived_write_faults: 0,
            ui_catalog,
            ui_panel_state: std::collections::HashMap::new(),
            skill_proposals,
        };
        st.reload_from_disk();
        st
    }

    /// 持久化 Skill 提议存储(采纳/丢弃/新增后调)。
    pub(crate) fn persist_skill_proposals(&self) {
        if let Some(s) = &self.store {
            s.kv_put("skill_proposals", &self.skill_proposals);
        }
    }

    /// ★S3★ 尝试入队一条 idle 起草的 skill 提议。三道防膨胀去重:已存在同名 skill(内置种子/已学)、
    /// 队列已有同名、已被拒、队列已满 → 拒绝(返回 false,不入队)。入队成功落库并返回 true。
    pub fn try_add_skill_proposal(&mut self, name: &str, trigger: &str, body: &str, rationale: &str) -> bool {
        let name = name.trim();
        if name.is_empty() || body.trim().is_empty() {
            return false;
        }
        if !self.skill_proposals.has_room()
            || self.skill_proposals.has_pending(name)
            || self.skill_proposals.is_rejected(name)
        {
            return false;
        }
        // 已是 skill(内置种子已物化成节点,learned_skill_body 覆盖之;seed_body 双保险)→ 不重复提议。
        if self.memory.learned_skill_body(name).is_some() || crate::skills::seed_body(name).is_some() {
            return false;
        }
        let now = growbox_core::now().timestamp_millis();
        self.skill_proposals.push(crate::skill_proposals::SkillProposal {
            id: format!("sp-{now}-{name}"),
            name: name.to_string(),
            trigger: trigger.trim().to_string(),
            body: body.trim().to_string(),
            rationale: rationale.trim().to_string(),
            created_ms: now,
        });
        self.persist_skill_proposals();
        true
    }

    /// 记录某面板可见态(前端 `ui_state_changed` 上报,含用户手动开关)。
    /// 返回是否相对**已知旧值**发生真实翻转;真实翻转时 `perceive` 让 agent 看见(感知无盲区)。
    /// 首次上报(无旧值=启动同步)只填缓存、不算翻转、不 perceive(去噪)。
    pub fn note_ui_panel(&mut self, panel_id: &str, open: bool) -> bool {
        let prev = self.ui_panel_state.insert(panel_id.to_string(), open);
        let flipped = matches!(prev, Some(p) if p != open);
        if flipped {
            let code = if open { "ui.panel_opened" } else { "ui.panel_closed" };
            crate::notify::perceive_notice(
                &mut self.memory,
                &self.settings.lang,
                code,
                &serde_json::json!({ "panel": panel_id }),
            );
        }
        flipped
    }

    /// 标记前台活动(回合起止各调一次),供 IdleWorker 判断是否进入 idle。
    /// 只写一个原子时间戳,不持任何业务锁——后台无锁读它实现"前台优先"。
    pub fn touch_activity(&self) {
        self.last_activity
            .store(growbox_core::now().timestamp_millis(), Ordering::Relaxed);
    }

    /// 重载设置与项目(从单文件数据库,不受 runtime_dir 影响)。
    pub fn reload_from_disk(&mut self) {
        self.settings = self.store.as_ref().and_then(|s| s.kv_get("settings")).unwrap_or_default();
        let projs: Vec<ProjectConfig> =
            self.store.as_ref().and_then(|s| s.kv_get("projects")).unwrap_or_default();
        self.projects = dedup_projects(projs);
        self.current = None;
        if let Some(first) = self.projects.first().map(|p| p.id.clone()) {
            self.switch_project(&first);
        } else {
            // 无项目时，data_dir 本身作为默认可写根，确保记忆/设置落盘不受阻
            self.work_dir = self.data_dir.clone();
            self.sandbox = Sandbox::new(vec![self.data_dir.clone()], vec![]);
        }
    }

    /// 设置运行时工作目录(不影响持久化 data_dir)。用于 set_runtime_dir / connect。
    pub fn set_runtime(&mut self, runtime_dir: PathBuf) {
        let _ = std::fs::create_dir_all(&runtime_dir);
        // 尝试从 runtime_dir 加载已有项目(用户可能之前就在这个目录工作)
        let projs: Vec<ProjectConfig> = dedup_projects(load_json(&runtime_dir.join("projects.json")).unwrap_or_default());
        if !projs.is_empty() {
            self.projects = projs;
            self.current = None;
            if let Some(first) = self.projects.first().map(|p| p.id.clone()) {
                self.switch_project(&first);
            }
            self.save_projects(); // 同步到持久化 data_dir
        } else if self.projects.is_empty() {
            self.work_dir = runtime_dir.clone();
            self.sandbox = Sandbox::new(vec![runtime_dir], vec![]);
        }
    }

    /// 连接 LLM:用设置建主模型驱动 + 潜意识桥(含独立的嵌入槽位)。
    pub fn connect(&mut self, settings: Settings) -> String {
        self.settings = settings;
        let driver: Arc<dyn LlmDriver> =
            Arc::new(LlmClient::new(self.settings.api_base.clone(), self.settings.api_key.clone()));
        self.llm = Some(driver.clone());
        let embedder = build_embedder(&self.settings, &self.data_dir, self.resource_dir.as_deref());
        // ★独立潜意识模型槽★:subconscious_model 留空 = 复用主模型(同一 driver+model,今天的行为);
        // 填了则潜意识(judge/distill + 自转译 Subconscious 角色)走它自己的端点/key/模型,
        // base/key 留空回退主模型的。其余(agent 主循环)始终走主 driver。
        let sub_model_set = !self.settings.subconscious_model.trim().is_empty();
        let sub_driver: Arc<dyn LlmDriver> = if sub_model_set {
            let base = if self.settings.subconscious_api_base.trim().is_empty() {
                self.settings.api_base.clone()
            } else {
                self.settings.subconscious_api_base.clone()
            };
            let key = if self.settings.subconscious_api_key.trim().is_empty() {
                self.settings.api_key.clone()
            } else {
                self.settings.subconscious_api_key.clone()
            };
            Arc::new(LlmClient::new(base, key))
        } else {
            driver.clone()
        };
        let sub_model = if sub_model_set {
            self.settings.subconscious_model.clone()
        } else {
            self.settings.model.clone()
        };
        self.bridge = Some(Arc::new(
            LlmBridge::new(
                sub_driver,
                sub_model.clone(),
                self.settings.max_tokens,
                embedder,
                self.settings.complete_silence_secs as u64,
            )
            .with_prompt_lang(&self.settings.lang),
        ));
        self.connected = true;
        // ★提示词自转译(自我负责-输入侧)★:把开关 + 两个模型 id + 覆盖表推入全局取用层。
        // 今天潜意识 == 主模型(同一个 model),故 sub_model 传 == main;覆盖表 = 版本库当前激活版
        //(「历史提示词」可后悔;default/空 = 原文,零影响)。
        let overrides = self
            .store
            .as_ref()
            .map(crate::transpile_store::active_overrides)
            .unwrap_or_default();
        // sub_model = 独立潜意识模型(留空时 == 主模型);转译「谁转译谁」按此分桶。
        // 启用 = 当前激活版有覆盖(选区=默认/原文 → 空 → 关);不再用独立开关(UI 改成"当前提示词"选区)。
        let transpile_enabled = !overrides.is_empty();
        crate::transpile::configure(
            transpile_enabled,
            &self.settings.model,
            &sub_model,
            overrides,
        );
        // P4d:上下文组装层预算随用户设置(0 = 代码默认,随模型可调)。
        self.memory.configure_context(
            self.settings.working_context_chars as usize,
            self.settings.recent_ring_chars as usize,
        );
        // 邻域缓存容量随用户设置(0 = 代码默认 256);数值参数全可设(推论9)。
        self.memory
            .set_cache_capacity(self.settings.neighbor_cache_cap as usize);
        // 学习型指针旋钮随用户设置(匹配档 + 4 阈值);推论9 数值全可设。
        self.memory.set_pointer_config(growbox_memory::PointerConfig {
            match_mode: growbox_memory::PointerMatchMode::from_setting(&self.settings.pointer_match_mode),
            follow_threshold: self.settings.pointer_follow_threshold,
            neg_block_threshold: self.settings.pointer_neg_block_threshold,
            k_merge_threshold: self.settings.pointer_k_merge_threshold,
            weight_gain: self.settings.pointer_weight_gain,
            k_cap: self.settings.pointer_k_cap as usize,
            force_judge_on_cosine_hit: self.settings.pointer_force_judge,
        });
        // 检索行为旋钮随用户设置(RAG 命中阈/候选数 + 精确层入口/批量);推论9 数值全可设。
        self.memory.set_retrieval_config(growbox_memory::RetrievalConfig {
            rag_hit_threshold: self.settings.retrieval_rag_hit_threshold,
            rag_topk: self.settings.retrieval_rag_topk as usize,
            entry_k: self.settings.retrieval_entry_k as usize,
            entry_min_sim: self.settings.retrieval_entry_min_sim,
            scan_batch: self.settings.retrieval_scan_batch as usize,
            scan_max: self.settings.retrieval_scan_max as usize,
            project_boost: self.settings.retrieval_project_boost,
        });
        // 疲劳公式权重旋钮随用户设置(推论9 数值全可设)。
        self.memory.set_fatigue_config(growbox_memory::FatigueConfig {
            w_hitrate: self.settings.fatigue_w_hitrate as f64,
            w_evict: self.settings.fatigue_w_evict as f64,
            w_fragment: self.settings.fatigue_w_fragment as f64,
        });
        // 瞬态容量旋钮随用户设置(碎片/二级/内部环 cap + 反K复核;天换算成毫秒;推论9 数值全可设)。
        self.memory.set_transient_caps(growbox_memory::TransientCapsConfig {
            fragment_ledger_cap: self.settings.transient_fragment_ledger_cap as usize,
            secondary_index_cap: self.settings.transient_secondary_index_cap as usize,
            internal_events_cap: self.settings.transient_internal_events_cap as usize,
            artifact_interactions_cap: self.settings.transient_artifact_interactions_cap as usize,
            neg_review_max_age_ms: self.settings.transient_neg_review_max_age_days as i64 * 86_400_000,
            neg_review_max_edges: self.settings.transient_neg_review_max_edges as usize,
        });
        // Skill 系统旋钮随用户设置(总开关 + 清单上限 + 停用名单;设计/09,数值/开关全可设)。
        self.memory.set_skill_config(growbox_memory::SkillConfig {
            enabled: self.settings.skill_enabled,
            list_max: self.settings.skill_list_max as usize,
            autoload_threshold: self.settings.skill_autoload_threshold,
            disabled: self.settings.skill_disabled.iter().map(|s| s.to_ascii_lowercase()).collect(),
        });
        // 后台任务等待退避旋钮随用户设置(推论9 数值全可设)。
        self.task_mgr.set_backoff(
            self.settings.task_backoff_base_ms as u64,
            self.settings.task_backoff_cap_ms as u64,
        );
        self.task_mgr.set_output_cap(self.settings.task_output_cap as usize);
        // 工具输出上限旋钮随用户设置(经 Registry → ExecCtx 注入执行器;推论9 数值全可设)。
        self.registry.set_limits(growbox_core::ToolLimits {
            max_read_bytes: self.settings.tool_max_read_bytes as usize,
            max_list_entries: self.settings.tool_max_list_entries as usize,
            max_output_bytes: self.settings.tool_max_output_bytes as usize,
            max_outline_symbols: self.settings.tool_max_outline_symbols as usize,
            shell_timeout_secs: self.settings.shell_timeout_secs as u64,
        });
        // Web 工具配置随用户设置(搜索 provider/端点/key + 条数/超时;推论9 数值全可设)。
        self.registry.set_web_config(self.web_config_from_settings());
        // ★二期 C1★:懒加载总开关 + deferred 名单随用户设置(关=旧行为;开=核心常驻+tool_search 按需加载)。
        self.registry.set_lazy_tools(self.settings.lazy_tools, self.settings.deferred_tools.clone());
        let sid = format!("sess-{}", growbox_core::now().timestamp_millis());
        self.session_id = Some(sid.clone());
        self.save_settings();
        sid
    }

    /// 新建项目并切换过去。若提供非空 id 则用,否则自动生成。同 ID 已存在则更新。
    pub fn create_project(
        &mut self,
        id: Option<&str>,
        name: impl Into<String>,
        writable: Vec<PathBuf>,
        readonly: Vec<PathBuf>,
    ) -> String {
        let name = name.into();
        let mut id = match id.filter(|s| !s.is_empty()) {
            Some(s) => s.to_string(),
            None => format!("proj-{}", growbox_core::now().timestamp_millis()),
        };
        // ID 已存在 → 自动加序号重命名,不丢数据
        if self.projects.iter().any(|p| p.id == id) {
            let mut n = 2;
            loop {
                let candidate = format!("{id}-{n}");
                if !self.projects.iter().any(|p| p.id == candidate) {
                    id = candidate;
                    break;
                }
                n += 1;
            }
        }
        let mut cfg = ProjectConfig::new(id.clone(), name);
        cfg.writable_roots = writable;
        cfg.readonly_roots = readonly;
        // ★新项目可写目录递归创建★(真机暴露:不建则 work_dir 磁盘上不存在 → shell 等以它为 cwd 的
        // 进程 spawn 直接 "No such file or directory" → 模型被迫绕路)。只建可写根(只读根应是已有内容)。
        for root in &cfg.writable_roots {
            if let Err(e) = std::fs::create_dir_all(root) {
                eprintln!("[create_project] 递归创建可写目录失败 {}: {e}", root.display());
            }
        }
        self.projects.push(cfg);
        self.save_projects();
        self.switch_project(&id);
        id
    }

    /// 切换当前项目:重建沙箱与工作目录。
    pub fn switch_project(&mut self, id: &str) -> bool {
        let Some(cfg) = self.projects.iter().find(|p| p.id == id).cloned() else {
            return false;
        };
        self.work_dir = cfg
            .writable_roots
            .first()
            .or_else(|| cfg.readonly_roots.first())
            .cloned()
            .unwrap_or_else(|| self.data_dir.clone());
        // 防御:确保 work_dir 在磁盘上存在(新建/切换/启动恢复都经此)——任何以它为 cwd 的执行器
        // (shell/lsp/task)才不会因目录缺失而起不来。idempotent,已存在即无操作。
        let _ = std::fs::create_dir_all(&self.work_dir);
        // 造物文件夹(`<project>/.growbox/`)恒纳入可写 —— 即便项目只有只读根,造物也能存自己的状态/记忆,
        // 免每次弹授权(见 artifact_fs + 计划/造物交互-v2 §6)。
        let mut writable = cfg.writable_roots.clone();
        writable.push(crate::artifact_fs::growbox_root(&self.work_dir));
        let mut sb = Sandbox::new(writable, cfg.readonly_roots.clone());
        // 项目级已记授权重新装载(只升不降,见 设计/03)。
        for g in &cfg.grants {
            sb.grant(growbox_safety::GrantScope::ThisProjectPath(PathBuf::from(g)));
        }
        // 网络主机授权同样随项目装载(与路径授权各自独立)。
        for h in &cfg.net_grants {
            sb.grant(growbox_safety::GrantScope::ThisProjectHost(h.clone()));
        }
        self.sandbox = sb;
        self.current = Some(id.to_string());
        // 软隔离 tag:之后 ingest 的节点盖当前项目;检索对本项目命中加权、显示历史按项目过滤。
        // 记忆仍是一整块(不分库),跨项目高相关仍可被召回。
        self.memory.set_current_project(Some(id.to_string()));
        // P3 工作流持久化:注入新项目的持久目标(store/work_dir/project_id)并重载——
        // 清掉旧项目/造物工作流,从 redb 载本项目工作流 + 扫本项目造物文件夹载造物工作流(跨重启复用)。
        self.registry.set_workflow_context(self.store.clone(), self.work_dir.clone(), Some(id.to_string()));
        self.registry.reload_workflows();
        true
    }

    pub fn current_project(&self) -> Option<&ProjectConfig> {
        let id = self.current.as_ref()?;
        self.projects.iter().find(|p| &p.id == id)
    }

    pub(crate) fn save_settings(&self) {
        if let Some(s) = &self.store {
            s.kv_put("settings", &self.settings);
        }
    }
    pub fn save_projects(&self) {
        if let Some(s) = &self.store {
            s.kv_put("projects", &self.projects);
        }
    }

    /// Settings → Web 工具配置(连接时注入 + 设置面板改动时热更)。
    pub(crate) fn web_config_from_settings(&self) -> crate::executors::WebConfig {
        crate::executors::WebConfig {
            provider: self.settings.web_search_provider.trim().to_string(),
            api_base: self.settings.web_search_api_base.trim().to_string(),
            api_key: self.settings.web_search_api_key.trim().to_string(),
            max_results: self.settings.web_search_max_results.clamp(1, 10),
            timeout_secs: self.settings.web_timeout_secs as u64,
        }
    }

    /// 用户授权某内网/本机主机(决定脊柱 net 授权的持久化):落当前项目 `net_grants` + 当场生效。
    /// 入参容错:URL 或裸主机名都行(取主机、小写、去端口)。
    pub fn grant_net_host(&mut self, host_or_url: &str) -> Result<String, String> {
        let host = match growbox_safety::parse_http_url(host_or_url) {
            Ok((_s, h, _p)) => h,
            Err(_) => {
                let h = host_or_url.trim().trim_matches('/').to_ascii_lowercase();
                // 裸 host[:port] 形态:去端口(授权按主机)。
                h.rsplit_once(':')
                    .filter(|(_, p)| p.parse::<u16>().is_ok())
                    .map(|(h, _)| h.to_string())
                    .unwrap_or(h)
            }
        };
        if host.is_empty() {
            return Err("空主机名,无法授权".into());
        }
        if let Some(id) = self.current.clone() {
            if let Some(p) = self.projects.iter_mut().find(|p| p.id == id) {
                if !p.net_grants.iter().any(|g| g == &host) {
                    p.net_grants.push(host.clone());
                }
            }
            self.save_projects();
        }
        self.sandbox.grant(growbox_safety::GrantScope::ThisProjectHost(host.clone()));
        Ok(host)
    }
}

/// 按设置选嵌入实现:配齐远程三项且开了开关 → 远程;否则走本地默认。
/// 本地默认 = candle e5(feature `local-embed`,见 `embedding-service.md`/`打包设计.md`);
/// 关掉该 feature 时退化为词法版(离线、便宜,但无真语义)。
fn build_embedder(s: &Settings, data_dir: &Path, resource_dir: Option<&Path>) -> Arc<dyn Embedder> {
    if s.embed_remote && !s.embed_api_base.is_empty() && !s.embed_model.is_empty() {
        return Arc::new(RemoteEmbedder::new(
            s.embed_api_base.clone(),
            s.embed_api_key.clone(),
            s.embed_model.clone(),
        ));
    }
    build_local_embedder(data_dir, resource_dir)
}

/// 本地默认嵌入器:candle e5。模型解析顺序见 `打包设计.md`:
/// resource_dir/models(带模型包预置)→ data_dir/models(下载缓存)→ hf-hub 下载到 data_dir/models。
#[cfg(feature = "local-embed")]
fn build_local_embedder(data_dir: &Path, resource_dir: Option<&Path>) -> Arc<dyn Embedder> {
    let data_models = data_dir.join("models");
    let mut search = Vec::new();
    if let Some(rd) = resource_dir {
        search.push(rd.join("models"));
    }
    search.push(data_models.clone());
    Arc::new(growbox_llm::LocalE5Embedder::new(search, data_models))
}

/// 未编入本地模型(--no-default-features)时退化为词法版。
#[cfg(not(feature = "local-embed"))]
fn build_local_embedder(_data_dir: &Path, _resource_dir: Option<&Path>) -> Arc<dyn Embedder> {
    Arc::new(growbox_llm::LexicalEmbedder)
}

/// 一次性迁移:旧的 settings.json / projects.json → DB(仅当 DB 内还没有时)。
/// 迁移后旧文件保留不动(只读一次),避免误删用户数据。
fn migrate_legacy_json(store: &Store, data_dir: &Path) {
    if store.kv_get::<Settings>("settings").is_none() {
        if let Some(s) = load_json::<Settings>(&data_dir.join("settings.json")) {
            store.kv_put("settings", &s);
        }
    }
    if store.kv_get::<Vec<ProjectConfig>>("projects").is_none() {
        if let Some(p) = load_json::<Vec<ProjectConfig>>(&data_dir.join("projects.json")) {
            store.kv_put("projects", &p);
        }
    }
}

fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

/// 同 ID 的项目自动重命名(加 -2, -3 ...),保留全部数据不丢。
fn dedup_projects(mut projects: Vec<ProjectConfig>) -> Vec<ProjectConfig> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for p in &mut projects {
        let entry = seen.entry(p.id.clone()).or_insert(0);
        if *entry > 0 {
            // 重复:原 ID 加序号
            p.id = format!("{}-{}", p.id, *entry + 1);
            p.name = format!("{}-{}", p.name, *entry + 1);
        }
        *entry += 1;
    }
    projects
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn new_uses_defaults_when_empty() {
        let dir = tempdir().unwrap();
        let st = AppState::new(dir.path().to_path_buf());
        assert!(!st.connected);
        assert_eq!(st.settings.model, "deepseek-v4-flash");
        assert!(st.projects.is_empty());
    }

    #[test]
    fn create_and_switch_project_rebuilds_sandbox() {
        let dir = tempdir().unwrap();
        let work = dir.path().join("proj");
        std::fs::create_dir_all(&work).unwrap();
        let mut st = AppState::new(dir.path().to_path_buf());
        let id = st.create_project(None, "博客", vec![work.clone()], vec![]);
        assert_eq!(st.current.as_deref(), Some(id.as_str()));
        assert_eq!(st.work_dir, work);
        // 沙箱按项目可写根判定。
        use growbox_safety::{Operation, Verdict};
        assert_eq!(st.sandbox.judge(&Operation::Write(&work.join("a.txt"))), Verdict::Allow);
    }

    #[test]
    fn create_project_recursively_creates_missing_writable_dir() {
        // 真机暴露:新建项目指向"不存在的多层路径"时,旧逻辑不建目录 → work_dir 磁盘上没有 →
        // shell 等以它为 cwd 的进程 spawn 直接失败。修复后应递归建好。
        let dir = tempdir().unwrap();
        let work = dir.path().join("Games").join("VibeGame"); // 故意不预先 mkdir
        assert!(!work.exists(), "前提:目录一开始不存在");
        let mut st = AppState::new(dir.path().to_path_buf());
        st.create_project(None, "VibeGame", vec![work.clone()], vec![]);
        assert!(work.exists() && work.is_dir(), "create_project 应递归创建缺失的可写目录");
        assert_eq!(st.work_dir, work);
    }

    #[test]
    fn note_ui_panel_perceives_only_on_real_flip() {
        let dir = tempdir().unwrap();
        let mut st = AppState::new(dir.path().to_path_buf());
        // 首次上报(启动同步):填缓存,不算翻转、不 perceive。
        assert!(!st.note_ui_panel("memory", false));
        // 同值再报:无翻转。
        assert!(!st.note_ui_panel("memory", false));
        // 真实翻转:perceive 一条。
        assert!(st.note_ui_panel("memory", true));
        // 缓存恒真。
        assert_eq!(st.ui_panel_state.get("memory"), Some(&true));
        // 内部状态环里能查到这条界面变化感知(agent 可见)。
        assert!(st.memory.render_internal_state("zh").map(|s| s.contains("memory 面板")).unwrap_or(false));
    }

    #[test]
    fn settings_and_projects_persist_across_reload() {
        let dir = tempdir().unwrap();
        let work = dir.path().join("p");
        std::fs::create_dir_all(&work).unwrap();
        {
            let mut st = AppState::new(dir.path().to_path_buf());
            let s = Settings { api_key: "k".into(), ..Default::default() };
            st.connect(s);
            st.create_project(None, "P", vec![work.clone()], vec![]);
        }
        // 重新加载:设置与项目还在。
        let st2 = AppState::new(dir.path().to_path_buf());
        assert_eq!(st2.settings.api_key, "k");
        assert_eq!(st2.projects.len(), 1);
        assert_eq!(st2.current_project().unwrap().name, "P");
    }

    #[test]
    fn skill_proposals_dedup_and_persist() {
        let dir = tempdir().unwrap();
        let mut st = AppState::new(dir.path().to_path_buf());
        // 正常入队。
        assert!(st.try_add_skill_proposal("deploy-canary", "金丝雀发布时", "1. 起小流量", "rat"));
        assert_eq!(st.skill_proposals.pending.len(), 1);
        // 同名再提 → 拒(已在队列)。
        assert!(!st.try_add_skill_proposal("deploy-canary", "x", "y", "z"));
        // 名撞内置种子 → 拒(已是 skill,种子连接时已物化)。
        assert!(!st.try_add_skill_proposal("read-before-write", "x", "playbook body", "z"));
        // 空名/空正文 → 拒。
        assert!(!st.try_add_skill_proposal("", "t", "b", "r"));
        assert!(!st.try_add_skill_proposal("ok-name", "t", "  ", "r"));
        // 丢弃后进"不再提"名单 → 同名不再入队。
        let id = st.skill_proposals.pending[0].id.clone();
        assert!(st.skill_proposals.reject(&id).is_some());
        st.persist_skill_proposals();
        assert!(!st.try_add_skill_proposal("deploy-canary", "金丝雀", "body", "r"), "拒过的名不再提");
        drop(st); // 释放 redb 单写锁,才能重开同一文件验证持久化。
        // 持久化:重载后 rejected 名单还在。
        let st2 = AppState::new(dir.path().to_path_buf());
        assert!(st2.skill_proposals.is_rejected("deploy-canary"));
    }
}
