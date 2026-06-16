// 每 2 秒调 get_status 写入 statusInfo signal。
// 注意：始终轮询（无论 connected 状态），让 store.connected 反映**后端真实状态**——
// 这样即使脚本走 raw invoke('connect') 绕过 UI，仪表/连接点也能正确显示。
// 副作用：未连接时也会拉一次 get_status，但开销极小（毫秒级、无 IO）。

import { api } from "./tauri-api";
import {
  connected, setConnected,
  connecting, setConnecting,
  setStatusInfo,
  sessionId, setSessionId,
  setConnectedAt,
} from "./store";
import { refreshProjects, refreshProjectDirs } from "./projects";

let timer: number | null = null;

async function pollOnce(): Promise<void> {
  try {
    const s = await api.getStatus();
    setStatusInfo(s);

    const backendConnected = s.connected;
    const sessionChanged = s.session_id != null && s.session_id !== sessionId();
    const connectChanged = backendConnected !== connected();

    if (connectChanged) {
      setConnected(backendConnected);
      setConnectedAt(backendConnected ? Date.now() : null);
      if (backendConnected && connecting()) {
        setConnecting(false);
      }
    }
    if (sessionChanged) {
      setSessionId(s.session_id ?? null);
    } else if (!s.session_id && sessionId()) {
      setSessionId(null);
    }

    // session_id 变（首次 connect 或切项目）→ 刷新项目/目录。
    // 注意:不在主聊天区加载历史——用户开新会话看到的是空白对话区。
    // 历史通过右侧 HistoryDrawer 面板独立浏览。
    if (backendConnected && (connectChanged || sessionChanged)) {
      void refreshProjects();
      void refreshProjectDirs();
    }

  } catch {
    // 静默
  }
}

export function startPolling(): void {
  if (timer != null) return;
  void pollOnce();
  timer = window.setInterval(() => void pollOnce(), 2000);
}
