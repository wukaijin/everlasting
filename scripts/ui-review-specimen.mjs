#!/usr/bin/env node
// scripts/ui-review-specimen.mjs — 定向视觉评审的「对比度样本页」生成器。
//
// 与 scripts/ui-review.sh(整屏截图 + 泛化 VLM 评审)互补:那个覆盖"真实界面的
// 整体观感",这个覆盖"设计 token 全矩阵的系统性核对"。动机(2026-08-15 对比度
// 专项评审):真实截图只能看到"恰好出现在当前界面上"的色对,accent-作-文字、
// error-文字-在-elevated、彩底-muted 这些 FAIL 组合在 7 张标准截图里时有时无;
// 样本页把全部 文字token × 背景token 渲染在真实字体/字号下,每个格子标注
// 计算出的 WCAG 比值,再过 VLM 做"感知层"复核 —— 数字与观感各管一半。
//
// 用法:
//   node scripts/ui-review-specimen.mjs --out out/ui-review/<ts>-contrast
//   node scripts/ui-review-specimen.mjs --out ... --shoot   # 顺带 headless 截图
//     (--shoot 依赖 ~/.cache/everlasting-ui-review/node_modules/playwright-core,
//      即 ui-review.sh 首次运行时装的 scratch 依赖,以及 ms-playwright Chromium)
//
// 产物:specimen.html(自包含,base64 内嵌 HarmonyOS Sans SC,可离线打开)
//       specimen.png(--shoot 时,2x DPR 全页截图)
//
// 章节结构(与 VLM 评审 prompt 一一对应,改章节记得同步 skill 里的 prompt):
//   A 灰阶文字 × 全部背景层(13px 正文 + 11px mono 元数据双档)
//   B 彩色文字(accent/tool/status)× 背景层(11px mono + 13px 双档)
//   C 交互态行(hover/selected 叠加态上的文字)
//   D 现状 vs 候选修法并排(本轮: accent 文字色 / error 文字色 / 彩底元数据 /
//     elevated 提亮)—— 候选色硬编码在本文件 CANDIDATES 段,评审通过后
//     才进 style.css,届时同步删除对应候选。
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";

const REPO = resolve(dirname(new URL(import.meta.url).pathname), "..");

// ── 参数 ────────────────────────────────────────────────────────────
const args = process.argv.slice(2);
const outDirIdx = args.indexOf("--out");
const OUT = outDirIdx >= 0 ? resolve(args[outDirIdx + 1]) : resolve("out/ui-review/specimen");
const SHOOT = args.includes("--shoot");
mkdirSync(OUT, { recursive: true });

// ── 解析 style.css 的 --color-* token(@theme 块内;值可为 color-mix())──
const css = readFileSync(`${REPO}/app/src/style.css`, "utf8");
const tokens = {};
for (const m of css.matchAll(/^\s*(--color-[\w-]+):\s*([^;]+);/gm)) tokens[m[1]] = m[2].trim();
if (!tokens["--color-text-primary"]) {
  console.error("✗ 未能从 app/src/style.css 解析出 --color-* token(文件结构变了?)");
  process.exit(1);
}

