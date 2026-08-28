# Everlasting 品牌标识

> 2026-08-15 设计,2026-08-28 v2 质感升级(几何不变)。打开 `index.html` 可看全部静态变体,
> `motion/index.html` 看 9 款动效 + 2 款浅底动效的实拍 QA 页。

## 概念

**互锁双环 + 中心 spark**:

- 两条圆弧互相穿过对方环体再停下 —— 真正的链环互锁(不是相切或叠放),象征
  人与 agent、agent 与 agent 的协作关系(群聊多参与者编排是这个项目的主线)。
- 环不闭合、循环往复 —— 对话持续、session 永续,呼应 "Everlasting"。
- 中心一枚白色圆点 —— 循环中的人 / 项目 / 灵感;也和大弧线的圆头端点形成
  形状上的一致(全图形只有圆弧和圆)。
- 双色取自 app 工具色体系:青 = 读/知识(`--color-tool-read` 系),琥珀 =
  执行/活动(`--color-tool-shell` 系)。深底用 400 级,浅底用 600 级。

## v2 质感升级(2026-08-28,几何零改动)

1. **顶部受光渐变**:两条弧各挂一条 `userSpaceOnUse` 竖向渐变(y 122→392),
   青 `#67e8f9→#22d3ee→#06b6d4`(300/400/500),琥珀 `#fcd34d→#fbbf24→#f59e0b`;
   浅底版用 500→600→700 级。光源统一来自正上方,双环立体感来自同一套光照。
2. **交叉投影**:沿琥珀路径画一条加宽(52)+模糊(blur 5)+ 45% 透明度的深色描边,
   用 `<mask>` 限定**只落在青色笔画带内** —— 琥珀压过青的穿插处出现 AO 阴影,
   链环"穿过去"的错觉成立;任何底色上都干净(不会沿整条琥珀拖灰尾巴)。
3. **spark 辉光**:中心点(r13)外加大半径 40 的青调径向 halo(`#a5f3fc`)。
   motion 版的 halo 都跟随各自 spark 的动画节奏(呼吸/心跳/暴胀/明灭)。

SVG 内部 id 固定为 `gc/ga/gl/soft/onCyan`。**内联进 HTML 且同页多实例时**要给 id
加实例前缀防冲突;经 `<img>`/data-URI 引用则无需处理。

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
**每个端帽周围留 ≥ 半个描边宽的负空间**(不糊)。上色顺序固定:青先、投影、
琥珀后,交叉处琥珀在上,织感一致。

## 调色板

| 用途 | 值 | 说明 |
|---|---|---|
| 深色底(主) | 渐变 `#67e8f9→#22d3ee→#06b6d4` / `#fcd34d→#fbbf24→#f59e0b`,点 `#f8fafc` | cyan/amber 300→500 竖向渐变 |
| 浅色底 | 渐变 `#06b6d4→#0891b2→#0e7490` / `#f59e0b→#d97706→#b45309`,点 `#1e293b` | 500→600→700 级保证对比度 |
| 单色 | `currentColor`(见 logo-mark-mono.svg,纯平色不加渐变) | UI chrome 内嵌用 |
| App icon 底 | `#0a0e14` = `--color-bg-app` | 微径向光 #1c2a42→#0a0e14 |

## 文件

| 文件 | 用途 |
|---|---|
| `logo-mark.svg` | 主标识,透明底,深色表面用(v2 全效果) |
| `logo-mark-light.svg` | 浅色表面变体;`scripts/export-icons.mjs` 会复用其 `<g>`(defs 必须留在首个 `<g>` 内) |
| `logo-mark-mono.svg` | 单色 currentColor 版(纯平色,小尺寸最干净) |
| `logo-app-icon.svg` | squircle 应用图标(rx≈22.3%),mark 74%(v2: 66%→74% 提升方形容器存在感) |
| `logo-app-icon-maskable.svg` | 全出血 + mark 60%(v2: 50%→60%;半对角 153.7 < PWA 安全区半径 204.8) |
| `logo-wordmark.svg` | 横排 lockup。2026-08-28 起**文字已转曲**(源:`app/src/assets/fonts/source/HarmonyOSSansSC-Regular.ttf`,opentype.js,字号 128 / 字距 6 / 基线 y=197),对外可直接用 |
| `logo-favicon.svg` | ≤24px 专用:去中心点、描边加粗到 48,**保持纯平色**(小尺寸渐变无意义) |
| `og-image.svg` → `png/og-image-1200.png` | 社交卡 1200×630(浅径向光 + 居中 lockup) |
| `png/` | Playwright 导出的常用位图 |
| `motion/` | 13 款 SMIL 动态 logo(深底,三批)+ `logo-flow/evolve/aurora-light` 三款浅底变体;QA 页 `motion/index.html` |

## 尺寸使用阶梯

- **≥32px**:`logo-mark.svg` 原样使用(含中心点)。
- **16–24px**:必须换 `logo-favicon.svg`(实测 16px 下含点版会糊成一团)。
- **PWA / 启动器**:maskable 版;安全区规则已按圆/方/圆角矩形 mask 留好。

## 落地到 app(已执行,改动后需重跑)

1. **PWA/浏览器**(`app/public/icons/`):
   `CHROME_EXE=<chromium> node scripts/export-icons.mjs /tmp/pwa-icons`,
   产物按名拷贝:`app-icon-192-light.png→192.png`、`app-icon-512-light.png→512.png`、
   `app-icon-512-maskable-light.png→512-maskable.png`、`favicon-32-light.png→favicon-32.png`。
   这些是**浅底深线**版(2026-08-24 1f72d5e:国产 Launcher 深底图标隐形);`icon.svg`/
   `icon-maskable.svg` 为对应 SVG fallback。playwright-core 从 `out/logo-work/node_modules`
   回退解析,无需额外安装。
2. **Tauri 桌面**:`cd app && pnpm tauri icon ../brand/png/logo-app-icon-1024.png`
   —— 整体重写 `app/src-tauri/icons/`(深底 squircle 版);生成的 `icons/android|ios/`
   子目录在项目未初始化移动端时删掉即可。
3. **SVG 导出位图**:仓库无 rsvg/inkscape,用 `out/logo-work/` 下脚本
   (playwright-core + `~/.cache/ms-playwright` Chromium);注意 SVG 必须 data-URI 内嵌
   (`<img src="file://…">` 在 setContent 页面会被拦,详见 export.mjs 注释)。
4. **GitHub 社交卡**:仓库 Settings → Social preview 上传 `brand/png/og-image-1200.png`。
