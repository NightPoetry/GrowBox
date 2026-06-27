#!/usr/bin/env node
// i18n 完整性守卫(健康体检 Tier3c)。两项检查:
//   ① 组件里 t("key") 用到的 key 必须存在于 i18n.ts 的 FALLBACK_ZH_CN(中文唯一真相)——否则 UI 显示裸 key(硬错)。
//   ② 中文源的每个 key 应存在于 en/ja/zh-TW 三语 locale——否则该语种回退中文(软错,只警告)。
// 用法:node scripts/check-i18n.mjs   (用到的 key 缺源 → 退出码 1,可挂进 npm build / CI 拦漏)。
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const SRC = join(dirname(fileURLToPath(import.meta.url)), "..", "crates", "growbox-gui", "frontend", "src");

// 抓一个对象字面量里的顶层 key(行首缩进 + `key:` 或 `"key":`)。region 限定避免抓到无关代码。
function keysIn(text, fromMarker) {
  let region = text;
  if (fromMarker) {
    const i = text.indexOf(fromMarker);
    if (i < 0) return new Set();
    region = text.slice(i + fromMarker.length);
    const end = region.indexOf("\n};");
    if (end >= 0) region = region.slice(0, end);
  }
  const keys = new Set();
  for (const m of region.matchAll(/^\s*["']?([A-Za-z_][A-Za-z0-9_]*)["']?\s*:\s*["'`]/gm)) keys.add(m[1]);
  return keys;
}

function walk(dir) {
  let out = [];
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    if (statSync(p).isDirectory()) out = out.concat(walk(p));
    else if (/\.(ts|tsx)$/.test(e)) out.push(p);
  }
  return out;
}

const src = keysIn(readFileSync(join(SRC, "i18n.ts"), "utf8"), "FALLBACK_ZH_CN: Record<string, string> = {");
const locales = {
  en: keysIn(readFileSync(join(SRC, "locales", "en.ts"), "utf8")),
  ja: keysIn(readFileSync(join(SRC, "locales", "ja.ts"), "utf8")),
  "zh-TW": keysIn(readFileSync(join(SRC, "locales", "zh-TW.ts"), "utf8")),
};

// 收集组件里用到的字面量 key:t("X")（跳过动态 t(变量) — 无法静态校验）。
const used = new Set();
for (const f of walk(SRC)) {
  if (f.endsWith("i18n.ts") || f.includes(`${join("locales", "")}`)) continue;
  const text = readFileSync(f, "utf8");
  for (const m of text.matchAll(/\bt\(\s*["']([A-Za-z0-9_]+)["']\s*\)/g)) used.add(m[1]);
}

let fail = false;
const missingSrc = [...used].filter((k) => !src.has(k)).sort();
if (missingSrc.length) {
  fail = true;
  console.error(`[i18n] ✗ ${missingSrc.length} 个 t("key") 用到的 key 不在 i18n.ts 中文源(UI 会显示裸 key):`);
  for (const k of missingSrc) console.error(`    - ${k}`);
}

for (const [lang, ks] of Object.entries(locales)) {
  const miss = [...src].filter((k) => !ks.has(k)).sort();
  if (miss.length) console.warn(`[i18n] ! ${lang} 缺 ${miss.length} 条翻译(回退中文):${miss.slice(0, 8).join(", ")}${miss.length > 8 ? " …" : ""}`);
}

console.log(`[i18n] 源 ${src.size} 键 / 组件用到 ${used.size} 键 / en ${locales.en.size} ja ${locales.ja.size} zh-TW ${locales["zh-TW"].size}`);
if (fail) { console.error("[i18n] 守卫失败:补齐上面的中文源 key 再继续。"); process.exit(1); }
console.log("[i18n] 守卫通过:所有 t(\"key\") 都有中文源。");
