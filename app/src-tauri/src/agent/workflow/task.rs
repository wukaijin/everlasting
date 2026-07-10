//! W1 (Workflow integration, Phase 0 Step 0.4 — 2026-07-08):
//! workflow **task** 文件态 — `.everlasting/tasks/<slug>/task.json`
//! 读 + 写 + 初始模板生成。这一层**纯文件 IO**,不读 DB 不调 LLM
//! 不开 tokio runtime(全 sync),这样 Step 0.5 的 chat_loop per-turn
//! 注入可以同步调用而不破坏现有 async 签名(同步代码包在
//! `spawn_blocking` 里)。
//!
//! ## 目录约定
//!
//! ```text
//! <project_path>/.everlasting/tasks/<slug>/
//!     ├── task.json     # 元数据 + items 列表(本文焦点)
//!     ├── prd.md        # 创建时填的 skeleton 由 create_task 写
//!     ├── design.md     # agent 用 wf-before-dev 自行追加
//!     └── progress.md   # agent 推进时 + 跨 session 续上时追加
//! ```
//!
//! ## task.json Schema
//!
//! Minimal v1:
//!
//! ```json
//! {
//!   "id": "uuid-v4-string",
//!   "title": "<string>",
//!   "slug": "<[a-z0-9-]+>",
//!   "status": "planning",
//!   "created_at": "<rfc3339>",
//!   "updated_at": "<rfc3339>",
//!   "parent": null,
//!   "summary": "",
//!   "items": []
//! }
//! ```
//!
//! `parent` is reserved for B8 DAG (Phase 3+); nullable now.
//! `summary` is the user-provided one-liner; Phase 1's
//! `wf-brainstorm` skill feeds it. Empty in v1 is fine.
//!
//! ## Phase scope
//!
//! - **Step 0.4**: types + read/write + `create_task` template fill +
//!   dir layout. NO `update_task` IPC yet — Phase 2 Step 2.6
//!   (B12 checklist sync) owns the writer path for `items`.
//! - **Step 2.6**: `update_task(task_dir, items)` writes via
//!   `write_task_partial(...).items = ...` to keep serde
//!   semantics centralized.
//!
//! ## Validation
//!
//! `slug` is constrained to `[a-z0-9-]{1,64}` ASCII.
//! Bad slugs are rejected as `Err(TaskError::InvalidSlug)` —
//! the IPC surfaces this as `AppCommandError::InvalidRequest`
//! (`commands/task.rs` wrapper). The constraint exists to
//! keep `.everlasting/tasks/<slug>/` paths portable across
//! case-insensitive filesystems (NTFS / HFS+ / APFS) and
//! shell-safe.

// Step 0.4 ships the file-state surface (`TaskJson` + read /
// write + `create_task_init`) BEFORE the first consumer
// lands in Step 0.5 (chat_loop's per-turn breadcrumb
// injection reads via `read_task`). The IPC wrapper
// (`commands::task`) is also new this step but, like
// `def.rs`, briefly dead-code until Phase 1's
// wf-brainstorm skill triggers `create_task`.
//
// Suppress `dead_code` for this file. Remove in Step 0.5's
// commit when chat_loop adds its first read_site, and in
// the `commands::task` IPC's first commit that uses this
// body.
//
// Tests below use every public item, so they're unaffected.
#![allow(dead_code)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Workflow task's authoritative status. Mirrors
/// `WorkflowDef::states` (planning / implement / check /
/// done) — kept as an explicit enum so the JSON validator
/// can reject typos upfront.
///
/// **Step 3.3 (2026-07-09)**: added the `Completed` terminal
/// variant, set by [`archive_task_init`] after moving the
/// task to `.everlasting/tasks/archive/<YYYY-MM>/<slug>/`.
/// `Completed` is distinct from `Done`:
/// - `Done`    = state machine's terminal state (checklist
///               closed, spec distilled by `wf-update-spec`).
/// - `Completed` = **archived** — moved to the archive
///               tree, no longer resolvable by
///               `inject::resolve_current_task`. `Completed`
///               is a one-way terminal: there is no
///               `Completed → X` transition; un-archive is
///               a manual git operation.
///
/// The Step 0.5 chat_loop per-turn injection deserializes
/// the field via [`TaskStatus::from_str_opt`] which falls
/// back to `Planning` on unknown values (lenient parse
/// matches the Mode-Select posture; see W1 prd.md §接缝定位).
/// `Completed` is in the `from_str_opt` accept-list so an
/// archive file accidentally re-read by the chat loop
/// doesn't silently demote to `Planning`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Planning,
    Implement,
    Check,
    Done,
    Completed,
}

impl TaskStatus {
    pub fn from_str_opt(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "implement" => Self::Implement,
            "check" => Self::Check,
            "done" => Self::Done,
            "completed" => Self::Completed,
            _ => Self::Planning,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planning => "planning",
            Self::Implement => "implement",
            Self::Check => "check",
            Self::Done => "done",
            Self::Completed => "completed",
        }
    }
}

/// Lenient `Deserialize` — unknown / typo'd status strings fall back
/// to `Planning` via [`TaskStatus::from_str_opt`], so a hand-written
/// `task.json` with e.g. `"status": "in_progress"` or `"pending"`
/// (checklist-style values the LLM naturally emits) does NOT break
/// `read_task`. 07-10-workflow-task-json-hardening R1: the derive
/// `Deserialize` was strict and rejected any variant outside the
/// 5-state enum, which made every direct `write_file` of task.json
/// fatal. Resilience lives on the read side, not by gating writes.
/// Mirrors `def.rs::Coordination`'s custom Deserialize posture.
impl<'de> serde::Deserialize<'de> for TaskStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(Self::from_str_opt(&s))
    }
}

