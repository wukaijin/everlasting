# Checkpoint Contract — N2 悬空快照与 revert

> 基线:2026-09-20 任务 `09-20-n2-checkpoint-revert` 四 PR(a880a85b / 4f41fafd / e81bda3c / d49fb8fd)。
> 何时读本文:涉及 `git/checkpoint.rs`、`agent/checkpoint.rs`(快照挂钩)、`commands/checkpoint.rs`(四命令)、`db/checkpoint.rs`、`RevertConfirmModal` 或任何要**在用户仓库上创建 git 对象**的新特性时。
> 关联:[worktree-contract](./worktree-contract.md)(repo 路径解析三态)、[git-diff](./git-diff.md)(FileDiff/numstat 兜底)、[database-guidelines](./database-guidelines.md)。

## Scenario: 悬空快照链 + UI revert(N2,2026-09-20)

### 1. Scope / Trigger

- 触发:新增四命令 IPC 面 + `turn_checkpoints` 表迁移 + agent loop 挂钩 + `AuditKind::CheckpointReverted`——跨层契约,须 code-spec 深度。
- 一句话:turn 边界的文件快照是 **DB 寻址的悬空 git commit 链**,不是分支上的 commit;revert 仅 UI 触发,还原工作区不碰分支/索引。

### 2. Signatures

**git 纯函数层**(`git/checkpoint.rs`,零 db/daemon import):

```rust
pub fn build_state_tree(repo: &Repository) -> Result<Oid, GitError>          // 内存索引快照,零接触
pub fn append_snapshot(repo, parent: Option<Oid>, tree: Oid, sid: &str, seq: u64) -> Result<Oid, GitError>  // update_ref=None 悬空
pub fn set_umbrella_ref(repo, sid, tip) / delete_umbrella_ref(repo, sid)    // refs/everlasting/<sid>,删除幂等
pub fn diff_snapshots(repo, a: Oid, b: Oid) -> Result<DiffResult, GitError>
pub fn count_snapshot_deltas(repo, a, b) -> Result<usize, GitError>          // 徽标专用,零子进程
pub fn compute_restore_set_from(repo, current_tree: Oid, target_tree: Oid) -> Result<Vec<RestorePath>, GitError>
pub fn restore_paths(repo, target_tree: Oid, paths: &[RestorePath]) -> Result<RestoreOutcome, GitError>
```

**挂钩层**(`agent/checkpoint.rs`):`ensure_turn_baseline`(轮首,首轮 user 行落库后、早于任何 tool 执行)/ `snapshot_turn_if_written`(轮末,hub `finalize_turn` 成功后)/ `cleanup_umbrella_refs_best_effort`(delete_session,恒回退主仓库路径再试)。

**DB**(`db/checkpoint.rs`):`upsert_checkpoint`(INSERT OR REPLACE)/ `latest_checkpoint` / `list_checkpoints` / `has_checkpoint_baseline`。

**命令面**(`commands/checkpoint.rs`,`_inner` 单源 + daemon `routes/checkpoint.rs` 薄壳 + Tauri 双注册):`list_turn_checkpoints` / `get_turn_checkpoint_diff` / `revert_to_checkpoint_preview` / `revert_to_checkpoint_execute`。

### 3. Contracts

**表**:`turn_checkpoints(session_id TEXT FK sessions ON DELETE CASCADE, seq INTEGER, tree_sha TEXT, commit_sha TEXT, created_at INTEGER, PK(session_id, seq))`。seq 语义 = **轮末 assistant 行的 messages.seq**;基线行挂首轮 user 行 seq(通常 0),`prev_seq = null`。

**写信号门**(无信号且已有基线 → 零扫描零行):write 家族 tool(`classify_tool` 注册表:write_file/edit_file)‖ A2+ 判写前台 shell(`classify_prefix != ReadOnly`,SideEffect+Ask 均计,宁多勿缺)‖ 后台 shell 完成事件(`DriveTurnOutcome.background_writes`)‖ merge_worker 派发(失败也计)。`run_background_shell` 派发时不算——其异步写由完成事件路观测。

**config**:`app_config` 键 `checkpoints_enabled`,fail-open 仅字面 `"false"` 关(读法对齐 `sandbox_enabled` 单例)。

**revert 两步**:preview 返回 `{files: [{path, action, attribution: ToolWritten|ShellWrite|Unknown}], foreign_delta: Option<Vec<FileDiff>>, preview_token: "{target_tree_oid}:{gate_tree_oid}"}`;execute 入参带 token,重算 gate 树 + DB 重解 target 树再拼比对。**token 验证与还原集派生共用同一次 `build_state_tree` 结果**(`compute_restore_set_from` 不二次扫盘)。

