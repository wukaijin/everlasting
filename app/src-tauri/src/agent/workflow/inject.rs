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
//! reminders into the request tail". `inject.rs` is the
//! **engine integration glue** — the bridge that knows about
//! `SqlitePool`, `ProjectRow`, `SessionRow`, and the
//! cache-control contract on user-role messages.
//!
//! ## S-B invariant (Phase 0 design review §10.7 + S-B)
//!
//! > 注入一律 append 到既有 user-role 消息,`cache_control:
//! > None`,绝不(向头部)新开 user message。
//!
//! **Tail sink (08-31-cache-head-volatility D1)**: the breadcrumb
//! used to be appended to `messages[0]` (the instructions
//! synthetic head). The OpenAI-compatible path has a strict
//! byte-0 prefix cache with no `cache_control` breakpoints, and
//! the breadcrumb flips per-turn (status can change mid-loop), so
//! a head-positioned breadcrumb busted the whole cached prefix on
//! every state transition (live evidence: session d6728b3a seq
//! 435, cache_read=0 on a 280k-token request). The block now
//! appends to the **last** message of the per-turn request clone
//! — the same position family as the loop-hint (tail = the newest
//! content, chronologically correct for a "current state"
//! reminder). The S-B guard semantics are preserved: append into
//! an existing user-role message (`Text` payloads are widened to
//! `Blocks` in place); NEVER prepend a synthetic user message,
//! which would break the Anthropic cache breakpoint at
//! `messages[0]`. When the guard trips (empty Vec / last message
//! is assistant), the helper logs a `warn!` and skips.
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
//!   (`turn_messages`) gets the breadcrumb pushed onto the
//!   LAST message's block list — never wrapped in a new
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
use crate::llm::types::{ChatMessage, ContentBlock};

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
    ///
    /// Reflects the **session's** plugin — drives skill/tool
    /// surfacing (e.g. review session shows the `reviewer`
    /// dispatch enum). For task-state invariants (role gate,
    /// transition validation) use [`task_workflow_def`], which
    /// tracks the **task's** owning plugin and stays stable
    /// across mid-session plugin switches.
    pub workflow_def: WorkflowDef,

    /// C5 (2026-07-28): the **task's** owning plugin's state
    /// machine. When `current_task` is `Some`, this is
    /// `load_workflow(task.workflow_plugin, ...)`; when
    /// `None`, it falls back to `workflow_def` (session
    /// plugin). Role gate / transition / breadcrumb read
    /// THIS field (not `workflow_def`) so a task created
    /// under the dev plugin keeps dev's state-machine
    /// invariants even after the session switches to review
    /// — and vice versa.
    pub task_workflow_def: WorkflowDef,

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

    /// 09-01-workflow-task-json-deadlock: task dirs whose
    /// `task.json` exists but failed to parse —
    /// `(slug, error)` pairs collected by the same scan
    /// that resolves [`current_task`]. The breadcrumb
    /// renders them as a `<workflow-task-warning>` section
    /// so the LLM sees WHY the session reports "no active
    /// task" while a task dir plainly exists on disk, and
    /// can repair the file instead of dead-locking between
    /// `request_task_state_transition` ("slug mismatch:
    /// None") and `create_task` ("already exists").
    /// Empty when every `task.json` parses (the common case).
    pub malformed_tasks: Vec<(String, String)>,
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
            let workflow_def = load_workflow(plugin_name, &project_path.to_string_lossy());
            return Ok(Some(WorkflowCtx {
                // No task → task_workflow_def falls back to session plugin.
                task_workflow_def: workflow_def.clone(),
                workflow_def,
                current_task: None,
                malformed_tasks: Vec::new(),
            }));
        }
    };

    let project_path = std::path::PathBuf::from(&project.path);
    let (current_task, malformed_tasks) = resolve_tasks_with_notes(&project_path).await;

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

    // C5 (2026-07-28): the task's owning plugin drives role gate /
    // transition / breadcrumb, NOT the session plugin. When a task
    // exists, load its owning plugin's state machine; otherwise fall
    // back to the session plugin (no task yet → session plugin is
    // the only signal). This keeps a dev-created task's invariants
    // stable when the session switches to review mid-task.
    let task_workflow_def = match &current_task {
        Some(task) => load_workflow(&task.workflow_plugin, &project.path),
        None => workflow_def.clone(),
    };

    Ok(Some(WorkflowCtx {
        workflow_def,
        task_workflow_def,
        current_task,
        malformed_tasks,
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
/// encountered file. Use [`resolve_tasks_with_notes`] when
/// the caller also needs the parse failures surfaced
/// (breadcrumb / diagnostics).
pub async fn resolve_current_task(project_path: &Path) -> Option<TaskJson> {
    resolve_tasks_with_notes(project_path).await.0
}

/// 09-01-workflow-task-json-deadlock: the scan behind
/// [`resolve_current_task`] that ALSO reports task dirs
/// whose `task.json` failed to parse, as `(slug, error)`
/// pairs (serde message included verbatim — the LLM can
/// act on "missing field `status`" directly).
///
/// The silent-swallow posture alone caused a real deadlock
/// (session 2e438939): the LLM hand-wrote a `task.json`
/// that failed to parse, every turn resolved
/// `current_task = None`, and the session dead-locked
/// between `request_task_state_transition` ("slug
/// mismatch: None") and `create_task` ("already exists")
/// until the LLM resorted to `rm -rf`. Surfacing the parse
/// error in the breadcrumb lets the LLM repair the file
/// instead.
pub async fn resolve_tasks_with_notes(
    project_path: &Path,
) -> (Option<TaskJson>, Vec<(String, String)>) {
    let tasks_root = project_path.join(".everlasting").join("tasks");
    if !tasks_root.exists() {
        return (None, Vec::new());
    }
    let mut entries = match std::fs::read_dir(&tasks_root) {
        Ok(it) => it,
        Err(e) => {
            tracing::warn!(
                path = %tasks_root.display(),
                error = %e,
                "resolve_current_task: failed to read tasks dir; treating as no current_task",
            );
            return (None, Vec::new());
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

    let mut malformed: Vec<(String, String)> = Vec::new();
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
                    return (Some(task), malformed);
                }
            }
            Err(e) => {
                // `TaskError::NotFound` = dir without a
                // task.json that `json_path.exists()` raced
                // past (deleted mid-scan) — not actionable,
                // keep it swallow-only. Everything else is a
                // real parse/IO failure the LLM can repair;
                // record it for the breadcrumb.
                if !matches!(e, crate::agent::workflow::TaskError::NotFound(_)) {
                    malformed.push((slug.clone(), e.to_string()));
                }
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
    (None, malformed)
}

// ---------------------------------------------------------------------------
// append_workflow_breadcrumb — per-turn injection
// ---------------------------------------------------------------------------

/// Append the workflow breadcrumb to the **last** message of the
/// per-turn request. Cache-correctness contract (D1,
/// 08-31-cache-head-volatility):
///
/// - Find the last message; if it's a user-role message, push a
///   new `ContentBlock::Text { text, cache_control: None }` onto
///   its block list (a plain `MessageContent::Text` payload is
///   widened to `Blocks([Text, breadcrumb])` in place — the wire
///   output is identical on both protocols).
/// - If the precondition fails (NOT user-role), log a `warn!` and
///   skip — never prepend a synthetic user message (S-B invariant;
///   prepending would bust the memory cache breakpoint at
///   `messages[0]`).
///
/// WHY the tail (not `messages[0]`): the OpenAI-compatible path
/// prefix-caches from byte 0 with no breakpoint markers, and the
/// breadcrumb mutates per-turn (status flips mid-loop), so a
/// head-positioned block forked the cached prefix on every state
/// transition. The tail block lives AFTER all cached-prefix
/// content on every request, so the prefix stays byte-stable.
///
/// Called from the per-turn loop in `run_chat_loop`,
/// *after* `inject_recall_into_turn` (recall stays at
/// `messages[0]`; the two injectors no longer share a target).
/// The injection mutates the per-turn REQUEST clone only — the
/// persisted `messages` Vec never carries a breadcrumb block, so
/// the block does not accumulate across turns.
///
/// Returns `true` when the breadcrumb was appended,
/// `false` when the S-B guard tripped (caller doesn't
/// branch on this — the log line is the signal).
pub fn append_workflow_breadcrumb(turn_messages: &mut [ChatMessage], ctx: &WorkflowCtx) -> bool {
    let block = build_breadcrumb_block(ctx);
    crate::agent::helpers::append_tail_text_block(
        turn_messages,
        "append_workflow_breadcrumb",
        block,
    )
}

// ---------------------------------------------------------------------------
// W1 (Workflow integration, Step 2.5 — 2026-07-08):
// delegation template — filled plugin-role system prompt
// for a worker turn. Lives in `inject.rs` next to
// `append_workflow_breadcrumb` because both target the
// per-turn tail of the request with the same S-B guard
// shape.
//
// **Step 2.5 contract**:
// - `compute_delegation_template` substitute `{title}`
//   / `{summary}` / `{state}` / `{relevant_specs}` from
//   the workflow_ctx + project_path. Returns `None`
//   when the plugin doesn't define a template for the
//   role (caller falls back to the sub-agent's own
//   system prompt).
// - `append_delegation_template` mutates the worker's
//   LAST message to push the filled template as a Text
//   block (D1: was `messages[0]`; the tail keeps the
//   worker's memory-blocks head byte-stable across
//   dispatches). The same S-B guard as the breadcrumb
//   helper: must land in a user-role message or the
//   append is skipped (with a `warn!`).
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
/// **`{relevant_specs}`** (Step 2.5; curated since
/// 09-02-wf-trellis-alignment R3): when the current task has a
/// `relevant-specs.jsonl` sidecar (`.everlasting/tasks/<slug>/`),
/// the placeholder resolves to that curated `file — reason` list;
/// otherwise it falls back to scanning
/// `<project>/.everlasting/spec/` for `.md` files. When the spec
/// dir is missing (Phase 3.2 not yet landed), or no files match,
/// the placeholder resolves to `(auto-detect via wf-before-dev)`
/// so the worker always gets an actionable hint.
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
    let task_slug = workflow_ctx.current_task.as_ref().map(|t| t.slug.as_str());
    let relevant_specs = resolve_relevant_specs(project_path, task_slug);
    Some(
        raw.replace("{title}", title)
            .replace("{summary}", summary)
            .replace("{state}", state)
            .replace("{relevant_specs}", &relevant_specs),
    )
}

/// Resolve the `{relevant_specs}` placeholder body.
///
/// **Curated path (09-02-wf-trellis-alignment R3)**: when
/// `task_slug` is `Some` and
/// `<project>/.everlasting/tasks/<slug>/relevant-specs.jsonl`
/// exists, it wins. Each line is `{"file": "...", "reason": "..."}`;
/// the output is one `file — reason` line per entry. Malformed
/// lines are SKIPPED (partial curation still beats no curation);
/// an empty file or a file where every line fails to parse falls
/// through to the tree-walk fallback below — the placeholder never
/// resolves to an empty string, the worker always gets an
/// actionable hint.
///
/// **Fallback**: the pre-R3 behavior, byte-identical — a recursive
/// `.md` listing of `<project>/.everlasting/spec/` (relative paths,
/// sorted, comma-joined), or `(auto-detect via wf-before-dev)` when
/// the dir is missing/empty.
///
/// **Known path fork (review P3)**: `project_path` here is the
/// DISPATCHER's path — `current_ctx.worktree_path` (see
/// `agent/subagent/dispatch/parse.rs`), i.e. the parent session's
/// worktree — while `task_slug` comes from `workflow_ctx
/// .current_task`, which `build_workflow_ctx` resolved against the
/// DB `project.path`. When the session itself runs inside a session
/// worktree, the two roots diverge and the curation lookup misses;
/// the fallback below covers that case with the full-tree listing
/// (accepted; not worth introducing a DB dependency into this
/// sync, IO-light helper).
pub(crate) fn resolve_relevant_specs(project_path: &str, task_slug: Option<&str>) -> String {
    if let Some(slug) = task_slug {
        if let Some(curated) = read_curated_specs(project_path, slug) {
            return curated;
        }
    }
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

/// Per-task spec curation sidecar (09-02-wf-trellis-alignment R3).
/// Reads `<project>/.everlasting/tasks/<slug>/relevant-specs.jsonl`
/// — one `{"file": "...", "reason": "..."}` object per line, written
/// by the main LLM during planning (wf-brainstorm) — and renders it
/// as `file — reason` lines for the `{relevant_specs}` placeholder.
///
/// Returns `None` (→ caller falls back to the full-tree listing)
/// when the file is missing, unreadable, empty, or contains NOT A
/// SINGLE valid entry. Blank lines and malformed lines are skipped
/// individually — a partially-corrupt file still yields its good
/// entries (partial curation beats no curation).
///
/// Deliberately NO per-file existence check on the `file` values:
/// this stays a pure text renderer (one small file read, no tree
/// IO, no permission boundary probes). A stale path in the
/// sidecar surfaces as a failed `read_file` on the worker side,
/// which the worker handles on its own.
fn read_curated_specs(project_path: &str, slug: &str) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct RelevantSpecEntry {
        file: String,
        reason: String,
    }

    let sidecar = std::path::Path::new(project_path)
        .join(".everlasting")
        .join("tasks")
        .join(slug)
        .join("relevant-specs.jsonl");
    let body = match std::fs::read_to_string(&sidecar) {
        Ok(b) => b,
        Err(_) => return None,
    };
    let mut lines: Vec<String> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry = match serde_json::from_str::<RelevantSpecEntry>(trimmed) {
            Ok(e) => e,
            Err(_) => continue, // malformed line — skip, keep the rest
        };
        if entry.file.trim().is_empty() {
            continue; // parseable but useless — treat as a bad line
        }
        lines.push(format!("{} — {}", entry.file, entry.reason));
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Push the filled delegation template (a Text block) onto the
/// worker's LAST message. Same S-B guard shape as
/// `append_workflow_breadcrumb` — skipped + `warn!` when the last
/// message isn't a user-role message.
///
/// `cache_control: None` — delegation templates are
/// per-dispatch (not per-turn stable), so they MUST NOT
/// become a cache breakpoint marker. D1 (08-31-cache-head-
/// volatility): the template used to ride `messages[0]` (the
/// worker's memory-blocks head); it now rides the tail so the
/// memory head — and hence the worker-side cached prefix — stays
/// byte-stable across dispatches of the same project.
///
/// Returns `true` when appended, `false` when the guard
/// tripped or the template is `None` (no plugin
/// template for this role → caller falls back to the
/// sub-agent's own system prompt).
pub fn append_delegation_template(turn_messages: &mut [ChatMessage], body: Option<String>) -> bool {
    let body = match body {
        Some(b) => b,
        None => return false,
    };
    let block = ContentBlock::Text {
        text: body,
        cache_control: None,
    };
    crate::agent::helpers::append_tail_text_block(
        turn_messages,
        "append_delegation_template",
        block,
    )
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
    ContentBlock::Text {
        text: breadcrumb_body(ctx),
        cache_control: None,
    }
}

/// Public accessor for the breadcrumb text body. Used by the E2 trace
/// pipeline (`agent::trace::record_breadcrumb`) to snapshot the
/// per-turn breadcrumb without re-implementing the body construction.
/// Returns the same string that `build_breadcrumb_block` pushes as a
/// `ContentBlock::Text`.
pub fn breadcrumb_body(ctx: &WorkflowCtx) -> String {
    // C4 (2026-07-27): the `None` branch fallback MUST come from the
    // active plugin's declared `initial` state, NOT a hard-coded
    // "planning". The previous `unwrap_or("planning")` was copied from
    // the dev plugin and silently broke every other plugin's bootstrap:
    // for review (states: intake/reviewing/revising/reported) the key
    // "planning" is absent, so `breadcrumb_for` returned "" and the LLM
    // got an empty state breadcrumb for the entire intake phase — the
    // root cause of session 99866757's "LLM lost in review plugin" E2E.
    // `initial` is validated non-empty at plugin-load time (see
    // `load_workflow`), so this is never an empty key.
    let state_str = ctx
        .current_task
        .as_ref()
        .map(|t| t.status.as_str())
        .unwrap_or_else(|| ctx.task_workflow_def.initial.as_str());
    // C5: breadcrumb resolves against the TASK's owning plugin so a
    // dev task shows dev's state guidance even in a review session.
    let breadcrumb = breadcrumb_for(&ctx.task_workflow_def, state_str);

    match &ctx.current_task {
        Some(task) => {
            // task-fragment branch: 3-line metadata +
            // breadcrumb text + pointer.
            let body = format!(
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
            );
            append_malformed_warning(body, ctx)
        }
        None => {
            // Bootstrap branch (C4 2026-07-27): expose plugin +
            // initial state + the workflow-only tools the LLM
            // has, so it can orient itself BEFORE creating a
            // task. Previously this block only said "call
            // create_task" with no plugin/state context — the
            // LLM treated a review-plugin session as a plain
            // chat and never used its workflow tools.
            //
            // The tool list mirrors the workflow-only whitelist
            // in `filter_tools_for_workflow` (`tools/mod.rs`):
            // `create_task` + `request_task_state_transition`,
            // plus `dispatch_subagent` (per-turn append, gated
            // on non-worker) and `update_checklist` (always in
            // builtin_tools). Hard-coded here with a pointer —
            // `None`-branch means current_task is absent but the
            // plugin IS known, so the set is statically
            // derivable; keeping it literal avoids pulling the
            // whole tool registry into this hot path.
            //
            // 09-01-workflow-task-json-deadlock: the old text
            // claimed "read_task 会 lenient 兜底" for hand-written
            // task.json — false (a missing required field failed
            // the whole parse and the swallow dropped it), and it
            // steered session 2e438939 into hand-writing a
            // schema-violating file. The schema note now states
            // the actual contract.
            let body = format!(
                "<workflow-task-meta>\n\
                 plugin: {plugin}\n\
                 state: {state} (initial — no active task yet)\n\
                 workflow-only tools you have: create_task, dispatch_subagent, request_task_state_transition, update_checklist\n\
                 no active task — call the create_task tool to start one (字段全、自带 prd skeleton,会自动被会话识别)。\n\
                 (若坚持 write_file 手写 .everlasting/tasks/<slug>/task.json:top-level 必填 id/title/slug/status;items[] 每项必填 id,status 缺省 planning;文件解析失败会以 <workflow-task-warning> 形式出现在本 breadcrumb。)\n\
                 </workflow-task-meta>\n\n\
                 {breadcrumb}\n",
                plugin = ctx.workflow_def.name,
                state = ctx.workflow_def.initial,
            );
            append_malformed_warning(body, ctx)
        }
    }
}

/// 09-01-workflow-task-json-deadlock: render the
/// [`WorkflowCtx::malformed_tasks`] notes as a trailing
/// `<workflow-task-warning>` section. Tells the LLM the
/// exact slug + parse error and the recovery contract:
/// repair the file (the scan re-reads it every turn) —
/// NOT `rm -rf`, NOT re-running `create_task` (it will
/// be rejected with "already exists"). Appended in BOTH
/// breadcrumb branches: a malformed task dir is invisible
/// to `current_task` resolution regardless of whether
/// another valid task is active.
fn append_malformed_warning(body: String, ctx: &WorkflowCtx) -> String {
    if ctx.malformed_tasks.is_empty() {
        return body;
    }
    let notes = ctx
        .malformed_tasks
        .iter()
        .map(|(slug, error)| format!("  - {slug}: {error}", slug = slug, error = error))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{body}\n<workflow-task-warning>\n\
         task dir(s) exist but their task.json FAILED to parse (this is why no active task resolves):\n\
         {notes}\n\
         Fix: edit the listed task.json to satisfy the schema — the workflow scan re-reads it every turn and picks it up automatically. Do NOT rm -rf the dir and do NOT retry create_task (rejected: already exists).\n\
         </workflow-task-warning>\n",
        body = body,
        notes = notes,
    )
}
