//! N2 checkpoint wiring layer (task `09-20-n2-checkpoint-revert`):
//! everything between the agent loop and the pure `git::checkpoint`
//! primitives — the config gate, per-turn write-signal gate,
//! session→repo path resolution, DB row writes, and the session-delete
//! umbrella-ref cleanup.
//!
//! ## Hook points (design §4, one deliberate placement note)
//!
//! - **Baseline at loop entry**: fired once per `run_chat_loop` from
//!   the hub (`chat_loop.rs`) right after `prepare_loop_state` — the
//!   turn's user row is already persisted, no tool has executed. If
//!   the session has no baseline row, one is built at the FIRST user
//!   row's seq so "revert to baseline = back to before the session"
//!   stays reachable even when the first turn writes (review P0).
//! - **Turn-end snapshot**: fired after the turn's TOOLS HAVE RUN and
//!   their results are persisted (the hub, right after `finalize_turn`
//!   succeeds). The design text anchors the hook on
//!   `db::finalize_turn_persist` (the assistant-row persist inside
//!   `drive_turn`), but that point runs BEFORE the turn's own tool
//!   dispatch — a snapshot taken there would (a) shift every snapshot
//!   one turn late (AC2/AC3 attribution) and (b) never fire for the
//!   loop's final write turn, which ends on a text-only assistant
//!   turn. The hub point is the same turn boundary with the tools'
//!   net effect already on disk; "写轮恒有行" and AC8 (当轮 merge 当轮
//!   收编) only hold there. The checkpoint `seq` is that turn's
//!   assistant row `messages.seq` as specified.
//!
//! ## Fail-open contract
//!
//! Every hook is best-effort: any error (config read, repo open,
//! `build_state_tree` — including the EUNMERGED conflicted-index
//! case —, append, ref, DB row) logs `warn!` with the session id and
//! the turn finishes normally. A failed turn-end snapshot simply
//! leaves a gap; the next successful snapshot re-parents onto the
//! latest surviving DB row ("链 parent 跳接上一可用快照").
//!
//! Layering: this module may import db + git; `git/checkpoint.rs`
//! must never import db/daemon.

use std::path::Path;

use sqlx::SqlitePool;

use crate::db;
use crate::git::checkpoint as git_ckpt;

/// app_config key for the checkpoint kill switch. Stored `"true"` /
/// `"false"` literals by `set_app_config_flag`; read fail-open — only
/// the literal `"false"` disables (default ON, design §7), mirroring
/// `sandbox::policy::sandbox_enabled`'s single-source read.
pub const CHECKPOINTS_ENABLED_KEY: &str = "checkpoints_enabled";

