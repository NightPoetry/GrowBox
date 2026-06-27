// 设置·智能体 Tab:Agent 循环旋钮(轮数/token/超时/重试)+ 自我负责(主动自检 + 提示词自转译)+ 后台维护(idle/睡眠)。
// 工具相关(懒加载/MCP/输出上限/显示)拆到「工具」Tab(ToolsTab);低层旋钮(上下文/存储/超时退避/工作流分支)拆到「高级」Tab(AdvancedTab)。
import { type Component, Show, For } from "solid-js";
import {
  maxTurns, setMaxTurns, maxTokens, setMaxTokens,
  selfDriveIdleLimit, setSelfDriveIdleLimit, selfDriveGapSecs, setSelfDriveGapSecs,
  selfDriveMaxRounds, setSelfDriveMaxRounds, selfDriveDigestEvery, setSelfDriveDigestEvery,
} from "../../store";
import { t } from "../../i18n";
import {
  agMaxTokenRetries, setAgMaxTokenRetries, agTokenCeil, setAgTokenCeil,
  agSilenceSecs, setAgSilenceSecs, agMaxStall, setAgMaxStall, agParallelMax, setAgParallelMax,
  agCompleteSilence, setAgCompleteSilence, agReasoningEffort, setAgReasoningEffort, saveAgentConfig,
  agSelfVerify, setAgSelfVerify, agSelfVerifyMin, setAgSelfVerifyMin,
  agRecallInLoop, setAgRecallInLoop,
  tpCount, tpBusy, tpProgress, tpResult, tpError, runTranspile,
  tpConcurrency, setTpConcurrency, saveTranspileConcurrency,
  tpSnapshots, tpActive, activateSnapshot, renameSnapshot, deleteSnapshot,
  tpExportMsg, exportSnapshot, importSnapshotFile,
  idThreshold, setIdThreshold, idTick, setIdTick, idFatigue, setIdFatigue,
  idMaxSleep, setIdMaxSleep, idMaxRehearsals, setIdMaxRehearsals, saveIdleConfig,
} from "./state";

