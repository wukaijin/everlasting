//! N2 PR2 (2026-09-20, task `09-20-n2-checkpoint-revert`): the
//! read-side checkpoint IPC surface — the turn-diff viewing half of
//! R4 (「本轮 diff」). Two commands, dual-registered Tauri + daemon
//! route (Q0 单源:routes thin-wrap these `_inner`s):
//!
//! - [`list_turn_checkpoints`] — the per-session snapshot list with
//!   `prev_seq` (the *previous existing row*, NOT literal `seq - 1` —
//!   the write-signal gate makes the chain sparse) and a
//!   `files_changed` badge count. The frontend derives the TurnCard
//!   「本轮 diff」 entry visibility from this list.
//! - [`get_turn_checkpoint_diff`] — `diff_snapshots(prev_seq → seq)`
//!   for one turn, reusing the `diff_worktree` wire shapes
//!   (`DiffResult` / `FileDiff`) so the existing `DiffView` renders
//!   it unchanged.
//!
//! ## Degradation contract (design §5, review-corrected)
//!
//! Every failure that is NOT a bug collapses into one of two typed
//! error kinds (the `AppCommandError.kind` string — the frontend's
//! routing key):
//!
//! - `CheckpointsUnavailable` — the capability doesn't apply: non-git
//!   project, group-chat session, session with no checkpoint rows, or
//!   a diff request against the baseline row (baseline = the state
//!   BEFORE the session; diffing it against an empty tree would list
//!   the whole repo as additions). The frontend hides the entry — no
//!   error surfaces to the user.
//! - `CheckpointBroken` — DB rows exist but their git objects are
//!   gone (user ran `git gc` with the umbrella ref stripped, manual
//!   ref cleanup). Detected at lookup time: any commit/tree/object
//!   resolution failure degrades the WHOLE session (list errors, so
//!   the entry hides everywhere). `warn!`-logged, never panics, never
//!   half-renders.
//!
//! Session-not-found stays a plain `InvalidRequest` (matches
//! `diff_worktree`'s shape).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::db;
use crate::error::{AppCommandError, ErrorCategory};
use crate::git::checkpoint as git_ckpt;
use crate::state::AppState;

/// One entry of [`list_turn_checkpoints`] — one snapshot row with its
/// chain context. Wire shape is snake_case (default serde), consistent
/// with the `DiffResult`/`FileDiff` shapes the same UI consumes.
#[derive(Debug, Clone, Serialize)]
pub struct TurnCheckpointSummary {
    /// The snapshot's seq (turn-end assistant row `messages.seq`;
    /// the baseline row sits on the first user row's seq).
    pub seq: i64,
    /// The PREVIOUS EXISTING row's seq (sparse-chain semantics: the
    /// write-signal gate leaves gaps, so this is "the row before me
    /// in the chain", not `seq - 1`). `None` on the baseline row —
    /// the baseline has no diff by definition.
    pub prev_seq: Option<i64>,
    /// Unix milliseconds (the `turn_checkpoints.created_at` column).
    pub created_at: i64,
    /// Changed-path count between this snapshot and the previous
    /// one. 0 for the baseline row AND for net-zero write turns
    /// (design §4: 净效果为零的写轮仍落行,「本轮 diff」显示为空).
    pub files_changed: usize,
}

// ---------------------------------------------------------------------------
// Typed degradation errors
// ---------------------------------------------------------------------------

/// Capability-doesn't-apply (non-git / group_chat / no rows / baseline
/// diff request). The frontend reads `kind` and hides the entry; the
/// global error bus treats InvalidRequest as console-only, so this
/// never toasts.
fn checkpoints_unavailable(msg: impl Into<String>) -> AppCommandError {
    AppCommandError {
        category: ErrorCategory::InvalidRequest,
        kind: "CheckpointsUnavailable".to_string(),
        message: msg.into(),
        retryable: false,
        request_id: None,
    }
}

