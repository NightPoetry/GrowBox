// 设置·高级 Tab:上下文预算(记忆置换) + 存储 + 超时与重试(shell/UI/任务退避 + 缓存预热/自关机权) + 工作流(分支日志)。
// 从原「智能体」Tab 拆出低层旋钮 + 从 Settings.tsx 内联的 advanced pane 收编,集中到这里。
import { type Component, Show } from "solid-js";
import { api } from "../../tauri-api";
import { t } from "../../i18n";
import {
  runtimeDir, setRuntimeDir,
  workingContextChars, setWorkingContextChars,
  recentRingChars, setRecentRingChars,
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
                {/* 上下文预算(记忆置换):工作区 + 最近 ring 字符。改完点「重连」生效。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <rect x="3" y="4" width="18" height="6" rx="1" />
                      <rect x="3" y="14" width="12" height="6" rx="1" />
                    </svg>
                    {t("contextBudget") || "上下文预算 (记忆置换)"}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("contextBudgetHint") || "工作区 = 检索按指针调入的记忆;最近 ring = 永远置于末尾的近期原文。单位为字符(token 的保守上界)。填 0 = 随模型自动推算。改完点下方「重连」生效。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("workingChars") || "工作区字符"}</label>
                    <input
                      type="number"
                      min="0"
                      step="1000"
                      placeholder={t("workingCharsPlaceholder") || "0 = 默认 48000"}
                      value={workingContextChars()}
                      onInput={(e) => setWorkingContextChars(e.currentTarget.value)}
                      style={{ flex: 1, "min-width": 0 }}
                    />
                  </div>
                  <div class="settings-field">
                    <label>{t("ringChars") || "最近 ring 字符"}</label>
                    <input
                      type="number"
                      min="0"
                      step="1000"
                      placeholder={t("ringCharsPlaceholder") || "0 = 默认 8000"}
                      value={recentRingChars()}
                      onInput={(e) => setRecentRingChars(e.currentTarget.value)}
                      style={{ flex: 1, "min-width": 0 }}
                    />
                  </div>
                </div>
                <div class="settings-section">
                  <h3>{t("storage") || "存储"}</h3>
                  <div class="settings-field">
                    <label>{t("runtimeDir") || "运行时目录"}</label>
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
                    {t("timeoutSetting") || "超时与重试"}
                  </h3>
                  <div class="settings-field">
                    <label>{t("shellApprovalTimeoutLabel") || "shell 批准超时(秒)"}</label>
                    <input type="number" min="5" step="30" placeholder="300" value={mcShellApproval()}
                      onInput={(e) => setMcShellApproval(e.currentTarget.value)} onChange={saveMiscConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("shellApprovalTimeoutHint") || "手动模式下,shell 批准弹窗等你裁决多久;超时按拒绝(安全侧)。默认 300(5 分钟)。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("uiAckTimeoutLabel") || "UI 回执超时(秒)"}</label>
                    <input type="number" min="1" step="1" placeholder="3" value={mcUiAck()}
                      onInput={(e) => setMcUiAck(e.currentTarget.value)} onChange={saveMiscConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("uiAckTimeoutHint") || "AI 操控界面(活的 IDE)等前端回执多久,超时判未生效。默认 3。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("backoffBaseLabel") || "任务退避基数(毫秒)"}</label>
                    <input type="number" min="100" step="500" placeholder="2000" value={mcBackoffBase()}
                      onInput={(e) => setMcBackoffBase(e.currentTarget.value)} onChange={saveMiscConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("backoffCapLabel") || "任务退避上限(毫秒)"}</label>
                    <input type="number" min="1000" step="5000" placeholder="60000" value={mcBackoffCap()}
                      onInput={(e) => setMcBackoffCap(e.currentTarget.value)} onChange={saveMiscConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 10px 0", "line-height": "1.5" }}>
                    {t("backoffHint") || "等待后台任务完成时的轮询退避(指数增长,从基数翻倍到上限)。默认 2000→60000。"}
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
                    {t("workflowSetting") || "工作流(分支)"}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("workflowSettingHint") || "派生分支(工作流 v2)不与你对话、不写主记忆,但全部调用信息原样存进项目日志,环形覆盖。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("branchLogLabel") || "分支日志上限(GB)"}</label>
                    <input type="number" min="-1" step="5" placeholder="25" value={mcBranchLogGb()}
                      onInput={(e) => setMcBranchLogGb(e.currentTarget.value)} onChange={saveMiscConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0", "line-height": "1.5" }}>
                    {t("branchLogHint") || "分支调用日志存项目 .growbox/branch.log,到此上限即轮替(旧的被覆盖)。默认 25;-1 = 无限制(慎用)。下回合生效。"}
                  </p>
                </div>
                {/* ★工具记忆 + 不犯第二遍★:分发前查「小本本」,已知不可行硬否决重试、已知失败软提醒。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <path d="M4 4h12l4 4v12H4zM8 4v6h8" /><path d="M8 14h8M8 17h5" />
                    </svg>
                    {t("toolMemorySetting") || "工具记忆 · 不犯第二遍"}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("toolMemoryHint") || "每个工具在每个项目有个「小本本」:AI 记下某工具在某情况下不可行/失败,之后调用前会自动会诊——已知不可行的高相似重试会被一票否决,已知失败的会提醒。关掉则完全不会诊(行为同以前)。"}
                  </p>
                  <div class="tool-toggle">
                    <div class="tool-toggle-info">
                      <div class="tool-toggle-name">{t("toolMemoryEnable") || "启用工具记忆"}</div>
                      <div class="tool-toggle-desc">
                        {tmEnabled() ? (t("toolMemoryOn") || "分发前会诊 + 不犯第二遍守卫") : (t("toolMemoryOff") || "关闭(不会诊不守卫)")}
                      </div>
                    </div>
                    <label class="toggle-switch">
                      <input type="checkbox" checked={tmEnabled()} onChange={(e) => { setTmEnabled(e.currentTarget.checked); saveToolMemoryConfig(); }} />
                      <span class="toggle-track" />
                    </label>
                  </div>
                  <Show when={tmEnabled()}>
                    <div class="settings-field">
                      <label>{t("toolMemoryVetoLabel") || "不可行·硬否决阈(0~1)"}</label>
                      <input type="number" min="0" max="1" step="0.01" placeholder="0.85" value={tmVeto()}
                        onInput={(e) => setTmVeto(e.currentTarget.value)} onChange={saveToolMemoryConfig}
                        style={{ flex: 1, "min-width": 0 }} />
                    </div>
                    <div class="settings-field">
                      <label>{t("toolMemoryWarnLabel") || "失败·软提醒阈(0~1)"}</label>
                      <input type="number" min="0" max="1" step="0.01" placeholder="0.80" value={tmWarn()}
                        onInput={(e) => setTmWarn(e.currentTarget.value)} onChange={saveToolMemoryConfig}
                        style={{ flex: 1, "min-width": 0 }} />
                    </div>
                    <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0", "line-height": "1.5" }}>
                      {t("toolMemoryFootHint") || "相似度越高越严。否决阈高(默认 0.85)= 只挡几乎相同的已知不可行;提醒阈(默认 0.80)略低。AI 经 note_tool_memory 记小本本;关键因素变了再记一条即可解除否决。"}
                    </p>
                  </Show>
                </div>
              </div>
);

export default AdvancedTab;