/// Kill-switch read (fail-open): only the literal `"false"` disables.
pub async fn checkpoints_enabled(db: &SqlitePool) -> bool {
    match crate::db::config::get_config_value(db, CHECKPOINTS_ENABLED_KEY).await {
        Ok(Some(v)) => v != "false",
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// Per-turn write-signal gate (design §4)
// ---------------------------------------------------------------------------

/// The four write-signal sources for one turn, as a flat bundle. The
/// gate fires a turn-end snapshot iff [`WriteSignals::any`] is true
/// (or the session has no baseline yet — a failed baseline hook gets
/// repaired by the next turn-end).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteSignals {
    /// A write-family tool was dispatched this turn
    /// (`write_file` / `edit_file` — the registry's mutating path
    /// tools, per `permissions::check::permission::classify_tool`).
    pub write_tool: bool,
    /// A foreground `shell` command classified as writing by the A2+
    /// trust layer (`shell_trust::classify_prefix` ≠ ReadOnly —
    /// SideEffect covers `mkdir`/`cargo build`/`git push`/redirects,
    /// Ask covers unknown commands; both are conservative "may have
    /// written" signals, and a signal surplus only costs one
    /// dedupe-able scan while a signal deficit loses a snapshot).
    pub shell_write: bool,
    /// Background-shell completion events were drained this turn
    /// (L1 async writes — completions may land any time between
    /// turns, so the drain in `drive_turn` is the observable).
    pub background_shell_done: bool,
    /// A `merge_worker` was dispatched this turn (worker产物收编 —
    /// the merge lands in the host workdir during dispatch; a failed
    /// merge still counts, and its snapshot dedupes to the same tree
    /// at the cost of one row, matching「净效果为零的写轮仍落行」).
    pub worker_merge: bool,
}

impl WriteSignals {
    /// Classify one turn's dispatched tool calls into the three
    /// tool-derived signals (the drain flag is OR-ed in by the hub,
    /// which owns the `drive_turn` outcome).
    pub fn for_tool_calls(tool_calls: &[(String, String, serde_json::Value)]) -> WriteSignals {
        let mut s = WriteSignals::default();
        for (_id, name, input) in tool_calls {
            match name.as_str() {
                "write_file" | "edit_file" => s.write_tool = true,
                "shell" => {
                    let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
                    if crate::agent::permissions::shell_trust::classify_prefix(cmd)
                        != crate::agent::permissions::shell_trust::ShellTrust::ReadOnly
                    {
                        s.shell_write = true;
                    }
                }
                "merge_worker" => s.worker_merge = true,
                _ => {}
            }
        }
        s
    }

    /// Fold in the background-shell drain flag from `drive_turn`.
    pub fn with_background(mut self, background_shell_done: bool) -> WriteSignals {
        self.background_shell_done = background_shell_done;
        self
    }

    fn any(self) -> bool {
        self.write_tool || self.shell_write || self.background_shell_done || self.worker_merge
    }
}

// ---------------------------------------------------------------------------
// Hooks (all best-effort — warn + swallow, never fail the turn)
// ---------------------------------------------------------------------------

/// Loop-entry baseline hook. Builds the session's baseline snapshot at
/// `baseline_seq` (the first user row's seq) when the session has no
/// checkpoint rows yet; a no-op otherwise. `repo_path` is the loop's
/// session root (worktree path when attached, project path otherwise —
/// the same directory the tools write into).
pub(crate) async fn ensure_turn_baseline(
    db: &SqlitePool,
    session_id: &str,
    repo_path: &Path,
    baseline_seq: i64,
) {
    match db::checkpoint::has_checkpoint_baseline(db, session_id).await {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            tracing::warn!(
                error = %e,
                session_id = %session_id,
                "checkpoint: baseline probe failed, skipping baseline (non-fatal)"
            );
            return;
        }
    }
    capture_and_record(db, session_id, repo_path, baseline_seq, "baseline").await;
}

/// Turn-end snapshot hook with the write-signal gate. Called after the
/// turn's tools ran and their results persisted; `seq` is the turn's
/// assistant row seq. A signal-less turn with an existing baseline
/// skips entirely (zero scans, zero rows — the AC9 property).
pub(crate) async fn snapshot_turn_if_written(
    db: &SqlitePool,
    session_id: &str,
    repo_path: &Path,
    seq: i64,
    signals: WriteSignals,
) {
    let latest = match db::checkpoint::latest_checkpoint(db, session_id).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(
                error = %e,
                session_id = %session_id,
                "checkpoint: latest probe failed, skipping turn snapshot (non-fatal)"
            );
            return;
        }
    };
    if latest.is_some() && !signals.any() {
        // 零扫描零行:无写信号且链已建立(基线在),本轮不产生任何 git
        // 或 DB 动作(design §4 写触发门;AC9 的成本侧)。
        return;
    }
    capture_and_record(db, session_id, repo_path, seq, "turn").await;
}

