# 修复：关闭项目后无法重新打开(create_project already exists)

## Goal

修复用户关闭项目后无路重新打开的 bug。当前唯一路径走"添加项目"对话框 → 后端 `create_project` 撞 SQLite UNIQUE 约束报错 `a project with path '...' already exists`。

修复 3 条契约:
1. **`addProject()` 命中 hidden 路径 → 自动 unhide + focus**(核心)
2. **`EmptyProjectState` onMounted 自动 loadHiddenProjects**(首屏列表可见,无需额外点击)
3. **主 UI 暴露隐藏项目入口**(`HiddenProjectsMenu` popover,多项目场景可恢复单个)

---

## What I already know

### 用户反馈(2026-06-23)

报错:
```
添加项目失败:create_project failed:encountered unexpected or invalid data:
a project with path '/usr/local/code/github/everlasting' already exists
```

### 代码调研发现(已 Read + grep)

- `app/src/stores/projects.ts:147 addProject()` 路径检查**只覆盖 `projects.value`**(visible):
  ```ts
  const existing = projects.value.find((p) => p.path === picked);
  ```
  → hidden 项目不命中 → 继续调 `create_project` → 后端 `db/projects.rs:16-61 create_project`:
  ```rust
  Err(sqlx::Error::Database(db)) if db.is_unique_violation() => Err(sqlx::Error::Protocol(
    format!("a project with path '{}' already exists", path)
  ))
  ```
  → 错误冒泡 → IPC `String` → 前端 toast。

- `EmptyProjectState.vue` 是唯一渲染 `hiddenProjects` 的组件,但**只在 `currentProjectId === null` 时挂载**(`ChatWindow.vue:66-68`)。
- 且 `hiddenProjects` 是 lazy-loaded:首屏只显"查看最近隐藏的项目"按钮,需点一下才 list。
- `projects.ts:217 unhideProject()` 已存在完整可用流程(`invoke unhide_project` + `loadHiddenProjects` + `loadProjects` + `currentProjectId = fresh.id`),无 bug,直接复用。
- `commands::projects::unhide_project` 已注册(`lib.rs:127`),后端 OK。
- 没有 `projects.test.ts`(前端 store 无测试),新增。

### 项目隐藏机制

- `projects` 表 `hidden INTEGER`(0/1),`hide_project` 设 1,`unhide_project` 设 0。
- `list_projects(filter={hidden:false})` → tab 栏 visible。
- `list_hidden_projects` → empty state hidden 列表。
- DB 行不删,数据完整保留,只是 UI 不可见。

---

## Requirements

### R1 — `addProject()` 命中 hidden 路径自动 unhide(主修复)

**文件**: `app/src/stores/projects.ts:147-190`

修改 `addProject()` 检查顺序:
1. **dialog 失败**:`pickError` toast 现有行为不变,return null。
2. **dialog 取消**:`picked === null` return null(不变)。
3. **首次进入若 hiddenProjects 未加载**:`await loadHiddenProjects()`(防 race)。
4. **visible 命中**:`projects.value.find` → focus + toast「项目已存在」(不变)。
5. **hidden 命中(新增)**:`hiddenProjects.value.find` → 调已有 `unhideProject(hidden.id)`(内部已 load + focus) + toast「已重新打开」,return hidden。
6. **全新**:`create_project` IPC(不变)。

### R2 — `EmptyProjectState` onMounted 自动加载

**文件**: `app/src/components/chat/EmptyProjectState.vue`

`<script setup>` 加 `onMounted(() => projectsStore.loadHiddenProjects())`,首屏 `hiddenProjects` 即有数据,列表立即显。

保留 "查看最近隐藏的项目" 按钮(隐藏条件 `hiddenProjects.length === 0`)作为兜底——若 IPC 失败用户可手动重试。

### R3 — 主 UI 暴露隐藏项目恢复入口(`HiddenProjectsMenu`)

**新文件**: `app/src/components/HiddenProjectsMenu.vue`

UI 行为:
- 仅 `hiddenProjects.length > 0` 时显示(否则不渲染)
- AppHeader 中:`+` 按钮前,archive 图标 + 数量 badge
- 点击 → popover/dropdown 列 hidden projects,每行 "重新打开" 按钮 + 项目名 + path
- 调 `unhideProject(id)`,store 已处理后续状态

**修改**: `app/src/components/layout/AppHeader.vue` 引入新组件。

### R4 — 前端 store 单测

**新文件**: `app/src/stores/projects.test.ts`

覆盖:
- `addProject()` 命中 visible → 不调 IPC,return existing
- `addProject()` 命中 hidden → 调 `unhide_project`,不调 `create_project`,return hidden
- `addProject()` 全新路径 → 调 `create_project`
- `addProject()` 用户取消(picked=null) → 不调 IPC,return null
- `addProject()` dialog 失败(pickError) → 不调 IPC,return null

mock 模式:`vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }))` + spy。

---

## Acceptance Criteria

### Bug 路径
- [ ] 关掉项目后,主 UI 出现 "已隐藏项目" 入口(badge 显示数量),点击 popover 列 hidden projects。
- [ ] 点 "重新打开" → 项目回到 tab,会话列表正常加载。
- [ ] **核心回归**:关闭项目后,点 `+` 添加同路径 → toast「已重新打开: <name>」,不报错,项目回到 tab。
- [ ] EmptyProjectState 首次进入 → hidden 列表立即可见,无需点 "查看最近隐藏的项目"。

### 不回归
- [ ] 添加全新路径 → 走 `create_project` 不报错。
- [ ] 添加已存在的 visible 路径 → focus 现有项目 + toast「项目已存在」。
- [ ] dialog 取消 / 失败 → 不调 IPC,return null。

