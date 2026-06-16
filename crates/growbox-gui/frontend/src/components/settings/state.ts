// Settings 逻辑层:模块级配置信号 + 保存函数 + 扫描/连接动作。
// 从 Settings.tsx 抽出(视图与状态分离);各 Tab 子组件与 Settings 外壳共享这些模块级 signal。

import { createSignal } from "solid-js";
import { api, listen } from "../../tauri-api";
import { notify } from "../../notices";
import { sfx } from "../../sfx";
import { refreshProjects, refreshProjectDirs } from "../../projects";
import { refreshToolKinds } from "../../tools";
import {
  apiBase, model, setModel, apiKey, runtimeDir,
  supervisorModel, supervisorApiBase, supervisorApiKey,
  maxTurns, maxTokens, workingContextChars, recentRingChars,
  embedRemote, embedApiBase, embedApiKey, embedModel,
  subconsciousModel, subconsciousApiBase, subconsciousApiKey,
  setSessionId, setConnected, setConfigDirty, setSettingsOpen, setConnecting,
} from "../../store";

// 扫描扫到的模型 ID 列表。空数组 = 还没扫描或扫描失败 → 模型字段降级为 input 让用户手动输入。
export const [modelOptions, setModelOptions] = createSignal<string[]>([]);
export const [scanning, setScanning] = createSignal(false);

// Agent 循环旋钮(自含:面板打开时 getAgentConfig 回显,改即 setAgentConfig 落库;推论9 数值全可设)。
// 前 4 项下回合生效;complete 沉默超时下次重连生效。空串=保持当前(不强制填)。
export const [agMaxTokenRetries, setAgMaxTokenRetries] = createSignal("");
export const [agTokenCeil, setAgTokenCeil] = createSignal("");
export const [agSilenceSecs, setAgSilenceSecs] = createSignal("");
export const [agMaxStall, setAgMaxStall] = createSignal("");
export const [agCompleteSilence, setAgCompleteSilence] = createSignal("");
export const [agReasoningEffort, setAgReasoningEffort] = createSignal("high");
// ★主动自检★开关 + 触发阈值(本次任务工具调用数 ≥ 阈值才自检)。
export const [agSelfVerify, setAgSelfVerify] = createSignal(true);
export const [agSelfVerifyMin, setAgSelfVerifyMin] = createSignal("");

// ★提示词自转译(自我负责-输入侧)★:开关 + 覆盖条数 + 重写进度。
// 开关即时落库;「用当前模型重写提示词」是长任务(每条一次重写+一次复核 LLM 调用),进度走事件。
export const [tpEnabled, setTpEnabled] = createSignal(false);
export const [tpCount, setTpCount] = createSignal(0);
export const [tpBusy, setTpBusy] = createSignal(false);
export const [tpConcurrency, setTpConcurrency] = createSignal("4");
export const [tpProgress, setTpProgress] = createSignal<{ done: number; total: number; key: string } | null>(null);
// 上次重写结果(写入/跳过/总数)+ 是否出错,面板内联展示(不走 notices catalog,免散装守卫)。
export const [tpResult, setTpResult] = createSignal<{ total: number; written: number; skipped: number } | null>(null);
export const [tpError, setTpError] = createSignal("");

// ★历史提示词★版本库:版本列表(新→旧)+ 当前激活 id(default = 原文)。
export type TpSnapshot = { id: string; name: string; model: string; count: number; created_ms: number };
export const [tpSnapshots, setTpSnapshots] = createSignal<TpSnapshot[]>([]);
export const [tpActive, setTpActive] = createSignal("default");

// 面板打开时回显自转译状态 + 历史版本。
export function loadTranspileStatus(): void {
  void api.getTranspileStatus().then((s) => {
    setTpEnabled(!!s.enabled);
    setTpCount(s.override_count ?? 0);
    if (typeof s.concurrency === "number") setTpConcurrency(String(s.concurrency));
  }).catch(() => {});
  loadTranspileSnapshots();
}

// 设转译并发数(落库,下次重写即用)。
export function saveTranspileConcurrency(): void {
  const n = Math.max(1, parseInt(tpConcurrency(), 10) || 4);
  void api.setTranspileConcurrency(n).catch(() => {});
}

