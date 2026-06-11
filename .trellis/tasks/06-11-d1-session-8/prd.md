# D1: Session 重命名 + 8 色标记

## 背景

Session 列表当前只有自动标题（首条 user message 前 50 字），用户无法手动修改。也没有视觉分类手段，当 session 数量多时难以快速定位目标对话。

## 目标

1. 用户可手动重命名 session 标题
2. 用户可用 8 色调色板给 session 打颜色标记，方便视觉聚焦

## 决策记录

### D1: 手动重命名锁定

- 一旦用户手动设置过标题，自动标题逻辑（`persist_turn` 中 `title = '新对话'` 时覆盖）永不再触发
- 实现方式：新增 `title_source` 标记列（可选，见实现细节），或在 `rename_session` 时将标题改为非"新对话"的值即可利用现有逻辑
- **颜色标记与标题独立**：给 session 标颜色不影响自动标题触发

### D2: 8 色低饱和度预设调色板

- 固定 8 色，低饱和度，不刺眼但能区分
- DB 存 `color_tag INTEGER DEFAULT NULL`（0-7 索引），前端维护调色板数组
- `NULL` = 无标记，视觉与现在完全一致

### D3: 视觉呈现

- **SessionList 卡片**（inactive 状态）：`color_tag` 非 NULL 时，卡片背景加标记色 10% 透明度底色
- **ChatInput**：`chat-input__row` 加标记色 5% 透明度底色
- **Active 状态**：保持现有样式不变（`--color-accent-muted` 背景 + `--color-accent` 左边框）
- dot 圆点保持不变（继续表示 active/inactive + streaming）

### D4: 交互入口

- **双击标题** → 行内编辑（input 框，maxlength=80，Enter 提交，Esc 取消）
- **右键菜单**（reka-ui DropdownMenu）：
  - "重命名"：触发行内编辑
  - "标记颜色"：子菜单展开 8 色调色板 + "取消标记"选项
  - "删除"：与现有 x 按钮行为一致（confirm 弹窗）
- **x 按钮保留**：hover 显示，快速删除

### D5: 后端 API

- 新增 `rename_session(session_id, new_title)` Tauri command
  - `UPDATE sessions SET title = ?, updated_at = ? WHERE id = ?`
  - 标题长度服务端也做校验（截断 80 字符）
- 新增 `set_session_color(session_id, color_tag)` Tauri command
  - `color_tag` 接收 0-7 或 null（空字符串 → NULL）
  - `UPDATE sessions SET color_tag = ?, updated_at = ? WHERE id = ?`
- 两个专用 command，不复用通用 update

### D6: 标题长度

- 自动标题：保持 50 字符截断（不变）
- 手动输入：`maxlength=80`
- CSS `text-overflow: ellipsis` 兜底显示截断

## 变更清单

### DB 层

- `migrations.rs`：新增 `color_tag INTEGER` 列到 sessions 表
- `types.rs`：`SessionRow` + `SessionSummary` 加 `color_tag: Option<i32>`

### Rust CRUD

- `sessions.rs`：新增 `rename_session()` + `set_session_color()`
- `sessions.rs`：`list_sessions` / `load_session` 的 SELECT 补 `color_tag` 列
- `sessions.rs`：`create_session` 的 INSERT 补 `color_tag` 列（默认 NULL）

### Rust Commands

- `commands/sessions.rs`（或 `commands/mod.rs`）：新增两个 `#[tauri::command]`
- `lib.rs`：注册新 command

### 前端 Store

- `stores/chat.ts`：新增 `renameSession(id, title)` + `setSessionColor(id, colorTag)` action
- `SessionSummary` type 加 `color_tag: number | null`

### 前端组件

- `SessionList.vue`：
  - 右键菜单（reka-ui DropdownMenu）：重命名 / 标记颜色(子菜单) / 删除
  - 双击标题行内编辑
  - inactive 卡片条件底色
- `ChatInput.vue`：`chat-input__row` 条件底色（5%）
- 新建 `utils/colorTag.ts`：8 色调色板常量 + 索引→色值映射

## 不做

- 不做"恢复自动标题"选项
- 不做颜色筛选/过滤（未来 D2 档可能加）
- 不改 active 状态的视觉样式
- 不加 color picker 自由选色
