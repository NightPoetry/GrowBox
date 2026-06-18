import { onMount, onCleanup, createEffect, type Component } from "solid-js";

import Sidebar from "./components/Sidebar";
import ChatArea from "./components/ChatArea";
import StatusBar from "./components/StatusBar";
import Settings from "./components/Settings";
import SystemHealthMonitor from "./components/SystemHealthMonitor";
import HistoryDrawer from "./components/HistoryDrawer";
import ProjectCreateModal from "./components/ProjectCreateModal";
import AddPathModal from "./components/AddPathModal";
import PermissionDialog from "./components/PermissionDialog";
import { showPermissionRequest } from "./components/PermissionDialog";
import ShellApprovalDialog from "./components/ShellApprovalDialog";
import { showShellApproval } from "./components/ShellApprovalDialog";
import CloseConfirmDialog from "./components/CloseConfirmDialog";
import { requestCloseConfirm } from "./components/CloseConfirmDialog";
import SuggestionDialog from "./components/SuggestionDialog";
import ChoicePopup from "./components/ChoicePopup";
import MemoryViz from "./components/MemoryViz";
import DreamPanel from "./components/DreamPanel";
import ArtifactCanvas from "./components/ArtifactCanvas";
import TerminalPanel from "./components/TerminalPanel";
import ToastContainer from "./components/ToastContainer";
import { startPolling } from "./status";
import { refreshDict, currentLang } from "./i18n";
import { refreshTools, refreshToolKinds } from "./tools";
import { showToast } from "./toast";
import { notify, loadNoticeCatalog, startNoticeListener } from "./notices";
import { startDirAccessListener, bootstrapOffline, addPath, refreshProjects, refreshProjectDirs } from "./projects";
import {
  messages, sending, connected, statusInfo,
  projects, currentProjectId, intentEvents, flashingPaths,
  truncateToolDisplay, autoMode, selfDriveActive, setSelfDriveActive, dangerMode, setDangerMode,
  theme,
} from "./store";
import { doSend, sendWebDebugEdit } from "./chat";
import { resetAndLoadHistory, saveTranscript } from "./history";
import { switchToProject } from "./projects";
import { api, listen } from "./tauri-api";
import { setMessages, setCurrentProjectId, setAppVersion, setStatusInfo } from "./store";
import { dispatchUiAction, registerUiSurfaces, watchPanelsAndReport } from "./ui-actions";
import { doConnect } from "./components/Settings";

declare global {
  interface Window {
    __GROWBOX_TEST__?: Record<string, unknown>;
  }
}