// ── WCAG 对比度(color-mix() 值在页面里原生解析;这里只算静态 hex 对)──
const hex = (v) => /^#[0-9a-f]{6}$/i.test(v);
const lum = (h) => {
  const c = [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16) / 255)
    .map((v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
};
const contrast = (a, b) => {
  if (!hex(a) || !hex(b)) return null; // color-mix 动态值交给浏览器,不算数
  const [l1, l2] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (l1 + 0.05) / (l2 + 0.05);
};
const badge = (fg, bg) => {
  const r = contrast(fg, bg);
  if (r == null) return "";
  const cls = r >= 7 ? "aaa" : r >= 4.5 ? "aa" : r >= 3 ? "large" : "fail";
  return `<span class="ratio ${cls}">${r.toFixed(2)}</span>`;
};
// 候选修法。已采纳进 style.css 的候选必须从这里删掉(08-15-contrast-color-r1
// 已采纳 accent-text/error-text 并删除),否则 D 章变成自我对照。仅保留
// 未决候选: elevatedUp(D4 被 mmx 驳回为"半步方案",专题停泊中)。
const CANDIDATES = {
  elevatedUp: "#1d2433", // bg-elevated 提亮一档(未决,层级感知专题)
};

// 彩底规则(design-tokens.md,08-15-contrast-color-r1):彩色/低亮度底上文字
// 最低 secondary、禁同色系 accent/紫字。这些"文字×彩底"组合在样本里标
// 「禁用」而非当 FAIL —— 它们不该被用到,数字仅作参考。
const DISALLOWED_FG = new Set(["--color-text-muted"]); // × 彩底(accent-muted)

// ── 样本内容(贴近真实 UI 文案,含中英混排)─────────────────────────
const BODY = "深色主题里的正文文字,混排 English 与数字 42% 的可读性样本。";
const META = "11s · read_file · 2.4K tokens";

const bgLadder = [
  ["app", "--color-bg-app"],
  ["surface", "--color-bg-surface"],
  ["elevated", "--color-bg-elevated"],
  ["accent-muted", "--color-accent-muted"],
];
const grayTiers = [
  ["text-primary", "--color-text-primary"],
  ["text-secondary", "--color-text-secondary"],
  ["text-muted", "--color-text-muted"],
];
const colorTiers = [
  ["accent-text(文字)", "--color-accent-text"],
  ["tool-read", "--color-tool-read"],
  ["tool-write", "--color-tool-write"],
  ["tool-shell", "--color-tool-shell"],
  ["tool-error-text(文字)", "--color-tool-error-text"],
  ["tool-thinking", "--color-tool-thinking"],
  ["status-success", "--color-status-success"],
  ["status-warn", "--color-status-warn"],
];

const cell = (fgVar, bgVar, fgHex, bgHex) => {
  const banned = DISALLOWED_FG.has(fgVar) && bgVar === "--color-accent-muted";
  return `
  <div class="cell" style="background:var(${bgVar})">
    <p class="body" style="color:var(${fgVar})">${BODY}${banned ? '<span class="ratio disallowed">禁用</span>' : badge(fgHex, bgHex)}</p>
    <p class="meta" style="color:var(${fgVar})">${META}</p>
    <span class="pair">${fgVar.replace("--color-", "")} × ${bgVar.replace("--color-bg-", "")}</span>
  </div>`;
};

const sections = [];

// A 灰阶 × 背景
sections.push(`<section><h2>A · 灰阶文字 × 背景层</h2><div class="grid">`);
for (const [fn, fv] of grayTiers)
  for (const [bn, bv] of bgLadder)
    sections.push(cell(fv, bv, tokens[fv], tokens[bv]));
sections.push(`</div></section>`);

// B 彩色文字 × 背景(accent-muted 只列高频组合,避免矩阵爆炸)
sections.push(`<section><h2>B · 彩色文字 × 背景层</h2><div class="grid">`);
for (const [fn, fv] of colorTiers)
  for (const [bn, bv] of bgLadder.slice(1)) // surface / elevated / accent-muted
    sections.push(cell(fv, bv, tokens[fv], tokens[bv]));
sections.push(`</div></section>`);

// C 交互态(surface 容器内 hover/selected 叠加;color-mix 透明叠加会与父底色合成)
const stateRow = (stateBg, label, fgVar, fgHex) => `
  <div class="staterow" style="background:${stateBg}">
    <span class="body" style="color:var(${fgVar})">${BODY}</span>
    <span class="meta" style="color:var(--color-text-muted)">· ${label}</span>
  </div>`;
sections.push(`<section><h2>C · 交互态叠加(surface 之上)</h2><div class="states">
  <div style="background:var(--color-bg-surface);border-radius:8px;overflow:hidden">
    ${stateRow("transparent", "default", "--color-text-primary")}
    ${stateRow("var(--color-bg-hover)", "hover", "--color-text-secondary")}
    ${stateRow("var(--color-bg-active)", "active", "--color-text-secondary")}
    ${stateRow("var(--color-bg-selected)", "selected", "--color-text-primary")}
  </div>
  <p class="note">hover/active/selected 为 color-mix 透明叠加,与父级 surface 合成;比值无法静态计算,靠观感核对。</p>
</div></section>`);

// D 未决候选 + 规则演示。已采纳的候选(accent-text / error-text,2026-08-15)
// 进 token 后从此章移除 —— B 章的 ink token 行即它们的"采纳后"状态。
const vs = (curFg, curBg, candFg, candBg, curFgHex, curBgHex, candFgHex, candBgHex, sizeCls = "body") => `
  <div class="vs">
    <div class="cell" style="background:${curBg}">
      <span class="tag">${curBg === candBg ? "对照组A" : "现状"}</span>
      <p class="${sizeCls}" style="color:${curFg}">${BODY}</p>${badge(curFgHex, curBgHex)}
    </div>
    <div class="cell" style="background:${candBg}">
      <span class="tag cand">${curBg === candBg ? "对照组B" : "候选"}</span>
      <p class="${sizeCls}" style="color:${candFg}">${BODY}</p>${badge(candFgHex, candBgHex)}
    </div>
  </div>`;
const bgv = (n) => `var(${n})`;
sections.push(`<section><h2>D · 彩底规则演示 + 未决候选</h2>
  <h3>D1 彩底(accent-muted)最低 secondary 规则:muted(禁用)vs secondary</h3><div class="vscol">
  ${vs(bgv("--color-text-muted"), bgv("--color-accent-muted"), bgv("--color-text-secondary"), bgv("--color-accent-muted"),
       tokens["--color-text-muted"], tokens["--color-accent-muted"], tokens["--color-text-secondary"], tokens["--color-accent-muted"])}
  </div>
  <h3>D2 elevated 提亮一档(未决:mmx 裁决"半步方案",层级感知专题停泊中)</h3><div class="vscol">
  ${vs(bgv("--color-text-muted"), bgv("--color-bg-elevated"), bgv("--color-text-muted"), CANDIDATES.elevatedUp,
       tokens["--color-text-muted"], tokens["--color-bg-elevated"], tokens["--color-text-muted"], CANDIDATES.elevatedUp)}
  </div>
</section>`);

// ── 组装 HTML(base64 内嵌 HarmonyOS Sans SC,保证字体渲染与真实 app 一致)──
const fontB64 = readFileSync(
  `${REPO}/app/src/assets/fonts/HarmonyOSSansSC-Regular.subset.woff2`,
).toString("base64");
const tokenBlock = Object.entries(tokens).map(([k, v]) => `${k}:${v};`).join("");

const html = `<!doctype html><html><head><meta charset="utf-8"><style>
@font-face{font-family:"HarmonyOS Sans SC";src:url(data:font/woff2;base64,${fontB64}) format("woff2");font-weight:400}
:root{${tokenBlock}}
*{margin:0;padding:0;box-sizing:border-box}
body{background:var(--color-bg-app);color:var(--color-text-primary);
  font-family:"HarmonyOS Sans SC","Noto Sans CJK SC",system-ui,sans-serif;
  font-size:13px;line-height:1.6;padding:24px;max-width:1080px}
h1{font-size:16px;margin-bottom:4px;color:var(--color-text-primary)}
h2{font-size:14px;margin:24px 0 10px;color:var(--color-text-primary);border-bottom:1px solid var(--color-bg-border-strong);padding-bottom:6px}
h3{font-size:12px;margin:14px 0 8px;color:var(--color-text-secondary)}
.sub{font-size:11px;color:var(--color-text-muted);margin-bottom:16px}
.grid{display:grid;grid-template-columns:repeat(2,1fr);gap:10px}
.cell{border-radius:6px;padding:12px;position:relative;min-height:74px}
.body{font-size:13px;line-height:1.6}
.meta{font-family:var(--font-mono);font-size:11px;margin-top:4px;line-height:1.4}
.pair{position:absolute;right:10px;bottom:6px;font-family:var(--font-mono);font-size:9px;color:var(--color-text-muted);opacity:.85}
.ratio{position:absolute;top:8px;right:10px;font-family:var(--font-mono);font-size:10px;padding:1px 5px;border-radius:3px}
.ratio.aaa{background:#14532d;color:#bbf7d0}.ratio.aa{background:#1e3a5f;color:#bfdbfe}
.ratio.large{background:#7c2d12;color:#fed7aa}.ratio.fail{background:#7f1d1d;color:#fecaca}
.ratio.disallowed{background:#374151;color:#d1d5db}
.vscol{display:flex;flex-direction:column;gap:8px}
.vs{display:grid;grid-template-columns:1fr 1fr;gap:10px}
.tag{font-family:var(--font-mono);font-size:9px;color:var(--color-text-muted);display:block;margin-bottom:4px}
.tag.cand{color:var(--color-status-success)}
.staterow{padding:10px 12px;display:flex;gap:8px;align-items:baseline;border-bottom:1px solid var(--color-bg-border)}
.staterow:last-child{border-bottom:0}
.note{font-size:10px;color:var(--color-text-muted);margin-top:8px}
</style></head><body>
<h1>对比度样本页 · Contrast Specimen</h1>
<p class="sub">token 直读自 app/src/style.css;右上角数字 = WCAG 对比度(绿 AAA≥7 / 蓝 AA≥4.5 / 橙 仅大字≥3 / 红 FAIL / 灰=规则禁用对,数字仅供参考)。body 13px 中英混排 + mono 11px 元数据双档。彩底(accent-muted)上的文字组合受彩底规则约束:最低 secondary、禁同色系 accent/紫字。</p>
${sections.join("\n")}
</body></html>`;

const htmlPath = `${OUT}/specimen.html`;
writeFileSync(htmlPath, html);
console.log(`✓ ${htmlPath}(${(html.length / 1024).toFixed(0)} KB,token ${Object.keys(tokens).length} 个)`);

// ── --shoot:headless 截图(scratch playwright-core,2x DPR 保小字清晰)──
if (SHOOT) {
  const require = createRequire(`${process.env.HOME}/.cache/everlasting-ui-review/`);
  const { chromium } = require("playwright-core");
  const { execSync } = require("node:child_process");
  const chrome = execSync(
    `find ~/.cache/ms-playwright -maxdepth 4 -type f -name chrome -path '*chromium*' | sort -V | tail -1`,
  ).toString().trim();
  const browser = await chromium.launch({
    executablePath: chrome,
    args: ["--no-sandbox", "--disable-gpu", "--force-color-profile=srgb"],
  });
  const page = await browser.newPage({
    viewport: { width: 1080, height: 900 },
    deviceScaleFactor: 2,
  });
  await page.goto(`file://${htmlPath}`, { waitUntil: "networkidle" });
  await page.waitForTimeout(600); // 字体解码
  // 全页大图 + 分章节图。教训(2026-08-15 首跑):全页图太高,VLM 下采样后
  // 文字成糊,模型只能幻觉(把 AAA 格说成 FAIL、把暗底说成浅底)。分章节
  // 截图保证每张都在可判读尺寸内,VLM 逐章过,全页图只留给人看。
  await page.screenshot({ path: `${OUT}/specimen.png`, fullPage: true });
  const secs = await page.$$("section");
  const names = ["a", "b", "c", "d"];
  for (let i = 0; i < secs.length && i < names.length; i++)
    await secs[i].screenshot({ path: `${OUT}/specimen-${names[i]}.png` });
  await browser.close();
  console.log(`✓ ${OUT}/specimen.png + specimen-{a,b,c,d}.png`);
}
