// 极简 markdown 渲染（v1 frontend/index.html:894 移植 + 扩展）。
// 支持：代码块、行内 code、列表（无序/有序）、引用、标题 h1-h6、加粗/斜体、表格。
// 非标准 MD 兼容：未闭合 fence、heading 与正文同段、表格分隔符行任意空格。
// Tool call marker：[TOOL] `name` [OK|FAIL] 渲染为 SVG icon + name + 状态徽章；
// 旧 emoji 标记（🔧 ... ✓/✗）仍识别用于历史 audit 回放（per
// [[feedback_no_emoji_use_svg]]：UI 不出现 emoji）。
// 代码块：highlight.js core + 按需注册语言（html/css/js/ts/json/python/rust/bash）。
// 注意：innerHTML 渲染，依赖输入信任——assistant 消息来自后端 LM，已经过滤。

import { tTool } from "./i18n";
import { toolKind, toolExpandDefault } from "./store";
import hljs from "highlight.js/lib/core";
import javascript from "highlight.js/lib/languages/javascript";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml"; // html via xml
import css from "highlight.js/lib/languages/css";
import json from "highlight.js/lib/languages/json";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import bash from "highlight.js/lib/languages/bash";

hljs.registerLanguage("javascript", javascript);
hljs.registerLanguage("js", javascript);
hljs.registerLanguage("typescript", typescript);
hljs.registerLanguage("ts", typescript);
hljs.registerLanguage("html", xml);
hljs.registerLanguage("xml", xml);
hljs.registerLanguage("css", css);
hljs.registerLanguage("json", json);
hljs.registerLanguage("python", python);
hljs.registerLanguage("py", python);
hljs.registerLanguage("rust", rust);
hljs.registerLanguage("rs", rust);
hljs.registerLanguage("bash", bash);
hljs.registerLanguage("sh", bash);
hljs.registerLanguage("shell", bash);

function highlightCode(code: string, lang: string): string {
  const trimmedLang = (lang || "").toLowerCase().trim();
  if (trimmedLang && hljs.getLanguage(trimmedLang)) {
    try {
      return hljs.highlight(code, { language: trimmedLang, ignoreIllegals: true }).value;
    } catch {
      // fallback to plain
    }
  }
  // auto-detect for unknown lang (only useful subset registered above)
  if (!trimmedLang) {
    try {
      return hljs.highlightAuto(code).value;
    } catch {
      // fallback
    }
  }
  return esc(code);
}

