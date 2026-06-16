// WebAudio 音效系统（v1 frontend/index.html:863 SFX 原样移植）。
// tap/connected/sent/done/error 是不同质感的两段叠加振荡器，
// muted 状态走 localStorage 持久化。

import { createSignal } from "solid-js";

const STORAGE_KEY = "growbox-mute";

const [muted, setMutedSignal] = createSignal<boolean>(initialMuted());

function initialMuted(): boolean {
  if (typeof localStorage === "undefined") return false;
  return localStorage.getItem(STORAGE_KEY) === "1";
}

let ctx: AudioContext | null = null;

function ensureCtx(): AudioContext {
  if (!ctx) {
    const AC = window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
    ctx = new AC();
  }
  return ctx;
}

function tone(
  freq: number,
  dur: number,
  type: OscillatorType = "sine",
  vol: number = 0.04,
  delay: number = 0,
): void {
  if (muted()) return;
  const c = ensureCtx();
  const o = c.createOscillator();
  const g = c.createGain();
  o.type = type;
  o.frequency.value = freq;
  g.gain.value = vol;
  o.connect(g); g.connect(c.destination);
  const t = c.currentTime + delay;
  o.start(t);
  g.gain.exponentialRampToValueAtTime(0.001, t + dur);
  o.stop(t + dur + 0.01);
}

export const sfx = {
  isMuted: muted,
  tap() { tone(3200, 0.025, "square", 0.03); tone(1800, 0.04, "sine", 0.03, 0.01); },
  connected() { tone(1000, 0.08, "sine", 0.05); tone(1200, 0.08, "sine", 0.05, 0.08); tone(1500, 0.1, "sine", 0.05, 0.16); },
  sent() { tone(2000, 0.025, "sine", 0.03); },
  done() { tone(800, 0.07, "triangle", 0.05); tone(1100, 0.09, "triangle", 0.05, 0.07); },
  error() { tone(500, 0.06, "sawtooth", 0.04); tone(300, 0.1, "sawtooth", 0.04, 0.06); },
  toggle() {
    const next = !muted();
    setMutedSignal(next);
    if (typeof localStorage !== "undefined") {
      localStorage.setItem(STORAGE_KEY, next ? "1" : "0");
    }
    if (!next) {
      // 取消静音时给一声 tap 反馈
      tone(3200, 0.025, "square", 0.03);
    }
  },
};
