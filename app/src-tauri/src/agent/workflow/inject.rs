//! W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
//! per-session workflow context + per-turn breadcrumb injection
//! seam.
//!
//! ## Why a separate module (not part of `task.rs` or `def.rs`)
//!
//! `def.rs` is **pure data** (types + accessors + constant).
//! `task.rs` is **file IO** (read/write/init). Neither is
//! the right home for "consult the DB, list the disk, build a
//! session-scoped context struct, and inject per-turn state
//! reminders into `messages[0]`". `inject.rs` is the **engine
//! integration glue** — the bridge that knows about
//! `SqlitePool`, `ProjectRow`, `SessionRow`, and the
//! `messages[0]` cache-control contract.
//!
//! ## S-B invariant (Phase 0 design review §10.7 + S-B)
//!
//! > 注入一律 append 到 per-turn messages[0],`cache_control:
//! > None`,绝不新开 user message。
//!
//! Workflow breadcrumb injection MUST follow the same rule
//! as `memory_recall::inject_recall_into_turn`: append a
//! `ContentBlock::Text` (cache_control: None) to the existing
//! user-role Blocks message at `messages[0]`. If `messages[0]`
//! is NOT a user-role Blocks message (defensive — the B5
//! instruction-files load guarantees it for normal turns, but
//! edge cases exist), the helper MUST log a `warn!` and skip
//! — NEVER prepend a synthetic user message, which would
//! break the Anthropic cache breakpoint at `messages[0]`.
//!
//! ## Engine contract
//!
//! - [`build_workflow_ctx`] (async, DB-heavy) is called ONCE
//!   per IPC entry from `agent::chat::chat`. Resolves the
//!   current task eagerly so the per-turn loop can read it
//!   in O(1) — avoids re-listing `.everlasting/tasks/` on
//!   every turn of the 200-turn loop.
//! - [`append_workflow_breadcrumb`] (sync, hot-path) is
//!   called in the per-turn loop AFTER
//!   `memory_recall::inject_recall_into_turn`. Same Vec
//!   (`turn_messages`) gets the breadcrumb pushed onto
//!   `messages[0]`'s block list — never wrapped in a new
//!   `ChatMessage`.
//!
//! ## Phase scope
//!
//! - **Step 0.5**: ship `WorkflowCtx` + breadcrumb append +
//!   eager `current_task` resolution at IPC entry.
//! - **Phase 2 Step 2.5**: add `append_delegation_template`
//!   sibling that reads `WorkflowCtx.current_task` + the
//!   dispatched `SubagentDef`'s role and injects the
//!   per-template placeholder-filled text after this helper.
//! - **Phase 3 Step 3.1**: `set_task_state` mutates
//!   `WorkflowCtx.current_task.status` in place; the next
//!   turn's breadcrumb picks up the new state automatically.

use std::path::Path;

use sqlx::SqlitePool;

// `default_workflow` is only used by `#[cfg(test)] mod tests`
// below; silencing the warning at the import site keeps the
// production binary warning-free.
#[allow(unused_imports)]
use crate::agent::workflow::{
    breadcrumb_for, default_workflow, delegation_template_for, load_workflow, read_task, TaskJson,
    WorkflowDef,
};
use crate::db;
use crate::llm::types::{ChatMessage, ContentBlock, MessageContent, Role};

// ---------------------------------------------------------------------------
// WorkflowCtx
// ---------------------------------------------------------------------------

/// Session-scoped workflow context. Lives for the duration
/// of one `run_chat_loop` call (one user message's 200-turn
/// loop). Cheap to clone (small POD struct: 2 strings +
/// 1 nested `Option<TaskJson>` of <1 KiB on disk).
///
/// Construction is async + DB-heavy ([`build_workflow_ctx`])
/// and runs ONCE at the IPC entry. The per-turn loop reads
/// these fields through `&WorkflowCtx` — no DB I/O on the
/// hot path.
#[derive(Debug, Clone)]
pub struct WorkflowCtx {
    /// The active plugin's state machine + breadcrumb +
    /// delegation templates. Phase 0 sources this from
    /// `default_workflow()`; Phase 2 swaps in
    /// `load_workflow(name, project_path)` (with fallback
    /// to `default_workflow()` on serde / validate failure).
    pub workflow_def: WorkflowDef,

