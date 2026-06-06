# 打包 HarmonyOS Sans SC 中文字体子集（方案 2）

> 关联 task: `.trellis/tasks/06-06-font-cjk-harmosans-bundle/`
> 前置 task: `06-06-font-cjk-stack-polish`（方案 1 已落地但效果不达预期，因 Windows WebView2 命中 `Microsoft YaHei` 而非更好的中文字体）
> 优先级: P2
> 范围: 新增字体资产 + `app/src/style.css` + `index.html`（可选 preload）

## Goal

把 HarmonyOS Sans SC 切成常用 3500 字 woff2 子集打包进 Tauri 应用，作为全局中文 UI 字体的最高优先级来源，跨平台渲染一致、Dark theme 下清晰可读。

## What I already know

- 方案 1 改完 `--font-sans` 后 HMR 无可见变化，根因：`Microsoft YaHei UI` 在 Win 10/11 默认不装，回退到 `Microsoft YaHei`（与原栈相同）
- 微软雅黑本身在 Dark theme + 14-15px + 抗锯齿弱场景下是糊的天花板，CSS 杠杆救不了
- `app/src/style.css` 当前 `--font-sans` 已是 `HarmonyOS Sans SC` 在第一位，但系统无该字体
- Tauri 2 桌面应用能稳定加载本地 woff2（无 CORS 问题、无外部网络依赖）
- `app/src/assets/` 已有目录；现有 `app/src/assets/index-*.css` 34 kB 主要是 Tailwind + theme
- `fontTools` / `pyftsubset` 当前未装（`pip install fonttools brotli` 装上）
- Tauri 打包走 Vite 静态资产，无需在 `tauri.conf.json` 注册字体（自动作为资源跟随 dist/）

## Assumptions (resolved)

- [A1] ✅ **用 HarmonyOS Sans SC Regular 一个字重**：MVP 够用，Medium/Heavy 留给后续迭代
- [A2] ✅ **子集化到 3500 常用字**：覆盖 99.9% UI 用字（聊天消息、菜单、按钮、状态），罕见字回退到系统字体栈
- [A3] ✅ **输出 woff2 + 真名 `HarmonyOS Sans SC`**：保持与原字体同名，`@font-face` 用 `font-display: swap` 避免 FOIT
- [A4] ✅ **fonttools 用 pip 装到 user 级别**（不污染项目 venv），不写进 requirements
- [A5] ✅ **放在 `app/src/assets/fonts/`**：Vite 默认会内联 ≤ 4KB 资产 / > 4KB 拷贝到 dist，woff2 必走 dist
- [A6] ✅ **不动 Tauri 配置**：资源随 Vite 构建产物自动打包

## Requirements

### R1 — 字体下载与归档

- 从华为开源仓库 / 官方设计资源下载 HarmonyOS Sans SC Regular（`HarmonyOSSansSC-Regular.ttf`）
- 校验文件大小、版本号
- 原始 TTF 放 `app/src/assets/fonts/source/`（不进 Vite 资源，仅作 subset 源）
- 切完后 source/ 目录可考虑删掉节省空间（或保留作文档），倾向于删

### R2 — 子集化（3500 常用字）

- 用 `pyftsubset` 切到 3500 常用字（GB2312 一级 + 常用二级 ≈ 6700 字中的高频 3500）
- 字表来源：内置（`fontTools.subset.Subsetter` 配 `unicodes`）或下载 `cn-3500.txt`
- 输出 `HarmonyOSSansSC-Regular.subset.woff2`
- 保留 OpenType 特性：`palt` / `pctx` / `kern` / `liga`
- 目标文件大小 ≤ 1.8 MB

### R3 — `@font-face` 接入

- 在 `app/src/style.css` 顶部（`@import "tailwindcss"` 之前或之后）声明 `@font-face`：
  - `font-family: 'HarmonyOS Sans SC'`
  - `src: url('./assets/fonts/HarmonyOSSansSC-Regular.subset.woff2') format('woff2')`
  - `font-weight: 400`
  - `font-style: normal`
  - `font-display: swap`
  - `unicode-range`: 3500 字 + ASCII 标点（让浏览器对未覆盖字符走 fallback）

### R4 — 字体栈顺序确认

- `--font-sans` 第一位已是 `"HarmonyOS Sans SC"`，无需再动
- 验证 Tailwind v4 `@theme` 生成的 `--font-sans` CSS 变量在打包后产物里包含新源

### R5 — 构建与可移植性

- `pnpm build` 通过
- 检查 `app/dist/assets/` 输出包含 woff2 文件
- 字体大小增加 ≤ 1.8 MB
- WebView2 启动后能正常加载字体（无 404、无 CORS 报错）

## Acceptance Criteria

- [ ] `app/src/assets/fonts/HarmonyOSSansSC-Regular.subset.woff2` 存在
- [ ] woff2 文件大小 ≤ 1.8 MB
- [ ] `app/src/style.css` 顶部有 `@font-face` 声明，`font-display: swap`
- [ ] `pnpm build` 通过
- [ ] `app/dist/assets/` 含 woff2 文件
- [ ] 在 Tauri dev 中文字符明显比之前清晰（用户视觉确认）
- [ ] DevTools 查 `getComputedStyle(body).fontFamily` 第一项是 `"HarmonyOS Sans SC"`
- [ ] 未在字表里的罕见汉字回退到系统字体（不自造字形）
- [ ] 字体加载期间无 FOIT（白屏）/ FOUT 闪屏可接受（swap 模式）

## Out of Scope

