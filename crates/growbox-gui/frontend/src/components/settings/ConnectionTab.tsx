// 设置·连接 Tab:LLM 提供商 + 潜意识网络 + 嵌入模型 + 连接按钮。
import { Show, For, type Component } from "solid-js";
import {
  apiBase, setApiBase, model, setModel, apiKey, setApiKey,
  embedRemote, setEmbedRemote,
  embedApiBase, setEmbedApiBase, embedApiKey, setEmbedApiKey, embedModel, setEmbedModel,
  subconsciousModel, setSubconsciousModel, subconsciousApiBase, setSubconsciousApiBase, subconsciousApiKey, setSubconsciousApiKey,
  connected, connecting, configDirty,
} from "../../store";
import { t, currentPromptLang, setPromptLang, type PromptLang } from "../../i18n";
import { sfx } from "../../sfx";
import {
  modelOptions, setModelOptions, scanning, doScanModels, doConnect,
  webProvider, setWebProvider, webApiBase, setWebApiBase, webApiKey, setWebApiKey,
  webMaxResults, setWebMaxResults, webTimeout, setWebTimeout, saveWebConfig,
} from "./state";

const ConnectionTab: Component = () => (
              <div class="settings-tab-pane active">
                <div class="settings-section">
                  <h3>{t("llmProvider") || "LLM 提供商"}</h3>
                  <div class="settings-field">
                    <label>{t("apiBase") || "API 地址"}</label>
                    <div style={{ display: "flex", gap: "6px", "flex": 1 }}>
                      <input
                        id="set-api_base"
                        value={apiBase()}
                        onInput={(e) => {
                          setApiBase(e.currentTarget.value);
                          // 改地址后清空模型选项，让用户重新扫描
                          if (modelOptions().length > 0) setModelOptions([]);
                        }}
                        style={{ flex: 1, "min-width": 0 }}
                      />
                      <button
                        type="button"
                        title={t("scanModelsTip") || "从 API 拉取可用模型列表"}
                        disabled={scanning() || !apiBase().trim()}
                        onClick={() => { sfx.tap(); void doScanModels(); }}
                        style={{
                          padding: "4px 12px",
                          background: "var(--bg-input, #2a2a2a)",
                          color: "var(--text-primary, #eee)",
                          border: "1px solid var(--border, #444)",
                          "border-radius": "4px",
                          cursor: scanning() ? "wait" : "pointer",
                          opacity: scanning() || !apiBase().trim() ? 0.5 : 1,
                          display: "inline-flex",
                          "align-items": "center",
                          gap: "4px",
                          "white-space": "nowrap",
                          "font-size": "12px",
                        }}
                      >
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                          <circle cx="11" cy="11" r="7" />
                          <line x1="21" y1="21" x2="16.65" y2="16.65" />
                        </svg>
                        {scanning() ? t("scanning") : t("scan")}
                      </button>
                    </div>
                  </div>
                  <div class="settings-field">
                    <label>{t("model") || "模型"}</label>
                    <Show
                      when={modelOptions().length > 0}
                      fallback={
                        <input
                          id="set-model"
                          value={model()}
                          onInput={(e) => setModel(e.currentTarget.value)}
                        />
                      }
                    >
                      <select
                        id="set-model"
                        value={model()}
                        onChange={(e) => setModel(e.currentTarget.value)}
                        style={{
                          padding: "4px 8px",
                          background: "var(--bg-input, #2a2a2a)",
                          color: "var(--text-primary, #eee)",
                          border: "1px solid var(--border, #444)",
                          "border-radius": "4px",
                          flex: 1,
                          "min-width": 0,
                        }}
                      >
                        <For each={modelOptions()}>
                          {(id) => <option value={id}>{id}</option>}
                        </For>
                      </select>
                    </Show>
                  </div>
                  <div class="settings-field">
                    <label>{t("apiKey") || "API Key"}</label>
                    <input
                      id="set-api_key"
                      type="password"
                      placeholder="lm-studio"
                      value={apiKey()}
                      onInput={(e) => setApiKey(e.currentTarget.value)}
                    />
                  </div>
                  <div class="settings-field">
                    <label style={{ "white-space": "nowrap" }}>{t("promptLang")}</label>
                    <select
                      id="set-prompt_lang"
                      value={currentPromptLang()}
                      onChange={(e) => void setPromptLang(e.currentTarget.value as PromptLang)}
                      style={{
                        padding: "4px 8px",
                        background: "var(--bg-input, #2a2a2a)",
                        color: "var(--text-primary, #eee)",
                        border: "1px solid var(--border, #444)",
                        "border-radius": "4px",
                        flex: 1,
                        "min-width": 0,
                      }}
                    >
                      <option value="zh">{t("promptLangZh")}</option>
                      <option value="en">{t("promptLangEn")}</option>
                    </select>
                  </div>
                  <p class="settings-section-hint">{t("promptLangHint")}</p>
                </div>
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <circle cx="8" cy="12" r="3" />
                      <circle cx="16" cy="8" r="2" />
                      <circle cx="16" cy="16" r="2" />
                      <path d="M11 11l3-2M11 13l3 2" />
                    </svg>
                    {t("subModelSlot") || "潜意识模型(选填)"}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("subModelHint") || "检索判断 / 联想跳转 / 经验蒸馏 + 提示词自转译的潜意识那份,用这个模型。留空 = 复用主模型(默认)。可填更便宜/本地的小模型分担后台活。改完点下方「重连」生效。"}
                  </p>
                  {/* 独立厂商:API 地址 / 模型 / Key 三件一直显示(潜意识可能是别家,需自己的端点和 Key);全留空 = 复用主模型。 */}
                  <div class="settings-field">
                    <label>{t("apiBase") || "API 地址"}</label>
                    <input
                      value={subconsciousApiBase()}
                      onInput={(e) => setSubconsciousApiBase(e.currentTarget.value)}
                      placeholder={t("subModelReuseBase") || "留空 = 用主模型端点"}
                      style={{ flex: 1, "min-width": 0 }}
                    />
                  </div>
                  <div class="settings-field">
                    <label>{t("model") || "模型"}</label>
                    <input
                      value={subconsciousModel()}
                      onInput={(e) => setSubconsciousModel(e.currentTarget.value)}
                      placeholder={t("subModelReuse") || "留空 = 复用主模型"}
                      style={{ flex: 1, "min-width": 0 }}
                    />
                  </div>
                  <div class="settings-field">
                    <label>{t("apiKey") || "API Key"}</label>
                    <input
                      type="password"
                      value={subconsciousApiKey()}
                      onInput={(e) => setSubconsciousApiKey(e.currentTarget.value)}
                      placeholder={t("subModelReuseKey") || "留空 = 用主模型 Key"}
                    />
                  </div>
                </div>
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <circle cx="12" cy="12" r="3" />
                      <path d="M12 2v4M12 18v4M2 12h4M18 12h4M5 5l2.5 2.5M16.5 16.5L19 19M19 5l-2.5 2.5M7.5 16.5L5 19" />
                    </svg>
                    {t("embeddingSlot") || "嵌入模型 (Embedding)"}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("embeddingHint") || "记忆检索的语义向量来源。默认本地 e5(离线、各平台通吃);开远程走 OpenAI 兼容端点。换嵌入器后历史会自动重嵌。改完点下方「重连」生效。"}
                  </p>
                  <div class="tool-toggle">
                    <div class="tool-toggle-info">
                      <div class="tool-toggle-name">{t("embedUseRemote") || "使用远程嵌入"}</div>
                      <div class="tool-toggle-desc">
                        {embedRemote() ? (t("embedRemoteOn") || "远程 OpenAI 兼容端点") : (t("embedLocalDefault") || "本地 e5(默认)")}
                      </div>
                    </div>
                    <label class="toggle-switch">
                      <input
                        type="checkbox"
                        checked={embedRemote()}
                        onChange={(e) => setEmbedRemote(e.currentTarget.checked)}
                      />
                      <span class="toggle-track" />
                    </label>
                  </div>
                  <Show when={embedRemote()}>
                    <div class="settings-field">
                      <label>{t("apiBase") || "API 地址"}</label>
                      <input
                        value={embedApiBase()}
                        onInput={(e) => setEmbedApiBase(e.currentTarget.value)}
                        placeholder="https://api.openai.com/v1"
                        style={{ flex: 1, "min-width": 0 }}
                      />
                    </div>
                    <div class="settings-field">
                      <label>{t("model") || "模型"}</label>
                      <input
                        value={embedModel()}
                        onInput={(e) => setEmbedModel(e.currentTarget.value)}
                        placeholder="text-embedding-3-small"
                        style={{ flex: 1, "min-width": 0 }}
                      />
                    </div>
                    <div class="settings-field">
                      <label>{t("apiKey") || "API Key"}</label>
                      <input
                        type="password"
                        value={embedApiKey()}
                        onInput={(e) => setEmbedApiKey(e.currentTarget.value)}
                      />
                    </div>
                  </Show>
                </div>
                {/* ★Web 搜索(web_search provider)★:自含配置,改即落库+即时生效(不依赖「重连」)。 */}
                <div class="settings-section">
                  <h3>
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style={{ "vertical-align": "-2px", "margin-right": "6px" }}>
                      <circle cx="12" cy="12" r="9" />
                      <path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" />
                    </svg>
                    {t("webSearchTitle") || "Web 搜索"}
                  </h3>
                  <p style={{ "font-size": "11px", color: "var(--text-secondary)", margin: "0 0 8px 0", "line-height": "1.5" }}>
                    {t("webSearchHint") || "web_search 的搜索引擎来源:选择提供商并填好 key,AI 才能联网搜资料。web_fetch(抓网页)不依赖此项、开箱即用。改动即时生效。"}
                  </p>
                  <div class="settings-field">
                    <label>{t("webSearchProviderLabel") || "提供商"}</label>
                    <select
                      id="set-web_search_provider"
                      value={webProvider()}
                      onChange={(e) => { setWebProvider(e.currentTarget.value); saveWebConfig(); }}
                      style={{
                        padding: "4px 8px",
                        background: "var(--bg-input, #2a2a2a)",
                        color: "var(--text-primary, #eee)",
                        border: "1px solid var(--border, #444)",
                        "border-radius": "4px",
                        flex: 1,
                        "min-width": 0,
                      }}
                    >
                      <option value="">{t("webSearchProviderDuckDuckGo") || "DuckDuckGo(免 key · 默认)"}</option>
                      <option value="tavily">Tavily</option>
                      <option value="brave">Brave Search</option>
                      <option value="searxng">SearXNG</option>
                    </select>
                  </div>
                  <Show when={webProvider()}>
                    <div class="settings-field">
                      <label>{t("apiBase") || "API 地址"}</label>
                      <input
                        value={webApiBase()}
                        onInput={(e) => setWebApiBase(e.currentTarget.value)}
                        onChange={saveWebConfig}
                        placeholder={webProvider() === "searxng" ? (t("webSearchBaseSearxng") || "自建实例地址,如 http://nas.lan:8888(必填)") : (t("webSearchBaseOptional") || "留空 = 官方端点")}
                        style={{ flex: 1, "min-width": 0 }}
                      />
                    </div>
                    <Show when={webProvider() !== "searxng"}>
                      <div class="settings-field">
                        <label>{t("apiKey") || "API Key"}</label>
                        <input
                          type="password"
                          value={webApiKey()}
                          onInput={(e) => setWebApiKey(e.currentTarget.value)}
                          onChange={saveWebConfig}
                        />
                      </div>
                    </Show>
                    <div class="settings-field">
                      <label>{t("webSearchMaxResultsLabel") || "返回条数(1~10)"}</label>
                      <input type="number" min="1" max="10" step="1" placeholder="5" value={webMaxResults()}
                        onInput={(e) => setWebMaxResults(e.currentTarget.value)} onChange={saveWebConfig}
                        style={{ flex: 1, "min-width": 0 }} />
                    </div>
                  </Show>
                  <div class="settings-field">
                    <label>{t("webTimeoutLabel") || "Web 请求超时(秒,0=不限)"}</label>
                    <input type="number" min="0" step="5" placeholder="30" value={webTimeout()}
                      onInput={(e) => setWebTimeout(e.currentTarget.value)} onChange={saveWebConfig}
                      style={{ flex: 1, "min-width": 0 }} />
                  </div>
                  <p style={{ "font-size": "10px", color: "var(--text-secondary)", margin: "0", "line-height": "1.5" }}>
                    {t("webToolsFootHint") || "内网/本机地址访问会弹窗请你授权;公网直接抓取。抓回内容按不可信外部输入处理。"}
                  </p>
                </div>
                <div class="settings-connect-row">
                  <button
                    class={`btn-connect ${connecting() ? "cancellable" : connected() && !configDirty() ? "connected" : ""}`}
                    onClick={doConnect}
                  >
                    {connecting()
                      ? t("cancelConnect")
                      : connected()
                      ? t("reconnect")
                      : t("connect") || "连接"}
                  </button>
                </div>
              </div>
);

export default ConnectionTab;
