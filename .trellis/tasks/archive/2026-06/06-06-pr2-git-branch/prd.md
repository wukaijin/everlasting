# PR2: git branch 真显示 (backend 探测 + DB migration + chip 渲染)

> Source spike: [`docs/spikes/2026-06-06-feature-requests.md`](../../../../../docs/spikes/2026-06-06-feature-requests.md) 第 2c 子项
> 父 task: `06-06-spike-005-follow-up`
> 父 prd: [../06-06-spike-005-follow-up/prd.md](../06-06-spike-005-follow-up/prd.md) (PR2 段)
> Priority: P1
> 关联 research: [`research/git-detection-rust.md`](research/git-detection-rust.md)
> 锁定决策: 复用 `projects/detector.rs` 现有 `is_git_repo` async pattern; `git rev-parse --abbrev-ref HEAD` 在 detached 时存字面量 `"HEAD"`

## Goal

让 chat panel header 右上角 git chip 显示**真实**的当前分支名 (而不是静态 "git" 占位)。
- Backend: DB `projects` 表加 `is_git_repo: bool` + `git_branch: Option<String>` 列 (幂等 migration)
- Backend: `projects/detector.rs` 加 `current_branch_sync(path)` + `current_branch(path)` 镜像 `is_git_repo` async pair
- Backend: `create_project` / `update_project_path` 时调用 `current_branch` 探测并存 DB
- Frontend: `ChatPanel.vue:74` 静态 "git" 替换为 `currentProject.value.git_branch` (已有占位 chip 在那)

## What I already know

- `app/src-tauri/src/projects/detector.rs` 已有 `is_git_repo_sync(path) -> bool` (sync) + `is_git_repo(path) -> bool` (async, 1s timeout + spawn_blocking)
- `app/src-tauri/src/projects/store.rs:57` `update_project_path` 已 re-probe `is_git_repo`
- `app/src-tauri/src/db.rs:287-301` 有 `pragma_table_info` + `ALTER TABLE ADD COLUMN` 幂等 migration pattern (3b-1 阶段加 session columns 用的)
- `app/src-tauri/src/tools/shell.rs` 走 `tokio::process::Command` shell-out 模式, 跟 `detector.rs` 一致
- `app/src/components/chat/ChatPanel.vue:55-60` 注释明确: "the backend doesn't yet expose a real branch name on the project; the chip will swap to a real branch string once the Rust side grows a `git_branch` column"
- `app/src/components/chat/ChatPanel.vue:72-75` 当前 chip: `<span v-if="showGitChip" class="chat-panel__chip chat-panel__chip--git">...git</span>`
- `app/src-tauri/src/projects/store.rs` 应该有 `create_project` (从父 task 创建时调用)
- `git rev-parse --abbrev-ref HEAD` 返回当前分支名, **detached HEAD** 时返回字面量 `"HEAD"`
- `git rev-parse --is-inside-work-tree` 已有, 返回 `true`/`false`, exit code 0/128
- 父 research `git-detection-rust.md` 强烈推荐: `std::process::Command` shell-out, 零新依赖, 复用 detector 现有 pattern
- 父 prd 锁定 detached HEAD 存 "HEAD" 字符串 (让 UI 区分)

## Requirements

### Backend

#### 1. `projects/detector.rs` 新增 `current_branch_sync` + `current_branch`
- 镜像 `is_git_repo_sync` / `is_git_repo` 的 pattern (sync + async with 1s timeout + spawn_blocking)
- 签名:
  ```rust
  pub fn current_branch_sync(path: &Path) -> Option<String>
  pub async fn current_branch(path: &Path) -> Option<String>
  ```
- `current_branch_sync` 内部:
  - 先调 `is_git_repo_sync(path)` 确认是 git repo, 不是则返回 `None`
  - 跑 `git -C <path> rev-parse --abbrev-ref HEAD`
  - exit code != 0 → `None`
  - stdout 末尾 `\n` strip
  - 空字符串 → `None`
  - 否则 → `Some(stdout)` (保留 `"HEAD"` 字面量)