**audit**:`AuditKind::CheckpointReverted`(wire `"checkpoint_reverted"`),payload:`target_seq / restored / deleted / paths(封顶 32+total)/ foreign_delta 摘要 / gate_tree_oid / source:"ui"`;仅 restore 成功后落行,失败路径不落。

### 4. Validation & Error Matrix

| 条件 | 行为 |
|------|------|
| 非 git 项目 / 群聊 session / 无行 / 基线行请求 diff | typed kind `CheckpointsUnavailable`(InvalidRequest 类,前端隐藏不 toast) |
| DB 行在而 git 对象不在(手动 gc/ref 被清) | typed kind `CheckpointBroken`,warn 日志,不 panic |
| execute 时本 session busy(`session_active_request` 注册面) | typed `SessionBusy`;preview 纯读不设门(OQ-C:MVP 只拒本 session,他 session 不拒) |
| preview→execute 之间外来写入 | gate 树失配 → `StalePreview`(retryable,重新 preview) |
| 用户仓库 EUNMERGED 未解冲突 | `write_tree_to` Err → fail-open warn,该轮无快照,整链停摆可观测 |
| 挂钩任一步失败 | warn + 轮收尾照常(fail-open 全链) |

### 5. Good/Base/Bad Cases

- **Good**:写轮(shell 重定向落盘)→ 当轮快照收录产物;轮间 diff 恰含产物文件;revert 后分支指针与 `git diff --cached` 逐字节不变。
- **Base**:纯只读 session → 仅基线 1 行、零新增 git 对象、零额外扫描(AC9);基线 revert = 回到会话前(AC10)。
- **Bad(禁止)**:在用户分支/索引上落任何东西——`index.write()`、`reset --hard`、`checkout <branch>`;revert 做成 agent tool 进 ⑨ 工具面(PRD 约束:仅 UI)。

### 6. Tests Required

- PR0 单测 14(零接触三不变:index 字节+mtime+inode / 去重 / untracked 含嵌套 / 删除传播含目录 / tracked-后-gitignore 照收 / EUNMERGED / restore 全语义 / 伞 ref / worktree 共享 refs)。
- PR1 集成 17(挂钩 fire / fail-open / AC8 前半 / AC9 对象计数恰 +1 / AC10 / AC6 含死 worktree 回退 / AC11 D3 级联)。
- PR2/PR3 命令层 19(prev_seq 稀疏链 / 降级臂 / AC3-5 / AC8 后半真 merge / 链不截断 / busy / StalePreview 恢复)。
- e2e `checkpoint-revert.spec.ts` 4 用例(route-mock 确定性,CI 收录档)。

### 7. Wrong vs Correct

**Wrong**——快照用独立内存索引:
```rust
let mut idx = git2::Index::new();      // bare index 无仓库背书
idx.add_all(["*"].iter(), ...)?;        // libgit2 直接 fail(官方文档 + sweep.rs 先例双证)
```
**Correct**——真句柄 + 内存操作 + 永不持久化:
```rust
let mut idx = repo.index()?;            // 真索引句柄(内存形态)
idx.add_all(["*"].iter(), IndexAddOption::DEFAULT, None)?;
let tree = idx.write_tree_to(repo)?;    // 只写 tree 对象
// 全程无 idx.write() —— stat cache 不落盘(AC1 三不变断言钉死)
```

**Wrong**——轮末钩挂在 drive.rs 的 `finalize_turn_persist` 后:该点在本轮工具执行**之前**(assistant 行先落库、工具后 dispatch),快照晚一轮 + 纯文本收尾轮的最后一个写轮永无快照。
**Correct**——挂 hub `chat_loop.rs` 的 `finalize_turn`(tool_result 落库)成功后;drive_turn Err 的错误/取消路径不挂钩(部分写入靠 revert foreign_delta 门兜底,是设计不是缺陷)。

**Wrong**——`diff(基线, seq)` 字面 `seq-1`:写触发门稀疏链下 seq-1 常无行。
**Correct**——prev_seq =「前一个存在行」(list 单趟有序扫描回传),文案用「自上一快照以来」。

### 已知边界(如实记录)

- untracked 且 gitignored 不入快照;tracked 后进 gitignore 仍收录且删除照传。gitignored 路径双重不可见(diff 不显 + revert 不还原),确认弹窗常驻脚注披露;实测命中率 0/16=0%(2026-09-20 本机 DB),include-ignored 档留 follow-up。
- busy check-then-act 毫秒级窗口(检查后到 restore 前有 turn 可启动)——OQ-C 裁定接受,与「共享 cwd 争用不恶化不负责治」一致。
- submodule(gitlink)条目 restore 时 `find_blob` 失败 → Err fail-loud,不做 submodule 支持。
- 末轮后台 shell 完成事件恰在 drive 轮顶被 drain 且该轮 Err 终态 → 当轮旗标丢弃,下轮 run 首轮再 drain;残留写入落 foreign_delta 门。
