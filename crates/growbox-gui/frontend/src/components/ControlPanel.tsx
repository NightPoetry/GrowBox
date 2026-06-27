import { createSignal, For, Show, onMount, onCleanup, type Component } from "solid-js";
import { api, type ControlState } from "../tauri-api";
import { intentEvents, statusInfo } from "../store";
import { t } from "../i18n";
import { openMemoryViz } from "./MemoryViz";

const POLL_MS = 3_000;

const IconIntent: Component = () => (
  <svg class="ctrl-svg-icon" viewBox="0 0 16 16" aria-hidden="true">
    <path
      d="M 2.5 4 C 2.5 3.2 3.2 2.5 4 2.5 L 12 2.5 C 12.8 2.5 13.5 3.2 13.5 4 L 13.5 10 C 13.5 10.8 12.8 11.5 12 11.5 L 6.5 11.5 L 4 13.5 L 4 11.5 C 3.2 11.5 2.5 10.8 2.5 10 Z"
      stroke="currentColor" stroke-width="1.2" fill="none" stroke-linejoin="round"
    />
    <circle cx="5.5" cy="7" r="0.7" fill="currentColor" />
    <circle cx="8" cy="7" r="0.7" fill="currentColor" />
    <circle cx="10.5" cy="7" r="0.7" fill="currentColor" />
  </svg>
);
const IconShield: Component = () => (
  <svg class="ctrl-svg-icon" viewBox="0 0 16 16" aria-hidden="true">
    <path
      d="M 8 2 L 13 3.8 L 13 8.5 C 13 11 11 13 8 14 C 5 13 3 11 3 8.5 L 3 3.8 Z"
      stroke="currentColor" stroke-width="1.2" fill="none" stroke-linejoin="round"
    />
    <path d="M 5.8 8 L 7.4 9.5 L 10.2 6.5" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linecap="round" stroke-linejoin="round" />
  </svg>
);
const IconBrain: Component = () => (
  <svg class="ctrl-svg-icon" viewBox="0 0 16 16" aria-hidden="true">
    <circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="1.2" fill="none" />
    <path d="M 5 6 Q 8 4 11 6 M 5 10 Q 8 8 11 10" stroke="currentColor" stroke-width="1" fill="none" />
    <circle cx="8" cy="8" r="1.2" fill="currentColor" />
  </svg>
);
const IconNetwork: Component = () => (
  <svg class="ctrl-svg-icon" viewBox="0 0 16 16" aria-hidden="true">
    <circle cx="4" cy="8" r="2" stroke="currentColor" stroke-width="1.2" fill="none" />
    <circle cx="12" cy="5" r="2" stroke="currentColor" stroke-width="1.2" fill="none" />
    <circle cx="12" cy="11" r="2" stroke="currentColor" stroke-width="1.2" fill="none" />
    <path d="M 6 7.3 L 10 5.7 M 6 8.7 L 10 10.3" stroke="currentColor" stroke-width="1" />
  </svg>
);
const IconAlert: Component = () => (
  <svg class="intent-svg-icon" viewBox="0 0 16 16" aria-hidden="true">
    <path d="M 8 2.5 L 14 13 L 2 13 Z" stroke="currentColor" stroke-width="1.2" fill="none" stroke-linejoin="round" />
    <path d="M 8 6.5 L 8 9.5" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
    <circle cx="8" cy="11.2" r="0.7" fill="currentColor" />
  </svg>
);
const IconCheck: Component = () => (
  <svg class="intent-svg-icon" viewBox="0 0 16 16" aria-hidden="true">
    <circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="1.2" fill="none" />
    <path d="M 5 8.2 L 7.2 10.5 L 11 6.2" stroke="currentColor" stroke-width="1.4" fill="none" stroke-linecap="round" stroke-linejoin="round" />
  </svg>
);

function fatigueColor(v: number): string {
  if (v < 0.3) return "#4ade80";
  if (v <= 0.7) return "#facc15";
  return "#ef4444";
}