    /// The "current" workflow task. Eagerly resolved on
    /// IPC entry (NOT per-turn) by listing the project's
    /// `.everlasting/tasks/`, parsing each `task.json`,
    /// picking the first with `status != Done`.
    ///
    /// `None` when:
    /// - the project has no `.everlasting/tasks/` dir yet
    ///   (fresh workflow session), OR
    /// - every existing task is already `status = Done`
    ///   (everything archived; nothing to continue).
    ///
    /// The LLM agent can still call `update_checklist` /
    /// `create_task` to start a new task mid-loop; the
    /// breadcrumb reflects the cached value until the next
    /// IPC invocation re-resolves. Phase 3 will add a
    /// mid-loop `set_task_state` hook that mutates this
    /// in place so mid-loop state transitions immediately
    /// surface.
    pub current_task: Option<TaskJson>,
}

// ---------------------------------------------------------------------------
// build_workflow_ctx — IPC-entry resolution
// ---------------------------------------------------------------------------

/// Build a `WorkflowCtx` for the session if `workflow_enabled`
/// is on; return `None` otherwise. Async because the
/// `sessions.workflow_enabled` read + project lookup +
/// `.everlasting/tasks/*/task.json` parse are all DB / IO
/// bounded.
///
/// **Cost target**: ~10 ms on a warm pool (one SELECT +
/// one project lookup + at most a handful of small file
/// reads). Cached at IPC entry, so the per-turn loop has
/// zero overhead when the field is `None`.
///
/// **Pre-condition**: `SessionRow.workflow_enabled`
/// (added in Step 0.1). When `false`, returns `Ok(None)`
/// without touching the project's `.everlasting/tasks/`
/// — this is the "opt-in" gate at the engine entry, so
/// non-workflow sessions are byte-identical to pre-Step-0.5
/// behavior.
///
/// **Error policy**: DB errors propagate as `Err(sqlx::Error)`
/// so the IPC layer surfaces them as `Server`-category
/// `AppCommandError`. File errors during current_task
/// resolution are LOGGED + swallowed (we set
/// `current_task = None` and continue — a corrupted
/// `task.json` must NOT break the chat loop).
pub async fn build_workflow_ctx(
    db: &SqlitePool,
    session_id: &str,
) -> Result<Option<WorkflowCtx>, sqlx::Error> {
    // 1. Load the session row. workflow_enabled is the
    //    feature gate (Step 0.1 B5 / B7-style). When `false`
    //    we return `None` BEFORE touching the task dir —
    //    no workflow overhead on non-workflow sessions.
    let loaded = match db::load_session(db, session_id).await? {
        Some(l) => l,
        None => {
            // Stale session id (deleted between IPC entry
            // and now). Treat as workflow-disabled — chat
            // proceeds without workflow context.
            tracing::warn!(
                session_id = %session_id,
                "build_workflow_ctx: session not found; treating as workflow-disabled"
            );
            return Ok(None);
        }
    };
    if !loaded.session.workflow_enabled {
        return Ok(None);
    }

    // 2. Look up the project root. Empty / missing project
    //    is treated as "no task yet" rather than an error
    //    — the breadcrumb will hint the LLM to call
    //    `create_task` / `list_dir` to bootstrap.
    let project = match db::get_project(db, &loaded.session.project_id).await? {
        Some(p) => p,
        None => {
            tracing::warn!(
                session_id = %session_id,
                project_id = %loaded.session.project_id,
                "build_workflow_ctx: project not found; returning ctx with no current_task",
            );
            // Step 2.2: still consult `load_workflow` (now
            // parametrized on the session's plugin_name) —
            // missing project is a task-resolution failure,
            // not a plugin-resolution failure.
            let plugin_name = &loaded.session.plugin_name;
            let project_path = std::path::PathBuf::new();
            return Ok(Some(WorkflowCtx {
                workflow_def: load_workflow(plugin_name, &project_path.to_string_lossy()),
                current_task: None,
            }));
        }
    };

    let project_path = std::path::PathBuf::from(&project.path);
    let current_task = resolve_current_task(&project_path).await;

    // Step 2.2 (2026-07-08): load the on-disk plugin
    // (`<project>/.everlasting/workflow/<plugin_name>/workflow.json`)
    // instead of the in-memory `default_workflow()` constant.
    // The loader validates and falls back to `default_workflow()`
    // on any failure (missing file / malformed JSON / validation
    // error), so a stale `plugin_name` never breaks the engine —
    // it just gets the dev workflow until the user picks a real
    // one via `PluginSelect.vue`.
    let plugin_name = &loaded.session.plugin_name;
    let workflow_def = load_workflow(plugin_name, &project.path);

    Ok(Some(WorkflowCtx {
        workflow_def,
        current_task,
    }))
}

