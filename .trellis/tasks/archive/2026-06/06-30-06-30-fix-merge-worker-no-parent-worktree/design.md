# Design: merge_worker lazy auto-attach

> 配套 PRD: `prd.md`(同目录)
> 技术决策 + 契约 + 文件级设计

---

## 1. 范围

### 1.1 本设计覆盖

- 三个调用点的 lazy auto-attach 行为:
  - `merge_worker_run` Tauri IPC(UI 点击 "合并" 按钮)
  - `merge_worker` 工具 `execute()`(LLM 调工具)
  - **不在**:dispatch 时机自动 attach(本 PR 不引入)
- `attach_worktree` 的内层逻辑抽离为一个 free function `git::worktree::attach_session`
- IPC 返回值结构化(`MergeWorkerResult`)以承载自动 attach 信号

### 1.2 不覆盖

- `create_session` 默认 attach 行为变更
- dispatch 路径修改
- attach UI、worktree chip UI 自身的逻辑变更(只刷新其 store 数据源)
- 任何 audit / permission 层变更(lazy attach 走的是已存在的 attach_worktree 的权限语义;LRM 子代理 merge 行为本身已在 B6 PR2 的 audit 体系内)

---

## 2. 关键不变量(新增 — 必须锁住 spec)

| ID | 不变量 | 触发面 |
|---|---|---|
| INV-M1 | `merge_worker_run` 和 `merge_worker::execute` 在 spawn `do_merge_blocking` 前都调用 `ensure_parent_worktree_attached`,**两个入口行为等价** | 两个入口 |
| INV-M2 | `WorktreeState::Active` 父 → helper 是 no-op,无任何 side-effect、不写 DB、不 inject system event | helper 实现 |
| INV-M3 | `WorktreeState::Detached` 父 → helper 返回 `Ok(None)`,**不**尝试 re-attach(尊重用户 detach 意图) | helper 实现 |
| INV-M4 | `WorktreeState::None` 父 → helper 调 `git::worktree::attach_session`;失败透传 `attach_worktree` 的原错误文案(prefix 加 `cannot auto-attach parent worktree` 给 IPC 调用方),DB 不残留半成品 | helper + attach_session |
| INV-M5 | lazy attach 注入的 `[worktree event] attached:` 系统消息(**已存在**,attach_worktree 路径走) 是下一个 LLM turn 的 authoritative state | 透传 attach_session 的副作用 |
| INV-M6 | 两入口共用 `merge_lock_for(parent_session_id)`,并发 merge 序列化(已存在,本 PR 不动) | 不变 |
| INV-M7 | 在父 dirty / 非 git 项目时,**不**做 lazy attach,直接返回 actionable error 给前端 toast | helper 错误路径 |
| INV-M8 | IPC 返回值 `MergeWorkerResult { message, auto_attached_parent }` 替换原 `String`;后端 libgit2 merge 文案一律走 `message` 字段,前端 `parseConflictFiles` 解析逻辑不变(在 `message` 上跑) | IPC 协议 |

---

## 3. 文件级设计

### 3.1 `app/src-tauri/src/git/worktree.rs`

新增 free function:

```rust
/// Attach a session's `session/<id>` worktree. The inner work of
/// `commands::worktree::attach_worktree` extracted as a free
/// function so tool-layer call sites (merge_worker lazy-attach)
/// can invoke it without dragging a Tauri `State`.
///
/// Contract: same as `attach_worktree` minus the IPC envelope
/// (cancel hook, reload, return SessionRow). The function
/// preserves every invariant of the original:
///   - Reject `WorktreeState::Active` (already attached)
///   - Reject `WorktreeState::Detached` (user explicitly
///     detached — do not silently re-attach; merge_worker skips
///     detached, see INV-M3)
///   - Reject non-git project with `"is not a git repository"`
///   - Reject dirty project root with `"uncommitted changes"`
///   - On success: write `set_worktree_state(Active)` and inject
///     `[worktree event] attached:` system event (the LLM's
///     next turn sees the transition via history)
///
/// Errors propagate as `GitError` (variant maps to user-facing
/// message in the caller). No string-format coupling — caller
/// decides prefix.
pub async fn attach_session(
    db: &SqlitePool,
    project: &ProjectRow,
    session_id: &str,
) -> Result<PathBuf, GitError>
```