// 拉取历史提示词列表 + 当前激活版。
export function loadTranspileSnapshots(): void {
  void api.transpileListSnapshots().then((r) => {
    setTpSnapshots(Array.isArray(r.snapshots) ? r.snapshots : []);
    setTpActive(r.active || "default");
  }).catch(() => {});
}

// 激活某历史版本(id=default 还原原文)。即时生效 + 刷新覆盖条数/列表。
export function activateSnapshot(id: string): void {
  void api.transpileActivateSnapshot(id).then(() => {
    setTpActive(id);
    loadTranspileStatus();
  }).catch(() => {});
}

// 重命名历史版本(default 不可改)。
export function renameSnapshot(id: string, name: string): void {
  const n = name.trim();
  if (!n || id === "default") return;
  void api.transpileRenameSnapshot(id, n).then(() => loadTranspileSnapshots()).catch(() => {});
}

// 删除历史版本(default 拒删;删激活版回落原文)。
export function deleteSnapshot(id: string): void {
  if (id === "default") return;
  void api.transpileDeleteSnapshot(id).then(() => loadTranspileStatus()).catch(() => {});
}

// 导出/导入磁盘 .zip:最近导出路径 + 导入结果/错误,面板内联展示。
export const [tpExportMsg, setTpExportMsg] = createSignal("");

// 导出某版本成 .zip(写到数据目录),回填路径。
export function exportSnapshot(id: string): void {
  setTpExportMsg("");
  void api.transpileExportSnapshot(id)
    .then((path) => setTpExportMsg(path))
    .catch((e) => setTpExportMsg(String(e)));
}

