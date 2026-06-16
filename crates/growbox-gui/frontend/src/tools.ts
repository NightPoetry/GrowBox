// 工具开关 + 任务提交服务。

import { api } from "./tauri-api";
import { setToolList, toolList, setWorkflowNames, setMcpToolNames } from "./store";
import { notify } from "./notices";
import { currentLang } from "./i18n";

export async function refreshTools(): Promise<void> {
  try {
    // 传当前界面语言:后端按 ui_lang 返回本地化的 label/description(单一源)。
    const list = await api.getTools(currentLang());
    setToolList(list);
  } catch (e) {
    notify("tools.list_read_failed", { detail: String(e) });
  }
}

/// 刷新"工具分类"(哪些名是工作流 / MCP),给聊天里不同图标用。失败静默(图标退回默认扳手)。
export async function refreshToolKinds(): Promise<void> {
  try {
    const k = await api.getToolKinds();
    setWorkflowNames(Array.isArray(k?.workflows) ? k.workflows : []);
    setMcpToolNames(Array.isArray(k?.mcp) ? k.mcp : []);
  } catch {
    /* 未连接/查询失败:忽略,图标退回默认 */
  }
}

export async function toggleTool(name: string, enabled: boolean): Promise<void> {
  const cur = toolList();
  const next = cur.map((t) => (t.name === name ? { ...t, enabled } : t));
  setToolList(next);
  const enabledNames = next.filter((t) => t.enabled).map((t) => t.name);
  try {
    await api.setTools(enabledNames);
  } catch (e) {
    notify("tools.toggle_save_failed", { detail: String(e) });
    await refreshTools();
  }
}