**实现要点**:
- 复制现有 `attach_worktree` 命令体内 `line 47-99` 的核心块(去掉 `cancel_inflight_for_session` 调用 —— 那层是 IPC 边界专属,merge 的 cancel 语义走 `merge_lock_for`)。
- 函数是 `async` 因为 `db::set_worktree_state` 和 `db::insert_system_event` 都是 `async`。
- 返回 `PathBuf` 而非 `String`,因为后续 `do_merge_blocking` 接 `&Path`。
- 不返回 `AttachNotice` 结构体 —— 该结构由调用方在 `Ok(Some(PathBuf))` 分支手动构造(简洁,不需要 serde)。

**单元测试**(写在 `git/worktree.rs` 已有 `#[cfg(test)]` 块下):
- `attach_session_happy_path`:用现有 test fixture 项目初始化,设置 session `worktree_state=None`,调 `attach_session`,断言:
  - 返回 `Ok(PathBuf)` 指向 `<data_dir>/worktrees/<pid>/<sid>`
  - DB 行 `worktree_state='active'` 且 `worktree_path=<上面那个>`
  - `messages` 表有 `role='user'` 且 content 含 `"worktree attached:"` 的行
  - libgit2 metadata(worktree list 多一条)
- `attach_session_active_state_rejected`:fixture 已 `Active` → 调 `attach_session` → 返回 `Err`,DB 不变
- `attach_session_detached_state_rejected`:fixture 已 `Detached` → 返回 `Err`,DB 不变(本函数不应被这类调用,但作为防御性契约测试守住)
- `attach_session_non_git_project`:project `is_git_repo=false` → 错误文案含 `"is not a git repository"`,无 disk 副作用
- `attach_session_dirty_project`:fixture 项目根创建一个 untracked 文件 → 错误文案含 `"uncommitted changes"`,DB 不变

### 3.2 `app/src-tauri/src/commands/worktree.rs`

`attach_worktree` 命令体改写为薄壳:

```rust
#[tauri::command]
pub async fn attach_worktree(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<db::SessionRow, String> {
    // 1. cancel hook (IPC 边界专属)
    let _ = cancel_inflight_for_session(
        &state.cancellations,
        &state.session_active_request,
        &session_id,
    ).await;
    // 2. 加载必要 row
    let loaded = db::load_session(&state.db, &session_id)...
    let project = db::get_project(&state.db, &loaded.session.project_id)...
    // 3. 委派 helper(同步 disk + DB 写入 + system event)
    git::worktree::attach_session(&state.db, &project, &session_id)
        .await
        .map_err(|e| format!("attach_worktree: {}", e))?;
    // 4. reload + return IPC envelope
    let updated = db::load_session(&state.db, &session_id)...
    Ok(updated.session)
}
```

**契约保证**: `cargo test` 现有的 `attach_worktree_*` 系测试(若有)不该改;若有回归由相应 test 守护。**没有**的话,本 PR 加一个新 test 锁住 IPC 入口的 happy 路径。

### 3.3 `app/src-tauri/src/commands/subagent_runs.rs`

`merge_worker_run` 串入 helper:

```rust
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeWorkerResult {
    pub message: String,
    pub auto_attached_parent: bool,
}

#[tauri::command]
pub async fn merge_worker_run(
    _rid: String,
    run_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<MergeWorkerResult, String> {
    let run_row = ...;  // 不变
    let parent_session_id = ...;
    let worker_wt = ...;  // 不变

    let session_row = db::load_session(&state.db, &parent_session_id)...
    let mut auto_attached = false;
    if session_row.session.worktree_state == db::WorktreeState::None
        || session_row.session.worktree_path.is_none()
    {
        // 对 None + worktree_path==None 都触发(INV-M2/3 改在 helper 内判断)
        match crate::tools::merge_worker::ensure_parent_worktree_attached(&state.db, &parent_session_id).await {
            Ok(true) => {
                tracing::info!(
                    parent_session_id = %parent_session_id,
                    "merge_worker_run: auto-attached parent worktree for merge"
                );
                auto_attached = true;
            }
            Ok(false) => {} // 已绑或 detached,不做事
            Err(e) => {
                return Err(format!(
                    "merge_worker_run: cannot auto-attach parent worktree: {}", e
                ));
            }
        }
    }

    // Stage 1: libgit2 merge(不变)
    let parent_wt_for_task = ...;
    let merge_result = tauri::async_runtime::spawn_blocking(move || {
        crate::tools::merge_worker::do_merge_blocking(
            &parent_wt_for_task,
            &parent_session_id_for_task,
            &run_id_for_task,
        )
    })
    .await
    .map_err(|e| format!("merge_worker_run: task join failed: {}", e))?;

    match merge_result {
        Ok(msg) => {
            let cleanup_result = crate::tools::merge_worker::finalize_merge(&state.db, &parent_session_id, &run_id).await;
            if let Err(e) = cleanup_result { tracing::warn!(...); }
            Ok(MergeWorkerResult {
                message: msg,
                auto_attached_parent: auto_attached,
            })
        }
        Err(msg) => Err(msg),  // 错误路径还是 String(前端不变)
    }
}
```

### 3.4 `app/src-tauri/src/tools/merge_worker.rs`

新增 helper:

```rust
/// Result: true = lazy attach 跑了;false = 父已 Active/Detached,不动
pub async fn ensure_parent_worktree_attached(
    db: &SqlitePool,
    parent_session_id: &str,
) -> Result<bool, String> {
    let loaded = db::load_session(db, parent_session_id).await
        .map_err(|e| format!("failed to load parent session: {}", e))?
        .ok_or_else(|| format!("parent session '{}' not found", parent_session_id))?;
    match loaded.session.worktree_state {
        db::WorktreeState::Active => Ok(false),       // INV-M2
        db::WorktreeState::Detached => Ok(false),     // INV-M3
        db::WorktreeState::None => {
            let project = db::get_project(db, &loaded.session.project_id).await
                .map_err(|e| format!("failed to load project: {}", e))?
                .ok_or_else(|| format!("project '{}' not found", loaded.session.project_id))?;
            crate::git::worktree::attach_session(db, &project, parent_session_id)
                .await
                .map_err(|e| e.to_string())?;
            Ok(true)                                    // INV-M4
        }
    }
}
```

`execute()` 改造:

```rust
pub async fn execute(...) -> (String, bool, ToolContextUpdate, Option<i32>) {
    // ... 解析 input / run_row 的现有代码不变 ...

    // ----- NEW: lazy attach parent -----
    let session_id_for_attach = parent_session_id.clone();
    match ensure_parent_worktree_attached(&ctx.db, &session_id_for_attach).await {
        Ok(true) => {
            tracing::info!(
                parent_session_id = %session_id_for_attach,
                run_id = %run_id,
                "merge_worker: auto-attached parent worktree for merge"
            );
            // 不影响返回值结构(tool tuple 不携带 auto-attach 信号给 LLM,
            // 没必要——LLM 行为本来就以 tool result 为准)
        }
        Ok(false) => {}
        Err(e) => {
            return (
                format!("merge_worker: cannot auto-attach parent worktree: {}", e),
                true,  // is_error
                ToolContextUpdate::default(),
                None,
            );
        }
    }

    // ----- NEW: 重新加载 parent_wt(因为 attach 后才落到真正的 worktree 路径) -----
    let parent_wt = match db::load_session(&ctx.db, &parent_session_id).await {
        Ok(Some(s)) => s.session.worktree_path
            .map(std::path::PathBuf::from)
            .ok_or_else(|| format!("parent worktree path missing after attach")),
        Ok(None) => Err(format!("parent session '{}' disappeared", parent_session_id)),
        Err(e) => Err(format!("failed to reload parent session: {}", e)),
    };
    let parent_wt = match parent_wt {
        Ok(p) => p,
        Err(e) => return (e, true, ToolContextUpdate::default(), None),
    };

    // ----- Stage 2: do_merge_blocking(原有) -----
    let parent_wt_for_task = parent_wt;
    let session_id_for_task = parent_session_id.clone();
    let run_id_for_task = run_id.clone();
    let merge_result = tokio::task::spawn_blocking(move || do_merge_blocking(
        &parent_wt_for_task, &session_id_for_task, &run_id_for_task,
    ))
    .await
    .unwrap_or_else(|e| Err(format!("merge_worker task panicked: {}", e)));

    // ... 原 match merge_result { Ok / Err / finalize_merge } 全部不变 ...
}
```

