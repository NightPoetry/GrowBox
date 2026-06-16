// 网页调试运行时 —— 经 Tauri initialization_script 注入"被调试的外部页面"(本地 dev server)。
// 计划/网页调试窗-可视化框选改源.md Phase 2。
//
// 为什么是 fetch 而非 Tauri IPC:Tauri v2 对 WebviewUrl::External 的 IPC 不可靠(CEF 不带 Origin /
// ipc.localhost 的 CSP 失败,见官方 #15190/#8476)。故回传走本机 HTTP(text/plain 简单请求免预检 +
// mode:no-cors 不读响应)。__GROWBOX_WEBLASSO_PORT__ 在 Rust 侧 .replace() 成真实端口。
//
// UI(工具栏+输入面板)放进 Shadow DOM,避免与被调试页面的 CSS 互相污染;套索 overlay/canvas/高亮
// 直接操作页面真实 DOM(要量真元素)。lasso 几何与 Phase 1(造物 iframe 版)一致。
(function () {
  if (window.__gxdbg) return; // 防 initialization_script 在同页重复注入
  window.__gxdbg = true;
  // 回传改走"导航到 gxlasso:// 哨兵"(被 on_navigation 拦截),不再用本机 HTTP 端口(被页面 CSP 拦)。

  // ★QA 自反馈调试:缓冲本页 console.error / 未捕获错误★,供 observe() 读(每页独立,导航即重置;
  //  initialization_script 先于页面脚本跑 → 能抓到加载期错误,正是"按钮跳转坏了"这类的证据)。
  window.__gxerrs = window.__gxerrs || [];
  (function () {
    var oe = console.error;
    console.error = function () { try { window.__gxerrs.push(Array.prototype.map.call(arguments, String).join(" ").slice(0, 200)); } catch (e) {} return oe.apply(console, arguments); };
    window.addEventListener("error", function (ev) { try { window.__gxerrs.push("[uncaught] " + String((ev && ev.message) || (ev && ev.error && ev.error.message) || "").slice(0, 200)); } catch (e) {} });
    window.addEventListener("unhandledrejection", function (ev) { try { window.__gxerrs.push("[reject] " + String((ev && ev.reason) || "").slice(0, 200)); } catch (e) {} });
  })();

  // 诊断 HUD + 选中高亮(临时,便于真机定位"套索无效/不准确/没反应")。
  var hud = null;
  function hudShow(t) { if (!hud) { hud = document.createElement("div"); hud.style.cssText = "position:fixed;left:8px;top:8px;z-index:2147483647;background:rgba(0,0,0,0.82);color:#3f6;font:11px monospace;padding:4px 8px;border-radius:6px;pointer-events:none;white-space:pre"; (document.body || document.documentElement).appendChild(hud); } hud.textContent = "[网页调试] " + t; }
  function hudHide() { if (hud) { hud.remove(); hud = null; } }
  function markSelected(els) { for (var i = 0; i < els.length; i++) { (function (el) { var prev = el.style.outline; el.style.outline = "2px solid #22c55e"; setTimeout(function () { try { el.style.outline = prev; } catch (e) {} }, 1400); })(els[i]); } }

  // ── lasso 几何(与造物版一致)──
  var ins = { on: false, ov: null, cv: null, ctx: null, draw: false, pts: [], hi: null, hiPrev: "" };
  function rectOf(el) { var r = el.getBoundingClientRect(); return { x: Math.round(r.left), y: Math.round(r.top), w: Math.round(r.width), h: Math.round(r.height) }; }
  function selectorOf(el) {
    if (!el || el.nodeType !== 1) return "";
    var parts = [], cur = el, depth = 0;
    while (cur && cur.nodeType === 1 && cur !== document.body && depth < 6) {
      var seg = cur.tagName.toLowerCase();
      if (cur.id) { parts.unshift("#" + cur.id); break; }
      if (typeof cur.className === "string") { var c = cur.className.trim().split(" ").filter(function (s) { return !!s; }).slice(0, 2).join("."); if (c) seg += "." + c; }
      var p = cur.parentNode;
      if (p && p.children) { var same = []; for (var i = 0; i < p.children.length; i++) { if (p.children[i].tagName === cur.tagName) same.push(p.children[i]); } if (same.length > 1) seg += ":nth-of-type(" + (same.indexOf(cur) + 1) + ")"; }
      parts.unshift(seg); cur = p; depth++;
    }
    return parts.join(" > ");
  }
  function snapOf(el) {
    var oh = (el.outerHTML || ""); if (oh.length > 240) oh = oh.slice(0, 240) + "...";
    var tx = (el.textContent || "").trim(); if (tx.length > 80) tx = tx.slice(0, 80) + "...";
    return { selector: selectorOf(el), tag: el.tagName.toLowerCase(), id: el.id || "", classes: (typeof el.className === "string" ? el.className : ""), text: tx, outerHTML: oh, rect: rectOf(el) };
  }
  function clrHi() { if (ins.hi) { try { ins.hi.style.outline = ins.hiPrev; } catch (e) {} ins.hi = null; } }
  function hi(el) { if (el === ins.hi) return; clrHi(); if (el && el.nodeType === 1) { ins.hi = el; ins.hiPrev = el.style.outline; el.style.outline = "2px solid #0a84ff"; } }
  function at(x, y) { ins.ov.style.pointerEvents = "none"; var el = document.elementFromPoint(x, y); ins.ov.style.pointerEvents = "auto"; return (el && el !== ins.ov) ? el : null; }
  function inPoly(x, y, pts) {
    var inside = false, n = pts.length, j = n - 1;
    for (var i = 0; i < n; i++) { var xi = pts[i].x, yi = pts[i].y, xj = pts[j].x, yj = pts[j].y; if (((yi > y) !== (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi)) inside = !inside; j = i; }
    return inside;
  }
  function inLasso(pts) {
    var minx = 1e9, miny = 1e9, maxx = -1e9, maxy = -1e9;
    for (var k = 0; k < pts.length; k++) { var p = pts[k]; if (p.x < minx) minx = p.x; if (p.x > maxx) maxx = p.x; if (p.y < miny) miny = p.y; if (p.y > maxy) maxy = p.y; }
    var hits = [], all = document.body ? document.body.querySelectorAll("*") : [];
    for (var i = 0; i < all.length; i++) { var b = all[i].getBoundingClientRect(); if (b.width === 0 || b.height === 0) continue; var cx = b.left + b.width / 2, cy = b.top + b.height / 2; if (cx < minx || cx > maxx || cy < miny || cy > maxy) continue; if (inPoly(cx, cy, pts)) hits.push(all[i]); }
    var leaf = []; for (var a = 0; a < hits.length; a++) { var anc = false; for (var c2 = 0; c2 < hits.length; c2++) { if (a !== c2 && hits[a].contains(hits[c2])) { anc = true; break; } } if (!anc) leaf.push(hits[a]); }
    return leaf.slice(0, 12);
  }
  function redraw() {
    var c = ins.ctx; if (!c) return; c.clearRect(0, 0, ins.cv.width, ins.cv.height);
    var pts = ins.pts; if (pts.length < 2) return;
    c.beginPath(); c.moveTo(pts[0].x, pts[0].y); for (var i = 1; i < pts.length; i++) c.lineTo(pts[i].x, pts[i].y); c.closePath();
    c.fillStyle = "rgba(10,132,255,0.12)"; c.fill(); c.strokeStyle = "#0a84ff"; c.lineWidth = 1.5; c.setLineDash([5, 4]); c.stroke();
  }
  function startInspect() {
    if (ins.on) return; ins.on = true;
    if (bar) bar.style.display = "none"; // ★套索期间藏工具栏★:免后退/前进/刷新等浮钮遮挡贴边元素(Esc 退出)。
    var ov = document.createElement("div"); ov.style.cssText = "position:fixed;inset:0;z-index:2147483600;cursor:crosshair;background:transparent;user-select:none;-webkit-user-select:none;"; document.body.appendChild(ov); ins.ov = ov;
    var cv = document.createElement("canvas"); cv.width = window.innerWidth; cv.height = window.innerHeight; cv.style.cssText = "position:fixed;left:0;top:0;z-index:2147483601;pointer-events:none;"; document.body.appendChild(cv); ins.cv = cv; ins.ctx = cv.getContext("2d");
    // 套索期间彻底掐死原生选区(那个"框选框"),否则它抢走拖拽、套索点收不到。
    ins.noSel = function (ev) { ev.preventDefault(); }; document.addEventListener("selectstart", ins.noSel, true); document.addEventListener("dragstart", ins.noSel, true);
    hudShow("套索开 · 拖拽画圈 或 单击元素 · Esc 退出");
    // ★贴边元素★:鼠标滑出视口时坐标钳回边界,套索点贴边不丢线(否则圈不上靠边的东西)。
    function clampX(v) { return v < 0 ? 0 : (v > window.innerWidth ? window.innerWidth : v); }
    function clampY(v) { return v < 0 ? 0 : (v > window.innerHeight ? window.innerHeight : v); }
    // 收尾(点选/成圈)抽成函数:mouseup 与"窗外松开后回窗内(buttons===0)"两条路共用。
    ins.finish = function (e) {
      var was = ins.draw, pts = ins.pts; ins.draw = false; ins.pts = []; if (ins.ctx) ins.ctx.clearRect(0, 0, cv.width, cv.height);
      var fx = clampX(e.clientX), fy = clampY(e.clientY);
      var els, rect;
      if (!was || pts.length < 4) { var one = at(fx, fy); els = one ? [one] : []; rect = one ? rectOf(one) : { x: fx, y: fy, w: 0, h: 0 }; }
      else { els = inLasso(pts); if (els.length === 0) { var fcx = 0, fcy = 0; for (var fk = 0; fk < pts.length; fk++) { fcx += pts[fk].x; fcy += pts[fk].y; } var fb = at(fcx / pts.length, fcy / pts.length); if (fb) els = [fb]; } var mnx = 1e9, mny = 1e9, mxx = -1e9, mxy = -1e9; for (var k = 0; k < pts.length; k++) { var p = pts[k]; if (p.x < mnx) mnx = p.x; if (p.x > mxx) mxx = p.x; if (p.y < mny) mny = p.y; if (p.y > mxy) mxy = p.y; } rect = { x: Math.round(mnx), y: Math.round(mny), w: Math.round(mxx - mnx), h: Math.round(mxy - mny) }; }
      try { if (window.getSelection) window.getSelection().removeAllRanges(); } catch (e2) {}
      var snap = []; for (var i = 0; i < els.length; i++) snap.push(snapOf(els[i]));
      hudShow("选中 " + snap.length + " 个" + (snap[0] ? " · " + snap[0].selector : " ·(空,圈大点或单击)"));
      markSelected(els);
      stopInspect();
      if (snap.length) showPanel({ rect: rect, elements: snap });
    };
    // 静默:不做 hover 高亮,只画套索线(用户要求"只看到套索区域")。
    // ★move/up 挂 window(捕获)而非 overlay★:鼠标拖到窗口边缘外仍能收到事件继续追踪;
    //   贴边元素若在窗外松开会漏 mouseup → 回到窗内的 move 带 buttons===0,即补一次收尾。
    ins.onMove = function (e) {
      if (!ins.draw) return;
      if (e.buttons === 0) { ins.finish(e); return; }
      ins.pts.push({ x: clampX(e.clientX), y: clampY(e.clientY) }); redraw(); hudShow("画圈中 · 点 " + ins.pts.length);
    };
    ins.onUp = function (e) { if (!ins.draw) return; ins.finish(e); };
    ins.onKey = function (e) { if (e.key === "Escape" || e.key === "Esc") { e.preventDefault(); stopInspect(); } };
    ov.addEventListener("mousedown", function (e) { ins.draw = true; ins.pts = [{ x: clampX(e.clientX), y: clampY(e.clientY) }]; clrHi(); e.preventDefault(); });
    window.addEventListener("mousemove", ins.onMove, true);
    window.addEventListener("mouseup", ins.onUp, true);
    window.addEventListener("keydown", ins.onKey, true);
  }
  function stopInspect() {
    if (!ins.on) return; ins.on = false; clrHi();
    if (ins.noSel) { document.removeEventListener("selectstart", ins.noSel, true); document.removeEventListener("dragstart", ins.noSel, true); ins.noSel = null; }
    if (ins.onMove) { window.removeEventListener("mousemove", ins.onMove, true); ins.onMove = null; }
    if (ins.onUp) { window.removeEventListener("mouseup", ins.onUp, true); ins.onUp = null; }
    if (ins.onKey) { window.removeEventListener("keydown", ins.onKey, true); ins.onKey = null; }
    ins.finish = null;
    if (ins.ov) ins.ov.remove(); if (ins.cv) ins.cv.remove();
    ins.ov = null; ins.cv = null; ins.ctx = null; ins.draw = false; ins.pts = [];
    if (bar) bar.style.display = ""; // 还原工具栏
    if (lassoBtn) lassoBtn.classList.remove("on");
  }

  // ── 回传:导航到哨兵 scheme(gxlasso://),被 Tauri on_navigation 拦截取消 + emit 给主窗。
  //    CSP 的 connect-src 管不着导航,故任何带 CSP 的被调试页都能回传(fetch 会被 default-src 'self' 拦)。──
  function postEdit(selection, suggestion) {
    try {
      var payload = JSON.stringify({ url: location.href, selection: selection, suggestion: suggestion });
      hudShow("已发送 ✓ 等待 AI 改…");
      window.location.href = "gxlasso://x/?d=" + encodeURIComponent(payload);
    } catch (e) { hudShow("发送失败 ✗ " + e); }
  }

  // ── Shadow DOM 隔离的 UI(工具栏 + 修改建议输入)──
  var shadow = null, bar = null, lassoBtn = null, panel = null, ta = null, curSel = null;
  function ensureUI() {
    if (shadow) return;
    var host = document.createElement("div"); host.id = "__gxdbg_host";
    host.style.cssText = "position:fixed;left:0;top:0;width:0;height:0;z-index:2147483646;";
    (document.body || document.documentElement).appendChild(host);
    shadow = host.attachShadow({ mode: "open" });
    var style = document.createElement("style");
    style.textContent = ".bar{position:fixed;top:12px;right:12px;display:flex;gap:6px;align-items:center;font:13px -apple-system,system-ui,sans-serif}" +
      ".btn{background:#23262d;color:#e6e6e6;border:1px solid #0a84ff;border-radius:8px;padding:5px 10px;cursor:pointer;box-shadow:0 4px 16px rgba(0,0,0,0.4)}" +
      ".btn.on{background:#0a84ff;color:#fff}" +
      ".panel{position:fixed;left:12px;right:12px;bottom:12px;max-width:560px;margin:0 auto;background:#23262d;color:#e6e6e6;border:1px solid #0a84ff;border-radius:10px;padding:10px;display:flex;flex-direction:column;gap:8px;box-shadow:0 8px 28px rgba(0,0,0,0.55);font:13px -apple-system,system-ui,sans-serif}" +
      ".sel{font-size:12px;color:#9aa0aa;line-height:1.5}" +
      ".panel textarea{width:100%;box-sizing:border-box;resize:vertical;background:#1b1d22;color:#e6e6e6;border:1px solid #2e323a;border-radius:6px;padding:6px 8px;font-size:12.5px}" +
      ".row{display:flex;justify-content:flex-end;gap:8px}" +
      ".row button{border-radius:6px;padding:4px 12px;font-size:12.5px;cursor:pointer;border:none}" +
      ".cancel{background:transparent;border:1px solid #3a3f48;color:#9aa0aa}" +
      ".send{background:#0a84ff;color:#fff}";
    shadow.appendChild(style);
    bar = document.createElement("div"); bar.className = "bar";
    // ── 浏览器式导航(后退/前进)── 被调试站点点深了或撞 404 时靠这个回退,不依赖页面自带按钮。
    // 客户端 history 导航即可:本地 dev server 同源;套索运行时每次导航都重注入,工具栏随之常驻(404 页也在)。
    var backBtn = document.createElement("button"); backBtn.className = "btn nav"; backBtn.textContent = "◀"; backBtn.title = "后退(浏览历史上一页)";
    backBtn.addEventListener("click", function () { try { history.back(); } catch (e) {} });
    bar.appendChild(backBtn);
    var fwdBtn = document.createElement("button"); fwdBtn.className = "btn nav"; fwdBtn.textContent = "▶"; fwdBtn.title = "前进(浏览历史下一页)";
    fwdBtn.addEventListener("click", function () { try { history.forward(); } catch (e) {} });
    bar.appendChild(fwdBtn);
    lassoBtn = document.createElement("button"); lassoBtn.className = "btn"; lassoBtn.textContent = "⌖ 套索"; // ⌖ 套索
    lassoBtn.addEventListener("click", function () {
      if (ins.on) { stopInspect(); lassoBtn.classList.remove("on"); hudHide(); }
      else { hidePanel(); startInspect(); lassoBtn.classList.add("on"); }
    });
    bar.appendChild(lassoBtn);
    // 手动刷新:无 HMR 的工程(EJS/Express)改完源码点它看最新效果(改完已自动刷一次,这是兜底/随时复看)。
    // ★cache-busting★:纯 location.reload() 在 WKWebView 可能命中磁盘/内存缓存看不到改动 → 改成
    // 在 URL 上换一个 _gxr 时间戳参数强制服务端重取(EJS/Express 忽略未知 query;SPA 也无害)。
    var refreshBtn = document.createElement("button"); refreshBtn.className = "btn"; refreshBtn.textContent = "⟳ 刷新"; refreshBtn.title = "刷新页面(强制不走缓存),看 AI 改源后的最新效果";
    refreshBtn.addEventListener("click", function () {
      try { var u = new URL(location.href); u.searchParams.set("_gxr", String(Date.now())); location.href = u.toString(); }
      catch (e) { location.reload(); }
    });
    bar.appendChild(refreshBtn);
    shadow.appendChild(bar);
  }
  function showPanel(sel) {
    ensureUI(); curSel = sel; lassoBtn.classList.remove("on"); hidePanel();
    panel = document.createElement("div"); panel.className = "panel";
    var info = document.createElement("div"); info.className = "sel";
    var names = sel.elements.map(function (el) { return el.selector; }).slice(0, 3).join("  /  ");
    info.textContent = "已框选 " + sel.elements.length + " 个元素：" + names; // 已框选 N 个元素:
    panel.appendChild(info);
    ta = document.createElement("textarea"); ta.rows = 2; ta.placeholder = "描述要怎么改（例：把这个按钮改成绿色、字号调大）…"; panel.appendChild(ta);
    var row = document.createElement("div"); row.className = "row";
    var cancel = document.createElement("button"); cancel.className = "cancel"; cancel.textContent = "取消"; cancel.addEventListener("click", hidePanel);
    var send = document.createElement("button"); send.className = "send"; send.textContent = "发送给 AI"; // 发送给 AI
    send.addEventListener("click", function () {
      var sug = (ta.value || "").trim(); if (!sug || !curSel) return;
      postEdit(curSel, sug); hidePanel();
    });
    row.appendChild(cancel); row.appendChild(send); panel.appendChild(row);
    shadow.appendChild(panel); ta.focus();
  }
  function hidePanel() { if (panel) { panel.remove(); panel = null; ta = null; } }

  // ── ★QA 自反馈调试:真模拟操作 + 观察★(从"枚举回调"升级到"真点 / 真填 / 真提交 + 读结果")──
  //    主窗 web_debug.rs 的 web_debug_drive 命令 eval 这些:动作(click/fill/submit)fire-and-forget;
  //    backend 等一拍后单独 eval observe()(在导航后的新页里跑)读 url/title/errors,经 gxlasso://act 哨兵回传(穿 CSP)。
  function gxReport(obj) { try { location.href = "gxlasso://act/?d=" + encodeURIComponent(JSON.stringify(obj)); } catch (e) {} }
  // ★最近一次 click/fill/submit 的结果★:observe 回传时一并带上,让 AI 能区分"没点到(选择器没匹配)"
  //  /"点了没跳转(JS 按钮/同页)"/"选择器语法错"——否则 AI 只看到 url 没变,只能瞎猜。
  //  导航换页后随新 window 重置成 null(此时 url 已变 = 跳转成功的硬证,lastAction 丢失无碍)。
  window.__gxlast = window.__gxlast || null;
  window.__gxqa = {
    click: function (sel) {
      try { var el = document.querySelector(sel); if (!el) { window.__gxlast = { op: "click", selector: sel, matched: false, note: "选择器没匹配到元素" }; return false; } el.click(); window.__gxlast = { op: "click", selector: sel, matched: true }; return true; }
      catch (e) { window.__gxlast = { op: "click", selector: sel, matched: false, error: String(e).slice(0, 160) }; return false; }
    },
    fill: function (sel, val) {
      try { var el = document.querySelector(sel); if (!el) { window.__gxlast = { op: "fill", selector: sel, matched: false, note: "选择器没匹配到输入框" }; return false; } el.focus(); el.value = val; el.dispatchEvent(new Event("input", { bubbles: true })); el.dispatchEvent(new Event("change", { bubbles: true })); window.__gxlast = { op: "fill", selector: sel, matched: true, value: String(val).slice(0, 80) }; return true; }
      catch (e) { window.__gxlast = { op: "fill", selector: sel, matched: false, error: String(e).slice(0, 160) }; return false; }
    },
    submit: function (sel) {
      try { var f = document.querySelector(sel); if (!f) { window.__gxlast = { op: "submit", selector: sel, matched: false, note: "选择器没匹配到表单" }; return false; } if (f.requestSubmit) f.requestSubmit(); else f.submit(); window.__gxlast = { op: "submit", selector: sel, matched: true }; return true; }
      catch (e) { window.__gxlast = { op: "submit", selector: sel, matched: false, error: String(e).slice(0, 160) }; return false; }
    },
    // 观察当前页状态(动作 + 等待后单独跑;回传 url/title/本页错误/最近动作 = 判"跳转对不对/有没有报错"的硬证)。
    observe: function () { gxReport({ kind: "observe", url: location.href, title: document.title || "", errors: (window.__gxerrs || []).slice(-6), lastAction: window.__gxlast }); },
    // 枚举可交互点(给"有计划"列测试计划):按钮/链接/表单/输入,带文本/位置,回传。
    scan: function () {
      var out = [], all = document.querySelectorAll("a[href],button,input,select,textarea,form,[role=button],[onclick]");
      for (var i = 0; i < all.length && out.length < 60; i++) { var e = all[i], r = e.getBoundingClientRect(); if (r.width === 0 && r.height === 0) continue; out.push({ tag: e.tagName.toLowerCase(), type: e.getAttribute("type") || "", text: (e.textContent || e.value || e.getAttribute("href") || "").trim().slice(0, 48), id: e.id || "", name: e.getAttribute("name") || "" }); }
      gxReport({ kind: "scan", url: location.href, elements: out });
    }
  };

  function boot() { ensureUI(); }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", boot);
  else boot();
})();
