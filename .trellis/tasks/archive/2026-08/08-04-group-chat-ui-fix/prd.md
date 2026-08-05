# 修复群聊 UI 三个问题：入口图标缺失、新建群聊下拉 option 不可见、modal 滚动应限定内容区

## Goal

修复群聊功能（07-29-group-chat Phase 4 Step 3）上线后用户反馈的三个前端问题。
三个问题相互独立、定位已明确，均为单文件或小范围 CSS/注册修复，无数据流/后端改动。

## Background / 根因

### ① 群聊入口按钮看不到图标
- `app/src/components/layout/Sidebar.vue:142` 用了 `<Icon name="users" />`，
  但 `app/src/components/Icon.vue` 的图标 `map` 注册表（75–160 行）**没有 `"users"` key**。
- `map["users"]` 为 `undefined` → `Component` 为 `null` → `v-if="Component"` 渲染空 `<span>`，
  按钮框仍在、可点击，但无 SVG，看起来"没图标"。
- 同一处 `<Icon name="users" />` 也用在 `ChatPanel.vue:627`（编辑参与者 chip）。
- 图标本身存在于 `@heroicons/vue/24/outline`（`UsersIcon`、`UserGroupIcon`），
  只是没 import + 注册。`Icon.vue` 头注释（8–10 行）记录了新增图标的两步流程。

### ② "新建群聊" modal 模型下拉看不到 option
- `GroupChatConfigModal.vue` 的 Select 样式（`.gcfg-select-content` /
  `.gcfg-select-item`，530–546 行）写成普通 `<style scoped>`，**没用 `:deep()`**。
- 这正是仓库记录的头号 Select 可见性坑：portal 子元素（teleport 到 `<body>`）
  在 scoped 下需要 `:deep()` 才稳定命中。
  - 权威文档：`.trellis/spec/frontend/reka-ui-usage.md`（184–379 行）
  - 次级文档：`.trellis/spec/frontend/popover-pattern.md`（575–601 行 overflow 裁剪坑）
- 当前还缺 `position="popper"` 与正确的 z-index 分层（现 `10001` 仅比 dialog `10000` 高 1）。
- 正确范例：`app/src/components/memory/RuntimeMemoryModal.vue`
  （Select 用 `position="popper"`、CSS 全包 `:deep()`、z-index `3000 !important`、
  dialog `2001`、`--reka-select-trigger-width`）。

### ③ modal 内容超高时连标题一起滚动
- 现状：`.gcfg-content`（DialogContent 本身）`max-height: 88vh; overflow-y: auto`，
  标题 + 副标题 + 内容 + footer 全在一个滚动容器里，超高时 header 也被滚走。
- 期望：标题（+副标题/错误条）固定在顶部，只有中间参与者列表区域滚动。
- 参照 `RuntimeMemoryModal.vue` 的安全做法：dialog 用 `display:flex; flex-direction:column;
  overflow:hidden` + 一个 `flex:1; overflow-y:auto; min-height:0` 的 body 区。

## Requirements

### R1 — 修复入口图标（Icon 注册）
- R1.1 在 `Icon.vue` 给 `@heroicons/vue/24/outline` 的 import 块新增一个群聊图标
  （`UserGroupIcon` 语义更贴"群聊"，或 `UsersIcon`）。
- R1.2 在 `map` 注册表新增 `"users"` key 指向该图标。
- R1.3 不改 `Sidebar.vue` / `ChatPanel.vue` 的调用处（它们已用 `name="users"`），
  注册表补齐即两处同时恢复显示。

### R2 — 修复 Select option 不可见（对齐仓库既有规范）
- R2.1 `SelectContent` 增加 `position="popper"`（与 RuntimeMemoryModal 一致，
  避免绝对定位偏移导致的裁剪）。
- R2.2 把 `.gcfg-select-content` / `.gcfg-select-item`（及相关 `[data-highlighted]`）
  改成 `:deep(...)` 写法。
- R2.3 SelectContent z-index 抬到 `3000`（高于 dialog）；同时把 dialog overlay/content
  的 z-index 下调到与 RuntimeMemoryModal 同一基线（overlay `2000` / content `2001`），
  留足分层空间。（注：此 modal 与 RuntimeMemoryModal 不会同时打开，z-index 基线对齐无冲突。）
- R2.4 SelectContent 加 `--reka-select-trigger-width` 宽度约束（按 trigger 宽度展开）。

### R3 — 滚动限定到内容区
- R3.1 `.gcfg-content`（DialogContent）改成 flex 列容器：
  `display:flex; flex-direction:column; overflow:hidden`，去掉 `overflow-y:auto`。
- R3.2 新增一个滚动 body 容器包裹"错误条 + 参与者列表 + 添加按钮"，
  设 `flex:1; overflow-y:auto; min-height:0`。
- R3.3 标题、副标题、footer 不进入滚动区，保持固定。
- R3.4 保留 `max-height: 88vh` 整体上限（避免超高时顶满视口）。

## Non-Goals / 范围外

- 不改参与者校验逻辑、提交流程、IPC。
- 不重写整个 modal 的视觉风格，仅修滚动结构 + Select 样式。
- 不调整 reka-ui 版本或引入新依赖。

## Acceptance Criteria

- [ ] **AC1**：侧边栏"新建群聊"按钮（`data-testid="sidebar-new-group-chat"`）
  渲染出一个可见的群聊 SVG 图标，不再是空白按钮。
- [ ] **AC2**：群聊会话头部"编辑参与者" chip（`ChatPanel.vue`）的 `users` 图标同样可见。
- [ ] **AC3**：打开"新建群聊"modal，点击任一参与者"模型"下拉，
  option 列表可见、带背景/边框/高亮，宽度贴合 trigger。
- [ ] **AC4**：把参与者列表撑到超高（≥3 个参与者 + 长 persona），
  滚动时标题/副标题/footer 保持固定，仅中间内容区滚动。
- [ ] **AC5**：现有测试 `GroupChatConfigModal.test.ts` 全绿
  （DOM 结构调整后这些基于 testid/class 的断言仍需通过；必要时同步调整测试选择器）。
- [ ] **AC6**：`Icon.vue` 新增图标后，前端 lint + typecheck 通过。
- [ ] **AC7**：改动符合 `.trellis/spec/frontend/reka-ui-usage.md` 的 `:deep()` + z-index 规范。

## Notes

- 轻量任务，PRD-only。
- 实现要点已在 Background 写清，无需额外 design.md。
- 提交信息沿用 `fix(group-chat): 中文描述` 风格（见最近 commit）。