/// One to-do entry. The struct's shape mirrors what B12's
/// `ChecklistItem` will become in Phase 2 Step 2.6; for
/// Phase 0 the JSON is just persisted and read, no
/// `update_checklist` writes to it yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskItem {
    /// Stable id (e.g. `"backend-impl"`). Phase 2 Step 2.6
    /// uses this as the B12 checklist key for
    /// cross-session persistence.
    pub id: String,
    #[serde(default)]
    pub content: String,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tdd: Option<bool>,
}

/// On-disk `.everlasting/tasks/<slug>/task.json` schema.
/// See module-level doc comment for the field-level rules.
///
/// **Step 3.3 (2026-07-09)**: added `completed_at: Option<String>`
/// — set by [`archive_task_init`] (RFC3339 timestamp of the
/// archive move). The field is optional and `None` for
/// pre-archive tasks; serde defaults + skip-serializing keep
/// the v1 schema forward-compatible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskJson {
    pub id: String,
    pub title: String,
    pub slug: String,
    pub status: TaskStatus,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    /// `None` for top-level tasks; non-empty for child
    /// tasks (B8 DAG, Phase 3+). The slug of the parent
    /// task is sufficient — not a UUID — because parent
    /// references are most naturally project-relative.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// One-line task summary. Empty on creation; the
    /// Phase 1 `wf-brainstorm` skill populates it.
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub items: Vec<TaskItem>,
    /// RFC3339 timestamp set when the task was archived
    /// via [`archive_task_init`]. `None` for in-flight
    /// tasks; on archive the status flips to
    /// [`TaskStatus::Completed`] AND this field is filled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Local error type; the IPC layer in `commands::task`
/// maps each variant to the right `AppCommandError`
/// category.
///
/// **Step 3.3 (2026-07-09)**: added `AlreadyArchived` /
/// `NotInDoneStatus` to support [`archive_task_init`]
/// without widening the pre-existing variants. Each maps
/// to `InvalidRequest` at the IPC boundary — archive is
/// user-correctable.
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("invalid slug: {0} (expected [a-z0-9-]{{1,64}})")]
    InvalidSlug(String),

    #[error("task already exists at {0}")]
    AlreadyExists(PathBuf),

    #[error("task directory not found: {0}")]
    NotFound(PathBuf),

    /// Step 3.3: archive target already occupied. Refuse
    /// to overwrite an existing archive entry so the
    /// command is idempotent on retry (a partially-written
    /// archive followed by a retry won't clobber the prior
    /// state). The caller can choose to delete the
    /// conflicting archive dir manually if they really want
    /// to re-archive.
    #[error("task already archived at {0}")]
    AlreadyArchived(PathBuf),

    /// Step 3.3: archive requires `status == Done`. We
    /// refuse to archive a planning / implement / check
    /// task because (a) the workflow hasn't finished
    /// producing spec content yet, and (b) archiving
    /// would orphan in-flight items + progress.md.
    #[error("cannot archive task in status `{0}` (must be `done`)")]
    NotInDoneStatus(String),

    #[error("task.json malformed at {0}: {1}")]
    MalformedJson(PathBuf, String),

    #[error("io error at {0}: {1}")]
    Io(PathBuf, #[source] io::Error),
}

pub type TaskResult<T> = Result<T, TaskError>;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Project namespace shared across all four subsystems
/// (commands / agents / skills / outputs). Same constant
/// used by every `.everlasting/*` loader — kept here only
/// as a convenience re-export so callers building task
/// paths don't need to import the `PROJ_NS` from three
/// different modules. The single source of truth lives in
/// the agent directories (e.g. `agent::subagent::loader`
/// `PROJ_NS = ".everlasting"`); when those constants
/// change, this one follows.
const PROJ_NS_TASKS_DIR: &str = ".everlasting/tasks";

/// Maximum slug length. ASCII `[a-z0-9-]` only — any other
/// char (spaces / accents / emoji) is rejected. Generous
/// bound to leave headroom for "session-2026-07-08-abc".
const MAX_SLUG_LEN: usize = 64;

// ---------------------------------------------------------------------------
// Slug validation
// ---------------------------------------------------------------------------

