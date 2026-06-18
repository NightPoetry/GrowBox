// 全局应用状态：连接信息、settings/modal 可见性、轮询缓存。
// 设计：用 createSignal 而不是 createStore——所有状态都是平的、单值的，signal 更简单。
// 持久化：apiBase/model/runtimeDir 走 localStorage，跨会话保留用户的连接配置。

import { createSignal, createEffect, onCleanup } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import type { StatusInfo, ProjectSummary, ProjectDirectories } from "./tauri-api";

const STORAGE = {
  apiBase: "growbox_api_base",
  model: "growbox_model",
  runtimeDir: "growbox_runtime_dir",
  supervisorModel: "growbox_supervisor_model",
  supervisorApiBase: "growbox_supervisor_api_base",
  supervisorApiKey: "growbox_supervisor_api_key",
  maxTurns: "growbox_max_turns",
  maxTokens: "growbox_max_tokens",
  workingContextChars: "growbox_working_context_chars",
  recentRingChars: "growbox_recent_ring_chars",
  embedRemote: "growbox_embed_remote",
  theme: "growbox_theme",
  truncateToolDisplay: "growbox_truncate_tool_display",
  expandedTools: "growbox_expanded_tools",
  autoMode: "growbox_auto_mode",
  selfDriveIdleLimit: "growbox_self_drive_idle_limit",
  selfDriveGapSecs: "growbox_self_drive_gap_secs",
  selfDriveMaxRounds: "growbox_self_drive_max_rounds",
  selfDriveDigestEvery: "growbox_self_drive_digest_every",
  embedApiBase: "growbox_embed_api_base",
  embedApiKey: "growbox_embed_api_key",
  embedModel: "growbox_embed_model",
  subconsciousModel: "growbox_subconscious_model",
  subconsciousApiBase: "growbox_subconscious_api_base",
  subconsciousApiKey: "growbox_subconscious_api_key",
};

function lsGet(key: string, dflt: string): string {
  if (typeof localStorage === "undefined") return dflt;
  return localStorage.getItem(key) ?? dflt;
}
function lsSet(key: string, val: string): void {
  if (typeof localStorage === "undefined") return;
  localStorage.setItem(key, val);
}
function lsGetJson<T>(key: string, dflt: T): T {
  if (typeof localStorage === "undefined") return dflt;
  const raw = localStorage.getItem(key);
  if (raw == null) return dflt;
  try { return JSON.parse(raw) as T; } catch { return dflt; }
}

// 应用版本号:从后端 app_version 命令取(单一源 = workspace Cargo.toml),App mount 时拉一次。
export const [appVersion, setAppVersion] = createSignal("");

// ── 连接相关 ────────────────────────────────────────────────
export const [connected, setConnected] = createSignal(false);
export const [sessionId, setSessionId] = createSignal<string | null>(null);
export const [connecting, setConnecting] = createSignal(false);
export const [configDirty, setConfigDirty] = createSignal(false);