### 3.5 `app/src/stores/subagentRuns.ts`

```typescript
// subagentRuns.types.ts 新增:
export type MergeWorkerResult = {
  message: string;
  autoAttachedParent: boolean;
};

// subagentRuns.ts mergeWorker 改造:
async function mergeWorker(
  runId: string,
  parentSessionId: string,    // 新增参数(从 drawer 传进来)
): Promise<MergeResult> {
  if (mergeStateByRunId.has(runId)) {
    return { kind: "error", message: "another action is already in flight" };
  }
  mergeStateByRunId.set(runId, { kind: "merge", loading: true });
  try {
    const result = await invoke<MergeWorkerResult>("merge_worker_run", {
      rid: "merge-pr4",
      runId,
    });
    // 解析 conflict(从 result.message,与原逻辑一致)
    const conflictFiles = parseConflictFiles(result.message);
    if (conflictFiles !== null) {
      return { kind: "conflict", files: conflictFiles };
    }
    // success: 若 backend auto-attached 了父,刷 chat store
    if (result.autoAttachedParent) {
      const chatStore = useChatStore();
      await chatStore.refreshSession(parentSessionId);  // 新增 action,详见 §3.6
    }
    // 清缓存的 worker row(worktree_path → null),逻辑不变
    const row = getRunCache.get(runId);
    if (row) { getRunCache.set(runId, { ...row, worktreePath: null }); }
    return {
      kind: "success",
      autoAttachedParent: result.autoAttachedParent,  // 新增字段,UI 用
    };
  } catch (e) {
    const msg = String(e);
    const files = parseConflictFiles(msg);
    if (files !== null) return { kind: "conflict", files };
    return { kind: "error", message: msg };
  } finally {
    mergeStateByRunId.delete(runId);
  }
}
```

**前端 signature 变更**:`mergeWorker(runId, parentSessionId?)` 新增可选第二参;老的 `mergeWorker(runId)` 调用点(`WorkerMergeControls.vue`)必须传 `parentSessionId` 才能用上 auto-attach 刷新;不传则跳过 refresh(toast 仍然告知 auto-attach 成功,只是 chat header chip 不自动翻)。**这是有意识的兼容设计**,不破坏既有测试覆盖。

### 3.6 `app/src/stores/chat.ts` —— 新增 `refreshSession` action

```typescript
async function refreshSession(sessionId: string): Promise<void> {
  // 重新 load single session,从 DB 拉最新 row;更新 SessionSummary 缓存
  const row = await invoke<SessionRow | null>("get_session", { sessionId });
  if (!row) return;
  const summary = toSessionSummary(row);
  // 替换 Map 里的旧 entry
  sessions.value.set(sessionId, summary);
  // 若是 currentSession,currentSession.value 也更新
  if (currentSessionId.value === sessionId) currentSession.value = summary;
}
```

