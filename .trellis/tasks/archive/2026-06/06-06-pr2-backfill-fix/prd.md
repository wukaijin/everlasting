# PR2 follow-up: 启动时 batch backfill 老项目的 git_branch

> 父 task: `06-06-spike-005-follow-up` (已 archived)
> 父 PR2: `8f25b7f` feat(ui): 显示真实 git branch
> 父 prd: [.trellis/tasks/06-06-spike-005-follow-up/prd.md](../06-06-spike-005-follow-up/prd.md) (PR2 段)
> Priority: P1
> 关联 spike: `docs/spikes/2026-06-06-feature-requests.md` §2c + PR2 commit message

## Goal

修复 PR2 锁定的 **lazy backfill 决策**导致的 bug: 老项目 (PR2 之前创建) migration 后 `is_git_repo=0, git_branch=NULL`, **永远**显示 fallback 文本 "git", 跟 spike 原始需求"显示真实 branch"冲突。

**修法**: 取消 lazy backfill 锁定, 改成**启动时 batch re-probe**。`AppState::load` 完成后, `tokio::spawn` 一个 fire-and-forget 任务, 对所有 `is_git_repo=0` 的老项目重新探测并写回 DB。

## What I already know

- `app/src-tauri/src/db.rs:213-220` PR2 加了 `add_project_column_if_missing(pool, "is_git_repo", "INTEGER NOT NULL DEFAULT 0")` + `git_branch TEXT`, 老 row 走 ALTER TABLE default (`is_git_repo=0, git_branch=NULL`)
- `app/src-tauri/src/lib.rs:80-89` `AppState::load` 当前只开 sqlite pool, 没 batch re-probe
- `app/src-tauri/src/projects/store.rs:67-73` `update_project_path` 探测逻辑可复用 (sync 版)
- `app/src-tauri/src/projects/detector.rs:27, 79` `is_git_repo_sync` + `current_branch_sync` 已存在且 <1s
- 父 prd "Decision 2: 不批量 backfill" 在用户实测下是错的, 应**改写**
- 项目数预计 < 10, 总探测耗时 < 1s, 启动阻塞可接受
- 启动**后** (DB 写入) 才查看到 chip 更新, 用户体验: 重启后立即看到正确 branch (不需操作)

## Requirements

### Backend
- 新 `db::list_projects_with_stale_git_probe(pool) -> Result<Vec<ProjectRow>>`:
  - SELECT 所有 `is_git_repo = 0` 的项目
  - 走现有 `list_projects` 的 SELECT 结构, 加 `WHERE is_git_repo = 0`
- 新 `projects::store::batch_reprobe_git_metadata(pool) -> Result<usize, String>`:
  - 调 `db::list_projects_with_stale_git_probe` 拿老项目
  - 对每个 project: 调 `is_git_repo_sync` + `current_branch_sync`
  - 如果 `is_git=true` 但 `git_branch=None` (矛盾), 重新探测时优先信任 `is_git=true`, 把 `git_branch` 设 None (保持现状, 不强填)
  - UPDATE projects SET is_git_repo=?, git_branch=?, updated_at=? WHERE id=?
  - 返回更新的行数
- `lib.rs:80-89` `AppState::load` 末尾:
  ```rust
  let pool_for_backfill = db.clone();
  tauri::async_runtime::spawn(async move {
      if let Err(e) = projects::store::batch_reprobe_git_metadata(&pool_for_backfill).await {
          tracing::warn!(error = %e, "git metadata backfill failed");
      }
  });
  ```
- 用 `tokio::spawn` 不阻塞 `AppState::load`, 后台跑
- 加 `tracing::info!` 记录: 启动时探测了多少项目, 多少更新了
- 失败时 `tracing::warn!` 不 panic, 留待下次启动重试

### Frontend
- 不变 (用户视角: 重启后 store 自动 reload, chip 立刻显示 branch)
- 实际需要: 前端在 backfill 完成后自动 refresh 一次 (否则 stale). 两种选择:
  - **A**: Tauri event `app.emit("projects:refreshed", ())` 触发前端 `loadProjects()`
  - **B**: 不发 event, 让用户在 chat panel 切项目时自然 reload (因为 `currentProject` 切换会重新调 `loadSessions` 等)
  - 选 A: 1 行 emit + 1 个前端 listener, 用户重启后无需切项目就看到 branch

## Acceptance Criteria