- Medium / Bold / Heavy 字重（仅 Regular）
- 字体子集动态加载（按页面 / 按字符组）
- 字体的可变字体（Variable Font）支持
- 任何 Light theme 支持
- 卸载 1.5 MB+ 罕见字子集（先求覆盖率）
- 字体 license 文档（华为官方 OFL 商用 OK，无需额外标注）
- 字体在 macOS / Linux 上的额外处理（HarmonyOS Sans SC 是跨平台字体，woff2 通吃）

## Technical Approach

### 步骤 1: 装 fonttools

```bash
pip install --user fonttools brotli
# 验证
pyftsubset --help | head -5
```

### 步骤 2: 下载 HarmonyOS Sans SC

从 https://developer.huawei.com/consumer/cn/design/harmonyos-font 或 GitHub 镜像下载 `HarmonyOSSansSC-Regular.ttf`

### 步骤 3: 准备 3500 常用字表

两个方案二选一：
- (a) 硬编码 3500 字符（参考 hanzijun / common-chinese-characters 仓库）
- (b) 从 cnchar / 常用字表项目拉 `cn-3500.txt`

倾向于 (a)：硬编码在 `scripts/subset-font.py` 里可复现。

### 步骤 4: 写 subset 脚本 `app/scripts/subset-font.py`

```python
from fontTools.subset import Subsetter, Options
from fontTools.ttLib import TTFont

FONT_SRC = "app/src/assets/fonts/source/HarmonyOSSansSC-Regular.ttf"
FONT_OUT = "app/src/assets/fonts/HarmonyOSSansSC-Regular.subset.woff2"

# 3500 常用字 + ASCII 标点
COMMON_3500 = "的一是了我不在有..."  # 实际是 3500 字字符串

options = Options()
options.flavor = "woff2"
options.layout_features = ["palt", "pctx", "kern", "liga"]
options.name_IDs = ["*"]  # 保留所有 name 表
options.notdef_outline = True

font = TTFont(FONT_SRC)
subsetter = Subsetter(options=options)
subsetter.populate(text=COMMON_3500 + "0123456789.,;:!?-_()[]{}'\"")
subsetter.subset(font)
font.flavor = "woff2"
font.save(FONT_OUT)
print(f"Saved: {FONT_OUT}")
```

### 步骤 5: 在 `style.css` 顶部加 `@font-face`

```css
@font-face {
  font-family: "HarmonyOS Sans SC";
  src: url("./assets/fonts/HarmonyOSSansSC-Regular.subset.woff2")
       format("woff2");
  font-weight: 400;
  font-style: normal;
  font-display: swap;
  unicode-range: U+4E00-9FFF, U+3000-303F, U+FF00-FFEF, U+0020-007F;  # 后续可细化
}

@import "tailwindcss";
/* ... */
```

### 步骤 6: 构建验证

```bash
cd app && pnpm build
# 确认 dist/assets/ 含 woff2
ls -lh dist/assets/*.woff2
```

### 步骤 7: 用户视觉验证

`pnpm tauri dev` 打开应用，肉眼对比：
- 之前：微软雅黑 14-15px Dark theme
- 现在：HarmonyOS Sans SC 15px Dark theme

DevTools 验证：
```js
getComputedStyle(document.body).fontFamily
// 应返回: "HarmonyOS Sans SC", "Microsoft YaHei UI", ...
```

## Decision (ADR-lite)

### Decision 1: 只打包 Regular 一个字重
- **Context**: Medium/Bold 在 UI 里用于 header / 强调，常见用法
- **Decision**: 先 Regular 落地，确认链路通后单独 task 加 Medium
- **Consequences**: header / 强调字重暂时会触发 Regular + font-synthesis: bold 合成；不影响阅读，后续补

### Decision 2: 3500 常用字子集（不全量）
- **Context**: 全量 HarmonyOS Sans SC 约 22 MB，3500 字 woff2 约 1.5-1.8 MB
- **Decision**: 切 3500 字 + ASCII 标点 + 全角标点
- **Consequences**: 罕见字回退到系统字体（栈中第二位的 `Microsoft YaHei UI` 等）；UX 几乎无损（聊天 UI + 代码场景 99.9% 覆盖）

### Decision 3: 用 woff2 + 3500 字硬编码
- **Context**: woff1 / OTF / TTF 都不如 woff2 紧凑；动态字表需要后端配合
- **Decision**: woff2 一次性硬编码字表
- **Consequences**: 用户偶尔遇到罕见字会"掉字"（如 LLM 输出不常见人名），但可通过 unicode-range 让浏览器自动 fallback，UX 仍可接受

## Technical Notes

- 改动 / 新增文件:
  - `app/src/assets/fonts/HarmonyOSSansSC-Regular.subset.woff2` (新, ~1.5 MB)
  - `app/src/assets/fonts/source/HarmonyOSSansSC-Regular.ttf` (新, ~22 MB, 可选保留)
  - `app/src/style.css` (改, 顶部加 @font-face)
  - `app/scripts/subset-font.py` (新, 可复现的子集化脚本)
- 不需要改: `package.json` / `vite.config` / `tauri.conf.json` / 任何 Vue 组件
- 关联: `app/src/style.css` 现有 `--font-sans` 栈第一项已是 `"HarmonyOS Sans SC"`，无需调整
- Rollback: 删 woff2 + 删 `@font-face` block + 删脚本即可

## Research References

- HarmonyOS Sans SC 官方: https://developer.huawei.com/consumer/cn/design/harmonyos-font
- fonttools subset 文档: https://fonttools.readthedocs.io/en/latest/subset/
- 3500 常用字参考: https://github.com/elkmovie/most-common-chinese-characters
