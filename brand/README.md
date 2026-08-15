# Everlasting 品牌标识

> 2026-08-15 设计。打开 `index.html` 可看全部变体的实拍 QA 页。

## 概念

**互锁双环 + 中心 spark**:

- 两条圆弧互相穿过对方环体再停下 —— 真正的链环互锁(不是相切或叠放),象征
  人与 agent、agent 与 agent 的协作关系(群聊多参与者编排是这个项目的主线)。
- 环不闭合、循环往复 —— 对话持续、session 永续,呼应 "Everlasting"。
- 中心一枚白色圆点 —— 循环中的人 / 项目 / 灵感;也和大弧线的圆头端点形成
  形状上的一致(全图形只有圆弧和圆)。
- 双色取自 app 工具色体系:青 = 读/知识(`--color-tool-read` 系),琥珀 =
  执行/活动(`--color-tool-shell` 系)。深底用 400 级,浅底用 600 级。

## 几何(master 坐标,viewBox 512)

| 参数 | 值 |
|---|---|
| 半径 R / 描边 | 118 / 30,圆头端点 |
| 圆心 | 左环 (170,256),右环 (342,256) |
| 开口半角 β | 20°(弧端点 (280.88, 215.64) 与镜像) |
| 穿插交点 | (256, 175.2) / (256, 336.8) |
| 端点帽与对方环内缘间隙 | ≈15 单位(帽心距对方圆心 73.2 < 内缘 103) |
| 外接盒 | x 37–475,y 123–389(438×266,≈1.65:1) |

改动几何时保持两条不变量:**弧必须穿过对方笔画带再停在内孔里**(互锁成立),
**每个端帽周围留 ≥ 半个描边宽的负空间**(不糊)。上色顺序固定:青先、琥珀后,
交叉处琥珀在上,织感一致。

## 调色板

| 用途 | 值 | 说明 |
|---|---|---|
| 深色底(主) | `#22d3ee` / `#fbbf24` / 点 `#f8fafc` | cyan-400 / amber-400 系,贴近工具色 |
| 浅色底 | `#0891b2` / `#d97706` / 点 `#1e293b` | 600 级保证对比度 |
| 单色 | `currentColor`(见 logo-mark-mono.svg) | UI chrome 内嵌用 |
| App icon 底 | `#0a0e14` = `--color-bg-app` | 微径向光 #1c2a42→#0a0e14 |

## 文件

| 文件 | 用途 |
|---|---|
| `logo-mark.svg` | 主标识,透明底,深色表面用 |
| `logo-mark-light.svg` | 浅色表面变体 |
| `logo-mark-mono.svg` | 单色 currentColor 版 |
| `logo-app-icon.svg` | squircle 应用图标(rx≈22.3%) |
| `logo-app-icon-maskable.svg` | 全出血 + mark 50%,PWA maskable |
| `logo-wordmark.svg` | 横排 lockup(HarmonyOS Sans SC 活字,对外使用前请转曲) |
| `logo-favicon.svg` | ≤24px 专用:去中心点、描边加粗到 48 |
| `png/` | Playwright 导出的常用位图 |

## 尺寸使用阶梯

- **≥32px**:`logo-mark.svg` 原样使用(含中心点)。
- **16–24px**:必须换 `logo-favicon.svg`(实测 16px 下含点版会糊成一团)。
- **PWA / 启动器**:maskable 版;安全区规则已按 20% 圆角留好。

## 落地到 app(待执行)

1. favicon:`app/public/icons/icon.svg`(目前还是占位字母 "E")与
   `icon-maskable.svg` 替换为本目录对应文件;`index.html` 加 PNG fallback
   (Safari 不吃 SVG favicon):`png/logo-favicon-32.png`。
2. Tauri 图标:`pnpm tauri icon brand/png/logo-app-icon-1024.png`
   —— 会整体重写 `app/src-tauri/icons/`,覆盖现有自定义图标前先确认。
3. SVG 导出位图:仓库无 rsvg/inkscape,用
   `out/logo-work/export.mjs`(playwright-core + `~/.cache/ms-playwright` Chromium)。
