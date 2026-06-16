// ============================================================
// GrowBox UI Debug Bridge — 注入 window.__GROWBOX__
// ------------------------------------------------------------
// 后端通过 Tauri window.eval() 或 debug_ui_* 工具调用此全局对象，
// 让 AI 能远程检查/操控 UI 状态，不需要人当肉调试器。
// ============================================================

interface DOMInfo {
  tag: string;
  id: string;
  class: string;
  text: string;
  visible: boolean;
  rect: { x: number; y: number; w: number; h: number };
}

interface LogEntry {
  ts: string;
  level: string;
  msg: string;
}

interface StateSnapshot {
  connected: boolean;
  sending: boolean;
  settingsOpen: boolean;
  currentProjectId: string | null;
  messageCount: number;
  lastMessage: string;
  statusInfo: Record<string, unknown>;
  errors: string[];
}

interface TestReport {
  pass: number;
  fail: number;
  checks: { name: string; ok: boolean; detail: string }[];
}

import { listen } from "./tauri-api";
import { getToasts } from "./toast";
// 截图引擎:原生逐节点绘制(非 SVG foreignObject——后者在 WKWebView 会污染 canvas,
// toDataURL 抛 SecurityError,真机实测)。本模块是 VITE_GROWBOX_DEBUG 动态 chunk,
// html2canvas 只进测试包,正式构建摇树不带。
import html2canvas from "html2canvas";

const _consoleBuf: LogEntry[] = [];
const MAX_CONSOLE = 200;

function _ts(): string {
  return new Date().toISOString().slice(11, 23);
}

// ---- 后端事件埋点缓冲(全自动调试:外部经 /eval 读 getEvents 即知一回合发生了什么) ----
interface EvtEntry { ts: string; event: string; payload: string }
const _evtBuf: EvtEntry[] = [];
const MAX_EVT = 300;
const WATCH_EVENTS = [
  "chat-status", "decision-request", "notice", "ui-action",
  "context-tokens", "terminal-open", "web-debug-edit", "project-switched",
];
for (const ev of WATCH_EVENTS) {
  void listen<unknown>(ev, (p) => {
    _evtBuf.push({ ts: _ts(), event: ev, payload: JSON.stringify(p)?.slice(0, 400) ?? "" });
    if (_evtBuf.length > MAX_EVT) _evtBuf.shift();
  }).catch(() => { /* 浏览器环境无桥:埋点不可用,其余功能照常 */ });
}

// ---- console interceptor ----
function _installConsoleHook(): void {
  const orig = { log: console.log, warn: console.warn, error: console.error };
  function _push(level: string, args: unknown[]): void {
    _consoleBuf.push({ ts: _ts(), level, msg: args.map(String).join(' ') });
    if (_consoleBuf.length > MAX_CONSOLE) _consoleBuf.shift();
  }
  console.log = (...a: unknown[]) => { _push('log', a); orig.log(...a); };
  console.warn = (...a: unknown[]) => { _push('warn', a); orig.warn(...a); };
  console.error = (...a: unknown[]) => { _push('error', a); orig.error(...a); };
}
_installConsoleHook();

// ---- DOM helpers ----
function _query(sel: string): Element | null {
  try { return document.querySelector(sel); } catch { return null; }
}

function _domInfo(el: Element): DOMInfo {
  const r = el.getBoundingClientRect();
  const style = window.getComputedStyle(el);
  return {
    tag: el.tagName.toLowerCase(),
    id: el.id || '',
    class: el.className?.toString() || '',
    text: el.textContent?.slice(0, 200) || '',
    visible: style.display !== 'none' && style.visibility !== 'hidden' && r.width > 0,
    rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) },
  };
}