- [ ] 重启 Tauri (含老 git 项目), 老项目 chip 自动显示真实 branch (无需手动 `update_project_path`)
- [ ] `cargo test --lib` 全过 (含 1 个新增的 batch_reprobe_git_metadata test, 测 fixture: 创建 1 个 `is_git_repo=0` 的老 row, 调 backfill, 验证变 `is_git_repo=1, git_branch=Some("main")`)
- [ ] `pnpm build` + `pnpm test` 通过
- [ ] 启动时间不显著变长 (< 2s 总)
- [ ] 启动后台任务失败时 `tracing::warn!` 记录但不 panic
- [ ] 前端 (Tauri event listener) 收到 `projects:refreshed` 时调 `loadProjects()` refresh 一次

## Definition of Done

- 修改 ~3 个文件 (`db.rs` / `projects/store.rs` / `lib.rs` + 可能前端 1 个 store 监听)
- cargo test + pnpm build + pnpm test 三过
- 跑完 standard Trellis 流程到 archived
- 视觉验证: 重启 Tauri, 老 git 项目 chip 显示 branch (无需操作)

## Out of Scope

- 删除 "lazy backfill" 历史决策的文档 (BACKLOG / 父 prd) — 留作 docs cleanup
- ~~监听 git 事件实时刷新 chip~~ → 见 Future Work 段 (用户决定先做方案 B, 实时性留 v2)
- 多设备同步 (BACKLOG §4)
- 把 `update_project_path` 改成探测而非不探测 (现状已对)

## Future Work: git 实时性 (v2+)

用户场景: 在 shell 里跑 `git checkout feature/x` 后, 期望 chat header 的 git chip 立即反映新 branch, **不需要 restart Tauri**。

### 候选方案

1. **方案 A — 切项目 lazy 探测**: `currentProject` 切换时, 触发 IPC `reprobe_git(id)`, 几百 ms 后 chip 变真实 branch. 适合老项目切到时补数据.
2. **方案 C — 切项目每次探测**: 每次切项目都触发探测. 重复 IPC, 不必要 (DB 已有数据).
3. **方案 D — git fsnotify 监听**: 用 `notify` crate 监听 `.git/HEAD` 文件变化, 变化时重新探测 + emit `projects:refreshed` event. 实时性最佳, 但增加 dep + 跨平台 fsnotify 复杂度 (WSL 已知坑见 `docs/HACKING-wsl.md`).

### 决策: 暂不做 (本 PR 仅方案 B 启动 batch backfill)

- 理由: 启动 backfill 一次性修复老项目, 之后切项目 DB 都正确, 实时性需求不强
- 监听 `.git/HEAD` 跨平台复杂度 (WSL fsnotify inotify 限制) 不值得为这个 UX 投入
- 备选: 若 v2 启动后用户频繁报"chip 没更新", 优先走方案 A (切项目 lazy 探测) 简单方案

### 文档更新

PR2 commit message 跟本 PR 决策段都应记录: "git 实时刷新留 v2, 候选方案 A/C/D, 见 `.trellis/tasks/06-06-pr2-backfill-fix/prd.md` §Future Work"。

## Technical Notes

- 改动文件:
  - `app/src-tauri/src/db.rs` (`list_projects_with_stale_git_probe` + 1 新增 cargo test)
  - `app/src-tauri/src/projects/store.rs` (`batch_reprobe_git_metadata` 复用 `is_git_repo_sync` + `current_branch_sync`)
  - `app/src-tauri/src/lib.rs` (`AppState::load` 末尾 spawn 后台任务)
  - `app/src/stores/projects.ts` (新 listener `projects:refreshed` 调 `loadProjects()`)
  - `app/src/components/ChatWindow.vue` (onMounted 注册 listener, 已有 `listen` pattern 可参考 chat.ts)
- 风险: 启动阻塞 — 用 `tokio::spawn` 异步, 探测 < 1s 不会感知
- 风险: 探测期间用户切到老项目, 看到 stale 数据 — 选 A (Tauri event) 解决
- 风险: 用户 git repo 在 WSL 下路径 case sensitivity 不同 — 探测按用户原始 path 探测, 跟现状一致
- 关联: 父 prd 锁定 "Decision 2: 不批量 backfill" 推翻, 在 commit message 解释

## Decision (ADR-lite)

- **决策 1**: 取消父 prd "Decision 2: 不批量 backfill" 锁定, 改为启动 batch backfill
  - **理由**: 实测发现 lazy 决策导致老项目永远显示 fallback, 跟 spike 原始需求冲突; 项目数 < 10 时启动阻塞 < 1s 可接受
  - **后果**: 启动时间微增 (用户感知不到); 老项目 chip 自动正确; 父 prd 决策需在 commit message 解释推翻
- **决策 2**: 后台 `tokio::spawn` + Tauri event 通知前端 refresh
  - **理由**: 不阻塞启动, 不阻塞 chat panel 加载; 完成后 emit event 让前端 reload store
  - **后果**: 多 1 个 IPC channel (`projects:refreshed`); 前端多 1 个 listener; 用户体验更平滑
