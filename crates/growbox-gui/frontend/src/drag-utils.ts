// 跨平台引用拖拽 —— 统一 Pointer Events 实现。
//
// 为什么不用 HTML5 DnD:macOS WKWebView(Tauri/wry 的 webview)对 HTML5
// draggable / dataTransfer 支持残缺,Tauri 还会劫持原生 drag-drop 事件用于文件拖入。
// 指针事件(pointerdown/move/up)在所有平台 + 触摸上都可靠,故全程自绘拖拽。
//
// 命中判定用目标元素的 getBoundingClientRect 几何包含,而非 elementFromPoint——
// 后者会被历史抽屉那层全屏 backdrop(z-index 100)截胡,导致永远落不到 .chat-area
// (这正是上一版"点击能引用、拖拽不能"的真实原因,与打包 tree-shaking 无关)。

import { t } from "./i18n";

const DRAG_THRESHOLD = 6; // px,移动超过才算拖拽,否则视为点击
const DROP_TARGET = ".chat-area"; // 落点目标(整个对话区都可接收)
const TARGET_CLASS = "citation-target"; // 拖拽悬停时给目标加的高亮类(独立于文件拖入的 drag-over)

export interface CitationDragItem {
  session_id: string;
  ts: string;
  role: string;
  content: string;
}

export interface CitationDragHandlers {
  onDrop: (item: CitationDragItem) => void;
  onClick?: (item: CitationDragItem) => void;
}

function buildGhost(item: CitationDragItem): HTMLDivElement {
  const g = document.createElement("div");
  g.className = "citation-ghost";

  const role = document.createElement("span");
  role.className =
    "citation-ghost-role " + (item.role === "user" ? "role-user" : "role-assistant");
  role.textContent = item.role === "user" ? t("roleYou") : t("roleAi");

  const text = document.createElement("span");
  text.className = "citation-ghost-text";
  text.textContent = item.content.slice(0, 60);

  g.appendChild(role);
  g.appendChild(text);
  document.body.appendChild(g);
  return g;
}

/** 坐标是否落在落点目标矩形内 —— 几何判定,不受层叠/遮挡影响。 */
function targetAt(x: number, y: number): Element | null {
  const el = document.querySelector(DROP_TARGET);
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom ? el : null;
}

/** 落点处的小脉冲反馈。 */
function flashDrop(x: number, y: number): void {
  const p = document.createElement("div");
  p.className = "citation-drop-pulse";
  p.style.left = x + "px";
  p.style.top = y + "px";
  document.body.appendChild(p);
  setTimeout(() => p.remove(), 460);
}

/**
 * 启动一次引用交互(点击 or 拖拽,跨平台)。
 * 调用方在 `onPointerDown` 时调用;内部按移动阈值自动区分点击与拖拽,
 * 分别回调 `onClick` / `onDrop`,避免与原生 click 重复触发。
 */
export function startCitationDrag(
  e: PointerEvent | MouseEvent,
  item: CitationDragItem,
  handlers: CitationDragHandlers,
): void {
  const startX = e.clientX;
  const startY = e.clientY;
  // 被拖出的源条目:拖拽期间给它"被拎起"的反馈(变暗缩一下)。
  const sourceEl = (e.currentTarget as HTMLElement | null) ?? null;

  let dragging = false;
  let ghost: HTMLDivElement | null = null;
  let highlighted: Element | null = null;

  function setHighlight(el: Element | null): void {
    if (el === highlighted) return;
    highlighted?.classList.remove(TARGET_CLASS);
    highlighted = el;
    highlighted?.classList.add(TARGET_CLASS);
  }

  function placeGhost(x: number, y: number): void {
    if (!ghost) return;
    ghost.style.left = x + "px";
    ghost.style.top = y + "px";
  }

  function begin(x: number, y: number): void {
    dragging = true;
    ghost = buildGhost(item);
    document.body.classList.add("citation-dragging");
    sourceEl?.classList.add("citation-grabbed");
    placeGhost(x, y);
  }

  function onMove(ev: PointerEvent): void {
    if (!dragging) {
      if (Math.hypot(ev.clientX - startX, ev.clientY - startY) < DRAG_THRESHOLD) return;
      begin(ev.clientX, ev.clientY);
    }
    ev.preventDefault();
    placeGhost(ev.clientX, ev.clientY);
    setHighlight(targetAt(ev.clientX, ev.clientY));
  }

  function onUp(ev: PointerEvent): void {
    const wasDragging = dragging;
    const hit = wasDragging ? targetAt(ev.clientX, ev.clientY) : null;
    cleanup();
    if (!wasDragging) {
      handlers.onClick?.(item);
      return;
    }
    if (hit) {
      flashDrop(ev.clientX, ev.clientY);
      handlers.onDrop(item);
    }
  }

  function onKey(ev: KeyboardEvent): void {
    if (ev.key === "Escape") cleanup();
  }

  function cleanup(): void {
    document.removeEventListener("pointermove", onMove);
    document.removeEventListener("pointerup", onUp);
    document.removeEventListener("pointercancel", cleanup);
    document.removeEventListener("keydown", onKey);
    document.body.classList.remove("citation-dragging");
    sourceEl?.classList.remove("citation-grabbed");
    setHighlight(null);
    if (ghost) {
      ghost.remove();
      ghost = null;
    }
  }

  document.addEventListener("pointermove", onMove);
  document.addEventListener("pointerup", onUp);
  document.addEventListener("pointercancel", cleanup);
  document.addEventListener("keydown", onKey);
  e.preventDefault();
}
