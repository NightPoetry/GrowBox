// 发送消息 + 流式接 chat-chunk 事件。
// v1 frontend/index.html:267 doStreamRound 的语义化重写。

import { api, listen, type ChatResponse, type SelfDriveResponse } from "./tauri-api";
import {
  appendMessage, patchMessageById, nextMsgId,
  sending, setSending,
  connected,
  pushIntentEvent,
  setVerifyStatus,
  selfDriveActive, setSelfDriveActive,
  selfDriveIdleLimit, selfDriveGapSecs, selfDriveMaxRounds, selfDriveDigestEvery,
} from "./store";
import { notify } from "./notices";
import { showToast } from "./toast";
import { t } from "./i18n";
import { sfx } from "./sfx";
import { refreshToolKinds } from "./tools";
import { presentArtifactIfReady, beginArtifactThinking, pushArtifactThinking, endArtifactThinking } from "./components/ArtifactCanvas";

interface ChunkPayload {
  delta: string;
  kind?: string;
  detail?: string;
}

interface IntentMetaPayload {
  is_challenge: boolean;
  confidence: number;
  topic: string[];
}

const msgQueue: string[] = [];

// 终止当前回合(造物交互 v2 §2):用户按「终止」→ 通知后端置取消标志,脊柱下一检查点优雅收口。
// 队列里待发的批次也清掉(用户想停,不再续发)。前端不强行中断流,等后端收口的 done 自然停转。
export async function cancelSend(): Promise<void> {
  msgQueue.length = 0;
  // 「终止」= 真停下:同时关掉自驱续跑(否则本回合刚停又被自动续上,按钮形同虚设)。
  setSelfDriveActive(false);
  try {
    await api.cancelChat();
  } catch {
    // 未连接/无后端:忽略(无回合可终止)。
  }
}

// 造物交互回合(造物交互 v2 §1):离散交互(非 realtime)→ 跑一个"接收回合",写造物(render/selftest)
// 的进度进聊天(后端 ArtifactSink 已按二分只放写造物事件;端口落子/思考静默)。
// ★懒创建气泡★:只在收到第一条可见 chunk 才建 assistant 气泡 —— 纯端口/落子回合(无写造物)不留空泡。
// realtime 高频输入不进聊天(只跑后端 + present)。若已有回合在跑(sending),退回静默调用避免串聊。
export async function artifactReceiveRound(
  canvasId: string,
  callbackId: string,
  value: string,
  realtime: boolean,
): Promise<void> {
  if (realtime || sending()) {
    try {
      await api.artifactEvent(canvasId, callbackId, value, realtime);
    } finally {
      presentArtifactIfReady();
    }
    return;
  }
  let assistantId: ReturnType<typeof nextMsgId> | null = null;
  let full = "";
  let unlisten: (() => void) | null = null;
  setSending(true);
  // ★造物思考窗★:造物回合进行时,把 reasoning 推到造物浮层(用户在看造物、不在看聊天),
  // 否则盯着静止的棋盘干等不知 AI 在想什么。回合结束清。
  beginArtifactThinking();
  try {
    unlisten = await listen<ChunkPayload>("chat-chunk", (payload) => {
      const { delta, kind } = payload;
      if (kind === "keepalive" || kind === "round_start" || kind === "self_drive") return;
      if (kind === "thinking") {
        pushArtifactThinking(delta); // reasoning → 造物思考窗(不进聊天气泡)
        return;
      }
      if (assistantId === null) {
        // 第一条可见内容才建气泡(静默回合不留空泡)。
        assistantId = nextMsgId();
        appendMessage({ id: assistantId, role: "assistant", content: "", streaming: true, ts: Date.now(), lastActivity: Date.now() });
      }
      full += delta;
      patchMessageById(assistantId, { content: full, lastActivity: Date.now() });
    });
    await api.artifactEvent(canvasId, callbackId, value, realtime);
    if (assistantId !== null) patchMessageById(assistantId, { content: full, streaming: false });
  } catch {
    if (assistantId !== null) patchMessageById(assistantId, { streaming: false, content: full });
  } finally {
    if (unlisten) unlisten();
    endArtifactThinking();
    setSending(false);
    presentArtifactIfReady();
  }
}

