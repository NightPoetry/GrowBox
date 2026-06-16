import { For, Show, createSignal, createEffect, createMemo, type Component } from "solid-js";

const RENDER_LIMIT = 500;
import { messages, setMessages, sending, connected, model, historyLoading, historyHasMore, setLoadingSessionId, setHistoryHasMore, setHistoryOldestTs, setCanLoadPrevSession, consumeJumpButtonSuppress, activeCitations, setActiveCitations, setSettingsOpen, setHealthMonitorOpen, setHistoryDrawerOpen, nowTick, verifyStatus, autoMode, selfDriveActive, setSelfDriveActive, type Msg } from "../store";
import { doSend, cancelSend, maybeSelfDrive } from "../chat";
import { loadOlderChat } from "../history";
import { renderMd } from "../markdown";
import { t, setLang, currentLang, type Lang } from "../i18n";
import { sfx } from "../sfx";
import { api, listen } from "../tauri-api";
import { notify } from "../notices";

// 阻塞等待动效:静默判定阈值。
// 有正文后 idle 超过 QUIET 才显示 spinner 行(流畅吐字时 chunk 间隔远小于此,不打扰);
// idle 超过 STALL 升级文案+变色(对齐 chat.ts watchdog 软阈值 60s)。
const WAIT_QUIET_MS = 4000;
const WAIT_STALL_MS = 60_000;

// 流式 assistant 消息上的实时等待指示器。三态:
// ① 思考态(无正文)= 脉冲点 + 已等待秒数;
// ② 静默态(有正文但 idle>QUIET,典型=工具执行/等响应)= spinner + 秒数,超 STALL 变"仍在等待"红;
// ③ 流畅态(有正文且刚来 chunk)= 不渲染(正文末尾的 blink 光标已够),避免吐字时干扰。
const WaitingIndicator: Component<{ m: Msg }> = (props) => {
  const elapsed = () => Math.max(0, Math.floor((nowTick() - props.m.ts) / 1000));
  const idleMs = () => nowTick() - (props.m.lastActivity ?? props.m.ts);
  const stalled = () => idleMs() > WAIT_STALL_MS;
  const showQuiet = () => !!props.m.content && idleMs() > WAIT_QUIET_MS;
  return (
    // ★主动自检动效★:核查阶段优先显示"正在核查 xxx"动态 pill(随核查对象变);否则走思考/等待指示器。
    <Show
      when={verifyStatus()}
      fallback={
        <>
          <Show when={!props.m.content}>
            <div class="msg-waiting msg-waiting-think">
              <span class="thinking-dots"><span /><span /><span /></span>
              <span class="wait-elapsed">{t("waitThinking")} · {elapsed()}s</span>
            </div>
          </Show>
          <Show when={showQuiet()}>
            <div class={`msg-waiting msg-waiting-quiet${stalled() ? " stalled" : ""}`}>
              <span class="wait-spinner" />
              <span class="wait-elapsed">
                {stalled() ? t("waitStalled") : t("waitResponding")} · {elapsed()}s
              </span>
            </div>
          </Show>
        </>
      }
    >
      <div class="msg-waiting msg-waiting-verify">
        <span class="thinking-dots"><span /><span /><span /></span>
        <span class="wait-elapsed verify-label">{verifyStatus()}</span>
      </div>
    </Show>
  );
};