(注:若 `get_session` IPC 不存在则用现有的 list 接口查,具体看 `db::list_sessions_by_project`;若 chat store 已有 reload 路径则直接复用。)

### 3.7 `app/src/components/chat/WorkerMergeControls.vue`

```typescript
// 新增 prop
const props = defineProps<{
  runId: string;
  parentSessionId: string;   // 新增
}>();

// doMerge 改:
async function doMerge() {
  confirmKind.value = null;
  const result = await store.mergeWorker(props.runId, props.parentSessionId);
  if (result.kind === "success") {
    conflictFiles.value = null;
    const msg = result.autoAttachedParent
      ? "已合并到父 session 分支,并自动绑定了父工作区"
      : "已合并到 session 分支";
    projects.showToast(msg, "info");
  } else if (result.kind === "conflict") { ... 不变 ... }
  else { projects.showToast(`合并失败: ${result.message}`, "error"); }
}
```

**父 Session ID 从哪儿来**:`WorkerMergeControls` 的父(SubagentDrawer)从 `run.parentSessionId` 取(已在 `subagent_runs` 行里有这个字段,无需新查)。

---

## 4. IPC 协议变更总结

| 接口 | Before | After |
|---|---|---|
| `merge_worker_run(...)` 返回 | `Result<String, String>` (libgit2 merge message / error) | `Result<MergeWorkerResult, String>` |
| 错误文案 | 不变 | 不变 |
| 错误前缀(`Err(String)` 的内容) | `parent session has no worktree` / libgit2 conflict 等 | 新增 `merge_worker_run: cannot auto-attach parent worktree: <reason>`(在父 dirty/non-git 且 lazy attach 失败时) |
| `merge_worker` tool `(String, bool, ToolContextUpdate, None)` 元组 | 不变 | 不变(LLM 视角无 auto-attach 信号 — 没必要) |
| `attach_worktree(...)` 返回 | `Result<SessionRow, String>` | **不变** |

兼容性: `MergeWorkerResult` 是 Tauri IPC 自动 serde → camelCase,前端 TS 类型同步升级。所有现有 `merge_worker_run` 调用点都集中在 `subagentRuns.ts::mergeWorker`,本 PR 同步升级 → 编译期保证一致。

---

## 5. 为什么是 lazy-attach-on-merge,不是别的

候选方案对比(决策记录):

| 方案 | 改 dispatch? | 改 create_session? | 改 merge? | 用户感知 | 风险面 |
|---|---|---|---|---|---|
| **A. lazy auto-attach on merge**(本 PR) | ❌ | ❌ | ✅ | 用户点 merge,系统顺手把父 attach 了 → toast 告知 | 低:只动两个 merge 入口 + 一个 helper 抽出 |
| B. 自动 attach 在 dispatch 第一个 isolated worker 时 | ✅ | ❌ | ❌ | sub-agent 一启动父就被 attach,**用户感知的副作用**比 merge 时更早 | 高:**隐晦改了 dispatch 语义**(本应是"fork worker")、线程模型影响大、LLM context 也会更早看到 attached 状态(可能误以为可以写父) |
| C. 纯 UX 提醒 + 明确错误文案 | ❌ | ❌ | ❌ | 用户必须主动 attach 后才能 merge,体验差 | 0 行代码风险,但**漏 99% 的普通用户** |
| D. 默认 session 创建就 attach | ❌ | ✅(推翻 Step4 opt-in 决策) | ❌ | 用户感知:session 一建就有 worktree | 极高:**推翻 Step4 follow-up 的 opt-in 决定**,要回到 spec 历史讨论 |

**结论**: A 的改动面小、风险低、对用户友好(自动 + 透明告知),B/D 改动不可逆决策面太大,C 不解决实际问题。选 A。

**第 6 问:为什么 IPC 返回值要带回 `auto_attached_parent` 信号**
- 候选:`tracing::info!` 只 + 后端日志
- 否决:UI 必须给用户明示这个副作用("我替你 attach 了父"),不能只在服务端日志里有。结构化返回值是最小侵入的告知通道。

