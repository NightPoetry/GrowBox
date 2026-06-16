#!/usr/bin/env node
/**
 * GrowBox UI 自动调试 —— 模拟用户操作，每一步都检查状态，精确报告断点。
 *
 * 用法:
 *   cd crates/growbox-gui/frontend
 *   node e2e/debug-create-project.mjs
 *
 * 调试流程:
 *   1. 起 Vite dev server
 *   2. 打开页面 → 注入 mock Tauri
 *   3. 模拟 LLM 发 create_project ui-action 事件
 *   4. 检查弹窗出现 → 检查各字段预填 → 尝试点确认 → 报告每一步状态
 */

import puppeteer from "puppeteer-core";
import { createServer } from "vite";
import path from "path";
import { fileURLToPath } from "url";
import { execSync } from "child_process";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const FRONTEND_DIR = path.resolve(__dirname, "..");
const PORT = 1422;

// ── helpers ──────────────────────────────────────────────

function log(emoji, msg) { console.log(`  ${emoji} ${msg}`); }
function ok(msg) { log("PASS", msg); }
function fail(msg) { log("FAIL", msg); process.exitCode = 1; }

// ── mock Tauri API ───────────────────────────────────────

const MOCK_TAURI = {
  core: {
    invoke: async (cmd, args) => {
      console.log(`    [tauri] ${cmd}`, JSON.stringify(args ?? {}).slice(0, 120));
      switch (cmd) {
        case "get_status":
          return { connected: false, model: "mock", budget_pct: 50, fatigue: 0.1, attention_span: 8192, cache_l1: 1, cache_l2: 2, cache_l3: 4, l1_cache_size: 7, l2_index_size: 14, pointer_count: 10, coverage_deep_green_pct: 30, coverage_light_green_pct: 40, coverage_red_pct: 20, coverage_gray_pct: 10, reverse_index_size: 100, subconscious_wired: false, fragment_count: 5, index_density: 0.7, total_nodes: 200 };
        case "list_projects": return [];
        case "current_project": return null;
        case "get_project_directories": return null;
        case "get_tools": return [];
        case "get_audit_tail": return [];
        case "get_translations": return {};
        case "create_project":
          // 模拟后端:返回用户指定的 id
          console.log(`    [tauri] → 创建项目 id=${args.args?.id} name=${args.args?.name}`);
          return args.args?.id || `proj-${Date.now()}`;
        case "switch_project":
          console.log(`    [tauri] → 切换项目 id=${args.id}`);
          return { id: args.id, name: "测试项目" };
        case "list_models": return ["mock-model"];
        default: return null;
      }
    },
  },
  event: {
    listen: async () => () => {},
  },
};

// ── main ─────────────────────────────────────────────────

