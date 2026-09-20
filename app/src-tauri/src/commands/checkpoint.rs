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
//! N2 PR3 (2026-09-20, same task) adds the revert half — the two-step
//! dangerous pair (design §5):
//!
//! - [`revert_to_checkpoint_preview`] — restore set
//!   (`compute_restore_set_from`) + per-path attribution badges
//!   (mined from the session's write audit) + the foreign-delta gate
//!   (recomputed gate tree vs chain-head tree) + a `preview_token`
//!   binding `(target_tree, gate_tree)`.
//! - [`revert_to_checkpoint_execute`] — own-session busy rejection
//!   (OQ-C: MVP scope), token re-validation (`StalePreview` on any
//!   write between preview and confirm), `restore_paths`, audit row
//!   (`AuditKind::CheckpointReverted`), chain untouched (no rollback,
//!   no truncation).
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
// revert_to_checkpoint_preview / revert_to_checkpoint_execute (N2 PR3)
// ---------------------------------------------------------------------------

/// Per-path attribution badge for the revert confirm dialog (design §5
/// "归属标记,增强非过滤"). Three values, from the session's write
/// audit (`session_audit_events` `tool_executed` rows):
///
/// - `tool_written` — a write-family tool (`write_file` / `edit_file`)
///   recorded this exact input path during the session;
/// - `shell_write` — a shell command the A2+ trust layer classified as
///   writing ran in a turn AFTER the target seq (shell targets are
///   unknowable — `echo x > f` has no path attribute — so the marker
///   is turn-level: unattributed paths get the "probably the agent's
///   shell" hint when such a turn exists between target and now);
/// - `unknown` — no audit evidence; under a shared cwd this is the
///   normal state for user hand-edits (the badge is deliberately
///   neutral-colored in the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathAttribution {
    ToolWritten,
    ShellWrite,
    Unknown,
}

/// One file of [`RevertPreview`].
#[derive(Debug, Clone, Serialize)]
pub struct RevertPreviewFile {
    pub path: String,
    /// What the revert will do to this path (checkout = restore target
    /// content, delete = remove — the file postdates the target).
    pub action: git_ckpt::RestoreAction,
    pub attribution: PathAttribution,
}

/// `revert_to_checkpoint_preview`'s payload. `preview_token` binds the
/// target tree and the gate tree (the TOCTOU fence — see
/// [`preview_token`]); `foreign_delta` is non-empty ONLY at preview
/// time (the gate's verdict on "is the current state explained by this
/// session's chain"), the confirm dialog renders a dedicated warning
/// area for it.
#[derive(Debug, Clone, Serialize)]
pub struct RevertPreview {
    pub files: Vec<RevertPreviewFile>,
    pub foreign_delta: Option<Vec<crate::git::diff::FileDiff>>,
    pub target_seq: i64,
    pub target_created_at: i64,
    pub preview_token: String,
}

/// `revert_to_checkpoint_execute`'s payload — the toast's counts.
#[derive(Debug, Clone, Serialize)]
pub struct RevertResult {
    pub restored: usize,
    pub deleted: usize,
}

/// Stale preview: the gate tree recomputed at execute time no longer
/// matches the previewed token (a write landed between preview and
/// confirm). The old confirmation must NOT authorize a new restore
/// set — the frontend offers a re-preview button (retryable).
fn stale_preview() -> AppCommandError {
    AppCommandError {
        category: ErrorCategory::InvalidRequest,
        kind: "StalePreview".to_string(),
        message: "预览后文件已再次变化,还原集已过期;请重新预览".to_string(),
        retryable: true,
        request_id: None,
    }
}

/// Own-session busy rejection (OQ-C ruling: MVP rejects ONLY the
/// target session's own in-flight turn — reverting files under a
/// running agent = pulling the floor from under its feet; other
/// sessions sharing the cwd are left to observation).
fn session_busy() -> AppCommandError {
    AppCommandError {
        category: ErrorCategory::InvalidRequest,
        kind: "SessionBusy".to_string(),
        message: "会话正在运行中,请先停止当前轮次再回退".to_string(),
        retryable: false,
        request_id: None,
    }
}

/// The preview token: binds `(target_tree, gate_tree)` as
/// `"<target>:<gate>"` (both lowercase 40-hex). Deliberately NOT a
/// separate hash function — the oid pair already IS content-addressed
/// (git's own hash); the concatenation is stable, self-describing in
/// logs, and the execute side recomputes and string-compares it.
fn preview_token(target_tree: git2::Oid, gate_tree: git2::Oid) -> String {
    format!("{target_tree}:{gate_tree}")
}

/// Session-level write-audit facts for [`PathAttribution`], mined from
/// the session's `tool_executed` audit rows (the internal
/// `list_audit_events` full-pull path per design §5 — not the paged UI
/// version).
struct WriteAuditFacts {
    /// Input paths recorded by write-family tools (`write_file` /
    /// `edit_file` — the same family as the loop's write-signal gate).
    tool_paths: std::collections::HashSet<String>,
    /// `turn_seq`s of rows whose shell command the A2+ trust layer
    /// classified as writing (`shell_trust::classify_prefix` ≠
    /// ReadOnly — same predicate as the loop's write-signal gate).
    /// Foreground `shell` + `run_background_shell` both count.
    shell_write_turns: Vec<i64>,
    /// A judged-write shell row with `turn_seq IS NULL` (outside the
    /// turn loop — defensive; counted conservatively as "may have
    /// written after any target").
    shell_write_unknown_turn: bool,
}