/// Shared snapshot core: capture the state tree, dedupe against the
/// latest DB row, append the dangling commit, move the umbrella ref,
/// and upsert the row. Any error aborts with a warn (fail-open).
///
/// Send-shape: `git2::Repository` is NOT `Sync`, and the loop future
/// is `tokio::spawn`'d — so the repo handle is opened inside each
/// synchronous git phase and never held across an `.await` (the
/// reopen cost is a sub-ms stat walk, same order as the per-turn
/// `lookup_head_sha`).
async fn capture_and_record(
    db: &SqlitePool,
    session_id: &str,
    repo_path: &Path,
    seq: i64,
    context: &str,
) {
    if let Err(e) = capture_and_record_inner(db, session_id, repo_path, seq).await {
        tracing::warn!(
            error = %e,
            session_id = %session_id,
            seq,
            phase = %context,
            "checkpoint: snapshot failed, turn continues without one (fail-open)"
        );
    }
}

async fn capture_and_record_inner(
    db: &SqlitePool,
    session_id: &str,
    repo_path: &Path,
    seq: i64,
) -> Result<(), HookError> {
    // Phase 1 (sync): capture the state tree. Errors here include the
    // EUNMERGED conflict state — the turn continues without a
    // snapshot and the chain resumes at the next clean one.
    let tree_str = {
        let repo = open_repo(repo_path)?;
        git_ckpt::build_state_tree(&repo)
            .map_err(HookError::Git)?
            .to_string()
    };

    let latest = db::checkpoint::latest_checkpoint(db, session_id)
        .await
        .map_err(HookError::Db)?;

    // Content-addressed dedupe: an unchanged state reuses the latest
    // commit (zero new objects); the row is still written (写轮恒有行).
    let commit_sha = match &latest {
        Some(row) if row.tree_sha == tree_str => row.commit_sha.clone(),
        latest => {
            let parent = match latest {
                Some(row) => Some(
                    git2::Oid::from_str(&row.commit_sha).map_err(|e| HookError::Git(e.into()))?,
                ),
                None => None,
            };
            // Phase 2 (sync): append the dangling commit + move the
            // umbrella ref.
            let commit = {
                let repo = open_repo(repo_path)?;
                let commit = git_ckpt::append_snapshot(
                    &repo,
                    parent,
                    git2::Oid::from_str(&tree_str).map_err(|e| HookError::Git(e.into()))?,
                    session_id,
                    seq as u64,
                )
                .map_err(HookError::Git)?;
                git_ckpt::set_umbrella_ref(&repo, session_id, commit).map_err(HookError::Git)?;
                commit
            };
            commit.to_string()
        }
    };

    db::checkpoint::upsert_checkpoint(db, session_id, seq, &tree_str, &commit_sha)
        .await
        .map_err(HookError::Db)?;
    Ok(())
}

fn open_repo(repo_path: &Path) -> Result<git2::Repository, HookError> {
    git2::Repository::open(repo_path).map_err(|e| HookError::Git(e.into()))
}

/// Internal error aggregate for the fail-open boundary: both failure
/// families collapse into one `warn!` with context.
#[derive(Debug)]
enum HookError {
    Git(crate::git::error::GitError),
    Db(sqlx::Error),
}