export const [apiBase, setApiBaseRaw] = createSignal(
  lsGet(STORAGE.apiBase, "http://localhost:1234/v1")
);
export const [model, setModelRaw] = createSignal(
  lsGet(STORAGE.model, "qwen3.5-2b")
);
export const [runtimeDir, setRuntimeDirRaw] = createSignal(
  lsGet(STORAGE.runtimeDir, ".growbox-runtime")
);
export const [supervisorModel, setSupervisorModelRaw] = createSignal(
  lsGet(STORAGE.supervisorModel, "")
);
export const [supervisorApiBase, setSupervisorApiBaseRaw] = createSignal(
  lsGet(STORAGE.supervisorApiBase, "")
);
export const [apiKey, setApiKeyRaw] = createSignal(
  lsGet("growbox_api_key", "")
);
export const [supervisorApiKey, setSupervisorApiKeyRaw] = createSignal(
  lsGet(STORAGE.supervisorApiKey, "")
);
// Agent 循环:轮数(0=无限,默认1000)、单轮输出 token(0=不限)。存为字符串,连接时解析。
export const [maxTurns, setMaxTurnsRaw] = createSignal(lsGet(STORAGE.maxTurns, "1000"));
export const [maxTokens, setMaxTokensRaw] = createSignal(lsGet(STORAGE.maxTokens, "0"));
// 上下文预算(记忆置换层 P4):工作区字符数 / 最近 ring 字符数。0 = 代码默认(随模型可调)。存为字符串,连接时解析。
export const [workingContextChars, setWorkingContextCharsRaw] = createSignal(lsGet(STORAGE.workingContextChars, "0"));
export const [recentRingChars, setRecentRingCharsRaw] = createSignal(lsGet(STORAGE.recentRingChars, "0"));
// 嵌入槽:远程开关(关=本地 e5 默认)+ 远程三项。
export const [embedRemote, setEmbedRemoteRaw] = createSignal(lsGet(STORAGE.embedRemote, "0") === "1");
// ★界面外观★:dark(默认,Apple 暗色)/ light(暖琥珀奶油亮色)/ auto(跟随系统明暗)。
// 纯前端展示偏好(同 truncateToolDisplay 一类),localStorage 持久、即时生效;真相源就是这里。
// 既可用户在设置里切,也可 LLM 经 set_appearance 执行器直接切(用户铁律:可设置项皆可被 LLM 操控)。
export type ThemePref = "dark" | "light" | "auto";
function normTheme(v: string): ThemePref { return v === "light" || v === "auto" ? v : "dark"; }
export const [theme, setThemeRaw] = createSignal<ThemePref>(normTheme(lsGet(STORAGE.theme, "dark")));
// 工具命令/路径是否截断显示。默认 "0" = 完整不截断。即时生效(不走 markDirty/重连)。
export const [truncateToolDisplay, setTruncateToolDisplayRaw] = createSignal(lsGet(STORAGE.truncateToolDisplay, "0") === "1");
// 自动模式(shell:false=手动逐条批准,true=LLM 审核)。立即生效,无需重连。
export const [autoMode, setAutoModeRaw] = createSignal(lsGet(STORAGE.autoMode, "0") === "1");
// ★自驱续跑★:全自动模式下解锁的"一直跑"开关(会话级,不持久——重启默认关,避免一开机就自己跑)。
// 激活后每当 AI 自己停下来,前端循环器自动注入"继续推进"种子(见 chat.ts runSelfDriveLoop)让它接着干。
// 只在 autoMode 为真时可用;autoMode 关掉时强制复位(见 App.tsx)。
export const [selfDriveActive, setSelfDriveActive] = createSignal(false);
// ★danger 模式(为所欲为)★:全自动之上的最高放行档(系统级操作/敏感路径/危险命令全不拦)。
// 会话级、不持久(同后端 serde(skip))——重启默认关,必须显式重开,防遗忘的危险模式自启。极高风险。
export const [dangerMode, setDangerMode] = createSignal(false);
// 自驱续跑的可调旋钮(用户铁律:一切涉及数值的都应能设置)。纯前端循环参数,localStorage 持久、即时生效。
// 存字符串(同 maxTurns),在 chat.ts 解析 + 越界回退默认。
// idleLimit = 连续多少轮 AI"没进展"(没动手 或 与上轮近乎全等)就判定没事可做/陷入退化,自动暂停(默认 2,最小 1)。
// gapSecs = 每轮之间的喘息秒数,给界面刷新和后台学习/整理一点缝隙,也降 API 速率(默认 3;0 = 不间断)。
// maxRounds = 自驱总轮数软上限,到顶暂停交还用户(默认 0 = 无限,靠进度指纹兜底防失控)。
// digestEvery = 每多少轮主动跑一次飞轮消化(防持续自驱时经验只采集不压缩堆积;默认 12,0 = 不主动消化)。
export const [selfDriveIdleLimit, setSelfDriveIdleLimitRaw] = createSignal(lsGet(STORAGE.selfDriveIdleLimit, "2"));
export const [selfDriveGapSecs, setSelfDriveGapSecsRaw] = createSignal(lsGet(STORAGE.selfDriveGapSecs, "3"));
export const [selfDriveMaxRounds, setSelfDriveMaxRoundsRaw] = createSignal(lsGet(STORAGE.selfDriveMaxRounds, "0"));
export const [selfDriveDigestEvery, setSelfDriveDigestEveryRaw] = createSignal(lsGet(STORAGE.selfDriveDigestEvery, "12"));
export function setSelfDriveIdleLimit(v: string) { setSelfDriveIdleLimitRaw(v); lsSet(STORAGE.selfDriveIdleLimit, v); }
export function setSelfDriveGapSecs(v: string) { setSelfDriveGapSecsRaw(v); lsSet(STORAGE.selfDriveGapSecs, v); }
export function setSelfDriveMaxRounds(v: string) { setSelfDriveMaxRoundsRaw(v); lsSet(STORAGE.selfDriveMaxRounds, v); }
export function setSelfDriveDigestEvery(v: string) { setSelfDriveDigestEveryRaw(v); lsSet(STORAGE.selfDriveDigestEvery, v); }
export const [embedApiBase, setEmbedApiBaseRaw] = createSignal(lsGet(STORAGE.embedApiBase, ""));
export const [embedApiKey, setEmbedApiKeyRaw] = createSignal(lsGet(STORAGE.embedApiKey, ""));
export const [embedModel, setEmbedModelRaw] = createSignal(lsGet(STORAGE.embedModel, ""));
// 独立潜意识模型槽(选填,留空=复用主模型)。改了需重连(markDirty)。
export const [subconsciousModel, setSubconsciousModelRaw] = createSignal(lsGet(STORAGE.subconsciousModel, ""));
export const [subconsciousApiBase, setSubconsciousApiBaseRaw] = createSignal(lsGet(STORAGE.subconsciousApiBase, ""));
export const [subconsciousApiKey, setSubconsciousApiKeyRaw] = createSignal(lsGet(STORAGE.subconsciousApiKey, ""));
function markDirty() { if (connected()) setConfigDirty(true); }
export function setApiBase(v: string) { setApiBaseRaw(v); lsSet(STORAGE.apiBase, v); markDirty(); }
export function setModel(v: string) { setModelRaw(v); lsSet(STORAGE.model, v); markDirty(); }
export function setApiKey(v: string) { setApiKeyRaw(v); lsSet("growbox_api_key", v); markDirty(); }
export function setRuntimeDir(v: string) { setRuntimeDirRaw(v); lsSet(STORAGE.runtimeDir, v); markDirty(); }
export function setSupervisorModel(v: string) { setSupervisorModelRaw(v); lsSet(STORAGE.supervisorModel, v); markDirty(); }
export function setSupervisorApiBase(v: string) { setSupervisorApiBaseRaw(v); lsSet(STORAGE.supervisorApiBase, v); markDirty(); }
export function setSupervisorApiKey(v: string) { setSupervisorApiKeyRaw(v); lsSet(STORAGE.supervisorApiKey, v); markDirty(); }
export function setMaxTurns(v: string) { setMaxTurnsRaw(v); lsSet(STORAGE.maxTurns, v); markDirty(); }
export function setMaxTokens(v: string) { setMaxTokensRaw(v); lsSet(STORAGE.maxTokens, v); markDirty(); }
export function setWorkingContextChars(v: string) { setWorkingContextCharsRaw(v); lsSet(STORAGE.workingContextChars, v); markDirty(); }
export function setRecentRingChars(v: string) { setRecentRingCharsRaw(v); lsSet(STORAGE.recentRingChars, v); markDirty(); }
export function setEmbedRemote(v: boolean) { setEmbedRemoteRaw(v); lsSet(STORAGE.embedRemote, v ? "1" : "0"); markDirty(); }
// 外观切换:即时生效、持久 localStorage、无需重连。body 类的实际应用见 App.tsx(含 auto 跟随系统)。
export function setTheme(v: ThemePref) { const t = normTheme(v); setThemeRaw(t); lsSet(STORAGE.theme, t); }
// 即时生效,不 markDirty(无需重连);持久化到 localStorage,backend 同步由调用方推送。
export function setTruncateToolDisplay(v: boolean) { setTruncateToolDisplayRaw(v); lsSet(STORAGE.truncateToolDisplay, v ? "1" : "0"); }
export function setAutoMode(v: boolean) { setAutoModeRaw(v); lsSet(STORAGE.autoMode, v ? "1" : "0"); }
export function setEmbedApiBase(v: string) { setEmbedApiBaseRaw(v); lsSet(STORAGE.embedApiBase, v); markDirty(); }
export function setEmbedApiKey(v: string) { setEmbedApiKeyRaw(v); lsSet(STORAGE.embedApiKey, v); markDirty(); }
export function setEmbedModel(v: string) { setEmbedModelRaw(v); lsSet(STORAGE.embedModel, v); markDirty(); }
export function setSubconsciousModel(v: string) { setSubconsciousModelRaw(v); lsSet(STORAGE.subconsciousModel, v); markDirty(); }
export function setSubconsciousApiBase(v: string) { setSubconsciousApiBaseRaw(v); lsSet(STORAGE.subconsciousApiBase, v); markDirty(); }
export function setSubconsciousApiKey(v: string) { setSubconsciousApiKeyRaw(v); lsSet(STORAGE.subconsciousApiKey, v); markDirty(); }