const ChatArea: Component = () => {
  const [input, setInput] = createSignal("");
  // 输入框历史导航(shell 式 ↑/↓ 翻历史询问)。history 存本会话用户提交过的原始输入
  // (未带引用包装的纯文本);histIndex=-1 表示未在翻历史(显示实时草稿);
  // draftStash 在开始上翻时暂存当前草稿,下翻回到底部时恢复。
  const [history, setHistory] = createSignal<string[]>([]);
  const [histIndex, setHistIndex] = createSignal(-1);
  let draftStash = "";
  const [dragOver, setDragOver] = createSignal(false);
  // jump-to-bottom 按钮：用户上滑期间来新消息时显示
  const [showJumpToBottom, setShowJumpToBottom] = createSignal(false);
  // 未读计数：用户上滑期间累计新消息条数；点按钮 / 滚到底清零
  const [unreadCount, setUnreadCount] = createSignal(0);
  // 软虚拟列表：超过 RENDER_LIMIT 时只渲染最后 N 条，避免 DOM 爆炸。
  // 历史 loadMore 仍能拉更早消息进 store —— 但若 store 已 >LIMIT，渲染时切片末尾。
  const renderedMessages = createMemo(() => {
    const list = messages().filter(m => {
      if (m.role === "assistant" && !m.content && !m.thinking && !m.streaming) return false;
      return true;
    });
    return list.length <= RENDER_LIMIT ? list : list.slice(-RENDER_LIMIT);
  });
  const lastAssistantDone = createMemo(() => {
    const list = messages();
    if (list.length === 0 || sending()) return false;
    const last = list[list.length - 1];
    return last.role === "assistant" && !last.streaming && !!last.content;
  });

  function retry() {
    const list = messages();
    let lastUserMsg = "";
    for (let i = list.length - 1; i >= 0; i--) {
      if (list[i].role === "user") { lastUserMsg = list[i].content; break; }
    }
    if (!lastUserMsg) return;
    sfx.tap();
    void doSend(lastUserMsg);
  }

  let messagesEl: HTMLDivElement | undefined;
  let textareaEl: HTMLTextAreaElement | undefined;
  // 输入法(IME)合字状态:组字中的回车只确认候选词、不发送。macOS WKWebView 上 e.isComposing 在
  // "确认合字的那次回车 keydown"可能已为 false(compositionend 先于该 keydown 触发)→ 单靠 isComposing
  // 会漏,导致中文输入法里用回车选词(含纯英文经拼音上屏)被误当发送。故再记 compositionend 时刻,
  // 紧随其后极短窗口(<120ms)内的回车也视为合字收尾、不发送(比"双回车"自然:确认后再敲才发)。
  let imeComposing = false;
  let imeEndedAt = 0;

  // 聊天里的超链接(markdown.ts 渲染的 a.chat-link)委托点击:拦下默认导航,经后端在系统浏览器打开。
  function onMessagesClick(e: MouseEvent) {
    const a = (e.target as HTMLElement).closest("a.chat-link") as HTMLElement | null;
    if (!a) return;
    e.preventDefault();
    const href = a.getAttribute("data-href");
    if (href) void api.openExternalUrl(href).catch(() => {});
  }

  function isNearBottom(): boolean {
    if (!messagesEl) return true;
    return messagesEl.scrollHeight - messagesEl.scrollTop - messagesEl.clientHeight < 80;
  }

  function scrollIfNear() {
    if (!messagesEl) return;
    if (isNearBottom()) {
      messagesEl.scrollTop = messagesEl.scrollHeight;
    }
  }

  function scrollToBottom() {
    if (!messagesEl) return;
    messagesEl.scrollTop = messagesEl.scrollHeight;
    setShowJumpToBottom(false);
    setUnreadCount(0);
  }

  // Effect 1：跟踪 list 长度变化。两种语义：
  //   (a) 0/N → N+M：常规追加（用户发 / 流式回 / 滚顶懒加载）→ 不在底部时显示 jump 按钮。
  //   (b) 0 → N：项目切换/首次进入，history 一次性灌入 → 强制滚到底（与 v1 一致）。
  let lastLen = -1; // -1：首次 effect 跑不显示按钮，仅初始化基线
  createEffect(() => {
    const len = messages().length;
    const prevLen = lastLen;
    lastLen = len;
    if (prevLen === 0 && len > 0) {
      // 从空 → 有内容：强制滚到底，按钮不显示
      queueMicrotask(() => {
        if (!messagesEl) return;
        messagesEl.scrollTop = messagesEl.scrollHeight;
        setShowJumpToBottom(false);
      });
    } else if (prevLen > 0 && len > prevLen) {
      // 已有内容时新增。同步判定底部 + 同步设按钮——避免 queueMicrotask 内 isNearBottom
      // 受 Effect 2 的 scrollIfNear 副作用干扰（Effect 2 在底部时会同步把 scrollTop 拉到底）。
      if (consumeJumpButtonSuppress()) {
        // pass
      } else if (!isNearBottom()) {
        setShowJumpToBottom(true);
        setUnreadCount((c) => c + (len - prevLen));
      }
    }
  });

  // Effect 2：流式跟随——长度 + 当前 streaming msg 的 content/thinking 变化都触发 scrollIfNear。
  // 用户已在底部 → 自动跟随；否则 scrollIfNear 内部判 isNearBottom 不动。
  createEffect(() => {
    const list = messages();
    list.length;
    for (const m of list) {
      if (m.streaming) {
        m.content;
        m.thinking;
        break;
      }
    }
    queueMicrotask(scrollIfNear);
  });

  async function onScroll() {
    if (!messagesEl) return;
    if (isNearBottom()) {
      setShowJumpToBottom(false);
      setUnreadCount(0);
    }
    if (messagesEl.scrollTop < 80 && historyHasMore() && !historyLoading()) {
      const prevHeight = messagesEl.scrollHeight;
      const prevTop = messagesEl.scrollTop;
      await loadOlderChat(30);
      // 维持视觉位置
      queueMicrotask(() => {
        if (!messagesEl) return;
        messagesEl.scrollTop = messagesEl.scrollHeight - prevHeight + prevTop;
      });
    }
  }

  function autoResize() {
    if (!textareaEl) return;
    textareaEl.style.height = "auto";
    const capped = Math.min(textareaEl.scrollHeight, 160);
    textareaEl.style.height = capped + "px";
    textareaEl.classList.toggle("scrollable", textareaEl.scrollHeight > 160);
  }

  async function send() {
    const text = input();
    if (!text.trim()) return;
    // 入历史(shell 式):记录用户原始输入,去掉与上一条连续重复;提交后退出翻历史态。
    const trimmedRaw = text.trim();
    setHistory((prev) => (prev[prev.length - 1] === trimmedRaw ? prev : [...prev, trimmedRaw]));
    setHistIndex(-1);
    draftStash = "";
    const citations = activeCitations();

    // 构建带结构化引用的消息
    let fullText = "";
    if (citations.length > 0) {
      // 每个引用块：标记 + 元数据 + 完整上下文
      for (const c of citations) {
        fullText += `[用户引用了历史记录 时间 ${c.ts} 会话 ${c.session_id.slice(0, 16)}]\n`;
        fullText += `引用内容：\n"${c.fullContent}"\n`;
        if (c.before.length > 0 || c.after.length > 0) {
          fullText += `上下文（前后各 ${Math.max(c.before.length, c.after.length)} 条）：\n`;
          for (const m of c.before) {
            fullText += `  [${m.role === "user" ? "用户" : "AI"}] ${m.ts.slice(11,19)}: ${m.content.slice(0, 200)}\n`;
          }
          fullText += `  → [引用] ${c.contentPreview}\n`;
          for (const m of c.after) {
            fullText += `  [${m.role === "user" ? "用户" : "AI"}] ${m.ts.slice(11,19)}: ${m.content.slice(0, 200)}\n`;
          }
        }
        fullText += "\n";
      }
      fullText += `[用户的问题]\n${text.trim()}`;
    } else {
      fullText = text.trim();
    }

    setInput("");
    setActiveCitations([]);
    if (textareaEl) {
      textareaEl.value = "";
      textareaEl.style.height = "auto";
    }
    scrollToBottom();
    const p = doSend(fullText);
    queueMicrotask(() => scrollToBottom());
    await p;
  }

  // 把文本写进输入框,同步 textarea.value/高度,并把光标移到末尾(便于继续编辑)。
  function applyHistoryInput(v: string) {
    setInput(v);
    if (!textareaEl) return;
    textareaEl.value = v;
    autoResize();
    queueMicrotask(() => {
      if (!textareaEl) return;
      textareaEl.selectionStart = textareaEl.selectionEnd = v.length;
    });
  }

  // 光标是否在首行/末行(用于决定 ↑/↓ 是翻历史还是在多行内移动光标)。
  function caretAtFirstLine(): boolean {
    if (!textareaEl) return true;
    return textareaEl.value.slice(0, textareaEl.selectionStart).indexOf("\n") === -1;
  }
  function caretAtLastLine(): boolean {
    if (!textareaEl) return true;
    return textareaEl.value.slice(textareaEl.selectionEnd).indexOf("\n") === -1;
  }

  // ↑:往更旧翻。返回 true=已处理(吞掉按键)。
  function historyPrev(): boolean {
    const h = history();
    if (h.length === 0) return false;
    let idx = histIndex();
    if (idx === -1) {
      draftStash = input(); // 进入历史前暂存当前草稿
      idx = h.length - 1;
    } else if (idx > 0) {
      idx -= 1;
    } else {
      return true; // 已在最旧:吞掉,不再上翻
    }
    setHistIndex(idx);
    applyHistoryInput(h[idx]);
    return true;
  }

  // ↓:往更新翻;越过最新一条则回到草稿。返回 true=已处理。
  function historyNext(): boolean {
    const idx = histIndex();
    if (idx === -1) return false; // 不在翻历史:放行默认(光标下移)
    const h = history();
    if (idx < h.length - 1) {
      const next = idx + 1;
      setHistIndex(next);
      applyHistoryInput(h[next]);
    } else {
      setHistIndex(-1);
      applyHistoryInput(draftStash); // 回到底部:恢复草稿
    }
    return true;
  }

  function onKeyDown(e: KeyboardEvent) {
    // 回车发送,但★输入法正在组字时的回车只确认候选词、不发送★。合字判定三重兜底(单靠 isComposing
    // 在 macOS WKWebView 漏掉"确认合字的回车"):① e.isComposing/keyCode 229 = 当前 keydown 在合成中;
    // ② imeComposing = compositionstart/end 跟踪的实时态;③ compositionend 后极短窗口(收尾回车补漏)。
    const inIme = e.isComposing || e.keyCode === 229 || imeComposing || Date.now() - imeEndedAt < 120;
    // Shift+Enter 仍为换行;合字收尾的回车只上屏,确认后再敲一次才真发送。
    if (e.key === "Enter" && !e.shiftKey && !inIme) {
      e.preventDefault();
      void send();
      return;
    }
    // shell 式历史导航:仅在无 IME 合成/无修饰键、且光标处于首行(↑)/末行(↓)时触发,
    // 否则放行默认,多行编辑与光标移动不受影响。
    if (inIme || e.altKey || e.ctrlKey || e.metaKey) return;
    if (e.key === "ArrowUp" && caretAtFirstLine()) {
      if (historyPrev()) e.preventDefault();
    } else if (e.key === "ArrowDown" && caretAtLastLine()) {
      if (historyNext()) e.preventDefault();
    }
  }

  function isImagePath(p: string): boolean {
    return /\.(png|jpg|jpeg|gif|webp|bmp|svg|tiff?)$/i.test(p);
  }

  function modelSupportsVision(): boolean {
    const m = model().toLowerCase();
    return m.includes("vision") || m.includes("-vl") || m.includes("vl-")
      || m.includes("multimodal") || m.includes("gpt-4o") || m.includes("gemini");
  }

  function handleNativeDrop(paths: string[]) {
    const accepted: string[] = [];
    for (const p of paths) {
      if (isImagePath(p) && !modelSupportsVision()) {
        notify("chat.model_no_vision");
        continue;
      }
      accepted.push(p);
    }
    if (accepted.length === 0) return;
    const cur = input().trim();
    const joined = accepted.join("\n");
    const next = cur ? `${cur}\n${joined}` : joined;
    setInput(next);
    if (textareaEl) {
      textareaEl.value = next;
      textareaEl.focus();
      autoResize();
    }
  }

  // 历史引用的落点处理已统一到 drag-utils 的指针拖拽(几何命中 + 回调直接入 activeCitations),
  // 这里不再需要 HTML5 dragover/drop。下面的 tauri 事件仅用于原生文件拖入。

  void listen<{ paths: string[] }>("tauri://drag-enter", () => setDragOver(true));
  void listen("tauri://drag-leave", () => { setDragOver(false); });
  void listen<{ paths: string[] }>("tauri://drag-drop", (payload) => {
    setDragOver(false);
    if (payload.paths?.length) handleNativeDrop(payload.paths);
  });

  // ★自驱续跑★:全自动模式下解锁的"一直跑"开关。点亮 = 每当 AI 停下来就自动续上(见 chat.ts)。
  // 点暗 = 本轮跑完即停;按「终止」会同时点暗它(cancelSend)。
  function toggleSelfDrive() {
    sfx.tap();
    const next = !selfDriveActive();
    setSelfDriveActive(next);
    if (next) maybeSelfDrive(); // 当前空闲则立刻开跑;正在回合中则等本轮结束接力(doSend 末尾会触发)。
  }

  async function newChat() {
    try {
      sfx.tap();
      const newSid = await api.resetChatSession();
      setMessages([]);
      setLoadingSessionId(newSid);
      setHistoryOldestTs(null);
      setHistoryHasMore(false);
      setCanLoadPrevSession(true);
      notify("chat.reset");
    } catch (e) {
      notify("chat.reset_failed", { detail: String(e) });
    }
  }

  return (
    <div
      class={`chat-area${dragOver() ? " drag-over" : ""}`}
    >
      <div class="chat-toolbar">
        <button
          class="chat-toolbar-btn"
          onClick={() => void newChat()}
          disabled={sending()}
          title={t("newChat")}
        >
          <svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M12 5v14M5 12h14" />
          </svg>
          <span>{t("newChat")}</span>
        </button>
        <div class="chat-toolbar-spacer" />
        <button
          class="chat-toolbar-btn"
          title={t("settings") || "设置"}
          onClick={() => { sfx.tap(); setSettingsOpen(true); }}
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="3" />
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
          </svg>
        </button>
        <button
          class="chat-toolbar-btn"
          title={t("healthMonitor") || "系统健康监控"}
          onClick={() => { sfx.tap(); setHealthMonitorOpen(true); }}
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M22 12h-4l-3 9L9 3l-3 9H2" />
          </svg>
        </button>
        <button
          class="chat-toolbar-btn"
          title={t("historyPanel") || "会话历史"}
          onClick={() => { sfx.tap(); setHistoryDrawerOpen((p: boolean) => !p); }}
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="10" />
            <polyline points="12 6 12 12 16 14" />
          </svg>
        </button>
        <button
          class={`chat-toolbar-btn ${sfx.isMuted() ? "muted" : ""}`}
          title={sfx.isMuted() ? t("unmute") : t("mute")}
          onClick={() => sfx.toggle()}
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5" />
            <path d="M15.54 8.46a5 5 0 0 1 0 7.07" />
          </svg>
        </button>
        <div class="chat-toolbar-status">
          <span class={`dot ${connected() ? "on" : ""}`} />
          <span>{connected() ? t("connected") : t("disconnected")}</span>
        </div>
        <select
          class="chat-toolbar-lang"
          value={currentLang()}
          onChange={(e) => void setLang(e.currentTarget.value as Lang)}
        >
          <option value="zh-CN">中文</option>
          <option value="en">English</option>
          <option value="ja">日本語</option>
          <option value="zh-TW">繁體</option>
        </select>
      </div>
      <div class="messages" ref={messagesEl} onScroll={() => void onScroll()} onClick={onMessagesClick}>
        <Show when={historyLoading()}>
          <div class="history-loading" style={{ "text-align": "center", padding: "8px", "font-size": "12px", color: "var(--text-secondary)" }}>
            {t("loadingHistory")}
          </div>
        </Show>
        <Show
          when={renderedMessages().length > 0}
          fallback={
            <div class="empty-hint">
              <svg
                class="empty-icon"
                width="72"
                height="72"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="1.2"
              >
                <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
              </svg>
              <div class="empty-text">{t("noMessages")}</div>
              <div class="empty-sub">{t("emptyHint")}</div>
            </div>
          }
        >
          <For each={renderedMessages()}>
            {(m) => (
              <div
                class={`msg ${m.role} ${m.streaming ? "streaming" : ""}`}
              >
                <Show when={m.thinking}>
                  <details class="msg-thinking">
                    <summary>{t("thinkingProcess")}</summary>
                    <div class="think-body">{m.thinking}</div>
                  </details>
                </Show>
                <Show when={m.content}>
                  <div
                    class="msg-content"
                    innerHTML={
                      m.role === "user" ? escUser(m.content) : renderMd(m.content, !!m.streaming)
                    }
                  />
                </Show>
                <Show when={m.role === "assistant" && m.streaming}>
                  <WaitingIndicator m={m} />
                </Show>
                <Show when={m.meta}>
                  <div class="msg-meta">
                    {m.meta!.inputTokens}→{m.meta!.outputTokens} · {m.meta!.model}
                  </div>
                </Show>
              </div>
            )}
          </For>
          <Show when={lastAssistantDone()}>
            <div class="retry-bar">
              <button class="retry-btn" onClick={retry}>
                <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="1 4 1 10 7 10"/><path d="M3.51 15a9 9 0 1 0 2.13-9.36L1 10"/></svg>
                {t("retry")}
              </button>
            </div>
          </Show>
        </Show>
      </div>
      <Show when={showJumpToBottom()}>
        <button
          class="jump-to-bottom"
          title={t("jumpToBottom") || "回到底部"}
          onClick={() => {
            sfx.tap();
            if (typeof navigator !== "undefined" && navigator.vibrate) navigator.vibrate(20);
            scrollToBottom();
          }}
        >
          <Show when={unreadCount() > 0}>
            <span class="jump-unread-badge">
              {unreadCount() > 99 ? "99+" : unreadCount()}
            </span>
          </Show>
          <svg viewBox="0 0 24 24" width="18" height="18">
            <path d="M7 10l5 5 5-5" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round" />
          </svg>
        </button>
      </Show>
      {/* 引用栏：在输入框上方，每行一条，不挤占输入区 */}
      <Show when={activeCitations().length > 0}>
        <div class="citation-bar-above">
          <For each={activeCitations()}>
            {(block) => (
              <div class="citation-bar-item">
                <span class="citation-bar-time">{block.ts.slice(0,19)}</span>
                <span class="citation-bar-role">
                  {block.after.length > 0 || block.before.length > 0 ? "📎" : "📌"}
                </span>
                <span class="citation-bar-preview" title={block.contentPreview}>
                  {block.contentPreview}
                </span>
                {block.before.length > 0 || block.after.length > 0 ? (
                  <span class="citation-bar-context">±{Math.max(block.before.length, block.after.length)}</span>
                ) : null}
                <button
                  class="citation-bar-remove"
                  onClick={() => setActiveCitations(prev => prev.filter(c => c.id !== block.id))}
                >×</button>
              </div>
            )}
          </For>
        </div>
      </Show>
      <div class="compose">
        {/* ★自驱续跑开关★:仅全自动模式下解锁(无人值守连续干活需要自动审核 shell)。
            点亮后 AI 每次停下来都自动续上"继续推进"(见 chat.ts runSelfDriveLoop)。 */}
        <Show when={autoMode()}>
          <button
            class={`compose-selfdrive${selfDriveActive() ? " active" : ""}`}
            title={selfDriveActive() ? t("selfDriveOnTip") : t("selfDriveOffTip")}
            disabled={!connected()}
            onClick={toggleSelfDrive}
          >
            <svg viewBox="0 0 24 24" width="18" height="18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
              <polyline points="17 1 21 5 17 9" />
              <path d="M3 11V9a4 4 0 0 1 4-4h14" />
              <polyline points="7 23 3 19 7 15" />
              <path d="M21 13v2a4 4 0 0 1-4 4H3" />
            </svg>
          </button>
        </Show>
        <textarea
          ref={textareaEl}
          class="compose-input"
          placeholder={connected() ? t("placeholder") : t("placeholderDisconnected")}
          rows="1"
          value={input()}
          disabled={!connected()}
          onInput={(e) => {
            setInput(e.currentTarget.value);
            // 用户真在打字 = 离开历史浏览,当前文本成为新草稿。
            if (histIndex() !== -1) setHistIndex(-1);
            autoResize();
          }}
          onKeyDown={onKeyDown}
          onCompositionStart={() => { imeComposing = true; }}
          onCompositionEnd={() => { imeComposing = false; imeEndedAt = Date.now(); }}
        />
        <Show
          when={sending()}
          fallback={
            <button
              class="compose-send"
              title={t("send")}
              disabled={!connected() || !input().trim()}
              onClick={() => void send()}
            >
              <svg viewBox="0 0 24 24">
                <path d="M2.01 21L23 12 2.01 3 2 10l15 2-15 2z" />
              </svg>
            </button>
          }
        >
          {/* 回合进行中:发送按钮变「终止」(造物交互 v2 §2),点了叫停当前回合 */}
          <button
            class="compose-send compose-stop"
            title={t("stop")}
            onClick={() => { sfx.tap(); void cancelSend(); }}
          >
            <svg viewBox="0 0 24 24">
              <rect x="7" y="7" width="10" height="10" rx="1.5" />
            </svg>
          </button>
        </Show>
      </div>
    </div>
  );
};

function escUser(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\n/g, "<br>");
}

export default ChatArea;