- `current_branch` 内部用 `tokio::time::timeout(Duration::from_secs(1), tokio::task::spawn_blocking(move || current_branch_sync(&path)))`, 跟 `is_git_repo` 风格完全一致
- 加 cargo test:
  - 普通 git repo → 返回 `Some("main")` / `Some("feature/foo")` 等
  - detached HEAD → 返回 `Some("HEAD")`
  - 非 git repo → 返回 `None`
  - 路径不存在 → 返回 `None` (不 panic)
  - timeout (用 slowloris-style 模拟太重, 跳过, 代码 review 即可)

#### 2. `db.rs` migration: `projects` 表加列
- 用 `pragma_table_info('projects')` 检查列存在性
- 不存在则 `ALTER TABLE projects ADD COLUMN is_git_repo INTEGER NOT NULL DEFAULT 0`
- 不存在则 `ALTER TABLE projects ADD COLUMN git_branch TEXT` (nullable, no default)
- 幂等: 已存在的 project 二次启动不报错
- 老 project 启动时**不**批量 lazy backfill (避免启动阻塞); 改为 `get_project` 时如果 `is_git_repo = 0` 但 path 存在, 不主动探测; 用户点 `update_project_path` 或 `create_project` 时探测
- 或者: 启动时一次性 backfill (更激进, 但阻塞启动) — 选**lazy**策略, 跟 PR2 优先级匹配
- 新 `db::ProjectRow` struct 加 `is_git_repo: bool` + `git_branch: Option<String>` 字段 (用 `Option` 因为 `git_branch` nullable)

#### 3. `projects/store.rs` create + update 时探测
- `create_project(path)`:
  - 现有 `is_git_repo(path).await` 调用保留
  - **新增** `current_branch(path).await` 调用
  - 存 DB 时把 `git_branch: result` 一起写入
- `update_project_path(id, new_path)`:
  - 现有 re-probe `is_git_repo` 保留
  - **新增** re-probe `current_branch`
  - 存 DB 时把 `git_branch` 一起更新
- `update_project_name(id, new_name)`: 不动 (跟 git 探测无关)

### Frontend

#### 4. `ChatPanel.vue` chip 替换
- 现有 `showGitChip` computed (`!!currentProject.value?.is_git_repo`) 保留逻辑
- 把 `<span v-if="showGitChip" class="chat-panel__chip chat-panel__chip--git">...git</span>` 内的 "git" 文字替换为 `{{ currentProject.value?.git_branch ?? 'git' }}`
- detached HEAD (`git_branch === "HEAD"`) UI 显示 "HEAD @ <short sha>" — 但 short sha 需要额外探测 `git rev-parse --short HEAD`, v1 暂不实现, 只显示 "HEAD" (用户能区分)
- title attr (tooltip) 加 `title="Current branch: {{ git_branch }}"` 让用户能看完整字符串

#### 5. TS interface 更新
- `app/src/stores/projects.ts` 的 `Project` interface 加 `is_git_repo: boolean` + `git_branch: string | null` 字段
- 旧 IPC 调用 (snake_case) 跟 Rust 序列化匹配, **不**改 camelCase (跟 PR2 SPEC §5.2 决策: 暂不 camelCase, 维持一致性)

### 测试

- `cargo test`:
  - `current_branch_sync` 4 case (普通/HEAD/非git/路径不存在)
  - `db::migrate` 幂等 (跑两次不报错)
  - `store::create_project` + `store::update_project_path` 后 DB row 含正确 `git_branch` (需要 fixture git repo, 用 `git init` 在 tempdir 建)
  - `store::create_project` 对非 git 目录 → `git_branch: None` (不被错填)

## Acceptance Criteria

