// 手动模式 shell 逐条批准弹窗:显示命令 + 四个裁决。
// 后端经决定脊柱 emit "decision-request"{id,kind:"shell_approval",command} → 此处弹窗 → decision_ack(id,decision)。
import { createSignal, Show, type Component } from "solid-js";
import { t } from "../i18n";
import { ackDecision } from "../decisions";

// 统一决定脊柱的裁决字符串(见后端 Decision::parse)。
type Decision = "once" | "remember" | "trust_project" | "deny";

const [shellReq, setShellReq] = createSignal<{ id: string; command: string } | null>(null);

export function showShellApproval(id: string, command: string) {
  setShellReq({ id, command });
}

const ShellApprovalDialog: Component = () => {
  const req = () => shellReq();
  // 投递失败由 ackDecision 统一重试 + 双受众告知(用户裁决绝不静默丢弃)。
  function decide(decision: Decision) {
    const r = req();
    if (!r) return;
    ackDecision(r.id, decision);
    setShellReq(null);
  }
  return (
    <Show when={req()}>
      <div class="permission-overlay">
        <div class="perm-backdrop" />
        <div class="permission-dialog shell-approval-dialog">
          <h3>{t("shellApprovalTitle")}</h3>
          <p class="perm-desc">{t("shellApprovalDesc")}</p>
          <pre class="shell-approval-cmd"><code>{req()!.command}</code></pre>
          <div class="shell-approval-actions">
            <button class="sa-btn sa-primary" onClick={() => decide("once")}>{t("shellAllowOnce")}</button>
            <button class="sa-btn" onClick={() => decide("remember")}>{t("shellAlways")}</button>
            <button class="sa-btn" onClick={() => decide("trust_project")}>{t("shellTrustAll")}</button>
            <button class="sa-btn sa-deny" onClick={() => decide("deny")}>{t("shellDeny")}</button>
          </div>
        </div>
      </div>
    </Show>
  );
};

export default ShellApprovalDialog;
