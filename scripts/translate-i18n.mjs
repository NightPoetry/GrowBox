// 用 deepseek 把 zh-CN 源 UI 文案批量翻译成 en/ja/zh-TW(Phase 4)。
//   - 源1: frontend/src/i18n.ts 的 FALLBACK_ZH_CN(全部 UI key)→ 写 locales/{en,ja,zh-TW}.ts
//   - 源2: prompts/tools.i18n.json 的工具 label/ui_desc(zh-CN)→ 填该文件的 ja/zh-TW(en 已亲写,保留)
//   - 源3: prompts/notices.i18n.json 的提示 human(zh-CN)→ 填该文件的 ja/zh-TW(en + 对内 llm 已亲写,保留)
// key 只走环境变量 DEEPSEEK_API_KEY,不写文件。给机器看的(llm_desc/llm 渲染/系统提示词)不在此脚本范围。
//
// 用法: DEEPSEEK_API_KEY=sk-xxx node scripts/translate-i18n.mjs   (cwd = 仓库根)
//   默认增量:只译目标 locale/json 里缺失的键(省 API/省时;改 zh-CN 源加新键后直接跑)。
//   改了已存在键的 zh-CN 文本要重译,加 --full 强制全量:node scripts/translate-i18n.mjs --full
//   (简单新增 UI 标签也可由维护者直接手译补进 locales/*.ts,无需开 key 端点。)

import fs from "node:fs";

const ROOT = process.cwd();
const I18N_TS = `${ROOT}/crates/growbox-gui/frontend/src/i18n.ts`;
const LOCALES_DIR = `${ROOT}/crates/growbox-gui/frontend/src/locales`;
const TOOLS_JSON = `${ROOT}/prompts/tools.i18n.json`;
const NOTICES_JSON = `${ROOT}/prompts/notices.i18n.json`;

const API_BASE = "https://api.deepseek.com";
const MODEL = "deepseek-v4-pro"; // 翻译用 pro,质量比 flash 好(用户要求 2026-06-02)
const KEY = process.env.DEEPSEEK_API_KEY;
if (!KEY) { console.error("缺 DEEPSEEK_API_KEY"); process.exit(1); }

const LANGS = {
  en: "English",
  ja: "Japanese (日本語)",
  "zh-TW": "Traditional Chinese (繁體中文, Taiwan)",
};

// GrowBox 专有名词固定译法(给翻译器约束,保证术语一致)。
const GLOSSARY = `Fixed terminology (use consistently):
- 记忆 = Memory (ja: メモリ/記憶, zh-TW: 記憶)
- 做梦/做梦整理 = Dreaming (ja: ドリーミング, zh-TW: 做夢整理)
- 疲劳/劳累/劳累度 = Fatigue (ja: 疲労度, zh-TW: 疲勞度)
- 精确层 = Precision layer
- 飞轮 = Flywheel
- 潜意识 = Subconscious
- 指针 = Pointer, 节点 = Node, 碎片 = Fragment
- 项目 = Project, 工具 = Tool, 沙箱 = Sandbox
- 提示词 = Prompt, 界面 = UI, 设置 = Settings
Keep product name "GrowBox" untranslated.`;

// --- 提取 FALLBACK_ZH_CN ---
function extractFallback() {
  const src = fs.readFileSync(I18N_TS, "utf8");
  const decl = src.indexOf("FALLBACK_ZH_CN");
  const braceStart = src.indexOf("{", decl);
  // 大括号配平(忽略字符串内的括号)
  let depth = 0, i = braceStart, inStr = false, esc = false;
  for (; i < src.length; i++) {
    const c = src[i];
    if (inStr) {
      if (esc) esc = false;
      else if (c === "\\") esc = true;
      else if (c === '"') inStr = false;
    } else {
      if (c === '"') inStr = true;
      else if (c === "{") depth++;
      else if (c === "}") { depth--; if (depth === 0) break; }
    }
  }
  const body = src.slice(braceStart + 1, i);
  const map = {};
  // 逐条匹配  key: "value",  (value 允许转义,单行)
  const re = /^[ \t]*([A-Za-z0-9_]+)[ \t]*:[ \t]*"((?:[^"\\]|\\.)*)"[ \t]*,?[ \t]*$/gm;
  let m;
  while ((m = re.exec(body)) !== null) {
    const key = m[1];
    // 反转义成真实字符串
    const val = JSON.parse(`"${m[2]}"`);
    map[key] = val;
  }
  return map;
}