const App: Component = () => {
  // 工具卡片 + 提示告知目录都按界面语言本地化(单一源后端 get_tools / get_notice_catalog(ui_lang))。
  // 挂全局:首帧拉一次 + 切界面语言即重拉 → 工具标记(tTool 读 toolList.label)与 toast 文案都跟随。
  createEffect(() => {
    const l = currentLang();
    void refreshTools();
    void refreshToolKinds();
    void loadNoticeCatalog(l);
  });

  // ★自驱续跑的前置约束★:关掉全自动模式 / 断开连接时,强制复位自驱开关(按钮也随 autoMode 隐藏)。
  // 自驱需要自动审核 shell 才能无人值守连续干活,故离开全自动就不该继续自己跑。
  createEffect(() => {
    if ((!autoMode() || !connected()) && selfDriveActive()) {
      setSelfDriveActive(false);
    }
    // ★danger 是 auto 的升级★:关全自动 / 断连 → 同时收掉 danger(否则 danger 开关随 auto 隐藏、
    // 后端却仍开着 = 无 UI 可关的陷阱)。同步推后端复位。
    if ((!autoMode() || !connected()) && dangerMode()) {
      setDangerMode(false);
      void api.setDangerMode(false).catch(() => {});
    }
  });

  // ★danger 模式全局红边★:开启时给 <body> 挂 danger-active 类 → CSS 把全局线条/边框变量染微红
  // (var(--separator)/--border 等 69+ 处线条一起变)+ 整窗红框,作为"危险中"的持续视觉信号。
  createEffect(() => {
    if (typeof document !== "undefined") {
      document.body.classList.toggle("danger-active", dangerMode());
    }
  });

  // ★界面外观★:dark = 默认(无类);light = 挂 body.theme-light(CSS 变量级整体翻转成暖中性亮色);
  // auto = 跟随系统 prefers-color-scheme,且注册监听,系统明暗一变即时重切(仅 auto 档生效)。
  // 与 danger-active 同机制(变量级覆盖,零侵入逐元素改)。dark/light/auto 三档,setTheme 持久 localStorage。
  // 切换时给 <body> 短暂挂 theme-animating → CSS 让颜色相关属性平滑渐变(首帧跳过,避免开场闪)。
  let themeAnimReady = false;
  let themeAnimTimer: ReturnType<typeof setTimeout> | undefined;
  createEffect(() => {
    if (typeof document === "undefined") return;
    const pref = theme();
    const mql = typeof window !== "undefined" && window.matchMedia
      ? window.matchMedia("(prefers-color-scheme: dark)")
      : null;
    const apply = () => {
      const isLight = pref === "light" || (pref === "auto" && mql !== null && !mql.matches);
      if (themeAnimReady) {
        document.body.classList.add("theme-animating");
        if (themeAnimTimer) clearTimeout(themeAnimTimer);
        themeAnimTimer = setTimeout(() => document.body.classList.remove("theme-animating"), 480);
      }
      document.body.classList.toggle("theme-light", isLight);
    };
    apply();
    themeAnimReady = true; // 首帧之后再开启过渡
    // 仅 auto 档需要随系统变化重算;其余两档固定,无需监听。
    if (pref === "auto" && mql) {
      mql.addEventListener("change", apply);
      onCleanup(() => mql.removeEventListener("change", apply));
    }
  });

  // ★完整保真展示记录·自动存★:消息每次变动都 track;一旦本轮静默(没有消息在流式中),防抖 1s
  // 整存当前项目的界面记录(含思考/工具卡/meta)。下次重启/切项目 resetAndLoadHistory 原样还原。
  // 防抖合并流式期间的高频变动;空列表由 saveTranscript 内部跳过(不覆盖已有记录)。
  let saveTxTimer: ReturnType<typeof setTimeout> | undefined;
  createEffect(() => {
    const msgs = messages();
    if (saveTxTimer) clearTimeout(saveTxTimer); // 先撤上一个待执行的存,防多气泡回合中途误存
    if (msgs.some((m) => m.streaming)) return;   // 流式中,等本轮 settle 再存
    saveTxTimer = setTimeout(() => { void saveTranscript(); }, 1000);
  });

  onMount(() => {
    startPolling();
    void refreshDict();
    // 版本号:从后端单一源(workspace Cargo.toml)拉一次,显示绝不硬编码。
    void api.appVersion().then(setAppVersion).catch(() => {});
    void startDirAccessListener();
    // 后端来源提示(如后台任务完成/失败)→ "notice" 事件 → 就地按界面语言弹 toast。
    void startNoticeListener();
    // 把持久化的"工具显示截断" / "自动模式"偏好同步给后端(UI 是真相,backend Settings 跟随)。
    void api.setTruncateToolDisplay(truncateToolDisplay()).catch(() => {});
    void api.setAutoMode(autoMode()).catch(() => {});
    // danger 会话级、默认关:启动时显式推后端复位(防任何残留),与前端 dangerMode 信号(默认 false)一致。
    void api.setDangerMode(dangerMode()).catch(() => {});

    // 拦截窗口关闭：后端 emit close-requested → 弹确认框 → 确认调 confirm_app_exit
    void listen("close-requested", async () => {
      const confirmed = await requestCloseConfirm();
      if (confirmed) {
        await api.confirmAppExit();
      }
    });
    // 未连 LM 也能 list 项目/看历史(用户 v0.2.8 原话)
    void bootstrapOffline();
    // 实时上下文压力:后端每个 LLM 回合(上一轮工具结果灌入后)emit "context-tokens" → 立刻刷新仪表,
    // 不等 2s 轮询 → 看得到逐次工具调用后的上下文动态增长(用户要求)。轮询仍兜底(断连/漏事件)。
    void listen<number>("context-tokens", (tokens) => {
      setStatusInfo((prev) => (prev ? { ...prev, ctx_prompt_tokens: tokens } : prev));
    });
    // ★用户决定脊柱★:凡需用户裁决的动作(路径授权 / shell 审批)都经唯一 "decision-request" 事件,
    // 按 kind 路由到对应弹窗;用户裁决经 decision_ack(id, decision) 回投阻塞中的脊柱。
    void listen<{ id: string; kind: string; path?: string; reason?: string; access?: string; privacy?: boolean; command?: string }>(
      "decision-request",
      (p) => {
        if (p.kind === "shell_approval") {
          showShellApproval(p.id, p.command ?? "");
        } else if (p.kind === "path_permission") {
          showPermissionRequest(p.id, p.path ?? "", p.reason ?? "", p.access ?? "write", !!p.privacy);
        }
      }
    );
    // LLM switch_project 工具 → 后端 emit "project-switched" → 前端同步 sidebar
    void listen<{id: string; name: string; work_dir: string}>("project-switched", (payload) => {
      setCurrentProjectId(payload.id);
      void refreshProjects();
      void refreshProjectDirs();
      // ★切项目重载聊天记录★:后端 get_chat_history 按当前项目过滤(fc30215),但前端原先切项目
      // 不重载 messages,导致聊天框停在旧项目。这里单点补上(用户点切/AI 切/建项目都经此事件)。
      void resetAndLoadHistory();
    });
    // 活的 IDE:后端所有 UI 意图(家族一弹表单 + 家族二 ui_control 往返)经唯一分发器落地。
    // 替掉了原来的 if/else action 链(前端版"三套分发打架"的根),新增面板只改 ui-actions.ts。
    void listen<{action: string; data?: Record<string, unknown>; id?: string}>("ui-action", (payload) => {
      dispatchUiAction(payload.action, payload.data, payload.id);
    });
    // 网页调试(Phase 2):调试 webview 里框选+建议经本机 HTTP 回传 → 后端 emit web-debug-edit →
    // 这里拼消息走 doSend,AI 在本地源码定位改(复用 Phase 1 改源回合)。
    void listen<{ url?: string; suggestion?: string; selection?: { elements?: Array<{ selector?: string; text?: string; outerHTML?: string }> } }>(
      "web-debug-edit",
      (payload) => {
        void sendWebDebugEdit(payload);
      },
    );
    // 向后端声明本前端可被 LLM 操控的面板(单一真相在前端,ui_control 据此生成 schema)。
    void registerUiSurfaces();
    // 感知闭合:监视各面板可见态,任何变化(含用户手动开关)上报后端 → get_control_state 恒真 + agent 可感知。
    watchPanelsAndReport();
    // agentic loop 中 switch_project 成功 → 对话结束后 emit project-navigate → 倒计时后切换对话
    void listen<{id: string; name: string; delay: number}>("project-navigate", (payload) => {
      const { name, delay } = payload;
      let remaining = delay;
      notify("project.navigate_countdown", { remaining, name }, delay * 1000);
      const timer = setInterval(() => {
        remaining -= 1;
        if (remaining <= 0) {
          clearInterval(timer);
          void (async () => {
            await api.resetChatSession();
            await resetAndLoadHistory();
            notify("chat.reset");
          })();
        }
      }, 1000);
    });
    // E2E 调试钩子仅测试构建注入(VITE_GROWBOX_DEBUG)。正式构建静态条件为 false,
    // 整段被 Vite 删除,window 上不挂任何 __GROWBOX_TEST__ / 自测逻辑。
    if (import.meta.env.VITE_GROWBOX_DEBUG) {
      // e2e 自测模式：localStorage growbox_e2e=1 → 启动后 3s 自动跑全套 UI 检查
      if (typeof localStorage !== "undefined" && localStorage.getItem("growbox_e2e") === "1") {
        setTimeout(() => {
          const gb = (window as any).__GROWBOX__;
          if (gb?.runFullTest) {
            const report = gb.runFullTest();
            console.log("[GROWBOX_E2E]", JSON.stringify(report));
            const msg = `E2E 自测: ${report.pass}/${report.pass + report.fail} 通过`;
            showToast(msg, report.fail === 0 ? "success" : "warn", 10000);
          }
        }, 3000);
      }
      // e2e test hook：通过 /eval 可读取 store 实时状态（不可写——保持单向）
      window.__GROWBOX_TEST__ = {
        version: "0.1.0",
        messages: () => messages(),
        sending: () => sending(),
        connected: () => connected(),
        statusInfo: () => statusInfo(),
        projects: () => projects(),
        currentProjectId: () => currentProjectId(),
        intentEvents: () => intentEvents(),
        flashingPaths: () => flashingPaths(),
        doSend,
        doConnect,
        // 走前端真实操作路径（不直接 invoke）：Sidebar 项目切换 dropdown
        // 点击调的就是 switchToProject。e2e 测试通过这个 hook 模拟点击。
        doSwitchProject: switchToProject,
        // reset 当前 chat：调后端 reset_chat_session + 清前端 messages。
        // 等价于点 ChatArea 顶 toolbar "新会话" 按钮（跳过 confirm）。
        doResetChat: async () => {
          await api.resetChatSession();
          setMessages([]);
        },
      };
    }
  });
  return (
    <>
      <div class="main">
        <Sidebar />
        <ChatArea />
      </div>
      <StatusBar />
      <Settings />
      <SystemHealthMonitor />
      <HistoryDrawer />
      <ProjectCreateModal />
      <AddPathModal />
      <PermissionDialog onGrant={(path, kind) => {
        // 按访问类型正确分流授权(用户决策 2026-06-02:只读/shell 不该被授权成可写目录)。
        if (kind === "shell") {
          // shell 的 path 是命令字符串非路径:授权 = 项目级 shell 放行,绝不加进目录列表。
          void api.grantShellAccess().catch(() => {});
          return;
        }
        if (kind === "net") {
          // net 的 path 是 URL:授权 = 项目级放行该主机(web_fetch 出站),绝不加进目录列表。
          // 失败要告知(用户以为授了权,下次又弹)。
          void api.grantNetHost(path).catch((e) => notify("project.net_grant_failed", { detail: String(e) }));
          return;
        }
        // 取 path 的父目录作为目录加入项目。
        const sep = path.includes("/") ? "/" : "\\";
        const parts = path.split(sep);
        const parentDir = parts.length > 1 ? parts.slice(0, -1).join(sep) : path;
        // 如果 path 本身已经是目录（末尾无文件扩展名启发判断），直接用
        const target = path.endsWith(sep) || !parts[parts.length - 1].includes(".") ? path : parentDir;
        // 只读访问 → 只授 readonly(读 /usr/bin 不该让它变可写);写访问 → writable。
        void addPath(kind === "read" ? "readonly" : "writable", target);
      }} />
      <ShellApprovalDialog />
      <SuggestionDialog />
      <ChoicePopup />
      <MemoryViz />
      <DreamPanel />
      <ArtifactCanvas />
      <TerminalPanel />
      <CloseConfirmDialog />
      <ToastContainer />
    </>
  );
};

export default App;
