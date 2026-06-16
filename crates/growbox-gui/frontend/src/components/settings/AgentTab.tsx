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
  agSilenceSecs, setAgSilenceSecs, agMaxStall, setAgMaxStall,
  agCompleteSilence, setAgCompleteSilence, agReasoningEffort, setAgReasoningEffort, saveAgentConfig,
  agSelfVerify, setAgSelfVerify, agSelfVerifyMin, setAgSelfVerifyMin,
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
                    {t("agentLoopSetting") || "Agent 循环"}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("agentLoopHint") || "长任务能跑多少步、单步能写多长。改完点下方「重连」生效。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("maxTurnsLabel") || "最大轮数"}</label>
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
                    {t("maxTurnsHint") || "一轮 = 一次模型调用 + 工具结果回填。默认 1000;填 0 = 无限模式(只在完成/空转/出错时收口)。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("maxTokensLabel") || "单轮 token"}</label>
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
                    {t("maxTokensHint") || "单次模型输出上限。0 = 不限,由模型自然停(推荐;flash 是推理模型,给死上限易把工具调用截断)。"}
                  </p>
                  {/* ★自驱续跑旋钮(用户铁律:一切数值都可设)★:全自动模式下「一直跑」循环按钮的参数,纯前端、即时生效。 */}
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "10px 0 6px 0", "font-weight": 600 }}>
                    {t("selfDriveSectionLabel") || "自驱续跑"}
                  </p>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("selfDriveSectionHint") || "全自动模式下「一直跑」循环按钮的参数。即时生效,无需重连。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("selfDriveIdleLimitLabel") || "空转暂停轮数"}</label>
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
                    {t("selfDriveIdleLimitHint") || "连续多少轮 AI 没动手(只回话)就判定没事可做、自动暂停续跑。默认 2,最小 1。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("selfDriveGapLabel") || "每轮间隔(秒)"}</label>
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
                    {t("selfDriveGapHint") || "每轮之间的喘息,给界面刷新、后台学习/整理缝隙,也降低 API 速率。默认 3;0 = 不间断。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("selfDriveMaxRoundsLabel") || "总轮数上限"}</label>
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
                    {t("selfDriveMaxRoundsHint") || "自驱跑满这么多轮就暂停、交还给你确认。默认 0 = 无限(靠进度指纹兜底防失控)。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("selfDriveDigestEveryLabel") || "每隔几轮消化"}</label>
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
                    {t("selfDriveDigestEveryHint") || "每隔这么多轮主动跑一次「飞轮消化」(把积压经验压缩成知识),防持续自驱时学习被饿死。默认 12;0 = 不主动消化。"}
                  </p>
                  {/* Agent 高级旋钮(超时/重试/空转;推论9 数值全可设)。前 4 项改即下回合生效;complete 超时下次重连生效。 */}
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 6px 0", "font-weight": 600 }}>
                    {t("agentAdvancedLabel") || "高级:超时与韧性"}
                  </p>
                  <div class="settings-field">
                    <label>{t("reasoningEffortLabel") || "思考强度"}</label>
                    <select value={agReasoningEffort()}
                      onChange={(e) => { setAgReasoningEffort(e.currentTarget.value); saveAgentConfig(); }}
                      style={{ flex: 1, "min-width": 0 }}>
                      <option value="high">{t("reasoningEffortHigh") || "high(快 · 推荐)"}</option>
                      <option value="max">{t("reasoningEffortMax") || "max(最深 · 慢,复杂推理用)"}</option>
                    </select>
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("reasoningEffortHint") || "deepseek V4 思考强度。max=想得最透(GrowBox 是 agent,默认 max);high=更快省 token。下回合生效。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("agentSilenceLabel") || "沉默超时(秒)"}</label>
                    <input type="number" min="5" step="10" placeholder="90" value={agSilenceSecs()}
                      onInput={(e) => setAgSilenceSecs(e.currentTarget.value)} onChange={saveAgentConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("agentSilenceHint") || "流式响应多久无任何输出(含思考)判超时。长任务/慢模型可调大。默认 90。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("agentCompleteSilenceLabel") || "判断/蒸馏超时(秒)"}</label>
                    <input type="number" min="5" step="10" placeholder="60" value={agCompleteSilence()}
                      onInput={(e) => setAgCompleteSilence(e.currentTarget.value)} onChange={saveAgentConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("agentCompleteSilenceHint") || "后台判断/经验蒸馏的沉默超时(流卡住时多久收手降级)。改后需重连生效。默认 60。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("agentMaxStallLabel") || "退化重复上限"}</label>
                    <input type="number" min="2" step="1" placeholder="2" value={agMaxStall()}
                      onInput={(e) => setAgMaxStall(e.currentTarget.value)} onChange={saveAgentConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("agentMaxStallHint") || "思考免死:只有连续多少轮产出近乎全等(真高频重复死循环)才收口。默认 2。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("agentMaxRetriesLabel") || "截断重试次数"}</label>
                    <input type="number" min="0" step="1" placeholder="2" value={agMaxTokenRetries()}
                      onInput={(e) => setAgMaxTokenRetries(e.currentTarget.value)} onChange={saveAgentConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("agentTokenCeilLabel") || "重试 token 上限"}</label>
                    <input type="number" min="1024" step="1024" placeholder="32768" value={agTokenCeil()}
                      onInput={(e) => setAgTokenCeil(e.currentTarget.value)} onChange={saveAgentConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0", "line-height": "1.5" }}>
                    {t("agentRetriesHint") || "工具调用被 token 截断时,翻倍 token 重试的次数与上限。默认 2 次 / 32768。"}
                  </p>
                </div>

                {/* ★自我负责(设计/08)★:输出侧=主动自检(收尾前重读文件核对) + 输入侧=提示词自转译(用当前模型按自己风格重写提示词)。两者同源、都增成本,故都做成开关。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <path d="M9 11l3 3L22 4M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
                    </svg>
                    {t("selfResponsibilitySetting") || "自我负责(自检 + 自转译)"}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("selfResponsibilityHint") || "让模型对自己的输出与输入负责:输出侧重读真实证据核对结论,输入侧把提示词改写成自己最顺的话。都可关、可还原。"}
                  </p>
                  {/* 输出侧:主动自检(grounded verification) */}
                  <div class="tool-toggle">
                    <div class="tool-toggle-info">
                      <div class="tool-toggle-name">{t("selfVerifyLabel") || "完成后主动自检"}</div>
                      <div class="tool-toggle-desc">{t("selfVerifyDesc") || "收尾前重读相关文件核对结论、改正证据不支持的说法、标注无法验证项,再收口。提升准确性、防过度声称/幻觉;关掉省 token。"}</div>
                    </div>
                    <label class="toggle-switch">
                      <input type="checkbox" checked={agSelfVerify()}
                        onChange={(e) => { setAgSelfVerify(e.currentTarget.checked); saveAgentConfig(); }} />
                      <span class="toggle-track" />
                    </label>
                  </div>
                  <Show when={agSelfVerify()}>
                    <div class="settings-field">
                      <label>{t("selfVerifyMinLabel") || "自检触发阈值"}</label>
                      <input type="number" min="1" step="1" placeholder="3" value={agSelfVerifyMin()}
                        onInput={(e) => setAgSelfVerifyMin(e.currentTarget.value)} onChange={saveAgentConfig}
                        style={{ flex: 1, "min-width": 0 }} />
                    </div>
                    <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0", "line-height": "1.5" }}>
                      {t("selfVerifyMinHint") || "本次任务工具调用数 ≥ 此值才自检(轻任务/纯问答不花这个钱)。默认 3。"}
                    </p>
                  </Show>
                  {/* ── 输入侧:提示词自转译(decoder 自亲和)──。无开关:用「当前提示词」选区选包,默认(原文)即关。 */}
                  <div style={{ "margin-top": "12px", "border-top": "1px solid var(--separator)", "padding-top": "12px" }}>
                    <div class="tool-toggle-name" style={{ "margin-bottom": "4px" }}>{t("transpileLabel") || "提示词自转译"}</div>
                    <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 10px 0", "line-height": "1.5" }}>
                      {t("transpileDesc2") || "用当前模型把喂给它的提示词(系统提示/工具说明/自检/judge·distill)按它自己的风格重写——模型最能执行自己写的话。下方选「当前提示词」用哪一包,选「默认(原文)」即不转译。"}
                    </p>
                    {/* 当前提示词:选区(默认原文 + 各历史包)。选了即激活、即时生效。 */}
                    <div class="settings-field">
                      <label style={{ "white-space": "nowrap" }}>{t("transpileCurrent") || "当前提示词"}</label>
                      <select value={tpActive()} onChange={(e) => activateSnapshot(e.currentTarget.value)} style={{ flex: 1, "min-width": 0 }}>
                        <option value="default">{t("transpileDefaultName") || "默认(原文)"}</option>
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
                          ? (t("transpileRunning") || "正在重写…") + (tpProgress() && tpProgress()!.total > 0 ? ` ${tpProgress()!.done}/${tpProgress()!.total}` : "")
                          : (t("transpileRun2") || "用当前模型生成新提示词包")}
                      </button>
                      <label style={{ "white-space": "nowrap", "font-size": "11px", color: "var(--text-secondary)" }}>{t("transpileConcurrency") || "并发"}</label>
                      <input type="number" min="1" max="32" step="1" value={tpConcurrency()}
                        onInput={(e) => setTpConcurrency(e.currentTarget.value)} onChange={saveTranspileConcurrency}
                        style={{ width: "56px", "min-width": 0 }} />
                    </div>
                    <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "2px 0 0 0", "line-height": "1.5" }}>
                      {t("transpileStatus") || "已有覆盖条数"}: {tpCount()}
                      <Show when={tpResult()}>
                        {" · "}{t("transpileLastRun") || "上次"}: {tpResult()!.written}/{tpResult()!.total}
                        {tpResult()!.skipped > 0 ? ` (${t("transpileSkipped") || "跳过"} ${tpResult()!.skipped})` : ""}
                      </Show>
                      <Show when={tpError()}>
                        <span style={{ color: "var(--danger, #e55)" }}>{" · "}{tpError()}</span>
                      </Show>
                    </p>
                    <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "2px 0 8px 0", "line-height": "1.5" }}>
                      {t("transpileCostHint") || "重写是长任务(每条提示词一次重写+一次保真复核 LLM 调用,成本高),按需触发即可,不必每次启动都跑。换模型后建议重跑。原文永不丢、随时可关还原。"}
                    </p>
                    {/* 历史版本管理(可后悔):改名 / 导出 / 删除;切换用上面的选区。默认(原文)只导出、删不掉。 */}
                    <Show when={tpSnapshots().length > 0}>
                      <p class="settings-section-hint" style={{ margin: "4px 0 6px", "font-weight": 600 }}>
                        {t("transpileManage") || "历史版本(改名 / 导出 / 删除)"}
                      </p>
                      <div class="settings-list">
                        <For each={tpSnapshots()}>
                          {(snap) => (
                            <div class="tool-toggle">
                              <div class="tool-toggle-info">
                                <input value={snap.name} title={t("transpileRename") || "改名"}
                                  onChange={(e) => renameSnapshot(snap.id, e.currentTarget.value)}
                                  style={{ "font-weight": 600, "font-size": "12px", color: "var(--text-primary)", background: "var(--bg-input, transparent)", border: "1px solid var(--separator)", "border-radius": "4px", padding: "2px 6px", width: "100%" }} />
                                <div class="tool-toggle-desc">
                                  {snap.model || "—"} · {snap.count} {t("transpileItems") || "条"}
                                  <Show when={tpActive() === snap.id}><span style={{ color: "var(--green)", "margin-left": "6px" }}>· {t("transpileActive") || "当前"}</span></Show>
                                </div>
                              </div>
                              <div class="mcp-row-ctl">
                                <button class="settings-btn" style={{ "min-width": 0, padding: "3px 10px" }} title={t("transpileExport") || "导出 .zip"} onClick={() => exportSnapshot(snap.id)}>
                                  {t("transpileExport") || "导出"}
                                </button>
                                <button class="mcp-remove" title={t("transpileDelete") || "删除"} onClick={() => deleteSnapshot(snap.id)}>✕</button>
                              </div>
                            </div>
                          )}
                        </For>
                      </div>
                    </Show>
                    {/* 导入 .zip 成为新版本。 */}
                    <div class="settings-field" style={{ "margin-top": "8px", "align-items": "center", gap: "8px" }}>
                      <label style={{ "white-space": "nowrap" }}>{t("transpileImport") || "导入 .zip"}</label>
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
                    {t("idleSetting") || "后台维护(做梦/睡眠)"}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("idleSettingHint") || "你离开后,AI 才在后台做梦整理记忆、提炼经验。下面控制何时开始、做多久。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("idleThresholdLabel") || "进入空闲(秒)"}</label>
                    <input type="number" min="10" step="30" placeholder="480" value={idThreshold()}
                      onInput={(e) => setIdThreshold(e.currentTarget.value)} onChange={saveIdleConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("idleThresholdHint") || "静默多久才算你真离开、可动后台。默认 480(8 分钟),不打断正在思考/打字的你。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("idleTickLabel") || "巡检间隔(秒)"}</label>
                    <input type="number" min="5" step="10" placeholder="30" value={idTick()}
                      onInput={(e) => setIdTick(e.currentTarget.value)} onChange={saveIdleConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("idleFatigueLabel") || "睡眠疲劳阈"}</label>
                    <input type="number" min="0" max="1" step="0.05" placeholder="0.5" value={idFatigue()}
                      onInput={(e) => setIdFatigue(e.currentTarget.value)} onChange={saveIdleConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("idleFatigueHint") || "疲劳到此值(0~1)且有碎片债才开始睡眠维护。调低=更勤做梦。默认 0.5。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("idleMaxSleepLabel") || "单次睡眠步数"}</label>
                    <input type="number" min="1" step="1" placeholder="16" value={idMaxSleep()}
                      onInput={(e) => setIdMaxSleep(e.currentTarget.value)} onChange={saveIdleConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("idleMaxRehearsalsLabel") || "单次推演次数"}</label>
                    <input type="number" min="0" step="1" placeholder="4" value={idMaxRehearsals()}
                      onInput={(e) => setIdMaxRehearsals(e.currentTarget.value)} onChange={saveIdleConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0", "line-height": "1.5" }}>
                    {t("idleStepsHint") || "一次空闲内做梦/推演的步数上限(防独占,随时让位给你)。默认 16 步 / 4 次推演。"}
                  </p>
                </div>
              </div>
);

export default AgentTab;
