// 设置·工具 Tab:已启用工具开关 + 工具懒加载(deferred 名单) + MCP 服务器(收编生态工具) + 工具输出上限 + 显示。
// 从原「智能体」Tab 拆出工具相关旋钮 + 从 Settings.tsx 内联的 tools pane(工具开关列表)收编,集中到这里。
import { type Component, For, Show } from "solid-js";
import { toolList, truncateToolDisplay, setTruncateToolDisplay, toolExpandDefault, setToolExpanded } from "../../store";
import { toggleTool } from "../../tools";
import { t, tTool } from "../../i18n";
import { api } from "../../tauri-api";
import {
  mcLazyTools, setMcLazyTools, saveLazyToolsConfig, isDeferred, toggleDeferred, NEVER_DEFER,
  mcMcpServers, mcMcpError, mcMcpBusy, mcpStatusOf,
  mcNewName, setMcNewName, mcNewCmd, setMcNewCmd, mcNewArgs, setMcNewArgs, mcNewEnv, setMcNewEnv,
  mcNewTransport, setMcNewTransport, mcNewUrl, setMcNewUrl,
  addMcpServer, removeMcpServer, toggleMcpServer,
  tlRead, setTlRead, tlList, setTlList, tlOutput, setTlOutput, tlOutlineSymbols, setTlOutlineSymbols, tlTaskCap, setTlTaskCap, tlCtxWindow, setTlCtxWindow, tlShellTimeout, setTlShellTimeout, saveToolLimits,
  tsInstalled, tsNpm, tsBusy, tsMsg, installTsserver,
  skillList, skillEnabled, setSkillEnabled, skillListMax, setSkillListMax, skillAutoload, setSkillAutoload, skillExpanded, skillBody,
  toggleSkill, toggleSkillBody, saveSkillConfig,
  skillProposals, skillProposalExpanded, setSkillProposalExpanded, acceptSkillProposal, rejectSkillProposal,
} from "./state";

