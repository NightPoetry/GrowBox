// ============================================================
// GrowBox Tauri API 桥（phase 4 实装）
// ------------------------------------------------------------
// 集中所有 invoke 调用，组件不直接访问 window.__TAURI__。
// 命令名/参数 schema 与 v1 完全一致（见 crates/growbox-gui/src/cmds.rs）。
// ============================================================

export type SessionId = string;

export interface ProjectInfo {
  id: string;
  name: string;
  work_dir: string;
  writable: string[];
  readonly: string[];
  description?: string;
}

export interface ProjectSummary {
  id: string;
  name: string;
  archived: boolean;
  writable: string[];
  readonly: string[];
  experience_count: number;
  knowledge_count: number;
  understanding_count: number;
}

export interface ProjectDirectories {
  id: string;
  name: string;
  writable: string[];
  readonly: string[];
  work_dir: string;
}

export interface StatusInfo {
  connected: boolean;
  session_id?: string;
  model?: string;
  budget_pct: number;
  fatigue: number;
  // 记忆置换率 [0,1]:邻域缓存淘汰压力(面板原"压力"表改名实装)。
  replacement_rate: number;
  // 实时上下文压力:最近请求实发上下文 token(模型亲口算)/ 上下文窗口总量(可设)。
  ctx_prompt_tokens: number;
  ctx_window_tokens: number;
  attention_span: number;
  cache_used: number;       // 邻域缓存占用条目数(平铺单 LFU,退役三级;L2 加速器,非记忆缓存)
  cache_capacity: number;   // 邻域缓存容量(可在控制面板设置)
  // ★缓存队列(工作区=存放区=真·临时记忆)★:侧栏仪表真实来源(常驻数 + 真/假指针;填充率读 budget_pct)。
  queue_resident: number;   // 缓存队列常驻块数(随检索涨,Nap 清零)
  queue_fake: number;       // 队列里假指针(RAG 命中)条数
  queue_real: number;       // 队列里真指针(L2 命中)条数
  running_tasks: number;    // 后台运行中的任务数(状态栏 "shell k")
  l2_index_size: number;
  pointer_count: number;
  // Phase 8: enriched memory metrics
  coverage_deep_green_pct: number;
  coverage_light_green_pct: number;
  coverage_red_pct: number;
  coverage_gray_pct: number;
  reverse_index_size: number;
  subconscious_wired: boolean;
  fragment_count: number;
  index_density: number;
  total_nodes: number;
  health?: HealthInfo;
}

// 健康/异常告知(对应后端 health.rs;设计见 异常告知.md)。
export type HealthLevel = "ok" | "notice" | "degraded" | "fatal";
export interface HealthIssue {
  code: string;
  severity: HealthLevel;
  // 显示文案由前端按 ui_lang 从 notices.i18n.json catalog 渲染(noticeText(code,params));后端不再传死中文。
  params?: Record<string, unknown>;
}
export interface HealthInfo {
  level: HealthLevel;
  issues: HealthIssue[];
}

export interface ChatHistoryItem {
  session_id: string;
  seq: number;
  ts: string;       // RFC3339
  role: string;
  content: string;
}

export interface CitationContext {
  cited: ChatHistoryItem;
  before: ChatHistoryItem[];
  after: ChatHistoryItem[];
  session_id: string;
}

export interface CitationBlock {
  id: string;
  session_id: string;
  ts: string;
  contentPreview: string;
  fullContent: string;
  before: ChatHistoryItem[];
  after: ChatHistoryItem[];
}

export interface ChatResponse {
  content: string;
  model: string;
  input_tokens: number;
  output_tokens: number;
}

// 自驱续跑单步返回:ChatResponse + did_work(这一轮自驱有没有真正动手)。
export interface SelfDriveResponse extends ChatResponse {
  did_work: boolean;
}

export interface SafetySummary {
  backups_dir: string;
  backup_count: number;
  note: string;
}
export interface NetworkStatus {
  main_model: string;
  sub_model?: string;
  has_embedder: boolean;
}
export interface ControlState {
  safety: SafetySummary;
  memory_stats: MemoryStats | null;
  network_status: NetworkStatus;
  error_rate_pct: number;
  json_compliance_pct: number;
  tool_call_total: number;
  tool_call_fail: number;
  json_call_total: number;
  json_call_valid: number;
}