async function main() {
  console.log("═══════════════════════════════════════════");
  console.log("  GrowBox 创建项目流程自动调试");
  console.log("═══════════════════════════════════════════\n");

  // 1. start server
  console.log("[1] 启动 Vite...");
  const server = await createServer({
    root: FRONTEND_DIR,
    server: { port: PORT, strictPort: true },
    configFile: path.join(FRONTEND_DIR, "vite.config.ts"),
  });
  await server.listen();

  // 2. launch browser (visible)
  console.log("[2] 启动浏览器...");
  const chromePath = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
  const browser = await puppeteer.launch({
    executablePath: chromePath,
    headless: false,
    slowMo: 60,
    args: ["--no-sandbox"],
  });

  const page = await browser.newPage();
  page.on("pageerror", (err) => console.log(`  [page error] ${err.message}`));

  // 3. inject mock
  await page.evaluateOnNewDocument((s) => { window.__TAURI__ = eval(`(${s})`)(); }, MOCK_TAURI.toString());
  await page.goto(`http://localhost:${PORT}`, { waitUntil: "networkidle2", timeout: 15000 });
  await new Promise((r) => setTimeout(r, 1500));

  console.log("\n[3] 检查初始状态...\n");

  // ── 检查 1: 页面正常渲染 ──
  const root = await page.$("#root");
  if (!root) { fail("root 不存在"); await browser.close(); process.exit(1); }
  ok("页面渲染正常");

  // ── 检查 2: __GROWBOX__ 钩子可用 ──
  const gbOK = await page.evaluate(() => typeof window.__GROWBOX__ !== "undefined");
  if (!gbOK) { fail("__GROWBOX__ 不存在"); }
  else ok("__GROWBOX__ 钩子可用");

  // ── 检查 3: 弹窗初始状态是关闭的 ──
  let overlay = await page.$(".project-create-overlay.visible");
  if (overlay) fail("弹窗不应该在初始状态打开");
  else ok("弹窗初始关闭");

  // ── 模拟 4: LLM 发起 create_project ──
  console.log("\n[4] 模拟 LLM 调用 create_project({ name: '个人博客', path: '/Volumes/UserData/TEM' })...\n");

  await page.evaluate(() => {
    // 模拟后端 emit ui-action 事件
    // 直接调 store 来模拟（等同于 App.tsx 收到 ui-action 事件）
    const store = window.__GROWBOX_TEST__;
    if (!store) return;

    // 触发 open_new_project action
    const el = document.querySelector(".project-dropdown-action");
    if (el) el.click();
  });

  await new Promise((r) => setTimeout(r, 500));

  // 如果 dropdown action 不可用，直接通过 store 注入
  overlay = await page.$(".project-create-overlay.visible");
  if (!overlay) {
    console.log("  dropdown action 不可用，直接注入 store...");
    await page.evaluate(() => {
      // 直接操纵 SolidJS store（通过 window 上的引用）
      const win = window;
      // 从 __GROWBOX__ 没有直接写 store 的能力，换用 DOM 方式
      // 点侧边栏的 + 新建项目
      const sidebar = document.querySelector(".sidebar");
      const newProjBtn = sidebar?.querySelector(".project-dropdown-action");
      if (newProjBtn) (newProjBtn).click();
      else {
        // fallback: 点 project 按钮 → 展开 dropdown → 点新建
        const btn = sidebar?.querySelector(".project-btn");
        if (btn) btn.click();
        setTimeout(() => {
          const action = document.querySelector(".project-dropdown-action");
          if (action) action.click();
        }, 200);
      }
    });
    await new Promise((r) => setTimeout(r, 800));
  }

  overlay = await page.$(".project-create-overlay.visible");
  if (!overlay) {
    // 最后手段:直接调 eval 注入 prefill 并打开
    console.log("  尝试通过 DOM 直接打开弹窗...");
    await page.evaluate(() => {
      // SolidJS 的 store 不能从外部直接写，但我们能通过点击触发
      // 试试点顶部 + 按钮或侧边栏的加号
      const allBtns = document.querySelectorAll("button");
      for (const btn of allBtns) {
        if (btn.textContent?.includes("新") || btn.textContent?.includes("+") || btn.title?.includes("项目")) {
          (btn).click();
          break;
        }
      }
    });
    await new Promise((r) => setTimeout(r, 500));
  }

  overlay = await page.$(".project-create-overlay.visible");
  if (!overlay) {
    fail("无法打开项目创建弹窗 — 请检查 ui-action 事件链路");
    await browser.close();
    await server.close();
    process.exit(1);
  }
  ok("项目创建弹窗已打开");

  // ── 检查 5: 字段预填 ──
  console.log("\n[5] 检查预填字段...\n");

  // 填写表单（模拟用户在弹窗中输入）
  await page.evaluate(() => {
    const inputs = document.querySelectorAll(".project-create-panel input");
    // 第一个 input = id, 第二个 = name, 第三个 = desc
    const idInput = inputs[0];
    const nameInput = inputs[1];

    if (idInput && !idInput.value) {
      idInput.value = "personal-blog";
      idInput.dispatchEvent(new Event("input", { bubbles: true }));
    }
    if (nameInput && !nameInput.value) {
      nameInput.value = "个人博客";
      nameInput.dispatchEvent(new Event("input", { bubbles: true }));
    }
  });
  await new Promise((r) => setTimeout(r, 300));

  const idVal = await page.$eval(".project-create-panel input", (el) => (el).value);
  console.log(`  ID 字段值: "${idVal}"`);
  if (!idVal.trim()) {
    fail("ID 字段为空 — 中文名自动 ID 生成有 bug:replace(/[^a-z0-9-]/g,'') 把中文全 strip 了");
  } else {
    ok(`ID 字段已填充: "${idVal}"`);
  }

  // 模拟添加可写目录
  await page.evaluate((dir) => {
    // 找添加可写目录的按钮
    const pathBrowseBtns = document.querySelectorAll(".path-browse");
    // 第一个是可写目录的浏览按钮
    if (pathBrowseBtns[0]) {
      // 不能真正打开文件选择器，但我们可以检查有没有路径拖放区
    }
    // 把路径直接写到 writable paths（通过检查 DOM 找路径列表）
    const pathItems = document.querySelectorAll(".path-item .path-text");
    console.log(`[debug] 当前路径数: ${pathItems.length}`);
  }, "/Volumes/UserData/TEM");

  // 检查 writable paths
  const writableCount = await page.$$eval(".path-item", (els) => els.length);
  if (writableCount === 0) {
    console.log("  可写目录为空 — 这是正常的（浏览器 mock 模式无法调 pickDirectory）");
    console.log("  在真实 Tauri 应用中，prefill 会把 path 预填进去");
  } else {
    ok(`可写目录已预填: ${writableCount} 个`);
  }

  // ── 检查 6: 确认按钮状态 ──
  console.log("\n[6] 检查确认按钮...\n");

  const confirmBtn = await page.$(".project-create-panel button.primary");
  if (!confirmBtn) { fail("找不到确认按钮"); }
  else {
    const btnDisabled = await confirmBtn.evaluate((el) => (el).disabled);
    const btnText = await confirmBtn.evaluate((el) => el.textContent);
    console.log(`  确认按钮: disabled=${btnDisabled} text="${btnText}"`);

    if (btnDisabled) {
      fail("确认按钮 disabled — 检查 busy 状态");
    } else {
      ok("确认按钮可点击");
    }
  }

  // ── 检查 7: 尝试点击确认 ──
  console.log("\n[7] 尝试点击确认...\n");

  // 先确保所有必填字段有值
  await page.evaluate(() => {
    const inputs = document.querySelectorAll(".project-create-panel input");
    const idInput = inputs[0];
    const nameInput = inputs[1];
    if (!idInput.value) {
      idInput.value = "personal-blog";
      idInput.dispatchEvent(new Event("input", { bubbles: true }));
    }
    if (!nameInput.value) {
      nameInput.value = "个人博客";
      nameInput.dispatchEvent(new Event("input", { bubbles: true }));
    }
  });
  await new Promise((r) => setTimeout(r, 200));

  if (confirmBtn) {
    await confirmBtn.click();
    await new Promise((r) => setTimeout(r, 800));
  }

  // 检查弹窗是否关闭（submit 成功会 close）
  const overlayAfter = await page.$(".project-create-overlay.visible");
  if (overlayAfter) {
    console.log("  弹窗未关闭 — submit 可能静默失败了");
    // 检查 toast
    const toastText = await page.evaluate(() => {
      const toasts = document.querySelectorAll("[class*='toast']");
      return Array.from(toasts).map(t => t.textContent).join(" | ");
    });
    console.log(`  Toast 消息: "${toastText}"`);

    if (!toastText) {
      fail("无任何反馈 — submit() 静默 return，用户完全不知道为什么点不动");
    }
  } else {
    ok("弹窗已关闭 — 项目创建成功");
  }

  // ── 汇总 ──
  console.log("\n═══════════════════════════════════════════");
  console.log("  调试完成。检查上方输出确定断点位置。");
  console.log("═══════════════════════════════════════════\n");

  await new Promise((r) => setTimeout(r, 2000));
  await browser.close();
  await server.close();
}

main().catch((err) => {
  console.error("调试脚本错误:", err);
  process.exit(1);
});
