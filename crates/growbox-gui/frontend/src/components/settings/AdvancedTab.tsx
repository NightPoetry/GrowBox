// 设置·高级 Tab:上下文预算(记忆置换) + 存储 + 超时与重试(shell/UI/任务退避 + 缓存预热/自关机权) + 工作流(分支日志)。
// 从原「智能体」Tab 拆出低层旋钮 + 从 Settings.tsx 内联的 advanced pane 收编,集中到这里。
import { type Component, Show } from "solid-js";
import { api } from "../../tauri-api";
import { t } from "../../i18n";
import {
  runtimeDir, setRuntimeDir,
  workingContextChars, setWorkingContextChars,
  recentRingChars, setRecentRingChars,
  theme, setTheme,
} from "../../store";
import {
  mcShellApproval, setMcShellApproval, mcUiAck, setMcUiAck,
  mcBackoffBase, setMcBackoffBase, mcBackoffCap, setMcBackoffCap,
  mcCachePrewarm, setMcCachePrewarm, saveMiscConfig,
  mcBranchLogGb, setMcBranchLogGb,
  tmEnabled, setTmEnabled, tmVeto, setTmVeto, tmWarn, setTmWarn, saveToolMemoryConfig,
} from "./state";

const AdvancedTab: Component = () => (
              <div class="settings-tab-pane active">
                {/* 外观:暗 / 亮(暖琥珀) / 跟随系统。即时生效、localStorage 持久;AI 也能用 set_appearance 工具切。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <circle cx="12" cy="12" r="5" />
                      <path d="M12 1v2M12 21v2M4.2 4.2l1.4 1.4M18.4 18.4l1.4 1.4M1 12h2M21 12h2M4.2 19.8l1.4-1.4M18.4 5.6l1.4-1.4" />
                    </svg>
                    {t("appearance")}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("appearanceHint")}
                  </p>
                  <div class="appearance-seg">
                    {/* 滑块:按选中项 translateX 平移过去(dark=0 / light=100% / auto=200%)。 */}
                    <div
                      class="appearance-seg-thumb"
                      style={{ transform: `translateX(${theme() === "light" ? 100 : theme() === "auto" ? 200 : 0}%)` }}
                    />
                    <button classList={{ active: theme() === "dark" }} onClick={() => setTheme("dark")}>{t("themeDark")}</button>
                    <button classList={{ active: theme() === "light" }} onClick={() => setTheme("light")}>{t("themeLight")}</button>
                    <button classList={{ active: theme() === "auto" }} onClick={() => setTheme("auto")}>{t("themeAuto")}</button>
                  </div>
                </div>
                {/* 上下文预算(记忆置换):工作区 + 最近 ring 字符。改完点「重连」生效。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <rect x="3" y="4" width="18" height="6" rx="1" />
                      <rect x="3" y="14" width="12" height="6" rx="1" />
                    </svg>
                    {t("contextBudget")}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("contextBudgetHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("workingChars")}</label>
                    <input
                      type="number"
                      min="0"
                      step="1000"
                      placeholder={t("workingCharsPlaceholder")}
                      value={workingContextChars()}
                      onInput={(e) => setWorkingContextChars(e.currentTarget.value)}
                      style={{ flex: 1, "min-width": 0 }}
                    />
                  </div>
                  <div class="settings-field">
                    <label>{t("ringChars")}</label>
                    <input
                      type="number"
                      min="0"
                      step="1000"
                      placeholder={t("ringCharsPlaceholder")}
                      value={recentRingChars()}
                      onInput={(e) => setRecentRingChars(e.currentTarget.value)}
                      style={{ flex: 1, "min-width": 0 }}
                    />
                  </div>
                </div>
                <div class="settings-section">
                  <h3>{t("storage")}</h3>
                  <div class="settings-field">
                    <label>{t("runtimeDir")}</label>
                    <input
                      id="set-runtime_dir"
                      value={runtimeDir()}
                      onInput={(e) => setRuntimeDir(e.currentTarget.value)}
                    />
                  </div>
                </div>
                {/* 超时与重试旋钮(推论9 数值全可设)。shell/UI 超时下回合生效;任务退避即时。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <circle cx="12" cy="12" r="9" /><path d="M12 7v5l3 2" />
                    </svg>
                    {t("timeoutSetting")}
                  </h3>
                  <div class="settings-field">
                    <label>{t("shellApprovalTimeoutLabel")}</label>
                    <input type="number" min="5" step="30" placeholder="300" value={mcShellApproval()}
                      onInput={(e) => setMcShellApproval(e.currentTarget.value)} onChange={saveMiscConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("shellApprovalTimeoutHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("uiAckTimeoutLabel")}</label>
                    <input type="number" min="1" step="1" placeholder="3" value={mcUiAck()}
                      onInput={(e) => setMcUiAck(e.currentTarget.value)} onChange={saveMiscConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("uiAckTimeoutHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("backoffBaseLabel")}</label>
                    <input type="number" min="100" step="500" placeholder="2000" value={mcBackoffBase()}
                      onInput={(e) => setMcBackoffBase(e.currentTarget.value)} onChange={saveMiscConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("backoffCapLabel")}</label>
                    <input type="number" min="1000" step="5000" placeholder="60000" value={mcBackoffCap()}
                      onInput={(e) => setMcBackoffCap(e.currentTarget.value)} onChange={saveMiscConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 10px 0", "line-height": "1.5" }}>
                    {t("backoffHint")}
                  </p>
                  {/* 缓存预热开关(优化造物首次回复慢)。默认开。 */}
                  <div class="tool-toggle">
                    <div class="tool-toggle-info">
                      <div class="tool-toggle-name">{t("cachePrewarm")}</div>
                      <div class="tool-toggle-desc">{t("cachePrewarmDesc")}</div>
                    </div>
                    <label class="toggle-switch">
                      <input type="checkbox" checked={mcCachePrewarm()}
                        onChange={(e) => { setMcCachePrewarm(e.currentTarget.checked); void api.setCachePrewarm(e.currentTarget.checked); }} />
                      <span class="toggle-track" />
                    </label>
                  </div>
                  {/* 自关机永久权已移至「安全·权限」tab 的权限清单(带状态徽章,集中管理)。 */}
                </div>
                {/* 工作流(栈函数 v2)旋钮:分支日志上限。下回合生效(AgentConfig 每回合重建 BranchLog)+ 落库。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <path d="M6 3v12M6 15a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9a9 9 0 0 1-9 9" />
                    </svg>
                    {t("workflowSetting")}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("workflowSettingHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("branchLogLabel")}</label>
                    <input type="number" min="-1" step="5" placeholder="25" value={mcBranchLogGb()}
                      onInput={(e) => setMcBranchLogGb(e.currentTarget.value)} onChange={saveMiscConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0", "line-height": "1.5" }}>
                    {t("branchLogHint")}
                  </p>
                </div>
                {/* ★工具记忆 + 不犯第二遍★:分发前查「小本本」,已知不可行硬否决重试、已知失败软提醒。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <path d="M4 4h12l4 4v12H4zM8 4v6h8" /><path d="M8 14h8M8 17h5" />
                    </svg>
                    {t("toolMemorySetting")}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("toolMemoryHint")}
                  </p>
                  <div class="tool-toggle">
                    <div class="tool-toggle-info">
                      <div class="tool-toggle-name">{t("toolMemoryEnable")}</div>
                      <div class="tool-toggle-desc">
                        {tmEnabled() ? (t("toolMemoryOn")) : (t("toolMemoryOff"))}
                      </div>
                    </div>
                    <label class="toggle-switch">
                      <input type="checkbox" checked={tmEnabled()} onChange={(e) => { setTmEnabled(e.currentTarget.checked); saveToolMemoryConfig(); }} />
                      <span class="toggle-track" />
                    </label>
                  </div>
                  <Show when={tmEnabled()}>
                    <div class="settings-field">
                      <label>{t("toolMemoryVetoLabel")}</label>
                      <input type="number" min="0" max="1" step="0.01" placeholder="0.85" value={tmVeto()}
                        onInput={(e) => setTmVeto(e.currentTarget.value)} onChange={saveToolMemoryConfig}
                        style={{ flex: 1, "min-width": 0 }} />
                    </div>
                    <div class="settings-field">
                      <label>{t("toolMemoryWarnLabel")}</label>
                      <input type="number" min="0" max="1" step="0.01" placeholder="0.80" value={tmWarn()}
                        onInput={(e) => setTmWarn(e.currentTarget.value)} onChange={saveToolMemoryConfig}
                        style={{ flex: 1, "min-width": 0 }} />
                    </div>
                    <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0", "line-height": "1.5" }}>
                      {t("toolMemoryFootHint")}
                    </p>
                  </Show>
                </div>
              </div>
);

export default AdvancedTab;
