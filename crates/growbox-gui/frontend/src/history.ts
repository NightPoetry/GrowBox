// Chat history 会话隔离加载：
// 1. 进入项目 → 只加载当前（最新）session 的消息
// 2. 滚到顶部 → 当前 session 内继续加载更旧的消息
// 3. 当前 session 消息全部加载完 → 设 canLoadPrevSession=true
// 4. 用户点击"加载上一个会话" → 找到上一个 session，加载其消息

import { api, type ChatHistoryItem } from "./tauri-api";
import {
  messages, setMessages, nextMsgId,
  historyOldestTs, setHistoryOldestTs,
  historyHasMore, setHistoryHasMore,
  historyLoading, setHistoryLoading,
  loadingSessionId, setLoadingSessionId,
  setCanLoadPrevSession,
  suppressJumpButtonOnce,
  type MsgRole, type Msg,
} from "./store";

function toMsg(it: ChatHistoryItem) {
  return {
    id: nextMsgId(),
    role: (it.role as MsgRole),
    content: it.content,
    ts: Date.parse(it.ts) || Date.now(),
  };
}

// 完整保真记录里的一条富消息(存的就是前端 Msg,ts 是 epoch ms 数字)→ 还原成 Msg。
function toRichMsg(it: Record<string, unknown>): Msg {
  return {
    id: nextMsgId(),
    role: (it.role as MsgRole) ?? "assistant",
    content: typeof it.content === "string" ? it.content : "",
    thinking: typeof it.thinking === "string" && it.thinking ? it.thinking : undefined,
    meta: (it.meta && typeof it.meta === "object") ? (it.meta as Msg["meta"]) : undefined,
    ts: typeof it.ts === "number" ? it.ts : (Date.parse(String(it.ts)) || Date.now()),
  };
}

// 把"用户实际看到的"当前消息整存(剥掉 streaming/lastActivity 这类瞬态),按当前项目落库。
// 每回合静默后调一次(见 App.tsx 防抖 effect);重启/切项目时 restoreTranscript 原样还原。
export async function saveTranscript(): Promise<void> {
  const msgs = messages();
  if (msgs.length === 0) return; // 空列表不存,避免覆盖已有记录(如还原前的初始空态)。
  const slim = msgs.map((m) => ({
    id: m.id, role: m.role, content: m.content,
    thinking: m.thinking, meta: m.meta, ts: m.ts,
  }));
  try {
    await api.saveChatTranscript(slim);
  } catch (e) {
    console.warn("saveTranscript failed", e);
  }
}

// 重启/切项目还原:优先用完整保真记录(含思考块/工具卡/meta,与用时一模一样);
// 没存过(老项目)返回 false → 调用方回退时间线派生。
async function restoreTranscript(): Promise<boolean> {
  try {
    const t = await api.loadChatTranscript();
    if (Array.isArray(t) && t.length > 0) {
      setMessages(t.map((it) => toRichMsg(it as Record<string, unknown>)));
      // 完整记录已全在场:不再分页加载更旧 / 不提示"加载上一会话"(那是时间线派生路径的事)。
      setHistoryHasMore(false);
      setCanLoadPrevSession(false);
      setHistoryOldestTs(null);
      return true;
    }
  } catch (e) {
    console.warn("restoreTranscript failed", e);
  }
  return false;
}

export async function loadOlderChat(n: number = 30): Promise<void> {
  if (historyLoading() || !historyHasMore()) return;
  setHistoryLoading(true);
  try {
    const sid = loadingSessionId();
    const items = await api.getChatHistory(historyOldestTs(), n, sid);
    if (!items || items.length === 0) {
      setHistoryHasMore(false);
      setCanLoadPrevSession(true);
      return;
    }
    const older = items.map(toMsg);
    suppressJumpButtonOnce();
    setMessages((arr) => [...older, ...arr]);
    setHistoryOldestTs(items[0].ts);
    if (items.length < n) {
      setHistoryHasMore(false);
      setCanLoadPrevSession(true);
    }
  } catch (e) {
    console.warn("loadOlderChat failed", e);
  } finally {
    setHistoryLoading(false);
  }
}

/// 加载上一个会话的消息。找到比当前 loadingSessionId 更旧的 session。
export async function loadPreviousSession(n: number = 30): Promise<boolean> {
  setHistoryLoading(true);
  try {
    const sessions = await api.listSessions();
    const curSid = loadingSessionId();
    const curIdx = curSid ? sessions.findIndex((s) => s.session_id === curSid) : 0;
    const prevIdx = curIdx + 1;
    if (prevIdx >= sessions.length) {
      return false; // 没有更早的会话了
    }
    const prevSid = sessions[prevIdx].session_id;
    setLoadingSessionId(prevSid);
    setHistoryOldestTs(null);
    setHistoryHasMore(true);
    setCanLoadPrevSession(false);

    const items = await api.getChatHistory(null, n, prevSid);
    if (!items || items.length === 0) {
      setCanLoadPrevSession(true);
      return false;
    }
    const older = items.map(toMsg);
    suppressJumpButtonOnce();
    // 插入一个会话分隔标记
    const separator = {
      id: nextMsgId(),
      role: "system" as MsgRole,
      content: `── ${prevSid} ──`,
      ts: Date.parse(items[items.length - 1].ts) || Date.now(),
    };
    setMessages((arr) => [...older, separator, ...arr]);
    setHistoryOldestTs(items[0].ts);
    if (items.length < n) {
      setHistoryHasMore(false);
      setCanLoadPrevSession(true);
    }
    return true;
  } catch (e) {
    console.warn("loadPreviousSession failed", e);
    return false;
  } finally {
    setHistoryLoading(false);
  }
}

export async function resetAndLoadHistory(): Promise<void> {
  setMessages([]);
  // ★完整保真★:优先用保存的完整界面记录原样还原(思考/工具卡/回复全在,和用时一模一样)。
  if (await restoreTranscript()) return;
  // 回退:老项目/没存过完整记录 → 时间线派生(只有 user/assistant 正文),分页加载。
  const status = await api.getStatus().catch(() => null);
  const sid = status?.session_id ?? null;
  setLoadingSessionId(sid);
  setHistoryOldestTs(null);
  setHistoryHasMore(true);
  setCanLoadPrevSession(false);
  await loadOlderChat(30);
}
