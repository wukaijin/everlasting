# step 4: git worktree + diff 视图 (重定范围: 不做 auto-commit)

## Goal

每个 chat session 在 git worktree 内跑（独立分支 `session/<session_id>`），
agent 改的文件全部进 worktree，**前端能看 diff**。

**重定说明**（2026-06-07 brainstorm 收口）：原计划包含 turn 边界
auto-commit，经讨论后决定**不 baked into core**——auto-commit 是
policy，不同人偏好不同；Claude Code / Cursor / Copilot Workspace 也
都不要 auto-commit。**未来用 Skill 替代**：用户装 `git-isolation-skill`
就开启 turn 边界 auto-commit，不装就走 manual。Core 只做 state
management（worktree 生命周期 + 工具路径）+ 可见性（diff 视图）。

## What I already know

* 决策来自 `docs/ARCHITECTURE.md §3`：每个 session 一个 worktree
* 路径约定：`~/.local/share/everlasting/worktrees/<project_uuid>/<session_id>`，分支 `session/<session_id>`
* libgit2 worktree `add` API 完整，`remove` 缺（要 spawn `git worktree remove`）
* `ToolContext` 当前有 `project_root: PathBuf` + `cwd: PathBuf`（`tools/mod.rs:57-61`）
  - 7 个 tool 全部用 `ctx.cwd` 跑，step 4 必须把 `cwd` 切到 worktree 路径
* `db::sessions` 当前字段：(project_id, current_cwd, model)，要加 `worktree_path`
* `is_git_repo = false` 的 project：拒绝创建 session（用户已确认）
* Cargo.toml 当前无 git 依赖，要加 `git2 = { version = "0.20", features = ["vendored-libgit2"] }`

## Requirements (MVP)

* **R1** `git::create_worktree(session_id, project_id)` — 绑 session 生命周期，session 创建时调
* **R2** `git::destroy_worktree(session_id)` — session 删除时调，**worktree 路径 + 分支都删**（用户已确认）
* **R3** `ToolContext.worktree_path: PathBuf` 字段，所有 7 个 tool 用它替代 `ctx.cwd`
* **R4** `db::sessions` 加 `worktree_path: TEXT NULL` 列；旧 session（pre-step-4）该列为 NULL，工具 fallback 到 `current_cwd`
* **R5** `git::diff_worktree(session_id)` Tauri command，返回**文件列表 + per-file diff**（unified 文本）
* **R6** 前端 `<DiffView>` 组件：jsdiff + 自渲染 unified diff（参考 `research/frontend-diff.md`）
* **R7** **双视角 diff 触发**：
  - **R7a** ChatPanel header 加 "diff (N files)" 按钮 → 弹窗显示整个 session 的所有文件 diff
  - **R7b** edit_file 工具卡片加 "diff" 按钮 → 弹窗显示单文件 diff（pre-edit vs post-edit 内容）
* **R8** `edit_file` tool 在执行前捕获文件内容 pre-edit，与 post-edit 一起存入 `tool_result` 旁路（供 R7b 渲染）

## Acceptance Criteria

* [ ] 新建 session 后，`<worktree_path>/.git` 是有效的 git worktree，分支 `session/<session_id>` 存在
* [ ] 工具改动落在 worktree 路径，**不污染 project 主目录**（shell 跑在 worktree 路径下）
* [ ] session 删除后 worktree 路径被清理（**分支是否清理待定**）
* [ ] pre-step-4 的旧 session 仍可用（worktree_path NULL 时 fallback 到 current_cwd）
* [ ] 前端能看 session 的 worktree diff（unified 视图）
* [ ] non-git project 创建 session 时报错，UI 给出清晰提示
* [ ] `pnpm tauri dev` 跑通，无 cargo / vue-tsc 报错

## Decision (ADR-lite)

**Context**: 原 plan 把 auto-commit 列为 step 4 必做。讨论后发现：
(1) auto-commit 是 policy，baked into core 不灵活；
(2) Claude Code / Cursor / Copilot Workspace 都不 auto-commit；
(3) BACKLOG §2 规划了 Skill 系统——auto-commit 的天然归宿是 Skill。

**Decision**: step 4 **不做 auto-commit**。Worktree 生命周期 + diff 视图仍
是 core（state + 可见性）。未来用 `git-isolation-skill` 让用户自行开启。

**Consequences**:
- + Core 更瘦，policy 灵活
- + 跟大厂 agent 产品行为一致
- - 数据安全靠用户手动 `git commit`（如果他忘了，崩了会丢 worktree 修改）
- - 未来 Skill 实现需要"在 worktree 内调 git"的 tool 入口（tool 路径已经切到 worktree，自动 work）

## Out of Scope

* Auto-commit（任何时机、任何形式）→ 未来 Skill
* LRU 容量策略、worktree 占用 UI → 真实场景触发再做
* Merge 回主分支流程
* 跨设备 worktree 同步（ARCH §3 提到）
* Sub-agent / 编排相关 worktree 扩展（架构 B 思路）→ 未来
* 历史 commit UI 时间线

## Open Questions

1. ✓ Worktree + 分支清理：两者都删（已确认）
2. ✓ Diff API 形态：文件列表 + per-file diff（已确认）
3. ✓ Diff UI 触发点：header (session 视角) + edit_file card (单文件视角) 双视角（已确认）
4. ✓ DB schema migration：pre-step-4 session worktree_path NULL，工具 fallback 到 current_cwd（已确认）
5. **git2-rs vendored build 慢 30-90s** — 团队接受？（待确认）
6. **edit_file pre-edit 捕获策略**：在 tool 内部读文件再写入（IO × 2）vs 写前 `cp` 临时副本（多 1 步 fs 操作）？哪个？
7. **diff 视图弹窗形态**：modal / popover / 抽屉？

## Technical Notes

* Files: `lib.rs:248-308` (session CRUD), `tools/mod.rs:57-61` (ToolContext), `db.rs:43-67` (SessionRow)
* Spec: `.trellis/spec/backend/project-cwd-boundary.md` (path 边界，step 4 后是 worktree root)
* Cargo: 加 `git2 = { version = "0.20", features = ["vendored-libgit2"] }`
* Research: `research/git-backend.md`, `research/frontend-diff.md`

## Implementation Plan (3 PR)

* **PR1** `feat(git): worktree create/destroy on session lifecycle` — DB 加 worktree_path + libgit2 集成 + session 生命周期 hook
* **PR2** `feat(tools): run all 7 tools in worktree path` — ToolContext 改造 + 7 个 tool 路径切换 + 边界检查更新
* **PR3** `feat(chat): diff view for worktree` — git::diff_worktree IPC + 前端 DiffView 组件 + 触发点