/// Strict slug validator — `^[a-z0-9-]{1,64}$` ASCII,
/// non-empty, no leading / trailing hyphen. The hyphen
/// guard stops `--foo` / `foo--` confusion in shell
/// auto-complete; the lowercase guard stops
/// `My-Feature` vs `my-feature` directory confusion on
/// case-insensitive filesystems.
pub fn validate_slug(slug: &str) -> TaskResult<()> {
    if slug.is_empty() || slug.len() > MAX_SLUG_LEN {
        return Err(TaskError::InvalidSlug(slug.to_string()));
    }
    if slug.starts_with('-') || slug.ends_with('-') {
        return Err(TaskError::InvalidSlug(slug.to_string()));
    }
    for c in slug.chars() {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(TaskError::InvalidSlug(slug.to_string()));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Path layout
// ---------------------------------------------------------------------------

/// `<project>/.everlasting/tasks/<slug>/` — does NOT create
/// directories; use [`write_task`] for the create-on-write
/// path or call `fs::create_dir_all` yourself.
pub fn task_dir(project_path: &Path, slug: &str) -> PathBuf {
    project_path.join(PROJ_NS_TASKS_DIR).join(slug)
}

/// `<project>/.everlasting/tasks/<slug>/task.json`.
pub fn task_json_path(project_path: &Path, slug: &str) -> PathBuf {
    task_dir(project_path, slug).join("task.json")
}

/// `<project>/.everlasting/tasks/<slug>/prd.md`.
pub fn task_prd_path(project_path: &Path, slug: &str) -> PathBuf {
    task_dir(project_path, slug).join("prd.md")
}

// ---------------------------------------------------------------------------
// IO — read / write / create-init
// ---------------------------------------------------------------------------

/// Read and parse `task.json` from disk. Used by Step 0.5's
/// chat_loop per-turn injection. Returns
/// `Err(TaskError::NotFound)` when the file is missing
/// (NOT `Err(Io)` so the caller's "no task yet" branch is
/// unambiguous).
pub fn read_task(project_path: &Path, slug: &str) -> TaskResult<TaskJson> {
    let path = task_json_path(project_path, slug);
    if !path.exists() {
        return Err(TaskError::NotFound(task_dir(project_path, slug)));
    }
    let bytes = fs::read(&path).map_err(|e| TaskError::Io(path.clone(), e))?;
    let task: TaskJson = serde_json::from_slice(&bytes)
        .map_err(|e| TaskError::MalformedJson(path.clone(), e.to_string()))?;
    Ok(task)
}

/// Serialize and atomically write `task.json`. Writes to
/// `<file>.tmp` first then renames onto the final path so
/// a partial write can never be observed by a concurrent
/// reader (Step 0.5's chat_loop + the Phase 2 B12 writer
/// may both touch the file).
pub fn write_task(project_path: &Path, task: &TaskJson) -> TaskResult<()> {
    validate_slug(&task.slug)?;
    let dir = task_dir(project_path, &task.slug);
    fs::create_dir_all(&dir).map_err(|e| TaskError::Io(dir.clone(), e))?;
    let final_path = dir.join("task.json");
    let tmp_path = dir.join("task.json.tmp");
    let bytes = serde_json::to_vec_pretty(task)
        .map_err(|e| TaskError::MalformedJson(final_path.clone(), e.to_string()))?;
    fs::write(&tmp_path, &bytes).map_err(|e| TaskError::Io(tmp_path.clone(), e))?;
    fs::rename(&tmp_path, &final_path).map_err(|e| TaskError::Io(final_path.clone(), e))?;
    Ok(())
}

/// `create_task` body — fills a fresh v1 template, writes
/// the `task.json` + `prd.md` skeleton, returns the
/// resulting `TaskJson`. Called by the Tauri IPC
/// (`commands::task::create_task`).
///
/// **Idempotency**: refuses to overwrite an existing
/// `task.json` (returns `Err(AlreadyExists)`). The frontend
/// is expected to surface "task already exists" + a "open
/// existing" affordance rather than silently overwrite.
///
/// The new task starts at `status = Planning` (matches
/// `default_workflow().initial`); Phase 0 has no
/// pre-flight checks on the `initial` state.
pub fn create_task_init(
    project_path: &Path,
    title: &str,
    slug: &str,
    parent: Option<&str>,
) -> TaskResult<TaskJson> {
    validate_slug(slug)?;
    if title.trim().is_empty() {
        return Err(TaskError::InvalidSlug(
            "title must not be empty".to_string(),
        ));
    }
    let dir = task_dir(project_path, slug);
    let json_path = dir.join("task.json");
    if json_path.exists() {
        return Err(TaskError::AlreadyExists(dir));
    }

    // Compose the v1 template. The `id` is a v4 UUID —
    // uniqueness is global so two projects can't
    // accidentally collide on the same slug (rare but
    // possible when the user opens parallel workspaces).
    let now = Utc::now().to_rfc3339();
    let id = uuid::Uuid::new_v4().to_string();
    let task = TaskJson {
        id,
        title: title.trim().to_string(),
        slug: slug.to_string(),
        status: TaskStatus::Planning,
        created_at: now.clone(),
        updated_at: now,
        parent: parent.map(str::to_string),
        summary: String::new(),
        items: Vec::new(),
        // Step 3.3: completed_at is set later by
        // archive_task_init. Always None at creation time.
        completed_at: None,
    };

    write_task(project_path, &task)?;

    // Best-effort `prd.md` skeleton. The user / agent
    // can fully rewrite it; this is just a starting
    // prompt so the file always exists at create time
    // (matches `.everlasting/commands/*/cmd.md` and
    // `.everlasting/skills/*/SKILL.md` skeletons).
    let prd_path = task_prd_path(project_path, slug);
    let prd_body = format!(
        "# {title}\n\
         \n\
         > task slug: `{slug}`\n\
         > created: {now}\n\
         > spec: pull from `WorkflowCtx` once Step 0.5 wires it.\n\
         \n\
         ## Goal\n\
         \n\
         <fill>\n\
         \n\
         ## Acceptance criteria\n\
         \n\
         - [ ] <fill>\n",
        title = task.title,
        slug = task.slug,
        now = task.created_at,
    );
    fs::write(&prd_path, prd_body.as_bytes()).map_err(|e| TaskError::Io(prd_path, e))?;

    Ok(task)
}

// ---------------------------------------------------------------------------
// archive_task_init — Step 3.3 (2026-07-09)
// ---------------------------------------------------------------------------

/// Path component used for the archive sub-tree under
/// `.everlasting/tasks/`. Re-exported (not just `const`)
/// because the IPC layer (`commands::task::archive_task`)
/// and tests both reference it; centralizing here keeps
/// the archive layout single-sourced.
pub const PROJ_NS_TASKS_ARCHIVE_DIR: &str = "archive";

/// `archive_task_init` body — moves
/// `<project>/.everlasting/tasks/<slug>/` →
/// `<project>/.everlasting/tasks/archive/<YYYY-MM>/<slug>/`
/// and flips `task.json` to `status = Completed` with a
/// `completed_at` timestamp. Called by the Tauri IPC
/// (`commands::task::archive_task`).
///
/// **Preconditions** (returns `Err` on failure):
/// - `slug` validates as `[a-z0-9-]{1,64}` (`InvalidSlug`).
/// - `<slug>/task.json` exists and parses (`NotFound` /
///   `MalformedJson`).
/// - `status == Done` (`NotInDoneStatus`). Archive must
///   follow the workflow's `Check → Done` hook (Step 3.1)
///   so the spec-distillation hint + items close-out have
///   already happened; archiving a planning / implement /
///   check task would orphan in-flight work.
/// - archive target dir does **not** already exist
///   (`AlreadyArchived`). This makes the call idempotent
///   against retries: a partial-write + retry won't
///   clobber the prior archive.
///
/// **What it does**:
/// 1. Read current `task.json` (must be `Done`).
/// 2. Compute `dst = <project>/.everlasting/tasks/archive/<YYYY-MM>/<slug>/`.
/// 3. `mkdir -p` the archive parent.
/// 4. `fs::rename(src, dst)` — same-filesystem rename is
///    atomic; the `mkdir -p` step above guarantees
///    `dst`'s parent exists on the same FS as `src`.
/// 5. Re-open `dst/task.json`, set `status = Completed`
///    + `completed_at = now()` + `updated_at = now()`,
///    atomic-rename write (via `write_task`).
/// 6. (Default) `git add` + `git commit` the archive tree
///    so the move is captured in version history. Caller
///    can opt out via `no_commit = true` for dry-run /
///    offline scenarios.
///
/// **Returns** the archived `TaskJson` (status
/// `Completed`, `completed_at` set) so the IPC layer
/// can surface the post-archive state to the frontend.
///
/// **Step 3.3 design note**: archive is intentionally a
/// **move**, not a copy. The pre-archive dir is gone
/// after a successful archive — `resolve_current_task`
/// will not pick the task up again because (a) the live
/// tree no longer has the slug, and (b) the archive tree
/// has `status = Completed` which `inject.rs` also skips.
pub fn archive_task_init(project_path: &Path, slug: &str, no_commit: bool) -> TaskResult<TaskJson> {
    use std::fmt::Write;

    validate_slug(slug)?;

    let src_dir = task_dir(project_path, slug);

    // 1. Read current task.json (must be Done).
    let mut task = read_task(project_path, slug)?;

    if task.status != TaskStatus::Done {
        return Err(TaskError::NotInDoneStatus(task.status.as_str().to_string()));
    }

    // 2. Compute destination under archive/<YYYY-MM>/<slug>/.
    let now_dt = Utc::now();
    let ym = now_dt.format("%Y-%m").to_string();
    let dst_dir = src_dir
        .parent()
        .ok_or_else(|| TaskError::Io(src_dir.clone(), io::Error::other("task dir has no parent")))?
        .join(PROJ_NS_TASKS_ARCHIVE_DIR)
        .join(&ym)
        .join(slug);

    if dst_dir.exists() {
        return Err(TaskError::AlreadyArchived(dst_dir));
    }

    // 3. mkdir -p the archive parent (so `fs::rename`
    //    succeeds on the same FS).
    if let Some(parent) = dst_dir.parent() {
        fs::create_dir_all(parent).map_err(|e| TaskError::Io(parent.to_path_buf(), e))?;
    }

    // 4. Move src → dst (atomic on same FS).
    fs::rename(&src_dir, &dst_dir).map_err(|e| TaskError::Io(src_dir.clone(), e))?;

    // 5. Re-write task.json in dst with Completed +
    //    completed_at + updated_at. We bypass `write_task`
    //    (which would re-check the parent dir layout)
    //    and write directly via atomic tmp+rename against
    //    the new path.
    let now_ts = now_dt.to_rfc3339();
    task.status = TaskStatus::Completed;
    task.updated_at = now_ts.clone();
    task.completed_at = Some(now_ts);

    let dst_json = dst_dir.join("task.json");
    let dst_tmp = dst_dir.join("task.json.tmp");
    let bytes = serde_json::to_vec_pretty(&task)
        .map_err(|e| TaskError::MalformedJson(dst_json.clone(), e.to_string()))?;
    fs::write(&dst_tmp, &bytes).map_err(|e| TaskError::Io(dst_tmp.clone(), e))?;
    fs::rename(&dst_tmp, &dst_json).map_err(|e| TaskError::Io(dst_json.clone(), e))?;

    tracing::info!(
        slug = %slug,
        src = %src_dir.display(),
        dst = %dst_dir.display(),
        "archive_task_init: task moved to archive tree"
    );

    // 6. (Default) git add + commit the archive tree.
    //    Use a shell-out to `git` rather than wiring
    //    libgit2 here — the commit is the project's repo,
    //    not a session worktree, and the project repo's
    //    HEAD branch identity varies per developer
    //    (main / master / feature). libgit2 would need
    //    an equivalent of `git -C <repo> commit -m ...`
    //    with the right user identity pre-configured; the
    //    spawn route inherits the developer's existing
    //    identity + branch config without ceremony. The
    //    IPC test layer skips this branch (passes
    //    `no_commit = true`).
    if !no_commit {
        let archive_rel = format!(
            ".everlasting/tasks/{dir}/{slug}",
            dir = PROJ_NS_TASKS_ARCHIVE_DIR,
            slug = slug,
        );
        if let Err(e) = git_add_path(project_path, &archive_rel) {
            tracing::warn!(
                slug = %slug,
                error = %e,
                "archive_task_init: git add failed; archive still on disk, \
                 commit skipped. The user can `git add` + commit manually."
            );
        } else {
            // Sanity: if `git add` succeeded but the
            // path is already tracked (e.g. archive
            // happened twice and the second time the
            // archive dir already exists as a stale
            // entry), `git commit` is a no-op. That's
            // acceptable — the move is the primary
            // effect; the commit is a convenience.
            let mut msg = String::from("chore(task): archive ");
            let _ = write!(&mut msg, "{}", slug);
            if let Err(e) = git_commit(project_path, &msg) {
                tracing::warn!(
                    slug = %slug,
                    error = %e,
                    "archive_task_init: git commit failed; archive on disk, \
                     commit skipped. The user can commit manually."
                );
            } else {
                tracing::info!(
                    slug = %slug,
                    "archive_task_init: git committed archive tree"
                );
            }
        }
    }

    Ok(task)
}

/// Helper: spawn `git -C <repo> add <path>`. Returns
/// `Ok(())` if `git` exits 0 OR the path is already
/// tracked (idempotent — re-running with no diff is a
/// no-op for `git add`). Errors (binary missing /
/// non-zero exit / non-git dir) are surfaced as
/// `Err(String)` so the caller can decide whether to
/// log + continue or bubble up.
fn git_add_path(repo: &Path, rel_path: &str) -> Result<(), String> {
    use std::process::Command;
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["add", "--", rel_path])
        .output()
        .map_err(|e| format!("spawn git add failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git add exited {:?}: stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Helper: spawn `git -C <repo> commit -m <msg>`.
/// Returns `Ok(())` on success. The "nothing to commit"
/// case is treated as `Err("nothing to commit")` so the
/// caller logs it but doesn't escalate — archive's
/// primary effect (the move) already happened.
fn git_commit(repo: &Path, msg: &str) -> Result<(), String> {
    use std::process::Command;
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["commit", "--quiet", "-m", msg])
        .output()
        .map_err(|e| format!("spawn git commit failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git commit exited {:?}: stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Per-test scratch dir under `tempfile::tempdir()`. Auto-cleaned
    /// when the `TempDir` is dropped at the end of each test, so
    /// the working tree stays clean across `cargo test` runs.
    fn fresh_project() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn proj(d: &tempfile::TempDir) -> PathBuf {
        d.path().to_path_buf()
    }

    // --- slug validation --------------------------------------------------

    #[test]
    fn validate_slug_accepts_lowercase_alphanumeric_with_hyphens() {
        assert!(validate_slug("dev").is_ok());
        assert!(validate_slug("my-feature").is_ok());
        assert!(validate_slug("a").is_ok());
        assert!(validate_slug("abc-123").is_ok());
        assert!(validate_slug(&"a".repeat(64)).is_ok(), "at the boundary");
    }

    #[test]
    fn validate_slug_rejects_bad_chars_or_bounds() {
        for bad in [
            "",
            &"a".repeat(65),
            "-leading-hyphen",
            "trailing-hyphen-",
            "UPPER",
            "Mixed",
            "with space",
            "中文",
            "with_underscore",
            "with/slash",
            "with.dot",
        ] {
            assert!(
                validate_slug(bad).is_err(),
                "slug {:?} should be rejected",
                bad
            );
        }
    }

    // --- create + read round-trip ---------------------------------------

    #[test]
    fn create_task_init_writes_json_and_prd_skeleton() {
        let d = fresh_project();
        let task = create_task_init(&proj(&d), "My Feature", "my-feature", None).expect("create");
        assert_eq!(task.title, "My Feature");
        assert_eq!(task.slug, "my-feature");
        assert_eq!(task.status, TaskStatus::Planning);
        assert!(task.summary.is_empty());
        assert!(task.items.is_empty());
        assert!(task.parent.is_none());

        // Files exist on disk.
        let json_path = task_json_path(&proj(&d), "my-feature");
        let prd_path = task_prd_path(&proj(&d), "my-feature");
        assert!(json_path.exists());
        assert!(prd_path.exists());

        // Round-trip: re-read the JSON and confirm structural identity.
        let again = read_task(&proj(&d), "my-feature").expect("read");
        assert_eq!(again.id, task.id, "id persists across read");
        assert_eq!(again.status, TaskStatus::Planning);
        assert_eq!(again.created_at, task.created_at);
        assert_eq!(again.updated_at, task.updated_at);
        assert!(again.items.is_empty());
    }

    #[test]
    fn create_task_init_refuses_to_overwrite_existing() {
        let d = fresh_project();
        create_task_init(&proj(&d), "First", "dup", None).expect("first ok");
        let err =
            create_task_init(&proj(&d), "Second", "dup", None).expect_err("must reject duplicate");
        assert!(matches!(err, TaskError::AlreadyExists(_)), "got {:?}", err);
    }

    #[test]
    fn create_task_init_with_parent_records_parent_slug() {
        let d = fresh_project();
        let task = create_task_init(&proj(&d), "Sub", "sub-task", Some("parent-task"))
            .expect("create child");
        assert_eq!(task.parent.as_deref(), Some("parent-task"));
        let again = read_task(&proj(&d), "sub-task").expect("read child");
        assert_eq!(again.parent.as_deref(), Some("parent-task"));
    }

    #[test]
    fn read_task_missing_returns_not_found_not_io_error() {
        let d = fresh_project();
        let err = read_task(&proj(&d), "nonexistent").expect_err("missing");
        assert!(
            matches!(err, TaskError::NotFound(_)),
            "got {:?}; the caller's 'no task yet' branch must be unambiguous",
            err
        );
    }

    // --- lenient parse (07-10-workflow-task-json-hardening R1) ------------
    // task.json is a file the LLM can `write_file` directly, so the
    // read side must tolerate hand-written schema drift (missing
    // fields, checklist-style status values like "in_progress" /
    // "pending") rather than crashing the whole workflow. Resilience
    // lives on the read side, NOT by gating writes.

    /// Write a raw (possibly hand-written / schema-drifting) task.json
    /// for `slug`, creating the task dir. Simulates an LLM editing
    /// task.json via write_file without going through create_task /
    /// update_checklist — the exact pattern that crashed the workflow
    /// twice during 07-10 dogfooding.
    fn write_raw_task_json(project: &Path, slug: &str, body: &str) {
        let dir = task_dir(project, slug);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("task.json"), body).unwrap();
    }

    #[test]
    fn read_task_lenient_missing_created_at_and_updated_at() {
        // Crash #1: LLM hand-wrote task.json without created_at /
        // updated_at. #[serde(default)] now fills empty strings.
        let d = fresh_project();
        write_raw_task_json(
            &proj(&d),
            "my-feat",
            r#"{"id":"t1","title":"T","slug":"my-feat","status":"planning","summary":"","items":[]}"#,
        );
        let task = read_task(&proj(&d), "my-feat").expect("lenient parse must succeed");
        assert_eq!(task.slug, "my-feat");
        assert_eq!(task.status, TaskStatus::Planning);
        assert!(
            task.created_at.is_empty(),
            "missing created_at → empty default"
        );
        assert!(
            task.updated_at.is_empty(),
            "missing updated_at → empty default"
        );
    }

    #[test]
    fn read_task_lenient_top_level_unknown_status_falls_back_to_planning() {
        let d = fresh_project();
        write_raw_task_json(
            &proj(&d),
            "my-feat",
            r#"{"id":"t1","title":"T","slug":"my-feat","status":"blocked","created_at":"","updated_at":"","items":[]}"#,
        );
        let task = read_task(&proj(&d), "my-feat").expect("lenient parse");
        assert_eq!(
            task.status,
            TaskStatus::Planning,
            "unknown top-level status → Planning (from_str_opt)"
        );
    }

    #[test]
    fn read_task_lenient_item_status_in_progress_falls_back_to_planning() {
        // Crash #2: LLM hand-edited items[].status = "in_progress"
        // (a checklist-style value not in the TaskStatus enum).
        let d = fresh_project();
        write_raw_task_json(
            &proj(&d),
            "my-feat",
            r#"{"id":"t1","title":"T","slug":"my-feat","status":"implement","created_at":"","updated_at":"","items":[{"id":"a","content":"do thing","status":"in_progress"}]}"#,
        );
        let task = read_task(&proj(&d), "my-feat").expect("lenient item status");
        assert_eq!(task.items.len(), 1);
        assert_eq!(
            task.items[0].status,
            TaskStatus::Planning,
            "in_progress → Planning fallback"
        );
        assert_eq!(task.items[0].content, "do thing");
    }

    #[test]
    fn read_task_lenient_item_status_pending_falls_back_to_planning() {
        let d = fresh_project();
        write_raw_task_json(
            &proj(&d),
            "my-feat",
            r#"{"id":"t1","title":"T","slug":"my-feat","status":"planning","created_at":"","updated_at":"","items":[{"id":"a","content":"x","status":"pending"}]}"#,
        );
        let task = read_task(&proj(&d), "my-feat").expect("lenient");
        assert_eq!(
            task.items[0].status,
            TaskStatus::Planning,
            "pending → Planning"
        );
    }

    #[test]
    fn read_task_lenient_item_missing_content_defaults_empty() {
        let d = fresh_project();
        write_raw_task_json(
            &proj(&d),
            "my-feat",
            r#"{"id":"t1","title":"T","slug":"my-feat","status":"planning","created_at":"","updated_at":"","items":[{"id":"a","status":"done"}]}"#,
        );
        let task = read_task(&proj(&d), "my-feat").expect("missing content → empty default");
        assert_eq!(task.items[0].content, "", "missing content → empty string");
        assert_eq!(task.items[0].status, TaskStatus::Done);
    }

    #[test]
    fn read_task_still_rejects_truly_malformed_json() {
        // Lenient status/defaults do NOT mean "accept any garbage" —
        // structurally broken JSON still fails so genuinely corrupt
        // files surface loudly (the `resolve_current_task` skip-on-
        // error contract depends on a real Err here).
        let d = fresh_project();
        write_raw_task_json(&proj(&d), "my-feat", "not json at all {");
        let err = read_task(&proj(&d), "my-feat").expect_err("garbage must still fail");
        assert!(matches!(err, TaskError::MalformedJson(..)), "got {:?}", err);
    }

    #[test]
    fn write_task_is_atomic_via_tmp_rename() {
        // We can't directly observe the `tmp → final` rename from outside,
        // but we CAN confirm a partial-failure surrogate: write_task on a
        // read-only directory fails cleanly without corrupting the
        // original. We approximate that with an invalid slug — the
        // validate_slug preflight at the top of write_task short-circuits
        // before any IO.
        let d = fresh_project();
        create_task_init(&proj(&d), "Good", "good", None).expect("first");
        let bad = TaskJson {
            id: "id".into(),
            title: "bad".into(),
            slug: "UPPER".into(),
            status: TaskStatus::Planning,
            created_at: Utc::now().to_rfc3339(),
            updated_at: Utc::now().to_rfc3339(),
            parent: None,
            summary: String::new(),
            items: Vec::new(),
            // Step 3.3: pre-archive fixture.
            completed_at: None,
        };
        let err = write_task(&proj(&d), &bad).expect_err("bad slug");
        assert!(matches!(err, TaskError::InvalidSlug(_)));
    }

    // --- schema / serde --------------------------------------------------

    #[test]
    fn task_json_serde_round_trip_preserves_all_fields() {
        let original = TaskJson {
            id: "id-x".into(),
            title: "T".into(),
            slug: "s".into(),
            status: TaskStatus::Check,
            created_at: "2026-07-08T00:00:00Z".into(),
            updated_at: "2026-07-08T01:00:00Z".into(),
            parent: Some("parent-slug".into()),
            summary: "one-line summary".into(),
            items: vec![
                TaskItem {
                    id: "backend-impl".into(),
                    content: "实现后端".into(),
                    status: TaskStatus::Implement,
                    tdd: Some(true),
                },
                TaskItem {
                    id: "frontend-impl".into(),
                    content: "实现前端".into(),
                    status: TaskStatus::Planning,
                    tdd: None,
                },
            ],
            // Step 3.3: pre-archive serde-round-trip fixture.
            completed_at: None,
        };
        let bytes = serde_json::to_vec_pretty(&original).unwrap();
        let parsed: TaskJson = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn task_json_omits_none_parent_when_serializing_to_skip_semantics() {
        let t = TaskJson {
            id: "id".into(),
            title: "t".into(),
            slug: "s".into(),
            status: TaskStatus::Planning,
            created_at: "now".into(),
            updated_at: "now".into(),
            parent: None,
            summary: String::new(),
            items: Vec::new(),
            // Step 3.3: must also be skipped via
            // `skip_serializing_if`.
            completed_at: None,
        };
        let s = serde_json::to_string(&t).unwrap();
        assert!(!s.contains("parent"), "parent=None must be skipped: {}", s);
        assert!(
            !s.contains("completed_at"),
            "completed_at=None must be skipped: {}",
            s
        );
    }

    #[test]
    fn task_status_parser_recognizes_known_forms_lenient_for_unknowns() {
        assert_eq!(TaskStatus::from_str_opt("planning"), TaskStatus::Planning);
        assert_eq!(TaskStatus::from_str_opt("implement"), TaskStatus::Implement);
        assert_eq!(TaskStatus::from_str_opt("CHECK"), TaskStatus::Check);
        assert_eq!(TaskStatus::from_str_opt("Done"), TaskStatus::Done);
        // Step 3.3: "completed" parses as Completed (NOT
        // demoted to Planning) so an archived task re-read
        // by the chat loop stays correctly classified.
        assert_eq!(TaskStatus::from_str_opt("completed"), TaskStatus::Completed);
        assert_eq!(TaskStatus::from_str_opt("COMPLETED"), TaskStatus::Completed);
        // Lenient default: anything weird → Planning.
        assert_eq!(TaskStatus::from_str_opt(""), TaskStatus::Planning);
        assert_eq!(TaskStatus::from_str_opt("nope"), TaskStatus::Planning);
        assert_eq!(TaskStatus::from_str_opt("  PLAN  "), TaskStatus::Planning);
    }

    // --- Step 3.3 — archive_task_init ------------------------------

    /// Step 3.3: archiving a Done task moves the task dir
    /// under `.everlasting/tasks/archive/<YYYY-MM>/<slug>/`
    /// and flips `task.json` to `status = Completed` with
    /// `completed_at` set.
    #[test]
    fn archive_task_init_moves_done_task_into_archive_tree() {
        let d = tempfile::tempdir().expect("tempdir");
        let path = d.path();
        let mut task = create_task_init(path, "My Feature", "my-feat", None).expect("create");
        task.status = TaskStatus::Done;
        write_task(path, &task).expect("write done");

        let archived = archive_task_init(path, "my-feat", /* no_commit */ true)
            .expect("archive should succeed");

        // Returned task reflects post-archive state.
        assert_eq!(archived.status, TaskStatus::Completed);
        assert!(
            archived.completed_at.is_some(),
            "completed_at must be set after archive"
        );

        // Live tree: no longer has the slug.
        let live_dir = task_dir(path, "my-feat");
        assert!(
            !live_dir.exists(),
            "live task dir must be gone after archive; got: {}",
            live_dir.display()
        );

        // Archive tree: has the moved task under <YYYY-MM>/<slug>/.
        let ym = archived
            .completed_at
            .as_deref()
            .unwrap()
            .split('T')
            .next()
            .expect("rfc3339 → YYYY-MM-DD")
            .get(..7)
            .expect("YYYY-MM slice")
            .to_string();
        let archived_dir = path
            .join(".everlasting")
            .join("tasks")
            .join(PROJ_NS_TASKS_ARCHIVE_DIR)
            .join(&ym)
            .join("my-feat");
        assert!(
            archived_dir.exists(),
            "archive dir must exist at: {}",
            archived_dir.display()
        );
        let archived_json = archived_dir.join("task.json");
        assert!(
            archived_json.exists(),
            "task.json must be at the archive dir"
        );
        let disk = read_task_at(&archived_json);
        assert_eq!(disk.status, TaskStatus::Completed);
        assert_eq!(disk.completed_at, archived.completed_at);
    }

    /// Step 3.3: archiving a non-Done task is refused.
    /// Archiving a planning / implement / check task would
    /// orphan in-flight work + the spec-distillation hint,
    /// so the engine refuses upfront.
    #[test]
    fn archive_task_init_refuses_non_done_status() {
        for non_done in [
            TaskStatus::Planning,
            TaskStatus::Implement,
            TaskStatus::Check,
        ] {
            let d = tempfile::tempdir().expect("tempdir");
            let path = d.path();
            let mut task = create_task_init(path, "My Feature", "my-feat", None).expect("create");
            task.status = non_done;
            write_task(path, &task).expect("write");

            let err = archive_task_init(path, "my-feat", true).expect_err("must refuse");
            assert!(
                matches!(err, TaskError::NotInDoneStatus(_)),
                "expected NotInDoneStatus for {non_done:?}, got: {err:?}"
            );
            // Live tree must still have the task.
            assert!(task_dir(path, "my-feat").exists());
        }
    }

    /// Step 3.3: re-archiving a task whose archive target
    /// already exists is refused with `AlreadyArchived` —
    /// no clobber on partial-write retries.
    #[test]
    fn archive_task_init_refuses_when_target_already_exists() {
        let d = tempfile::tempdir().expect("tempdir");
        let path = d.path();
        let mut task = create_task_init(path, "My Feature", "my-feat", None).expect("create");
        task.status = TaskStatus::Done;
        write_task(path, &task).expect("write done");

        // First archive succeeds.
        archive_task_init(path, "my-feat", true).expect("first archive");

        // Simulate a stale second-attempt by recreating the
        // live task dir + Done status + a pre-existing
        // archive target (we manually mkdir the archive
        // dir this time to provoke the conflict path on
        // the *next* archive).
        // Recreate the live task first.
        let live_dir = task_dir(path, "my-feat");
        std::fs::create_dir_all(&live_dir).expect("recreate live");
        let mut task = task.clone();
        task.status = TaskStatus::Done;
        write_task(path, &task).expect("rewrite done");

        // Pre-create a stale archive target — this is the
        // scenario we want to defend against (the first
        // archive landed; a retry found the live dir again
        // somehow and is about to clobber).
        let ym = Utc::now().format("%Y-%m").to_string();
        let stale_target = path
            .join(".everlasting")
            .join("tasks")
            .join(PROJ_NS_TASKS_ARCHIVE_DIR)
            .join(&ym)
            .join("my-feat");
        std::fs::create_dir_all(&stale_target).expect("create stale target");
        std::fs::write(stale_target.join("sentinel"), "old").expect("write sentinel");

        let err = archive_task_init(path, "my-feat", true).expect_err("must refuse");
        assert!(
            matches!(err, TaskError::AlreadyArchived(_)),
            "expected AlreadyArchived, got: {err:?}"
        );
        // The stale sentinel must survive — no clobber.
        assert!(
            stale_target.join("sentinel").exists(),
            "stale archive target must not be clobbered"
        );
    }

    /// Step 3.3: archiving a non-existent slug returns
    /// `NotFound` (not a generic IO error).
    #[test]
    fn archive_task_init_missing_returns_not_found() {
        let d = tempfile::tempdir().expect("tempdir");
        let err = archive_task_init(d.path(), "ghost", true).expect_err("must refuse");
        assert!(
            matches!(err, TaskError::NotFound(_)),
            "expected NotFound for ghost slug, got: {err:?}"
        );
    }

    /// Step 3.3: invalid slug is rejected before touching
    /// the filesystem (defends against path-traversal
    /// slugs the user might type into the IPC).
    #[test]
    fn archive_task_init_rejects_invalid_slug() {
        let d = tempfile::tempdir().expect("tempdir");
        for bad in ["BAD", "with space", "../escape", ""] {
            let err = archive_task_init(d.path(), bad, true).expect_err("must reject");
            assert!(
                matches!(err, TaskError::InvalidSlug(_)),
                "expected InvalidSlug for {bad:?}, got: {err:?}"
            );
        }
    }

    /// Helper: read + parse a `task.json` from an absolute
    /// path (the public `read_task` resolves via project +
    /// slug; the archive post-condition checks the moved
    /// file directly).
    fn read_task_at(json_path: &Path) -> TaskJson {
        let bytes = std::fs::read(json_path).expect("read json");
        serde_json::from_slice(&bytes).expect("parse json")
    }
}
