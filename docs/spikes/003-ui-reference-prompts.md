# spike-003: UI 参考图生图提示词(Midjourney v6)

**日期**: 2026-06-06
**状态**: 草稿(未实际跑过 Midjourney,提示词待实际生成后迭代)
**目标**: 用生图模型生成 3 张 Figma 风格参考图,作为前端重做的视觉锚点
**前置依赖**: 无

## 决策(从 10 题脑暴收敛)

1. 风格: Linear × Cursor 深色
2. 信息架构: 顶项目 Tab + 左侧 session 栏 + 主区 + 输入框(3b-1 已落地)
3. 产出: A=1 张 hero 锚点 / B=2 张(B-1 项目 Dialog + B-2 权限 Dialog)
4. 模型: Midjourney v6,`--style raw`
5. 色板: 现代普鲁士蓝 `#3B5BDB` 主色 + 青/翡翠/琥珀/朱砂/紫罗兰状态色
6. 字体: Noto Sans CJK SC(UI)+ JetBrains Mono(代码)
7. 画布: 16:10(1440×900,匹配 tauri.conf.json 即将调整的窗口)
8. 写实度: Figma 截图(无渐变、无阴影、无美化)
9. Hero 强调: 顶部 Tab + tool call 卡片 + 流式光标
10. 文本: 中英混排(UI 中文、代码/路径英文)

## 色板 token(在所有提示词里复用)

| 用途 | hex |
|---|---|
| bg / surface / elevated / border | `#0A0E14` / `#131822` / `#1A2030` / `#1E2530` |
| text primary / secondary / muted | `#E5E7EB` / `#8B95A7` / `#64748B` |
| accent(主蓝/hover/选中 bg) | `#3B5BDB` / `#4263EB` / `#1E2A5E` |
| read / write / shell / error / thinking | `#06B6D4` / `#10B981` / `#F59E0B` / `#EF4444` / `#A78BFA` |

## 提示词 A — Hero 锚点(主 chat 界面)

(每段均 ≤2000 字符,可直接复制到 Midjourney)

```
A Figma mockup screenshot of a dark-mode AI coding workbench desktop app, 1440x900. Linear-meets-Cursor aesthetic, modern Prussian blue accent. Sharp edges, flat shading, no shadows, no reflections, no gradients. Mixed Chinese-English UI text. Noto Sans CJK SC for UI, JetBrains Mono for code. Layout: top 40px project tabs bar, left 260px session sidebar, center 1100px main chat, bottom 96px input. Top bar: 4 tabs. Active "everlasting" with 2px Prussian blue underline. Others "blog" "spike" "experiments". Right: plus icon and "⌘K" hint. Sidebar: uppercase header "会话 SESSIONS" muted. 5 session items 56px each. First active with subtle Prussian tint. Each: title CN+EN, status dot, timestamp. Examples "添加图片上传功能" green dot, "修复 sidebar 闪烁" amber dot, "step 3b-1 PR review" green dot. Bottom: tiny avatar + gear. Main area: top session title and "⎇ main" branch label. Chat region: (1) user message right-aligned surface bg showing CN "请在 ChatWindow.vue 加图片上传功能，支持拖拽和点击"; (2) assistant streaming with 3 lines visible ending in blinking Prussian blue cursor; (3) two stacked tool cards. Card 1 read_file 3px cyan left border, header mono "read_file · src/components/ChatWindow.vue", body syntax-highlighted code preview with subtle fade-to-transparent at bottom, footer "✓ 读取完成 · 14.2 KB". Card 2 write_file 3px emerald border, header "write_file · src/components/ChatWindow.vue", body small diff red minus green plus, footer "✓ 写入完成". Input: multi-line 80px rounded 8px surface bg, placeholder "问点什么，或输入 / 调出命令...". Right: Prussian blue circular send button. Below: small muted hint "⏎ 发送 · ⇧⏎ 换行 · @ 引用文件 · / 命令". Color tokens: bg #0A0E14, surface #131822, elevated #1A2030, border #1E2530, text #E5E7EB, secondary #8B95A7, muted #64748B, accent #3B5BDB, read #06B6D4, write #10B981, shell #F59E0B, error #EF4444, thinking #A78BFA. --ar 16:10 --style raw --s 30 --v 6 --no photo, 3d, illustration, blur, glow, neon, gradient, watermark, hands, fingers, low quality
```

