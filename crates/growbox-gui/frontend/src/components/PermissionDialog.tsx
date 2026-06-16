import { createSignal, Show, type Component } from "solid-js";
import { t } from "../i18n";
import { ackDecision } from "../decisions";

// 路径/能力授权弹窗(决定脊柱的一个弹窗):后端经 "decision-request"{id,kind:"path_permission",...} 弹出。
// access = 访问类型(write/read/shell):前端据此正确分流持久化授权(只读/shell 不该被授权成可写目录)。
// privacy = 命中用户设置的隐私文件夹:着重强调 + 二次确认(用户决策 2026-06-02)。
// 用户裁决经 decision_ack(id, decision) 回投等待的脊柱(脊柱阻塞等这个回执);确认=remember(放行+持久),取消=deny。
const [permRequest, setPermRequest] = createSignal<
  { id: string; path: string; reason: string; access: string; privacy?: boolean } | null
>(null);

export function showPermissionRequest(id: string, path: string, reason: string, access = "write", privacy = false) {
  setPermRequest({ id, path, reason, access, privacy });
}

export function permissionPending() { return permRequest(); }

const PermissionDialog: Component<{ onGrant: (path: string, kind: string) => void }> = (props) => {
  const req = () => permRequest();
  // 隐私文件夹二次确认:0=待第一次确认,1=待再次确认。
  const [confirmStage, setConfirmStage] = createSignal(0);

  function close() {
    setPermRequest(null);
    setConfirmStage(0);
  }

  // 把裁决投回脊柱(脊柱在 request_decision 里阻塞等它)。deny/超时都让脊柱解除阻塞。
  // 投递失败由 ackDecision 统一重试 + 双受众告知(用户裁决绝不静默丢弃)。
  function ack(decision: "remember" | "deny") {
    const r = req();
    if (r) ackDecision(r.id, decision);
  }

  function deny() {
    ack("deny");
    close();
  }

  function grant() {
    const r = req();
    if (!r) return;
    // 隐私文件夹:第一次点"确认"先进入二次确认,再点一次才真正授权。
    if (r.privacy && confirmStage() === 0) {
      setConfirmStage(1);
      return;
    }
    // 放行 + 记住(本会话当场执行经脊柱旁路;持久化经 onGrant 落项目配置,回合后生效)。
    ack("remember");
    props.onGrant(r.path, r.access);
    close();
  }

  function onBackdrop(e: MouseEvent) {
    if ((e.target as HTMLElement).classList.contains("perm-backdrop")) deny();
  }

  return (
    <Show when={req()}>
      <div class="permission-overlay" onClick={onBackdrop}>
        <div class="perm-backdrop" />
        <div class={`permission-dialog ${req()!.privacy ? "perm-privacy" : ""}`}>
          <h3>{req()!.privacy ? t("privacyPermTitle") : req()!.access === "net" ? t("netPermTitle") : t("permissionTitle")}</h3>
          <Show when={req()!.privacy}>
            <p class="perm-privacy-warn">{t("privacyPermWarn")}</p>
          </Show>
          <p class="perm-desc">{req()!.privacy ? t("privacyPermDesc") : req()!.access === "net" ? t("netPermDesc") : t("permissionDesc")}</p>
          <div class="perm-path">{req()!.path}</div>
          <p class="perm-reason">{req()!.reason}</p>
          <div class="perm-actions">
            <button onClick={deny}>{t("cancel")}</button>
            <button class={req()!.privacy && confirmStage() === 1 ? "primary danger" : "primary"} onClick={grant}>
              {req()!.privacy && confirmStage() === 1 ? t("privacyConfirmAgain") : t("confirm")}
            </button>
          </div>
        </div>
      </div>
    </Show>
  );
};

export default PermissionDialog;