function esc(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

// Tool marker SVG（stroke=currentColor 主题适配，14×14 inline）。
const TOOL_ICON =
  '<svg class="tool-icon" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/></svg>';
// ★区分图标(2026-06-08)★:工作流=分支流程,MCP 外部工具=包,内置工具=扳手(上面)。一眼区分。
const WORKFLOW_ICON =
  '<svg class="tool-icon tool-icon-workflow" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<path d="M6 3v12"/><circle cx="18" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><path d="M18 9a9 9 0 0 1-9 9"/></svg>';
const MCP_ICON =
  '<svg class="tool-icon tool-icon-mcp" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<path d="m7.5 4.27 9 5.15"/><path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z"/><path d="M3.3 7 12 12l8.7-5"/><path d="M12 22V12"/></svg>';
/// 据可调用名分类挑图标:工作流 / MCP / 内置工具(回退扳手)。
function iconForName(name: string): string {
  const k = toolKind(name);
  return k === "workflow" ? WORKFLOW_ICON : k === "mcp" ? MCP_ICON : TOOL_ICON;
}
const OK_ICON =
  '<svg class="tool-status-icon" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<polyline points="20 6 9 17 4 12"/></svg>';
const FAIL_ICON =
  '<svg class="tool-status-icon" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
  '<line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>';
const PENDING_ICON =
  '<svg class="tool-status-icon tool-spinning" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" aria-hidden="true">' +
  '<circle cx="12" cy="12" r="9" stroke-dasharray="14 8"/></svg>';
const STOPPED_ICON =
  '<svg class="tool-status-icon" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" aria-hidden="true" opacity="0.4">' +
  '<circle cx="12" cy="12" r="9" stroke-dasharray="14 8"/></svg>';

function toolHtml(name: string, status: "ok" | "fail" | "pending" | "stopped"): string {
  const icon = status === "ok" ? OK_ICON : status === "fail" ? FAIL_ICON : status === "stopped" ? STOPPED_ICON : PENDING_ICON;
  return (
    `<span class="tool-marker tool-marker-${status}">` +
    iconForName(name) +
    `<code class="tool-name">${esc(tTool(name))}</code>` +
    icon +
    `</span>`
  );
}

function extFromPath(p: string): string {
  const dot = p.lastIndexOf(".");
  return dot >= 0 ? p.slice(dot + 1).toLowerCase() : "";
}
function langForExt(ext: string): string | null {
  const map: Record<string, string> = {
    html: "html", htm: "html", xml: "xml", svg: "xml",
    css: "css", js: "javascript", ts: "typescript", tsx: "typescript",
    json: "json", py: "python", rs: "rust", sh: "bash", bash: "bash",
    toml: "bash", yaml: "bash", yml: "bash", md: "bash",
  };
  return map[ext] ?? null;
}
function parseToolArgs(raw: string): Record<string, string> | null {
  try {
    const v = JSON.parse(raw.trim());
    if (v && typeof v === "object" && !Array.isArray(v)) return v;
  } catch { /* not JSON */ }
  return null;
}

function toolBlockHtml(name: string, args: string, status: "ok" | "fail" | "pending" | "stopped", output: string): string {
  const icon = status === "ok" ? OK_ICON : status === "fail" ? FAIL_ICON : status === "stopped" ? STOPPED_ICON : PENDING_ICON;
  // 非 ok 状态(失败/进行中/中止)恒展开(需调试可见);ok 状态按用户的逐工具"默认展开"偏好(默认仅 ask_user)。
  const open = (status !== "ok" || toolExpandDefault(name)) ? " open" : "";
  const parsed = parseToolArgs(args);

  let summaryExtra = "";
  let bodyHtml = "";

  if (name === "file_write" && parsed?.path) {
    const bytes = parsed.content ? new TextEncoder().encode(parsed.content).length : 0;
    const sizeStr = bytes > 0 ? ` <span class="tool-size">(${bytes.toLocaleString()} bytes)</span>` : "";
    summaryExtra = ` <span class="tool-path">${esc(parsed.path)}</span>${sizeStr}`;
    // 仅非 OK 状态展示内容（失败/进行中需调试），正常写入不刷屏
    if (status !== "ok" && parsed.content) {
      const contentPreview = parsed.content.length > 2000
        ? parsed.content.slice(0, 2000) + "\n… (truncated)"
        : parsed.content;
      const lang = langForExt(extFromPath(parsed.path)) ?? "";
      bodyHtml = `<pre class="tool-code"><code>${highlightCode(contentPreview, lang)}</code></pre>`;
    }
  } else if (name === "shell" && parsed?.command) {
    summaryExtra = ` <span class="tool-cmd">${esc(parsed.command.length > 60 ? parsed.command.slice(0, 57) + "..." : parsed.command)}</span>`;
    if (parsed.command.length > 60) {
      bodyHtml = `<pre class="tool-args"><code>${esc(parsed.command)}</code></pre>`;
    }
  } else if ((name === "file_read" || name === "file_list") && parsed?.path) {
    summaryExtra = ` <span class="tool-path">${esc(parsed.path)}</span>`;
  } else if (args.trim()) {
    bodyHtml = `<pre class="tool-args"><code>${esc(args.trim())}</code></pre>`;
  }

  if (output.trim()) {
    bodyHtml += `<pre class="tool-output"><code>${esc(output.trim())}</code></pre>`;
  }

  return (
    `<details class="tool-block tool-block-${status}"${open}>` +
    `<summary class="tool-block-summary">` +
    `<span class="tool-block-prompt">$</span>` +
    iconForName(name) +
    `<code class="tool-name">${esc(tTool(name))}</code>` +
    summaryExtra +
    icon +
    `</summary>` +
    bodyHtml +
    `</details>`
  );
}

function fmt(s: string): string {
  s = esc(s);
  s = s.replace(/&lt;br\s*\/?&gt;/gi, "<br>");
  s = s.replace(/&lt;(\/?(?:b|strong|i|em|u|s|code|hr))&gt;/gi, "<$1>");
  s = linkify(s);
  s = s.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
  s = s.replace(/\*(.+?)\*/g, "<em>$1</em>");
  return s;
}

// 超链接渲染:markdown `[文字](http(s)://…)` 与裸 http(s) URL → 可点击 <a>。
// 用 data-href(非 href)+ chat-link 类:点击由 ChatArea 委托拦截、经后端在系统浏览器打开,
// 绝不让 webview 自身导航走(否则点链接会把整个应用导航掉)。输入已 esc 过(& 变 &amp;,
// 作为属性值浏览器读 dataset 时会自动解码回 &,URL 正确)。单次 alternation 扫描,避免重复处理。
function linkify(s: string): string {
  // 裸 URL 字符类**排除 CJK**(一-鿿 等)：LLM 常把 URL 紧贴中文写(如 "style.css退出码"），
  // 旧版贪婪 [^\s<"']+ 会把中文也吞进链接 → href 坏、点开打不开。排除 CJK 后遇中文即止。
  // 再在处理时剥掉尾部的句子标点（.,;:!?)]} 与中文标点），它们通常不属于 URL。
  return s.replace(
    /\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)|(https?:\/\/[^\s<"'一-鿿　-〿＀-￯]+)/g,
    (_m, txt: string | undefined, mdUrl: string | undefined, bareUrl: string | undefined) => {
      if (mdUrl) return `<a class="chat-link" data-href="${mdUrl}">${txt}</a>`;
      let url = bareUrl || "";
      let trail = "";
      const tm = url.match(/[.,;:!?)\]}]+$/); // 尾部英文标点剥出来放链接外（不剥 & 防坏 &amp;）
      if (tm) { trail = tm[0]; url = url.slice(0, -trail.length); }
      return `<a class="chat-link" data-href="${url}">${url}</a>${trail}`;
    }
  );
}

