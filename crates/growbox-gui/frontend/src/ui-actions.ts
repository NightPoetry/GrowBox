// 活的 IDE —— 前端单一 UI 意图分发器 + 声明式面板注册表(推论 7,见 计划/活的IDE-UI执行器.md)。
//
// 这是后端"一个注册表、一条分发路径"公理在前端的镜像:替掉 App.tsx 里在长的 if/else action 链。
// - PANELS = 声明式面板注册表,也是"身体的解剖":前端是唯一作者,mount 时 register_ui_surfaces 上报后端。
// - dispatchUiAction = 单一分发:ui_control 查 target/op 调 open/close/toggle 并回 ui_action_ack 验证态(往返);
//   5 个 legacy 扁平 action(家族一:弹预填表单交用户裁决)收编到同一处,发出即落地(不回执)。

import { createEffect } from "solid-js";
import { api, type UiSurface } from "./tauri-api";
import { notify } from "./notices";
import {
  setProjectPrefill, setProjectCreateOpen, setProjectCreateFromAgent,
  setSettingsTab, setSettingsOpen,
  setAddPathPrefill, setAddPathOpen,
  healthMonitorOpen, setHealthMonitorOpen,
  historyDrawerOpen, setHistoryDrawerOpen,
} from "./store";
import { memoryVizOpen, openMemoryViz, closeMemoryViz, toggleMemoryViz } from "./components/MemoryViz";
import { dreamOpen, openDreamPanel, closeDreamPanel, toggleDreamPanel } from "./components/DreamPanel";
import { artifactCanvasOpen, openArtifactCanvas, closeArtifactCanvas, toggleArtifactCanvas, renderArtifact, pushArtifactNotice, commandArtifact, runArtifactSelftest } from "./components/ArtifactCanvas";
import { showToast } from "./toast";
import { t } from "./i18n";
import { showSuggestion, type SuggestionOption } from "./components/SuggestionDialog";
import { showChoicePopup, type ChoiceOption } from "./components/ChoicePopup";

// ── 声明式面板注册表(可被 LLM 经 ui_control 操控的"纯可见性"面板)──
// 每个面板提供 isOpen/open/close/toggle;新增面板只往这里加一行,dispatchUiAction 不用动。
interface PanelEntry {
  label: string;            // 给 LLM 的人话(进 register_ui_surfaces)
  isOpen: () => boolean;
  open: () => void;
  close: () => void;
  toggle: () => void;
}

const PANELS: Record<string, PanelEntry> = {
  memory: {
    label: "记忆可视化面板",
    isOpen: memoryVizOpen, open: openMemoryViz, close: closeMemoryViz, toggle: toggleMemoryViz,
  },
  dream: {
    label: "做梦/睡眠面板",
    isOpen: dreamOpen, open: openDreamPanel, close: closeDreamPanel, toggle: toggleDreamPanel,
  },
  health: {
    label: "系统健康监控面板",
    isOpen: healthMonitorOpen,
    open: () => setHealthMonitorOpen(true),
    close: () => setHealthMonitorOpen(false),
    toggle: () => setHealthMonitorOpen((p) => !p),
  },
  history: {
    label: "对话历史抽屉",
    isOpen: historyDrawerOpen,
    open: () => setHistoryDrawerOpen(true),
    close: () => setHistoryDrawerOpen(false),
    toggle: () => setHistoryDrawerOpen((p) => !p),
  },
  artifact: {
    label: "造物画布(AI 现造的 UI)",
    isOpen: artifactCanvasOpen, open: openArtifactCanvas, close: closeArtifactCanvas, toggle: toggleArtifactCanvas,
  },
};

const UI_CONTROL_OPS = ["open", "close", "toggle"];

// 上报给后端的面板目录(单一真相在此)。后端据此生成 ui_control 的 target enum + 校验。
export function uiSurfaceCatalog(): UiSurface[] {
  return Object.entries(PANELS).map(([id, p]) => ({ id, label: p.label, ops: UI_CONTROL_OPS }));
}

// mount 时调一次:把本前端的可控面板声明给后端。
export async function registerUiSurfaces(): Promise<void> {
  try {
    await api.registerUiSurfaces(uiSurfaceCatalog());
  } catch {
    // 浏览器环境(无 Tauri 桥)忽略;真实窗口内才有意义。
  }
}