impl WriteAuditFacts {
    fn from_audit_rows(rows: &[db::AuditEventRow]) -> WriteAuditFacts {
        let mut facts = WriteAuditFacts {
            tool_paths: std::collections::HashSet::new(),
            shell_write_turns: Vec::new(),
            shell_write_unknown_turn: false,
        };
        for row in rows {
            if row.kind != crate::agent::permissions::AuditKind::ToolExecuted.as_str() {
                continue;
            }
            let Ok(payload) = serde_json::from_str::<serde_json::Value>(
                row.payload_json.as_deref().unwrap_or_default(),
            ) else {
                continue;
            };
            let tool = payload.get("tool_name").and_then(|v| v.as_str());
            let input = payload.get("tool_input");
            match tool {
                Some("write_file") | Some("edit_file") => {
                    if let Some(path) = input.and_then(|i| i.get("path")).and_then(|v| v.as_str()) {
                        facts.tool_paths.insert(path.to_string());
                    }
                }
                Some("shell") | Some("run_background_shell") => {
                    let cmd = input
                        .and_then(|i| i.get("command"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if crate::agent::permissions::shell_trust::classify_prefix(cmd)
                        != crate::agent::permissions::shell_trust::ShellTrust::ReadOnly
                    {
                        match row.turn_seq {
                            Some(seq) => facts.shell_write_turns.push(seq),
                            None => facts.shell_write_unknown_turn = true,
                        }
                    }
                }
                _ => {}
            }
        }
        facts
    }

    /// Attribute one restore-set path relative to the revert target:
    /// tool evidence wins; otherwise a judged-write shell turn AFTER
    /// the target (or of unknown turn) earns the shell hint; the rest
    /// is Unknown.
    fn attribute(&self, path: &str, target_seq: i64) -> PathAttribution {
        if self.tool_paths.contains(path) {
            return PathAttribution::ToolWritten;
        }
        if self.shell_write_unknown_turn || self.shell_write_turns.iter().any(|t| *t > target_seq) {
            return PathAttribution::ShellWrite;
        }
        PathAttribution::Unknown
    }
}

/// Load the checkpoint rows and locate `target_seq`. Shared by
/// preview + execute; the baseline row IS a valid target here
/// (「回到会话前」, AC10) — unlike the diff command there is no
/// baseline exclusion.
async fn load_target_row(
    state: &Arc<AppState>,
    session_id: &str,
    target_seq: i64,
) -> Result<Vec<db::checkpoint::CheckpointRow>, AppCommandError> {
    let rows = db::checkpoint::list_checkpoints(&state.db, session_id)
        .await
        .map_err(|e| anyhow::anyhow!("revert: db failed: {}", e))?;
    if rows.is_empty() {
        return Err(checkpoints_unavailable(
            "checkpoint: session has no checkpoint rows",
        ));
    }
    if !rows.iter().any(|r| r.seq == target_seq) {
        return Err(checkpoints_unavailable(format!(
            "checkpoint: no snapshot row for seq {target_seq}"
        )));
    }
    Ok(rows)
}

pub async fn revert_to_checkpoint_preview_inner(
    state: &Arc<AppState>,
    session_id: String,
    target_seq: i64,
) -> Result<RevertPreview, AppCommandError> {
    let ctx = resolve_ctx(state, &session_id).await?;
    let rows = load_target_row(state, &session_id, target_seq).await?;
    let target_row = rows
        .iter()
        .find(|r| r.seq == target_seq)
        .expect("checked above");
    // 归属证据(异步 DB 侧)先取,再进同步 git 相(repo 句柄不过 .await)。
    let audit_rows = db::list_audit_events(&state.db, &session_id)
        .await
        .map_err(|e| anyhow::anyhow!("revert preview: audit query failed: {}", e))?;
    let facts = WriteAuditFacts::from_audit_rows(&audit_rows);

    let (set, foreign_delta, token) = {
        let repo = open_repo(&ctx.repo_path)?;
        let target_tree = resolve_row_tree(&repo, target_row)?;
        // 门禁重算:当前工作态树(gate)。失败(含 EUNMERGED 冲突态)
        // 按 Broken 降级 —— 门禁跑不了就不该发起还原。
        let gate_tree = git_ckpt::build_state_tree(&repo)
            .map_err(|e| checkpoint_broken(format!("checkpoint: gate tree failed: {}", e)))?;
        let set = git_ckpt::compute_restore_set_from(&repo, gate_tree, target_tree)
            .map_err(|e| checkpoint_broken(format!("checkpoint: restore set failed: {}", e)))?;
        // 外来写入门:gate 树 ≠ 链头快照树 → 有非本会话快照内的变更
        // (轮间隙用户手改等)。diff 方向 = 链头 → gate(「多出来的部分」)。
        let head_row = rows.last().expect("non-empty checked");
        let head_tree = resolve_row_tree(&repo, head_row)?;
        let foreign = if head_tree != gate_tree {
            let d = git_ckpt::diff_snapshots(&repo, head_tree, gate_tree).map_err(|e| {
                checkpoint_broken(format!("checkpoint: foreign delta failed: {}", e))
            })?;
            Some(d.files)
        } else {
            None
        };
        (set, foreign, preview_token(target_tree, gate_tree))
    };

    let files: Vec<RevertPreviewFile> = set
        .into_iter()
        .map(|p| RevertPreviewFile {
            attribution: facts.attribute(&p.path, target_seq),
            path: p.path,
            action: p.action,
        })
        .collect();
    Ok(RevertPreview {
        files,
        foreign_delta,
        target_seq,
        target_created_at: target_row.created_at,
        preview_token: token,
    })
}

#[tauri::command]
pub async fn revert_to_checkpoint_preview(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    target_seq: i64,
) -> Result<RevertPreview, AppCommandError> {
    revert_to_checkpoint_preview_inner(&state, session_id, target_seq).await
}

pub async fn revert_to_checkpoint_execute_inner(
    state: &Arc<AppState>,
    session_id: String,
    target_seq: i64,
    preview_token_param: String,
) -> Result<RevertResult, AppCommandError> {
    let ctx = resolve_ctx(state, &session_id).await?;
    // busy 拒绝(OQ-C:MVP 只拒本 session)。真源 = loop 的
    // `session_active_request` 注册面(与 chat 路由的 busy 判定同源)。
    if state
        .session_active_request
        .lock()
        .await
        .contains_key(&session_id)
    {
        return Err(session_busy());
    }
    let rows = load_target_row(state, &session_id, target_seq).await?;
    let target_row = rows
        .iter()
        .find(|r| r.seq == target_seq)
        .expect("checked above");

    let (outcome, gate_tree_str, foreign_paths, executed_paths) = {
        let repo = open_repo(&ctx.repo_path)?;
        let target_tree = resolve_row_tree(&repo, target_row)?;
        let gate_tree = git_ckpt::build_state_tree(&repo)
            .map_err(|e| checkpoint_broken(format!("checkpoint: gate tree failed: {}", e)))?;
        // TOCTOU 重验:preview→confirm 之间任何落盘都会换 gate 树,
        // 旧 token 不得授权新还原集(评审修正)。
        if preview_token(target_tree, gate_tree) != preview_token_param {
            return Err(stale_preview());
        }
        let set = git_ckpt::compute_restore_set_from(&repo, gate_tree, target_tree)
            .map_err(|e| checkpoint_broken(format!("checkpoint: restore set failed: {}", e)))?;
        // audit 摘要输入在 restore 前收集(restore 消费 &set,不 move)。
        let executed_paths: Vec<String> = set.iter().map(|p| p.path.clone()).collect();
        let head_row = rows.last().expect("non-empty checked");
        let head_tree = resolve_row_tree(&repo, head_row)?;
        let foreign_paths: Vec<String> = if head_tree != gate_tree {
            let d = git_ckpt::diff_snapshots(&repo, head_tree, gate_tree).map_err(|e| {
                checkpoint_broken(format!("checkpoint: foreign delta failed: {}", e))
            })?;
            d.files.into_iter().map(|f| f.path).collect()
        } else {
            Vec::new()
        };
        let outcome = git_ckpt::restore_paths(&repo, target_tree, &set).map_err(|e| {
            AppCommandError::new(
                ErrorCategory::Server,
                format!("checkpoint: restore failed: {e}"),
            )
        })?;
        (
            outcome,
            gate_tree.to_string(),
            foreign_paths,
            executed_paths,
        )
    };

    // audit 落账(best-effort,UiDiffApplied 惯例:失败 warn 不回滚
    // 已执行的还原 —— 还原成功是事实,audit 缺行可由 turn diff 对账)。
    if let Err(e) = crate::agent::permissions::audit::record_checkpoint_reverted_audit(
        &state.db,
        &session_id,
        crate::agent::permissions::audit::CheckpointRevertAudit {
            target_seq,
            restored: outcome.restored,
            deleted: outcome.deleted,
            paths: &executed_paths,
            foreign_paths: &foreign_paths,
            gate_tree_oid: &gate_tree_str,
        },
    )
    .await
    {
        tracing::warn!(
            session_id = %session_id,
            error = %e,
            "revert_to_checkpoint: audit write failed (non-fatal)"
        );
    }

    Ok(RevertResult {
        restored: outcome.restored,
        deleted: outcome.deleted,
    })
}

#[tauri::command]
pub async fn revert_to_checkpoint_execute(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    target_seq: i64,
    preview_token: String,
) -> Result<RevertResult, AppCommandError> {
    revert_to_checkpoint_execute_inner(&state, session_id, target_seq, preview_token).await
}

// ---------------------------------------------------------------------------
// 命令层测试(design §5 降级矩阵 + prev_seq 稀疏链)
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests_checkpoint.rs"]
mod tests_checkpoint;