// 网页调试(Phase 2):调试 webview 里框选 + 修改建议经本机 HTTP 回传 → 后端 emit "web-debug-edit" →
// 这里拼成带 DOM 上下文的消息走 doSend(复用改源回合)。AI 据选择器/outerHTML 在本地源码定位改。
export async function sendWebDebugEdit(payload: {
  url?: string;
  suggestion?: string;
  selection?: { elements?: Array<{ selector?: string; text?: string; outerHTML?: string }> };
}): Promise<void> {
  const sug = (payload?.suggestion ?? "").trim();
  const els = payload?.selection?.elements ?? [];
  if (!sug || els.length === 0) return;
  const lines = els
    .map((el, i) => `${i + 1}. ${el.selector ?? ""}${el.text ? `  文字:"${el.text}"` : ""}`)
    .join("\n");
  const html = els.map((el) => el.outerHTML ?? "").join("\n");
  const url = payload?.url ?? "";
  const msg =
    `${sug}\n\n[网页调试·框选] 我在调试窗(${url})里用套索选中了下面的元素。这是本地正在跑的网页,` +
    `请在**本地源码文件**里定位它(用 code_search 按 class/文本/结构反查;编译框架渲出的 DOM 是产物、` +
    `要推断到对应组件源文件),只改选中的部分、别动没选的,改完保存源码让 dev server 热更新:\n${lines}\n\n` +
    `选中元素的 outerHTML:\n${html}`;
  await doSend(msg);
  // 改源回合结束 → 刷新调试 webview(EJS/Express 无 HMR,靠这个看到改动;Vite 工程本就 HMR、刷一下无害)。
  await api.reloadDebugWebview().catch(() => {});
}

export async function doSend(text: string): Promise<void> {
  const trimmed = text.trim();
  if (!trimmed) return;
  if (!connected()) {
    notify("chat.not_connected");
    return;
  }

  appendMessage({
    id: nextMsgId(),
    role: "user",
    content: trimmed,
    ts: Date.now(),
  });

  if (sending()) {
    msgQueue.push(trimmed);
    return;
  }

  setSending(true);
  sfx.sent();
  try {
    await doStreamRound(trimmed);
    while (msgQueue.length > 0) {
      const batch = msgQueue.splice(0).join("\n\n");
      await doStreamRound(batch);
    }
  } finally {
    setSending(false);
  }
  // 本轮(用户发起)结束:若自驱续跑已激活,接力让 AI 自己继续往下做(见 runSelfDriveLoop)。
  void maybeSelfDrive();
}

// 注入一条内部消息(非用户说的话:面板裁决回流、系统通知等)重新驱动 Agent。
// 与 doSend 的区别:① 气泡 role="system"(渲染成内部通知,不冒充用户)② 走 send_internal_message
// (后端 perceive 感知 + 内部 seed,AI 有权"仅当信息"不执行)。见用户决策 2026-06-02。
export async function injectInternalMessage(text: string): Promise<void> {
  const trimmed = text.trim();
  if (!trimmed || !connected()) return;
  appendMessage({
    id: nextMsgId(),
    role: "system",
    content: trimmed,
    ts: Date.now(),
  });
  if (sending()) {
    // 前台正忙:作为内部信息排队,跟在当前轮之后(同 doSend 的队列语义)。
    msgQueue.push(trimmed);
    return;
  }
  setSending(true);
  try {
    await doStreamRound(trimmed, true);
    while (msgQueue.length > 0) {
      const batch = msgQueue.splice(0).join("\n\n");
      await doStreamRound(batch);
    }
  } finally {
    setSending(false);
  }
}

// LLM 长内容生成本身就有静默期。前端不应 abort，由后端 router HTTP 超时兜底。
// 这里只做"软提示"：60s 静默 → info toast；300s → warn toast；都不停止流。
// 后端 send_message_stream 已每 30s emit keepalive chunk，正常路径不会触发。
const STREAM_IDLE_SOFT_MS = 60_000;
const STREAM_IDLE_HARD_MS = 300_000;
const STREAM_WATCHDOG_TICK_MS = 5_000;

