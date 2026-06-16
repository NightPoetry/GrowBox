// 感知告知 双受众 —— 前端"显示半 + 触发对内感知"(设计:设计文档/感知告知-双受众.md,Phase 2)。
//
// notify(code, params) 一处两受众:
//   ① 对外显示:按 ui_lang 取 human 模板、填参、就地 toast(无往返延迟)。
//   ② 对内感知:perceive=true 则 fire-and-forget 回后端 perceive_notice → 按 prompt_lang 渲染 llm
//      交 Memory::perceive,LLM 下一回合即可感知到这条用户级 UX 事件(自我感知原则)。
//
// 文案单一事实源 = prompts/notices.i18n.json,经后端 get_notice_catalog(ui_lang) 下发并缓存
// (镜像 get_tools 的"后端按 ui_lang 本地化文案"模式,绕开打包路径坑)。前端不再散装维护 toast 文案。
//
// 说明:后端来源的提示(失败/健康)→前端显示走 "notice" 事件 + 常驻指示灯,与 health 一并落到 Phase 3
// (当前无任何 surface=toast 的后端来源提示,提前加 emitter/listener 是死代码);本模块只管前端来源。

import { createSignal } from "solid-js";
import { api, listen, tauriAvailable, type NoticeEntry } from "./tauri-api";
import { showToast, type ToastKind } from "./toast";

// code → 目录条目(按当前界面语言渲染好的 human + 标志)。切界面语言时整张重拉。
const [catalog, setCatalog] = createSignal<Record<string, NoticeEntry>>({});

// severity 与 ToastKind 同集合,直接映射;非法值缺省 info。
function toastKind(sev: string): ToastKind {
  return sev === "success" || sev === "warn" || sev === "error" ? sev : "info";
}

// 瞬态 toast 停留时长按 severity(error 久、info 短);可被 notify 的 ttlMs 覆盖。
function defaultTtl(sev: string): number {
  if (sev === "error") return 6000;
  if (sev === "warn") return 4000;
  return 3000;
}

// {key} → params[key](字符串/数字),镜像后端 notice_i18n::fill。用 split/join 做全替换,免依赖 ES2021。
function fill(template: string, params: Record<string, unknown>): string {
  let out = template;
  for (const [k, v] of Object.entries(params)) {
    out = out.split(`{${k}}`).join(String(v));
  }
  return out;
}

/// 拉取对外目录(按界面语言)。App 挂载时调一次,切界面语言时重拉。浏览器无桥时静默跳过。
export async function loadNoticeCatalog(uiLang: string): Promise<void> {
  if (!tauriAvailable()) return;
  try {
    const list = await api.getNoticeCatalog(uiLang);
    const map: Record<string, NoticeEntry> = {};
    for (const e of list) map[e.code] = e;
    setCatalog(map);
  } catch (err) {
    console.warn("[notices] loadNoticeCatalog failed:", err);
  }
}

/// 按当前界面语言渲染某 code 的对外文案(填参)。用于非 toast 的就地渲染:
/// 如健康状态灯(surface=health)的常驻显示——它走轮询而非 toast,故不用 notify,直接取 human 渲染。
/// catalog 是信号,调用处在响应式上下文里会随界面语言/catalog 加载自动重渲。缺条目回退 code。
export function noticeText(code: string, params: Record<string, unknown> = {}): string {
  const entry = catalog()[code];
  return entry ? fill(entry.human, params) : code;
}

/// 仅"对外显示半":按 catalog 渲染并弹 toast(surface=toast 才弹;silent/health 不弹)。无感知副作用。
/// 后端来源的提示(经 "notice" 事件)走这条——对内感知后端已自理(perceive 或 Supervisor 回合)。
function displayNotice(code: string, params: Record<string, unknown> = {}, ttlMs?: number): void {
  const entry = catalog()[code];
  if (entry) {
    if (entry.surface === "toast") {
      showToast(fill(entry.human, params), toastKind(entry.severity), ttlMs ?? defaultTtl(entry.severity));
    }
  } else {
    // 目录未加载/未知 code:退化显示 code 本身(开发期可见),不静默吞掉。
    showToast(code, "info", ttlMs ?? 3000);
  }
}

/// 发出一条提示(前端来源):对外显示(human/ui_lang)+ 对内感知(llm/prompt_lang,经后端)。
/// 取代散落的 showToast(t(...)) —— 文案、受众、语言、感知标志全在 notices.i18n.json 单一源。
export function notify(code: string, params: Record<string, unknown> = {}, ttlMs?: number): void {
  displayNotice(code, params, ttlMs);
  // 感知半:perceive=true 才回后端(默认 true;目录未就绪时也回,最终由后端按 catalog 决定)。
  const entry = catalog()[code];
  const shouldPerceive = entry ? entry.perceive : true;
  if (shouldPerceive && tauriAvailable()) {
    void api.perceiveNotice(code, params).catch(() => {});
  }
}

/// 监听后端来源的提示(2×2 第四格【后端产生 × 对外显示】):后端 emit "notice" {code,params}
/// → 这里按界面语言就地渲染 toast(只显示,不感知——后端已负责对内)。App 挂载时启动一次。
export async function startNoticeListener(): Promise<void> {
  if (!tauriAvailable()) return;
  await listen<{ code: string; params?: Record<string, unknown> }>("notice", (p) => {
    displayNotice(p.code, p.params ?? {});
  });
}