impl std::fmt::Display for HookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HookError::Git(e) => write!(f, "git: {e}"),
            HookError::Db(e) => write!(f, "db: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Session-delete umbrella-ref cleanup (design §7 / AC6)
// ---------------------------------------------------------------------------

/// Delete the session's umbrella ref, trying the session-bound repo
/// path first and ALWAYS falling back to the project main repo before
/// giving up. The review-corrected contract: worktree refs physically
/// land in the shared common-refs store (the main repo's
/// `.git/refs/everlasting/`), so a detached/deleted worktree whose
/// path no longer opens must not silently skip the cleanup — that
/// would leak the ref forever and break AC6's GC reclaim. Both paths
/// failing skips with a warn (best-effort; the DB rows are cleaned by
/// the FK CASCADE in `delete_session` regardless).
///
/// Returns `true` when a path resolved and the (idempotent) ref
/// deletion succeeded — including "no ref existed".
pub fn cleanup_umbrella_refs_best_effort(
    session: &db::SessionRow,
    project_path: &str,
    session_id: &str,
) -> bool {
    // Path 1: the session-bound repo (attached worktree). Detached
    // sessions have no `worktree_path` — the main-repo fallback below
    // is their primary (and correct) target.
    let bound = match session.worktree_state {
        db::WorktreeState::Active => session.worktree_path.as_deref(),
        _ => None,
    };
    if let Some(wt) = bound {
        if try_delete_ref(Path::new(wt), session_id) {
            return true;
        }
        tracing::warn!(
            session_id = %session_id,
            worktree = %wt,
            "checkpoint: umbrella-ref delete via worktree failed, retrying main repo"
        );
    }
    // Path 2 (恒试): the project main repository — where the shared
    // common refs physically live.
    if project_path.trim().is_empty() {
        tracing::warn!(
            session_id = %session_id,
            "checkpoint: umbrella-ref cleanup skipped (no resolvable repo path)"
        );
        return false;
    }
    if try_delete_ref(Path::new(project_path), session_id) {
        return true;
    }
    tracing::warn!(
        session_id = %session_id,
        project = %project_path,
        "checkpoint: umbrella-ref delete failed on both worktree and main repo (ref may leak)"
    );
    false
}

/// One open + idempotent delete attempt. `false` = open or delete
/// error (the caller logs and falls back).
fn try_delete_ref(repo_path: &Path, session_id: &str) -> bool {
    match git2::Repository::open(repo_path) {
        Ok(repo) => match git_ckpt::delete_umbrella_ref(&repo, session_id) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    session_id = %session_id,
                    repo = %repo_path.display(),
                    "checkpoint: delete_umbrella_ref failed"
                );
                false
            }
        },
        Err(e) => {
            tracing::warn!(
                error = %e,
                session_id = %session_id,
                repo = %repo_path.display(),
                "checkpoint: repo open failed for umbrella-ref cleanup"
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // WriteSignals classification (the write-trigger gate's unit face)
    // ------------------------------------------------------------------

    fn call(name: &str, input: serde_json::Value) -> (String, String, serde_json::Value) {
        ("toolu_1".to_string(), name.to_string(), input)
    }

    #[test]
    fn write_family_tools_signal() {
        let s = WriteSignals::for_tool_calls(&[
            call(
                "write_file",
                serde_json::json!({"path": "a.txt", "content": "x"}),
            ),
            call("edit_file", serde_json::json!({"path": "b.txt"})),
        ]);
        assert!(s.write_tool);
        assert!(!s.shell_write && !s.worker_merge);
        assert!(s.any());
    }

    #[test]
    fn readonly_shell_does_not_signal_but_write_shell_does() {
        let ro = WriteSignals::for_tool_calls(&[call(
            "shell",
            serde_json::json!({"command": "git diff"}),
        )]);
        assert!(!ro.any(), "纯只读 shell 不得触发写信号(AC9 前提)");

        // 写重定向 → SideEffect;未知命令 → Ask;两者都算「判写」。
        for cmd in ["echo x > out.txt", "nonsense-cmd-xyz", "rm -rf /"] {
            let s =
                WriteSignals::for_tool_calls(&[call("shell", serde_json::json!({"command": cmd}))]);
            assert!(s.shell_write, "{cmd} must signal");
            assert!(!s.write_tool && !s.worker_merge);
        }
    }

    #[test]
    fn merge_worker_signals_and_background_folds_in() {
        let s = WriteSignals::for_tool_calls(&[call(
            "merge_worker",
            serde_json::json!({"run_id": "r1"}),
        )]);
        assert!(s.worker_merge && !s.write_tool && !s.shell_write);

        let drained = WriteSignals::default().with_background(true);
        assert!(drained.background_shell_done && drained.any());
        assert!(!WriteSignals::default().any());
    }

    #[test]
    fn read_only_tools_never_signal() {
        let s = WriteSignals::for_tool_calls(&[
            call("read_file", serde_json::json!({"path": "a.txt"})),
            call("grep", serde_json::json!({"pattern": "x"})),
            call("list_dir", serde_json::json!({"path": "."})),
        ]);
        assert!(!s.any());
    }
}
