import { createSignal, For, Show, onMount, onCleanup, type Component } from "solid-js";
import {
  statusInfo,
  projects,
  currentProjectId,
  projectDirectories,
  projectDropdownOpen,
  setProjectDropdownOpen,
  setProjectCreateOpen,
  setProjectCreateFromAgent,
  setAddPathOpen,
  flashingPaths,
} from "../store";
import { switchToProject, removePath, addPath } from "../projects";
import { t } from "../i18n";
import { sfx } from "../sfx";

const GAUGE_TOTAL = 126;

// 0~1 压力色:绿(松)→黄(紧)→红(满)。劳累度/置换率/上下文/缓存占用 四表共用,视觉一致。
function levelColor(v: number): string {
  if (v < 0.3) return "#4ade80";
  if (v <= 0.7) return "#facc15";
  return "#ef4444";
}

// token 数紧凑成 k/m:12000→"12k",256000→"256k",1000000→"1.0m"。
function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}m`;
  if (n >= 1_000) return `${Math.round(n / 1_000)}k`;
  return String(n);
}

const Sidebar: Component = () => {
  const [editingPath, setEditingPath] = createSignal<string | null>(null);
  let sidebarRef: HTMLDivElement | undefined;

  // ── derived signals ──────────────────────────────────────

  // 记忆置换率(原"压力"表改名实装):邻域缓存淘汰压力 [0,1]。
  const replacementRate = () => statusInfo()?.replacement_rate ?? 0;
  const replacementOffset = () => GAUGE_TOTAL * (1 - replacementRate());

  const fatigueVal = () => statusInfo()?.fatigue ?? 0;
  const fatigueOffset = () => GAUGE_TOTAL * (1 - fatigueVal());

  const pointerCount = () => statusInfo()?.pointer_count ?? 0;
  const totalNodes = () => statusInfo()?.total_nodes ?? 0;

  // ★缓存队列(工作区/存放区=真·临时记忆)★:常驻块数(随检索涨,Nap 清零)+ 填充率(budget_pct)。
  // 此前读 cache_used/capacity(NeighborCache=L2 翻图加速器,RAG 命中不下沉 L2→恒 0/256,被 L1 命中率绑死),已改挂真置换。
  const queueResident = () => statusInfo()?.queue_resident ?? 0;
  const queueFrac = () => Math.min((statusInfo()?.budget_pct ?? 0) / 100, 1);
  const queueOffset = () => GAUGE_TOTAL * (1 - queueFrac());

  // 实时上下文压力:最近请求实发上下文 token / 上下文窗口总量(可设)。
  const ctxTokens = () => statusInfo()?.ctx_prompt_tokens ?? 0;
  const ctxWindow = () => statusInfo()?.ctx_window_tokens ?? 0;
  const ctxFrac = () => { const w = ctxWindow(); return w > 0 ? Math.min(ctxTokens() / w, 1) : 0; };
  const ctxOffset = () => GAUGE_TOTAL * (1 - ctxFrac());


  const currentName = () => {
    const cur = currentProjectId();
    if (!cur) return t("noActiveProject");
    const found = projects().find((p) => p.id === cur);
    return found?.name ?? cur;
  };

  function handleOutsideClick(e: MouseEvent) {
    if (!projectDropdownOpen()) return;
    if (sidebarRef && !sidebarRef.querySelector(".project-selector")?.contains(e.target as Node)) {
      setProjectDropdownOpen(false);
    }
  }
  onMount(() => document.addEventListener("mousedown", handleOutsideClick));
  onCleanup(() => document.removeEventListener("mousedown", handleOutsideClick));

  async function commitPathEdit(kind: "writable" | "readonly", oldPath: string, newValue: string) {
    if (editingPath() !== oldPath) return;
    const trimmed = newValue.trim();
    setEditingPath(null);
    if (!trimmed || trimmed === oldPath) return;
    await removePath(kind, oldPath);
    await addPath(kind, trimmed);
  }

  function onPathEditKeyDown(e: KeyboardEvent, kind: "writable" | "readonly", oldPath: string) {
    // 合字中的回车只确认候选、不提交路径(IME,同发送框)。
    if (e.key === "Enter" && !e.isComposing && e.keyCode !== 229) { e.preventDefault(); commitPathEdit(kind, oldPath, (e.currentTarget as HTMLInputElement).value); }
    else if (e.key === "Escape") { setEditingPath(null); }
  }

  return (
    <div class="sidebar" ref={sidebarRef}>
      {/* project selector */}
      <div class="project-selector" style={{ position: "relative" }}>
        <button class="project-btn" onClick={() => { sfx.tap(); setProjectDropdownOpen(!projectDropdownOpen()); }}>
          <span class="dot" />
          <span class="name">{currentName()}</span>
          <span class="arrow">▾</span>
        </button>
        <div class={`project-dropdown ${projectDropdownOpen() ? "visible" : ""}`} style={{ top: "42px", left: "12px", right: "12px" }}>
          <Show when={projects().length > 0} fallback={<div class="project-dropdown-item" style={{ opacity: 0.6 }}>{t("noProjects")}</div>}>
            <For each={projects()}>
              {(p) => (
                <div class={`project-dropdown-item ${p.id === currentProjectId() ? "active" : ""}`} onClick={() => { sfx.tap(); void switchToProject(p.id); }}>
                  <span>{p.name}</span>
                  <span class="meta">{p.experience_count}E {p.knowledge_count}K {p.understanding_count}U</span>
                </div>
              )}
            </For>
          </Show>
          <div class="project-dropdown-divider" />
          <div class="project-dropdown-action" onClick={() => { sfx.tap(); setProjectDropdownOpen(false); setProjectCreateFromAgent(false); setProjectCreateOpen(true); }}>+ {t("newProject")}</div>
        </div>
      </div>

      {/* dashboard */}
      <div class="sidebar-upper">
        <div class="sidebar-upper-body">
          <div class="dash-grid">

            <div class="dash-cell">
              <svg class="gauge-svg" viewBox="0 0 100 58">
                <path class="gauge-bg" d="M 10 52 A 40 40 0 0 1 90 52" />
                <path class="gauge-fill" d="M 10 52 A 40 40 0 0 1 90 52"
                  stroke-dasharray={String(GAUGE_TOTAL)} stroke-dashoffset={String(replacementOffset())} stroke={levelColor(replacementRate())} />
              </svg>
              <div class="gauge-val" style={{ color: levelColor(replacementRate()) }}>{(replacementRate() * 100).toFixed(0)}%</div>
              <div class="gauge-lbl">{t("memReplacement") || "记忆置换率"}</div>
            </div>

            <div class="dash-cell">
              <svg class="gauge-svg" viewBox="0 0 100 58">
                <path class="gauge-bg" d="M 10 52 A 40 40 0 0 1 90 52" />
                <path class="gauge-fill" d="M 10 52 A 40 40 0 0 1 90 52"
                  stroke-dasharray={String(GAUGE_TOTAL)} stroke-dashoffset={String(fatigueOffset())} stroke={levelColor(fatigueVal())} />
              </svg>
              <div class="gauge-val" style={{ color: levelColor(fatigueVal()) }}>{(fatigueVal() * 100).toFixed(0)}%</div>
              <div class="gauge-lbl">{t("shmFatigue") || "劳累度"}</div>
            </div>

            <div class="dash-cell">
              <svg class="gauge-svg" viewBox="0 0 100 58">
                <path class="gauge-bg" d="M 10 52 A 40 40 0 0 1 90 52" />
                <path class="gauge-fill" d="M 10 52 A 40 40 0 0 1 90 52"
                  stroke-dasharray={String(GAUGE_TOTAL)} stroke-dashoffset={String(queueOffset())} stroke={levelColor(queueFrac())} />
              </svg>
              <div class="gauge-val" style={{ color: levelColor(queueFrac()) }}>{queueResident()}</div>
              <div class="gauge-lbl">{t("queueOccupancy") || "缓存队列"}</div>
            </div>

            <div class="dash-cell">
              <svg class="gauge-svg" viewBox="0 0 100 58">
                <path class="gauge-bg" d="M 10 52 A 40 40 0 0 1 90 52" />
                <path class="gauge-fill" d="M 10 52 A 40 40 0 0 1 90 52"
                  stroke-dasharray={String(GAUGE_TOTAL)} stroke-dashoffset={String(ctxOffset())} stroke={levelColor(ctxFrac())} />
              </svg>
              <div class="gauge-val" style={{ color: levelColor(ctxFrac()) }}>{fmtTokens(ctxTokens())}/{fmtTokens(ctxWindow())}</div>
              <div class="gauge-lbl">{t("contextPressure") || "上下文压力"}</div>
            </div>

            <div class="dash-cell">
              <div class="num-val">{pointerCount()}</div>
              <div class="num-lbl">{t("pointers")}</div>
            </div>

            <div class="dash-cell">
              <div class="num-val">{totalNodes()}</div>
              <div class="num-lbl">{t("ctrlTotalNodes") || "节点数"}</div>
            </div>

          </div>
        </div>
      </div>

      {/* divider + directories */}
      <div class="sidebar-divider" />

      <div class="sidebar-scroll">
        <div class="project-dirs">
          <div class="pd-header">
            <span class="pd-title">{t("projectDirs")}</span>
            <button class="pd-add" title={t("addWritable")} onClick={() => { sfx.tap(); setAddPathOpen(true); }}>+</button>
          </div>
          <div class="pd-section">
            <div class="pd-section-title"><span class="pd-badge writable">RW</span><span>{t("projectWritableShort")}</span></div>
            <div class="pd-list">
              <Show when={projectDirectories() && projectDirectories()!.writable.length > 0} fallback={<div class="pd-empty">{t("pdEmptyWritable")}</div>}>
                <For each={projectDirectories()!.writable}>
                  {(p) => {
                    const flashClass = () => { const k = flashingPaths()[p]; return k === "write" ? "flash-write" : k === "read" ? "flash-read" : ""; };
                    const isEditing = () => editingPath() === p;
                    return (
                      <div class={`pd-row ${flashClass()}`} data-kind="writable" data-root-path={p} title={p}>
                        <Show when={isEditing()} fallback={<span class="pd-path">{p}</span>}>
                          <input class="pd-path" style={{ flex:"1","min-width":"0",background:"transparent",border:"1px solid var(--accent)","border-radius":"3px",color:"var(--text-primary)","font-family":"inherit","font-size":"inherit",padding:"1px 4px",outline:"none" }}
                            value={p} onKeyDown={(e) => onPathEditKeyDown(e, "writable", p)}
                            onBlur={(e) => commitPathEdit("writable", p, e.currentTarget.value)} />
                        </Show>
                        <button class="pd-edit" onClick={() => setEditingPath(isEditing() ? null : p)} title={t("editLabel")}><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9" /><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z" /></svg></button>
                        <button class="pd-del" onClick={() => { sfx.tap(); void removePath("writable", p); }}>×</button>
                      </div>
                    );
                  }}
                </For>
              </Show>
            </div>
          </div>
          <div class="pd-section">
            <div class="pd-section-title"><span class="pd-badge readonly">RO</span><span>{t("projectReadonlyShort")}</span></div>
            <div class="pd-list">
              <Show when={projectDirectories() && projectDirectories()!.readonly.length > 0} fallback={<div class="pd-empty">{t("pdEmptyReadonly")}</div>}>
                <For each={projectDirectories()!.readonly}>
                  {(p) => {
                    const flashClass = () => { const k = flashingPaths()[p]; return k === "write" ? "flash-write" : k === "read" ? "flash-read" : ""; };
                    const isEditing = () => editingPath() === p;
                    return (
                      <div class={`pd-row ${flashClass()}`} data-kind="readonly" data-root-path={p} title={p}>
                        <Show when={isEditing()} fallback={<span class="pd-path">{p}</span>}>
                          <input class="pd-path" style={{ flex:"1","min-width":"0",background:"transparent",border:"1px solid var(--accent)","border-radius":"3px",color:"var(--text-primary)","font-family":"inherit","font-size":"inherit",padding:"1px 4px",outline:"none" }}
                            value={p} onKeyDown={(e) => onPathEditKeyDown(e, "readonly", p)}
                            onBlur={(e) => commitPathEdit("readonly", p, e.currentTarget.value)} />
                        </Show>
                        <button class="pd-edit" onClick={() => setEditingPath(isEditing() ? null : p)} title={t("editLabel")}><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20h9" /><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z" /></svg></button>
                        <button class="pd-del" onClick={() => { sfx.tap(); void removePath("readonly", p); }}>×</button>
                      </div>
                    );
                  }}
                </For>
              </Show>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
};

export default Sidebar;
