import { createSignal, createEffect, onCleanup, For, Show, type Component } from "solid-js";
import { api } from "../tauri-api";
import { t } from "../i18n";

export interface ChoiceOption {
  id: string;
  label: string;
  description?: string;
  recommended?: boolean;
}

interface ChoiceState {
  title: string;
  description?: string;
  options: ChoiceOption[];
}

const [choiceData, setChoiceData] = createSignal<ChoiceState | null>(null);
const [selectedIdx, setSelectedIdx] = createSignal(0);

export function showChoicePopup(data: {
  title?: string;
  description?: string;
  options?: ChoiceOption[];
}) {
  setChoiceData({
    title: data.title ?? "",
    description: data.description,
    options: data.options ?? [],
  });
  // Pre-select the recommended option, or the first option
  const recIdx = (data.options ?? []).findIndex((o) => o.recommended);
  setSelectedIdx(recIdx >= 0 ? recIdx : 0);
}

function dismiss() {
  setChoiceData(null);
}

function confirmSelection() {
  const data = choiceData();
  if (!data) return;
  const opt = data.options[selectedIdx()];
  if (!opt) return;
  dismiss();
  void api.suggestionResponse(opt.id);
}

function selectDiscuss() {
  dismiss();
  void api.suggestionResponse("discuss");
}

function onBackdrop(e: MouseEvent) {
  if ((e.target as HTMLElement).classList.contains("choice-backdrop")) {
    selectDiscuss();
  }
}

const ChoicePopup: Component = () => {
  const data = () => choiceData();

  // Keyboard navigation
  createEffect(() => {
    if (!data()) return;

    const handler = (e: KeyboardEvent) => {
      const d = data();
      if (!d) return;
      const len = d.options.length;
      if (len === 0) return;

      if (e.key === "ArrowDown" || e.key === "j") {
        e.preventDefault();
        setSelectedIdx((prev) => (prev + 1) % len);
      } else if (e.key === "ArrowUp" || e.key === "k") {
        e.preventDefault();
        setSelectedIdx((prev) => (prev - 1 + len) % len);
      } else if (e.key === "Enter" && !e.isComposing && e.keyCode !== 229) {
        e.preventDefault();
        confirmSelection();
      } else if (e.key === "Escape") {
        e.preventDefault();
        selectDiscuss();
      }
    };

    window.addEventListener("keydown", handler);
    onCleanup(() => window.removeEventListener("keydown", handler));
  });

  return (
    <Show when={data()}>
      <div class="choice-overlay" onClick={onBackdrop}>
        <div class="choice-backdrop" />
        <div class="choice-dialog">
          <h3>{data()!.title}</h3>
          <Show when={data()!.description}>
            <p class="choice-desc">{data()!.description}</p>
          </Show>
          <div class="choice-options">
            <For each={data()!.options}>
              {(opt, idx) => {
                const isDiscuss = () => opt.id === "discuss";
                const isSelected = () => selectedIdx() === idx();
                return (
                  <>
                    <Show when={isDiscuss()}>
                      <div class="choice-separator" />
                    </Show>
                    <button
                      class={`choice-option${isSelected() ? " choice-selected" : ""}${opt.recommended ? " choice-recommended" : ""}${isDiscuss() ? " choice-discuss" : ""}`}
                      onClick={() => {
                        setSelectedIdx(idx());
                        if (isDiscuss()) {
                          selectDiscuss();
                        } else {
                          confirmSelection();
                        }
                      }}
                      onMouseEnter={() => setSelectedIdx(idx())}
                    >
                      <span class="choice-radio">
                        <span class={`choice-radio-inner${isSelected() ? " active" : ""}`} />
                      </span>
                      <span class="choice-option-body">
                        <span class="choice-label">
                          {opt.label}
                          <Show when={opt.recommended}>
                            <span class="choice-badge">{t("choiceRecommended")}</span>
                          </Show>
                        </span>
                        <Show when={opt.description}>
                          <span class="choice-option-desc">{opt.description}</span>
                        </Show>
                      </span>
                    </button>
                  </>
                );
              }}
            </For>
          </div>
          <div class="choice-actions">
            <button class="choice-cancel" onClick={selectDiscuss}>{t("choiceCancel")}</button>
            <button class="choice-confirm primary" onClick={confirmSelection}>{t("choiceConfirm")}</button>
          </div>
        </div>
      </div>
    </Show>
  );
};

export default ChoicePopup;
