import { createSignal, createEffect, For, Show, type Component } from "solid-js";
import { projectCreateOpen, setProjectCreateOpen, projectPrefill, setProjectPrefill, projectCreateFromAgent, setProjectCreateFromAgent } from "../store";
import { createProject } from "../projects";
import { api } from "../tauri-api";
import { t } from "../i18n";
import { notify } from "../notices";
import { injectInternalMessage } from "../chat";

const ProjectCreateModal: Component = () => {
  const [id, setId] = createSignal("");
  const [name, setName] = createSignal("");
  const [writablePaths, setWritablePaths] = createSignal<string[]>([]);
  const [readonlyPaths, setReadonlyPaths] = createSignal<string[]>([]);
  const [desc, setDesc] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  // 用户是否手动改过 ID(用于 name→id 自动推导的一次性开关)
  let idTouched = false;

  function autoId(name: string): string {
    // 英文/数字/连字符保留，其余去掉；纯中文名 → 时间戳 fallback
    const s = name.toLowerCase().replace(/\s+/g, "-").replace(/[^a-z0-9-]/g, "");
    return s || `proj-${Date.now()}`;
  }

  createEffect(() => {
    const pf = projectPrefill();
    if (pf && projectCreateOpen()) {
      idTouched = false;
      if (pf.id) {
        setId(pf.id.toLowerCase().replace(/[^a-z0-9-]/g, "-"));
        idTouched = true;
      } else if (pf.name) {
        setId(autoId(pf.name));
      }
      if (pf.name) setName(pf.name);
      if (pf.writable?.length) setWritablePaths(pf.writable);
      if (pf.readonly?.length) setReadonlyPaths(pf.readonly);
      if (pf.description) setDesc(pf.description);
      setProjectPrefill(null);
    }
  });

  function reset() {
    idTouched = false;
    setId(""); setName(""); setWritablePaths([]); setReadonlyPaths([]); setDesc("");
  }

  async function submit() {
    if (busy()) return;
    if (!id().trim() || !name().trim() || writablePaths().length === 0) {
      // 静默失败太隐蔽——高亮缺失字段
      if (!id().trim()) notify("project.id_required");
      else if (!name().trim()) notify("project.name_required");
      else if (writablePaths().length === 0) notify("project.min_writable_required");
      return;
    }
    setBusy(true);
    const projName = name().trim();
    const projWritable = writablePaths()[0] ?? "";
    const fromAgent = projectCreateFromAgent();
    const ok = await createProject({
      id: id().trim(),
      name: projName,
      writable: writablePaths(),
      readonly: readonlyPaths(),
      description: desc().trim(),
    });
    setBusy(false);
    if (ok) {
      reset();
      // Agent 弹的(它已让位暂停)→ 项目已建并切为当前项目,注入续做消息重新驱动 Agent(模型 A)。
      if (fromAgent) {
        setProjectCreateFromAgent(false);
        void injectInternalMessage(
          t("projectCreatedMessage").replace("{name}", projName).replace("{dir}", projWritable),
        );
      }
    }
  }

  function cancel() {
    const fromAgent = projectCreateFromAgent();
    setProjectCreateOpen(false);
    reset();
    // Agent 弹的面板被用户关闭/取消 → 让 Agent 感知到(全链路感知),困惑而非静默绕过。
    if (fromAgent) {
      setProjectCreateFromAgent(false);
      void injectInternalMessage(t("projectCreateCancelledMessage"));
    }
  }

  function onBackdrop(e: MouseEvent) {
    if ((e.target as HTMLElement).classList.contains("backdrop")) cancel();
  }

  async function browseAndAdd(setter: typeof setWritablePaths) {
    const dir = await api.pickDirectory();
    if (dir) setter((prev) => prev.includes(dir) ? prev : [...prev, dir]);
  }

  function removePath(setter: typeof setWritablePaths, path: string) {
    setter((prev) => prev.filter((p) => p !== path));
  }

  function handleDrop(e: DragEvent, setter: typeof setWritablePaths) {
    e.preventDefault();
    const items = e.dataTransfer?.files;
    if (!items) return;
    const paths: string[] = [];
    for (let i = 0; i < items.length; i++) {
      const f = items[i] as File & { path?: string };
      if (f.path) paths.push(f.path);
    }
    if (paths.length > 0) {
      setter((prev) => {
        const next = [...prev];
        for (const p of paths) { if (!next.includes(p)) next.push(p); }
        return next;
      });
    }
  }

  function handleDragOver(e: DragEvent) { e.preventDefault(); }

  return (
    <Show when={projectCreateOpen()}>
      <div class="project-create-overlay visible" onClick={onBackdrop}>
        <div class="backdrop" />
        <div class="project-create-panel">
          <h3>{t("newProject")}</h3>
          <div class="field">
            <label>{t("projectId")}</label>
            <input
              placeholder="my-blog"
              autocomplete="off"
              value={id()}
              onInput={(e) => { idTouched = true; setId(e.currentTarget.value.toLowerCase().replace(/[^a-z0-9-]/g, "-")); }}
            />
            <div class="hint">{t("projectIdHint")}</div>
          </div>
          <div class="field">
            <label>{t("projectName")}</label>
            <input
              placeholder="My Blog"
              autocomplete="off"
              value={name()}
              onInput={(e) => { const v = e.currentTarget.value; setName(v); if (!idTouched) setId(autoId(v)); }}
            />
          </div>
          <div class="field">
            <label>{t("projectWritableLabel")}</label>
            <div
              class="path-drop-zone"
              onDrop={(e) => handleDrop(e, setWritablePaths)}
              onDragOver={handleDragOver}
            >
              <For each={writablePaths()}>
                {(p) => (
                  <div class="path-item">
                    <span class="path-text" title={p}>{p}</span>
                    <button class="path-remove" onClick={() => removePath(setWritablePaths, p)}>
                      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
                    </button>
                  </div>
                )}
              </For>
              <button class="path-browse" onClick={() => void browseAndAdd(setWritablePaths)}>
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
                {t("addWritable")}
              </button>
            </div>
          </div>
          <div class="field">
            <label>{t("projectReadonlyLabel")}</label>
            <div
              class="path-drop-zone"
              onDrop={(e) => handleDrop(e, setReadonlyPaths)}
              onDragOver={handleDragOver}
            >
              <For each={readonlyPaths()}>
                {(p) => (
                  <div class="path-item">
                    <span class="path-text" title={p}>{p}</span>
                    <button class="path-remove" onClick={() => removePath(setReadonlyPaths, p)}>
                      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
                    </button>
                  </div>
                )}
              </For>
              <button class="path-browse" onClick={() => void browseAndAdd(setReadonlyPaths)}>
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>
                {t("addReadonly")}
              </button>
            </div>
          </div>
          <div class="field">
            <label>{t("projectDescription")}{t("optional")}</label>
            <input
              autocomplete="off"
              value={desc()}
              onInput={(e) => setDesc(e.currentTarget.value)}
            />
          </div>
          <div class="actions">
            <button onClick={cancel} disabled={busy()}>{t("cancel")}</button>
            <button class="primary" onClick={() => void submit()} disabled={busy()}>
              {busy() ? "..." : t("confirm")}
            </button>
          </div>
        </div>
      </div>
    </Show>
  );
};

export default ProjectCreateModal;
