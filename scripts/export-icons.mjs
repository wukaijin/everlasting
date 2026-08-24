// scripts/export-icons.mjs — 用 headless Chromium 把 brand SVG 导出为 PWA 图标 PNG。
// 用法:node scripts/export-icons.mjs [OUT_DIR]  (默认 brand/png)
// 依赖:playwright-core + ~/.cache/ms-playwright 下任一 Chromium。
// 产出(浅色底变体,解决桌面快捷方式图标空白/不可见的问题):
//   app-icon-192-light.png / app-icon-512-light.png (浅底 #f8fafc + mark 66%)
//   app-icon-512-maskable-light.png                (浅底 + mark 50%,20% 安全区)
//   favicon-32-light.png                           (透明底 + 深色弧线,64px)
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import fs from "node:fs";
import path from "node:path";
const require = createRequire(import.meta.url);
const { chromium } = require("playwright-core");

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const BRAND = path.join(REPO, "brand");
const OUT_DIR = process.argv[2] || path.join(BRAND, "png");

const CHROME_EXE =
  process.env.CHROME_EXE ||
  "/root/.cache/ms-playwright/chromium-1223/chrome-linux64/chrome";

// 复用 logo-mark-light.svg 的 <g> 内容(600 级深色弧线 + 深色 spark)。
function markInner() {
  const svg = fs.readFileSync(path.join(BRAND, "logo-mark-light.svg"), "utf8");
  return svg.replace(/^[\s\S]*?<g/s, "<g").replace(/<\/svg>[\s\S]*$/s, "");
}

// 应用图标:全出血浅底 + 居中 mark(scale 决定安全区)。
// width/height = size(容器同尺寸),viewBox 固定 512 保证几何一致。
function appIconSVG(markScale, size) {
  const t = 256 * (1 - markScale); // 使 mark 外接盒居中
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 512 512" role="img" aria-label="Everlasting app icon">
  <rect width="512" height="512" fill="#f8fafc"/>
  <g transform="translate(${t} ${t}) scale(${markScale})">${markInner()}</g>
</svg>`;
}

// favicon:透明底 + 深色弧线(同现有 favicon 结构,只换配色)。
function faviconSVG(size) {
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 512 512" role="img" aria-label="Everlasting favicon">
  <g transform="translate(0 0) scale(1)">${markInner()}</g>
</svg>`;
}

async function render(svg, size, outPath) {
  const browser = await chromium.launch({
    executablePath: CHROME_EXE,
    args: ["--no-sandbox", "--disable-gpu", "--force-color-profile=srgb"],
  });
  try {
    const page = await browser.newPage({ viewport: { width: size, height: size } });
    await page.setContent(
      `<html><body style="margin:0"><div style="width:${size}px;height:${size}px">${svg}</div></body></html>`,
      { waitUntil: "load" },
    );
    await page.waitForTimeout(300);
    await page.$eval("div", (el) => el);
    await page.$("div").then((el) => el.screenshot({ path: outPath }));
  } finally {
    await browser.close();
  }
}

fs.mkdirSync(OUT_DIR, { recursive: true });
const jobs = [
  ["app-icon-192-light.png", 192, appIconSVG(0.66, 192)],
  ["app-icon-512-light.png", 512, appIconSVG(0.66, 512)],
  ["app-icon-512-maskable-light.png", 512, appIconSVG(0.5, 512)],
  ["favicon-32-light.png", 64, faviconSVG(64)],
];
for (const [name, size, svg] of jobs) {
  render(svg, size, path.join(OUT_DIR, name)).then(() =>
    console.log("wrote", name),
  );
}
