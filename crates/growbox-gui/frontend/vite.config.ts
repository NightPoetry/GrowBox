import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

// Tauri 推荐前端 dev server 端口 1420（与旧 GUI 一致便于 phase 4 接入）
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  // Tauri 加载 dist/ 时 protocol 是 tauri://localhost/，绝对路径 /assets/xxx 解析失败 → 空白。
  // 用相对路径 ./assets/xxx 让 Tauri 正确定位。
  base: "./",
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    target: "safari15",
    minify: "esbuild",
    sourcemap: true,
  },
});
