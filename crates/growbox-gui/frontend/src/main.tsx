/* @refresh reload */
import { render } from "solid-js/web";
import "./index.css";
import "highlight.js/styles/github-dark.css";
import App from "./App";

// 调试桥(window.__GROWBOX__)仅测试构建注入。正式构建里静态条件为 false,
// Vite 会把整个 growbox-debug chunk 摇树删除,不打进包。
if (import.meta.env.VITE_GROWBOX_DEBUG) {
  void import("./growbox-debug");
}

const root = document.getElementById("root");
if (!root) throw new Error("Root element #root not found");
render(() => <App />, root);