- [ ] 新建 git 项目: chip 显示真实分支名 (e.g. `main`, `feature/foo`)
- [ ] 新建非 git 项目: chip 不显示 (沿用当前 `is_git_repo=false` 逻辑)
- [ ] detached HEAD: chip 显示 "HEAD"
- [ ] `update_project_path` 切换到非 git 目录: chip 消失, `git_branch=None`
- [ ] `update_project_path` 切换到另一个 git 目录: chip 显示新分支
- [ ] DB migration 幂等 (老库二次启动不报错)
- [ ] `cargo test` 全过, 含 detector + store + migration 测试
- [ ] `pnpm build` (vue-tsc + vite) 通过
- [ ] `pnpm test` (vitest) 通过 (PR6 regression)
- [ ] 视觉: chip 颜色/样式跟现状一致, 只是文字变化

## Definition of Done

- 修改 ~6-8 个文件
- cargo test + pnpm build + pnpm test 三过
- 跑完 standard Trellis 流程到 archived
- 视觉验证: 起 Tauri, 创建 git/non-git 项目, 看到 chip 行为符合预期

## Out of Scope

- 监听 git branch 切换 (实时刷新 chip) — v1 切 branch 后需 reload project 或 restart
- 显示 short sha for detached HEAD (v1 只显示 "HEAD")
- worktree 路径展示 (BACKLOG §3 每个 session 一个 git worktree, 暂缓)
- `git2-rs` crate 引入 (TECH.md 锁给 step-4 worktree/diff/commit 阶段)
- 复杂 status (uncommitted changes 提示) — v1 仅显示分支
- `merge` / `rebase` 等状态指示

## Technical Notes

- 改动文件:
  - `app/src-tauri/src/projects/detector.rs` (新增 current_branch sync+async)
  - `app/src-tauri/src/db.rs` (migration + ProjectRow 字段)
  - `app/src-tauri/src/projects/store.rs` (create/update 调用探测)
  - `app/src/components/chat/ChatPanel.vue` (chip 文字 + title)
  - `app/src/stores/projects.ts` (Project interface 加 2 字段)
  - 关联: 父 prd 锁定所有决策, 实施直接套
- 风险: git rev-parse 卡死 (大 repo) — 1s timeout 硬卡, 跟 is_git_repo 一致
- 风险: Windows 下 `git` 不在 PATH — 跟 is_git_repo 一样, 视为非 git repo
- 风险: 测试时在 tempdir `git init` 需要 git CLI, 跟 is_git_repo 测试一致
- 风险: 老 project DB row 没有 `git_branch` 字段 (migration 走 ALTER 后 ALTER 不补值) — DB migration 加列后老行 `git_branch = NULL`, UI chip 不显示 (依赖 `is_git_repo` 计算), 行为正确
- 风险: 探测时机 — `create_project` 时探测一次, 后续用户在外部 `git checkout` 切 branch chip 不更新 — 接受, v1 不监听 git 事件
- 关联: `update_project_path` (store.rs:57) 已 re-probe `is_git_repo`, 同样位置需 re-probe `current_branch`

## Decision (ADR-lite)

- **决策 1**: 探测用 `git rev-parse --abbrev-ref HEAD`, detached HEAD 返回字面量 "HEAD", DB 存 `Some("HEAD")`
  - **理由**: 保留 "detached" 信息 (vs 折叠为 None), UI 可区分
  - **后果**: UI 显示 "HEAD" 而非分支名, v1 不补 short sha
- **决策 2**: 老 project 不批量 lazy backfill `git_branch`
  - **理由**: 避免启动阻塞; 用户操作 (create/update_project_path) 时才探测
  - **后果**: 老 project chip 在 `update_project_path` 之前不显示 branch (但 `is_git_repo` 也是 0, chip 不显示, 一致)
- **决策 3**: TS interface 字段维持 `snake_case` (跟 Rust 序列化一致)
  - **理由**: BACKLOG §5.2 follow-up 暂不决定 camelCase, 维持现状
  - **后果**: 前端写 `currentProject.value.git_branch` 而不是 `gitBranch`