export interface MemoryStats {
  total_nodes: number;
  total_pointers: number;
  // 平铺单 LFU 邻域缓存(退役"三级 1:2:4"):占用 / 容量 / 命中率 / 淘汰数。这是 L2 导航边缓存=加速层(幕后)。
  cache: {
    used: number;
    capacity: number;
    hit_rate: number;
    total_evictions: number;
  };
  // ★缓存队列(工作区=存放区=置换系统的"物理内存")★:面板「缓存队列」(原「队列占用/邻域缓存」)真实来源。
  // fill_pct=占用/预算(队列从空涨起,满了是常态)· resident=常驻块数 · evictions/replacement_rate=真置换 churn
  //（队列换出=记忆区换出,同一事件;Nap 归零)· fake/real_pointers=队列里假指针(RAG)/真指针(L2)各几条。
  queue: {
    fill_pct: number;
    resident: number;
    evictions: number;
    replacement_rate: number;
    fake_pointers: number;
    real_pointers: number;
  };
  fatigue: {
    cache_hit_rate: number;
    eviction_rate: number;
    fragment_count: number;
    fragment_ratio: number;
    fatigue_value: number;
  };
  secondary_indexes: {
    total: number;
    forced_jumps: number;
    fragment_count: number;
  };
  // 学习型指针旋钮(控制面板回显 + 可设;推论9 数值全可设)。
  pointer: {
    match_mode: string; // "weighted_cosine" | "llm_judge"
    follow_threshold: number;
    neg_block_threshold: number;
    k_merge_threshold: number;
    weight_gain: number;
    k_cap: number;
    force_judge: boolean; // 档A 命中后是否仍走前沿 judge 确认(true=精确,false=直接采纳省 LLM)
  };
  // 检索行为旋钮(控制面板回显 + 可设;推论9 数值全可设)。
  retrieval: {
    rag_hit_threshold: number; // 第一层 RAG 命中阈(≥ 即返回不下沉)
    rag_topk: number;
    entry_k: number; // 精确层进图入口数
    entry_min_sim: number;
    scan_batch: number; // 精确层线性扫每批 judge 数
    project_boost: number; // 项目软偏好系数(本项目命中相似度 ×(1+此值))
  };
  // 疲劳公式权重旋钮(控制面板回显 + 可设;推论9 数值全可设)。
  fatigue_weights: {
    w_hitrate: number;
    w_evict: number;
    w_fragment: number;
  };
  // 瞬态容量旋钮(控制面板回显 + 可设;推论9 数值全可设)。老化阈以天回显。
  transient: {
    fragment_ledger_cap: number;
    secondary_index_cap: number;
    internal_events_cap: number;
    artifact_interactions_cap: number;
    neg_review_max_age_days: number;
    neg_review_max_edges: number;
  };
}

// 后台任务(#4):对外列表展示 tag(label)+ 原始命令 + 状态(get_tasks 与 LLM 的 list_tasks 同源)。
export interface TaskInfo {
  id: string;
  label: string;
  command: string;
  state: "running" | "done" | "failed";
  elapsed_ms: number;
}
export interface TasksSnapshot {
  running: number;
  tasks: TaskInfo[];
}

export interface DreamSession {
  session_id: string;
  total_fragments: number;
  processed: number;
  discoveries: number;
  is_complete: boolean;
}

export interface DreamSummary {
  session_id: string;
  total_fragments: number;
  processed: number;
  total_discoveries: number;
  duration_ms: number;
}

declare global {
  interface Window {
    __TAURI__?: {
      core: { invoke: <T = unknown>(cmd: string, args?: unknown) => Promise<T> };
      event: {
        listen: <T = unknown>(
          event: string,
          cb: (e: { payload: T }) => void
        ) => Promise<() => void>;
      };
    };
  }
}

export function tauriAvailable(): boolean {
  return typeof window !== "undefined" && !!window.__TAURI__;
}

function invoke<T>(cmd: string, args?: unknown): Promise<T> {
  if (!window.__TAURI__) {
    return Promise.reject(
      new Error(`tauri-api[${cmd}]: 当前在浏览器环境（无 Tauri 桥），需要在 Tauri 窗口内运行`)
    );
  }
  return window.__TAURI__.core.invoke<T>(cmd, args);
}

export function listen<T = unknown>(
  event: string,
  cb: (payload: T) => void
): Promise<() => void> {
  if (!window.__TAURI__) {
    return Promise.reject(
      new Error(`tauri-api[listen:${event}]: 当前在浏览器环境（无 Tauri 桥）`)
    );
  }
  return window.__TAURI__.event.listen<T>(event, (e) => cb(e.payload));
}