// ── 状态轮询缓存 ─────────────────────────────────────────────
export const [statusInfo, setStatusInfo] = createSignal<StatusInfo | null>(null);

// ── settings modal 可见性 ───────────────────────────────────
export const [settingsOpen, setSettingsOpen] = createSignal(false);
export const [settingsTab, setSettingsTab] = createSignal<
  "connection" | "agent" | "security" | "advanced" | "tools" | "control" | "about" | "dlc"
>("connection");

// ── 系统健康监控 modal 可见性 ───────────────────────────────
export const [healthMonitorOpen, setHealthMonitorOpen] = createSignal(false);
// 右侧历史抽屉可见性
export const [historyDrawerOpen, setHistoryDrawerOpen] = createSignal(false);
// 连接成功时间戳（ms epoch）,用于 uptime 显示
export const [connectedAt, setConnectedAt] = createSignal<number | null>(null);

// ── 消息列表 ─────────────────────────────────────────────────
// 用 createStore 而不是 createSignal：流式 token 走 setMsgStore('list', idx, 'content', ...)
// 精确到字段，避免 <For> 整条 row DOM 重建（v0.2.6 用户反馈"流式打字气泡闪烁"的根因）。
export type MsgRole = "user" | "assistant" | "system";
export interface Msg {
  id: number;
  role: MsgRole;
  content: string;
  thinking?: string;
  streaming?: boolean;
  meta?: { inputTokens: number; outputTokens: number; model: string };
  ts: number;
  // 最后一次收到 chunk 的时间戳（ms epoch）。用于阻塞等待动效判"静默/卡住"。
  // 缺省时回退到 ts（消息开始时间）。
  lastActivity?: number;
}
const [msgStore, setMsgStore] = createStore<{ list: Msg[] }>({ list: [] });

