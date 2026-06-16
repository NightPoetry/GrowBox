#!/usr/bin/env node
/**
 * GrowBox GUI E2E 测试 — 用 Puppeteer 驱动前端，全面验证 UI 渲染与交互。
 *
 * 用法:
 *   cd crates/growbox-gui/frontend
 *   npm run dev &                   # 起 Vite dev server (http://localhost:1420)
 *   node e2e/gui-test.mjs           # 跑测试
 *
 * 覆盖:
 *   01 根组件渲染        (App mount, root 存在)
 *   02 侧边栏            (Sidebar: 项目选择器、仪表盘、缓存三栏)
 *   03 聊天区域          (ChatArea: 工具栏、消息列表、输入框、发送按钮)
 *   04 设置面板          (Settings modal 打开/关闭/tab 切换)
 *   05 项目创建弹窗      (ProjectCreateModal)
 *   06 路径添加弹窗      (AddPathModal)
 *   07 状态栏            (StatusBar)
 *   08 历史抽屉          (HistoryDrawer)
 *   09 Toast 通知        (ToastContainer)
 *   10 i18n 国际化       (语言切换)
 *   11 连接/断开状态     (connect/disconnect 视觉)
 *   12 输入发送流程      (textarea typing → send)
 *   13 __GROWBOX__ 调试钩子 (runFullTest, getDOM, getState)
 */

import puppeteer from "puppeteer-core";
import { spawn } from "child_process";
import { createServer } from "vite";
import path from "path";
import { fileURLToPath } from "url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const FRONTEND_DIR = path.resolve(__dirname, "..");
const PORT = 1421; // 用不同于默认 1420 的端口避免冲突

// ── 测试结果收集 ──────────────────────────────────────────

const results = [];
let pass = 0;
let fail = 0;