// File → base64(无第三方依赖,逐字节 btoa)。
async function fileToB64(file: File): Promise<string> {
  const buf = await file.arrayBuffer();
  const bytes = new Uint8Array(buf);
  let binary = "";
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

// 导入 .zip 文件成新版本(置激活),刷新列表。
export function importSnapshotFile(file: File): void {
  setTpExportMsg("");
  const name = file.name.replace(/\.zip$/i, "");
  void fileToB64(file)
    .then((b64) => api.transpileImportSnapshot(name, b64))
    .then((r) => { setTpExportMsg(`✓ ${r.name} (${r.count})`); loadTranspileStatus(); })
    .catch((e) => setTpExportMsg(String(e)));
}

// 翻自转译开关(即时生效 + 落库)。
export function setTranspileEnabled(on: boolean): void {
  setTpEnabled(on);
  void api.setTranspileEnabled(on).catch(() => {});
}

// 「用当前模型重写提示词」:订阅进度事件 + 跑转译;完成后刷新覆盖条数 + 记结果。
export function runTranspile(): void {
  if (tpBusy()) return;
  setTpBusy(true);
  setTpError("");
  setTpResult(null);
  setTpProgress({ done: 0, total: 0, key: "" });
  let unlisten: (() => void) | null = null;
  void listen<{ done: number; total: number; key?: string; finished?: boolean }>("transpile-progress", (p) => {
    setTpProgress({ done: p.done ?? 0, total: p.total ?? 0, key: p.key ?? "" });
  }).then((u) => { unlisten = u; });
  void api.transpilePrompts()
    .then((r) => { setTpResult(r); })
    .catch((e) => { setTpError(String(e)); })
    .finally(() => {
      setTpBusy(false);
      setTpProgress(null);
      if (unlisten) unlisten();
      loadTranspileStatus();
    });
}

export function saveAgentConfig(): void {
  const n = (s: string, d: number) => { const v = parseInt(s, 10); return Number.isNaN(v) || v < 0 ? d : v; };
  void api.setAgentConfig({
    maxTokenRetries: n(agMaxTokenRetries(), 2),
    tokenCeil: n(agTokenCeil(), 32768),
    silenceSecs: n(agSilenceSecs(), 90),
    maxStall: n(agMaxStall(), 2),
    completeSilenceSecs: n(agCompleteSilence(), 60),
    reasoningEffort: agReasoningEffort() === "high" ? "high" : "max",
    selfVerify: agSelfVerify(),
    selfVerifyMinTools: Math.max(1, n(agSelfVerifyMin(), 3)),
  });
}

// idle/做梦/睡眠旋钮(自含:面板打开时 getIdleConfig 回显,改即 setIdleConfig 落库;IdleWorker 下一拍生效)。
export const [idThreshold, setIdThreshold] = createSignal("");
export const [idTick, setIdTick] = createSignal("");
export const [idFatigue, setIdFatigue] = createSignal("");
export const [idMaxSleep, setIdMaxSleep] = createSignal("");
export const [idMaxRehearsals, setIdMaxRehearsals] = createSignal("");

export function saveIdleConfig(): void {
  const ni = (s: string, d: number) => { const v = parseInt(s, 10); return Number.isNaN(v) || v < 1 ? d : v; };
  const nf = (s: string, d: number) => { const v = parseFloat(s); return Number.isNaN(v) || v < 0 ? d : v; };
  void api.setIdleConfig({
    idleThresholdSecs: ni(idThreshold(), 480),
    idleTickSecs: ni(idTick(), 30),
    idleFatigueThreshold: nf(idFatigue(), 0.5),
    idleMaxSleepSteps: ni(idMaxSleep(), 16),
    idleMaxRehearsals: ni(idMaxRehearsals(), 4),
  });
}

// 超时/退避旋钮(自含:面板打开时 getMiscConfig 回显,改即 setMiscConfig 落库)。
export const [mcShellApproval, setMcShellApproval] = createSignal("");
export const [mcUiAck, setMcUiAck] = createSignal("");
export const [mcBackoffBase, setMcBackoffBase] = createSignal("");
export const [mcBackoffCap, setMcBackoffCap] = createSignal("");
// 自关机永久权 + 缓存预热开关(回显自 getMiscConfig;改即各自命令落库)。
export const [mcAutoShutdown, setMcAutoShutdown] = createSignal(false);
export const [mcCachePrewarm, setMcCachePrewarm] = createSignal(true);
// 分支日志上限(GB,工作流 v2;回显自 getMiscConfig)。空串=保持默认 25;-1=无限制。
export const [mcBranchLogGb, setMcBranchLogGb] = createSignal("");
// ★二期 C1★ 工具懒加载:总开关 + deferred 名单(从真实工具列表逐项勾选,即时落库)。
// 名单 = 字符串数组(被勾"懒加载"的工具名);改即 setLazyToolsConfig 即时生效 + 落库。
export const [mcLazyTools, setMcLazyTools] = createSignal(false);
export const [mcDeferredList, setMcDeferredList] = createSignal<string[]>([]);
// 永不可 deferred(后端 NEVER_DEFER 强制常驻;前端从可选列表里隐藏)。
export const NEVER_DEFER = ["finish", "ask_user", "workflow_return", "tool_search"];

export function isDeferred(name: string): boolean {
  return mcDeferredList().includes(name);
}
export function toggleDeferred(name: string, on: boolean): void {
  const cur = mcDeferredList().filter((n) => n !== name);
  setMcDeferredList(on ? [...cur, name] : cur);
  saveLazyToolsConfig();
}
export function saveLazyToolsConfig(): void {
  void api.setLazyToolsConfig({ lazyTools: mcLazyTools(), deferredTools: mcDeferredList() });
}

// ★二期 D2★ MCP server 连接管理:结构化配置数组(每条一个 server)+ 每 server 实时状态 + 错误 + 添加表单。
// 回显自 mcpGetStatus;增删改任一项即 mcpSetServers(落库 + 全量重连),回写状态。
export const [mcMcpServers, setMcMcpServers] = createSignal<any[]>([]);
export const [mcMcpStatus, setMcMcpStatus] = createSignal<any[]>([]);
export const [mcMcpError, setMcMcpError] = createSignal("");
export const [mcMcpBusy, setMcMcpBusy] = createSignal(false);
// 添加表单字段。
export const [mcNewName, setMcNewName] = createSignal("");
export const [mcNewCmd, setMcNewCmd] = createSignal("");
export const [mcNewArgs, setMcNewArgs] = createSignal("");
export const [mcNewEnv, setMcNewEnv] = createSignal("");
// 传输方式:stdio(默认,spawn 命令)或 http(连 URL,Streamable HTTP)。
export const [mcNewTransport, setMcNewTransport] = createSignal("stdio");
export const [mcNewUrl, setMcNewUrl] = createSignal("");

/// 取某 server 的实时状态(连接/工具数/错误)。
export function mcpStatusOf(name: string): any | undefined {
  return mcMcpStatus().find((s) => s && s.name === name);
}

// 把当前 mcMcpServers 落库 + 全量重连,回写每 server 状态。
async function pushMcpServers(): Promise<void> {
  setMcMcpError("");
  setMcMcpBusy(true);
  try {
    const res: any = await api.mcpSetServers({ servers: mcMcpServers() });
    setMcMcpStatus(res?.servers || []);
    void refreshToolKinds(); // MCP 工具集变了 → 刷新分类,聊天里 MCP 工具用"包"图标
  } catch (e: any) {
    setMcMcpError(String(e?.message || e));
  } finally {
    setMcMcpBusy(false);
  }
}

export async function addMcpServer(): Promise<void> {
  const name = mcNewName().trim();
  const transport = mcNewTransport();
  const isHttp = transport === "http";
  const command = mcNewCmd().trim();
  const url = mcNewUrl().trim();
  if (!name) { setMcMcpError("名称不能为空"); return; }
  if (isHttp && !url) { setMcMcpError("HTTP 传输需填 URL"); return; }
  if (!isHttp && !command) { setMcMcpError("stdio 传输需填命令"); return; }
  if (mcMcpServers().some((s) => s.name === name)) { setMcMcpError(`已存在同名 server「${name}」`); return; }
  const args = mcNewArgs().trim() ? mcNewArgs().trim().split(/\s+/) : [];
  const env: Record<string, string> = {};
  for (const pair of mcNewEnv().trim().split(/\s+/).filter(Boolean)) {
    const i = pair.indexOf("=");
    if (i > 0) env[pair.slice(0, i)] = pair.slice(i + 1);
  }
  setMcMcpServers([...mcMcpServers(), { name, command, args, env, enabled: true, transport, url }]);
  setMcNewName(""); setMcNewCmd(""); setMcNewArgs(""); setMcNewEnv(""); setMcNewUrl(""); setMcNewTransport("stdio");
  await pushMcpServers();
}

export async function removeMcpServer(name: string): Promise<void> {
  setMcMcpServers(mcMcpServers().filter((s) => s.name !== name));
  await pushMcpServers();
}

export async function toggleMcpServer(name: string, enabled: boolean): Promise<void> {
  setMcMcpServers(mcMcpServers().map((s) => (s.name === name ? { ...s, enabled } : s)));
  await pushMcpServers();
}

export async function loadMcpStatus(): Promise<void> {
  try {
    const res: any = await api.mcpGetStatus();
    setMcMcpServers(Array.isArray(res?.configs) ? res.configs : []);
    setMcMcpStatus(res?.servers || []);
  } catch {
    /* 未连接/无 MCP:忽略 */
  }
}

export function saveMiscConfig(): void {
  const ni = (s: string, d: number) => { const v = parseInt(s, 10); return Number.isNaN(v) || v < 1 ? d : v; };
  // 分支日志是浮点 GB,允许 -1(无限制)与 0+,故单独解析(不用 >=1 的 ni)。
  const nGb = (s: string, d: number) => { const v = parseFloat(s); return Number.isNaN(v) ? d : (v < 0 ? -1 : v); };
  void api.setMiscConfig({
    shellApprovalTimeoutSecs: ni(mcShellApproval(), 300),
    uiAckTimeoutSecs: ni(mcUiAck(), 3),
    taskBackoffBaseMs: ni(mcBackoffBase(), 2000),
    taskBackoffCapMs: ni(mcBackoffCap(), 60000),
    branchLogMaxGb: nGb(mcBranchLogGb(), 25),
  });
}

// 工具输出上限旋钮(自含:面板打开时 getToolLimits 回显,改即 setToolLimits 落库)。
export const [tlRead, setTlRead] = createSignal("");
export const [tlList, setTlList] = createSignal("");
export const [tlOutput, setTlOutput] = createSignal("");
export const [tlOutlineSymbols, setTlOutlineSymbols] = createSignal("");
export const [tlTaskCap, setTlTaskCap] = createSignal("");
// 上下文窗口总量(token):面板"实时上下文压力"分母,随模型设(默认 256000)。
export const [tlCtxWindow, setTlCtxWindow] = createSignal("");
// shell 命令超时秒(0=不限,慎用):防命令永不返回致工具挂死 + 终止失效(默认 60)。
export const [tlShellTimeout, setTlShellTimeout] = createSignal("");

// ★Web 工具(web_fetch/web_search)★:搜索 provider/端点/key + 条数 + 超时。
// 自含:打开设置回显(getWebConfig),改即落库 + 即时生效(setWebConfig)。
export const [webProvider, setWebProvider] = createSignal("");
export const [webApiBase, setWebApiBase] = createSignal("");
export const [webApiKey, setWebApiKey] = createSignal("");
export const [webMaxResults, setWebMaxResults] = createSignal("5");
export const [webTimeout, setWebTimeout] = createSignal("30");

export function loadWebConfig(): void {
  void api.getWebConfig().then((c) => {
    setWebProvider(c.provider ?? "");
    setWebApiBase(c.api_base ?? "");
    setWebApiKey(c.api_key ?? "");
    setWebMaxResults(String(c.max_results ?? 5));
    setWebTimeout(String(c.timeout_secs ?? 30));
  }).catch(() => { /* 未连接:忽略 */ });
}

export function saveWebConfig(): void {
  const n = parseInt(webMaxResults(), 10);
  const mr = Number.isNaN(n) || n < 1 ? 5 : Math.min(n, 10);
  // 超时允许 0(=不限,慎用):非法→默认 30,合法(含 0)原样。
  const tv = parseInt(webTimeout(), 10);
  const ts = Number.isNaN(tv) || tv < 0 ? 30 : tv;
  void api.setWebConfig(webProvider(), webApiBase().trim(), webApiKey().trim(), mr, ts)
    .catch((e) => notify("settings.save_failed", { detail: String(e) }));
}

// ★工具记忆 + 不犯第二遍(计划/工具记忆-不犯第二遍)★:总开关 + 两个相似度阈。
// 打开设置回显;改即落库 + 即时生效。
export const [tmEnabled, setTmEnabled] = createSignal(true);
export const [tmVeto, setTmVeto] = createSignal("0.85");
export const [tmWarn, setTmWarn] = createSignal("0.80");

export function loadToolMemoryConfig(): void {
  void api.getToolMemoryConfig().then((c) => {
    setTmEnabled(c.enabled !== false);
    setTmVeto(String(c.veto_threshold ?? 0.85));
    setTmWarn(String(c.warn_threshold ?? 0.8));
  }).catch(() => { /* 未连接:忽略 */ });
}

export function saveToolMemoryConfig(): void {
  const clamp = (s: string, d: number) => { const v = parseFloat(s); return Number.isNaN(v) || v < 0 || v > 1 ? d : v; };
  void api.setToolMemoryConfig(tmEnabled(), clamp(tmVeto(), 0.85), clamp(tmWarn(), 0.8))
    .catch((e) => notify("settings.save_failed", { detail: String(e) }));
}

// ★TS/JS 语言服务器装配★状态 + 动作。
export const [tsInstalled, setTsInstalled] = createSignal(false);
export const [tsNpm, setTsNpm] = createSignal(true);
export const [tsBusy, setTsBusy] = createSignal(false);
export const [tsMsg, setTsMsg] = createSignal("");

// ★Skill 系统(设计/09)★:全部 skill 列表(内置+已学)+ 总开关 + 清单上限 + 展开看正文。
export interface SkillRow { name: string; trigger: string; source: string; active: boolean }
export const [skillList, setSkillList] = createSignal<SkillRow[]>([]);
export const [skillEnabled, setSkillEnabled] = createSignal(true);
export const [skillListMax, setSkillListMax] = createSignal("24");
export const [skillAutoload, setSkillAutoload] = createSignal("0.88");
export const [skillExpanded, setSkillExpanded] = createSignal<string | null>(null);
export const [skillBody, setSkillBody] = createSignal("");

/// 打开设置(工具 tab)时拉 skill 列表 + 旋钮回显。
export function loadSkills(): void {
  void api.listSkills().then(setSkillList).catch(() => { /* 未连接:忽略 */ });
  void api.getSkillConfig().then((c) => {
    setSkillEnabled(!!c.enabled);
    setSkillListMax(String(c.list_max ?? 24));
    setSkillAutoload(String(c.autoload_threshold ?? 0.88));
  }).catch(() => {});
}

/// 停用/启用某 skill → 即时落库 + 重拉列表(刷新 active 态)。
export function toggleSkill(name: string, active: boolean): void {
  void api.setSkillActive(name, active).then(loadSkills).catch(() => {});
}

/// 展开/收起某 skill 看正文(懒取)。
export function toggleSkillBody(name: string): void {
  if (skillExpanded() === name) { setSkillExpanded(null); setSkillBody(""); return; }
  setSkillExpanded(name);
  setSkillBody("");
  void api.getSkillBody(name).then(setSkillBody).catch(() => setSkillBody("(读取失败)"));
}

// ★Skill 提议(S3)★:idle 飞轮起草的待裁决提议。打开设置时拉 + memory-event 推送时刷新。
export interface SkillProposalRow { id: string; name: string; trigger: string; body: string; rationale: string; created_ms: number }
export const [skillProposals, setSkillProposals] = createSignal<SkillProposalRow[]>([]);
export const [skillProposalExpanded, setSkillProposalExpanded] = createSignal<string | null>(null);

export function loadSkillProposals(): void {
  void api.listSkillProposals().then(setSkillProposals).catch(() => { /* 未连接/无:忽略 */ });
}

/// 采纳一条提议 → 结晶成 skill,刷新提议列表 + skill 列表(新 skill 出现)。
export function acceptSkillProposal(id: string): void {
  void api.acceptSkillProposal(id).then(() => { loadSkillProposals(); loadSkills(); }).catch((e) => notify("settings.save_failed", { detail: String(e) }));
}

/// 丢弃一条提议 → 出队 + 不再提。
export function rejectSkillProposal(id: string): void {
  void api.rejectSkillProposal(id).then(loadSkillProposals).catch((e) => notify("settings.save_failed", { detail: String(e) }));
}

/// 保存 Skill 总开关 + 清单上限 + 自动加载阈(改即落库 + 即时生效)。
export function saveSkillConfig(): void {
  const n = parseInt(skillListMax(), 10);
  const lm = Number.isNaN(n) || n < 1 ? 24 : n;
  const t = parseFloat(skillAutoload());
  const th = Number.isNaN(t) || t < 0 || t > 1 ? 0.88 : t;
  void api.setSkillConfig(skillEnabled(), lm, th).catch(() => {});
}

export function loadTsserverStatus(): void {
  void api.tsserverStatus().then((s) => { setTsInstalled(!!s.installed); setTsNpm(!!s.npm); }).catch(() => {});
}

export function installTsserver(): void {
  if (tsBusy()) return;
  setTsBusy(true);
  setTsMsg("");
  void api.installTsserver()
    .then((path) => { setTsMsg(`✓ ${path}`); loadTsserverStatus(); })
    .catch((e) => setTsMsg(String(e)))
    .finally(() => setTsBusy(false));
}

export function saveToolLimits(): void {
  const ni = (s: string, d: number) => { const v = parseInt(s, 10); return Number.isNaN(v) || v < 1 ? d : v; };
  // shell 超时允许 0(=不限),故用单独 parse:非法→默认 60,合法(含 0)原样。
  const nzero = (s: string, d: number) => { const v = parseInt(s, 10); return Number.isNaN(v) || v < 0 ? d : v; };
  void api.setToolLimits({
    maxReadBytes: ni(tlRead(), 204800),
    maxListEntries: ni(tlList(), 500),
    maxOutputBytes: ni(tlOutput(), 65536),
    maxOutlineSymbols: ni(tlOutlineSymbols(), 400),
    taskOutputCap: ni(tlTaskCap(), 4096),
    contextWindowTokens: ni(tlCtxWindow(), 256000),
    shellTimeoutSecs: nzero(tlShellTimeout(), 60),
  });
}

export async function doScanModels() {
  if (scanning()) return;
  const base = apiBase().trim();
  if (!base) {
    notify("connect.api_base_empty");
    return;
  }
  setScanning(true);
  try {
    const list = await api.listModels(base, apiKey() || undefined);
    setModelOptions(list);
    if (list.length === 0) {
      notify("connect.scan_empty");
    } else {
      notify("connect.scan_found", { n: list.length });
      // 当前 model 不在列表里 → 自动切到第一个，避免用户连接时 MODEL_NOT_FOUND
      if (!list.includes(model())) {
        setModel(list[0]);
      }
    }
  } catch (e) {
    const err = String(e);
    if (err.includes("API_UNREACHABLE")) {
      notify("connect.api_unreachable");
    } else if (err.includes("API_AUTH_FAILED")) {
      notify("connect.api_auth_failed");
    } else if (err.includes("API_BAD_RESPONSE")) {
      notify("connect.api_bad_response");
    } else {
      notify("connect.scan_failed", { detail: err });
    }
  } finally {
    setScanning(false);
  }
}

let connectGeneration = 0;
let connectLock = false;

export async function doConnect() {
  if (connectLock) {
    connectGeneration++;
    connectLock = false;
    setConnecting(false);
    notify("connect.cancelled");
    return;
  }
  connectLock = true;
  setConnecting(true);
  const gen = ++connectGeneration;
  try {
    const mt = parseInt(maxTurns(), 10);
    const mtk = parseInt(maxTokens(), 10);
    const wcc = parseInt(workingContextChars(), 10);
    const rrc = parseInt(recentRingChars(), 10);
    const sid = await api.connect({
      apiBase: apiBase(),
      model: model(),
      apiKey: apiKey() || undefined,
      runtimeDir: runtimeDir(),
      maxTurns: Number.isNaN(mt) ? undefined : mt,
      maxTokens: Number.isNaN(mtk) ? undefined : mtk,
      supervisorModel: supervisorModel() || undefined,
      supervisorApiBase: supervisorApiBase() || undefined,
      supervisorApiKey: supervisorApiKey() || undefined,
      embedRemote: embedRemote(),
      embedApiBase: embedApiBase() || undefined,
      embedApiKey: embedApiKey() || undefined,
      embedModel: embedModel() || undefined,
      // 总是传(含空串):空 = 清空 = 复用主模型(允许从"独立"切回"复用")。
      subconsciousModel: subconsciousModel(),
      subconsciousApiBase: subconsciousApiBase(),
      subconsciousApiKey: subconsciousApiKey(),
      workingContextChars: Number.isNaN(wcc) ? undefined : wcc,
      recentRingChars: Number.isNaN(rrc) ? undefined : rrc,
    });
    if (gen !== connectGeneration) return;
    setSessionId(sid);
    setConnected(true);
    setConfigDirty(false);
    sfx.connected();
    notify("connect.connected", { model: model() });
    setSettingsOpen(false);
    void refreshProjects();
    void refreshProjectDirs();
    // 历史不在此加载——右侧 HistoryDrawer 独立浏览,主聊天区起始空白。
  } catch (e) {
    if (gen !== connectGeneration) return;
    sfx.error();
    const err = String(e);
    if (err.startsWith("MODEL_NOT_LOADED:")) {
      const m = err.split(":")[1] ?? "";
      notify("connect.model_not_loaded", { model: m });
    } else if (err.startsWith("MODEL_NOT_FOUND:")) {
      const m = err.split(":")[1] ?? "";
      notify("connect.model_not_found", { model: m });
    } else if (err.startsWith("MODEL_ERROR:")) {
      notify("connect.model_error", { detail: err.slice("MODEL_ERROR:".length) });
    } else if (err.startsWith("API_UNREACHABLE:")) {
      notify("connect.api_unreachable");
    } else if (err.startsWith("API_AUTH_FAILED:")) {
      notify("connect.api_auth_failed");
    } else {
      notify("connect.failed", { detail: err });
    }
  } finally {
    if (gen === connectGeneration) {
      connectLock = false;
      setConnecting(false);
    }
  }
}