// 单个流式回合的通用内核:建 assistant 气泡、挂 chat-chunk/intent/status 监听、流式补字、收尾。
// `invokeBackend` = 实际驱动后端的调用(普通发送 / 内部消息 / 自驱续跑),返回该回合结果。
// 返回后端结果 + `streamedText`(本轮流式累积的全部内容,供自驱算跨轮进度指纹);
// 出错时兜底返回空结果(不抛,UI 已就地告知)。
async function streamRound(invokeBackend: () => Promise<ChatResponse>): Promise<ChatResponse & { streamedText: string }> {
  let assistantId = nextMsgId();
  appendMessage({
    id: assistantId,
    role: "assistant",
    content: "",
    streaming: true,
    ts: Date.now(),
    lastActivity: Date.now(),
  });

  let full = "";
  let thinkText = "";
  let firstChunk = true;
  let unlisten: (() => void) | null = null;
  let unlistenIntent: (() => void) | null = null;
  let unlistenStatus: (() => void) | null = null;
  let lastChunkTs = Date.now();
  let watchdogTimer: ReturnType<typeof setInterval> | null = null;

  try {
    // ★主动自检动效★:后端 chat-status 事件驱动"正在核查 xxx"动态指示器(随核查对象变)。
    unlistenStatus = await listen<{ label: string }>("chat-status", (p) => {
      setVerifyStatus(p?.label || "");
    });
    unlistenIntent = await listen<IntentMetaPayload>("intent-meta", (payload) => {
      pushIntentEvent({
        ts: Date.now(),
        is_challenge: payload.is_challenge,
        confidence: payload.confidence,
        topic: payload.topic ?? [],
      });
    });
    unlisten = await listen<ChunkPayload>("chat-chunk", (payload) => {
      lastChunkTs = Date.now();
      const { delta, kind } = payload;
      if (kind === "keepalive") return;
      if (kind === "round_start") {
        patchMessageById(assistantId, { streaming: false });
        assistantId = nextMsgId();
        full = "";
        thinkText = "";
        firstChunk = true;
        appendMessage({
          id: assistantId,
          role: "assistant",
          content: "",
          streaming: true,
          ts: Date.now(),
          lastActivity: Date.now(),
        });
        return;
      }
      if (kind === "self_drive") {
        patchMessageById(assistantId, { streaming: false });
        assistantId = nextMsgId();
        full = "";
        thinkText = "";
        firstChunk = true;
        appendMessage({
          id: assistantId,
          role: "assistant",
          content: "",
          streaming: true,
          ts: Date.now(),
          lastActivity: Date.now(),
        });
        return;
      }
      if (firstChunk) firstChunk = false;
      if (kind === "thinking") {
        thinkText += delta;
        patchMessageById(assistantId, { thinking: thinkText, lastActivity: lastChunkTs });
      } else if (kind === "tool_progress") {
        full += delta;
        patchMessageById(assistantId, { content: full, lastActivity: lastChunkTs });
      } else {
        full += delta;
        patchMessageById(assistantId, { content: full, lastActivity: lastChunkTs });
      }
    });

    // 软警告：不 abort 流，只给用户提示。后端有真正的超时兜底。
    let softWarned = false;
    let hardWarned = false;
    watchdogTimer = setInterval(() => {
      const idle = Date.now() - lastChunkTs;
      if (!hardWarned && idle > STREAM_IDLE_HARD_MS) {
        hardWarned = true;
        notify("chat.stream_idle_hard", { s: Math.round(idle / 1000) }, 8000);
      } else if (!softWarned && idle > STREAM_IDLE_SOFT_MS) {
        softWarned = true;
        notify("chat.stream_idle_soft", { s: Math.round(idle / 1000) }, 4000);
      }
    }, STREAM_WATCHDOG_TICK_MS);

    const r = await invokeBackend();
    // full = 流式累积的全部内容(多轮正文 + 内联工具进度);r.content = outcome.final_text 只有最后一段。
    // 必须优先保留 full,否则多轮回复会被最后一段覆盖、中间内容丢失(用户反馈 2026-06-02)。
    // 仅当流式什么都没收到(full 空)才用 r.content 兜底。
    const finalContent = full.length > 0 ? full : r.content;
    patchMessageById(assistantId, {
      content: finalContent,
      streaming: false,
      meta: {
        inputTokens: r.input_tokens,
        outputTokens: r.output_tokens,
        model: r.model,
      },
    });
    sfx.done();
    return { ...r, streamedText: full };
  } catch (e) {
    sfx.error();
    const errStr = String(e);
    patchMessageById(assistantId, { streaming: false, content: full });
    notify("chat.stream_error", { detail: errStr });
    return { content: full, model: "", input_tokens: 0, output_tokens: 0, streamedText: full };
  } finally {
    if (watchdogTimer) clearInterval(watchdogTimer);
    if (unlisten) unlisten();
    if (unlistenIntent) unlistenIntent();
    if (unlistenStatus) unlistenStatus();
    setVerifyStatus(""); // 回合结束:清掉核查动效

    // 回合结束:若 AI 在本轮渲染了造物(草稿),现在展示最终态(做好了才展示,用户 2026-06-04)。
    presentArtifactIfReady();
    // 回合可能新建了工作流(define_workflow)或连了 MCP → 刷新分类,使下条消息图标正确。
    void refreshToolKinds();
  }
}

