import { createSignal, Show, createEffect, onCleanup, type Component } from "solid-js";
import { api, tauriAvailable } from "../tauri-api";
import type { MemoryStats } from "../tauri-api";
import { connected } from "../store";
import { t } from "../i18n";

// 导出 getter:活的 IDE 的 PANELS 注册表(ui-actions.ts)读它判可见态。
export const [memoryVizOpen, setMemoryVizOpen] = createSignal(false);
const [stats, setStats] = createSignal<MemoryStats | null>(null);
const [loading, setLoading] = createSignal(false);

export function openMemoryViz() {
  setMemoryVizOpen(true);
  void fetchStats();
}

export function closeMemoryViz() {
  setMemoryVizOpen(false);
}

export function toggleMemoryViz() {
  if (memoryVizOpen()) {
    closeMemoryViz();
  } else {
    openMemoryViz();
  }
}

async function fetchStats() {
  if (!tauriAvailable() || !connected()) return;
  setLoading(true);
  try {
    const data = await api.getMemoryStats();
    setStats(data);
  } catch {
    // ignore
  } finally {
    setLoading(false);
  }
}

function pct(v: number): string {
  return (v * 100).toFixed(1) + "%";
}

function fatigueColor(v: number): string {
  if (v < 0.3) return "#4ade80";
  if (v <= 0.7) return "#facc15";
  return "#ef4444";
}

const MemoryViz: Component = () => {
  const data = () => stats();
  let panelEl: HTMLDivElement | undefined;

  // 点击面板外的空白即关闭(与其他面板一致,用户要求 2026-06-02)。
  // 仅在打开时挂 document 监听;setTimeout(0) 延后注册,避开"打开它的那次点击"自身。
  createEffect(() => {
    if (!memoryVizOpen()) return;
    const onDocMouseDown = (e: MouseEvent) => {
      if (panelEl && !panelEl.contains(e.target as Node)) closeMemoryViz();
    };
    const id = window.setTimeout(() => document.addEventListener("mousedown", onDocMouseDown), 0);
    onCleanup(() => {
      window.clearTimeout(id);
      document.removeEventListener("mousedown", onDocMouseDown);
    });
  });

  return (
    <Show when={memoryVizOpen()}>
      <div class="memviz-panel" ref={panelEl}>
        <div class="memviz-header">
          <h3>{t("memvizTitle")}</h3>
          <button class="memviz-refresh" onClick={() => void fetchStats()} disabled={loading()}>
            {loading() ? "..." : "↻"}
          </button>
          <button class="memviz-close" onClick={closeMemoryViz}>&times;</button>
        </div>
        <Show when={data()} fallback={<p class="memviz-empty">{t("memvizNoData")}</p>}>
          <div class="memviz-body">
            {/* -- Node counts -- */}
            <div class="memviz-section">
              <div class="memviz-section-title">{t("memvizNodesPointers")}</div>
              <div class="memviz-row">
                <span class="memviz-label">{t("ctrlTotalNodes")}</span>
                <span class="memviz-value">{data()!.total_nodes}</span>
              </div>
              <div class="memviz-row">
                <span class="memviz-label">{t("ctrlTotalPointers")}</span>
                <span class="memviz-value">{data()!.total_pointers}</span>
              </div>
            </div>

            {/* -- 队列占用(工作区=置换系统的"物理内存",真置换在此;满了是常态,Nap 清零)-- */}
            <div class="memviz-section">
              <div class="memviz-section-title">{t("queueOccupancy")}</div>
              <div class="memviz-cache-tiers">
                <div class="memviz-tier">
                  <span class="memviz-tier-label">{t("cacheUsage")}</span>
                  <div class="memviz-bar-track">
                    <div class="memviz-bar-fill" style={{ width: `${Math.min(100, data()!.queue.fill_pct * 100)}%` }} />
                  </div>
                  <span class="memviz-tier-count">{(data()!.queue.fill_pct * 100).toFixed(1)}%</span>
                </div>
              </div>
              <div class="memviz-row">
                <span class="memviz-label">{t("queueResident")}</span>
                <span class="memviz-value">{data()!.queue.resident}</span>
              </div>
              <div class="memviz-row">
                <span class="memviz-label">{t("memvizEvictions")}</span>
                <span class="memviz-value">{data()!.queue.evictions}</span>
              </div>
              {/* 队列里真指针(L2/精确层命中)/ 假指针(RAG 命中,不落序列)各几条。统一概念 2026-06-16。 */}
              <div class="memviz-row">
                <span class="memviz-label">{t("ptrReal")} / {t("ptrFake")}</span>
                <span class="memviz-value">{data()!.queue.real_pointers} / {data()!.queue.fake_pointers}</span>
              </div>
            </div>

            {/* L2 导航缓存=纯内部加速层(页表层翻图缓存),非记忆缓存,不再作为面板指标显示。
                真·临时记忆 = 上面的「缓存队列」(工作区/存放区);检索到的内容不论 RAG/L2 都 page_in 进那里。 */}

            {/* -- Fatigue -- */}
            <div class="memviz-section">
              <div class="memviz-section-title">{t("shmFatigue")}</div>
              <div class="memviz-fatigue-bar">
                <div class="memviz-bar-track">
                  <div
                    class="memviz-bar-fill"
                    style={{
                      width: pct(data()!.fatigue.fatigue_value),
                      background: fatigueColor(data()!.fatigue.fatigue_value),
                    }}
                  />
                </div>
                <span
                  class="memviz-fatigue-value"
                  style={{ color: fatigueColor(data()!.fatigue.fatigue_value) }}
                >
                  {pct(data()!.fatigue.fatigue_value)}
                </span>
              </div>
              <div class="memviz-row">
                <span class="memviz-label">{t("shmCacheHitRate")}</span>
                <span class="memviz-value">{pct(data()!.fatigue.cache_hit_rate)}</span>
              </div>
              <div class="memviz-row">
                <span class="memviz-label">{t("memvizEvictionRate")}</span>
                <span class="memviz-value">{data()!.fatigue.eviction_rate.toFixed(0)}</span>
              </div>
              <div class="memviz-row">
                <span class="memviz-label">{t("shmFragments")}</span>
                <span class="memviz-value">{data()!.fatigue.fragment_count}</span>
              </div>
            </div>

            {/* -- Secondary indexes -- */}
            <div class="memviz-section">
              <div class="memviz-section-title">{t("memvizSecondaryIndexes")}</div>
              <div class="memviz-row">
                <span class="memviz-label">{t("memvizTotal")}</span>
                <span class="memviz-value">{data()!.secondary_indexes.total}</span>
              </div>
              <div class="memviz-row">
                <span class="memviz-label">{t("memvizForcedJumps")}</span>
                <span class="memviz-value">{data()!.secondary_indexes.forced_jumps}</span>
              </div>
              <div class="memviz-row">
                <span class="memviz-label">{t("memvizFragmentCount")}</span>
                <span class="memviz-value">{data()!.secondary_indexes.fragment_count}</span>
              </div>
            </div>
          </div>
        </Show>
      </div>
    </Show>
  );
};

export default MemoryViz;