### 全局
- [ ] `pnpm exec vitest run` 全 pass(含新增 `projects.test.ts` 5 case)。
- [ ] `pnpm exec vue-tsc --noEmit` 0 error。
- [ ] `cd app/src-tauri && PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig cargo check` 0 warning。

---

## Definition of Done

- 3 处代码改动(Fix 1/2/3)+ 1 个新 store 测试文件 + 1 个新 UI 组件。
- spec 同步:`.trellis/spec/frontend/projects.md` (若有) 加"重新打开契约"小节。
- DEBT.md 新增 `RULE-FrontProj-001`(若 Fix 3 留作 follow-up;本次已实施则 close)。
- 四段式 commit(fix → docs(debt) → archive → journal)。

---

## Technical Approach

### 方案 A(采用)— 复用现有 `unhideProject()` API

**How**: `addProject()` 命中 hidden 路径时,直接 await `unhideProject(hidden.id)` —— 它内部已经做了 `invoke unhide_project` + `loadHiddenProjects` + `loadProjects` + `currentProjectId = fresh.id` 完整流程,无重复逻辑。

**Pros**: 零新逻辑,复用 100% 测试覆盖的 unhideProject;async/await 顺序保证 IPC 完成才 return;UI 立即 focus 新 unhidden 项目。

**Cons**: 隐含一次 IPC (`loadHiddenProjects` 在 unhideProject 内会再 await 一次)—— 可接受(只在 hidden 命中路径走)。

### 方案 B(否决)— `addProject` 内联 unhide SQL/命令

- 否决理由:重复 unhideProject 已有逻辑,代码分散,test 覆盖两份。

### HiddenProjectsMenu UI 选型

- reka-ui `DropdownMenu` (项目已用,见 SubagentDrawer + WorkerAskBanner) — 一致性最佳。
- 触发:archive icon button + count badge;popover 渲染列表 + per-row "重新打开" button。

---

## Decision (ADR-lite)

**Context**: 项目隐藏是 soft-delete(`hidden=1`),DB 行不删;但 UI 隐藏入口只在 `EmptyProjectState`(只有 0 项目时挂)。多项目场景用户隐藏单个后,主 UI 看不到任何 hidden 项目入口。唯一兜底"添加同路径"撞 UNIQUE 报错。

**Decision**:
- 主修复:`addProject` 命中 hidden 路径 → 自动 unhide(消除用户撞 UNIQUE 的路径)。
- EmptyProjectState 自动加载 hidden 列表(降低首屏摩擦)。
- 主 UI 加 `HiddenProjectsMenu`(多项目场景恢复入口)。
- 复用既有 `unhideProject()` API,零新后端逻辑。

**Consequences**:
- 三个 fix 互为补充,任何一个都能让 bug 不再发生(核心是 Fix 1)。
- 新增 1 UI 组件 + 1 测试文件,改动面小。
- 若 `HiddenProjectsMenu` 留作 follow-up,Fix 1 + Fix 2 仍能保证核心 bug 不再出现。

---

## Out of Scope

- 项目硬删除(`delete_project`)—— PROPOSAL `docs/_archive/2026-06-3b-1/PROPOSAL-project-binding-and-top-tabs.md` 明确 out of scope。
- main-chat permission_ask 历史 outcome 回放(Session 64 follow-up,已 bundle RULE-WorkerAsk-001/004 收尾)。
- 多级项目目录嵌套 / 项目组 / 项目标签 —— V2 路线图功能,不在本 task。
- `HiddenProjectsMenu` 用 reka-ui Dialog vs DropdownMenu 决策 —— 取项目现有 DropdownMenu 风格(WorkerAskBanner 同源)。

---

## Implementation Plan (single PR)

按 PR 顺序:
1. **PR1 Fix 1 + Fix 2 + R4 tests**:核心修复 + 自动加载 + 单测(改 2 文件 + 新 1 文件)
2. **PR2 Fix 3**:主 UI 入口(改 1 文件 + 新 1 文件)
3. **PR3 docs**:spec 同步 + DEBT.md 回填
4. **PR4 archive + journal**:trellis 收尾

---

## Technical Notes

### 关键 file:line

- `app/src/stores/projects.ts:147-190` — `addProject` 主体(改)
- `app/src/stores/projects.ts:217-229` — `unhideProject` (复用,不动)
- `app/src/stores/projects.ts:137-141` — `loadHiddenProjects` (复用)
- `app/src/components/chat/EmptyProjectState.vue` — 改 (Fix 2)
- `app/src/components/layout/AppHeader.vue:28-41` — 引入 HiddenProjectsMenu (改)
- `app/src/components/HiddenProjectsMenu.vue` — 新增 (Fix 3)
- `app/src/stores/projects.test.ts` — 新增 (R4)
- `app/src-tauri/src/db/projects.rs:16-61` — `create_project` UNIQUE 报错位置(只读不改)
- `app/src-tauri/src/lib.rs:127` — `unhide_project` IPC 注册(已 OK)
- `app/src/components/chat/ChatWindow.vue:66-68` — `showEmptyState` 条件(只读不改)

### 相关 spec

- `docs/_archive/2026-06-3b-1/PROPOSAL-project-binding-and-top-tabs.md` — 项目隐藏机制原始设计
- `app/src-tauri/src/db/migrations.rs:57-97` — `projects` 表 schema + `hidden` 列

### 复用 / 风格参考

- `WorkerAskBanner.vue` — reka-ui DropdownMenu 风格参考
- `app/src/components/SessionList.vue:78-82` — onDelete 模式参考
- `app/src-tauri/src/db/tests.rs:104-119` — `hide_and_unhide_project` 已覆盖后端逻辑