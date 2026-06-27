import { createSignal, onCleanup, Show, type Component } from "solid-js";
import { api, tauriAvailable } from "../tauri-api";
import type { DreamSession, DreamSummary } from "../tauri-api";
import { connected } from "../store";
import { t } from "../i18n";

type DreamPhase = "idle" | "running" | "complete";

// 导出 getter:活的 IDE 的 PANELS 注册表(ui-actions.ts)读它判可见态。
export const [dreamOpen, setDreamOpen] = createSignal(false);
const [phase, setPhase] = createSignal<DreamPhase>("idle");
const [session, setSession] = createSignal<DreamSession | null>(null);
const [summary, setSummary] = createSignal<DreamSummary | null>(null);

export function openDreamPanel() {
  setDreamOpen(true);
}

export function closeDreamPanel() {
  setDreamOpen(false);
  if (phase() === "complete") {
    // Reset for next use
    setPhase("idle");
    setSession(null);
    setSummary(null);
  }
}

export function toggleDreamPanel() {
  if (dreamOpen()) {
    closeDreamPanel();
  } else {
    openDreamPanel();
  }
}

export function isDreamActive(): boolean {
  return phase() === "running";
}

let pollTimer: ReturnType<typeof setInterval> | undefined;

async function startDream() {
  if (!tauriAvailable() || !connected()) return;
  setPhase("running");
  setSummary(null);
  try {
    const sess = await api.dreamStart();
    setSession(sess);
    if (sess.is_complete) {
      // No fragments to process
      setPhase("complete");
      return;
    }
    // Poll for status
    pollTimer = setInterval(() => void pollStatus(), 2000);
  } catch {
    setPhase("idle");
  }
}

async function pollStatus() {
  if (!tauriAvailable() || !connected()) return;
  try {
    const result = await api.dreamStatus();
    setSummary(result);
    // dream_status runs all steps synchronously so it's always complete
    setPhase("complete");
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = undefined;
    }
  } catch {
    // keep polling
  }
}

const DreamPanel: Component = () => {
  onCleanup(() => {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = undefined;
    }
  });

  const sess = () => session();
  const sum = () => summary();
  const progressPct = () => {
    const s = sess();
    if (!s || s.total_fragments === 0) return 0;
    return Math.min(100, (s.processed / s.total_fragments) * 100);
  };

  return (
    <Show when={dreamOpen()}>
      <div class="dream-panel">
        <div class="dream-header">
          <h3>{t("dreamTitle")}</h3>
          <button class="dream-close" onClick={closeDreamPanel}>&times;</button>
        </div>
        <div class="dream-body">
          <Show when={phase() === "idle"}>
            <p class="dream-info">
              {t("dreamInfo")}
            </p>
            <button class="dream-start primary" onClick={() => void startDream()}>
              {t("dreamStartBtn")}
            </button>
          </Show>

          <Show when={phase() === "running"}>
            <p class="dream-status-text">
              {t("dreamRunning")} {sess()?.processed ?? 0}/{sess()?.total_fragments ?? 0}
            </p>
            <div class="dream-progress-track">
              <div
                class="dream-progress-fill"
                style={{ width: `${progressPct()}%` }}
              />
            </div>
          </Show>

          <Show when={phase() === "complete"}>
            <div class="dream-complete">
              <p class="dream-done-label">{t("dreamComplete")}</p>
              <Show when={sum()}>
                <div class="dream-summary">
                  <div class="dream-row">
                    <span class="dream-label">{t("dreamFragmentsProcessed")}</span>
                    <span class="dream-value">{sum()!.total_fragments}</span>
                  </div>
                  <div class="dream-row">
                    <span class="dream-label">{t("dreamProcessed")}</span>
                    <span class="dream-value">{sum()!.processed}</span>
                  </div>
                  <div class="dream-row">
                    <span class="dream-label">{t("dreamDiscoveries")}</span>
                    <span class="dream-value">{sum()!.total_discoveries}</span>
                  </div>
                  <div class="dream-row">
                    <span class="dream-label">{t("dreamDuration")}</span>
                    <span class="dream-value">{sum()!.duration_ms}ms</span>
                  </div>
                </div>
              </Show>
              <button class="dream-start" onClick={() => { setPhase("idle"); setSummary(null); setSession(null); }}>
                {t("dreamRunAgain")}
              </button>
            </div>
          </Show>
        </div>
      </div>
    </Show>
  );
};

export default DreamPanel;
