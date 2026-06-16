// 异常告知 / 状态指示灯(状态栏)。设计文档 异常告知.md:状态栏**常驻**指示灯,颜色随状态变。
// 红=健康致命错误 / 橙=降级 / 黄=提示 / 蓝=未连接 / 绿=已连接且正常(健康异常优先于连接态)。点开看详情。
// 数据:get_status 轮询的 health 字段(后端 health.rs)+ connected 连接态。
import { Show, For, createSignal, createEffect, onCleanup, type Component } from "solid-js";
import { statusInfo, connected } from "../store";
import { t } from "../i18n";
import { noticeText } from "../notices";
import type { HealthLevel } from "../tauri-api";

const COLORS: Record<HealthLevel, string> = {
  ok: "#30d158",
  notice: "#ffd60a",
  degraded: "#ff9f0a",
  fatal: "#ff453a",
};
// 未连接 = 蓝(中性,与降级橙/提示黄区分明显;用户 2026-06-02:橙黄太接近)。
const DISCONNECTED_COLOR = "#0a84ff";
// 健康等级 → i18n key(走 t() 多语言;t 是响应式,切语言自动重渲)。
const LABEL_KEYS: Record<HealthLevel, string> = {
  ok: "healthOk",
  notice: "healthNotice",
  degraded: "healthDegraded",
  fatal: "healthFatal",
};

const HealthIndicator: Component = () => {
  const [open, setOpen] = createSignal(false);
  const [copied, setCopied] = createSignal(false);
  const health = () => statusInfo()?.health;
  const level = (): HealthLevel => health()?.level ?? "ok";
  const issues = () => health()?.issues ?? [];
  const hasIssues = () => issues().length > 0;
  // 综合状态色:健康异常优先(红 fatal / 橙 degraded / 黄 notice);
  // 健康正常时反映连接态——绿=已连接且正常,蓝=未连接。
  const color = () => {
    if (level() !== "ok") return COLORS[level()];
    return connected() ? COLORS.ok : DISCONNECTED_COLOR;
  };
  const stateLabel = () =>
    level() !== "ok"
      ? t(LABEL_KEYS[level()])
      : connected()
      ? t("healthConnectedOk")
      : t("disconnected");

  // 点击展开区域之外的任意空白处即收起(用户 2026-06-02:展开是点开的,关闭也应顺手)。
  // 含整组 wrapper 判定:点灯按钮/弹窗内部(如复制按钮)都不算"外部",不误关;
  // 仅当点到 wrapper 之外才关。监听只在展开时挂,收起即摘(onCleanup)。
  let wrapperRef: HTMLDivElement | undefined;
  createEffect(() => {
    if (!open()) return;
    const onDocClick = (e: MouseEvent) => {
      if (wrapperRef && !wrapperRef.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("click", onDocClick);
    onCleanup(() => document.removeEventListener("click", onDocClick));
  });

  // 复制异常详情(含 code,便于联网查询)。带 code 是因为搜引擎/AI 靠它精确定位。
  const copyIssues = async () => {
    const lines = issues().map((it) => `- [${it.code}] ${noticeText(it.code, it.params)}`).join("\n");
    const text = `[GrowBox ${t(LABEL_KEYS[level()])}]\n${lines}`;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // 剪贴板不可用(罕见):静默,不打断告警本身
    }
  };

  // 常驻:始终显示一个状态点,颜色随状态变(不再"绿就隐藏")。
  return (
    <div ref={wrapperRef} style={{ position: "relative", display: "inline-flex", "align-items": "center" }}>
      <button
        onClick={() => setOpen((o) => !o)}
        title={`${stateLabel()}:${t("healthClickDetail")}`}
        style={{
          display: "inline-flex",
          "align-items": "center",
          gap: "5px",
          border: "none",
          background: "transparent",
          cursor: "pointer",
          color: color(),
          font: "inherit",
          padding: "0 6px",
        }}
      >
        <span
          style={{
            width: "8px",
            height: "8px",
            "border-radius": "50%",
            background: color(),
            "box-shadow": `0 0 6px ${color()}`,
            animation: level() === "fatal" ? "gb-health-pulse 1s infinite" : "none",
          }}
        />
        {/* 仅健康异常时显示文字命名问题;正常/未连接只亮点(连接态右侧状态栏已示,避免重复) */}
        <Show when={level() !== "ok"}>
          <span style={{ "font-size": "11px", "font-weight": 600 }}>{t(LABEL_KEYS[level()])}</span>
        </Show>
      </button>
      <Show when={open()}>
        <div
          style={{
            position: "absolute",
            bottom: "22px",
            left: "0",
            "z-index": 2000,
            background: "#1c1c1e",
            border: `1px solid ${color()}`,
            "border-radius": "8px",
            padding: "10px 12px",
            "min-width": "260px",
            "max-width": "440px",
            "box-shadow": "0 6px 24px rgba(0,0,0,0.5)",
          }}
        >
          <div
            style={{
              "font-size": "12px",
              "font-weight": 700,
              "margin-bottom": "8px",
              color: color(),
            }}
          >
            {hasIssues() ? t("healthIssuesTitle") : stateLabel()}
          </div>
          <Show
            when={hasIssues()}
            fallback={
              <div style={{ "font-size": "12px", color: "#e5e5ea", "line-height": 1.4 }}>
                {connected() ? t("healthConnectedOk") : t("pleaseConnectFirst")}
              </div>
            }
          >
            <For each={issues()}>
              {(it) => (
                <div style={{ "font-size": "12px", "margin-bottom": "6px", "line-height": 1.4 }}>
                  <span style={{ color: COLORS[it.severity], "margin-right": "5px" }}>●</span>
                  <span style={{ color: "#e5e5ea" }}>{noticeText(it.code, it.params)}</span>
                </div>
              )}
            </For>
            {/* 复制按钮:右下角,风格匹配弹窗(暗底/细边/小字),复制异常详情便于联网查询 */}
            <div
              style={{
                display: "flex",
                "justify-content": "flex-end",
                "margin-top": "8px",
                "padding-top": "8px",
                "border-top": "1px solid rgba(255,255,255,0.08)",
              }}
            >
              <button
                onClick={(e) => {
                  e.stopPropagation();
                  void copyIssues();
                }}
                title={t("healthCopyTip")}
                style={{
                  display: "inline-flex",
                  "align-items": "center",
                  gap: "5px",
                  border: "1px solid rgba(255,255,255,0.14)",
                  background: "transparent",
                  color: copied() ? COLORS.ok : "#aeaeb2",
                  "border-color": copied() ? COLORS.ok : "rgba(255,255,255,0.14)",
                  cursor: "pointer",
                  "border-radius": "6px",
                  padding: "4px 9px",
                  "font-size": "11px",
                  "font-weight": 600,
                  transition: "color .14s, border-color .14s",
                }}
              >
                <Show
                  when={copied()}
                  fallback={
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                      <rect x="9" y="9" width="13" height="13" rx="2" />
                      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
                    </svg>
                  }
                >
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M20 6 9 17l-5-5" />
                  </svg>
                </Show>
                {copied() ? t("healthCopied") : t("healthCopy")}
              </button>
            </div>
          </Show>
        </div>
      </Show>
    </div>
  );
};

export default HealthIndicator;
