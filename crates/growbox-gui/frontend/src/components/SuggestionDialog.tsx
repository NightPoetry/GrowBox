import { createSignal, For, Show, type Component } from "solid-js";
import { t } from "../i18n";
import { api } from "../tauri-api";

export interface SuggestionOption {
  label: string;
  description: string;
}

interface SuggestionState {
  title: string;
  options: SuggestionOption[];
}

const [suggestionData, setSuggestionData] = createSignal<SuggestionState | null>(null);

export function showSuggestion(title: string, options: SuggestionOption[]) {
  setSuggestionData({ title, options });
}

function dismiss() {
  setSuggestionData(null);
}

function onSelect(label: string) {
  dismiss();
  void api.suggestionResponse(label);
}

function onBackdrop(e: MouseEvent) {
  if ((e.target as HTMLElement).classList.contains("suggestion-backdrop")) {
    dismiss();
  }
}

const SuggestionDialog: Component = () => {
  const data = () => suggestionData();

  return (
    <Show when={data()}>
      <div class="suggestion-overlay" onClick={onBackdrop}>
        <div class="suggestion-backdrop" />
        <div class="suggestion-dialog">
          <h3>{data()!.title}</h3>
          <div class="suggestion-options">
            <For each={data()!.options}>
              {(opt, idx) => (
                <button
                  class={`suggestion-option${idx() === 0 ? " suggestion-first" : ""}`}
                  onClick={() => onSelect(opt.label)}
                >
                  <span class="suggestion-label">
                    {opt.label}
                    <Show when={idx() === 0}>
                      <span class="suggestion-badge">{t("suggestionRecommended")}</span>
                    </Show>
                  </span>
                  <span class="suggestion-desc">{opt.description}</span>
                </button>
              )}
            </For>
          </div>
        </div>
      </div>
    </Show>
  );
};

export default SuggestionDialog;