const ControlPanel: Component = () => {
  const [state, setState] = createSignal<ControlState | null>(null);
  const [err, setErr] = createSignal<string | null>(null);

  let timer: ReturnType<typeof setInterval> | null = null;

  async function refresh(): Promise<void> {
    try {
      const s = await api.getControlState();
      setState(s);
      setErr(null);
    } catch (e) {
      setErr(String(e));
    }
  }

  onMount(() => {
    void refresh();
    timer = setInterval(() => void refresh(), POLL_MS);
  });
  onCleanup(() => {
    if (timer) clearInterval(timer);
  });

  const memStats = () => state()?.memory_stats;
  const netStatus = () => state()?.network_status;

  // 学习型指针旋钮:任一项改动,带上当前其余值整套提交(setPointerConfig 收全部旋钮)。
  function savePtr(over: Partial<{
    matchMode: string; followThreshold: number; negBlockThreshold: number; kMergeThreshold: number; weightGain: number; kCap: number; forceJudge: boolean;
  }>): void {
    const p = memStats()?.pointer;
    if (!p) return;
    void api.setPointerConfig({
      matchMode: over.matchMode ?? p.match_mode,
      followThreshold: over.followThreshold ?? p.follow_threshold,
      negBlockThreshold: over.negBlockThreshold ?? p.neg_block_threshold,
      kMergeThreshold: over.kMergeThreshold ?? p.k_merge_threshold,
      weightGain: over.weightGain ?? p.weight_gain,
      kCap: over.kCap ?? p.k_cap,
      forceJudge: over.forceJudge ?? p.force_judge,
    }).then(() => void refresh());
  }

  // 检索行为旋钮:任一项改动,带上当前其余值整套提交(setRetrievalConfig 收全部 6 项)。
  function saveRetrieval(over: Partial<{
    ragHitThreshold: number; ragTopk: number; entryK: number; entryMinSim: number; scanBatch: number; projectBoost: number;
  }>): void {
    const r = memStats()?.retrieval;
    if (!r) return;
    void api.setRetrievalConfig({
      ragHitThreshold: over.ragHitThreshold ?? r.rag_hit_threshold,
      ragTopk: over.ragTopk ?? r.rag_topk,
      entryK: over.entryK ?? r.entry_k,
      entryMinSim: over.entryMinSim ?? r.entry_min_sim,
      scanBatch: over.scanBatch ?? r.scan_batch,
      projectBoost: over.projectBoost ?? r.project_boost,
    }).then(() => void refresh());
  }

  // 疲劳公式权重旋钮:任一项改动,带上当前其余值整套提交。
  function saveFatigue(over: Partial<{ wHitrate: number; wEvict: number; wFragment: number }>): void {
    const f = memStats()?.fatigue_weights;
    if (!f) return;
    void api.setFatigueConfig({
      wHitrate: over.wHitrate ?? f.w_hitrate,
      wEvict: over.wEvict ?? f.w_evict,
      wFragment: over.wFragment ?? f.w_fragment,
    }).then(() => void refresh());
  }

  // 瞬态容量旋钮:任一项改动,带上当前其余值整套提交。
  function saveTransient(over: Partial<{
    fragmentLedgerCap: number; secondaryIndexCap: number; internalEventsCap: number; artifactInteractionsCap: number; negReviewMaxAgeDays: number; negReviewMaxEdges: number;
  }>): void {
    const tc = memStats()?.transient;
    if (!tc) return;
    void api.setTransientCaps({
      fragmentLedgerCap: over.fragmentLedgerCap ?? tc.fragment_ledger_cap,
      secondaryIndexCap: over.secondaryIndexCap ?? tc.secondary_index_cap,
      internalEventsCap: over.internalEventsCap ?? tc.internal_events_cap,
      artifactInteractionsCap: over.artifactInteractionsCap ?? tc.artifact_interactions_cap,
      negReviewMaxAgeDays: over.negReviewMaxAgeDays ?? tc.neg_review_max_age_days,
      negReviewMaxEdges: over.negReviewMaxEdges ?? tc.neg_review_max_edges,
    }).then(() => void refresh());
  }

  return (
    <div class="control-panel">
      <Show when={err()}>
        <div class="ctrl-err">{err()}</div>
      </Show>

      {/* 记忆网络 */}
      <div class="ctrl-section">
        <div class="ctrl-section-title">
          <span class="ctrl-icon"><IconBrain /></span> {t("ctrlMemoryNetwork")}
          {/* 展开记忆网络可视化(原入口在状态栏劳累度绿点,劳累度已并入下方仪表,绿点移除后入口移到这) */}
          <button
            title={t("ctrlOpenMemViz")}
            onClick={() => openMemoryViz()}
            style={{
              "margin-left": "auto",
              display: "inline-flex",
              "align-items": "center",
              border: "none",
              background: "transparent",
              color: "var(--text-tertiary)",
              cursor: "pointer",
              padding: "2px",
              "border-radius": "4px",
              transition: "color 0.14s",
            }}
            onMouseEnter={(e) => (e.currentTarget.style.color = "var(--text-primary)")}
            onMouseLeave={(e) => (e.currentTarget.style.color = "var(--text-tertiary)")}
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M15 3h6v6" />
              <path d="M9 21H3v-6" />
              <path d="M21 3l-7 7" />
              <path d="M3 21l7-7" />
            </svg>
          </button>
        </div>
        <Show when={memStats()} fallback={
          <div class="pd-empty" style={{ "font-size": "12px" }}>
            {t("ctrlNoMemoryData")}
          </div>
        }>
          <div class="ctrl-grid">
            <div class="ctrl-cell">
              <div class="ctrl-cell-val">{memStats()!.total_nodes}</div>
              <div class="ctrl-cell-lbl">{t("ctrlTotalNodes")}</div>
            </div>
            <div class="ctrl-cell">
              <div class="ctrl-cell-val">{memStats()!.total_pointers}</div>
              <div class="ctrl-cell-lbl">{t("ctrlTotalPointers")}</div>
            </div>
          </div>
          {/* 真置换=工作区(缓存队列=存放区):随检索填充,满了是常态,Nap 清零;真/假指针=L2/RAG 命中各几条。 */}
          <div class="ctrl-note" style={{ "margin-top": "6px", "font-size": "11px" }}>
            {t("queueOccupancy")}:
            {memStats()!.queue.resident} · {(memStats()!.queue.fill_pct * 100).toFixed(1)}% · {t("ptrReal")} {memStats()!.queue.real_pointers} · {t("ptrFake")} {memStats()!.queue.fake_pointers} · {t("shmEvictions")} {memStats()!.queue.evictions}
          </div>
          {/* L2 导航缓存=纯内部加速层(页表层翻图缓存),非记忆缓存,不再作为面板指标显示。 */}
          {/* 仅保留容量旋钮(数值全可设,推论9);标注清楚是内部 L2 加速,不冒充"记忆缓存"。 */}
          <div class="ctrl-row" style={{ "margin-top": "6px", gap: "6px" }}>
            <span style={{ opacity: 0.6 }}>{t("l2NavCacheCap")}</span>
            <input
              type="number"
              min="1"
              value={memStats()!.cache.capacity}
              onChange={(e) => {
                const v = parseInt(e.currentTarget.value, 10);
                if (!Number.isNaN(v) && v > 0) void api.setNeighborCacheCap(v).then(() => void refresh());
              }}
              style={{
                width: "72px", "text-align": "right", background: "transparent",
                border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px",
                color: "inherit", font: "inherit", padding: "1px 4px",
              }}
            />
          </div>
          {/* 学习型指针旋钮(推论9 数值全可设)。即时生效(下次检索即用)+ 落库。 */}
          <Show when={memStats()!.pointer}>
            <div class="ctrl-note" style={{ "margin-top": "8px", "font-size": "11px", "font-weight": 600 }}>
              {t("ptrSectionTitle")}
            </div>
            <div class="ctrl-row" style={{ "margin-top": "4px", gap: "6px" }}>
              <span>{t("ptrMatchMode")}</span>
              <select
                value={memStats()!.pointer.match_mode}
                onChange={(e) => savePtr({ matchMode: e.currentTarget.value })}
                style={{
                  background: "transparent", border: "1px solid var(--border, #3a3a3c)",
                  "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px",
                }}
              >
                <option value="weighted_cosine">{t("ptrModeCosine")}</option>
                <option value="llm_judge">{t("ptrModeLlm")}</option>
              </select>
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span>{t("ptrFollow")}</span>
              <input
                type="number" min="0" max="3" step="0.05" value={memStats()!.pointer.follow_threshold}
                onChange={(e) => { const v = parseFloat(e.currentTarget.value); if (!Number.isNaN(v)) savePtr({ followThreshold: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span>{t("ptrNegBlock")}</span>
              <input
                type="number" min="0" max="1" step="0.01" value={memStats()!.pointer.neg_block_threshold}
                onChange={(e) => { const v = parseFloat(e.currentTarget.value); if (!Number.isNaN(v)) savePtr({ negBlockThreshold: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span>{t("ptrKMerge")}</span>
              <input
                type="number" min="0" max="1" step="0.01" value={memStats()!.pointer.k_merge_threshold}
                onChange={(e) => { const v = parseFloat(e.currentTarget.value); if (!Number.isNaN(v)) savePtr({ kMergeThreshold: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span>{t("ptrWeightGain")}</span>
              <input
                type="number" min="0" max="2" step="0.05" value={memStats()!.pointer.weight_gain}
                onChange={(e) => { const v = parseFloat(e.currentTarget.value); if (!Number.isNaN(v)) savePtr({ weightGain: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span>{t("ptrKCap")}</span>
              <input
                type="number" min="1" step="1" value={memStats()!.pointer.k_cap}
                onChange={(e) => { const v = parseInt(e.currentTarget.value, 10); if (!Number.isNaN(v) && v > 0) savePtr({ kCap: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
            {/* 档A 命中后是否仍走前沿 judge 确认(精确 vs 省钱)。仅档A 有意义,档B 下置灰。 */}
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span
                title={t("ptrForceJudgeHint")}
                style={{ opacity: memStats()!.pointer.match_mode === "weighted_cosine" ? 1 : 0.45 }}
              >{t("ptrForceJudge")}</span>
              <input
                type="checkbox"
                checked={memStats()!.pointer.force_judge}
                disabled={memStats()!.pointer.match_mode !== "weighted_cosine"}
                onChange={(e) => savePtr({ forceJudge: e.currentTarget.checked })}
                style={{ "margin-left": "auto", cursor: "pointer" }}
              />
            </div>
          </Show>
          {/* 检索行为旋钮(推论9 数值全可设)。即时生效(下次检索即用)+ 落库。 */}
          <Show when={memStats()!.retrieval}>
            <div class="ctrl-note" style={{ "margin-top": "8px", "font-size": "11px", "font-weight": 600 }}>
              {t("retrSectionTitle")}
            </div>
            <div class="ctrl-row" style={{ "margin-top": "4px", gap: "6px" }}>
              <span title={t("retrRagHitHint")}>{t("retrRagHit")}</span>
              <input
                type="number" min="0" max="1" step="0.01" value={memStats()!.retrieval.rag_hit_threshold}
                onChange={(e) => { const v = parseFloat(e.currentTarget.value); if (!Number.isNaN(v)) saveRetrieval({ ragHitThreshold: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span>{t("retrRagTopk")}</span>
              <input
                type="number" min="1" step="1" value={memStats()!.retrieval.rag_topk}
                onChange={(e) => { const v = parseInt(e.currentTarget.value, 10); if (!Number.isNaN(v) && v > 0) saveRetrieval({ ragTopk: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span title={t("retrEntryKHint")}>{t("retrEntryK")}</span>
              <input
                type="number" min="1" step="1" value={memStats()!.retrieval.entry_k}
                onChange={(e) => { const v = parseInt(e.currentTarget.value, 10); if (!Number.isNaN(v) && v > 0) saveRetrieval({ entryK: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span>{t("retrEntryMinSim")}</span>
              <input
                type="number" min="0" max="1" step="0.01" value={memStats()!.retrieval.entry_min_sim}
                onChange={(e) => { const v = parseFloat(e.currentTarget.value); if (!Number.isNaN(v)) saveRetrieval({ entryMinSim: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span title={t("retrScanBatchHint")}>{t("retrScanBatch")}</span>
              <input
                type="number" min="1" step="1" value={memStats()!.retrieval.scan_batch}
                onChange={(e) => { const v = parseInt(e.currentTarget.value, 10); if (!Number.isNaN(v) && v > 0) saveRetrieval({ scanBatch: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span title={t("retrProjectBoostHint")}>{t("retrProjectBoost")}</span>
              <input
                type="number" min="0" step="0.05" value={memStats()!.retrieval.project_boost}
                onChange={(e) => { const v = parseFloat(e.currentTarget.value); if (!Number.isNaN(v) && v >= 0) saveRetrieval({ projectBoost: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
          </Show>
          {/* 疲劳公式权重旋钮(推论9 数值全可设)。三权重决定"AI 凭什么觉得累",配套上方睡眠疲劳阈。 */}
          <Show when={memStats()!.fatigue_weights}>
            <div class="ctrl-note" style={{ "margin-top": "8px", "font-size": "11px", "font-weight": 600 }}>
              {t("fatSectionTitle")}
            </div>
            <div class="ctrl-row" style={{ "margin-top": "4px", gap: "6px" }}>
              <span title={t("fatHint")}>{t("fatHitrate")}</span>
              <input
                type="number" min="0" max="2" step="0.05" value={memStats()!.fatigue_weights.w_hitrate}
                onChange={(e) => { const v = parseFloat(e.currentTarget.value); if (!Number.isNaN(v) && v >= 0) saveFatigue({ wHitrate: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span>{t("fatEvict")}</span>
              <input
                type="number" min="0" max="2" step="0.05" value={memStats()!.fatigue_weights.w_evict}
                onChange={(e) => { const v = parseFloat(e.currentTarget.value); if (!Number.isNaN(v) && v >= 0) saveFatigue({ wEvict: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span>{t("fatFragment")}</span>
              <input
                type="number" min="0" max="2" step="0.05" value={memStats()!.fatigue_weights.w_fragment}
                onChange={(e) => { const v = parseFloat(e.currentTarget.value); if (!Number.isNaN(v) && v >= 0) saveFatigue({ wFragment: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }}
              />
            </div>
          </Show>
          {/* 瞬态容量旋钮(推论9 数值全可设,最低价值组)。可再生工作集的有界上限,改后清空重建。 */}
          <Show when={memStats()!.transient}>
            <div class="ctrl-note" style={{ "margin-top": "8px", "font-size": "11px", "font-weight": 600 }}>
              {t("tcSectionTitle")}
            </div>
            <div class="ctrl-row" style={{ "margin-top": "4px", gap: "6px" }}>
              <span title={t("tcHint")}>{t("tcFragment")}</span>
              <input type="number" min="1" step="64" value={memStats()!.transient.fragment_ledger_cap}
                onChange={(e) => { const v = parseInt(e.currentTarget.value, 10); if (!Number.isNaN(v) && v > 0) saveTransient({ fragmentLedgerCap: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }} />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span>{t("tcSecondary")}</span>
              <input type="number" min="1" step="16" value={memStats()!.transient.secondary_index_cap}
                onChange={(e) => { const v = parseInt(e.currentTarget.value, 10); if (!Number.isNaN(v) && v > 0) saveTransient({ secondaryIndexCap: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }} />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span>{t("tcInternalEvents")}</span>
              <input type="number" min="1" step="8" value={memStats()!.transient.internal_events_cap}
                onChange={(e) => { const v = parseInt(e.currentTarget.value, 10); if (!Number.isNaN(v) && v > 0) saveTransient({ internalEventsCap: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }} />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span title={t("tcArtifactHint")}>{t("tcArtifact")}</span>
              <input type="number" min="1" step="16" value={memStats()!.transient.artifact_interactions_cap}
                onChange={(e) => { const v = parseInt(e.currentTarget.value, 10); if (!Number.isNaN(v) && v > 0) saveTransient({ artifactInteractionsCap: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }} />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span title={t("tcNegAgeHint")}>{t("tcNegAge")}</span>
              <input type="number" min="1" step="1" value={memStats()!.transient.neg_review_max_age_days}
                onChange={(e) => { const v = parseInt(e.currentTarget.value, 10); if (!Number.isNaN(v) && v > 0) saveTransient({ negReviewMaxAgeDays: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }} />
            </div>
            <div class="ctrl-row" style={{ "margin-top": "2px", gap: "6px" }}>
              <span>{t("tcNegEdges")}</span>
              <input type="number" min="1" step="16" value={memStats()!.transient.neg_review_max_edges}
                onChange={(e) => { const v = parseInt(e.currentTarget.value, 10); if (!Number.isNaN(v) && v > 0) saveTransient({ negReviewMaxEdges: v }); }}
                style={{ width: "72px", "text-align": "right", background: "transparent", border: "1px solid var(--border, #3a3a3c)", "border-radius": "4px", color: "inherit", font: "inherit", padding: "1px 4px" }} />
            </div>
          </Show>
          {/* 劳累度仪表也是打开记忆网络可视化的入口(与标题右侧展开图标并存,用户要求 2026-06-02)。 */}
          <div
            class="ctrl-row"
            style={{ "margin-top": "4px", cursor: "pointer" }}
            title={t("ctrlOpenMemViz")}
            onClick={() => openMemoryViz()}
          >
            <span>{t("ctrlFatigue")}</span>
            <span
              class="ctrl-mono"
              style={{ color: fatigueColor(memStats()!.fatigue.fatigue_value) }}
            >
              {(memStats()!.fatigue.fatigue_value * 100).toFixed(1)}%
            </span>
          </div>
        </Show>
      </div>

      {/* 双网络 */}
      <div class="ctrl-section">
        <div class="ctrl-section-title">
          <span class="ctrl-icon"><IconNetwork /></span> {t("ctrlDualNetwork")}
        </div>
        <Show when={netStatus()} fallback={
          <div class="pd-empty" style={{ "font-size": "12px" }}>
            {t("ctrlNoNetworkData")}
          </div>
        }>
          <div class="ctrl-row">
            <span>{t("ctrlMainModel")}</span>
            <span class="ctrl-mono">{netStatus()!.main_model || "—"}</span>
          </div>
          <div class="ctrl-row">
            <span>{t("ctrlSubModel")}</span>
            <span class="ctrl-mono">{netStatus()!.sub_model || (t("ctrlNotConfigured"))}</span>
          </div>
          <div class="ctrl-row">
            <span>{t("ctrlEmbedding")}</span>
            <span class="ctrl-mono">{netStatus()!.has_embedder ? (t("ctrlEnabled")) : (t("ctrlDisabled"))}</span>
          </div>
        </Show>
      </div>

      {/* 注意力间隔 */}
      <div class="ctrl-section">
        <div class="ctrl-section-title">
          <span class="ctrl-icon"><IconBrain /></span> {t("ctrlAttentionSpan")}
        </div>
        <div class="ctrl-row">
          <span>{t("ctrlCurrentAttentionSpan")}</span>
          <span class="ctrl-mono">{statusInfo()?.attention_span ?? "—"}</span>
        </div>
      </div>

      {/* Intent 信号 (kept) */}
      <div class="ctrl-section">
        <div class="ctrl-section-title">
          <span class="ctrl-icon"><IconIntent /></span> {t("ctrlIntent")}
        </div>
        <Show
          when={intentEvents().length > 0}
          fallback={
            <div class="pd-empty" style={{ "font-size": "12px" }}>
              {t("ctrlNoIntent")}
            </div>
          }
        >
          <For each={intentEvents().slice(-8).reverse()}>
            {(ev) => {
              const time = new Date(ev.ts).toLocaleTimeString();
              const cls = ev.is_challenge ? "intent-challenge" : "intent-normal";
              return (
                <div class={`intent-event ${cls}`}>
                  <span class="intent-time">{time}</span>
                  <span class="intent-flag">
                    {ev.is_challenge ? <IconAlert /> : <IconCheck />}
                    <span class="intent-flag-text">{ev.is_challenge ? "challenge" : "normal"}</span>
                  </span>
                  <span class="intent-conf">{ev.confidence.toFixed(2)}</span>
                  <span class="intent-topic">{ev.topic.join(", ")}</span>
                </div>
              );
            }}
          </For>
        </Show>
      </div>

      {/* 数据保护 (Safety - kept) */}
      <div class="ctrl-section">
        <div class="ctrl-section-title">
          <span class="ctrl-icon"><IconShield /></span> {t("ctrlSafety")}
        </div>
        <Show when={state()}>
          <div class="ctrl-row">
            <span>{t("ctrlBackups")}</span>
            <span class="ctrl-mono">{state()!.safety.backup_count}</span>
          </div>
          <Show when={state()!.safety.backups_dir}>
            <div class="ctrl-row ctrl-path" title={state()!.safety.backups_dir}>
              {state()!.safety.backups_dir}
            </div>
          </Show>
          <div class="ctrl-note">{state()!.safety.note}</div>
        </Show>
      </div>
    </div>
  );
};

export default ControlPanel;
