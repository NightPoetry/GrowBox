import { createSignal, createEffect, For, Show, type Component } from "solid-js";
import { historyDrawerOpen, setHistoryDrawerOpen, activeCitations, setActiveCitations, currentProjectId, projects } from "../store";
import { api, type ChatHistoryItem, type CitationBlock } from "../tauri-api";
import { t } from "../i18n";
import { sfx } from "../sfx";
import { startCitationDrag, type CitationDragItem } from "../drag-utils";

const PAGE_SIZE = 50;

/// Right-side overlay drawer showing the current project's conversation history
/// as a single coherent dialog. Internally history is structured files across
/// sessions; externally the user sees one continuous conversation.
const HistoryDrawer: Component = () => {
  const [messages, setMessages] = createSignal<ChatHistoryItem[]>([]);
  const [loading, setLoading] = createSignal(false);
  const [hasMore, setHasMore] = createSignal(true);
  const [initialised, setInitialised] = createSignal(false);

  let scrollRef: HTMLDivElement | undefined;
  let autoScroll = true; // stay at bottom on first load

  // ── Data loading ──────────────────────────────────────────────

  async function loadNextPage() {
    if (loading() || !hasMore()) return;
    setLoading(true);
    try {
      const msgs = messages();
      const cursor = msgs.length > 0 ? msgs[0].ts : null;
      const items = await api.getProjectConversationHistory(cursor, PAGE_SIZE);
      if (!items || items.length === 0) {
        setHasMore(false);
      } else {
        // items are oldest-first; prepend to existing
        setMessages((prev) => [...items, ...prev]);
        if (items.length < PAGE_SIZE) setHasMore(false);
      }
    } catch (e) {
      console.warn("HistoryDrawer: loadNextPage failed", e);
    } finally {
      setLoading(false);
    }
  }

  function resetAndLoad() {
    setMessages([]);
    setHasMore(true);
    autoScroll = true;
    void loadInitial();
  }

  async function loadInitial() {
    setLoading(true);
    try {
      const items = await api.getProjectConversationHistory(null, PAGE_SIZE);
      if (!items || items.length === 0) {
        setHasMore(false);
      } else {
        setMessages(items);
        if (items.length < PAGE_SIZE) setHasMore(false);
      }
    } catch (e) {
      console.warn("HistoryDrawer: loadInitial failed", e);
    } finally {
      setLoading(false);
    }
  }

  // Track project changes to force reload
  const [lastProjectId, setLastProjectId] = createSignal<string | null>(null);

  // ── Reactive triggers ─────────────────────────────────────────

  // First open: initialise and load
  createEffect(() => {
    if (historyDrawerOpen() && !initialised()) {
      setInitialised(true);
      setLastProjectId(currentProjectId());
      resetAndLoad();
    }
  });

  // Project switch: reset and reload if drawer is open
  createEffect(() => {
    const pid = currentProjectId();
    if (pid !== lastProjectId() && initialised()) {
      setLastProjectId(pid);
      setMessages([]);
      setHasMore(true);
      autoScroll = true;
      if (historyDrawerOpen()) {
        void loadInitial();
      } else {
        // Mark as needing reload on next open
        setInitialised(false);
      }
    }
  });

  // Auto-scroll to bottom on first load
  createEffect(() => {
    const msgs = messages();
    if (autoScroll && msgs.length > 0 && scrollRef) {
      requestAnimationFrame(() => {
        scrollRef!.scrollTop = scrollRef!.scrollHeight;
        autoScroll = false;
      });
    }
  });

  // Scroll-to-top detection for pagination
  function handleScroll(e: Event) {
    const el = e.currentTarget as HTMLDivElement;
    if (el.scrollTop <= 60 && hasMore() && !loading()) {
      void loadNextPage();
    }
  }

  // ── Helpers ───────────────────────────────────────────────────

  function formatTs(ts: string): string {
    try {
      const d = new Date(ts);
      const pad = (n: number) => String(n).padStart(2, "0");
      return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
    } catch {
      return ts;
    }
  }

  function previewContent(content: string, max: number = 120): string {
    return content.length > max ? content.slice(0, max) + "…" : content;
  }

  // ── Event handlers ────────────────────────────────────────────

  // 点击与拖拽共用同一条引用逻辑(去重 + 拉上下文 + 降级)。
  async function addCitation(item: CitationDragItem) {
    sfx.tap();
    const id = `${item.session_id.slice(0,8)}-${item.ts.slice(0,19)}`;
    if (activeCitations().some(c => c.id === id)) return;
    try {
      const ctx = await api.getCitationContext(item.session_id, item.ts, 5);
      const block: CitationBlock = {
        id,
        session_id: ctx.session_id,
        ts: ctx.cited?.ts ?? item.ts,
        contentPreview: previewContent(ctx.cited?.content ?? item.content, 150),
        fullContent: ctx.cited?.content ?? item.content,
        before: ctx.before ?? [],
        after: ctx.after ?? [],
      };
      setActiveCitations(prev => [...prev, block]);
    } catch {
      // 降级:只用当前消息内容,无上下文
      const block: CitationBlock = {
        id,
        session_id: item.session_id,
        ts: item.ts,
        contentPreview: previewContent(item.content, 150),
        fullContent: item.content,
        before: [],
        after: [],
      };
      setActiveCitations(prev => [...prev, block]);
    }
  }

  // 单一入口:按住向左拖到对话区 = 拖拽引用;原地松手 = 点击引用。
  function onItemPointerDown(e: PointerEvent, item: ChatHistoryItem) {
    const dragItem: CitationDragItem = {
      session_id: item.session_id, ts: item.ts, role: item.role, content: item.content,
    };
    startCitationDrag(e, dragItem, {
      onDrop: (it) => void addCitation(it),
      onClick: (it) => void addCitation(it),
    });
  }

  // ── Render ────────────────────────────────────────────────────

  const projectName = () => {
    const pid = currentProjectId();
    if (!pid) return "";
    return projects().find((p) => p.id === pid)?.name ?? pid.slice(0, 8);
  };

  return (
    <>
      {/* Toggle tab — fixed to right edge */}
      <button
        class={`history-drawer-toggle${historyDrawerOpen() ? " history-drawer-toggle-open" : ""}`}
        onClick={() => {
          sfx.tap();
          setHistoryDrawerOpen((p) => !p);
        }}
        title={historyDrawerOpen() ? (t("removeCitation")) : (t("historyPanel"))}
      >
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          {historyDrawerOpen() ? (
            <>
              <path d="M18 6L6 18" />
              <path d="M6 6l12 12" />
            </>
          ) : (
            <>
              <circle cx="12" cy="12" r="10" />
              <polyline points="12 6 12 12 16 14" />
            </>
          )}
        </svg>
      </button>

      {/* Backdrop */}
      <Show when={historyDrawerOpen()}>
        <div class="history-drawer-backdrop" onClick={() => setHistoryDrawerOpen(false)} />
      </Show>

      {/* Drawer panel */}
      <div class={`history-drawer${historyDrawerOpen() ? " history-drawer-open" : ""}`}>
        {/* Header */}
        <div class="history-drawer-header">
          <h3 class="history-drawer-title">
            {t("historyPanel")}
            <Show when={projectName()}>
              <span class="history-drawer-project-name">
                {projectName()}
              </span>
            </Show>
          </h3>
          <span class="history-drawer-count">
            {messages().length} {t("historyMsgCount")}
          </span>
          <button
            class="history-drawer-close"
            onClick={() => setHistoryDrawerOpen(false)}
            title={t("removeCitation")}
          >
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <path d="M18 6L6 18" />
              <path d="M6 6l12 12" />
            </svg>
          </button>
        </div>

        {/* Message list — clean conversation view */}
        <div class="history-drawer-body" ref={scrollRef} onScroll={handleScroll}>
          <Show when={hasMore()}>
            <div class="history-drawer-loading-indicator">
              {loading() ? (t("loading")) : ""}
            </div>
          </Show>

          <Show
            when={messages().length > 0}
            fallback={
              <div class="history-drawer-empty">
                {loading() ? (t("loading")) : (t("noSessions"))}
              </div>
            }
          >
            <For each={messages()}>
              {(item) => (
                <div
                  class={`history-drawer-item history-item-${item.role}`}
                  onPointerDown={(e) => onItemPointerDown(e, item)}
                  title={t("dragToChat")}
                >
                  <div class="history-drawer-item-meta">
                    <span class={`history-drawer-item-role role-${item.role}`}>
                      {item.role === "user" ? t("roleYou") : t("roleAi")}
                    </span>
                    <span class="history-drawer-item-ts">{formatTs(item.ts)}</span>
                  </div>
                  <div class="history-drawer-item-content">
                    {previewContent(item.content)}
                  </div>
                </div>
              )}
            </For>
          </Show>
        </div>
      </div>
    </>
  );
};

export default HistoryDrawer;