// 尝试把整段 lines 解析为 markdown 表格（GFM 风格）。
// 失败返回 null,由调用方退回普通 paragraph 处理。
// ★鲁棒性(2026-06-08)★:LLM 偶尔把表写得不完美——分隔行列数与表头不一致、某行少个竖线、
// 行尾多/少格。旧版任一处不符就 `return null`、整张表退回原始竖线文本(用户看到的"出错")。
// 现在:① "这是张表"的强信号只看第2行是不是分隔行(全是 :?-+:?);② 列数以表头为准,
// 多退少补、不再因列数不齐整张毙;③ 中途某行没竖线只跳过该行,不毙全表。
function tryParseTable(lines: string[]): string | null {
  if (lines.length < 2) return null;
  const splitRow = (raw: string): string[] | null => {
    let line = raw.trim();
    if (!line.includes("|")) return null;
    if (line.startsWith("|")) line = line.slice(1);
    if (line.endsWith("|")) line = line.slice(0, -1);
    return line.split("|").map((c) => c.trim());
  };
  const headerCells = splitRow(lines[0]);
  const sepCells = splitRow(lines[1]);
  if (!headerCells || headerCells.length === 0 || !sepCells || sepCells.length === 0) return null;
  // 第2行必须整行都是分隔格(:?-+:?)——这是把它认定为"表格"而非含竖线散文的强信号。
  const isSeparator = sepCells.every((c) => /^:?-+:?$/.test(c.replace(/\s+/g, "")));
  if (!isSeparator) return null;
  const ncols = headerCells.length; // 列数以表头为准
  const aligns: (string | null)[] = [];
  for (let i = 0; i < ncols; i++) {
    const stripped = (sepCells[i] ?? "").replace(/\s+/g, ""); // 分隔列数不齐 → 缺的按无对齐
    if (stripped.startsWith(":") && stripped.endsWith(":")) aligns.push("center");
    else if (stripped.endsWith(":")) aligns.push("right");
    else if (stripped.startsWith(":")) aligns.push("left");
    else aligns.push(null);
  }
  const bodyRows: string[][] = [];
  for (let i = 2; i < lines.length; i++) {
    if (!lines[i].trim()) continue;
    const cells = splitRow(lines[i]);
    if (!cells) continue; // 中途某行没竖线:跳过它,不再整张表退回纯文本
    while (cells.length < ncols) cells.push("");
    if (cells.length > ncols) cells.length = ncols; // 多退少补,对齐表头列数
    bodyRows.push(cells);
  }
  const styleFor = (a: string | null) => (a ? ` style="text-align:${a}"` : "");
  let h = "<table><thead><tr>";
  for (let i = 0; i < ncols; i++) {
    h += `<th${styleFor(aligns[i])}>${fmt(headerCells[i])}</th>`;
  }
  h += "</tr></thead>";
  if (bodyRows.length > 0) {
    h += "<tbody>";
    for (const row of bodyRows) {
      h += "<tr>";
      for (let i = 0; i < ncols; i++) {
        h += `<td${styleFor(aligns[i])}>${fmt(row[i] ?? "")}</td>`;
      }
      h += "</tr>";
    }
    h += "</tbody>";
  }
  h += "</table>";
  return h;
}

