// 状态栏「shell k」后台任务计数 + 点开看列表(#4)。
// 双受众:对外这里只露计数(k = 运行中任务数),点开才看 tag(label)/ 原始命令 / 状态;
// 对内 LLM 走 list_tasks 工具拿同一份(get_tasks 与之同源 task_mgr.snapshot)。
import { Show, For, createSignal, createEffect, onCleanup, type Component } from "solid-js";
import { statusInfo } from "../store";
import { t } from "../i18n";
import { api, type TaskInfo } from "../tauri-api";

const ShellTasks: Component = () => {
  const [open, setOpen] = createSignal(false);
  const [tasks, setTasks] = createSignal<TaskInfo[]>([]);
  const count = () => statusInfo()?.running_tasks ?? 0;
  let wrapperRef: HTMLDivElement | undefined;

  async function refresh() {
    try {
      const snap = await api.getTasks();
      setTasks(snap.tasks);
    } catch {
      setTasks([]);
    }
  }
  async function toggle() {
    if (!open()) await refresh();
    setOpen((o) => !o);
  }

  // 点击面板外空白即关闭(与其它面板一致)。
  createEffect(() => {
    if (!open()) return;
    const onDoc = (e: MouseEvent) => {
      if (wrapperRef && !wrapperRef.contains(e.target as Node)) setOpen(false);
    };
    const id = window.setTimeout(() => document.addEventListener("mousedown", onDoc), 0);
    onCleanup(() => {
      window.clearTimeout(id);
      document.removeEventListener("mousedown", onDoc);
    });
  });

  const stateLabel = (s: string) =>
    s === "running" ? (t("taskStateRunning"))
    : s === "done" ? (t("taskStateDone"))
    : (t("taskStateFailed"));
  const stateColor = (s: string) => (s === "running" ? "#0a84ff" : s === "done" ? "#30d158" : "#ff453a");

  return (
    <Show when={count() > 0}>
      <div ref={wrapperRef} style={{ position: "relative", display: "inline-flex", "align-items": "center" }}>
        <button
          onClick={() => void toggle()}
          title={t("taskListTitle")}
          style={{
            display: "inline-flex", "align-items": "center", gap: "4px",
            border: "none", background: "transparent", cursor: "pointer",
            color: "var(--text-secondary, #aeaeb2)", font: "inherit", padding: "0 6px",
            "font-size": "11px", "font-weight": 600,
          }}
        >
          <span style={{ width: "6px", height: "6px", "border-radius": "50%", background: "#0a84ff", "box-shadow": "0 0 5px #0a84ff" }} />
          shell {count()}
        </button>
        <Show when={open()}>
          <div
            style={{
              position: "absolute", bottom: "22px", left: "0", "z-index": 2000,
              background: "#1c1c1e", border: "1px solid rgba(255,255,255,0.14)",
              "border-radius": "8px", padding: "8px 10px", "min-width": "260px", "max-width": "440px",
              "box-shadow": "0 6px 24px rgba(0,0,0,0.5)",
            }}
          >
            <div style={{ "font-size": "12px", "font-weight": 700, "margin-bottom": "6px", color: "#e5e5ea" }}>
              {t("taskListTitle")}
            </div>
            <Show
              when={tasks().length > 0}
              fallback={<div style={{ "font-size": "12px", color: "#8e8e93" }}>{t("noRunningTasks")}</div>}
            >
              <For each={tasks()}>
                {(tk) => (
                  <div style={{ "margin-bottom": "6px", "line-height": 1.4 }}>
                    <div style={{ "font-size": "12px", color: "#e5e5ea" }}>
                      <span style={{ color: stateColor(tk.state), "margin-right": "5px" }}>●</span>
                      <span>{stateLabel(tk.state)}</span> · {tk.label}
                    </div>
                    <div style={{ "font-size": "11px", "font-family": "monospace", color: "#8e8e93", "margin-left": "13px", "word-break": "break-all" }}>
                      $ {tk.command}
                    </div>
                  </div>
                )}
              </For>
            </Show>
          </div>
        </Show>
      </div>
    </Show>
  );
};

export default ShellTasks;
