import { createSignal, createEffect, Show, type Component } from "solid-js";
import { addPathOpen, setAddPathOpen, addPathPrefill, setAddPathPrefill, projectDirectories } from "../store";
import { addPath, movePath } from "../projects";
import { api } from "../tauri-api";
import { t } from "../i18n";

interface ConflictInfo {
  inKind: "writable" | "readonly";
  targetKind: "writable" | "readonly";
  path: string;
}

const AddPathModal: Component = () => {
  const [kind, setKind] = createSignal<"writable" | "readonly">("writable");
  const [path, setPath] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [conflict, setConflict] = createSignal<ConflictInfo | null>(null);

  createEffect(() => {
    const pf = addPathPrefill();
    if (pf && addPathOpen()) {
      if (pf.path) setPath(pf.path);
      if (pf.kind) setKind(pf.kind);
      setAddPathPrefill(null);
    }
  });

  function cancel() {
    setAddPathOpen(false);
    setPath("");
    setKind("writable");
    setConflict(null);
  }

  function kindLabel(k: "writable" | "readonly") {
    return k === "writable" ? t("projectWritableShort") : t("projectReadonlyShort");
  }

  async function submit() {
    if (busy()) return;
    const p = path().trim();
    if (!p) return;

    const dirs = projectDirectories();
    if (dirs) {
      const inWritable = dirs.writable.includes(p);
      const inReadonly = dirs.readonly.includes(p);
      if (inWritable || inReadonly) {
        const inKind: "writable" | "readonly" = inWritable ? "writable" : "readonly";
        setConflict({ inKind, targetKind: kind(), path: p });
        return;
      }
    }

    setBusy(true);
    const ok = await addPath(kind(), p);
    setBusy(false);
    if (ok) {
      setPath("");
      setKind("writable");
      setConflict(null);
    }
  }

  async function doMove() {
    const c = conflict();
    if (!c || busy()) return;
    setBusy(true);
    const ok = await movePath(c.inKind, c.targetKind, c.path);
    setBusy(false);
    if (ok) {
      setPath("");
      setKind("writable");
      setConflict(null);
    }
  }

  async function browse() {
    const dir = await api.pickDirectory();
    if (dir) setPath(dir);
  }

  function handleDrop(e: DragEvent) {
    e.preventDefault();
    const items = e.dataTransfer?.files;
    if (!items || items.length === 0) return;
    const f = items[0] as File & { path?: string };
    if (f.path) setPath(f.path);
  }

  function handleDragOver(e: DragEvent) { e.preventDefault(); }

  function onBackdrop(e: MouseEvent) {
    if ((e.target as HTMLElement).classList.contains("backdrop")) cancel();
  }

  return (
    <Show when={addPathOpen()}>
      <div class="project-create-overlay visible" onClick={onBackdrop}>
        <div class="backdrop" />
        <div class="project-create-panel" style={{ width: "420px" }}>
          <h3>{t("addProjectDir")}</h3>

          <Show when={conflict() !== null} fallback={
            <>
              <div class="field">
                <label>{t("pathType")}</label>
                <div class="kind-toggle">
                  <label class="kind-radio">
                    <input
                      type="radio"
                      name="pathKind"
                      value="writable"
                      checked={kind() === "writable"}
                      onChange={() => setKind("writable")}
                    />
                    <span>{t("projectWritableShort")}</span>
                  </label>
                  <label class="kind-radio">
                    <input
                      type="radio"
                      name="pathKind"
                      value="readonly"
                      checked={kind() === "readonly"}
                      onChange={() => setKind("readonly")}
                    />
                    <span>{t("projectReadonlyShort")}</span>
                  </label>
                </div>
              </div>
              <div class="field">
                <label>{t("absolutePath")}</label>
                <div
                  class="path-input-row"
                  onDrop={handleDrop}
                  onDragOver={handleDragOver}
                >
                  <input
                    autocomplete="off"
                    value={path()}
                    placeholder={t("addPathHint")}
                    onInput={(e) => setPath(e.currentTarget.value)}
                  />
                  <button class="path-browse-inline" onClick={() => void browse()}>
                    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
                  </button>
                </div>
              </div>
              <div class="actions">
                <button onClick={cancel} disabled={busy()}>{t("cancel")}</button>
                <button class="primary" onClick={() => void submit()} disabled={busy()}>
                  {busy() ? "..." : t("add")}
                </button>
              </div>
            </>
          }>
            {(() => {
              const c = conflict()!;
              const isSame = c.inKind === c.targetKind;
              return (
                <div class="dir-conflict-body">
                  <div class="dir-conflict-icon">
                    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                      <circle cx="12" cy="12" r="10"/>
                      <line x1="12" y1="8" x2="12" y2="12"/>
                      <line x1="12" y1="16" x2="12.01" y2="16"/>
                    </svg>
                  </div>
                  <p class="dir-conflict-msg">
                    {isSame
                      ? t("dirAlreadyInSame").replace("{k}", kindLabel(c.inKind))
                      : t("dirAlreadyInOther").replace("{from}", kindLabel(c.inKind))}
                  </p>
                  <p class="dir-conflict-path">{c.path}</p>
                  <div class="actions">
                    <button onClick={() => setConflict(null)} disabled={busy()}>{t("cancelAdd")}</button>
                    <Show when={!isSame}>
                      <button class="primary" onClick={() => void doMove()} disabled={busy()}>
                        {busy() ? "..." : t("moveToTarget").replace("{to}", kindLabel(c.targetKind))}
                      </button>
                    </Show>
                  </div>
                </div>
              );
            })()}
          </Show>
        </div>
      </div>
    </Show>
  );
};

export default AddPathModal;
