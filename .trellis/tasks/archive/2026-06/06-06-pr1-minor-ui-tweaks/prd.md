# PR1: 圆点+header 高度/字号 微调 + 接入 simplifiedCwd (chip pwd)

> Source spike: [`docs/spikes/2026-06-06-feature-requests.md`](../../../../../docs/spikes/2026-06-06-feature-requests.md) 第 1 条 + 第 2a/2b 子项 + 接入 PR3 的 `simplifiedCwd`
> 父 task: `06-06-spike-005-follow-up`
> 父 prd: [../06-06-spike-005-follow-up/prd.md](../06-06-spike-005-follow-up/prd.md) (PR1 段)
> Priority: P2 (纯 CSS 微调 + 接入 PR3 数据, ~15-30 行)

## Goal

合并 spike 第 1 条 (sessions 状态圆点) + 第 2a/2b 子项 (header 高度/字号) + 接入 PR3 的 `chatStore.simplifiedCwd` 在 header 右远端显示简化 pwd。
- sessions 状态圆点: 放最左侧, size 8px
- chat panel header 高度: 28px (PR2 已经在该位置加了 git chip, 高度由 14px padding 改成 6px 上下)
- session title 字体: 15px → 13px
- header 右远端: 显示 `chatStore.simplifiedCwd` (PR3 准备的数据)

## What I already know

- `app/src/components/chat/ChatPanel.vue:115` `padding: 14px 20px` 当前 header padding (高度 ~42px)
- `ChatPanel.vue:130-139` `.chat-panel__title` 当前 `font-size: 15px`, `font-weight: 600`
- PR2 已加 git chip (`ChatPanel.vue:72-75`), 显示分支名
- PR3 已加 `chatStore.simplifiedCwd` computed
- 父 prd 要求 header 高度 28px: 14px padding + 14px content = 28px ✓ (但需要 1 行文字 fit)
- sessions 状态圆点位置需要先看 sidebar 组件 (不在 ChatPanel 里, 在 sidebar)
- 圆点 size 8px 是 CSS 一行: `width: 8px; height: 8px;`

## Requirements

### 1. sessions 状态圆点
- 找到 sessions sidebar 组件 (可能叫 `SessionList.vue` 或 `EmptyProjectState.vue` 旁边)
- 圆点: `width: 8px; height: 8px;` (原 size 不明, spike 没说, 假设原 6px → 改 8px)
- 圆点位置: `margin-right` 调成 `order: -1` 之类, 放最左侧
- 圆点颜色: 保持现状 (绿色 = active streaming, 灰 = idle)

### 2. chat panel header 高度 28px
- `ChatPanel.vue:115` `padding: 14px 20px` → `padding: 6px 20px`
- 内容高度 (chip + title 13px line-height ~20px) = ~20-22px, 6+20+6 = ~32px, 接近 28px
- 接受: 28px 是目标但实际 ~30-32px 是 padding+content 的物理约束
- 或者更激进: `padding: 4px 20px` + `font-size: 12px` (让 chip 也变小)

### 3. session title 字体变小
- `ChatPanel.vue:132` `font-size: 15px` → `font-size: 13px`
- `font-weight: 600` 保持 (不要同时改 weight, 单独改 size)

### 4. header 接入 simplifiedCwd (PR3 数据)
- `ChatPanel.vue` template 在 `.chat-panel__title-row` 末尾加:
  ```vue
  <span class="chat-panel__chip chat-panel__chip--cwd">
    {{ chatStore.simplifiedCwd }}
  </span>
  ```
- CSS: 新 `.chat-panel__chip--cwd` 用次要色, 等宽字体 (跟 git chip 一致风格)
- 远端对齐: `chat-panel__title-row` 已有 `flex-wrap: wrap`, 加 `margin-left: auto` 让 pwd 推到右远端; 或改用 `space-between` 在更大容器上

## Acceptance Criteria

- [ ] sessions 状态圆点视觉确认: 大小 8px, 位置最左
- [ ] header 高度实测 ~28-32px (不再 42px)
- [ ] session title 字体缩小 (15px → 13px)
- [ ] header 远端显示 `~/...` 简化 pwd (来自 PR3)
- [ ] pwd 不在 home 下时显示全路径 (跟 PR3 行为一致)
- [ ] pnpm build + pnpm test (PR6/PR3 regression) 双过
- [ ] cargo test (PR2/5 regression) 通过
- [ ] 视觉无回归 (markdown 渲染 / cancel button / git branch chip 都正常)

## Definition of Done

- 修改 ~2-3 个文件
- 跑完 standard Trellis 流程到 archived
- 视觉验证: Tauri 启动, 看到 header 紧凑 + pwd 简化

## Out of Scope

- pwd 复制按钮 / 跳转 (BACKLOG 候选, 不在本 PR)
- 多 pwd 历史 (e.g. breadcrumb) — v1 只显示当前
- pwd 长度截断 (e.g. 超长路径截中间 `~/co.../backend`) — v1 不处理, 靠 flex-wrap 折行
- 圆点 hover tooltip (e.g. "Streaming 5s") — v1 不做
- 圆点 pulse 动画 — v1 静态

## Technical Notes

- 改动文件:
  - `app/src/components/chat/ChatPanel.vue` (header 高度/字号 + pwd chip)
  - `app/src/components/chat/SessionList.vue` 或 sidebar 组件 (圆点位置 + size)
  - 可能: 新 `.chat-panel__chip--cwd` 样式
- 风险: header 太挤 (28px 装 chip + title + pwd + 高度) — 接受 trade-off
- 风险: pwd 超长路径 flex-wrap 折行可能视觉乱 — 接受 v1, v2 加 truncation
- 风险: 圆点 order 调整可能影响其他 sidebar item 布局 — 单独看 sidebar 组件
- 关联: PR2 已经在 header 加 git chip, PR1 复用 `.chat-panel__chip` 样式类
- 关联: PR3 的 `chatStore.simplifiedCwd` 是本 PR 数据源, PR3 已 archived, 直接消费

## Decision (ADR-lite)

- **决策 1**: header 目标 28px, 实际 padding 6px+6px + content 13-15px line-height, 物理 ~28-32px
  - **理由**: 用户要求 28px 是 visual target, 物理约束允许小范围浮动
  - **后果**: 实际可能 30-32px, 视觉上 28px 紧凑感达成
- **决策 2**: pwd 放最右远端 (`margin-left: auto`), 不在 title-row 末尾
  - **理由**: 远端视觉重心稳, 跟 VSCode / JetBrains 风格一致
  - **后果**: title-row 需要 `flex` 容器, 已有 `inline-flex`, 改为 `flex` + `margin-left: auto` on pwd chip
- **决策 3**: 圆点 order 改 -1 (放最左)
  - **理由**: 不动 DOM 结构, 仅 CSS order, 副作用最小
  - **后果**: 屏幕阅读器顺序不变, 视觉顺序变, 可访问性 OK
