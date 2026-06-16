import {
  createSignal,
  onCleanup,
  onMount,
  Show,
  type Component,
  type JSX,
} from "solid-js";
import {
  healthMonitorOpen,
  setHealthMonitorOpen,
  statusInfo,
  connected,
  model as modelSig,
  connectedAt,
} from "../store";
import { api, type ControlState, type MemoryStats } from "../tauri-api";
import { t } from "../i18n";
import { sfx } from "../sfx";

const POLL_MS = 2_500;

const IconPlug: Component = () => (
  <svg viewBox="0 0 16 16" class="shm-icon" aria-hidden="true">
    <path d="M 3 8 L 9 8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
    <rect x="9" y="5.5" width="3.5" height="5" rx="0.8" stroke="currentColor" stroke-width="1.2" fill="none" />
    <path d="M 5 6 L 5 10 M 7 6 L 7 10" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
    <path d="M 12.5 7 L 14 7 M 12.5 9 L 14 9" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" />
  </svg>
);
const IconAlert: Component = () => (
  <svg viewBox="0 0 16 16" class="shm-icon" aria-hidden="true">
    <path d="M 8 2.5 L 14 13 L 2 13 Z" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linejoin="round" />
    <path d="M 8 6.5 L 8 9.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
    <circle cx="8" cy="11.2" r="0.7" fill="currentColor" />
  </svg>
);
const IconGauge: Component = () => (
  <svg viewBox="0 0 16 16" class="shm-icon" aria-hidden="true">
    <path d="M 2.5 11 A 6 6 0 0 1 13.5 11" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linecap="round" />
    <path d="M 8 11 L 11 6.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
    <circle cx="8" cy="11" r="1.1" fill="currentColor" />
  </svg>
);
const IconJson: Component = () => (
  <svg viewBox="0 0 16 16" class="shm-icon" aria-hidden="true">
    <path d="M 5.5 3 C 4 3 3 4 3 5.5 L 3 7 C 3 7.8 2.4 8 2 8 C 2.4 8 3 8.2 3 9 L 3 10.5 C 3 12 4 13 5.5 13"
      stroke="currentColor" stroke-width="1.2" fill="none" stroke-linecap="round" stroke-linejoin="round" />
    <path d="M 10.5 3 C 12 3 13 4 13 5.5 L 13 7 C 13 7.8 13.6 8 14 8 C 13.6 8 13 8.2 13 9 L 13 10.5 C 13 12 12 13 10.5 13"
      stroke="currentColor" stroke-width="1.2" fill="none" stroke-linecap="round" stroke-linejoin="round" />
  </svg>
);
const IconShield: Component = () => (
  <svg viewBox="0 0 16 16" class="shm-icon" aria-hidden="true">
    <path d="M 8 2 L 13 4 L 13 8.5 C 13 11 11 13 8 14 C 5 13 3 11 3 8.5 L 3 4 Z"
      stroke="currentColor" stroke-width="1.2" fill="none" stroke-linejoin="round" />
    <path d="M 6 8 L 7.5 9.5 L 10 6.5" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linecap="round" stroke-linejoin="round" />
  </svg>
);
const IconBrain: Component = () => (
  <svg viewBox="0 0 16 16" class="shm-icon" aria-hidden="true">
    <circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="1.2" fill="none" />
    <path d="M 5 6 Q 8 4 11 6 M 5 10 Q 8 8 11 10" stroke="currentColor" stroke-width="1" fill="none" />
    <circle cx="8" cy="8" r="1.2" fill="currentColor" />
  </svg>
);
const IconMoon: Component = () => (
  <svg viewBox="0 0 16 16" class="shm-icon" aria-hidden="true">
    <path d="M 10 2 A 6 6 0 1 0 14 10 A 4.5 4.5 0 0 1 10 2 Z"
      stroke="currentColor" stroke-width="1.2" fill="none" />
  </svg>
);

function fatigueColor(v: number): string {
  if (v < 0.3) return "#4ade80";
  if (v <= 0.7) return "#facc15";
  return "#ef4444";
}

// token 数紧凑成 k/m:12000→"12k",256000→"256k",1000000→"1.0m"。
function fmtCtxTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}m`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
  return String(n);
}

function fmtUptime(ms: number): string {
  if (ms < 0) ms = 0;
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${sec}s`;
  return `${sec}s`;
}

interface MetricRowProps {
  icon: () => JSX.Element;
  label: string;
  value: () => JSX.Element;
  hint?: () => JSX.Element;
}

