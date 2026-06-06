// scripts/subset-font.mjs
// 用 HarfBuzz WASM 子集化 HarmonyOS Sans SC 到 3500 常用字 + ASCII + 标点
// 输出: app/src/assets/fonts/HarmonyOSSansSC-Regular.subset.woff2
//
// 用法(从项目根 / 仓库根):
//   node app/scripts/subset-font.mjs
//   # 或:
//   TTF_PATH=/path/to/HarmonyOSSansSC-Regular.ttf \
//   CHARS=/path/to/cn-3500.txt \
//   OUT_PATH=/path/to/output.woff2 \
//   node app/scripts/subset-font.mjs
//
// 来源:
//   - 字体: https://github.com/SunsetMkt/HarmonyOS_Sans_SC_Webfont_Splitted
//     (原始 TTF 版权 © 2021 Huawei Device Co., Ltd., 详见 LICENSE.txt)
//   - 3500 常用字: https://github.com/jinghu-moon/Simplified-Chinese-Characters
//     (邢红兵《现代汉语常用字表》3500 字)
//
// 依赖(项目 devDependencies,已声明在 app/package.json):
//   - subset-font  HarfBuzz WASM subsetter
//   - wawoff2      woff2 编/解码 (Node binding to woff2)
//
// 如果 `pnpm install` 还没跑过(脚本没装上),会打印清晰错误,引导用户安装。

import { readFile, writeFile } from "node:fs/promises";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
// 仓库根: app/scripts/ → app/ → 仓库根
const APP_ROOT = resolve(__dirname, "..");
const REPO_ROOT = resolve(APP_ROOT, "..");

const DEFAULT_TTF = resolve(
  APP_ROOT,
  "src/assets/fonts/source/HarmonyOSSansSC-Regular.ttf",
);
const DEFAULT_CHARS = resolve(__dirname, "cn-3500.txt");
const DEFAULT_OUT = resolve(
  APP_ROOT,
  "src/assets/fonts/HarmonyOSSansSC-Regular.subset.woff2",
);

const TTF_PATH = resolve(REPO_ROOT, process.env.TTF_PATH ?? DEFAULT_TTF);
const CHARS_PATH = resolve(REPO_ROOT, process.env.CHARS ?? DEFAULT_CHARS);
const OUT_PATH = resolve(REPO_ROOT, process.env.OUT_PATH ?? DEFAULT_OUT);

async function loadDeps() {
  try {
    const [{ default: subsetFont }, { default: wawoff }] = await Promise.all([
      import("subset-font"),
      import("wawoff2"),
    ]);
    return { subsetFont, wawoff };
  } catch (err) {
    if (
      err instanceof Error &&
      (err.code === "ERR_MODULE_NOT_FOUND" ||
        /Cannot find module/.test(err.message))
    ) {
      console.error(
        [
          "[subset-font] 缺少 npm 依赖: subset-font / wawoff2",
          "",
          "此脚本仅在「重新子集化字体」时需要,不在 build 链路中。",
          "请在 app/ 目录下安装 devDependencies 后再跑:",
          "",
          "    cd app && pnpm install",
          "",
          "或在临时目录手动装:",
          "",
          "    npm install --no-save subset-font wawoff2",
          "    cd app && NODE_PATH=/path/to/node_modules node scripts/subset-font.mjs",
          "",
        ].join("\n"),
      );
      process.exit(1);
    }
    throw err;
  }
}

async function main() {
  const { subsetFont, wawoff } = await loadDeps();

  const [ttfBuffer, charListRaw] = await Promise.all([
    readFile(TTF_PATH),
    readFile(CHARS_PATH, "utf-8"),
  ]);

  // 3500 常用字 + ASCII + 标点
  const text = charListRaw.replace(/\s+/g, "");
  console.log(
    `[subset] input TTF: ${TTF_PATH} (${(ttfBuffer.byteLength / 1024 / 1024).toFixed(2)} MB)`,
  );
  console.log(
    `[subset] char list: ${CHARS_PATH} (${text.length} characters)`,
  );
  console.log(`[subset] output:    ${OUT_PATH}`);

  // 1. subset (returns TTF buffer)
  console.log("[subset] running HarfBuzz subset...");
  const t0 = Date.now();
  const subsetTtf = await subsetFont(ttfBuffer, text, {
    targetFormat: "truetype",
  });
  console.log(
    `[subset] subset done in ${Date.now() - t0}ms, ${(subsetTtf.byteLength / 1024 / 1024).toFixed(2)} MB`,
  );

  // 2. compress to woff2
  console.log("[woff2] compressing...");
  const t1 = Date.now();
  const woff2Buffer = await wawoff.compress(subsetTtf);
  console.log(
    `[woff2] compressed in ${Date.now() - t1}ms, ${(woff2Buffer.byteLength / 1024 / 1024).toFixed(2)} MB`,
  );

  // 3. write
  await writeFile(OUT_PATH, woff2Buffer);
  console.log(`[out] wrote: ${OUT_PATH}`);
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