---

## 6. 回滚 / 风险与对策

### 6.1 回滚策略

- `attach_session` helper 是新代码,抽离自原 `attach_worktree`,可独立 revert 而不影响 IPC contract(因为 `attach_worktree` 改成调它)。
- `merge_worker_run` 的 `MergeWorkerResult` 返回值引入是 strict 升级:旧前端代码调用 `mergeWorker(runId)` 第二参数 undefined → 走旧路径(`autoAttachedParent` 是 undefined → 不 refresh,但不影响 success 本身)。
- 整体 revert 一个 commit 即可,无 schema migration / no DB 列新增。

### 6.2 风险面

| 风险 | 影响 | 对策 |
|---|---|---|
| 项目脏 + 用户点 merge → 弹"uncommitted changes" toast | 用户需要先去项目根 commit/stash 后再 merge | 错误文案透传原 `attach_worktree` 文案,用户能识别;不强行 attach → 不破坏工作流 |
| 父 session detached 状态 → lazy attach 不重 attach → merge 仍失败 | 用户必须**主动** attach 才能 merge | 故意(尊重用户 detach 意图,见 INV-M3);UI 给通用 toast,提示用户走 chat header attach |
| 自动 attach 在并发 LL M 调用时 race(用户在主 session 聊天,sub-agent drawer 点 merge 同时发生) | 父 attach 与 LLM chat 写盘竞争 | 复用现有 `cancel_inflight_for_session` 保护:`merge_worker_run` 进入时同样 cancel 父 session 的 in-flight chat(若 attach 路径走 `attach_worktree` 内部的 IPC 流程,因为本 helper 不是 IPC,所以**该保护不自动生效** → 加显式调用,**见 §6.3 修订**) |
| `parseConflictFiles` 在结构化返回值下被遗漏 | conflict path 走 error 而不是 conflict kind,UI 不展示文件列表 | **强制约束**: `mergeWorker` store 在 success 分支也要 `parseConflictFiles(result.message)`,逻辑与 catch 分支相同。两个分支都解析 |

### 6.3 修订:in-flight cancel hook 要加在 helper 内

发现: `commands::worktree::attach_worktree` 命令体内置 `cancel_inflight_for_session(...)` 保护(IPC 边界)。本 helper 是 free function,没接 `AppState` 拿不到 `cancellations` / `session_active_request` 字段。

**对策**: helper 改成显式接 `CancelHookArgs` (一个 `&CancellationRegistry`),且调用方传入:

```rust
pub struct AttachCancelHook<'a> {
    pub cancellations: &'a Arc<Mutex<HashMap<String, CancellationToken>>>,
    pub session_active_request: &'a Arc<Mutex<HashMap<String, String>>>,
}

pub async fn attach_session(
    db: &SqlitePool,
    project: &ProjectRow,
    session_id: &str,
    hook: Option<&AttachCancelHook>,
) -> Result<PathBuf, GitError>
```

- `attach_worktree` 命令体传 `Some(hook)`(拿到 AppState 的字段)
- `ensure_parent_worktree_attached` 在 `merge_worker` tool 路径下从 `ctx` 取(检查 ctx 是否携带该字段,**当前 ctx 设计可能没有**——见 §6.4)
- `merge_worker_run` IPC 路径下传 `Some(hook)`