/// Broken chain: DB rows point at git objects that no longer resolve.
/// Warn-logged at the decision site (查得到 oid 失败即判) so the
/// operator can correlate with a user-side `git gc` / ref cleanup.
fn checkpoint_broken(msg: impl Into<String>) -> AppCommandError {
    let message = msg.into();
    tracing::warn!(
        kind = "CheckpointBroken",
        reason = %message,
        "checkpoint chain broken: DB rows reference missing git objects"
    );
    AppCommandError {
        category: ErrorCategory::InvalidRequest,
        kind: "CheckpointBroken".to_string(),
        message,
        retryable: false,
        request_id: None,
    }
}

// ---------------------------------------------------------------------------
// Session → repo-path resolution (the read-side twin of the loop's
// session_root: worktree path when bound, project path otherwise)
// ---------------------------------------------------------------------------

struct CheckpointCtx {
    /// Repo path the chain's objects live under: the session-bound
    /// worktree when attached, else the project main path. (Umbrella
    /// refs and snapshot objects land in the SHARED common store, so
    /// either handle reads the same chain — this mirrors where the
    /// loop WRITES from, `chat_loop.rs`'s `worktree_path`.)
    repo_path: PathBuf,
}

async fn resolve_ctx(
    state: &Arc<AppState>,
    session_id: &str,
) -> Result<CheckpointCtx, AppCommandError> {
    let loaded = db::load_session(&state.db, session_id)
        .await
        .map_err(|e| anyhow::anyhow!("checkpoint: failed to load session: {}", e))?
        .ok_or_else(|| {
            AppCommandError::new(
                ErrorCategory::InvalidRequest,
                format!("checkpoint: session '{}' not found", session_id),
            )
        })?;
    // 群聊 session 不建链(PRD R5)—— 无论行是否存在,能力不可用。
    if loaded.session.session_type == db::SessionType::GroupChat {
        return Err(checkpoints_unavailable(
            "checkpoint: group-chat sessions have no snapshot chain",
        ));
    }
    let project = db::get_project(&state.db, &loaded.session.project_id)
        .await
        .map_err(|e| anyhow::anyhow!("checkpoint: failed to load project: {}", e))?
        .ok_or_else(|| checkpoints_unavailable("checkpoint: session's project no longer exists"))?;
    // 非 git 项目:快照对象无处安放(PRD R5)。
    if !project.is_git_repo {
        return Err(checkpoints_unavailable(
            "checkpoint: project is not a git repository",
        ));
    }
    let repo_path = match loaded.session.worktree_path.as_deref() {
        Some(p) if !p.trim().is_empty() => PathBuf::from(p),
        // None / detached:the session works out of the project root
        // (same resolution as the loop's `session_root_raw`).
        _ => PathBuf::from(&project.path),
    };
    Ok(CheckpointCtx { repo_path })
}

// ---------------------------------------------------------------------------
// Shared git phase (sync block — the repo handle never crosses .await)
// ---------------------------------------------------------------------------

/// Resolve one row's commit and return its tree oid. Any lookup
/// failure = broken chain (the row's contract is "this commit exists
/// and carries this tree").
fn resolve_row_tree(
    repo: &git2::Repository,
    row: &db::checkpoint::CheckpointRow,
) -> Result<git2::Oid, AppCommandError> {
    let commit_oid = git2::Oid::from_str(&row.commit_sha)
        .map_err(|e| checkpoint_broken(format!("checkpoint commit sha 无效: {}", e)))?;
    let commit = repo
        .find_commit(commit_oid)
        .map_err(|e| checkpoint_broken(format!("checkpoint commit 对象缺失: {}", e)))?;
    let tree = commit
        .tree()
        .map_err(|e| checkpoint_broken(format!("checkpoint tree 对象缺失: {}", e)))?;
    Ok(tree.id())
}

fn open_repo(path: &Path) -> Result<git2::Repository, AppCommandError> {
    git2::Repository::open(path)
        .map_err(|e| checkpoint_broken(format!("checkpoint: repo open failed: {}", e)))
}

// ---------------------------------------------------------------------------
// list_turn_checkpoints
// ---------------------------------------------------------------------------