const ToolsTab: Component = () => (
              <div class="settings-tab-pane active">
                {/* 已启用工具:逐项开关。 */}
                <div class="settings-section">
                  <h3>{t("enabledTools")}</h3>
                  <Show
                    when={toolList().length > 0}
                    fallback={
                      <div style={{ "font-size": "12px", color: "var(--text-secondary)" }}>
                        ({t("statusDisconnected")} / {t("statusLoading")})
                      </div>
                    }
                  >
                    <For each={toolList()}>
                      {(tool) => (
                        <div class="tool-toggle">
                          <div class="tool-toggle-info">
                            <div class="tool-toggle-name">{tool.label ?? tTool(tool.name)}</div>
                            <div class="tool-toggle-desc">{tool.description}</div>
                          </div>
                          <label class="toggle-switch">
                            <input
                              type="checkbox"
                              checked={tool.enabled}
                              onChange={(e) => void toggleTool(tool.name, e.currentTarget.checked)}
                            />
                            <span class="toggle-track" />
                          </label>
                        </div>
                      )}
                    </For>
                  </Show>
                </div>
                {/* ★二期 C1★ 工具懒加载(总开关 + deferred 名单)。即时生效(下回合 tools 装配即用)+ 落库。默认开。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <path d="M3 6h18M7 12h10M10 18h4" />
                    </svg>
                    {t("lazyToolsSetting") || "工具懒加载"}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("lazyToolsHint") || "开:核心工具常驻,扩展工具只露名、用时 tool_search 按需加载(修工作流换工具的缓存破坏)。关=今天的行为不变。"}
                  </p>
                  <div class="tool-toggle">
                    <div class="tool-toggle-info">
                      <div class="tool-toggle-name">{t("lazyToolsToggle") || "启用工具懒加载"}</div>
                      <div class="tool-toggle-desc">{t("lazyToolsToggleDesc") || "默认关。开启后用 tool_search 按需加载非核心工具,缓存前缀更稳。"}</div>
                    </div>
                    <label class="toggle-switch">
                      <input type="checkbox" checked={mcLazyTools()}
                        onChange={(e) => { setMcLazyTools(e.currentTarget.checked); saveLazyToolsConfig(); }} />
                      <span class="toggle-track" />
                    </label>
                  </div>
                  {/* 懒加载开 → 从真实工具列表逐项勾选哪些走 deferred(只露名、用时 tool_search 加载)。 */}
                  <Show when={mcLazyTools()}>
                    <p class="settings-section-hint" style={{ margin: "11px 0 6px" }}>
                      {t("deferredToolsLabel") || "勾选要懒加载(按需 tool_search 拉回)的工具;未勾 = 常驻、低延迟。"}
                    </p>
                    <Show
                      when={toolList().filter((tt) => !NEVER_DEFER.includes(tt.name)).length > 0}
                      fallback={<p class="settings-section-hint">({t("statusLoading") || "加载中…"})</p>}
                    >
                      <div class="settings-list">
                        <For each={toolList().filter((tt) => !NEVER_DEFER.includes(tt.name))}>
                          {(tt) => (
                            <div class="tool-toggle">
                              <div class="tool-toggle-info">
                                <div class="tool-toggle-name">{tt.label ?? tt.name}</div>
                                <div class="tool-toggle-desc">
                                  {isDeferred(tt.name) ? (t("deferredOn") || "按需加载(deferred)") : (t("deferredOff") || "常驻")}
                                </div>
                              </div>
                              <label class="toggle-switch">
                                <input type="checkbox" checked={isDeferred(tt.name)}
                                  onChange={(e) => toggleDeferred(tt.name, e.currentTarget.checked)} />
                                <span class="toggle-track" />
                              </label>
                            </div>
                          )}
                        </For>
                      </div>
                    </Show>
                    <p class="settings-section-hint">
                      {t("deferredToolsHint") || "勾得越多=请求里工具越少、缓存前缀越稳、工作流锁越硬;留常用工具不勾=低延迟。finish/ask_user/workflow_return/tool_search 永远常驻(不在此列)。MCP 工具恒按需加载。"}
                    </p>
                  </Show>
                </div>
                {/* ★二期 D2★ MCP 服务器(收编生态工具):结构化 server 列表 + 添加表单(落库 + 自动重连)。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <path d="M5 12h14M12 5v14M7 7l10 10M17 7L7 17" />
                    </svg>
                    {t("mcpSetting") || "MCP 服务器(收编生态工具)"}
                  </h3>
                  <p class="settings-section-hint">
                    {t("mcpHint") || "连接 MCP 服务器(如 filesystem/github/playwright),其工具自动收编为可调用工具(懒加载下经 tool_search 按需加载)。MCP 结果按外部不可信输入处理。"}
                  </p>
                  {/* 已配置的 server 列表(实时状态 + 启用开关 + 移除) */}
                  <Show
                    when={mcMcpServers().length > 0}
                    fallback={<p class="settings-section-hint">{t("mcpNoServers") || "还没有连接任何 MCP 服务器。在下方添加一个。"}</p>}
                  >
                    <div class="settings-list">
                      <For each={mcMcpServers()}>
                        {(s: any) => (
                          <div class="tool-toggle">
                            <div class="tool-toggle-info">
                              <div class="tool-toggle-name">
                                <span style={{
                                  color: !s.enabled ? "var(--text-tertiary)"
                                    : mcpStatusOf(s.name)?.connected ? "var(--green)"
                                    : mcpStatusOf(s.name)?.error ? "var(--danger, #e5534b)"
                                    : "var(--text-tertiary)",
                                  "margin-right": "6px",
                                }}>●</span>
                                {s.name}
                              </div>
                              <div class="tool-toggle-desc">{s.transport === "http" ? `http · ${s.url}` : `${s.command} ${(s.args || []).join(" ")}`}</div>
                              <div class="tool-toggle-desc" style={{ color: mcpStatusOf(s.name)?.error ? "var(--danger, #e5534b)" : "var(--text-tertiary)" }}>
                                {!s.enabled ? (t("mcpDisabled") || "未启用")
                                  : !mcpStatusOf(s.name) ? (t("mcpConnecting") || "连接中…")
                                  : mcpStatusOf(s.name).connected ? `${t("mcpConnected") || "已连接"} · ${mcpStatusOf(s.name).tool_count} ${t("mcpTools") || "工具"}`
                                  : mcpStatusOf(s.name).error ? `${t("mcpFailed") || "连接失败"}: ${mcpStatusOf(s.name).error}`
                                  : (t("mcpDisabled") || "未启用")}
                              </div>
                            </div>
                            <div class="mcp-row-ctl">
                              <label class="toggle-switch">
                                <input type="checkbox" checked={s.enabled}
                                  onChange={(e) => void toggleMcpServer(s.name, e.currentTarget.checked)} />
                                <span class="toggle-track" />
                              </label>
                              <button type="button" class="mcp-remove" title={t("mcpRemove") || "移除"}
                                onClick={() => void removeMcpServer(s.name)}>✕</button>
                            </div>
                          </div>
                        )}
                      </For>
                    </div>
                  </Show>
                  {/* 添加 server 表单 */}
                  <div class="settings-field">
                    <label>{t("mcpName") || "名称"}</label>
                    <input type="text" placeholder="fs" value={mcNewName()} onInput={(e) => setMcNewName(e.currentTarget.value)} />
                  </div>
                  <div class="settings-field">
                    <label>{t("mcpTransport") || "传输"}</label>
                    <select value={mcNewTransport()} onChange={(e) => setMcNewTransport(e.currentTarget.value)} style={{ flex: 1, "min-width": 0 }}>
                      <option value="stdio">stdio ({t("mcpTransportStdio") || "本地命令"})</option>
                      <option value="http">http ({t("mcpTransportHttp") || "远程 URL"})</option>
                    </select>
                  </div>
                  <Show when={mcNewTransport() === "http"} fallback={
                    <>
                      <div class="settings-field">
                        <label>{t("mcpCommand") || "命令"}</label>
                        <input type="text" placeholder="npx" value={mcNewCmd()} onInput={(e) => setMcNewCmd(e.currentTarget.value)} />
                      </div>
                      <div class="settings-field">
                        <label>{t("mcpArgs") || "参数"}</label>
                        <input type="text" placeholder="-y @modelcontextprotocol/server-filesystem /path" value={mcNewArgs()} onInput={(e) => setMcNewArgs(e.currentTarget.value)} />
                      </div>
                      <div class="settings-field">
                        <label>{t("mcpEnv") || "环境变量"}</label>
                        <input type="text" placeholder={t("mcpEnvPlaceholder") || "可选,如 GITHUB_TOKEN=xxx(空格分隔)"} value={mcNewEnv()} onInput={(e) => setMcNewEnv(e.currentTarget.value)} />
                      </div>
                    </>
                  }>
                    <div class="settings-field">
                      <label>{t("mcpUrl") || "URL"}</label>
                      <input type="text" placeholder="https://example.com/mcp" value={mcNewUrl()} onInput={(e) => setMcNewUrl(e.currentTarget.value)} />
                    </div>
                  </Show>
                  <button type="button" class="settings-btn" disabled={mcMcpBusy()} onClick={() => void addMcpServer()}>
                    {mcMcpBusy() ? (t("mcpConnecting") || "连接中…") : (t("mcpAddServer") || "添加并连接")}
                  </button>
                  <Show when={mcMcpError()}>
                    <p class="settings-section-hint" style={{ color: "var(--danger, #e5534b)", "margin-top": "8px" }}>{mcMcpError()}</p>
                  </Show>
                  <p class="settings-section-hint" style={{ "margin-top": "8px" }}>
                    {t("mcpAddHint") || "参数用空格分隔。添加后落库持久,重启自动重连。开了工具懒加载时 MCP 工具经 tool_search 按需加载。"}
                  </p>
                </div>
                {/* ★Skill 系统(第四原语,设计/09)★:场景化知识/playbook。内置种子 + AI 已学;可看正文、停用/启用、
                    总开关 + 常驻清单上限。符合"所有能改的都能在设置控制"。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <path d="M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5" />
                    </svg>
                    {t("skillSection") || "技能(Skill · 场景化知识)"}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("skillHint") || "第四原语:某类场景该怎么把事做好的 playbook(知识,非工具)。AI 遇匹配场景自动加载、按它用通用工具施展;可被飞轮自学、越用越准。内置=出厂种子,已学=AI 结晶的。"}
                  </p>
                  {/* 总开关 + 清单上限 */}
                  <div class="settings-field" style={{ "align-items": "center", gap: "10px", "flex-wrap": "wrap" }}>
                    <label style={{ display: "flex", "align-items": "center", gap: "6px", cursor: "pointer" }}>
                      <input
                        type="checkbox"
                        checked={skillEnabled()}
                        onChange={(e) => { setSkillEnabled(e.currentTarget.checked); saveSkillConfig(); }}
                      />
                      <span>{t("skillEnabled") || "启用 Skill 系统"}</span>
                    </label>
                    <span style={{ color: "var(--text-secondary)", "font-size": "12px" }}>
                      {t("skillListMax") || "常驻清单上限"}
                    </span>
                    <input
                      type="number"
                      min="1"
                      value={skillListMax()}
                      disabled={!skillEnabled()}
                      onInput={(e) => setSkillListMax(e.currentTarget.value)}
                      onBlur={saveSkillConfig}
                      style={{ width: "64px" }}
                    />
                    <span style={{ color: "var(--text-secondary)", "font-size": "12px" }} title={t("skillAutoloadHint") || "语义召回相似度≥此值的 skill 直接注入正文(省 load 调用),低于则只浮名"}>
                      {t("skillAutoload") || "自动加载阈值"}
                    </span>
                    <input
                      type="number"
                      min="0"
                      max="1"
                      step="0.01"
                      value={skillAutoload()}
                      disabled={!skillEnabled()}
                      onInput={(e) => setSkillAutoload(e.currentTarget.value)}
                      onBlur={saveSkillConfig}
                      style={{ width: "64px" }}
                    />
                  </div>
                  {/* skill 列表 */}
                  <Show
                    when={skillList().length > 0}
                    fallback={<p class="settings-section-hint">{t("skillEmpty") || "(还没有 skill。AI 在某类场景摸索出可复用做法后会自动结晶。)"}</p>}
                  >
                    <div style={{ "margin-top": "8px", display: "flex", "flex-direction": "column", gap: "6px" }}>
                      <For each={skillList()}>
                        {(s) => (
                          <div style={{ border: "1px solid var(--border, #2e323a)", "border-radius": "8px", padding: "8px 10px", opacity: s.active ? "1" : "0.55" }}>
                            <div style={{ display: "flex", "align-items": "center", "justify-content": "space-between", gap: "8px" }}>
                              <div style={{ "min-width": "0", flex: "1", cursor: "pointer" }} onClick={() => toggleSkillBody(s.name)}>
                                <div style={{ display: "flex", "align-items": "center", gap: "6px" }}>
                                  <span style={{ "font-weight": "600", "font-size": "12.5px" }}>{s.name}</span>
                                  <span style={{ "font-size": "10px", color: s.source === "builtin" ? "var(--text-secondary)" : "var(--green)", border: "1px solid currentColor", "border-radius": "4px", padding: "0 4px" }}>
                                    {s.source === "builtin" ? (t("skillBuiltin") || "内置") : (t("skillLearned") || "已学")}
                                  </span>
                                </div>
                                <div style={{ "font-size": "11px", color: "var(--text-secondary)", "margin-top": "2px", "white-space": "nowrap", overflow: "hidden", "text-overflow": "ellipsis" }}>
                                  {s.trigger}
                                </div>
                              </div>
                              <label style={{ display: "flex", "align-items": "center", gap: "4px", cursor: "pointer", "flex-shrink": "0" }} title={s.active ? (t("skillDisable") || "停用") : (t("skillEnable") || "启用")}>
                                <input type="checkbox" checked={s.active} onChange={(e) => toggleSkill(s.name, e.currentTarget.checked)} />
                              </label>
                            </div>
                            <Show when={skillExpanded() === s.name}>
                              <pre style={{ "margin-top": "8px", "white-space": "pre-wrap", "word-break": "break-word", "font-size": "11px", "line-height": "1.55", color: "var(--text-secondary)", "max-height": "260px", overflow: "auto", background: "var(--bg-secondary, #1b1d22)", padding: "8px", "border-radius": "6px" }}>{skillBody()}</pre>
                            </Show>
                          </div>
                        )}
                      </For>
                    </div>
                  </Show>
                  {/* ★S3 飞轮自学提议★:idle 从反复经验里起草的 skill 提议,待你采纳(→结晶成 skill)或丢弃(→不再提)。 */}
                  <Show when={skillProposals().length > 0}>
                    <div style={{ "margin-top": "12px" }}>
                      <div style={{ "font-size": "12px", "font-weight": "600", color: "var(--green)", "margin-bottom": "4px" }}>
                        {t("skillProposalsTitle") || "技能提议(飞轮自学)"} · {skillProposals().length}
                      </div>
                      <p style={{ "font-size": "10.5px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                        {t("skillProposalsHint") || "AI 在空闲时从反复出现的经验里起草的可复用 playbook。采纳=沉淀成正式 skill(以后自动召回);丢弃=不再提这一条。"}
                      </p>
                      <div style={{ display: "flex", "flex-direction": "column", gap: "6px" }}>
                        <For each={skillProposals()}>
                          {(p) => (
                            <div style={{ border: "1px dashed var(--green, #3fb950)", "border-radius": "8px", padding: "8px 10px" }}>
                              <div style={{ display: "flex", "align-items": "center", "justify-content": "space-between", gap: "8px" }}>
                                <div style={{ "min-width": "0", flex: "1", cursor: "pointer" }} onClick={() => setSkillProposalExpanded(skillProposalExpanded() === p.id ? null : p.id)}>
                                  <div style={{ "font-weight": "600", "font-size": "12.5px" }}>{p.name}</div>
                                  <div style={{ "font-size": "11px", color: "var(--text-secondary)", "margin-top": "2px", "white-space": "nowrap", overflow: "hidden", "text-overflow": "ellipsis" }}>
                                    {p.trigger}
                                  </div>
                                </div>
                                <div style={{ display: "flex", gap: "6px", "flex-shrink": "0" }}>
                                  <button class="settings-btn" style={{ padding: "2px 10px", "font-size": "11px" }} onClick={() => acceptSkillProposal(p.id)}>
                                    {t("skillProposalAccept") || "采纳"}
                                  </button>
                                  <button class="settings-btn" style={{ padding: "2px 10px", "font-size": "11px", opacity: "0.75" }} onClick={() => rejectSkillProposal(p.id)}>
                                    {t("skillProposalReject") || "丢弃"}
                                  </button>
                                </div>
                              </div>
                              <Show when={skillProposalExpanded() === p.id}>
                                <Show when={p.rationale}>
                                  <div style={{ "font-size": "10.5px", color: "var(--text-secondary)", "margin-top": "6px", "font-style": "italic" }}>
                                    {t("skillProposalRationale") || "起草依据"}: {p.rationale}
                                  </div>
                                </Show>
                                <pre style={{ "margin-top": "6px", "white-space": "pre-wrap", "word-break": "break-word", "font-size": "11px", "line-height": "1.55", color: "var(--text-secondary)", "max-height": "240px", overflow: "auto", background: "var(--bg-secondary, #1b1d22)", padding: "8px", "border-radius": "6px" }}>{p.body}</pre>
                              </Show>
                            </div>
                          )}
                        </For>
                      </div>
                    </div>
                  </Show>
                </div>
                {/* ★TS/JS 语言服务器装配★:经 npm 装 typescript-language-server 进 GrowBox 自有目录(需 node)。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <path d="M4 17l6-6-6-6M12 19h8" />
                    </svg>
                    {t("tsserverSection") || "TS/JS 代码智能(语言服务器)"}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("tsserverHint") || "TS/JS 的语义级代码智能(诊断/定义/调用层级)需 typescript-language-server。它依赖 Node.js。装好后 TS/JS 走语义层,否则退化到结构层(code_outline)/文本层(code_search)。"}
                  </p>
                  <Show when={tsInstalled()} fallback={
                    <>
                      <div class="settings-field" style={{ "align-items": "center" }}>
                        <button class="settings-btn" disabled={tsBusy() || !tsNpm()} onClick={() => installTsserver()} style={{ flex: 1, "min-width": 0 }}>
                          {tsBusy() ? (t("tsserverInstalling") || "正在装配…(npm 下载中)") : (t("tsserverInstall") || "装配 TS/JS 语言服务器")}
                        </button>
                      </div>
                      <Show when={!tsNpm()}>
                        <p class="settings-section-hint" style={{ color: "var(--danger, #e5534b)" }}>{t("tsserverNoNpm") || "未检测到 npm —— 请先安装 Node.js 再装配。"}</p>
                      </Show>
                    </>
                  }>
                    <p class="settings-section-hint" style={{ color: "var(--green)" }}>✓ {t("tsserverInstalled") || "已装配,TS/JS 走语义层"}</p>
                  </Show>
                  <Show when={tsMsg()}>
                    <p class="settings-section-hint" style={{ "word-break": "break-all" }}>{tsMsg()}</p>
                  </Show>
                </div>
                {/* 工具输出上限旋钮(推论9 数值全可设)。即时生效(下次工具调用/任务即用)+ 落库。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <path d="M4 7V4h16v3M9 20h6M12 4v16" />
                    </svg>
                    {t("toolLimitsSetting") || "工具输出上限"}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("toolLimitsHint") || "AI 读文件/列目录/跑命令时,单次回传给它的内容上限。调大=看得更全但更费上下文。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("toolReadLabel") || "读文件上限(字节)"}</label>
                    <input type="number" min="1024" step="51200" placeholder="204800" value={tlRead()}
                      onInput={(e) => setTlRead(e.currentTarget.value)} onChange={saveToolLimits}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("toolListLabel") || "列目录条目上限"}</label>
                    <input type="number" min="1" step="50" placeholder="500" value={tlList()}
                      onInput={(e) => setTlList(e.currentTarget.value)} onChange={saveToolLimits}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("toolOutputLabel") || "shell 输出上限(字节)"}</label>
                    <input type="number" min="1024" step="16384" placeholder="65536" value={tlOutput()}
                      onInput={(e) => setTlOutput(e.currentTarget.value)} onChange={saveToolLimits}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("toolOutlineSymbolsLabel") || "code_outline 符号上限"}</label>
                    <input type="number" min="10" step="50" placeholder="400" value={tlOutlineSymbols()}
                      onInput={(e) => setTlOutlineSymbols(e.currentTarget.value)} onChange={saveToolLimits}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("taskOutputCapLabel") || "后台任务输出尾巴(字节)"}</label>
                    <input type="number" min="256" step="1024" placeholder="4096" value={tlTaskCap()}
                      onInput={(e) => setTlTaskCap(e.currentTarget.value)} onChange={saveToolLimits}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("ctxWindowLabel") || "上下文窗口总量(token)"}</label>
                    <input type="number" min="1024" step="1024" placeholder="256000" value={tlCtxWindow()}
                      onInput={(e) => setTlCtxWindow(e.currentTarget.value)} onChange={saveToolLimits}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <div class="settings-field">
                    <label>{t("shellTimeoutLabel") || "shell 命令超时(秒,0=不限)"}</label>
                    <input type="number" min="0" step="10" placeholder="60" value={tlShellTimeout()}
                      onInput={(e) => setTlShellTimeout(e.currentTarget.value)} onChange={saveToolLimits}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0", "line-height": "1.5" }}>
                    {t("toolLimitsFootHint") || "默认:读 200KB / 列 500 条 / shell 64KB / 任务尾巴 4KB。超出部分截断并标注。"}
                  </p>
                </div>
                {/* 显示:工具调用/路径过长是否截断。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <path d="M4 6h16M4 12h10M4 18h7" />
                    </svg>
                    {t("displaySetting") || "显示"}
                  </h3>
                  <div class="tool-toggle">
                    <div class="tool-toggle-info">
                      <div class="tool-toggle-name">{t("truncateToolLabel") || "截断过长的命令/路径"}</div>
                      <div class="tool-toggle-desc">
                        {truncateToolDisplay() ? (t("truncateToolOn") || "过长则省略,界面更紧凑") : (t("truncateToolOff") || "完整显示不省略(默认)")}
                      </div>
                    </div>
                    <label class="toggle-switch">
                      <input
                        type="checkbox"
                        checked={truncateToolDisplay()}
                        onChange={(e) => { const v = e.currentTarget.checked; setTruncateToolDisplay(v); void api.setTruncateToolDisplay(v).catch(() => {}); }}
                      />
                      <span class="toggle-track" />
                    </label>
                  </div>
                  {/* ★工具调用块默认展开★:聊天里某工具的调用块默认折叠还是展开。逐工具勾选(默认仅 ask_user 展开,让用户一眼看到提问/选项)。失败/进行中的块恒展开,不受此影响。 */}
                  <p class="settings-section-hint" style={{ margin: "11px 0 6px" }}>
                    {t("expandToolsLabel") || "勾选要默认展开调用块的工具(否则折叠,点开才看到详情)。提问类(ask_user)建议展开,否则问题/选项不易被看到。"}
                  </p>
                  <Show
                    when={toolList().length > 0}
                    fallback={<p class="settings-section-hint">({t("statusDisconnected")} / {t("statusLoading")})</p>}
                  >
                    <div class="settings-list">
                      <For each={toolList()}>
                        {(tt) => (
                          <div class="tool-toggle">
                            <div class="tool-toggle-info">
                              <div class="tool-toggle-name">{tt.label ?? tTool(tt.name)}</div>
                              <div class="tool-toggle-desc">
                                {toolExpandDefault(tt.name) ? (t("expandOn") || "默认展开") : (t("expandOff") || "默认折叠")}
                              </div>
                            </div>
                            <label class="toggle-switch">
                              <input type="checkbox" checked={toolExpandDefault(tt.name)}
                                onChange={(e) => setToolExpanded(tt.name, e.currentTarget.checked)} />
                              <span class="toggle-track" />
                            </label>
                          </div>
                        )}
                      </For>
                    </div>
                  </Show>
                </div>
              </div>
);

export default ToolsTab;