## 提示词 B-1 — 项目选择 Dialog

```
A Figma mockup screenshot of a dark-mode project picker dialog overlay, 1440x900. Prussian blue accent, Linear-meets-Cursor aesthetic, sharp edges, flat shading, no shadows. Mixed Chinese-English text. Noto Sans CJK SC for UI, JetBrains Mono for paths. Composition: 600x520 modal centered, 1px border #1E2530, elevated bg #1A2030. Behind: parent app darkened 60% slightly blurred. Dialog: (1) title "选择项目 / Open Project" 16px semibold, "ESC" hint right. (2) Search input full-width 40px rounded 6px surface bg #131822, magnifying glass left, placeholder "搜索项目名或路径..." muted. (3) Section header "最近项目 / Recent Projects" 11px uppercase muted. (4) 4 recent project items 56px each. Each: small folder icon Prussian blue, name semibold "everlasting", subtext muted mono "/usr/local/code/github/everlasting", right timestamp "2 小时前". Third item hovered with subtle Prussian tint. (5) Horizontal divider 1px #1E2530. (6) Two action rows 48px each: "📁 打开本地目录..." subtext "从文件系统选择项目根目录", "✏️ 手动输入路径..." subtext "粘贴绝对路径". Color tokens: bg #0A0E14, surface #131822, elevated #1A2030, border #1E2530, text #E5E7EB, secondary #8B95A7, muted #64748B, accent #3B5BDB, accent-muted #1E2A5E. --ar 16:10 --style raw --s 30 --v 6 --no photo, 3d, illustration, glow, neon, gradient, watermark, hands, fingers, low quality
```

## 提示词 B-2 — 权限确认 Dialog

```
A Figma mockup screenshot of a dark-mode AI agent permission dialog, 1440x900. Prussian blue accent, Linear-meets-Cursor aesthetic, sharp edges, flat shading, no shadows. Mixed Chinese-English text. Noto Sans CJK SC for UI, JetBrains Mono for commands. Composition: 480x340 modal centered horizontally, slightly above center. 1px border #1E2530, elevated bg #1A2030. Behind: parent chat darkened 60%. Dialog: (1) 56x56 rounded icon container top-left, bg #1E2A5E Prussian tint, shield icon Prussian blue. (2) Title "需要你的确认" 16px semibold top-right of icon. Right close muted. (3) Subtitle 14px "Agent 想在项目 everlasting 下执行以下操作:" #8B95A7. (4) Command preview block rounded 8px surface bg #131822, mono 13px "$ npm test -- --watchAll=false", terminal icon left, copy icon right. (5) Note muted "工具类别: shell · 风险等级: 中" with amber #F59E0B dot before "风险等级". (6) Bottom-right buttons: secondary "拒绝" transparent bg 1px border #1E2530 text #8B95A7 rounded 8px 32px tall 80px wide. Primary "允许" Prussian blue #3B5BDB bg white text same size. (7) Small "本次会话记住此选择" with unchecked checkbox 12px muted. Color tokens: bg #0A0E14, surface #131822, elevated #1A2030, border #1E2530, text #E5E7EB, secondary #8B95A7, muted #64748B, accent #3B5BDB, accent-muted #1E2A5E, shell amber #F59E0B. --ar 16:10 --style raw --s 30 --v 6 --no photo, 3d, illustration, glow, neon, gradient, watermark, hands, fingers, low quality
```

## 调试建议(待实际跑过再补)

- **首轮跑 A**: 用 Vary (Subtle) 出 4 张,选 1 张最贴近的做基准
- **CJK 渲染不稳**: 把所有中文行替换成英文占位,拿到骨架再用 Figma 加文字
- **风格化太重**: 把 `--s 30` 降到 `--s 0`,并去掉 `Linear-meets-Cursor` 这类感性词
- **拿到 1 张满意的 A 后**: 复制为 B-1/B-2 的 base,只改"主体描述"段——三张图色板/字体/质感一致
- **B-1 / B-2 单独跑**: 不要 seed 关联,让 MJ 各自发挥

## 已知未决(实战后再回填)

- [ ] CJK 字符在 MJ v6 渲染的稳定性
- [ ] `Linear-meets-Cursor` 关键词是否需要更具体(贴 reference 图链接?)
- [ ] `--s 30` 是否过低导致太"素"
- [ ] 是否需要补充 `--q 2`(高质量档)拉满细节