**第 6.4 子项**:`merge_worker` tool 的 `ctx` 是否有 cancel hook?(待 implement 阶段确认。如果 ctx 类型没携带,两条路:
- 加 ctx 字段(侵入大,影响所有 tool +60+ tests)
- skip cancel hook(tool 层 merge_worker 跟 in-flight LLM 冲突概率低,LLM 自己在 merge_worker 之内,主 chat 不会同时跑同一个 parent session 的 LLM 因为 `session_active_request` 是 per-session 互斥 —— **scenario 实际不发生**,可以 skip

决定: **skip cancel hook on tool path,只在 IPC(`merge_worker_run`)路径 cancel**。理由:
1. 工具路径下,在 `ensure_parent_worktree_attached` 内的 helper,被自己这个 LLM turn 触发,主 chat 在 parent session 上同一时间根本不是 in-flight 的(LLM turn 是串行的,一条 message 一次 RPC)。
2. 若用户切到 chat 标签页发新消息,对应的是新一轮 `chat` spawn,`session_active_request` 上挂的是新一轮的 token —— 跟正在跑的 tool spawn 不冲突,新的一轮 chat 可以正常 cancel 自己(它的 token 不影响 tool 的执行)。
3. 唯一 race 是用户在 tool 还在跑时切到 chat 标签发了消息 + merge in-flight:这时新 chat 会被 cancel(走它自己的 token),不影响 tool。
4. 因此 tool 路径下 attach 不需要 cancel hook,IPC 路径下 attach 跟用户手动 attach 行为对齐 —— 走 `attach_worktree` 命令等价保护。

**helper signature 最终**:`attach_session(db, project, session_id)` —— 无 cancel hook 参数(IPC `attach_worktree` 外部调它之前自行 cancel,tool 路径不需要)。

---

## 7. Spec 更新位置

### 7.1 `.trellis/spec/backend/worktree-contract.md`

在 §Pattern: Worker Worktree Sweep 之后加新章节:

```
## Pattern: Lazy Auto-Attach on Merge (06-30 follow-up)

Invariants:
- merge_worker_run + merge_worker::execute both call
  ensure_parent_worktree_attached before spawn_blocking
  (single source of truth: implement §3.4)
- Helper skips Active (INV-M2) and Detached (INV-M3); only
  fires on None
- Detached skipped to respect user intent (INV-M3)
- Side effects of attach_session: writes worktree_state=active,
  injects [worktree event] attached system event (same path as
  attach_worktree IPC)
- Concurrent cancellation: IPC attach_worktree keeps its
  cancel hook; helper does NOT duplicate (tool path doesn't
  need it — see design §6.4)

Error matrix:
| Condition | Result |
|-----------|--------|
| parent Active | helper no-op, no DB write, no event |
| parent Detached | helper no-op (no re-attach) |
| parent None + project non-git | propagates "not a git repository" |
| parent None + dirty project root | propagates "uncommitted changes" |
| parent None + disk write fails | propagates libgit2 error verbatim |
| IPC E string format | "merge_worker_run: cannot auto-attach parent worktree: <reason>" |
| tool tuple | (String, true, ToolContextUpdate::default(), None) |
```

### 7.2 `.trellis/spec/backend/tool-contract.md`

`merge_worker` 条目合约段加一行:

```
- 若父 session 未绑 worktree,调用方 helper 自动 attach
  (session/<id>) 并注入 [worktree event] attached
  系统消息(详细见 worktree-contract.md Pattern §Lazy
  Auto-Attach on Merge)
```

---

## 8. 测试矩阵(契约层)

(详细 acceptance 见 prd.md,这里只列结构化覆盖)

| 层 | 测试 | 路径 |
|---|---|---|
| Backend unit | `attach_session` happy / 已 Active reject / Detached reject / non-git / dirty | `git/worktree.rs::tests` |
| Backend unit | `ensure_parent_worktree_attached` 三态分支 | `tools/merge_worker.rs::tests` 新增 |
| Backend unit | `attach_worktree` IPC entry 不变 | `commands/worktree.rs::tests` 若有;无则新增 |
| Frontend vitest | `mergeWorker` 处理 `autoAttachedParent=true/false` 两条分支 | `subagentRuns.test.ts` 已有块 |
| Frontend vitest | `parseConflictFiles` 在新返回值下仍工作 | 已存在测试,不应回归 |
| Spec review | `worktree-contract.md` + `tool-contract.md` 双锁 | 实现完成后 review |
