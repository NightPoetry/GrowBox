// 全局 toast 队列 + showToast() 函数。
// CSS 类来自 v1 index.css `.toast-container/.toast/.toast.error/...`。

import { createSignal } from "solid-js";

export type ToastKind = "info" | "success" | "warn" | "error";

export interface Toast {
  id: number;
  kind: ToastKind;
  text: string;
  fadingOut: boolean;
}

const [toasts, setToasts] = createSignal<Toast[]>([]);
let seq = 0;

export function showToast(text: string, kind: ToastKind = "info", ttlMs: number = 3000): void {
  if (toasts().some((t) => t.text === text && !t.fadingOut)) return;
  const id = ++seq;
  setToasts((arr) => [...arr, { id, kind, text, fadingOut: false }]);
  window.setTimeout(() => {
    setToasts((arr) => arr.map((t) => (t.id === id ? { ...t, fadingOut: true } : t)));
    window.setTimeout(() => {
      setToasts((arr) => arr.filter((t) => t.id !== id));
    }, 320);
  }, ttlMs);
}

export function getToasts() {
  return toasts();
}