const AgentTab: Component = () => (
              <div class="settings-tab-pane active">
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <path d="M3 12a9 9 0 1 0 9-9" />
                      <polyline points="3 4 3 9 8 9" />
                    </svg>
                    {t("agentLoopSetting")}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("agentLoopHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("maxTurnsLabel")}</label>
                    <input
                      id="set-max_turns"
                      type="number"
                      min="0"
                      step="50"
                      placeholder="1000"
                      value={maxTurns()}
                      onInput={(e) => setMaxTurns(e.currentTarget.value)}
                      style={{ flex: 1, "min-width": 0 }}
                    />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("maxTurnsHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("maxTokensLabel")}</label>
                    <input
                      id="set-max_tokens"
                      type="number"
                      min="0"
                      step="1024"
                      placeholder="0"
                      value={maxTokens()}
                      onInput={(e) => setMaxTokens(e.currentTarget.value)}
                      style={{ flex: 1, "min-width": 0 }}
                    />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 10px 0", "line-height": "1.5" }}>
                    {t("maxTokensHint")}
                  </p>
                  {/* ★自驱续跑旋钮(用户铁律:一切数值都可设)★:全自动模式下「一直跑」循环按钮的参数,纯前端、即时生效。 */}
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "10px 0 6px 0", "font-weight": 600 }}>
                    {t("selfDriveSectionLabel")}
                  </p>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("selfDriveSectionHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("selfDriveIdleLimitLabel")}</label>
                    <input
                      id="set-self_drive_idle_limit"
                      type="number"
                      min="1"
                      step="1"
                      placeholder="2"
                      value={selfDriveIdleLimit()}
                      onInput={(e) => setSelfDriveIdleLimit(e.currentTarget.value)}
                      style={{ flex: 1, "min-width": 0 }}
                    />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("selfDriveIdleLimitHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("selfDriveGapLabel")}</label>
                    <input
                      id="set-self_drive_gap_secs"
                      type="number"
                      min="0"
                      step="0.5"
                      placeholder="1.5"
                      value={selfDriveGapSecs()}
                      onInput={(e) => setSelfDriveGapSecs(e.currentTarget.value)}
                      style={{ flex: 1, "min-width": 0 }}
                    />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("selfDriveGapHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("selfDriveMaxRoundsLabel")}</label>
                    <input
                      id="set-self_drive_max_rounds"
                      type="number"
                      min="0"
                      step="10"
                      placeholder="0"
                      value={selfDriveMaxRounds()}
                      onInput={(e) => setSelfDriveMaxRounds(e.currentTarget.value)}
                      style={{ flex: 1, "min-width": 0 }}
                    />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("selfDriveMaxRoundsHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("selfDriveDigestEveryLabel")}</label>
                    <input
                      id="set-self_drive_digest_every"
                      type="number"
                      min="0"
                      step="1"
                      placeholder="12"
                      value={selfDriveDigestEvery()}
                      onInput={(e) => setSelfDriveDigestEvery(e.currentTarget.value)}
                      style={{ flex: 1, "min-width": 0 }}
                    />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 10px 0", "line-height": "1.5" }}>
                    {t("selfDriveDigestEveryHint")}
                  </p>
                  {/* Agent 高级旋钮(超时/重试/空转;推论9 数值全可设)。前 4 项改即下回合生效;complete 超时下次重连生效。 */}
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 6px 0", "font-weight": 600 }}>
                    {t("agentAdvancedLabel")}
                  </p>
                  <div class="settings-field">
                    <label>{t("reasoningEffortLabel")}</label>
                    <select value={agReasoningEffort()}
                      onChange={(e) => { setAgReasoningEffort(e.currentTarget.value); saveAgentConfig(); }}
                      style={{ flex: 1, "min-width": 0 }}>
                      <option value="high">{t("reasoningEffortHigh")}</option>
                      <option value="max">{t("reasoningEffortMax")}</option>
                    </select>
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("reasoningEffortHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("agentSilenceLabel")}</label>
                    <input type="number" min="5" step="10" placeholder="90" value={agSilenceSecs()}
                      onInput={(e) => setAgSilenceSecs(e.currentTarget.value)} onChange={saveAgentConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("agentSilenceHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("agentCompleteSilenceLabel")}</label>
                    <input type="number" min="5" step="10" placeholder="60" value={agCompleteSilence()}
                      onInput={(e) => setAgCompleteSilence(e.currentTarget.value)} onChange={saveAgentConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("agentCompleteSilenceHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("agentMaxStallLabel")}</label>
                    <input type="number" min="2" step="1" placeholder="2" value={agMaxStall()}
                      onInput={(e) => setAgMaxStall(e.currentTarget.value)} onChange={saveAgentConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("agentMaxStallHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("agentParallelMaxLabel")}</label>
                    <input type="number" min="1" step="1" placeholder="4" value={agParallelMax()}
                      onInput={(e) => setAgParallelMax(e.currentTarget.value)} onChange={saveAgentConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("agentParallelMaxHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("agentMaxRetriesLabel")}</label>
                    <input type="number" min="0" step="1" placeholder="2" value={agMaxTokenRetries()}
                      onInput={(e) => setAgMaxTokenRetries(e.currentTarget.value)} onChange={saveAgentConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("agentTokenCeilLabel")}</label>
                    <input type="number" min="1024" step="1024" placeholder="32768" value={agTokenCeil()}
                      onInput={(e) => setAgTokenCeil(e.currentTarget.value)} onChange={saveAgentConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0", "line-height": "1.5" }}>
                    {t("agentRetriesHint")}
                  </p>
                </div>

                {/* ★回合内补检索(用户决策:回合内重跑检索)★:开场只按进场用户消息检索一次;任务跑到一半才需要的记忆(如开始 SSH 才要的凭据),每轮据 AI 当下进展再检索一次,新命中增量注入。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <circle cx="11" cy="11" r="7" />
                      <path d="M21 21l-4.3-4.3" />
                    </svg>
                    {t("recallInLoopLabel")}
                  </h3>
                  <div class="tool-toggle">
                    <div class="tool-toggle-info">
                      <div class="tool-toggle-desc">{t("recallInLoopDesc")}</div>
                    </div>
                    <label class="toggle-switch">
                      <input type="checkbox" checked={agRecallInLoop()}
                        onChange={(e) => { setAgRecallInLoop(e.currentTarget.checked); saveAgentConfig(); }} />
                      <span class="toggle-track" />
                    </label>
                  </div>
                </div>

                {/* ★自我负责(设计/08)★:输出侧=主动自检(收尾前重读文件核对) + 输入侧=提示词自转译(用当前模型按自己风格重写提示词)。两者同源、都增成本,故都做成开关。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <path d="M9 11l3 3L22 4M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
                    </svg>
                    {t("selfResponsibilitySetting")}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("selfResponsibilityHint")}
                  </p>
                  {/* 输出侧:主动自检(grounded verification) */}
                  <div class="tool-toggle">
                    <div class="tool-toggle-info">
                      <div class="tool-toggle-name">{t("selfVerifyLabel")}</div>
                      <div class="tool-toggle-desc">{t("selfVerifyDesc")}</div>
                    </div>
                    <label class="toggle-switch">
                      <input type="checkbox" checked={agSelfVerify()}
                        onChange={(e) => { setAgSelfVerify(e.currentTarget.checked); saveAgentConfig(); }} />
                      <span class="toggle-track" />
                    </label>
                  </div>
                  <Show when={agSelfVerify()}>
                    <div class="settings-field">
                      <label>{t("selfVerifyMinLabel")}</label>
                      <input type="number" min="1" step="1" placeholder="3" value={agSelfVerifyMin()}
                        onInput={(e) => setAgSelfVerifyMin(e.currentTarget.value)} onChange={saveAgentConfig}
                        style={{ flex: 1, "min-width": 0 }} />
                    </div>
                    <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0", "line-height": "1.5" }}>
                      {t("selfVerifyMinHint")}
                    </p>
                  </Show>
                  {/* ── 输入侧:提示词自转译(decoder 自亲和)──。无开关:用「当前提示词」选区选包,默认(原文)即关。 */}
                  <div style={{ "margin-top": "12px", "border-top": "1px solid var(--separator)", "padding-top": "12px" }}>
                    <div class="tool-toggle-name" style={{ "margin-bottom": "4px" }}>{t("transpileLabel")}</div>
                    <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 10px 0", "line-height": "1.5" }}>
                      {t("transpileDesc2")}
                    </p>
                    {/* 当前提示词:选区(默认原文 + 各历史包)。选了即激活、即时生效。 */}
                    <div class="settings-field">
                      <label style={{ "white-space": "nowrap" }}>{t("transpileCurrent")}</label>
                      <select value={tpActive()} onChange={(e) => activateSnapshot(e.currentTarget.value)} style={{ flex: 1, "min-width": 0 }}>
                        <option value="default">{t("transpileDefaultName")}</option>
                        <For each={tpSnapshots()}>
                          {(snap) => <option value={snap.id}>{snap.name}（{snap.model || "—"} · {snap.count}）</option>}
                        </For>
                      </select>
                    </div>
                    {/* 生成新包:重写按钮(带图标)+ 并发数。 */}
                    <div class="settings-field" style={{ "align-items": "center", gap: "8px" }}>
                      <button class="settings-btn" disabled={tpBusy()} onClick={() => runTranspile()}
                        style={{ flex: 1, "min-width": 0, display: "inline-flex", "align-items": "center", "justify-content": "center", gap: "6px" }}>
                        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                          <path d="M12 3v3m0 12v3M5.6 5.6l2.1 2.1m8.6 8.6l2.1 2.1M3 12h3m12 0h3M5.6 18.4l2.1-2.1m8.6-8.6l2.1-2.1" />
                        </svg>
                        {tpBusy()
                          ? (t("transpileRunning")) + (tpProgress() && tpProgress()!.total > 0 ? ` ${tpProgress()!.done}/${tpProgress()!.total}` : "")
                          : (t("transpileRun2"))}
                      </button>
                      <label style={{ "white-space": "nowrap", "font-size": "11px", color: "var(--text-secondary)" }}>{t("transpileConcurrency")}</label>
                      <input type="number" min="1" max="32" step="1" value={tpConcurrency()}
                        onInput={(e) => setTpConcurrency(e.currentTarget.value)} onChange={saveTranspileConcurrency}
                        style={{ width: "56px", "min-width": 0 }} />
                    </div>
                    <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "2px 0 0 0", "line-height": "1.5" }}>
                      {t("transpileStatus")}: {tpCount()}
                      <Show when={tpResult()}>
                        {" · "}{t("transpileLastRun")}: {tpResult()!.written}/{tpResult()!.total}
                        {tpResult()!.skipped > 0 ? ` (${t("transpileSkipped")} ${tpResult()!.skipped})` : ""}
                      </Show>
                      <Show when={tpError()}>
                        <span style={{ color: "var(--danger, #e55)" }}>{" · "}{tpError()}</span>
                      </Show>
                    </p>
                    <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "2px 0 8px 0", "line-height": "1.5" }}>
                      {t("transpileCostHint")}
                    </p>
                    {/* 历史版本管理(可后悔):改名 / 导出 / 删除;切换用上面的选区。默认(原文)只导出、删不掉。 */}
                    <Show when={tpSnapshots().length > 0}>
                      <p class="settings-section-hint" style={{ margin: "4px 0 6px", "font-weight": 600 }}>
                        {t("transpileManage")}
                      </p>
                      <div class="settings-list">
                        <For each={tpSnapshots()}>
                          {(snap) => (
                            <div class="tool-toggle">
                              <div class="tool-toggle-info">
                                <input value={snap.name} title={t("transpileRename")}
                                  onChange={(e) => renameSnapshot(snap.id, e.currentTarget.value)}
                                  style={{ "font-weight": 600, "font-size": "12px", color: "var(--text-primary)", background: "var(--bg-input, transparent)", border: "1px solid var(--separator)", "border-radius": "4px", padding: "2px 6px", width: "100%" }} />
                                <div class="tool-toggle-desc">
                                  {snap.model || "—"} · {snap.count} {t("transpileItems")}
                                  <Show when={tpActive() === snap.id}><span style={{ color: "var(--green)", "margin-left": "6px" }}>· {t("transpileActive")}</span></Show>
                                </div>
                              </div>
                              <div class="mcp-row-ctl">
                                <button class="settings-btn" style={{ "min-width": 0, padding: "3px 10px" }} title={t("transpileExport")} onClick={() => exportSnapshot(snap.id)}>
                                  {t("transpileExport")}
                                </button>
                                <button class="mcp-remove" title={t("transpileDelete")} onClick={() => deleteSnapshot(snap.id)}>✕</button>
                              </div>
                            </div>
                          )}
                        </For>
                      </div>
                    </Show>
                    {/* 导入 .zip 成为新版本。 */}
                    <div class="settings-field" style={{ "margin-top": "8px", "align-items": "center", gap: "8px" }}>
                      <label style={{ "white-space": "nowrap" }}>{t("transpileImport")}</label>
                      <input type="file" accept=".zip"
                        onChange={(e) => { const f = e.currentTarget.files?.[0]; if (f) { importSnapshotFile(f); e.currentTarget.value = ""; } }}
                        style={{ flex: 1, "min-width": 0, "font-size": "11px" }} />
                    </div>
                    <Show when={tpExportMsg()}>
                      <p class="settings-section-hint" style={{ "word-break": "break-all", "margin-top": "4px" }}>{tpExportMsg()}</p>
                    </Show>
                  </div>
                </div>

                {/* 后台维护(idle/做梦/睡眠)旋钮(推论9 数值全可设)。IdleWorker 下一拍重读生效。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9z" />
                    </svg>
                    {t("idleSetting")}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("idleSettingHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("idleThresholdLabel")}</label>
                    <input type="number" min="10" step="30" placeholder="480" value={idThreshold()}
                      onInput={(e) => setIdThreshold(e.currentTarget.value)} onChange={saveIdleConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("idleThresholdHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("idleTickLabel")}</label>
                    <input type="number" min="5" step="10" placeholder="30" value={idTick()}
                      onInput={(e) => setIdTick(e.currentTarget.value)} onChange={saveIdleConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("idleFatigueLabel")}</label>
                    <input type="number" min="0" max="1" step="0.05" placeholder="0.5" value={idFatigue()}
                      onInput={(e) => setIdFatigue(e.currentTarget.value)} onChange={saveIdleConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("idleFatigueHint")}
                  </p>
                  <div class="settings-field">
                    <label>{t("idleMaxSleepLabel")}</label>
                    <input type="number" min="1" step="1" placeholder="16" value={idMaxSleep()}
                      onInput={(e) => setIdMaxSleep(e.currentTarget.value)} onChange={saveIdleConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("idleMaxRehearsalsLabel")}</label>
                    <input type="number" min="0" step="1" placeholder="4" value={idMaxRehearsals()}
                      onInput={(e) => setIdMaxRehearsals(e.currentTarget.value)} onChange={saveIdleConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0", "line-height": "1.5" }}>
                    {t("idleStepsHint")}
                  </p>
                </div>
              </div>
);

export default AgentTab;
