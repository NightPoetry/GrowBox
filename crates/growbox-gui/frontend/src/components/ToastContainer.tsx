import { For, type Component } from "solid-js";
import { getToasts } from "../toast";

const ToastContainer: Component = () => {
  return (
    <div class="toast-container">
      <For each={getToasts()}>
        {(t) => (
          <div class={`toast ${t.kind} ${t.fadingOut ? "fade-out" : ""}`}>
            {t.text}
          </div>
        )}
      </For>
    </div>
  );
};

export default ToastContainer;
