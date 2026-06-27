import { Show, For, createEffect, createSignal, type Component } from "solid-js";
import { api } from "../tauri-api";
import { notify } from "../notices";
import { refreshTools, refreshToolKinds } from "../tools";
import { t } from "../i18n";
import { sfx } from "../sfx";
import ControlPanel from "./ControlPanel";
import ConnectionTab from "./settings/ConnectionTab";
import AgentTab from "./settings/AgentTab";
import ToolsTab from "./settings/ToolsTab";
import AdvancedTab from "./settings/AdvancedTab";
import {
  autoMode, setAutoMode, dangerMode, setDangerMode,
  settingsOpen, setSettingsOpen,
  settingsTab, setSettingsTab,
  appVersion,
} from "../store";
// 逻辑层(模块级配置信号 + 保存/扫描/连接动作)抽到 settings/state;此处导入面板回显 effect 用到的设值器。
import {
  setAgMaxTokenRetries, setAgTokenCeil, setAgSilenceSecs, setAgMaxStall, setAgParallelMax, setAgCompleteSilence, setAgReasoningEffort,
  setAgSelfVerify, setAgSelfVerifyMin, setAgRecallInLoop,
  setIdThreshold, setIdTick, setIdFatigue, setIdMaxSleep, setIdMaxRehearsals,
  setMcShellApproval, setMcUiAck, setMcBackoffBase, setMcBackoffCap, mcAutoShutdown, setMcAutoShutdown, setMcCachePrewarm, setMcBranchLogGb,
  setMcLazyTools, setMcDeferredList, loadMcpStatus,
  setTlRead, setTlList, setTlOutput, setTlOutlineSymbols, setTlTaskCap, setTlCtxWindow, setTlShellTimeout,
  loadTranspileStatus, loadTsserverStatus, tpBusy, loadSkills, loadSkillProposals, loadWebConfig, loadToolMemoryConfig,
} from "./settings/state";

// doConnect 仍由本模块对外导出(App.tsx 顶栏连接按钮 import 自此),实体在 settings/state。
export { doConnect } from "./settings/state";