export function messages(): Msg[] { return msgStore.list; }

export function setMessages(updater: Msg[] | ((prev: Msg[]) => Msg[])): void {
  const next = typeof updater === "function" ? updater(msgStore.list) : updater;
  setMsgStore("list", reconcile(next, { key: "id" }));
}

export function appendMessage(m: Msg): void {
  setMsgStore("list", msgStore.list.length, m);
}

export function patchMessageById(id: number, partial: Partial<Msg>): void {
  const idx = msgStore.list.findIndex((m) => m.id === id);
  if (idx < 0) return;
  setMsgStore("list", idx, partial);
}

let msgSeq = 0;
export function nextMsgId() { return ++msgSeq; }

export const [sending, setSending] = createSignal(false);

// ── 阻塞等待动效计时器 ───────────────────────────────────────
// 每秒一拍的全局时钟,驱动流式消息上的"已等待 Ns"实时计时。
// 仅在 sending() 期间运行（由 createEffect 启停）——空闲时不空转 setInterval。
export const [nowTick, setNowTick] = createSignal(Date.now());
createEffect(() => {
  if (!sending()) return;
  // sending 转真:立刻对齐一次,再每秒推进;sending 转假时 onCleanup 清掉(SolidJS
  // 的 createEffect 返回值不作清理用,必须用 onCleanup)。
  setNowTick(Date.now());
  const timer = setInterval(() => setNowTick(Date.now()), 1000);
  onCleanup(() => clearInterval(timer));
});

// ── chat history 懒加载 ─────────────────────────────────────
export const [historyOldestTs, setHistoryOldestTs] = createSignal<string | null>(null);
export const [historyHasMore, setHistoryHasMore] = createSignal(true);
export const [historyLoading, setHistoryLoading] = createSignal(false);
// 当前 session 在当前 session 内已加载完毕，可加载上一个会话
export const [canLoadPrevSession, setCanLoadPrevSession] = createSignal(false);
export const [loadingSessionId, setLoadingSessionId] = createSignal<string | null>(null);
// 历史面板：会话列表
export const [sessionsList, setSessionsList] = createSignal<{ session_id: string; size_bytes: number }[]>([]);
// 当前活跃的引用块（可多个，从历史面板拖入）
export const [activeCitations, setActiveCitations] = createSignal<CitationBlock[]>([]);
import type { CitationBlock } from "./tauri-api";

// ── 项目系统 ─────────────────────────────────────────────────
export const [projects, setProjects] = createSignal<ProjectSummary[]>([]);
export const [currentProjectId, setCurrentProjectId] = createSignal<string | null>(null);
export const [projectDirectories, setProjectDirectories] = createSignal<ProjectDirectories | null>(null);
export const [projectDropdownOpen, setProjectDropdownOpen] = createSignal(false);