const MetricRow: Component<MetricRowProps> = (p) => (
  <div class="shm-row">
    <span class="shm-row-icon">{p.icon()}</span>
    <span class="shm-row-label">{p.label}</span>
    <span class="shm-row-value">{p.value()}</span>
    <Show when={p.hint}>
      <span class="shm-row-hint">{p.hint!()}</span>
    </Show>
  </div>
);

const SystemHealthMonitor: Component = () => {
  const [ctrl, setCtrl] = createSignal<ControlState | null>(null);
  const [memStats, setMemStats] = createSignal<MemoryStats | null>(null);
  const [dreamSummary, setDreamSummary] = createSignal<{ session_id: string; total_fragments: number; processed: number; duration_ms: number } | null>(null);
  const [now, setNow] = createSignal<number>(Date.now());

  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let tickTimer: ReturnType<typeof setInterval> | null = null;

  async function refresh(): Promise<void> {
    try {
      const [ctrlData, mem, dream] = await Promise.all([
        api.getControlState(),
        api.getMemoryStats().catch(() => null),
        api.dreamStatus().catch(() => null),
      ]);
      setCtrl(ctrlData);
      setMemStats(mem);
      setDreamSummary(dream);
    } catch {
      // silent
    }
  }

  onMount(() => {
    void refresh();
    pollTimer = setInterval(() => void refresh(), POLL_MS);
    tickTimer = setInterval(() => setNow(Date.now()), 1000);
  });
  onCleanup(() => {
    if (pollTimer) clearInterval(pollTimer);
    if (tickTimer) clearInterval(tickTimer);
  });

  const uptimeLabel = (): string => {
    const at = connectedAt();
    if (!connected() || !at) return t("shmOffline") || "未连接";
    return fmtUptime(now() - at);
  };

  const handleBackdrop = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("shm-backdrop")) {
      setHealthMonitorOpen(false);
    }
  };

  return (
    <Show when={healthMonitorOpen()}>
      <div class="shm-overlay visible" onClick={handleBackdrop}>
        <div class="shm-backdrop" />
        <div class="shm-panel">
          <div class="shm-header">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M22 12h-4l-3 9L9 3l-3 9H2" />
            </svg>
            <span class="shm-title">{t("healthMonitor") || "系统健康监控"}</span>
            <button class="shm-close" onClick={() => { sfx.tap(); setHealthMonitorOpen(false); }}>×</button>
          </div>

          <div class="shm-body">
            {/* ── 运行 ── */}
            <div class="shm-section">{t("regionRuntime") || "运行"}</div>
            <MetricRow
              icon={() => <IconPlug />}
              label={t("shmConnection") || "连接"}
              value={() => (
                <span class={`shm-conn ${connected() ? "ok" : "off"}`}>
                  <span class="shm-dot" />
                  <span>{connected() ? (modelSig() || "—") : (t("disconnected") || "未连接")}</span>
                </span>
              )}
              hint={() => <span>{uptimeLabel()}</span>}
            />

            {/* 实时上下文压力:最近请求实发上下文 token / 上下文窗口总量 */}
            <MetricRow
              icon={() => <IconGauge />}
              label={t("contextPressure") || "上下文压力"}
              value={() => {
                const tk = statusInfo()?.ctx_prompt_tokens ?? 0;
                const win = statusInfo()?.ctx_window_tokens ?? 0;
                const frac = win > 0 ? Math.min(tk / win, 1) : 0;
                const pct = frac * 100;
                return (
                  <span class="shm-bar-wrap">
                    <span class="shm-bar-track">
                      <span class="shm-bar" style={{ width: `${pct}%`, background: fatigueColor(frac) }} />
                    </span>
                    <span class="shm-mono" style={{ color: fatigueColor(frac) }}>{pct.toFixed(1)}%</span>
                  </span>
                );
              }}
              hint={() => {
                const tk = statusInfo()?.ctx_prompt_tokens ?? 0;
                const win = statusInfo()?.ctx_window_tokens ?? 0;
                return <span>{fmtCtxTokens(tk)} / {fmtCtxTokens(win)} tokens</span>;
              }}
            />

            <MetricRow
              icon={() => <IconAlert />}
              label={t("shmErrorRate") || "错误率"}
              value={() => {
                const pct = ctrl()?.error_rate_pct ?? 0;
                return <span class="shm-mono">{pct.toFixed(1)}%</span>;
              }}
              hint={() => {
                const fail = ctrl()?.tool_call_fail ?? 0;
                const total = ctrl()?.tool_call_total ?? 0;
                return <span>{fail}/{total} {t("shmFailures") || "次失败"}</span>;
              }}
            />

            <MetricRow
              icon={() => <IconJson />}
              label={t("shmJsonCall") || "JSON Call 服从率"}
              value={() => {
                const pct = ctrl()?.json_compliance_pct ?? 0;
                return <span class="shm-mono">{pct.toFixed(0)}%</span>;
              }}
              hint={() => {
                const valid = ctrl()?.json_call_valid ?? 0;
                const total = ctrl()?.json_call_total ?? 0;
                return <span>{valid}/{total} {t("shmValidCalls") || "有效调用"}</span>;
              }}
            />

            <MetricRow
              icon={() => <IconShield />}
              label={t("shmSafety") || "数据保护"}
              value={() => {
                const n = ctrl()?.safety.backup_count ?? 0;
                return <span class="shm-mono">{n}</span>;
              }}
              hint={() => {
                const n = ctrl()?.safety.backup_count ?? 0;
                return <span>{n} {t("shmSafetyHint") || "次备份（file_write 覆写时快照）"}</span>;
              }}
            />

            {/* ── 存放区·临时记忆(唯一的"记忆缓存";RAG/L2 命中都进这里)── */}
            <div class="shm-section">{t("regionStore") || "存放区·临时记忆"}</div>

            {/* 缓存队列(工作区=存放区=置换系统"物理内存"):占用/预算填充度,队列从空涨起,满了是常态,Nap 清零。 */}
            <MetricRow
              icon={() => <IconGauge />}
              label={t("queueOccupancy") || "缓存队列"}
              value={() => {
                const q = memStats()?.queue;
                if (!q) return <span class="shm-mono">—</span>;
                const pct = (q.fill_pct ?? 0) * 100;
                // 占用高=好(上下文塞满相关内容);满了是常态,故高=绿、半满=浅绿、低=黄(刚 Nap/起步)。
                return (
                  <span class="shm-bar-wrap">
                    <span class="shm-bar-track">
                      <span class="shm-bar" style={{ width: `${pct}%`, background: pct > 90 ? "#4ade80" : pct > 50 ? "#86efac" : "#facc15" }} />
                    </span>
                    <span class="shm-mono">{pct.toFixed(1)}%</span>
                  </span>
                );
              }}
              hint={() => {
                const q = memStats()?.queue;
                if (!q) return <span>—</span>;
                return <span>{t("queueResident") || "常驻"} {q.resident ?? 0} · {t("ptrReal") || "真指针"} {q.real_pointers ?? 0} · {t("ptrFake") || "假指针"} {q.fake_pointers ?? 0} · {t("shmEvictions") || "淘汰"} {q.evictions ?? 0}</span>;
              }}
            />

            {/* 记忆置换率:工作区真换出频率 = 缓存队列换出 = 记忆区换出(同一事件,见 tooltip)。 */}
            <MetricRow
              icon={() => <IconGauge />}
              label={t("memReplacement") || "记忆置换率"}
              value={() => {
                const r = statusInfo()?.replacement_rate ?? 0;
                const pct = r * 100;
                return (
                  <span class="shm-bar-wrap">
                    <span class="shm-bar-track">
                      <span class="shm-bar" style={{ width: `${pct}%`, background: fatigueColor(r) }} />
                    </span>
                    <span class="shm-mono" style={{ color: fatigueColor(r) }}>{pct.toFixed(1)}%</span>
                  </span>
                );
              }}
              hint={() => {
                const q = memStats()?.queue;
                return <span title={t("memReplacementCoupling") || ""}>{t("shmEvictions") || "淘汰"} {q?.evictions ?? 0}</span>;
              }}
            />

            {/* 劳累度(命中率/淘汰/碎片的合成,反映存放区+索引区健康) */}
            <MetricRow
              icon={() => <IconBrain />}
              label={t("shmFatigue") || "劳累度"}
              value={() => {
                const f = memStats()?.fatigue;
                if (!f) return <span class="shm-mono">—</span>;
                const pct = f.fatigue_value * 100;
                return (
                  <span class="shm-bar-wrap">
                    <span class="shm-bar-track">
                      <span class="shm-bar" style={{ width: `${pct}%`, background: fatigueColor(f.fatigue_value) }} />
                    </span>
                    <span class="shm-mono" style={{ color: fatigueColor(f.fatigue_value) }}>{pct.toFixed(1)}%</span>
                  </span>
                );
              }}
              hint={() => {
                const f = memStats()?.fatigue;
                if (!f) return <span>—</span>;
                return (
                  <span>
                    {t("shmCacheHitRate") || "命中率"} {(f.cache_hit_rate * 100).toFixed(0)}%
                    · {t("shmEviction") || "淘汰"} {f.eviction_rate.toFixed(0)}
                    · {t("shmFragments") || "碎片"} {f.fragment_count}
                  </span>
                );
              }}
            />

            {/* ── 索引区(找"该调入谁"的指针;RAG/L2 只是索引手段)── */}
            <div class="shm-section">{t("regionIndex") || "索引区"}</div>

            <MetricRow
              icon={() => <IconBrain />}
              label={t("indexDensity") || "索引密度"}
              value={() => {
                const d = statusInfo()?.index_density ?? 0;
                return <span class="shm-mono">{d.toFixed(2)}</span>;
              }}
              hint={() => {
                const p = statusInfo()?.pointer_count ?? 0;
                const n = statusInfo()?.total_nodes ?? 0;
                return <span>{p}/{n} {t("shmPointersNodes")}</span>;
              }}
            />

            <MetricRow
              icon={() => <IconMoon />}
              label={t("shmFragments") || "碎片数"}
              value={() => {
                const n = statusInfo()?.fragment_count ?? 0;
                return <span class="shm-mono">{n}</span>;
              }}
              hint={() => {
                const f = statusInfo()?.fatigue ?? 0;
                return <span>{(f * 100).toFixed(0)}{t("shmFatigueValueSuffix")}</span>;
              }}
            />

            <MetricRow
              icon={() => <IconPlug />}
              label={t("reverseIndexSize") || "逆向索引"}
              value={() => {
                const n = statusInfo()?.reverse_index_size ?? 0;
                return <span class="shm-mono">{n}</span>;
              }}
              hint={() => {
                const n = statusInfo()?.total_nodes ?? 0;
                return <span>{n} {t("shmTotalNodes")}</span>;
              }}
            />

            <MetricRow
              icon={() => <IconGauge />}
              label={t("coverage") || "覆盖率"}
              value={() => {
                const dg = statusInfo()?.coverage_deep_green_pct ?? 0;
                const rd = statusInfo()?.coverage_red_pct ?? 0;
                const gr = statusInfo()?.coverage_gray_pct ?? 0;
                const lg = statusInfo()?.coverage_light_green_pct ?? 0;
                return (
                  <div style="display:flex;gap:4px;width:100%;height:8px;border-radius:4px;overflow:hidden">
                    <div style={{ flex: `${dg}%`, background: "#30d158", "min-width": dg > 0 ? "2px" : "0" }} title={`DeepGreen ${dg.toFixed(0)}%`} />
                    <div style={{ flex: `${lg}%`, background: "#5ecc6e", "min-width": lg > 0 ? "2px" : "0" }} title={`LightGreen ${lg.toFixed(0)}%`} />
                    <div style={{ flex: `${rd}%`, background: "#ff453a", "min-width": rd > 0 ? "2px" : "0" }} title={`Red ${rd.toFixed(0)}%`} />
                    <div style={{ flex: `${gr}%`, background: "#48484a", "min-width": gr > 0 ? "2px" : "0" }} title={`Gray ${gr.toFixed(0)}%`} />
                  </div>
                );
              }}
              hint={() => {
                const dg = statusInfo()?.coverage_deep_green_pct ?? 0;
                const rd = statusInfo()?.coverage_red_pct ?? 0;
                const gr = statusInfo()?.coverage_gray_pct ?? 0;
                return <span>{t("shmCoverageBreakdown").replace("{dg}", dg.toFixed(0)).replace("{rd}", rd.toFixed(0)).replace("{gr}", gr.toFixed(0))}</span>;
              }}
            />

            {/* 做梦整理(碎片回收:二次指针升一次、还扫描债)→ 索引区维护 */}
            <MetricRow
              icon={() => <IconMoon />}
              label={t("shmDreamStatus") || "做梦整理"}
              value={() => {
                const d = dreamSummary();
                if (!d) return <span class="shm-mono">{t("shmNoDream") || "无记录"}</span>;
                return <span class="shm-mono">{d.processed}/{d.total_fragments}</span>;
              }}
              hint={() => {
                const d = dreamSummary();
                if (!d) return <span>—</span>;
                return <span>{t("shmDuration") || "耗时"} {d.duration_ms}ms</span>;
              }}
            />
          </div>
        </div>
      </div>
    </Show>
  );
};

export default SystemHealthMonitor;
