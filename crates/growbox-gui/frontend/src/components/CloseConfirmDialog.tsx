import { createSignal, Show, type Component } from "solid-js";
import { t } from "../i18n";

const [open, setOpen] = createSignal(false);
let _resolve: ((confirmed: boolean) => void) | null = null;

export function requestCloseConfirm(): Promise<boolean> {
  setOpen(true);
  return new Promise((resolve) => {
    _resolve = resolve;
  });
}

const CloseConfirmDialog: Component = () => {
  function confirm() {
    setOpen(false);
    _resolve?.(true);
    _resolve = null;
  }

  function cancel() {
    setOpen(false);
    _resolve?.(false);
    _resolve = null;
  }

  return (
    <Show when={open()}>
      <div class="permission-overlay">
        <div class="perm-backdrop" onClick={cancel} />
        <div class="permission-dialog">
          <h3>{t("closeConfirmTitle")}</h3>
          <p class="perm-desc">{t("closeConfirmDesc")}</p>
          <div class="perm-actions">
            <button onClick={cancel}>{t("cancel")}</button>
            <button class="primary danger" onClick={confirm}>{t("closeConfirmOk")}</button>
          </div>
        </div>
      </div>
    </Show>
  );
};

export default CloseConfirmDialog;