// 普通流式回合(用户消息 / 内部消息)。保持原有 void 语义,既有调用方不变。
async function doStreamRound(text: string, internal = false): Promise<void> {
  await streamRound(() => (internal ? api.sendInternalMessage(text) : api.sendMessageStream(text)));
}

// ─────────────────── 自驱续跑(全自动模式下的"一直跑")───────────────────
// 激活后,每当 AI 自己停下来,就调后端 self_drive_step 注入一条"继续推进"种子(role=internal,
// 进记录不进历史),驱动它评估现状/治理屎山/动手做下一步,然后接力下一轮。直到:用户按「终止」
// (cancelSend 关掉开关)/ 关掉全自动模式 / 连续两轮没活干(确实没事可做了,自动暂停别空烧)。

// 防重入:同一时刻只允许一个续跑循环在跑(多处触发 maybeSelfDrive 也安全)。
let selfDriveLooping = false;

// 旋钮读取(用户铁律:数值皆可设;见 store.ts)。每轮现读 → 设置改了立刻生效;解析失败/越界回退默认。
// 连续多少轮"无进展"(没动手 或 与上轮近乎全等的退化)就判没事可做/卡死,自动暂停(默认 2,最小 1)。
function selfDriveIdleLimitVal(): number {
  const n = parseInt(selfDriveIdleLimit(), 10);
  return Number.isNaN(n) || n < 1 ? 2 : n;
}
// 每轮之间的小喘息(ms):给 UI 刷新 + 后台(idle 学习/整理)缝隙 + 降 API 速率(默认 3000;0=不间断)。
function selfDriveGapMsVal(): number {
  const s = parseFloat(selfDriveGapSecs());
  return Number.isNaN(s) || s < 0 ? 3000 : Math.round(s * 1000);
}
// 自驱总轮数软上限(默认 0=无限;到顶暂停交还用户,靠进度指纹兜底防失控)。
function selfDriveMaxRoundsVal(): number {
  const n = parseInt(selfDriveMaxRounds(), 10);
  return Number.isNaN(n) || n < 0 ? 0 : n;
}
// 每多少轮主动跑一次飞轮消化(默认 12;0=不主动消化)。
function selfDriveDigestEveryVal(): number {
  const n = parseInt(selfDriveDigestEvery(), 10);
  return Number.isNaN(n) || n < 0 ? 12 : n;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

// ★跨轮进度指纹(P1)★:把本轮流式全文规范化(折叠空白/小写/截断)成一个指纹。连续两轮指纹相同
// = AI 在原地打转(同样的工具+同样的输出),即便 did_work=true 也算"没进展"→ 防退化工具循环整夜烧钱。
function fingerprintRound(streamedText: string): string {
  return streamedText.replace(/\s+/g, " ").trim().toLowerCase().slice(0, 2000);
}

// 跑一轮自驱:建气泡、流式接 AI 的动手过程(种子本身不显示=不进历史)。
// 返回 didWork(是否调了非 finish/ask_user 工具)+ fingerprint(本轮产出指纹,供跨轮退化检测)。
async function selfDriveOneRound(): Promise<{ didWork: boolean; fingerprint: string }> {
  const r = (await streamRound(() => api.selfDriveStep())) as SelfDriveResponse & { streamedText: string };
  return { didWork: !!r.did_work, fingerprint: fingerprintRound(r.streamedText) };
}

// 自驱续跑主循环。幂等可重入(防重入锁);只在已激活 + 已连接 + 当前空闲时真正开跑。
export async function runSelfDriveLoop(): Promise<void> {
  if (selfDriveLooping) return;
  if (!selfDriveActive() || !connected() || sending()) return;
  selfDriveLooping = true;
  setSending(true); // 整个续跑期间保持"发送中"(终止按钮常驻、发送框不误触),循环结束才释放。
  let unproductive = 0; // 连续"无进展"轮数(没动手 或 与上轮近乎全等)。
  let rounds = 0;       // 本次续跑已跑轮数(用于总轮数软上限 + 周期性消化)。
  let lastFp = "";      // 上一轮产出指纹(跨轮退化检测)。
  try {
    while (selfDriveActive() && connected()) {
      // 用户优先:续跑期间用户敲了消息(发送框 Enter → doSend 见 sending 把它排进队列),
      // 就停掉自驱、先把用户的话当正常回合处理完——用户主动指挥永远盖过自动续跑。
      if (msgQueue.length > 0) {
        setSelfDriveActive(false);
        const batch = msgQueue.splice(0).join("\n\n");
        await streamRound(() => api.sendMessageStream(batch));
        break;
      }
      // 总轮数软上限(默认 0=无限):到顶暂停交还用户(再点循环按钮可继续)。
      const maxRounds = selfDriveMaxRoundsVal();
      if (maxRounds > 0 && rounds >= maxRounds) {
        setSelfDriveActive(false);
        showToast(t("selfDriveMaxReached"), "info", 6000);
        break;
      }
      const { didWork, fingerprint } = await selfDriveOneRound();
      rounds += 1;
      // 一轮跑完后再判:期间用户可能按了「终止」(cancelSend 关掉开关)。
      if (!selfDriveActive()) break;
      // ★无进展 = 没动手(只回话)或 与上轮近乎全等(退化工具循环)★ → 累计;有真进展则清零。
      const repeated = fingerprint !== "" && fingerprint === lastFp;
      lastFp = fingerprint;
      if (!didWork || repeated) {
        unproductive += 1;
        if (unproductive >= selfDriveIdleLimitVal()) {
          // 连续无进展:确实没事可做 / 陷入打转 → 自动暂停(再点循环按钮可继续)。
          setSelfDriveActive(false);
          showToast(t(repeated ? "selfDriveStuck" : "selfDrivePaused"), "info", 5000);
          break;
        }
      } else {
        unproductive = 0;
      }
      // ★周期性消化(P2)★:持续自驱时 idle 永不触发 → 经验只采集不压缩堆积;每 N 轮主动跑一次飞轮 digest。
      const digestEvery = selfDriveDigestEveryVal();
      if (digestEvery > 0 && rounds % digestEvery === 0) {
        await api.runDigestPass().catch(() => 0);
        if (!selfDriveActive() || !connected()) break;
      }
      await sleep(selfDriveGapMsVal());
      if (!selfDriveActive() || !connected()) break;
    }
  } finally {
    selfDriveLooping = false;
    setSending(false);
  }
}

// 触发点(发送回合结束 / 切换开关 / 重连后):激活且空闲就接力开跑。幂等。
export function maybeSelfDrive(): void {
  if (selfDriveActive() && connected() && !sending()) {
    void runSelfDriveLoop();
  }
}