const Settings: Component = () => {
  // 打开 settings + 切到 tools tab 时拉一次工具(界面语言变化时的重拉由 App 的全局 effect 统管)。
  createEffect(() => {
    if (settingsOpen() && settingsTab() === "tools") {
      void refreshTools();
    }
  });

  // 回显失败统一告知(每次打开至多一次):静默吞错会让用户把默认值当真值(虚假成功)。
  let recallWarned = false;
  createEffect(() => {
    if (!settingsOpen()) recallWarned = false;
  });
  function recallFailed(err: unknown): void {
    console.warn("[settings] 配置回显失败:", err);
    if (!recallWarned) {
      recallWarned = true;
      notify("settings.recall_failed");
    }
  }
  // 保存失败必告知:隐私目录等"用户以为已生效"的设置静默丢失是隐私/信任洞。
  function saveFailed(err: unknown): void {
    notify("settings.save_failed", { detail: String(err) });
  }

  // 隐私文件夹列表(命中必弹窗 + 二次确认)。打开设置时从后端拉。
  const [privacyDirs, setPrivacyDirsLocal] = createSignal<string[]>([]);
  createEffect(() => {
    if (settingsOpen()) void api.getPrivacyDirs().then(setPrivacyDirsLocal).catch(recallFailed);
  });
  // Agent 循环旋钮:打开设置时回显当前值。
  createEffect(() => {
    if (settingsOpen()) void api.getAgentConfig().then((c) => {
      setAgMaxTokenRetries(String(c.max_token_retries));
      setAgTokenCeil(String(c.token_ceil));
      setAgSilenceSecs(String(c.silence_secs));
      setAgMaxStall(String(c.max_stall));
      setAgParallelMax(String(c.parallel_max ?? 4));
      setAgCompleteSilence(String(c.complete_silence_secs));
      setAgReasoningEffort(c.reasoning_effort === "high" ? "high" : "max");
      setAgSelfVerify(c.self_verify !== false);
      setAgSelfVerifyMin(String(c.self_verify_min_tools ?? 3));
      setAgRecallInLoop(c.recall_in_loop !== false);
    }).catch(recallFailed);
  });
  // idle/睡眠旋钮:打开设置时回显当前值。
  createEffect(() => {
    if (settingsOpen()) void api.getIdleConfig().then((c) => {
      setIdThreshold(String(c.idle_threshold_secs));
      setIdTick(String(c.idle_tick_secs));
      setIdFatigue(String(c.idle_fatigue_threshold));
      setIdMaxSleep(String(c.idle_max_sleep_steps));
      setIdMaxRehearsals(String(c.idle_max_rehearsals));
    }).catch(recallFailed);
  });
  // 超时/退避旋钮:打开设置时回显当前值。
  createEffect(() => {
    if (settingsOpen()) void api.getMiscConfig().then((c) => {
      setMcShellApproval(String(c.shell_approval_timeout_secs));
      setMcUiAck(String(c.ui_ack_timeout_secs));
      setMcBackoffBase(String(c.task_backoff_base_ms));
      setMcBackoffCap(String(c.task_backoff_cap_ms));
      setMcAutoShutdown(!!c.auto_shutdown_allowed);
      setMcCachePrewarm(c.cache_prewarm !== false);
      setMcBranchLogGb(String(c.branch_log_max_gb));
      setMcLazyTools(!!c.lazy_tools);
      setMcDeferredList(Array.isArray(c.deferred_tools) ? c.deferred_tools : []);
    }).catch(recallFailed);
  });
  // ★二期 C1/D2★ 打开设置时:拉工具列表(懒加载逐项勾选用)+ MCP 配置/实时状态回显 + Skill 列表/旋钮 + Web 工具配置。
  createEffect(() => {
    if (settingsOpen()) { void refreshTools(); void refreshToolKinds(); void loadMcpStatus(); loadTranspileStatus(); loadTsserverStatus(); loadSkills(); loadSkillProposals(); loadWebConfig(); loadToolMemoryConfig(); }
  });
  // 工具输出上限旋钮:打开设置时回显当前值。
  createEffect(() => {
    if (settingsOpen()) void api.getToolLimits().then((c) => {
      setTlRead(String(c.max_read_bytes));
      setTlList(String(c.max_list_entries));
      setTlOutput(String(c.max_output_bytes));
      setTlOutlineSymbols(String(c.max_outline_symbols));
      setTlTaskCap(String(c.task_output_cap));
      setTlCtxWindow(String(c.context_window_tokens));
      setTlShellTimeout(String(c.shell_timeout_secs));
    }).catch(recallFailed);
  });
  async function addPrivacyDir() {
    const p = await api.pickDirectory().catch(() => null);
    if (!p || privacyDirs().includes(p)) return;
    const next = [...privacyDirs(), p];
    setPrivacyDirsLocal(next);
    void api.setPrivacyDirs(next).catch(saveFailed);
  }
  function removePrivacyDir(p: string) {
    const next = privacyDirs().filter((x) => x !== p);
    setPrivacyDirsLocal(next);
    void api.setPrivacyDirs(next).catch(saveFailed);
  }

  const handleBackdrop = (e: MouseEvent) => {
    // 重写提示词中:不准点空白退出(防误触);用 × 仍可关(后台重写不受影响)。
    if (tpBusy()) return;
    if ((e.target as HTMLElement).classList.contains("settings-backdrop")) {
      setSettingsOpen(false);
    }
  };

  return (
    <div
      class={`settings-overlay ${settingsOpen() ? "visible" : ""}`}
      onClick={handleBackdrop}
    >
      <div class="settings-backdrop" />
      <div class="settings-panel">
        <div class="settings-header">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
            <circle cx="12" cy="12" r="3" />
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
          </svg>
          <div class="title-group">
            <h2>GrowBox</h2>
            <Show when={appVersion()}><span class="ver">v{appVersion()}</span></Show>
          </div>
          <button class="settings-close" onClick={() => { sfx.tap(); setSettingsOpen(false); }}>×</button>
        </div>

        <div class="settings-main">
          <div class="settings-sidebar-nav">
            <button
              class={`settings-tab-btn ${settingsTab() === "connection" ? "active" : ""}`}
              onClick={() => { sfx.tap(); setSettingsTab("connection"); }}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M5 12h14M12 5l7 7-7 7" />
              </svg>
              <span>{t("connection") || "连接"}</span>
            </button>
            <button
              class={`settings-tab-btn ${settingsTab() === "agent" ? "active" : ""}`}
              onClick={() => { sfx.tap(); setSettingsTab("agent"); }}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M3 12a9 9 0 1 0 9-9" /><polyline points="3 4 3 9 8 9" />
              </svg>
              <span>{t("tabAgent")}</span>
            </button>
            <button
              class={`settings-tab-btn ${settingsTab() === "security" ? "active" : ""}`}
              onClick={() => { sfx.tap(); setSettingsTab("security"); }}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
              </svg>
              <span>{t("tabSecurity")}</span>
            </button>
            <button
              class={`settings-tab-btn ${settingsTab() === "advanced" ? "active" : ""}`}
              onClick={() => { sfx.tap(); setSettingsTab("advanced"); }}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
              </svg>
              <span>{t("tabAdvanced")}</span>
            </button>
            <button
              class={`settings-tab-btn ${settingsTab() === "tools" ? "active" : ""}`}
              onClick={() => { sfx.tap(); setSettingsTab("tools"); }}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />
              </svg>
              <span>{t("tools")}</span>
            </button>
            <button
              class={`settings-tab-btn ${settingsTab() === "control" ? "active" : ""}`}
              onClick={() => { sfx.tap(); setSettingsTab("control"); }}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <rect x="3" y="3" width="7" height="7" rx="1" />
                <rect x="14" y="3" width="7" height="7" rx="1" />
                <rect x="3" y="14" width="7" height="7" rx="1" />
                <rect x="14" y="14" width="7" height="7" rx="1" />
              </svg>
              <span>{t("settingsControlTab") || "控制面板"}</span>
            </button>
            <button
              class={`settings-tab-btn ${settingsTab() === "about" ? "active" : ""}`}
              onClick={() => { sfx.tap(); setSettingsTab("about"); }}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <circle cx="12" cy="12" r="10" />
                <line x1="12" y1="16" x2="12" y2="12" />
                <line x1="12" y1="8" x2="12.01" y2="8" />
              </svg>
              <span>{t("about")}</span>
            </button>
            <button
              class={`settings-tab-btn ${settingsTab() === "dlc" ? "active" : ""}`}
              onClick={() => { sfx.tap(); setSettingsTab("dlc"); }}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4" />
                <polyline points="17 8 12 3 7 8" />
                <line x1="12" y1="3" x2="12" y2="15" />
              </svg>
              <span>{t("dlcTab") || "DLC"}</span>
            </button>
          </div>

          <div class="settings-content">
            <Show when={settingsTab() === "connection"}>
              <ConnectionTab />
            </Show>

            <Show when={settingsTab() === "agent"}>
              <AgentTab />
            </Show>

            <Show when={settingsTab() === "security"}>
              <div class="settings-tab-pane active">
                {/* ★权限·授权清单★:通用授权集中一处,带状态徽章,可预先授权(避免如定时关机到点没授权而失败)。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                      <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" /><path d="M9 12l2 2 4-4" />
                    </svg>
                    {t("permissionsTitle")}
                  </h3>
                  <p class="settings-section-hint">{t("permissionsHint")}</p>
                  {/* 自动关机授权(高风险,默认关=每次弹一次性授权)。预先开 → 定时/自动关机到点不再因没授权失败。 */}
                  <div class="tool-toggle">
                    <div class="tool-toggle-info">
                      <div class="tool-toggle-name">
                        {t("autoShutdownAllowed")}
                        <span class={`perm-badge ${mcAutoShutdown() ? "granted" : "ungranted"}`}>
                          {mcAutoShutdown() ? t("permGranted") : t("permNotGranted")}
                        </span>
                      </div>
                      <div class="tool-toggle-desc">{t("autoShutdownAllowedDesc")}</div>
                    </div>
                    <label class="toggle-switch">
                      <input type="checkbox" checked={mcAutoShutdown()}
                        onChange={(e) => { setMcAutoShutdown(e.currentTarget.checked); void api.setAutoShutdownAllowed(e.currentTarget.checked); }} />
                      <span class="toggle-track" />
                    </label>
                  </div>
                  {/* ★疫苗式预接种系统授权★:点一下 spawn ShutdownHelper 探针 → 触发 macOS"控制 System Events"
                      弹窗,用户允许一次即永久 → 之后关机走自动化通道免 root(见 helpers.rs)。GrowBox 内部开关(上)
                      是一层,这是 OS 层授权。 */}
                  <div class="tool-toggle">
                    <div class="tool-toggle-info">
                      <div class="tool-toggle-name">{t("vaccinateShutdownTitle")}</div>
                      <div class="tool-toggle-desc">{t("vaccinateShutdownDesc")}</div>
                    </div>
                    <button class="privacy-dir-add" onClick={() => void api.vaccinatePermission("shutdown").catch(saveFailed)}>
                      {t("vaccinateBtn")}
                    </button>
                  </div>
                </div>
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                      <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
                    </svg>
                    {t("autoModeLabel")}
                  </h3>
                  <div class="tool-toggle">
                    <div class="tool-toggle-info">
                      <div class="tool-toggle-name">
                        {t("autoModeLabel")}
                        <span class={`perm-badge ${autoMode() ? "granted" : "ungranted"}`}>
                          {autoMode() ? t("permGranted") : t("permNotGranted")}
                        </span>
                      </div>
                      <div class="tool-toggle-desc">
                        {autoMode() ? t("autoModeOn") : t("autoModeOff")}
                      </div>
                    </div>
                    <label class="toggle-switch">
                      <input
                        type="checkbox"
                        checked={autoMode()}
                        onChange={(e) => { const v = e.currentTarget.checked; setAutoMode(v); void api.setAutoMode(v).catch(saveFailed); }}
                      />
                      <span class="toggle-track" />
                    </label>
                  </div>
                  <p class="settings-section-hint">{t("autoModeHint")}</p>
                  {/* ★danger 模式(为所欲为)★:全自动之上的最高放行档。仅在全自动开启时才显示(它是 auto 的升级)。
                      强红警告;会话级不持久(关 app 自动复位)。 */}
                  <Show when={autoMode()}>
                    <div class={`tool-toggle danger-toggle${dangerMode() ? " on" : ""}`}>
                      <div class="tool-toggle-info">
                        <div class="tool-toggle-name">
                          {t("dangerModeLabel")}
                          <span class={`perm-badge ${dangerMode() ? "danger-on" : "ungranted"}`}>
                            {dangerMode() ? t("dangerModeOnBadge") : t("permNotGranted")}
                          </span>
                        </div>
                        <div class="tool-toggle-desc danger-desc">{t("dangerModeDesc")}</div>
                      </div>
                      <label class="toggle-switch">
                        <input
                          type="checkbox"
                          checked={dangerMode()}
                          onChange={(e) => { const v = e.currentTarget.checked; setDangerMode(v); void api.setDangerMode(v).catch(saveFailed); }}
                        />
                        <span class="toggle-track" />
                      </label>
                    </div>
                    <p class="settings-section-hint danger-hint">{t("dangerModeWarn")}</p>
                  </Show>
                </div>
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                      <rect x="3" y="11" width="18" height="11" rx="2" /><path d="M7 11V7a5 5 0 0 1 10 0v4" />
                    </svg>
                    {t("privacyDirsTitle")}
                  </h3>
                  <p class="settings-section-hint">{t("privacyDirsHint")}</p>
                  <For each={privacyDirs()}>
                    {(p) => (
                      <div class="pd-row privacy-dir-row" title={p}>
                        <span class="pd-path">{p}</span>
                        <button class="pd-del" onClick={() => removePrivacyDir(p)}>×</button>
                      </div>
                    )}
                  </For>
                  <button class="privacy-dir-add" onClick={() => void addPrivacyDir()}>+ {t("privacyDirsAdd")}</button>
                </div>
              </div>
            </Show>

            <Show when={settingsTab() === "advanced"}>
              <AdvancedTab />
            </Show>

            <Show when={settingsTab() === "tools"}>
              <ToolsTab />
            </Show>

            <Show when={settingsTab() === "control"}>
              <div class="settings-tab-pane active">
                <div class="settings-section settings-control-pane">
                  <h3>{t("settingsControlTab") || "控制面板"}</h3>
                  <p class="settings-section-hint">
                    {t("settingsControlHint") || "事件历史与底层状态检视。需持续监控的实时信号请用顶栏「系统健康监控」。"}
                  </p>
                  <Show when={settingsOpen()}>
                    <ControlPanel />
                  </Show>
                </div>
              </div>
            </Show>

            <Show when={settingsTab() === "about"}>
              <div class="settings-tab-pane active">
                <div class="settings-section">
                  <h3>GrowBox</h3>
                  <p style={{ "font-size": "12px", color: "var(--text-secondary)", "line-height": "1.7" }}>
                    {t("aboutDesc") ||
                      "持久学习型 AI Agent。一次授权全权运行，跨会话经验复用，100% 可溯源审计，上下文永不压缩。"}
                  </p>
                </div>
              </div>
            </Show>
          </div>
        </div>

        <div class="settings-footer">{t("copyright")}</div>
      </div>
    </div>
  );
};

export default Settings;