// 感知闭合:监视每个面板的可见态,任何变化(用户手动点 / Agent 经 ui_control)都上报后端。
// 用 createEffect 统一捕获,无需改各组件内部、也无需区分触发来源。mount 时调一次(在 App 的 owner 下)。
// 首次 run 同步当前态(后端记为缓存、不 perceive);之后每次真实翻转后端才记一条感知。
// 上报失败 = AI 对界面的感知失真(自我感知原则):退避重试一次,仍失败则告知一次(会话级去重,免刷屏)。
let reportFailureNotified = false;
function reportPanelState(id: string, open: boolean): void {
  void (async () => {
    try {
      await api.uiStateChanged(id, open);
    } catch {
      await new Promise((r) => setTimeout(r, 300));
      try {
        await api.uiStateChanged(id, open);
      } catch (err) {
        console.warn("[ui-actions] uiStateChanged 两次投递失败:", id, err);
        if (!reportFailureNotified) {
          reportFailureNotified = true;
          notify("ui.report_failed");
        }
      }
    }
  })();
}

export function watchPanelsAndReport(): void {
  for (const [id, panel] of Object.entries(PANELS)) {
    createEffect(() => {
      const open = panel.isOpen();
      reportPanelState(id, open);
    });
  }
}

// ── 单一 UI 意图分发 ──
// 后端 emit "ui-action" { action, data, id? } → 这里统一落地。
// id 存在 = 家族二(ui_control 往返,需回执);无 id = 家族一(弹表单,发出即返回)。
export function dispatchUiAction(
  action: string,
  data: Record<string, unknown> | undefined,
  id: string | undefined,
): void {
  // 家族二:Agent 自己对 UI 动手 —— 查表执行 + 回执验证态(不撒谎)。
  if (action === "ui_control") {
    const target = typeof data?.target === "string" ? data.target : undefined;
    const op = typeof data?.op === "string" ? data.op : undefined;
    const panel = target ? PANELS[target] : undefined;
    let applied = false;
    if (panel && (op === "open" || op === "close" || op === "toggle")) {
      panel[op]();
      applied = true;
    }
    if (id) {
      const open = panel ? panel.isOpen() : false;
      void api.uiActionAck(id, applied, { open }, applied ? null : `未知面板或动作: ${target ?? "?"}/${op ?? "?"}`);
    }
    return;
  }

  // 家族二:被造物展示 —— 把 AI 现造的 HTML 渲进沙箱画布,回执验证态(不撒谎)。
  if (action === "render_artifact") {
    const html = typeof data?.html === "string" ? data.html : "";
    const applied = html.length > 0;
    // chrome: LLM 经 render_artifact 声明此造物是否要顶部横栏(默认 true;桌宠等可 false)。
    if (applied) renderArtifact(html, data?.chrome !== false);
    if (id) {
      void api.uiActionAck(id, applied, { rendered: applied }, applied ? null : "render_artifact 缺少 html 参数");
    }
    return;
  }

  // 家族二:造物覆盖层主动推送(AI 的吐槽/建议)→ 渲进覆盖层,回执验证态。
  if (action === "push_artifact_notice") {
    const text = typeof data?.text === "string" ? data.text : "";
    const applied = text.length > 0;
    if (applied) pushArtifactNotice(text);
    if (id) {
      void api.uiActionAck(id, applied, { shown: applied }, applied ? null : "push_artifact_notice 缺少 text 参数");
    }
    return;
  }

  // 家族二:★造物灵魂 LLM→造物指令★ —— 转发到造物 iframe,其 window.gxOnCommand 本地执行(不重画)。
  if (action === "artifact_command") {
    const command = typeof data?.command === "string" ? data.command : "";
    const applied = command.length > 0 && commandArtifact(command);
    if (id) {
      void api.uiActionAck(id, applied, { dispatched: applied }, applied ? null : "artifact_command 缺少 command 或造物未就绪");
    }
    return;
  }

  // 家族二:造物自检 —— 令 iframe 枚举其声明的回调面,回执清单(AI finish 前自测各感知接通)。
  // 自关机能力(计划/自关机能力.md):高风险、不可逆 → 每次弹一次性授权(window.confirm 阻塞确认),
  // 除非用户在设置里开了永久权(auto_shutdown_allowed)。确认后调 do_shutdown 真执行,回执给 AI。
  if (action === "shutdown") {
    if (id) {
      const act = String((data?.action as string) ?? "");
      const delaySecs = Number((data?.delay_secs as number) ?? 0);
      void (async () => {
        let auto = false;
        try { auto = !!(await api.getMiscConfig()).auto_shutdown_allowed; } catch { /* 读不到 → 当未授权,照常弹确认 */ }
        const msg = act === "exit_self" ? t("shutdownConfirmExit") : t("shutdownConfirmSystem");
        const proceed = auto || window.confirm(msg);
        if (!proceed) { void api.uiActionAck(id, false, {}, t("shutdownCancelled")); return; }
        try {
          await api.doShutdown(act, delaySecs);
          void api.uiActionAck(id, true, { performed: act }, null);
        } catch (e) {
          void api.uiActionAck(id, false, {}, String(e));
        }
      })();
    }
    return;
  }

  if (action === "selftest_artifact") {
    if (id) {
      void runArtifactSelftest().then((r) => {
        // §7 硬验证:回报声明的回调面 + AI 指令通道 gxOnCommand 是否注册。commandChannel=false →
        // AI 的落子指令到不了造物,AI 必须补 window.gxOnCommand handler 再 finish(治造物版虚假成功的操作面)。
        void api.uiActionAck(
          id,
          true,
          { callbacks: r.callbacks, count: r.callbacks.length, commandChannel: r.gxOnCommand },
          null,
        );
      });
    }
    return;
  }

  // 家族二:网页调试(Phase 2)—— 打开调试 webview 加载本地 URL(后端建窗 + 注入套索运行时)。
  if (action === "open_debug_url") {
    const url = typeof data?.url === "string" ? data.url : "";
    if (id) {
      if (!url) {
        void api.uiActionAck(id, false, {}, "open_debug_url 缺少 url");
        return;
      }
      void api
        .createDebugWebview(url)
        .then(() => api.uiActionAck(id, true, { opened: true }, null))
        .catch((e) => api.uiActionAck(id, false, {}, String(e)));
    }
    return;
  }

  // 家族二:网页 QA 自反馈调试 —— 在调试窗里真操作(click/fill/submit/scan/observe),把观察 JSON
  // (url/title/报错/选择器是否匹配)经回执 state 交给 AI(脊柱 Intent·await_ack 分支会格式化进工具结果)。
  if (action === "web_debug_drive") {
    const op = typeof data?.op === "string" ? data.op : "";
    const selector = typeof data?.selector === "string" ? data.selector : "";
    const value = typeof data?.value === "string" ? data.value : "";
    if (id) {
      void api
        .webDebugDrive(op, selector, value)
        .then((obs) => api.uiActionAck(id, true, (obs ?? {}) as Record<string, unknown>, null))
        .catch((e) => api.uiActionAck(id, false, {}, String(e)));
    }
    return;
  }

  // 家族一:交付用户裁决(弹预填表单/对话),发出即落地。
  if (action === "open_new_project") {
    if (data) {
      setProjectPrefill({
        id: data.id as string | undefined,
        name: data.name as string | undefined,
        writable: data.writable as string[] | undefined,
        readonly: data.readonly as string[] | undefined,
        description: data.description as string | undefined,
      });
    }
    // Agent(gated hand_off)弹的:Agent 已让位暂停等用户处理 → 确认/取消后要注入消息重新驱动。
    setProjectCreateFromAgent(true);
    setProjectCreateOpen(true);
  } else if (action === "open_settings") {
    // AI 不直接改设置:打开面板 + 切到连接页 + 滚动高亮到目标字段,改不改由用户定。
    setSettingsTab("connection");
    setSettingsOpen(true);
    if (data?.note) showToast(String(data.note), "info", 6000);
    const field = data?.field as string | undefined;
    if (field) {
      // 双 rAF:等 Show 把面板挂载出来再滚动高亮。
      requestAnimationFrame(() => requestAnimationFrame(() => {
        const el = document.getElementById(`set-${field}`);
        if (!el) return;
        el.scrollIntoView({ behavior: "smooth", block: "center" });
        el.classList.add("setting-flash");
        setTimeout(() => el.classList.remove("setting-flash"), 1600);
      }));
    }
  } else if (action === "open_add_path") {
    if (data) {
      setAddPathPrefill({
        path: data.path as string | undefined,
        kind: data.kind as "writable" | "readonly" | undefined,
      });
    }
    setAddPathOpen(true);
  } else if (action === "show_suggestion" && data) {
    showSuggestion(data.title as string, data.options as SuggestionOption[]);
  } else if (action === "show_choice" && data) {
    showChoicePopup({
      title: data.title as string | undefined,
      description: data.description as string | undefined,
      options: data.options as ChoiceOption[] | undefined,
    });
  }
}