/// List `<project>/.everlasting/tasks/*/task.json`, parse
/// each, return the FIRST with `status != Done`.
///
/// "First" means **lexicographic by slug** — deterministic
/// + matches the on-disk listing of `fs::read_dir`. Phase 3
/// may upgrade to "most-recently-updated" via the
/// `updated_at` field; for Phase 0 the deterministic order
/// is enough (there's typically 0 or 1 active task per
/// project — finishing one archives it before the next
/// starts, per the Step 3.3 `archive_task` IPC).
///
/// **Per-file errors are swallowed**: a corrupt
/// `task.json` is logged + skipped so a single bad task
/// doesn't break the whole list. The function returns
/// the first VALID unfinished task, not the first
/// encountered file.
pub async fn resolve_current_task(project_path: &Path) -> Option<TaskJson> {
    let tasks_root = project_path.join(".everlasting").join("tasks");
    if !tasks_root.exists() {
        return None;
    }
    let mut entries = match std::fs::read_dir(&tasks_root) {
        Ok(it) => it,
        Err(e) => {
            tracing::warn!(
                path = %tasks_root.display(),
                error = %e,
                "resolve_current_task: failed to read tasks dir; treating as no current_task",
            );
            return None;
        }
    };

    // Collect (slug, path) pairs so we can sort by slug
    // for determinism — `read_dir` order varies by FS on
    // Linux ext4 vs APFS vs NTFS.
    let mut slugs: Vec<(String, std::path::PathBuf)> = Vec::new();
    loop {
        match entries.next() {
            Some(Ok(entry)) => {
                let file_type = match entry.file_type() {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                if !file_type.is_dir() {
                    continue;
                }
                let slug = entry.file_name().to_string_lossy().into_owned();
                let json_path = entry.path().join("task.json");
                if json_path.exists() {
                    slugs.push((slug, json_path));
                }
            }
            Some(Err(e)) => {
                tracing::warn!(
                    error = %e,
                    "resolve_current_task: read_dir iteration failed mid-stream; skipping remaining",
                );
                break;
            }
            None => break,
        }
    }
    slugs.sort_by(|a, b| a.0.cmp(&b.0));

    for (slug, json_path) in slugs {
        match read_task(project_path, &slug) {
            Ok(task) => {
                // Skip terminal statuses. Step 3.3 added
                // `Completed` (= post-archive state) to the
                // skip list alongside `Done`. Two reasons:
                //   1. A `Completed` task lives under
                //      `tasks/archive/<YYYY-MM>/<slug>/`,
                //      not `tasks/<slug>/`, so
                //      `read_task` will normally fail
                //      with `NotFound` and the error
                //      branch swallows + continues. The
                //      explicit skip is defensive — if a
                //      future change moves archive back
                //      under the live tree (or someone
                //      hand-edits status), we still skip.
                //   2. Symmetry: `Done` and `Completed`
                //      are both terminal — neither
                //      should be auto-resumed.
                if task.status != crate::agent::workflow::TaskStatus::Done
                    && task.status != crate::agent::workflow::TaskStatus::Completed
                {
                    return Some(task);
                }
            }
            Err(e) => {
                tracing::warn!(
                    slug = %slug,
                    path = %json_path.display(),
                    error = %e,
                    "resolve_current_task: failed to read task.json; skipping (continue resolution)",
                );
                // Per-file error swallow — continue looking.
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// append_workflow_breadcrumb — per-turn injection
// ---------------------------------------------------------------------------

/// Append the workflow breadcrumb to `messages[0]`. Mirrors
/// `memory_recall::inject_recall_into_turn`'s safety
/// contract precisely:
///
/// - Find `messages[0]`; if it's a user-role
///   `MessageContent::Blocks`, push a new
///   `ContentBlock::Text { text, cache_control: None }`.
/// - If the precondition fails (NOT user-role OR NOT
///   Blocks), log a `warn!` and skip — never prepend a
///   synthetic user message (S-B invariant; prepending
///   would bust the memory cache breakpoint at
///   `messages[0]`).
///
/// Called from the per-turn loop in `run_chat_loop`,
/// *after* `inject_recall_into_turn`. The two helpers
/// share `messages[0]`'s block list; both rely on the
/// same S-B guard so the contract is consistent across
/// all `messages[0]`-mutating helpers.
///
/// Returns `true` when the breadcrumb was appended,
/// `false` when the S-B guard tripped (caller doesn't
/// branch on this — the log line is the signal).
pub fn append_workflow_breadcrumb(turn_messages: &mut Vec<ChatMessage>, ctx: &WorkflowCtx) -> bool {
    let block = build_breadcrumb_block(ctx);
    if let Some(first) = turn_messages.first_mut() {
        if first.role == Role::User {
            if let MessageContent::Blocks(ref mut blocks) = first.content {
                blocks.push(block);
                return true;
            }
        }
    }
    tracing::warn!(
        "append_workflow_breadcrumb: messages[0] is not a user-role Blocks message; \
         S-B guard forbids prepending a synthetic user message. Breadcrumb skipped \
         (turn proceeds without state reminder)."
    );
    false
}

// ---------------------------------------------------------------------------
// W1 (Workflow integration, Step 2.5 — 2026-07-08):
// delegation template — filled plugin-role system prompt
// for a worker turn. Lives in `inject.rs` next to
// `append_workflow_breadcrumb` because both target the
// same messages[0] block list with the same S-B guard.
//
// **Step 2.5 contract**:
// - `compute_delegation_template` substitutes `{title}`
//   / `{summary}` / `{state}` / `{relevant_specs}` from
//   the workflow_ctx + project_path. Returns `None`
//   when the plugin doesn't define a template for the
//   role (caller falls back to the sub-agent's own
//   system prompt).
// - `append_delegation_template` mutates `messages[0]`
//   to push the filled template as a Text block. The
//   same S-B guard as the breadcrumb helper: must be a
//   user-role Blocks message or the append is silently
//   skipped (with a `warn!`).
// - `cache_control: None` — delegation templates are
//   per-dispatch (not per-turn stable), so they MUST
//   NOT mark a cache breakpoint.
// ---------------------------------------------------------------------------

/// Compute the filled delegation template for `role` in
/// the current `workflow_ctx`. Returns `None` when the
/// plugin doesn't define a template for the role.
///
/// **Substitution placeholders**:
/// - `{title}` → `current_task.title` (empty when no task)
/// - `{summary}` → `current_task.summary` (empty when no task)
/// - `{state}` → `current_task.status` string (e.g.
///   `"planning"` / `"in_progress"` / `"done"`)
///
/// **`{relevant_specs}`** (Step 2.5): scans
/// `<project>/.everlasting/spec/` for `.md` files when
/// the directory exists. The scan is a flat listing for
/// now (FTS5 over `task.summary` is a Phase 3 refinement).
/// When the spec dir is missing (Phase 3.2 not yet
/// landed), or no files match, the placeholder resolves
/// to `(auto-detect via wf-before-dev)` so the worker
/// always gets an actionable hint.
///
/// The placeholder substitution is intentionally
/// permissive — a missing placeholder in the template
/// text stays verbatim (the LLM sees the literal
/// `{title}` and can flag it as a plugin-author bug).
pub fn compute_delegation_template(
    workflow_ctx: &WorkflowCtx,
    project_path: &str,
    role: &str,
) -> Option<String> {
    let raw = delegation_template_for(&workflow_ctx.workflow_def, role)?;
    let title = workflow_ctx
        .current_task
        .as_ref()
        .map(|t| t.title.as_str())
        .unwrap_or("");
    let summary = workflow_ctx
        .current_task
        .as_ref()
        .map(|t| t.summary.as_str())
        .unwrap_or("");
    let state = workflow_ctx
        .current_task
        .as_ref()
        .map(|t| t.status.as_str())
        .unwrap_or("");
    let relevant_specs = resolve_relevant_specs(project_path);
    Some(
        raw.replace("{title}", title)
            .replace("{summary}", summary)
            .replace("{state}", state)
            .replace("{relevant_specs}", &relevant_specs),
    )
}

/// Scan `<project>/.everlasting/spec/` for `.md` files
/// and return a comma-separated path list (relative to
/// project root). Missing dir or no files →
/// `(auto-detect via wf-before-dev)`.
///
/// Step 2.5 ships a flat listing (no FTS5 over
/// `task.summary` yet — that's a Phase 3 refinement per
/// the design doc §5.4). The hint text tells the worker
/// to use the `wf-before-dev` skill as a fallback when
/// the spec index is empty, matching the workflow
/// breadcrumb's "use wf-before-dev when nothing's
/// pinned" convention.
fn resolve_relevant_specs(project_path: &str) -> String {
    let spec_dir = std::path::Path::new(project_path)
        .join(".everlasting")
        .join("spec");
    if !spec_dir.exists() {
        return "(auto-detect via wf-before-dev)".to_string();
    }
    // Recursive walk: the spec tree is structured
    // (`<package>/<layer>/index.md` + guideline files,
    // per `.trellis/spec/...` convention). A flat
    // top-level listing would miss any `.md` nested in
    // package subdirs. Step 2.5 ships a depth-first walk
    // without following symlinks (defensive — the spec
    // tree should not contain any).
    let mut paths: Vec<String> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![spec_dir.clone()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(it) => it,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = entry.file_type().ok();
            let is_dir = file_type.as_ref().map(|t| t.is_dir()).unwrap_or(false);
            let is_file = file_type.as_ref().map(|t| t.is_file()).unwrap_or(false);
            let is_symlink = file_type.as_ref().map(|t| t.is_symlink()).unwrap_or(false);
            if is_symlink {
                // Skip symlinks — the spec tree is a
                // plain directory hierarchy; any
                // symlink is a misconfiguration and
                // could escape the project root.
                continue;
            }
            if is_dir {
                stack.push(path);
                continue;
            }
            if !is_file {
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            // Path relative to project root so the
            // template text is portable (works whether
            // project_path is absolute or relative).
            //
            // `strip_prefix` requires both sides to be
            // canonically equivalent. Fall back to
            // canonicalize-and-strip when the paths
            // diverge (e.g. on macOS where `/tmp` is a
            // symlink to `/private/tmp`).
            let rel: String = match path.strip_prefix(project_path) {
                Ok(p) => p.to_string_lossy().into_owned(),
                Err(_) => {
                    let canon_path = path.canonicalize().ok();
                    let canon_root = std::path::Path::new(project_path).canonicalize().ok();
                    match (canon_path, canon_root) {
                        (Some(p), Some(r)) => p
                            .strip_prefix(&r)
                            .map(|x| x.to_string_lossy().into_owned())
                            .unwrap_or_else(|_| p.to_string_lossy().into_owned()),
                        _ => path.to_string_lossy().into_owned(),
                    }
                }
            };
            paths.push(rel);
        }
    }
    paths.sort();
    if paths.is_empty() {
        "(auto-detect via wf-before-dev)".to_string()
    } else {
        paths.join(", ")
    }
}

/// Push the filled delegation template (a Text block) to
/// `messages[0]`'s block list. Same S-B guard as
/// `append_workflow_breadcrumb` — silently skipped +
/// `warn!` when messages[0] isn't a user-role Blocks
/// message.
///
/// `cache_control: None` — delegation templates are
/// per-dispatch (not per-turn stable), so they MUST NOT
/// become a cache breakpoint marker. They live alongside
/// the breadcrumb on messages[0], but never participate
/// in the worker's prompt-cache breakpoint.
///
/// Returns `true` when appended, `false` when the guard
/// tripped or the template is `None` (no plugin
/// template for this role → caller falls back to the
/// sub-agent's own system prompt).
pub fn append_delegation_template(
    turn_messages: &mut Vec<ChatMessage>,
    body: Option<String>,
) -> bool {
    let body = match body {
        Some(b) => b,
        None => return false,
    };
    let block = ContentBlock::Text {
        text: body,
        cache_control: None,
    };
    if let Some(first) = turn_messages.first_mut() {
        if first.role == Role::User {
            if let MessageContent::Blocks(ref mut blocks) = first.content {
                blocks.push(block);
                return true;
            }
        }
    }
    tracing::warn!(
        "append_delegation_template: messages[0] is not a user-role Blocks message; \
         S-B guard forbids prepending a synthetic user message. Template skipped \
         (worker proceeds without plugin role customization)."
    );
    false
}

/// Compose the breadcrumb Text block. Three layers:
///
/// 1. The plugin's state breadcrumb (`WorkflowDef::breadcrumb[state]`)
///    — empty string fallback when the state has no entry.
/// 2. The current task's id/title/slug/status (only when
///    `current_task.is_some()`).
/// 3. A pointer to the task's `prd.md` / `design.md` /
///    `progress.md` paths so the LLM knows where to look
///    for further context.
///
/// `cache_control: None` — the breadcrumb changes
/// per-turn (status can flip mid-loop), so it MUST NOT
/// become a cache breakpoint marker. Phase 3's
/// `set_task_state` makes the breadcrumb vary even
/// within a single loop, reinforcing the same invariant.
fn build_breadcrumb_block(ctx: &WorkflowCtx) -> ContentBlock {
    let state_str = ctx
        .current_task
        .as_ref()
        .map(|t| t.status.as_str())
        .unwrap_or("planning");
    let breadcrumb = breadcrumb_for(&ctx.workflow_def, state_str);

    let body = match &ctx.current_task {
        Some(task) => {
            // task-fragment branch: 3-line metadata +
            // breadcrumb text + pointer.
            format!(
                "<workflow-task-meta>\n\
                 task_id: {id}\n\
                 title: {title}\n\
                 slug: {slug}\n\
                 status: {status}\n\
                 </workflow-task-meta>\n\n\
                 {breadcrumb}\n\n\
                 task_dir: <project>/.everlasting/tasks/{slug}/\n\
                 read prd.md / design.md / progress.md for full context.\n",
                id = task.id,
                title = task.title,
                slug = task.slug,
                status = task.status.as_str(),
            )
        }
        None => {
            // Bootstrap branch: hint the LLM to call
            // the create_task tool (NOT a hand-written
            // write_file — resilience is on the read side
            // via lenient read_task, but create_task is the
            // lower-effort, lower-drift path). NOT empty —
            // the user enabled workflow, so the LLM needs
            // to know it should bootstrap the machinery.
            format!(
                "<workflow-task-meta>\n\
                 no active task — call the create_task tool to start one (省事、字段全、自带 prd skeleton)。\\n\
                 (write_file 也行,但 task.json schema 有约束;read_task 会 lenient 兜底,优先用 create_task tool 更稳。)\n\
                 </workflow-task-meta>\n\n\
                 {breadcrumb}\n",
            )
        }
    };

    ContentBlock::Text {
        text: body,
        cache_control: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::workflow::{default_workflow, TaskStatus};
    use crate::llm::types::{CacheControl, MessageContent, Role};

    /// 1-block user message at index 0 — the canonical
    /// "B5 instruction-loaded" precondition that S-B
    /// requires.
    fn fresh_user_message() -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::Text {
                text: "user-typed content".to_string(),
                cache_control: Some(CacheControl::Ephemeral),
            }]),
        }
    }

    fn sample_ctx_with_task() -> WorkflowCtx {
        WorkflowCtx {
            workflow_def: default_workflow(),
            current_task: Some(TaskJson {
                id: "t1".into(),
                title: "Sample task".into(),
                slug: "sample".into(),
                status: TaskStatus::InProgress,
                created_at: "2026-07-08T00:00:00Z".into(),
                updated_at: "2026-07-08T00:00:00Z".into(),
                parent: None,
                summary: "demo".into(),
                items: vec![],
                // Step 3.3: pre-archive fixture.
                completed_at: None,
            }),
        }
    }

    fn sample_ctx_no_task() -> WorkflowCtx {
        WorkflowCtx {
            workflow_def: default_workflow(),
            current_task: None,
        }
    }

    // --- append happy-path ----------------------------------------------

    #[test]
    fn append_workflow_breadcrumb_appends_block_to_user_messages_zero() {
        let mut msgs = vec![fresh_user_message()];
        let initial_block_count = match &msgs[0].content {
            MessageContent::Blocks(bs) => bs.len(),
            _ => unreachable!(),
        };
        let appended = append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
        assert!(appended);

        // First message still user-role Blocks; block count
        // grew by exactly one (no synthetic message added).
        assert_eq!(msgs.len(), 1);
        match &msgs[0].content {
            MessageContent::Blocks(bs) => {
                assert_eq!(bs.len(), initial_block_count + 1);
                // NEW block must be cache_control: None,
                // NOT inheriting the user's marker.
                match bs.last().unwrap() {
                    ContentBlock::Text { cache_control, .. } => {
                        assert!(
                            cache_control.is_none(),
                            "breadcrumb block must NOT carry a cache_control marker (S-B)",
                        );
                    }
                    _ => panic!("expected text block"),
                }
            }
            _ => panic!("messages[0] content shape corrupted"),
        }
    }

    #[test]
    fn append_workflow_breadcrumb_bootstrap_branch_when_no_current_task() {
        let mut msgs = vec![fresh_user_message()];
        let appended = append_workflow_breadcrumb(&mut msgs, &sample_ctx_no_task());
        assert!(appended);
        match &msgs[0].content {
            MessageContent::Blocks(bs) => {
                let last = bs.last().unwrap();
                match last {
                    ContentBlock::Text { text, .. } => {
                        assert!(
                            text.contains("workflow-task-meta"),
                            "bootstrap hint must include the meta marker"
                        );
                        assert!(
                            text.contains("create_task"),
                            "bootstrap hint must point at the create_task tool (got: {text})"
                        );
                    }
                    _ => panic!("expected text block"),
                }
            }
            _ => panic!("messages[0] content shape corrupted"),
        }
    }

    #[test]
    fn breadcrumb_text_reflects_current_state_via_workflow_def() {
        // Confirms the per-turn reflective intent: the
        // block's text mentions the dev plugin's state
        // breadcrumb for the current task's status
        // (`in_progress`).
        let mut msgs = vec![fresh_user_message()];
        append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
        match &msgs[0].content {
            MessageContent::Blocks(bs) => match bs.last().unwrap() {
                ContentBlock::Text { text, .. } => {
                    // dev plugin's `in_progress` breadcrumb
                    // references `wf-before-dev`; cheap
                    // identity probe.
                    assert!(
                        text.contains("wf-before-dev"),
                        "in_progress-state breadcrumb text missing: {}",
                        text
                    );
                }
                _ => panic!("expected text block"),
            },
            _ => panic!("messages[0] content shape corrupted"),
        }
    }

    /// Step 1.4 — artifact 查阅机制:`task.json` 元数据(id/title/
    /// slug/status)+ breadcrumb 文本 + prd/design/progress path pointer
    /// 必须随 state breadcrumb 一起进 messages[0],让 LLM 知道当前
    /// task + 全文去哪读。
    #[test]
    fn breadcrumb_includes_task_meta_and_artifact_paths() {
        let mut msgs = vec![fresh_user_message()];
        append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
        let text = match &msgs[0].content {
            MessageContent::Blocks(bs) => match bs.last().unwrap() {
                ContentBlock::Text { text, .. } => text.clone(),
                _ => panic!("expected text block"),
            },
            _ => panic!("messages[0] content shape corrupted"),
        };

        // task.json meta 全部进 breadcrumb
        assert!(
            text.contains("task_id: t1"),
            "missing task_id, got: {}",
            text
        );
        assert!(
            text.contains("title: Sample task"),
            "missing title, got: {}",
            text
        );
        assert!(text.contains("slug: sample"), "missing slug, got: {}", text);
        assert!(
            text.contains("status: in_progress"),
            "missing status, got: {}",
            text
        );

        // artifact path pointer — prd/design/progress 全文路径都给
        // LLM 走 read_file 自取,engine 不预加载
        assert!(
            text.contains(".everlasting/tasks/sample/"),
            "missing task_dir pointer, got: {}",
            text
        );
        assert!(
            text.contains("prd.md"),
            "missing prd.md path, got: {}",
            text
        );
        assert!(
            text.contains("design.md"),
            "missing design.md path, got: {}",
            text
        );
        assert!(
            text.contains("progress.md"),
            "missing progress.md path, got: {}",
            text
        );

        // meta 容器包裹(便于 LLM parse)
        assert!(
            text.contains("<workflow-task-meta>") && text.contains("</workflow-task-meta>"),
            "missing <workflow-task-meta> wrapper, got: {}",
            text
        );
    }

    // --- S-B guard -----------------------------------------------------

    #[test]
    fn append_workflow_breadcrumb_skips_when_messages_empty() {
        let mut msgs: Vec<ChatMessage> = vec![];
        // Should NOT prepend a synthetic user message —
        // S-B forbid.
        let appended = append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
        assert!(!appended);
        assert!(
            msgs.is_empty(),
            "S-B: empty Vec stays empty (no synthetic prepend)"
        );
    }

    #[test]
    fn append_workflow_breadcrumb_skips_when_first_message_is_assistant() {
        let mut msgs = vec![ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::Text {
                text: "i'm the assistant; not a user".to_string(),
                cache_control: None,
            }]),
        }];
        let appended = append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
        assert!(!appended);
        assert_eq!(
            msgs.len(),
            1,
            "S-B: no synthetic prepend; assistant stays at index 0"
        );
        match &msgs[0].content {
            MessageContent::Blocks(bs) => assert_eq!(bs.len(), 1, "no block appended"),
            _ => panic!("messages[0] content shape corrupted"),
        }
    }

    #[test]
    fn append_workflow_breadcrumb_skips_when_user_role_message_is_text_only() {
        // User-role but plain `MessageContent::Text(...)` (no
        // Blocks). The helper cannot append to a Text
        // payload; per S-B, it MUST skip rather than wrap
        // the breadcrumb in a new user message.
        let mut msgs = vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Text("plain string".to_string()),
        }];
        let appended = append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
        assert!(!appended);
        assert_eq!(msgs.len(), 1, "S-B: no synthetic prepend");
        match &msgs[0].content {
            MessageContent::Text(s) => assert_eq!(s, "plain string"),
            _ => panic!("messages[0] content shape changed unexpectedly"),
        }
    }

    // --- coherence with the user-role Blocks precondition ---

    #[test]
    fn append_workflow_breadcrumb_preserves_existing_blocks_order() {
        let mut msgs = vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: "instruction banner".to_string(),
                    cache_control: Some(CacheControl::Ephemeral),
                },
                ContentBlock::Text {
                    text: "agent body".to_string(),
                    cache_control: None,
                },
            ]),
        }];
        append_workflow_breadcrumb(&mut msgs, &sample_ctx_with_task());
        match &msgs[0].content {
            MessageContent::Blocks(bs) => {
                // Original first two blocks preserved verbatim.
                assert_eq!(bs.len(), 3);
                match &bs[0] {
                    ContentBlock::Text { text, .. } => assert_eq!(text, "instruction banner"),
                    _ => panic!(),
                }
                match &bs[1] {
                    ContentBlock::Text { text, .. } => assert_eq!(text, "agent body"),
                    _ => panic!(),
                }
                // New third block is the breadcrumb.
                match &bs[2] {
                    ContentBlock::Text {
                        text,
                        cache_control,
                    } => {
                        assert!(cache_control.is_none());
                        assert!(text.contains("workflow-task-meta"));
                    }
                    _ => panic!(),
                }
            }
            _ => panic!(),
        }
    }

    // --- resolve_current_task on a tempdir ---

    #[tokio::test]
    async fn resolve_current_task_returns_none_when_no_tasks_dir() {
        let d = tempfile::tempdir().unwrap();
        assert!(resolve_current_task(d.path()).await.is_none());
    }

    #[tokio::test]
    async fn resolve_current_task_picks_first_unfinished_task() {
        // Setup: 3 task dirs in `.everlasting/tasks/`:
        //   - `archived-task` → status = Done (skip)
        //   - `do-this-now`   → status = Implement (pick
        //                        FIRST by lexicographic
        //                        slug sort)
        //   - `zzz-other`      → status = Planning (also
        //                         unfinished)
        // Expected: `do-this-now` wins.
        let project = tempfile::tempdir().unwrap();
        let tasks = project.path().join(".everlasting/tasks");
        for (slug, status) in [
            ("archived-task", TaskStatus::Done),
            ("do-this-now", TaskStatus::InProgress),
            ("zzz-other", TaskStatus::Planning),
        ] {
            let dir = tasks.join(slug);
            std::fs::create_dir_all(&dir).unwrap();
            let task = TaskJson {
                id: format!("id-{slug}"),
                title: slug.to_string(),
                slug: slug.to_string(),
                status,
                created_at: "2026-07-08T00:00:00Z".into(),
                updated_at: "2026-07-08T00:00:00Z".into(),
                parent: None,
                summary: String::new(),
                items: vec![],
                // Step 3.3: pre-archive fixture.
                completed_at: None,
            };
            crate::agent::workflow::write_task(project.path(), &task).unwrap();
        }

        let resolved = resolve_current_task(project.path()).await;
        let resolved = resolved.expect("at least one unfinished task exists");
        assert_eq!(resolved.slug, "do-this-now");
        assert_eq!(resolved.status, TaskStatus::InProgress);
    }

    #[tokio::test]
    async fn resolve_current_task_returns_none_when_all_done() {
        let project = tempfile::tempdir().unwrap();
        let tasks = project.path().join(".everlasting/tasks");
        for slug in ["a-task", "b-task"] {
            let dir = tasks.join(slug);
            std::fs::create_dir_all(&dir).unwrap();
            let task = TaskJson {
                id: format!("id-{slug}"),
                title: slug.to_string(),
                slug: slug.to_string(),
                status: TaskStatus::Done,
                created_at: "2026-07-08T00:00:00Z".into(),
                updated_at: "2026-07-08T00:00:00Z".into(),
                parent: None,
                summary: String::new(),
                items: vec![],
                // Step 3.3: pre-archive fixture.
                completed_at: None,
            };
            crate::agent::workflow::write_task(project.path(), &task).unwrap();
        }
        assert!(resolve_current_task(project.path()).await.is_none());
    }

    #[tokio::test]
    async fn resolve_current_task_skips_corrupt_task_json_and_picks_valid_one() {
        // Defensive: the per-file error swallow contract.
        // A corrupt task.json must NOT break the resolution —
        // we walk past it to the next valid one.
        let project = tempfile::tempdir().unwrap();
        let tasks = project.path().join(".everlasting/tasks");
        // bad: invalid JSON
        std::fs::create_dir_all(tasks.join("bad")).unwrap();
        std::fs::write(
            tasks.join("bad/task.json"),
            b"this is not valid json at all",
        )
        .unwrap();
        // good: valid task
        std::fs::create_dir_all(tasks.join("good")).unwrap();
        let task = TaskJson {
            id: "id-good".into(),
            title: "Good".into(),
            slug: "good".into(),
            status: TaskStatus::Planning,
            created_at: "2026-07-08T00:00:00Z".into(),
            updated_at: "2026-07-08T00:00:00Z".into(),
            parent: None,
            summary: String::new(),
            items: vec![],
            // Step 3.3: pre-archive fixture.
            completed_at: None,
        };
        crate::agent::workflow::write_task(project.path(), &task).unwrap();

        let resolved = resolve_current_task(project.path()).await;
        assert!(resolved.is_some(), "should walk past bad task.json");
        assert_eq!(resolved.unwrap().slug, "good");
    }
}
