import { Show, type Component } from "solid-js";
import { connected, statusInfo, sessionId, appVersion } from "../store";
import { t } from "../i18n";
import HealthIndicator from "./HealthIndicator";
import ShellTasks from "./ShellTasks";

function shortSid(sid: string): string {
  if (sid.length <= 12) return sid;
  return sid.slice(0, 8) + "…";
}

// 注:原状态栏「潜意识 wired」绿点(SubDot)已移除(2026-06-02 用户反馈:连接后它与健康灯绿点并排=两个绿点、易混淆)。
// 潜意识/嵌入状态本就在控制面板「双网络」区展示(主模型/潜意识模型/嵌入状态),状态栏不再重复——同 OPUS14 移除劳累度绿点的处理。

const StatusBar: Component = () => {
  return (
    <div class="statusbar">
      <div class="statusbar-section statusbar-left">
        <span class="statusbar-brand">
          GrowBox <Show when={appVersion()}><span class="statusbar-ver">v{appVersion()}</span></Show>
        </span>
        {/* 健康指示灯:即使未连接也显示(持久化等启动期异常先于 connect)。 */}
        <HealthIndicator />
        {/* 后台任务计数 "shell k"(#4):仅在有运行中任务时显示,点开看 tag/原命令/状态。 */}
        <ShellTasks />
      </div>
      <div class="statusbar-section statusbar-center">
        <Show when={connected() && statusInfo()?.model}>
          <span>{statusInfo()!.model}</span>
        </Show>
      </div>
      <div class="statusbar-section statusbar-right">
        <Show
          when={connected() && sessionId()}
          fallback={<span>{t("disconnected")}</span>}
        >
          <span>{shortSid(sessionId()!)}</span>
        </Show>
      </div>
    </div>
  );
};

export default StatusBar;