// ---- exported global ----
;(window as any).__GROWBOX__ = {
  // ── 状态快照 ──
  getState(): StateSnapshot {
    const msgs: string[] = [];
    try {
      // SolidJS store is imported in store.ts — access via module-level variable
      const root = document.getElementById('root');
      const textEls = root?.querySelectorAll('[class*="message"]') || [];
      textEls.forEach(el => { const t = el.textContent?.trim().slice(0, 100); if (t) msgs.push(t); });
    } catch { /* ignore */ }
    return {
      connected: !!(document.querySelector('[class*="connected"]')),
      sending: !!(document.querySelector('[class*="sending"]')),
      settingsOpen: !!(document.querySelector('[class*="settings-panel"]')),
      currentProjectId: localStorage.getItem('currentProjectId'),
      messageCount: msgs.length,
      lastMessage: msgs[msgs.length - 1] || '',
      statusInfo: {},
      errors: _consoleBuf.filter(e => e.level === 'error').map(e => e.msg).slice(-10),
    };
  },

  // ── DOM 检查 ──
  getDOM(selector: string): DOMInfo[] {
    const els = document.querySelectorAll(selector);
    return Array.from(els).map(_domInfo);
  },

  // ── 元素测量 ──
  measureElement(selector: string): { found: boolean; rect?: { x: number; y: number; w: number; h: number } } {
    const el = _query(selector);
    if (!el) return { found: false };
    const r = el.getBoundingClientRect();
    return { found: true, rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) } };
  },

  // ── 样式检查 ──
  getComputedStyles(selector: string): Record<string, string> | null {
    const el = _query(selector);
    if (!el) return null;
    const s = window.getComputedStyle(el);
    const keys = ['display','visibility','width','height','color','background-color','font-size','margin','padding','border','position','opacity','z-index','overflow','flex-direction','align-items','justify-content'];
    const out: Record<string, string> = {};
    for (const k of keys) out[k] = s.getPropertyValue(k);
    return out;
  },

  // ── 交互模拟 ──
  clickElement(selector: string): boolean {
    const el = _query(selector) as HTMLElement | null;
    if (!el) return false;
    el.click();
    return true;
  },

  typeText(selector: string, text: string): boolean {
    const el = _query(selector) as HTMLInputElement | HTMLTextAreaElement | null;
    if (!el || !('value' in el)) return false;
    (el as HTMLInputElement).value = text;
    el.dispatchEvent(new Event('input', { bubbles: true }));
    return true;
  },

  scrollTo(selector: string): boolean {
    const el = _query(selector);
    if (!el) return false;
    el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    return true;
  },

  focus(selector: string): boolean {
    const el = _query(selector) as HTMLElement | null;
    if (!el || typeof el.focus !== 'function') return false;
    el.focus();
    return true;
  },

  // ── 存储 ──
  getStorage(key: string): string | null {
    return localStorage.getItem(key) || sessionStorage.getItem(key);
  },

  // ── 控制台日志 ──
  getConsoleLogs(n: number): LogEntry[] {
    const start = Math.max(0, _consoleBuf.length - (n || 50));
    return _consoleBuf.slice(start);
  },

  // ── 事件埋点 ──
  getEvents(n: number): EvtEntry[] {
    const start = Math.max(0, _evtBuf.length - (n || 50));
    return _evtBuf.slice(start);
  },

  // ── 当前在屏 toast(瞬态,配合 getEvents 的 notice 流看历史) ──
  getToasts(): { kind: string; text: string }[] {
    return getToasts().map((t) => ({ kind: t.kind, text: t.text }));
  },

  // ── 等待条件成真(全自动调试的节拍器;jsExpr 求值真值即返回) ──
  async waitFor(jsExpr: string, timeoutMs = 8000, intervalMs = 200): Promise<{ ok: boolean; value?: unknown; waitedMs: number }> {
    const t0 = Date.now();
    for (;;) {
      let v: unknown = null;
      try { v = new Function(`return (${jsExpr});`)(); } catch { v = null; }
      if (v) return { ok: true, value: v, waitedMs: Date.now() - t0 };
      if (Date.now() - t0 > timeoutMs) return { ok: false, waitedMs: Date.now() - t0 };
      await new Promise((r) => setTimeout(r, intervalMs));
    }
  },

  // ── 内部截图(非系统截图,零授权):html2canvas 原生逐节点绘制 → PNG dataUrl。
  // 外部存 dataUrl 为 .png 即可"亲眼看" UI。跨源 iframe 内容不渲(沙箱造物画布是边界);
  // 失败诚实回 error,不假装截到了。
  async screenshot(maxW = 1400): Promise<{ ok: boolean; dataUrl?: string; w?: number; h?: number; error?: string }> {
    try {
      const vw = window.innerWidth, vh = window.innerHeight;
      const scale = Math.min(1, maxW / vw);
      const bg = window.getComputedStyle(document.body).backgroundColor;
      const canvas = await html2canvas(document.body, {
        scale,
        width: vw,
        height: vh,
        windowWidth: vw,
        windowHeight: vh,
        backgroundColor: bg && bg !== "rgba(0, 0, 0, 0)" ? bg : "#1a1a1a",
        logging: false,
        useCORS: false,
      });
      return { ok: true, dataUrl: canvas.toDataURL("image/png"), w: canvas.width, h: canvas.height };
    } catch (e) {
      return { ok: false, error: String(e) };
    }
  },

  // ── 完整 UI 健康检查 ──
  runFullTest(): TestReport {
    const checks: { name: string; ok: boolean; detail: string }[] = [];

    // 1. 根元素存在
    const root = document.getElementById('root');
    checks.push({ name: 'root element', ok: !!root, detail: root ? 'found' : 'MISSING' });

    // 2. 侧边栏可见
    const sidebar = _query('[class*="sidebar"]');
    checks.push({ name: 'sidebar visible', ok: !!(sidebar && _domInfo(sidebar).visible), detail: sidebar ? _domInfo(sidebar).rect.w + 'px wide' : 'no sidebar' });

    // 3. 聊天区域
    const chat = _query('[class*="chat"]');
    checks.push({ name: 'chat area', ok: !!chat, detail: chat ? _domInfo(chat).rect.h + 'px tall' : 'MISSING' });

    // 4. 状态栏
    const status = _query('[class*="status"]');
    checks.push({ name: 'status bar', ok: !!status, detail: status ? 'found' : 'MISSING' });

    // 5. 控制面板
    const panel = _query('[class*="control-panel"]');
    checks.push({ name: 'control panel', ok: !!panel, detail: panel ? 'found' : 'MISSING' });

    // 6. 输入框
    const textarea = _query('textarea, input[type="text"]');
    checks.push({ name: 'text input', ok: !!textarea, detail: textarea ? textarea.tagName : 'MISSING' });

    // 7. 发送按钮
    const sendBtn = _query('button[class*="send"]');
    checks.push({ name: 'send button', ok: !!sendBtn, detail: sendBtn ? 'found' : 'MISSING' });

    // 8. 连接按钮
    const connectBtn = _query('button[class*="connect"]');
    checks.push({ name: 'connect button', ok: !!connectBtn, detail: connectBtn ? 'found' : 'MISSING' });

    // 9. 无 JS 错误
    const recentErrors = _consoleBuf.filter(e => e.level === 'error').length;
    checks.push({ name: 'no recent errors', ok: recentErrors === 0, detail: recentErrors + ' errors in console' });

    const pass = checks.filter(c => c.ok).length;
    const fail = checks.filter(c => !c.ok).length;
    return { pass, fail, checks };
  },
};