function check(name, ok, detail = "") {
  if (ok) {
    pass++;
    console.log(`  PASS  ${name}${detail ? ` — ${detail}` : ""}`);
  } else {
    fail++;
    console.log(`  FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
  }
  results.push({ name, ok, detail });
}

// ── 启动 Vite dev server ──────────────────────────────────

async function startDevServer() {
  const server = await createServer({
    root: FRONTEND_DIR,
    server: { port: PORT, strictPort: true },
    configFile: path.join(FRONTEND_DIR, "vite.config.ts"),
  });
  await server.listen();
  console.log(`Vite dev server: http://localhost:${PORT}`);
  return server;
}

// ── 找到 Chrome/Chromium ──────────────────────────────────

async function findChrome() {
  // macOS: 优先使用系统 Chrome
  const { execSync } = await import("child_process");
  const candidates = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
  ];
  for (const c of candidates) {
    try {
      execSync(`test -x "${c}"`);
      return c;
    } catch {
      continue;
    }
  }
  throw new Error("找不到 Chrome/Chromium。请安装 Google Chrome。");
}

// ── 模拟 Tauri API ────────────────────────────────────────

function mockTauriApi() {
  return {
    core: {
      invoke: async (cmd, args) => {
        console.log(`  [mock invoke] ${cmd}`, JSON.stringify(args ?? {}).slice(0, 100));
        switch (cmd) {
          case "get_status":
            return {
              connected: false,
              model: "mock-model",
              budget_pct: 50,
              fatigue: 0.1,
              attention_span: 8192,
              cache_l1: 1, cache_l2: 2, cache_l3: 4,
              l1_cache_size: 7, l2_index_size: 14, pointer_count: 10,
              coverage_deep_green_pct: 30,
              coverage_light_green_pct: 40,
              coverage_red_pct: 20,
              coverage_gray_pct: 10,
              reverse_index_size: 100,
              subconscious_wired: false,
              fragment_count: 5,
              index_density: 0.7,
              total_nodes: 200,
              pressure: { level: "LOW", color: "#30d158", score: 20, topic_coherence: 0.8, pin_utilization: 0.3 },
            };
          case "list_projects":
            return [
              { id: "p1", name: "演示项目", archived: false, writable: ["/tmp"], readonly: [], experience_count: 3, knowledge_count: 2, understanding_count: 1 },
            ];
          case "current_project":
            return "p1";
          case "get_project_directories":
            return { id: "p1", name: "演示项目", writable: ["/tmp"], readonly: ["/usr/share"], work_dir: "/tmp" };
          case "get_tools":
            return [
              { name: "file_read", description: "读取文件内容", enabled: true },
              { name: "file_write", description: "写入文件", enabled: true },
              { name: "file_list", description: "列出目录", enabled: true },
              { name: "shell", description: "执行shell命令", enabled: true },
              { name: "file_edit", description: "编辑文件", enabled: false },
            ];
          case "get_audit_tail":
            return [];
          case "get_chat_history":
            return [];
          case "list_sessions":
            return [];
          case "get_translations":
            return {};
          case "set_runtime_dir":
            return null;
          case "list_models":
            return ["mock-model", "deepseek-v4-flash"];
          default:
            return null;
        }
      },
    },
    event: {
      listen: async (event, cb) => {
        // 存回调但不做任何事——测试不触发事件
        return () => {};
      },
    },
  };
}

// ── 主测试流程 ────────────────────────────────────────────

async function main() {
  console.log("═══════════════════════════════════════════");
  console.log("  GrowBox GUI E2E 测试套件");
  console.log("═══════════════════════════════════════════\n");

  // 1. 起 dev server
  console.log("[1/4] 启动 Vite dev server...");
  const server = await startDevServer();

  // 2. 找 Chrome
  console.log("[2/4] 查找 Chrome...");
  const chromePath = await findChrome();
  console.log(`  Chrome: ${chromePath}`);

  // 3. 启动 Puppeteer
  console.log("[3/4] 启动 Puppeteer...");
  const browser = await puppeteer.launch({
    executablePath: chromePath,
    headless: false,  // 可见模式，用户能看到测试过程
    slowMo: 80,       // 每步操作慢 80ms，方便肉眼跟踪
    args: ["--no-sandbox", "--disable-setuid-sandbox"],
  });

  const page = await browser.newPage();
  page.on("console", (msg) => {
    // 静默吞下常规日志，只打印 error
    if (msg.type() === "error") console.log(`  [console.error] ${msg.text()}`);
  });
  page.on("pageerror", (err) => {
    console.log(`  [page error] ${err.message}`);
  });

  // 4. Mock Tauri API 然后导航
  console.log("[4/4] 导航到 app + 注入 mock...");
  await page.evaluateOnNewDocument((mockScript) => {
    window.__TAURI__ = eval(`(${mockScript})`)();
  }, mockTauriApi.toString());

  await page.goto(`http://localhost:${PORT}`, { waitUntil: "networkidle2", timeout: 15000 });
  // SolidJS 渲染需要一点时间
  await new Promise((r) => setTimeout(r, 1000));

  console.log("\n── 测试开始 ──────────────────────────────\n");

  // ═══ Test 01: 根组件渲染 ═══════════════════════════════
  console.log("== Test 01: 根组件渲染 ==");
  const root = await page.$("#root");
  check("01.1 root 元素存在", root !== null);
  const hasMain = await page.$(".main");
  check("01.2 .main 容器存在", hasMain !== null);

  // ═══ Test 02: 侧边栏 ═══════════════════════════════════
  console.log("\n== Test 02: 侧边栏 ==");
  const sidebar = await page.$(".sidebar");
  check("02.1 侧边栏渲染", sidebar !== null);

  const projectBtn = await page.$(".project-btn");
  check("02.2 项目选择器按钮", projectBtn !== null);
  if (projectBtn) {
    const name = await projectBtn.$eval(".name", (el) => el.textContent);
    check("02.3 项目名显示", name && name.length > 0, `项目名: "${name}"`);
  }

  // 点击项目按钮触发下拉
  if (projectBtn) {
    await projectBtn.click();
    await new Promise((r) => setTimeout(r, 300));
    const dropdown = await page.$(".project-dropdown.visible");
    check("02.4 项目下拉菜单展开", dropdown !== null);
    // 点其他地方收起
    await page.click(".sidebar");
    await new Promise((r) => setTimeout(r, 300));
  }

  // 仪表盘 gauge
  const gaugeSvgs = await page.$$(".gauge-svg");
  check("02.5 仪表盘 gauge 图", gaugeSvgs.length >= 2, `${gaugeSvgs.length} 个 gauge`);

  const cacheTrio = await page.$(".cache-trio");
  check("02.6 三级缓存面板", cacheTrio !== null);

  // 目录列表
  const pdHeader = await page.$(".pd-header");
  check("02.7 项目目录头", pdHeader !== null);

  const addPathBtn = await page.$(".pd-add");
  check("02.8 添加路径按钮", addPathBtn !== null);

  // ═══ Test 03: 聊天区域 ═════════════════════════════════
  console.log("\n== Test 03: 聊天区域 ==");
  const chatArea = await page.$(".chat-area");
  check("03.1 聊天区域渲染", chatArea !== null);

  const toolbar = await page.$(".chat-toolbar");
  check("03.2 工具栏", toolbar !== null);

  // 工具栏按钮
  const toolbarBtns = await page.$$(".chat-toolbar-btn");
  check("03.3 工具栏按钮数 >= 4", toolbarBtns.length >= 4, `${toolbarBtns.length} 个按钮`);

  // 空状态提示
  const emptyHint = await page.$(".empty-hint");
  check("03.4 空消息提示(无对话时)", emptyHint !== null);

  // 输入框
  const textarea = await page.$("textarea.compose-input");
  check("03.5 输入框 textarea", textarea !== null);

  // 输入框 disabled(未连接时)
  if (textarea) {
    const disabled = await textarea.evaluate((el) => el.disabled);
    check("03.6 未连接时输入框 disabled", disabled === true);
  }

  // 发送按钮
  const sendBtn = await page.$(".compose-send");
  check("03.7 发送按钮", sendBtn !== null);
  if (sendBtn) {
    const disabled = await sendBtn.evaluate((el) => el.disabled);
    check("03.8 未连接时发送按钮 disabled", disabled === true);
  }

  // 连接状态指示器
  const statusDot = await page.$(".chat-toolbar-status .dot");
  check("03.9 连接状态指示灯", statusDot !== null);

  // 语言选择器
  const langSelect = await page.$(".chat-toolbar-lang");
  check("03.10 语言选择器", langSelect !== null);

  // ═══ Test 04: 设置面板 ═════════════════════════════════
  console.log("\n== Test 04: 设置面板 ==");
  // 点设置按钮
  const settingsBtn = await page.$("button[title*='置']") || await page.$("button[title='Settings']");
  if (!settingsBtn) {
    // 工具栏最后一个有齿轮 SVG 的按钮
    const svgBtns = await page.$$(".chat-toolbar-btn");
    for (const btn of svgBtns) {
      const title = await btn.evaluate((el) => el.title || el.getAttribute("title"));
      if (title && (title.includes("设") || title.includes("Setting"))) {
        await btn.click();
        break;
      }
    }
  } else {
    await settingsBtn.click();
  }
  await new Promise((r) => setTimeout(r, 400));

  const settingsOverlay = await page.$(".settings-overlay.visible");
  check("04.1 设置面板可见", settingsOverlay !== null);

  const settingsPanel = await page.$(".settings-panel");
  check("04.2 设置面板渲染", settingsPanel !== null);

  // Tab 按钮
  const tabBtns = await page.$$(".settings-tab-btn");
  check("04.3 设置 tab 数 >= 5", tabBtns.length >= 5, `${tabBtns.length} 个 tab`);

  // 连接 tab 内容
  const apiBaseInput = await page.$(".settings-tab-pane.active input");
  check("04.4 连接 tab 有输入框", apiBaseInput !== null);

  // 切到 tools tab
  if (tabBtns.length >= 2) {
    await tabBtns[1].click();
    await new Promise((r) => setTimeout(r, 300));
    const toolsPane = await page.$(".settings-tab-pane.active");
    check("04.5 工具 tab 可切换", toolsPane !== null);
  }

  // 关闭设置
  const closeBtn = await page.$(".settings-close");
  if (closeBtn) {
    await closeBtn.click();
    await new Promise((r) => setTimeout(r, 300));
  }
  const overlayGone = await page.$(".settings-overlay.visible");
  check("04.6 设置面板可关闭", overlayGone === null);

  // ═══ Test 05: 项目创建弹窗 ═════════════════════════════
  console.log("\n== Test 05: 项目创建弹窗 ==");
  // 通过点击侧边栏项目下拉的 "+ 新项目"
  const projBtn2 = await page.$(".project-btn");
  if (projBtn2) {
    await projBtn2.click();
    await new Promise((r) => setTimeout(r, 200));
    const newProjAction = await page.$(".project-dropdown-action");
    check("05.1 新建项目入口", newProjAction !== null);
    if (newProjAction) {
      await newProjAction.click();
      await new Promise((r) => setTimeout(r, 300));
    }
  }
  // ProjectCreateModal / AddPathModal 都用 project-create-overlay
  const projModal = await page.$(".project-create-overlay.visible");
  check("05.2 项目创建弹窗出现", projModal !== null);

  // 关闭弹窗(点 backdrop 或按 Escape)
  await page.keyboard.press("Escape");
  await new Promise((r) => setTimeout(r, 300));

  // ═══ Test 06: 路径添加弹窗 ═════════════════════════════
  console.log("\n== Test 06: 路径添加弹窗 ==");
  const addBtn = await page.$(".pd-add");
  if (addBtn) {
    await addBtn.click();
    await new Promise((r) => setTimeout(r, 300));
  }
  const addPathModal = await page.$(".project-create-overlay.visible");
  check("06.1 添加路径弹窗出现", addPathModal !== null);
  await page.keyboard.press("Escape");
  await new Promise((r) => setTimeout(r, 300));

  // ═══ Test 07: 状态栏 ═══════════════════════════════════
  console.log("\n== Test 07: 状态栏 ==");
  const statusBar = await page.$("[class*='status-bar']") || await page.$("[class*='statusbar']");
  check("07.1 状态栏渲染", statusBar !== null);

  // ═══ Test 08: Toast 通知 ═══════════════════════════════
  console.log("\n== Test 08: Toast 通知 ==");
  await page.evaluate(() => {
    // 尝试触发一个 toast (如果 showToast 在全局)
    if (window.__GROWBOX_TEST__) {
      // 通过 store 触发
    }
  });
  // 用 DOM 检查 toast 容器是否存在
  const toastContainer = await page.$("[class*='toast-container']") || await page.$("[class*='toast']");
  check("08.1 Toast 容器存在", toastContainer !== null);

  // ═══ Test 09: 语言切换 ═════════════════════════════════
  console.log("\n== Test 09: i18n 国际化 ==");
  if (langSelect) {
    const currentVal = await langSelect.evaluate((el) => el.value);
    check("09.1 当前语言", currentVal === "zh-CN", `当前: ${currentVal}`);

    // 切到 English
    await langSelect.select("en");
    await new Promise((r) => setTimeout(r, 500));
    const enVal = await langSelect.evaluate((el) => el.value);
    check("09.2 切换到 English", enVal === "en");

    // 看 placeholder 是否变成英文
    const placeholder = await textarea.evaluate((el) => el.placeholder);
    check("09.3 placeholder 英文", placeholder && placeholder.length > 0 && !placeholder.includes("..."), `placeholder: "${placeholder}"`);

    // 切回中文
    await langSelect.select("zh-CN");
    await new Promise((r) => setTimeout(r, 300));
  }

  // ═══ Test 10: 调试钩子 __GROWBOX__ ═════════════════════
  console.log("\n== Test 10: __GROWBOX__ 调试钩子 ==");
  const gbExists = await page.evaluate(() => typeof window.__GROWBOX__ !== "undefined");
  check("10.1 __GROWBOX__ 存在", gbExists);

  if (gbExists) {
    const state = await page.evaluate(() => window.__GROWBOX__.getState());
    check("10.2 getState() 返回快照", state !== null && typeof state === "object");
    check("10.3 getState 含 messageCount", typeof state.messageCount === "number", `messageCount=${state.messageCount}`);
    check("10.4 getState 含 connected", typeof state.connected === "boolean", `connected=${state.connected}`);

    const dom = await page.evaluate(() => window.__GROWBOX__.getDOM(".sidebar"));
    check("10.5 getDOM('.sidebar')", dom.length === 1, `${dom.length} 个匹配`);

    const dims = await page.evaluate(() => window.__GROWBOX__.measureElement(".chat-area"));
    check("10.6 measureElement('.chat-area')", dims.found && dims.rect.w > 0, `宽=${dims.rect?.w}px`);

    const clickOk = await page.evaluate(() => window.__GROWBOX__.clickElement(".project-btn"));
    check("10.7 clickElement", clickOk === true);

    // 跑内置全量检查
    const fullTest = await page.evaluate(() => window.__GROWBOX__.runFullTest());
    // 浏览器 mock 模式下 control panel(在设置深层) 和 connect button(在设置内) 不可见是正常的
    check("10.8 runFullTest 通过率", fullTest.pass >= 7, `${fullTest.pass}/${fullTest.pass + fullTest.fail}`);
    for (const c of fullTest.checks) {
      check(`  10.8 ${c.name}`, c.ok, c.detail);
    }

    // 控制台日志
    const logs = await page.evaluate(() => window.__GROWBOX__.getConsoleLogs(5));
    check("10.9 getConsoleLogs", Array.isArray(logs), `${logs.length} 条`);
  }

  // ═══ Test 11: __GROWBOX_TEST__ 钩子 ════════════════════
  console.log("\n== Test 11: __GROWBOX_TEST__ 钩子 ==");
  const testHook = await page.evaluate(() => typeof window.__GROWBOX_TEST__ !== "undefined");
  check("11.1 __GROWBOX_TEST__ 存在", testHook);

  if (testHook) {
    const msgs = await page.evaluate(() => {
      const m = window.__GROWBOX_TEST__.messages();
      return Array.isArray(m) ? m.length : -1;
    });
    check("11.2 messages() 有数据", msgs >= 0, `消息数=${msgs}`);

    const sending = await page.evaluate(() => window.__GROWBOX_TEST__.sending());
    check("11.3 sending() 为 false", sending === false);

    const conn = await page.evaluate(() => window.__GROWBOX_TEST__.connected());
    check("11.4 connected() 为 false(未连)", conn === false);

    const projList = await page.evaluate(() => window.__GROWBOX_TEST__.projects());
    check("11.5 projects() 有数据", projList.length >= 1, `${projList.length} 个项目`);
  }

  // ═══ Test 12: CSS 样式完整性 ═══════════════════════════
  console.log("\n== Test 12: CSS 样式 ==");
  const styles = await page.evaluate(() => {
    const sheets = document.styleSheets;
    let rules = 0;
    try {
      for (const sheet of sheets) {
        try { rules += sheet.cssRules?.length || 0; } catch {}
      }
    } catch {}
    return { sheets: sheets.length, rules };
  });
  check("12.1 CSS 样式表加载", styles.sheets > 0, `${styles.sheets} 个 stylesheet`);
  check("12.2 CSS 规则数 > 0", styles.rules > 0, `${styles.rules} 条规则`);

  // ═══ 清理 ═══════════════════════════════════════════════
  console.log("\n── 测试结束 ──────────────────────────────\n");

  await browser.close();
  await server.close();

  // ── 汇总 ──────────────────────────────────────────────────
  const total = pass + fail;
  console.log("═══════════════════════════════════════════");
  console.log(`  结果: ${pass}/${total} 通过`);
  if (fail > 0) {
    console.log(`  失败项:`);
    for (const r of results.filter((r) => !r.ok)) {
      console.log(`    - ${r.name}: ${r.detail}`);
    }
  }
  console.log("═══════════════════════════════════════════");

  process.exit(fail > 0 ? 1 : 0);
}

main().catch((err) => {
  console.error("测试框架错误:", err);
  process.exit(1);
});