// 活的 IDE:前端声明给后端的可控面板(单一真相在前端,见 ui-actions.ts)。
export interface UiSurface {
  id: string;
  label: string;
  ops: string[];
}

// 感知告知 双受众:后端按 ui_lang 下发的对外目录条目(单一源 notices.i18n.json,见 notices.ts)。
export interface NoticeEntry {
  code: string;
  severity: "info" | "success" | "warn" | "error";
  surface: "toast" | "health" | "silent";
  perceive: boolean;
  human: string; // 含 {x} 占位符,前端填参后显示
}

export const api = {
  // 应用版本号:单一事实源 = 后端 env!(CARGO_PKG_VERSION) = workspace Cargo.toml。
  // 显示绝不硬编码,改版本只改 Cargo.toml 一处。
  appVersion(): Promise<string> {
    return invoke<string>("app_version");
  },
  // ── 连接 ─────────────────────────────────────────────────
  connect(params: {
    apiBase: string;
    model: string;
    apiKey?: string;
    runtimeDir: string;
    maxTurns?: number;
    maxTokens?: number;
    supervisorModel?: string;
    supervisorApiBase?: string;
    supervisorApiKey?: string;
    embedRemote?: boolean;
    embedApiBase?: string;
    embedApiKey?: string;
    embedModel?: string;
    subconsciousModel?: string;
    subconsciousApiBase?: string;
    subconsciousApiKey?: string;
    workingContextChars?: number;
    recentRingChars?: number;
    neighborCacheCap?: number;
  }): Promise<SessionId> {
    return invoke<SessionId>("connect", params);
  },
  // 仅初始化 runtime_dir + ProjectManager，不连 LLM。phase 5 加：让未连 LM 时也能浏览项目。
  setRuntimeDir(runtimeDir: string): Promise<void> {
    return invoke<void>("set_runtime_dir", { runtimeDir });
  },
  // 扫描 OpenAI-兼容 API 的可用模型列表（不触发 JIT-load，5s timeout）
  listModels(apiBase: string, apiKey?: string): Promise<string[]> {
    return invoke<string[]>("list_models", { apiBase, apiKey });
  },

  // ── 状态 / 仪表 ──────────────────────────────────────────
  getStatus(): Promise<StatusInfo> {
    return invoke<StatusInfo>("get_status");
  },
  resetChatSession(): Promise<string> {
    return invoke<string>("reset_chat_session");
  },

  // ── i18n ─────────────────────────────────────────────────
  getTranslations(lang: string): Promise<Record<string, string>> {
    return invoke<Record<string, string>>("get_translations", { lang });
  },
  setPromptLang(lang: string): Promise<void> {
    return invoke<void>("set_prompt_lang", { lang });
  },
  // 感知告知 双受众:按界面语言取对外目录(human 已渲染到 ui_lang,占位符保留)。见 notices.ts。
  getNoticeCatalog(uiLang: string): Promise<NoticeEntry[]> {
    return invoke<NoticeEntry[]>("get_notice_catalog", { uiLang });
  },
  // 前端来源的提示 fire-and-forget 回后端感知(对内半):后端按 perceive 标志决定是否进 LLM 感知。
  perceiveNotice(code: string, params: Record<string, unknown>): Promise<void> {
    return invoke<void>("perceive_notice", { code, params });
  },

  // ── 项目 ─────────────────────────────────────────────────
  listProjects(): Promise<ProjectSummary[]> {
    return invoke<ProjectSummary[]>("list_projects");
  },
  currentProject(): Promise<string | null> {
    return invoke<string | null>("current_project");
  },
  switchProject(id: string): Promise<unknown> {
    return invoke("switch_project", { id });
  },
  createProject(args: {
    id: string;
    name: string;
    writable: string[];
    readonly: string[];
    description?: string;
  }): Promise<string> {
    return invoke<string>("create_project", { args });
  },
  getProjectDirectories(id: string | null = null): Promise<ProjectDirectories | null> {
    return invoke<ProjectDirectories | null>("get_project_directories", { id });
  },
  updateProjectDirectories(args: {
    id: string;
    writable: string[];
    readonly: string[];
  }): Promise<unknown> {
    return invoke("update_project_directories", { args });
  },

  pickDirectory(): Promise<string | null> {
    return invoke<string | null>("pick_directory");
  },

  // ── chat ─────────────────────────────────────────────────
  sendMessage(message: string): Promise<ChatResponse> {
    return invoke<ChatResponse>("send_message", { message });
  },
  sendMessageStream(message: string): Promise<ChatResponse> {
    return invoke<ChatResponse>("send_message_stream", { message });
  },
  // 终止当前回合(造物交互 v2 §2):瞬时置取消标志,后端脊柱下一检查点优雅收口。
  cancelChat(): Promise<void> {
    return invoke<void>("cancel_chat");
  },
  // 造物窗口关闭硬机制(造物交互 v2 §4):用户真关造物 → 后端取消在跑回合 + AI 感知端口不通 + 用户 toast。
  artifactClosed(canvasId: string): Promise<void> {
    return invoke<void>("artifact_closed", { canvasId });
  },
  // 交互式终端(人机共驾 shell):用户在 xterm 键入 → 写进 PTY;事件点 → 唤醒 AI;关面板 → kill+感知。
  ptyInput(sessionId: string, data: string): Promise<void> {
    return invoke<void>("pty_input", { sessionId, data });
  },
  // 返回 AI 设的"看守间隔"秒(P3 自适应轮询;0=不轮询,纯事件驱动)。
  terminalEvent(sessionId: string): Promise<number> {
    return invoke<number>("terminal_event", { sessionId });
  },
  terminalClosed(sessionId: string): Promise<void> {
    return invoke<void>("terminal_closed", { sessionId });
  },
  // 自关机能力:用户一次性授权(或永久权)后真正执行关机。exit_self=关闭自己;system_shutdown=关机器。
  doShutdown(action: string, delaySecs: number): Promise<void> {
    return invoke<void>("do_shutdown", { action, delaySecs });
  },
  vaccinatePermission(kind: string): Promise<void> {
    return invoke<void>("vaccinate_permission", { kind });
  },
  setAutoShutdownAllowed(allowed: boolean): Promise<void> {
    return invoke<void>("set_auto_shutdown_allowed", { allowed });
  },
  setCachePrewarm(enabled: boolean): Promise<void> {
    return invoke<void>("set_cache_prewarm", { enabled });
  },
  // 内部消息(非用户说的话:面板裁决回流等)。后端经 perceive 感知 + 内部 seed,AI 有权不执行。
  sendInternalMessage(message: string): Promise<ChatResponse> {
    return invoke<ChatResponse>("send_internal_message", { message });
  },
  // 自驱续跑一步(全自动模式下的"自动鞭策"):后端自建"继续推进"种子(role=internal,进记录不进历史),
  // 驱动 AI 评估现状/治理屎山/动手做下一步;返回 did_work 让前端决定是否继续循环。
  selfDriveStep(): Promise<SelfDriveResponse> {
    return invoke<SelfDriveResponse>("self_drive_step", {});
  },
  // 工具命令/路径显示:完整 or 截断。即时生效,持久化到后端 Settings。
  setAutoMode(auto: boolean): Promise<void> {
    return invoke<void>("set_auto_mode", { auto });
  },
  // ★danger 模式(为所欲为)★:全自动之上的最高放行档,所有安全门一律放行。不持久(后端 serde(skip))。
  setDangerMode(danger: boolean): Promise<void> {
    return invoke<void>("set_danger_mode", { danger });
  },
  // 自驱续跑:主动跑一次飞轮消化(防持续自驱时经验堆积);返回提炼出的知识条数。
  runDigestPass(): Promise<number> {
    return invoke<number>("run_digest_pass", {});
  },
  // 用户决定脊柱回执(权限/shell 共用)。decision: "once"|"remember"|"trust_project"|"deny"。
  decisionAck(id: string, decision: string): Promise<void> {
    return invoke<void>("decision_ack", { id, decision });
  },
  // 打开外部 URL(系统浏览器)。校验后由后端 OS open/xdg-open/start 打开,不导航 webview。
  openExternalUrl(url: string): Promise<void> {
    return invoke<void>("open_external_url", { url });
  },
  // 隐私文件夹列表(命中必弹窗 + 二次确认)。
  getPrivacyDirs(): Promise<string[]> {
    return invoke<string[]>("get_privacy_dirs");
  },
  setPrivacyDirs(dirs: string[]): Promise<void> {
    return invoke<void>("set_privacy_dirs", { dirs });
  },
  setTruncateToolDisplay(truncate: boolean): Promise<void> {
    return invoke<void>("set_truncate_tool_display", { truncate });
  },
  // 授权放行本项目 shell 系统路径引用(项目级 shell 信任,不污染目录列表)。
  grantShellAccess(): Promise<void> {
    return invoke<void>("grant_shell_access");
  },
  // 授权放行某内网/本机主机(web_fetch 等出站访问;项目级持久,不污染目录/不放宽文件)。
  // 入参 URL 或主机名均可,返回实际授权的主机。
  grantNetHost(host: string): Promise<string> {
    return invoke<string>("grant_net_host", { host });
  },
  // ★Skill 提议(S3 飞轮自学)★:idle 起草的待裁决提议;采纳→结晶成 skill,丢弃→不再提。
  listSkillProposals(): Promise<{ id: string; name: string; trigger: string; body: string; rationale: string; created_ms: number }[]> {
    return invoke("list_skill_proposals");
  },
  acceptSkillProposal(id: string): Promise<string> {
    return invoke<string>("accept_skill_proposal", { id });
  },
  rejectSkillProposal(id: string): Promise<void> {
    return invoke<void>("reject_skill_proposal", { id });
  },
  // ★工具记忆 + 不犯第二遍★:总开关 + 两个相似度阈(infeasible 硬否决 / fails 软提醒)。
  getToolMemoryConfig(): Promise<{ enabled: boolean; veto_threshold: number; warn_threshold: number }> {
    return invoke("get_tool_memory_config");
  },
  setToolMemoryConfig(enabled: boolean, vetoThreshold: number, warnThreshold: number): Promise<void> {
    return invoke<void>("set_tool_memory_config", { enabled, vetoThreshold, warnThreshold });
  },
  // Web 工具配置(web_search provider/端点/key + 条数;web_fetch/web_search 超时)。
  getWebConfig(): Promise<{ provider: string; api_base: string; api_key: string; max_results: number; timeout_secs: number }> {
    return invoke("get_web_config");
  },
  setWebConfig(provider: string, apiBase: string, apiKey: string, maxResults: number, timeoutSecs: number): Promise<void> {
    return invoke<void>("set_web_config", { provider, apiBase, apiKey, maxResults, timeoutSecs });
  },
  getChatHistory(beforeTs: string | null, n: number, sessionId?: string | null): Promise<ChatHistoryItem[]> {
    return invoke<ChatHistoryItem[]>("get_chat_history", { beforeTs, n, sessionId: sessionId ?? null });
  },
  // 完整保真展示记录:存"用户看到的"富消息(含思考/工具卡/meta),按当前项目落库,重启原样还原。
  saveChatTranscript(messages: unknown[]): Promise<void> {
    return invoke("save_chat_transcript", { messages });
  },
  // 取当前项目的完整展示记录;null = 没存过 → 调用方回退 getChatHistory(时间线派生)。
  loadChatTranscript(): Promise<unknown[] | null> {
    return invoke<unknown[] | null>("load_chat_transcript");
  },
  /// 当前项目的连贯对话历史（合并所有 session）
  getProjectConversationHistory(beforeTs: string | null, limit?: number): Promise<ChatHistoryItem[]> {
    return invoke<ChatHistoryItem[]>("get_project_conversation_history", { beforeTs, limit: limit ?? 50 });
  },
  /// 获取引用消息的完整上下文（前后各 N 条，LLM 用于理解引用）
  getCitationContext(sessionId: string, ts: string, radius?: number): Promise<CitationContext> {
    return invoke<CitationContext>("get_citation_context", { sessionId, ts, radius: radius ?? 5 });
  },
  listSessions(): Promise<{ session_id: string; size_bytes: number }[]> {
    return invoke<{ session_id: string; size_bytes: number }[]>("list_sessions");
  },

  // ── 工具 ─────────────────────────────────────────────────
  // name 恒英文 key(toggle 用);label/description 由后端按 ui_lang 本地化(单一源 tools.i18n.json)。
  getTools(uiLang: string): Promise<{ name: string; label: string; description: string; enabled: boolean }[]> {
    return invoke<{ name: string; label: string; description: string; enabled: boolean }[]>("get_tools", { uiLang });
  },
  // 工具分类(聊天图标区分用):哪些可调用名是工作流 / MCP 外部工具(其余=内置)。
  getToolKinds(): Promise<{ workflows: string[]; mcp: string[] }> {
    return invoke("get_tool_kinds");
  },
  setTools(tools: string[]): Promise<unknown> {
    return invoke("set_tools", { tools });
  },

  // ── 控制闭环（6-C）──────────────────────────────────────
  getControlState(): Promise<ControlState> {
    return invoke<ControlState>("get_control_state");
  },

  // ── 记忆 / 疲劳 ──────────────────────────────────────────
  getMemoryStats(): Promise<MemoryStats> {
    return invoke<MemoryStats>("get_memory_stats");
  },
  getFatigueLevel(): Promise<number> {
    return invoke<number>("get_fatigue_level");
  },
  // 后台任务快照(#4 状态栏 shell k 点开看列表):running 计数 + 每条 tag/原命令/状态。
  getTasks(): Promise<TasksSnapshot> {
    return invoke<TasksSnapshot>("get_tasks");
  },
  // 设置邻域缓存容量(控制面板可调,即时生效+落库;推论9 数值全可设首例)。
  setNeighborCacheCap(cap: number): Promise<void> {
    return invoke<void>("set_neighbor_cache_cap", { cap });
  },
  // 设置学习型指针旋钮(匹配档 + 4 阈值;即时生效+落库;推论9)。
  setPointerConfig(p: {
    matchMode: string;
    followThreshold: number;
    negBlockThreshold: number;
    kMergeThreshold: number;
    weightGain: number;
    kCap: number;
    forceJudge: boolean;
  }): Promise<void> {
    return invoke<void>("set_pointer_config", p);
  },
  // 设置疲劳公式权重旋钮(命中率/淘汰/碎片;即时生效+落库;推论9)。
  setFatigueConfig(p: {
    wHitrate: number;
    wEvict: number;
    wFragment: number;
  }): Promise<void> {
    return invoke<void>("set_fatigue_config", p);
  },
  // 设置瞬态容量旋钮(碎片/二级/内部环 cap + 反K复核天数/边数;即时生效+落库;推论9)。
  setTransientCaps(p: {
    fragmentLedgerCap: number;
    secondaryIndexCap: number;
    internalEventsCap: number;
    artifactInteractionsCap: number;
    negReviewMaxAgeDays: number;
    negReviewMaxEdges: number;
  }): Promise<void> {
    return invoke<void>("set_transient_caps", p);
  },
  // 设置检索行为旋钮(RAG 命中阈/候选数 + 精确层入口/批量;即时生效+落库;推论9)。
  setRetrievalConfig(p: {
    ragHitThreshold: number;
    ragTopk: number;
    entryK: number;
    entryMinSim: number;
    scanBatch: number;
    projectBoost: number;
  }): Promise<void> {
    return invoke<void>("set_retrieval_config", p);
  },
  // 设置 Agent 循环旋钮(截断重试/token上限/沉默超时/空转 下回合生效;complete 沉默超时下次重连生效;落库;推论9)。
  setAgentConfig(p: {
    maxTokenRetries: number;
    tokenCeil: number;
    silenceSecs: number;
    maxStall: number;
    parallelMax: number;
    completeSilenceSecs: number;
    reasoningEffort: string;
    selfVerify: boolean;
    selfVerifyMinTools: number;
    recallInLoop: boolean;
  }): Promise<void> {
    return invoke<void>("set_agent_config", p);
  },
  // 回显当前 Agent 循环旋钮(Settings 面板加载时取)。
  getAgentConfig(): Promise<{
    max_token_retries: number;
    token_ceil: number;
    silence_secs: number;
    max_stall: number;
    parallel_max: number;
    complete_silence_secs: number;
    reasoning_effort: string;
    self_verify: boolean;
    self_verify_min_tools: number;
    recall_in_loop: boolean;
  }> {
    return invoke("get_agent_config");
  },
  // 设置 idle/做梦/睡眠旋钮(IdleWorker 下一拍生效;落库;推论9)。
  setIdleConfig(p: {
    idleThresholdSecs: number;
    idleTickSecs: number;
    idleFatigueThreshold: number;
    idleMaxSleepSteps: number;
    idleMaxRehearsals: number;
  }): Promise<void> {
    return invoke<void>("set_idle_config", p);
  },
  // 回显当前 idle/睡眠旋钮(Settings 面板加载时取)。
  getIdleConfig(): Promise<{
    idle_threshold_secs: number;
    idle_tick_secs: number;
    idle_fatigue_threshold: number;
    idle_max_sleep_steps: number;
    idle_max_rehearsals: number;
  }> {
    return invoke("get_idle_config");
  },
  // 设置超时/退避旋钮(shell 批准超时下回合生效;任务退避即时;落库;推论9)。
  setMiscConfig(p: {
    shellApprovalTimeoutSecs: number;
    uiAckTimeoutSecs: number;
    taskBackoffBaseMs: number;
    taskBackoffCapMs: number;
    branchLogMaxGb: number;
  }): Promise<void> {
    return invoke<void>("set_misc_config", p);
  },
  // ★二期 C1★ 工具懒加载总开关 + deferred 名单(即时生效:更新 settings + 重注入 registry;落库)。
  setLazyToolsConfig(p: { lazyTools: boolean; deferredTools: string[] }): Promise<void> {
    return invoke<void>("set_lazy_tools_config", p);
  },
  // ★提示词自转译(自我负责-输入侧)★ 开关(即时生效:运行时取用层立刻按新值走覆盖/原文;落库)。
  setTranspileEnabled(enabled: boolean): Promise<void> {
    return invoke<void>("set_transpile_enabled", { enabled });
  },
  // 回显自转译状态(面板加载时取):开关 + 覆盖条数 + 当前模型 + 并发数。
  getTranspileStatus(): Promise<{ enabled: boolean; override_count: number; model: string; connected: boolean; concurrency: number }> {
    return invoke("get_transpile_status");
  },
  // 设转译并发数(落库,下次重写即用)。
  setTranspileConcurrency(concurrency: number): Promise<void> {
    return invoke<void>("set_transpile_concurrency", { concurrency });
  },
  // ★用当前模型重写所有提示词★(谁消费谁转译;长任务,进度走 "transpile-progress" 事件)。存成新版本(可后悔)。
  transpilePrompts(): Promise<{ total: number; written: number; skipped: number; snapshot: string }> {
    return invoke("transpile_prompts");
  },
  // ★历史提示词★:列出版本(新→旧)+ 当前激活 id(default = 原文)。
  transpileListSnapshots(): Promise<{ active: string; snapshots: Array<{ id: string; name: string; model: string; count: number; created_ms: number }> }> {
    return invoke("transpile_list_snapshots");
  },
  // 激活某历史版本(id=default 还原到原文)。即时生效。
  transpileActivateSnapshot(id: string): Promise<void> {
    return invoke<void>("transpile_activate_snapshot", { id });
  },
  // 重命名历史版本(default 不可改)。
  transpileRenameSnapshot(id: string, name: string): Promise<void> {
    return invoke<void>("transpile_rename_snapshot", { id, name });
  },
  // 删除历史版本(default 拒删;删激活版回落原文)。
  transpileDeleteSnapshot(id: string): Promise<void> {
    return invoke<void>("transpile_delete_snapshot", { id });
  },
  // 导出历史版本成磁盘 .zip 文件,返回绝对路径。
  transpileExportSnapshot(id: string): Promise<string> {
    return invoke<string>("transpile_export_snapshot", { id });
  },
  // 从上传的 .zip(base64 字节)导入成新版本(置激活)。
  transpileImportSnapshot(name: string, dataB64: string): Promise<{ id: string; name: string; count: number }> {
    return invoke("transpile_import_snapshot", { name, dataB64 });
  },
  // ★二期 D2★ MCP server 连接配置:落库持久 + 全量重连 + 回每个 server 状态。
  mcpSetServers(p: {
    servers: Array<{ name: string; command: string; args: string[]; env: Record<string, string>; enabled: boolean }>;
  }): Promise<{ servers: any[] }> {
    return invoke("mcp_set_servers", p);
  },
  // ★二期 D2★ 取 MCP 配置 + 实时连接状态(面板加载/刷新)。
  mcpGetStatus(): Promise<{ servers: any[]; configs: any[] }> {
    return invoke("mcp_get_status");
  },
  // 回显当前超时/退避旋钮(Settings 面板加载时取)。
  getMiscConfig(): Promise<{
    shell_approval_timeout_secs: number;
    ui_ack_timeout_secs: number;
    task_backoff_base_ms: number;
    task_backoff_cap_ms: number;
    auto_shutdown_allowed: boolean;
    cache_prewarm: boolean;
    branch_log_max_gb: number;
    lazy_tools: boolean;
    deferred_tools: string[];
  }> {
    return invoke("get_misc_config");
  },
  // 设置工具输出上限旋钮(file/shell/任务输出;即时生效+落库;推论9)。
  setToolLimits(p: {
    maxReadBytes: number;
    maxListEntries: number;
    maxOutputBytes: number;
    maxOutlineSymbols: number;
    taskOutputCap: number;
    contextWindowTokens: number;
    shellTimeoutSecs: number;
  }): Promise<void> {
    return invoke<void>("set_tool_limits", p);
  },
  // ★tsserver 自动装配★:经 npm 装 TS/JS 语言服务器进 GrowBox 自有目录,返回二进制路径。
  installTsserver(): Promise<string> {
    return invoke<string>("install_tsserver");
  },
  // 回显 TS/JS 语言服务器状态(已装?有 npm?)。
  tsserverStatus(): Promise<{ installed: boolean; npm: boolean }> {
    return invoke("tsserver_status");
  },
  // 回显当前工具输出上限旋钮(Settings 面板加载时取)。
  getToolLimits(): Promise<{
    max_read_bytes: number;
    max_list_entries: number;
    max_output_bytes: number;
    max_outline_symbols: number;
    task_output_cap: number;
    context_window_tokens: number;
    shell_timeout_secs: number;
  }> {
    return invoke("get_tool_limits");
  },

  // ── 梦境 / 整理 ─────────────────────────────────────────
  dreamStart(fragmentIds?: string[]): Promise<DreamSession> {
    return invoke<DreamSession>("dream_start", { fragmentIds: fragmentIds ?? null });
  },
  dreamStatus(): Promise<DreamSummary> {
    return invoke<DreamSummary>("dream_status");
  },

  suggestionResponse(choice: string): Promise<unknown> {
    return invoke("suggestion_response", { choice });
  },

  // ── 退出 ──
  confirmAppExit(): Promise<void> {
    return invoke("confirm_app_exit");
  },

  // ── 活的 IDE:UI 操控(推论 7)──
  // mount 时声明本前端有哪些可被 LLM 操控的面板(后端据此生成 ui_control 的 schema)。
  registerUiSurfaces(surfaces: UiSurface[]): Promise<void> {
    return invoke("register_ui_surfaces", { surfaces });
  },
  // 落地一个家族二 UI 操作后回执(往返的另一半):把验证态投回等待的脊柱。
  uiActionAck(id: string, applied: boolean, uiState: unknown, note: string | null): Promise<void> {
    return invoke("ui_action_ack", { id, applied, uiState, note });
  },
  // 上报某面板可见态变化(含用户手动开关)——感知闭合:让后端缓存恒真 + agent 看见用户的 UI 动作。
  uiStateChanged(panelId: string, open: boolean): Promise<void> {
    return invoke("ui_state_changed", { panelId, open });
  },
  // 被造物:造物 UI 交互回传(流2)。canvasId/callbackId/value → 后端 artifact_event。
  artifactEvent(canvasId: string, callbackId: string, value: string, realtime = false): Promise<void> {
    return invoke("artifact_event", { canvasId, callbackId, value, realtime });
  },
  // 网页调试(Phase 2):打开/复用调试 webview 加载本地 URL(后端建窗 + 注入套索运行时)。
  createDebugWebview(url: string): Promise<void> {
    return invoke("create_debug_webview", { url });
  },
  // 网页调试:AI 改完源码后刷新调试 webview(EJS/Express 等无 HMR 的工程靠这个看到改动)。
  reloadDebugWebview(): Promise<void> {
    return invoke("reload_debug_webview");
  },
  // 网页 QA 自反馈调试:在调试 webview 里真操作(click/fill/submit/scan/observe)并读回观察 JSON
  // (url/title/本页报错/选择器是否匹配)。返回值作为 uiActionAck.state 经脊柱回给 AI。
  webDebugDrive(op: string, selector: string, value: string): Promise<Record<string, unknown>> {
    return invoke("web_debug_drive", { op, selector, value });
  },
  // ★Skill 系统(设计/09)★:列出全部 skill(内置+已学)/ 取正文 / 停用启用 / 总开关+清单上限 / 回显。
  listSkills(): Promise<Array<{ name: string; trigger: string; source: string; active: boolean }>> {
    return invoke("list_skills");
  },
  getSkillBody(name: string): Promise<string> {
    return invoke("get_skill_body", { name });
  },
  setSkillActive(name: string, active: boolean): Promise<void> {
    return invoke("set_skill_active", { name, active });
  },
  setSkillConfig(enabled: boolean, listMax: number, autoloadThreshold: number): Promise<void> {
    return invoke("set_skill_config", { enabled, listMax, autoloadThreshold });
  },
  getSkillConfig(): Promise<{ enabled: boolean; list_max: number; autoload_threshold: number }> {
    return invoke("get_skill_config");
  },
};