// 表格区强插空行,使其独立成段(容忍 LLM 把表格紧贴正文/列表、上下不留空行的写法,同 heading 处理)。
// ★根因(2026-06-15)★:renderMd 按空行 \n{2,} 切段、整段丢给 tryParseTable,而它要求段首行即表头。
// LLM 常写「可用账号:\n| 表头 |\n|---|\n| 行 |\n可体验功能:」上下不留空行 → 整坨一段、首行不是表头 →
// 解析返回 null → 退回纯文本(用户看到竖线原文)。这里在切段前先把表格区拆出来独立成段。
// 判定"这是表格" = 某行含 | 且下一行是分隔行(每格 :?-+:?);从表头行起、连续含 | 的行并入表格区。
function injectTableBoundaries(text: string): string {
  const isSep = (raw: string): boolean => {
    let line = raw.trim();
    if (!line.includes("|")) return false;
    if (line.startsWith("|")) line = line.slice(1);
    if (line.endsWith("|")) line = line.slice(0, -1);
    const cells = line.split("|").map((c) => c.trim());
    return cells.length > 0 && cells.every((c) => /^:?-+:?$/.test(c.replace(/\s+/g, "")));
  };
  const ls = text.split("\n");
  const out: string[] = [];
  for (let i = 0; i < ls.length; i++) {
    // 表头候选:本行含 |、下一行是分隔行、本行自身不是分隔行(避免分隔行被当表头)。
    if (ls[i].includes("|") && i + 1 < ls.length && isSep(ls[i + 1]) && !isSep(ls[i])) {
      if (out.length && out[out.length - 1].trim() !== "") out.push(""); // 表前补空行
      out.push(ls[i], ls[i + 1]); // 表头 + 分隔行
      i += 2;
      while (i < ls.length && ls[i].includes("|")) out.push(ls[i++]); // 连续含 | 的表体
      out.push(""); // 表后补空行
      i--; // for 的 ++ 会补回
      continue;
    }
    out.push(ls[i]);
  }
  return out.join("\n");
}