export const [projectCreateOpen, setProjectCreateOpen] = createSignal(false);
// 该新建项目面板是否由 Agent(gated hand_off)弹出并在等待:是 → 确认/取消后注入消息重新驱动 Agent;
// 用户手动从侧栏开的则为 false,不注入。见决策日志 2026-06-02。
export const [projectCreateFromAgent, setProjectCreateFromAgent] = createSignal(false);
export const [addPathOpen, setAddPathOpen] = createSignal(false);

export interface ProjectPrefill {
  id?: string;
  name?: string;
  writable?: string[];
  readonly?: string[];
  description?: string;
}
export interface AddPathPrefill {
  path?: string;
  kind?: "writable" | "readonly";
}
export const [projectPrefill, setProjectPrefill] = createSignal<ProjectPrefill | null>(null);
export const [addPathPrefill, setAddPathPrefill] = createSignal<AddPathPrefill | null>(null);

// ── 工具开关 ─────────────────────────────────────────────────
export interface ToolEntry {
  name: string;
  // 后端按 ui_lang 本地化的显示名(单一源 tools.i18n.json);name 仍是英文 key。
  label?: string;
  description: string;
  enabled: boolean;
}
export const [toolList, setToolList] = createSignal<ToolEntry[]>([]);

/// 工具的本地化显示名(来自后端单一源)。供 i18n.tTool 单一源取词;未加载时返回 undefined。
export function toolLabel(name: string): string | undefined {
  return toolList().find((t) => t.name === name)?.label;
}

// ── 工具调用块默认展开偏好 ────────────────────────────────────
// 纯前端展示决定(后端用不上):聊天里某工具的调用块默认折叠还是展开。
// 默认集 = ["ask_user"](提问类要让用户一眼看到问题/选项,否则折叠易错过)。
// 用户在 ToolsTab「显示」区逐工具勾选;markdown.ts 渲染工具块时读。localStorage 持久。
export const [expandedTools, setExpandedToolsRaw] = createSignal<string[]>(
  lsGetJson<string[]>(STORAGE.expandedTools, ["ask_user"])
);
/// 某工具的调用块是否默认展开(聊天渲染时查;非 ok 状态另由渲染逻辑强制展开)。
export function toolExpandDefault(name: string): boolean {
  return expandedTools().includes(name);
}
/// 勾选/取消某工具"默认展开";即时生效 + localStorage 持久(不重连)。
export function setToolExpanded(name: string, expanded: boolean): void {
  const cur = expandedTools();
  const next = expanded
    ? (cur.includes(name) ? cur : [...cur, name])
    : cur.filter((n) => n !== name);
  setExpandedToolsRaw(next);
  lsSet(STORAGE.expandedTools, JSON.stringify(next));
}

// ── 主动自检动效:正在核查的瞬态状态文案(来自后端 chat-status 事件;空=不在核查)──
export const [verifyStatus, setVerifyStatus] = createSignal("");

// ── 工具分类(聊天图标区分):工作流名 / MCP 外部工具名(由 get_tool_kinds 刷新)──
export const [workflowNames, setWorkflowNames] = createSignal<string[]>([]);
export const [mcpToolNames, setMcpToolNames] = createSignal<string[]>([]);
/// 一个可调用名属于哪类:workflow(工作流)/ mcp(外部 MCP 工具)/ tool(内置)。
export function toolKind(name: string): "workflow" | "mcp" | "tool" {
  if (workflowNames().includes(name)) return "workflow";
  if (mcpToolNames().includes(name)) return "mcp";
  return "tool";
}

// ── dir-access 闪烁：path → "write" | "read"，1.5s 后清除 ─────
export const [flashingPaths, setFlashingPaths] = createSignal<Record<string, "write" | "read">>({});

// ── Intent meta timeline（A 方案）：LLM 每回合末尾 emit 的元信息 ─────
export interface IntentEvent {
  ts: number;
  is_challenge: boolean;
  confidence: number;
  topic: string[];
}
export const [intentEvents, setIntentEvents] = createSignal<IntentEvent[]>([]);
export function pushIntentEvent(e: IntentEvent): void {
  setIntentEvents([...intentEvents().slice(-49), e]);
}

// ── jump-to-bottom 按钮一次性抑制（懒加载 prepend 时用）─────
let _suppressJumpButton = false;
export function suppressJumpButtonOnce(): void { _suppressJumpButton = true; }
export function consumeJumpButtonSuppress(): boolean {
  if (_suppressJumpButton) { _suppressJumpButton = false; return true; }
  return false;
}
