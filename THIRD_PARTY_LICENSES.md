# Third-Party Licenses

> Bundled third-party assets and their licenses.

---

## HarmonyOS Sans SC (字体)

本应用将 HarmonyOS Sans SC 的子集 (3500 常用字 + ASCII + 标点) 打包为内置字体,
用于在所有平台上提供跨 Windows / macOS / Linux 一致的中文 UI 渲染。

- **来源**: <https://github.com/SunsetMkt/HarmonyOS_Sans_SC_Webfont_Splitted>
- **原始 TTF 版权**: © 2021 Huawei Device Co., Ltd.
- **License**: HarmonyOS Sans Fonts License Agreement
- **完整许可证文本**: [`app/src/assets/fonts/LICENSE.txt`](./app/src/assets/fonts/LICENSE.txt)
- **子集脚本**: [`app/scripts/subset-font.mjs`](./app/scripts/subset-font.mjs)
- **字表来源**: 邢红兵《现代汉语常用字表》3500 字,见
  <https://github.com/jinghu-moon/Simplified-Chinese-Characters>
- **子集文件**: `app/src/assets/fonts/HarmonyOSSansSC-Regular.subset.woff2`
  (Regular 字重,woff2 格式,约 470 KB)

### 使用条件 (per License Agreement)

1. **Prominent notice** — 已在 `app/src/style.css` 顶部、本文件,以及同目录的
   `LICENSE.txt` 中显著标注 HarmonyOS Sans 的使用。
2. **No modification** — 字体文件未做任何修改,仅作子集 (subset) 与 woff2 压缩。
   子集化不修改原字体表中的字形与指令。
3. **No standalone redistribution** — 字体仅随本应用打包发布,不作为独立字体
   软件再分发或出售。