export function renderMd(src: string, streaming: boolean = false): string {
  if (!src) return "";
  const pendingAs: "pending" | "stopped" = streaming ? "pending" : "stopped";
  // 阶段 0：tool marker → 占位符 (\x03)。先于 fence/inline-code 抽取，
  // 否则 `name` 会被 inline-code 正则吞掉。
  const markers: string[] = [];
  const placeMarker = (html: string): string => {
    markers.push(html);
    return "\x03" + (markers.length - 1) + "\x03";
  };

  // 阶段 0a：完整 tool 区块（▸ calling + args + [TOOL] + output fence）→ 折叠 details
  // pattern: `▸ calling \`name\`:\n<args>\n[TOOL] \`name\` [OK|FAIL]\n```\n<output>\n```
  src = src.replace(
    /▸ calling `([^`\n]+)`:\n([\s\S]*?)\[TOOL\] `\1` \[(OK|FAIL)\]\n```\n([\s\S]*?)\n```/g,
    (_m, name, args, status, output) => placeMarker(
      toolBlockHtml(name, args, status === "OK" ? "ok" : "fail", output)
    )
  );
  // 0b：[TOOL] 已结束但 output fence 缺失（args 后立刻完成无 output）
  src = src.replace(
    /▸ calling `([^`\n]+)`:\n([\s\S]*?)\[TOOL\] `\1` \[(OK|FAIL)\]/g,
    (_m, name, args, status) => placeMarker(
      toolBlockHtml(name, args, status === "OK" ? "ok" : "fail", "")
    )
  );
  // 0c：pending — ▸ 已出但 [TOOL] 还没到（流式中或 stop 前），从 ▸ 抓到下一个 ▸ 或文末
  src = src.replace(
    /▸ calling `([^`\n]+)`:\n([\s\S]*?)(?=\n▸ calling `|$)/g,
    (_m, name, args) => placeMarker(toolBlockHtml(name, args, pendingAs, ""))
  );

  // 兼容：单独的 [TOOL] X [STATUS]（无 ▸ 上下文，老 audit 回放）→ inline marker
  src = src.replace(/\[TOOL\]\s+`([^`\n]+)`\s+\[OK\]/g, (_, n) => placeMarker(toolHtml(n, "ok")));
  src = src.replace(/\[TOOL\]\s+`([^`\n]+)`\s+\[FAIL\]/g, (_, n) => placeMarker(toolHtml(n, "fail")));
  src = src.replace(/\[TOOL\]\s+`([^`\n]+)`\s+/g, (_, n) => placeMarker(toolHtml(n, pendingAs)));
  // 旧 emoji marker（历史 audit 回放）
  src = src.replace(/🔧\s+`([^`\n]+)`\s+✓/g, (_, n) => placeMarker(toolHtml(n, "ok")));
  src = src.replace(/🔧\s+`([^`\n]+)`\s+✗/g, (_, n) => placeMarker(toolHtml(n, "fail")));
  src = src.replace(/🔧\s+`([^`\n]+)`\s+/g, (_, n) => placeMarker(toolHtml(n, pendingAs)));

  const fences = (src.match(/```/g) || []).length;
  if (fences % 2 !== 0) src += "\n```";
  const blocks: string[] = [];
  src = src.replace(/```(\w*)\n([\s\S]*?)```/g, (_, lang, code) => {
    const highlighted = highlightCode(code.trimEnd(), lang);
    const cls = lang ? ` class="language-${lang} hljs"` : ` class="hljs"`;
    blocks.push(`<pre><code${cls}>${highlighted}</code></pre>`);
    return "\x01" + (blocks.length - 1) + "\x01";
  });
  const codes: string[] = [];
  src = src.replace(/`([^`\n]+)`/g, (_, c) => {
    codes.push("<code>" + esc(c) + "</code>");
    return "\x02" + (codes.length - 1) + "\x02";
  });
  // heading 行强插空行,使其独立成段(容忍 LLM 不留空行就接正文/列表的写法)
  src = src.replace(/^(#{1,6} .+)$/gm, "\n\n$1\n\n");
  // 表格区强插空行,使其独立成段(容忍 LLM 表格紧贴正文/列表、上下不留空行 → 否则整段不被识别为表)。
  src = injectTableBoundaries(src);
  const paras = src.split(/\n{2,}/);
  let html = "";
  for (const p of paras) {
    const text = p.trim();
    if (!text) continue;
    const hm = text.match(/^(#{1,6}) (.+)/);
    if (hm) {
      const level = hm[1].length;
      html += "<h" + level + ">" + fmt(hm[2]) + "</h" + level + ">";
      continue;
    }
    const lines = text.split("\n");
    const tableHtml = tryParseTable(lines);
    if (tableHtml) {
      html += tableHtml;
      continue;
    }
    if (lines.length > 0 && lines.every((l) => /^\s*[-*] /.test(l) || !l.trim())) {
      html += "<ul>" + lines
        .filter((l) => l.trim())
        .map((l) => "<li>" + fmt(l.replace(/^\s*[-*] /, "")) + "</li>")
        .join("") + "</ul>";
      continue;
    }
    if (lines.length > 0 && lines.every((l) => /^\s*\d+\. /.test(l) || !l.trim())) {
      html += "<ol>" + lines
        .filter((l) => l.trim())
        .map((l) => "<li>" + fmt(l.replace(/^\s*\d+\. /, "")) + "</li>")
        .join("") + "</ol>";
      continue;
    }
    if (lines.length > 0 && lines.every((l) => /^> ?/.test(l) || !l.trim())) {
      html += "<blockquote>" + fmt(lines.map((l) => l.replace(/^> ?/, "")).join(" ")) + "</blockquote>";
      continue;
    }
    html += "<p>" + fmt(text).replace(/\n/g, "<br>") + "</p>";
  }
  // 用 \x01/\x02/\x03 占位回填,避免 v1 用纯数字占位被段落数字误伤
  html = html.replace(/\x01(\d+)\x01/g, (_, i) => blocks[parseInt(i)]);
  html = html.replace(/\x02(\d+)\x02/g, (_, i) => codes[parseInt(i)]);
  html = html.replace(/\x03(\d+)\x03/g, (_, i) => markers[parseInt(i)]);

  // 压缩连续重复的失败工具调用：同一工具连续 FAIL → 1个图标 + 红色计数徽章
  html = collapseConsecutiveToolFails(html);

  return html;
}

/** 将连续相同的 .tool-marker-fail 合并为一个带红色计数徽章的条目 */
function collapseConsecutiveToolFails(html: string): string {
  const failStart = '<span class="tool-marker tool-marker-fail">';
  const failEnd = '</span>';

  // 收集所有 fail marker 的起止位置和工具名
  interface Slot { start: number; end: number; name: string; }
  const slots: Slot[] = [];
  let pos = 0;
  while (true) {
    const s = html.indexOf(failStart, pos);
    if (s === -1) break;
    const e = html.indexOf(failEnd, s + failStart.length);
    if (e === -1) break;
    const inner = html.slice(s + failStart.length, e);
    const nameM = inner.match(/<code class="tool-name">([^<]+)<\/code>/);
    slots.push({ start: s, end: e + failEnd.length, name: nameM ? nameM[1] : "" });
    pos = e + failEnd.length;
  }

  if (slots.length < 2) return html;

  // 找连续相同 name 的组，从后往前替换（索引不变）
  let i = slots.length - 1;
  const replacements: { start: number; end: number; html: string }[] = [];
  while (i >= 0) {
    let j = i - 1;
    while (j >= 0 && slots[j].name === slots[i].name) j--;
    const groupSize = i - j;
    if (groupSize >= 2) {
      const first = slots[j + 1];
      const last = slots[i];
      // 在第一个 marker 的 </span> 前插入红色计数徽章
      const insertAt = first.end - failEnd.length;
      const badge = `<span class="tool-fail-count">${groupSize}</span>`;
      // 删除 first.end 到 last.end 之间的所有内容（即第 2~N 个重复 marker）
      const mergedHtml = html.slice(first.start, insertAt) + badge + failEnd;
      replacements.push({ start: first.start, end: last.end, html: mergedHtml });
    }
    i = j;
  }

  // 从后往前应用替换
  let result = html;
  replacements.sort((a, b) => b.start - a.start); // 降序
  for (const r of replacements) {
    result = result.slice(0, r.start) + r.html + result.slice(r.end);
  }

  return result;
}