pub async fn list_turn_checkpoints_inner(
    state: &Arc<AppState>,
    session_id: String,
) -> Result<Vec<TurnCheckpointSummary>, AppCommandError> {
    let ctx = resolve_ctx(state, &session_id).await?;
    let rows = db::checkpoint::list_checkpoints(&state.db, &session_id)
        .await
        .map_err(|e| anyhow::anyhow!("list_turn_checkpoints: db failed: {}", e))?;
    // 无行 = 能力不可用(评审补降级:旧 session / 只读 session,入口
    // 隐藏而非报错)。空列表理论上也由前端兜底(双保险)。
    if rows.is_empty() {
        return Err(checkpoints_unavailable(
            "checkpoint: session has no checkpoint rows",
        ));
    }

    // prev_seq + files_changed 需要仓库句柄;git2::Repository 非 Send,
    // 整段同步完成后才回到 async 世界(与 agent/checkpoint.rs 的
    // phase-split 同一纪律)。
    let rows_ref = &rows;
    let summaries: Vec<TurnCheckpointSummary> = {
        let repo = open_repo(&ctx.repo_path)?;
        let mut out: Vec<TurnCheckpointSummary> = Vec::with_capacity(rows_ref.len());
        // prev = 列表内前一行(list_checkpoints 按 seq ASC 回传,
        // 写触发门稀疏链下就是「前一个存在行」的单趟语义,设计 §5
        // 评审修正的 literal-seq-1 反例由此消解)。
        let mut prev: Option<&db::checkpoint::CheckpointRow> = None;
        for row in rows_ref {
            let tree = resolve_row_tree(&repo, row)?;
            let files_changed = match prev {
                None => 0,
                Some(p) => {
                    let prev_tree = resolve_row_tree(&repo, p)?;
                    git_ckpt::count_snapshot_deltas(&repo, prev_tree, tree).map_err(|e| {
                        checkpoint_broken(format!("checkpoint chain diff failed: {}", e))
                    })?
                }
            };
            out.push(TurnCheckpointSummary {
                seq: row.seq,
                prev_seq: prev.map(|r| r.seq),
                created_at: row.created_at,
                files_changed,
            });
            prev = Some(row);
        }
        out
    };
    Ok(summaries)
}

#[tauri::command]
pub async fn list_turn_checkpoints(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Vec<TurnCheckpointSummary>, AppCommandError> {
    list_turn_checkpoints_inner(&state, session_id).await
}

// ---------------------------------------------------------------------------
// get_turn_checkpoint_diff
// ---------------------------------------------------------------------------

pub async fn get_turn_checkpoint_diff_inner(
    state: &Arc<AppState>,
    session_id: String,
    seq: i64,
) -> Result<crate::git::diff::DiffResult, AppCommandError> {
    let ctx = resolve_ctx(state, &session_id).await?;
    let rows = db::checkpoint::list_checkpoints(&state.db, &session_id)
        .await
        .map_err(|e| anyhow::anyhow!("get_turn_checkpoint_diff: db failed: {}", e))?;
    let idx = rows.iter().position(|r| r.seq == seq).ok_or_else(|| {
        checkpoints_unavailable(format!("checkpoint: no snapshot row for seq {}", seq))
    })?;
    // 基线行(seq 区间的第一行)没有 diff 入口:diff 它 vs 空树会把
    // 整仓列成新增(设计 §5)。前端从 list 的 prev_seq=None 已隐藏;
    // 这里是纵深防御 —— 直接请求同样返回 Unavailable 类语义。
    if idx == 0 {
        return Err(checkpoints_unavailable(
            "checkpoint: the baseline row has no turn diff (it IS the pre-session state)",
        ));
    }
    let prev = &rows[idx - 1];
    let row = &rows[idx];

    let result = {
        let repo = open_repo(&ctx.repo_path)?;
        let a_tree = resolve_row_tree(&repo, prev)?;
        let b_tree = resolve_row_tree(&repo, row)?;
        git_ckpt::diff_snapshots(&repo, a_tree, b_tree)
            .map_err(|e| checkpoint_broken(format!("checkpoint snapshot diff failed: {}", e)))?
    };
    Ok(result)
}

#[tauri::command]
pub async fn get_turn_checkpoint_diff(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    seq: i64,
) -> Result<crate::git::diff::DiffResult, AppCommandError> {
    get_turn_checkpoint_diff_inner(&state, session_id, seq).await
}

// ---------------------------------------------------------------------------
// 命令层测试(design §5 降级矩阵 + prev_seq 稀疏链)
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests_checkpoint.rs"]
mod tests_checkpoint;