// --- deepseek 一次翻译一批 {key: zh} → {key: 目标语言} ---
async function translateChunk(entries, langName) {
  const payloadIn = Object.fromEntries(entries);
  const sys = `You are a precise UI localization engine for a desktop AI app. Translate the VALUES of the given JSON object from Simplified Chinese to ${langName}.
Rules:
- Return ONLY a JSON object with the SAME keys, values translated. No markdown, no commentary.
- Preserve every placeholder token EXACTLY as-is: {s} {e} {m} {n} {dg} {rd} {gr} {name} {dir} and any {xxx}. Do not translate or reorder them.
- Keep it concise and natural for UI labels/buttons/hints.
- Keep ASCII technical tokens (API, JSON, token, e5, L2, RAG, OpenAI) untranslated.
${GLOSSARY}`;
  const body = {
    model: MODEL,
    messages: [
      { role: "system", content: sys },
      { role: "user", content: JSON.stringify(payloadIn, null, 0) },
    ],
    temperature: 0,
    max_tokens: 16000,
  };
  const res = await fetch(`${API_BASE}/chat/completions`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${KEY}` },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`HTTP ${res.status}: ${(await res.text()).slice(0, 300)}`);
  const data = await res.json();
  let content = data.choices?.[0]?.message?.content ?? "";
  content = content.trim().replace(/^```(?:json)?/i, "").replace(/```$/, "").trim();
  const obj = JSON.parse(content);
  return obj;
}

async function translateMap(map, langName, chunkSize = 35) {
  const keys = Object.keys(map);
  const out = {};
  for (let i = 0; i < keys.length; i += chunkSize) {
    const slice = keys.slice(i, i + chunkSize).map((k) => [k, map[k]]);
    let attempt = 0;
    for (;;) {
      try {
        const res = await translateChunk(slice, langName);
        for (const [k] of slice) {
          if (typeof res[k] === "string" && res[k].length > 0) out[k] = res[k];
        }
        process.stdout.write(`  ${langName}: ${Math.min(i + chunkSize, keys.length)}/${keys.length}\n`);
        break;
      } catch (e) {
        attempt++;
        if (attempt >= 3) { console.error(`  chunk 失败(跳过,回退 zh-CN): ${e.message}`); break; }
        await new Promise((r) => setTimeout(r, 1500 * attempt));
      }
    }
  }
  return out;
}

// 增量模式(默认):只翻译目标里缺失的键,省 API/省时(用户 2026-06-03:别每次全量重译)。
// 改了 zh-CN 源里**已存在键**的文本时,用 `--full` 强制全量重译(否则增量不会覆盖旧译)。
const FULL = process.argv.includes("--full");

// 读已存 locale 的现有翻译(供增量跳过)。文件不存在/解析失败 → 空对象(等价全量)。
function loadLocale(lang) {
  try {
    const src = fs.readFileSync(`${LOCALES_DIR}/${lang}.ts`, "utf8");
    const b = src.indexOf("{", src.indexOf("Record<string, string> ="));
    let d = 0, i = b, inStr = false, esc = false;
    for (; i < src.length; i++) {
      const c = src[i];
      if (inStr) { if (esc) esc = false; else if (c === "\\") esc = true; else if (c === '"') inStr = false; }
      else { if (c === '"') inStr = true; else if (c === "{") d++; else if (c === "}") { d--; if (d === 0) break; } }
    }
    return JSON.parse(src.slice(b, i + 1));
  } catch { return {}; }
}

function emitLocale(lang, varName, map) {
  const entries = Object.entries(map)
    .map(([k, v]) => `  ${JSON.stringify(k)}: ${JSON.stringify(v)},`)
    .join("\n");
  const header = `// 自动生成(scripts/translate-i18n.mjs,deepseek 翻译 zh-CN 源)。缺键运行时回退 zh-CN。\n// 手改请改 zh-CN 源(i18n.ts FALLBACK_ZH_CN)后重跑脚本,勿直接编辑本文件。\n`;
  const content = `${header}const ${varName}: Record<string, string> = ${JSON.stringify(map, null, 2)};\n\nexport default ${varName};\n`;
  fs.writeFileSync(`${LOCALES_DIR}/${lang}.ts`, content);
  console.log(`写出 locales/${lang}.ts (${Object.keys(map).length} 键)`);
  void entries;
}

// --- tools.i18n.json 的 ja/zh-TW(label + ui_desc;en 已亲写,保留)---
async function translateTools() {
  const json = JSON.parse(fs.readFileSync(TOOLS_JSON, "utf8"));
  const toolNames = Object.keys(json).filter((k) => !k.startsWith("_"));
  for (const lang of ["ja", "zh-TW"]) {
    // 收集需译的 label/ui_desc 的 zh-CN 值(增量:跳过目标已有的),扁平 key = "tool|field"
    const flat = {};
    for (const name of toolNames) {
      if (FULL || !json[name].label[lang]) flat[`${name}|label`] = json[name].label["zh-CN"];
      if (FULL || !json[name].ui_desc[lang]) flat[`${name}|ui_desc`] = json[name].ui_desc["zh-CN"];
    }
    if (Object.keys(flat).length === 0) {
      console.log(`工具卡片 → ${lang}: 已是最新,跳过(增量)`);
      continue;
    }
    const translated = await translateMap(flat, LANGS[lang], 24);
    for (const name of toolNames) {
      const lab = translated[`${name}|label`];
      const desc = translated[`${name}|ui_desc`];
      if (lab) json[name].label[lang] = lab;
      if (desc) json[name].ui_desc[lang] = desc;
    }
  }
  fs.writeFileSync(TOOLS_JSON, JSON.stringify(json, null, 2) + "\n");
  console.log("更新 prompts/tools.i18n.json 的 ja/zh-TW(label/ui_desc)");
}

// --- notices.i18n.json 的 human ja/zh-TW(zh-CN/en + 对内 llm 已亲写,保留)---
// 维护单元 = 一条提示:改 zh-CN human 重跑即重生 ja/zh-TW。占位符 {x} 由 translateChunk 的规则原样保留。
async function translateNotices() {
  const json = JSON.parse(fs.readFileSync(NOTICES_JSON, "utf8"));
  const codes = Object.keys(json).filter((k) => !k.startsWith("_"));
  for (const lang of ["ja", "zh-TW"]) {
    const flat = {};
    for (const code of codes) if (FULL || !json[code].human[lang]) flat[code] = json[code].human["zh-CN"];
    if (Object.keys(flat).length === 0) {
      console.log(`提示告知 → ${lang}: 已是最新,跳过(增量)`);
      continue;
    }
    const translated = await translateMap(flat, LANGS[lang], 30);
    for (const code of codes) {
      if (translated[code]) json[code].human[lang] = translated[code];
    }
  }
  fs.writeFileSync(NOTICES_JSON, JSON.stringify(json, null, 2) + "\n");
  console.log("更新 prompts/notices.i18n.json 的 ja/zh-TW(human)");
}

async function main() {
  console.log("提取 FALLBACK_ZH_CN ...");
  const fallback = extractFallback();
  console.log(`  ${Object.keys(fallback).length} 个 UI key`);

  for (const [lang, langName] of Object.entries(LANGS)) {
    const existing = FULL ? {} : loadLocale(lang);
    const missing = Object.fromEntries(Object.entries(fallback).filter(([k]) => !(k in existing)));
    const nMissing = Object.keys(missing).length;
    const varName = lang === "zh-TW" ? "zhTW" : lang;
    if (nMissing === 0) {
      console.log(`UI 字典 → ${lang}: 已是最新,跳过(增量)`);
      continue;
    }
    console.log(`翻译 UI 字典 → ${lang} ...(${nMissing} 键${FULL ? ",全量" : ",仅缺键"})`);
    const newTrans = await translateMap(missing, langName);
    // 保留既有译文(键序:旧在前、新追加),只补缺键。
    emitLocale(lang, varName, { ...existing, ...newTrans });
  }

  console.log("翻译工具卡片 → ja/zh-TW ...");
  await translateTools();

  console.log("翻译提示告知 → ja/zh-TW ...");
  await translateNotices();

  console.log("完成。");
}

main().catch((e) => { console.error(e); process.exit(1); });
