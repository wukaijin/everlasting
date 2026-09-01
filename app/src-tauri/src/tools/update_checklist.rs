//! B12 `update_checklist` virtual tool — agent self-tracking progress list.
//!
//! The model calls `update_checklist(items)` to atomically replace the
//! per-request checklist Vec stored in the agent loop's run scope. The
//! agent loop re-injects the current checklist into each turn's request
//! as an ephemeral synthetic user block (no `cache_control`, no
//! persisted messages write — see `agent::chat_loop`).
//!
//! # Semantics (per B12 PRD §"Decisions" + Acceptance Criteria)
//!
//! - **Full replace** — the input `items` array replaces the loop's Vec
//!   in full (NOT append). Replay correctness requires "last
//!   `update_checklist` tool_result == current state".
//! - **At-most-one `in_progress` coerce** — if the model passes
//!   multiple `in_progress` items, we keep the LAST one in array
//!   order and demote any others to `pending`. We do NOT error and
//!   do NOT abort the agent loop.
//! - **Return value** — the tool_result carries the full resulting
//!   list (post-coerce), `is_error: false`. The frontend renders
//!   the checklist from this tool_result stream.
//!
//! # Lifetime
//!
//! Per-request: a fresh `Vec<ChecklistItem>` lives in each
//! `run_chat_loop` invocation. New user message → new run → new
//! empty checklist. C3 compaction is in-memory only (DB history
//! is never dropped), so a reload reconstructs the checklist from
//! the last `update_checklist` tool_result in the message history.
//!
//! # Plan mode
//!
//! `update_checklist` is auto-allowed in Plan mode because
//! `agent::permissions::filter_tools_for_mode` only drops
//! `write_file` / `edit_file` / `shell`. Checklist mutation has
//! no side-effect on the user's filesystem.

use std::path::Path;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::llm::types::ToolDef;

/// Per-request checklist handle held by [`crate::tools::ToolContext`]
/// and mutated atomically by `update_checklist::execute`.
///
/// The agent loop constructs one `Arc<Mutex<Vec<ChecklistItem>>>`
/// per `run_chat_loop` call, stores it inside the `ToolContext`, and
/// reads it every turn (after C3 compaction, before `provider.send`)
/// to build the ephemeral checklist injection block.
pub type ChecklistHandle = Arc<Mutex<Vec<ChecklistItem>>>;

/// Construct a fresh, empty checklist handle. Called once per
/// `run_chat_loop` invocation (production + tests).
pub fn new_handle() -> ChecklistHandle {
    Arc::new(Mutex::new(Vec::new()))
}

/// Status of a checklist item. Serialized lowercase to match the
/// LLM-facing JSON schema string values (`"pending"` / `"in_progress"`
/// / `"done"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde :: Serialize, serde :: Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistStatus {
    Pending,
    InProgress,
    Done,
}

/// One checklist item.
///
/// Phase 2 Step 2.6 (2026-07-08): added `id` and
/// `tdd: Option<bool>` to match `TaskItem` (the
/// on-disk `task.json.items[i]` schema). The `id`
/// is the cross-session persistence key — a workflow
/// session's checklist writes through to `task.json`
/// keyed by `id`, so an item's progress survives
/// across chat-loop invocations (and across worker
/// dispatches). `tdd` is optional; the plugin author
/// or implementer uses it to flag items that should
/// be done test-first.
///
/// **Backward compat**: `id` is `#[serde(default)]`
/// (empty string when missing) so existing LLM-emitted
/// items without an `id` still parse. The downstream
/// `task.json.items` writer (`task.rs::write_task`
/// with the updated items Vec) tolerates empty
/// `id`s by deriving one from the content hash.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChecklistItem {
    /// Stable id (Phase 2 Step 2.6). When the
    /// checklist is written through to `task.json.items`,
    /// this is the lookup key. Empty string for legacy
    /// items.
    #[serde(default)]
    pub id: String,
    pub content: String,
    pub status: ChecklistStatus,
    /// `Some(true)` / `Some(false)` = TDD flag set by
    /// plugin author / implementer; `None` = not
    /// declared. Phase 2 Step 2.6 surfaces this on
    /// `task.json.items[].tdd` so the workflow's
    /// implementer can flag test-first items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tdd: Option<bool>,
}

/// The `update_checklist` tool definition registered in
/// `builtin_tools()`.
pub fn definition() -> ToolDef {
    ToolDef {
        name: "update_checklist".to_string(),
        description: Some(
            "Update your running progress checklist for this task. Pass the FULL list of \
             items every call — the new list replaces the old one atomically (not append). \
             Each item has `id` (stable identifier for cross-session persistence — see \
             checklist guidance), `content` (short description), `status` \
             (`pending` / `in_progress` / `done`), and optional `tdd` (set true \
             for items that must be done test-first). At most one item should be \
             `in_progress` at a time; if you pass multiple, only the last is kept as \
             `in_progress` and the rest are demoted to `pending`. Call this whenever \
             your plan changes — the current list is re-injected into your context \
             every turn so you don't lose progress. Use it for any task with 3+ steps."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "description": "The full checklist (replaces any previous version).",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Stable identifier for this item (kebab-case, e.g. 'backend-impl'). Used for cross-session persistence when the session is a workflow session."
                            },
                            "content": {
                                "type": "string",
                                "description": "Short description of the step."
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "done"],
                                "description": "Current state of the item."
                            },
                            "tdd": {
                                "type": "boolean",
                                "description": "Optional. Set true for items that must be done test-first (Phase 2 Step 2.6)."
                            }
                        },
                        "required": ["content", "status"]
                    }
                }
            },
            "required": ["items"]
        }),
    }
}

/// Coerce at-most-one `in_progress`: keep the LAST `in_progress`
/// item (by array order) and demote any earlier ones to `pending`.
/// Pure function — does NOT mutate the input. Used by both
/// `execute` (production path) and the unit tests.
pub fn coerce_at_most_one_in_progress(items: &[ChecklistItem]) -> Vec<ChecklistItem> {
    // Find the index of the last `in_progress` (if any).
    let last_in_progress = items
        .iter()
        .rposition(|i| i.status == ChecklistStatus::InProgress);
    let mut out = Vec::with_capacity(items.len());
    for (idx, item) in items.iter().enumerate() {
        let mut cloned = item.clone();
        if cloned.status == ChecklistStatus::InProgress {
            // Demote every `in_progress` except the last one.
            if Some(idx) != last_in_progress {
                cloned.status = ChecklistStatus::Pending;
            }
        }
        out.push(cloned);
    }
    out
}

/// Parse + coerce the input JSON into a `Vec<ChecklistItem>`.
///
/// - Missing `items` array → empty Vec (the model is allowed to
///   clear the list by passing `{"items": []}`; an entirely missing
///   `items` key is treated the same way — atomically replace with
///   empty).
/// - An item missing `content` → skipped (don't error on a single
///   malformed entry; let the rest through).
/// - An item with an unrecognized `status` string → coerced to
///   `pending` (don't error; the model can self-correct on the
///   next call).
/// - Then the at-most-one-`in_progress` coercion runs.
fn parse_and_coerce(input: &serde_json::Value) -> Vec<ChecklistItem> {
    let Some(arr) = input.get("items").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut parsed: Vec<ChecklistItem> = Vec::with_capacity(arr.len());
    for entry in arr {
        let Some(content) = entry.get("content").and_then(|v| v.as_str()) else {
            continue;
        };
        let status = match entry.get("status").and_then(|v| v.as_str()) {
            Some("in_progress") => ChecklistStatus::InProgress,
            Some("done") => ChecklistStatus::Done,
            // Unknown / missing / "pending" / anything else → pending.
            _ => ChecklistStatus::Pending,
        };
        // Phase 2 Step 2.6: pull the optional `id` +
        // `tdd` from the entry. `id` defaults to "" so
        // legacy items still parse (the on-disk writer
        // derives one from the content hash).
        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tdd = entry.get("tdd").and_then(|v| v.as_bool());
        parsed.push(ChecklistItem {
            id,
            content: content.to_string(),
            status,
            tdd,
        });
    }
    coerce_at_most_one_in_progress(&parsed)
}

/// Format the full checklist as a single string for the tool_result
/// and for the ephemeral injection block. Pure function.
pub fn render_checklist(items: &[ChecklistItem]) -> String {
    if items.is_empty() {
        return "(empty checklist)".to_string();
    }
    let mut lines = Vec::with_capacity(items.len());
    for (idx, item) in items.iter().enumerate() {
        let marker = match item.status {
            ChecklistStatus::Pending => "[ ]",
            ChecklistStatus::InProgress => "[~]",
            ChecklistStatus::Done => "[x]",
        };
        let focus = if item.status == ChecklistStatus::InProgress {
            " <- in progress"
        } else {
            ""
        };
        lines.push(format!("{}. {} {}{}", idx + 1, marker, item.content, focus));
    }
    lines.join("\n")
}

/// Execute `update_checklist`: parse + coerce + atomically replace
/// the loop's Vec via the handle; return the full resulting list as
/// the tool_result (`is_error: false`).
///
/// **Phase 2 Step 2.6 (`07-08-workflow-integration`)**: when
/// `ctx.workflow_name` is `Some`, the items are persisted to
/// `task.json.items` (the on-disk workflow task file) in
/// addition to the in-memory Vec. Non-workflow callers
/// keep the legacy loop-local Vec behavior — the on-disk
/// write is skipped.
///
/// **Why both?**: the in-memory Vec drives the per-turn
/// ephemeral injection (B12), which is what the LLM sees
/// this turn. The on-disk write is what a *future* session
/// (or a worker dispatch) reads when resuming the task.
/// Splitting the two keeps the per-turn contract
/// byte-identical with the pre-Step-2.6 behavior while
/// adding the cross-session persistence layer.
pub async fn execute(
    input: &serde_json::Value,
    handle: &ChecklistHandle,
    ctx: &crate::tools::ToolContext,
) -> (String, bool) {
    let new_items = parse_and_coerce(input);
    // Atomic full-replace. The lock is held only for the swap; no
    // I/O inside the critical section.
    {
        let mut guard = handle.lock().await;
        guard.clear();
        guard.extend(new_items.iter().cloned());
    }

    // Phase 2 Step 2.6: workflow sessions persist items to
    // `task.json.items`. The mapping from ChecklistStatus
    // → TaskStatus (Pending→Planning, InProgress→InProgress,
    // Done→Done) mirrors B12's checklist status onto the
    // workflow state machine's status set. After the
    // 2026-07-10 merge (Implement+Check collapsed into
    // InProgress), the mapping is 1:1 — no coarsening.
    //
    // On any failure (no active task / read error / write
    // error), we log a `warn!` and return a tool_result
    // that surfaces the failure reason. The in-memory
    // handle is already updated above — that path is
    // unchanged from pre-Step-2.6.
    let persist_msg = maybe_persist_to_task_json(&new_items, ctx).await;

    let body = render_checklist(&new_items);
    let done_count = new_items
        .iter()
        .filter(|i| i.status == ChecklistStatus::Done)
        .count();
    let in_progress_count = new_items
        .iter()
        .filter(|i| i.status == ChecklistStatus::InProgress)
        .count();
    let summary = format!(
        "Checklist updated ({} items, {} done, {} in_progress).\n\n{}{}",
        new_items.len(),
        done_count,
        in_progress_count,
        body,
        // Step 2.6: append a `[persist]` line for workflow
        // sessions so the LLM sees the on-disk write
        // outcome in the same tool_result. For
        // non-workflow callers this is empty (legacy body
        // shape preserved).
        persist_msg,
    );
    (summary, false)
}

/// Phase 2 Step 2.6: persist `items` to the workflow's
/// active task.json when the caller is a workflow session
/// (`ctx.workflow_name.is_some()`). Returns a human-
/// readable suffix for the tool_result body — empty
/// string when no persistence was attempted (non-workflow
/// caller), a one-liner on success, a warning line on
/// failure.
///
/// **No-op cases** (returns `""`):
/// - `ctx.workflow_name` is `None`
/// - no project bound to the session
///
/// **Failure cases** (returns `"[persist] ⚠️ {reason}\n"`):
/// - cannot resolve the active task
/// - cannot read `task.json`
/// - cannot write `task.json` (atomic rename fails)
///
/// The function deliberately does NOT mutate the
/// in-memory `ChecklistHandle` (that's the caller's job,
/// already done). The on-disk write is a "best effort"
/// persistence layer; the loop-local Vec remains the
/// authoritative per-turn state. A failed disk write
/// degrades to "in-memory only this session" rather than
/// failing the whole tool call.
async fn maybe_persist_to_task_json(
    items: &[ChecklistItem],
    ctx: &crate::tools::ToolContext,
) -> String {
    if ctx.workflow_name.is_none() {
        return String::new();
    }
    // Locate the active task under the project root.
    // `ctx.worktree_path` is the session's worktree root
    // (or project root for non-worktree sessions); the
    // workflow task dir lives at `<root>/.everlasting/tasks/`.
    let project_path = ctx.worktree_path.clone();
    let tasks_root = project_path.join(".everlasting").join("tasks");
    if !tasks_root.exists() {
        return "[persist] ⚠️ no tasks dir (task bootstrap pending)\n".to_string();
    }
    // Find the first unfinished task (matches
    // `agent/workflow/inject.rs::resolve_current_task`'s
    // ordering: lexicographic by slug). We re-implement
    // the lookup here to avoid pulling the inject module
    // into a leaf helper (and to keep the per-task lookup
    // synchronous-and-fast for the tool's hot path).
    let current_task = match pick_first_unfinished_task(&tasks_root) {
        Ok(Some(t)) => t,
        Ok(None) => return "[persist] ⚠️ no active task\n".to_string(),
        Err(e) => return format!("[persist] ⚠️ task lookup failed: {}\n", e),
    };

    // Map ChecklistStatus → TaskStatus (Phase 2 simplification).
    let mapped: Vec<crate::agent::workflow::TaskItem> = items
        .iter()
        .map(|i| crate::agent::workflow::TaskItem {
            id: derive_item_id(i),
            content: i.content.clone(),
            status: map_status(i.status),
            tdd: i.tdd,
        })
        .collect();

    // Write the updated task.json. We rebuild the whole
    // struct to keep the persistence layer simple —
    // partial updates would need a writer API for the
    // items slice (Phase 3 candidate).
    let updated = crate::agent::workflow::TaskJson {
        items: mapped,
        ..current_task
    };
    // `write_task` expects the PROJECT ROOT (it appends
    // `.everlasting/tasks/<slug>` internally). Passing the
    // task dir directly would double-nest the path.
    if let Err(e) = crate::agent::workflow::write_task(&project_path, &updated) {
        return format!("[persist] ⚠️ task.json write failed: {}\n", e);
    }
    format!(
        "[persist] → task.json updated ({} items)\n",
        updated.items.len()
    )
}

/// Pick the first unfinished task from `<root>/.everlasting/tasks/`
/// by lexicographic slug order. Mirrors
/// `agent/workflow/inject.rs::resolve_current_task` (which is
/// not pub-exposable without leaking more surface area
/// than Step 2.6 needs).
fn pick_first_unfinished_task(
    tasks_root: &Path,
) -> std::io::Result<Option<crate::agent::workflow::TaskJson>> {
    let entries = std::fs::read_dir(tasks_root)?;
    let mut slugs: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let p = entry.path();
            if !p.is_dir() {
                return None;
            }
            entry.file_name().into_string().ok()
        })
        .collect();
    slugs.sort();
    for slug in slugs {
        let task_dir = tasks_root.join(&slug);
        let json_path = task_dir.join("task.json");
        if !json_path.is_file() {
            continue;
        }
        let raw = match std::fs::read_to_string(&json_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let task: crate::agent::workflow::TaskJson = match serde_json::from_str(&raw) {
            Ok(t) => t,
            Err(_) => continue,
        };
        // "Done" maps to TaskStatus::Done — skip.
        if matches!(task.status, crate::agent::workflow::TaskStatus::Done) {
            continue;
        }
        return Ok(Some(task));
    }
    Ok(None)
}

/// Phase 2 Step 2.6 ChecklistStatus → TaskStatus mapping.
/// After the 2026-07-10 merge the mapping is 1:1
/// (Pending→Planning, InProgress→InProgress, Done→Done).
fn map_status(s: ChecklistStatus) -> crate::agent::workflow::TaskStatus {
    use crate::agent::workflow::TaskStatus;
    match s {
        ChecklistStatus::Pending => TaskStatus::Planning,
        ChecklistStatus::InProgress => TaskStatus::InProgress,
        ChecklistStatus::Done => TaskStatus::Done,
    }
}

/// Derive a stable `id` for the persisted `TaskItem`:
/// prefer the LLM-supplied `id`; fall back to a hash of
/// `content` (so legacy items with empty `id` still get
/// a stable cross-session key).
fn derive_item_id(item: &ChecklistItem) -> String {
    if !item.id.is_empty() {
        return item.id.clone();
    }
    // Tiny FNV-1a hash — the content is short, no need for
    // a full cryptographic hash. The id is only used as a
    // cross-session persistence key; uniqueness within
    // the checklist is "good enough" not "guaranteed".
    let mut h: u64 = 0xcbf29ce484222325;
    for b in item.content.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("auto-{:016x}", h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolContext;
    use std::path::PathBuf;

    fn item(content: &str, status: ChecklistStatus) -> ChecklistItem {
        ChecklistItem {
            id: String::new(),
            content: content.to_string(),
            status,
            tdd: None,
        }
    }

    /// Non-workflow `ToolContext` for the legacy loop-local
    /// tests (the B12 path that doesn't touch `task.json`).
    /// Mirrors the `test_ctx` shape used by other tool
    /// tests; the exact path values don't matter for
    /// `update_checklist` because `maybe_persist_to_task_json`
    /// is gated on `workflow_name.is_some()`.
    fn legacy_ctx() -> ToolContext {
        ToolContext {
            tool_use_id: None,
            escalation: Default::default(),
            worktree_path: PathBuf::from("/tmp/test-proj"),
            cwd: PathBuf::from("/tmp/test-proj"),
            checklist: new_handle(),
            background_shells: crate::background_shell::default_registry(),
            db: crate::tools::test_default_pool(),
            project_id: "test-proj".to_string(),
            data_dir: PathBuf::from("/tmp"),
            workflow_name: None,
            mode: crate::db::Mode::Edit,
        }
    }

    // ---- definition ----

    #[test]
    fn definition_has_correct_name() {
        assert_eq!(definition().name, "update_checklist");
    }

    #[test]
    fn definition_schema_requires_items() {
        let schema = definition().input_schema;
        let required = schema
            .get("required")
            .and_then(|v| v.as_array())
            .expect("required array present");
        let has_items = required.iter().any(|v| v.as_str() == Some("items"));
        assert!(has_items, "items must be required");
    }

    #[test]
    fn definition_schema_status_enum_covers_three_states() {
        let schema = definition().input_schema;
        let status_enum = schema
            .pointer("/properties/items/items/properties/status/enum")
            .and_then(|v| v.as_array())
            .expect("status enum present");
        let strs: Vec<&str> = status_enum.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(strs, vec!["pending", "in_progress", "done"]);
    }

    // ---- coerce_at_most_one_in_progress ----

    #[test]
    fn coerce_keeps_single_in_progress() {
        let input = vec![
            item("a", ChecklistStatus::Done),
            item("b", ChecklistStatus::InProgress),
            item("c", ChecklistStatus::Pending),
        ];
        let out = coerce_at_most_one_in_progress(&input);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].status, ChecklistStatus::Done);
        assert_eq!(out[1].status, ChecklistStatus::InProgress);
        assert_eq!(out[2].status, ChecklistStatus::Pending);
    }

    #[test]
    fn coerce_keeps_last_in_progress_demotes_earlier() {
        let input = vec![
            item("first", ChecklistStatus::InProgress),
            item("middle", ChecklistStatus::InProgress),
            item("last", ChecklistStatus::InProgress),
        ];
        let out = coerce_at_most_one_in_progress(&input);
        // Only the LAST in_progress is kept; the others drop to pending.
        assert_eq!(out[0].status, ChecklistStatus::Pending);
        assert_eq!(out[1].status, ChecklistStatus::Pending);
        assert_eq!(out[2].status, ChecklistStatus::InProgress);
        // Contents preserved.
        assert_eq!(out[0].content, "first");
        assert_eq!(out[2].content, "last");
    }

    #[test]
    fn coerce_no_in_progress_leaves_all_untouched() {
        let input = vec![
            item("a", ChecklistStatus::Done),
            item("b", ChecklistStatus::Pending),
        ];
        let out = coerce_at_most_one_in_progress(&input);
        assert_eq!(out[0].status, ChecklistStatus::Done);
        assert_eq!(out[1].status, ChecklistStatus::Pending);
    }

    #[test]
    fn coerce_empty_input_returns_empty() {
        let out = coerce_at_most_one_in_progress(&[]);
        assert!(out.is_empty());
    }

    // ---- parse_and_coerce (via execute) ----

    #[tokio::test]
    async fn execute_full_replace_not_append() {
        let handle = new_handle();
        // First call: 3 items.
        let input1 = serde_json::json!({
            "items": [
                {"content": "a", "status": "done"},
                {"content": "b", "status": "in_progress"},
                {"content": "c", "status": "pending"}
            ]
        });
        let (out1, is_err1) = execute(&input1, &handle, &legacy_ctx()).await;
        assert!(!is_err1, "{}", out1);
        assert_eq!(handle.lock().await.len(), 3);

        // Second call: 2 completely different items.
        let input2 = serde_json::json!({
            "items": [
                {"content": "x", "status": "pending"},
                {"content": "y", "status": "pending"}
            ]
        });
        let (out2, is_err2) = execute(&input2, &handle, &legacy_ctx()).await;
        assert!(!is_err2, "{}", out2);
        // Vec must be the SECOND call's list (2 items), not 5.
        let after = handle.lock().await.clone();
        assert_eq!(after.len(), 2, "second call must full-replace, not append");
        assert_eq!(after[0].content, "x");
        assert_eq!(after[1].content, "y");
        // The result string reflects the new state. Use the marker
        // form (e.g. `[ ] a`) so the substring check doesn't match
        // a single-char false-positive inside another token.
        assert!(out2.contains("[ ] x"), "result: {}", out2);
        assert!(out2.contains("[ ] y"), "result: {}", out2);
        // The stale first-call items are gone — check the exact
        // rendered marker line shape so a stale "a" doesn't sneak
        // in via substring match inside another token.
        assert!(
            !out2.contains("[x] a"),
            "result must not include stale 'a' item, got: {}",
            out2
        );
        assert!(
            !out2.contains("[~] b"),
            "result must not include stale 'b' item, got: {}",
            out2
        );
    }

    #[tokio::test]
    async fn execute_two_in_progress_coerces_to_one() {
        let handle = new_handle();
        let input = serde_json::json!({
            "items": [
                {"content": "first", "status": "in_progress"},
                {"content": "last", "status": "in_progress"}
            ]
        });
        let (out, is_err) = execute(&input, &handle, &legacy_ctx()).await;
        assert!(!is_err, "coerce must NOT error");
        let stored = handle.lock().await.clone();
        assert_eq!(stored.len(), 2);
        let in_progress: Vec<_> = stored
            .iter()
            .filter(|i| i.status == ChecklistStatus::InProgress)
            .collect();
        assert_eq!(in_progress.len(), 1, "exactly one in_progress after coerce");
        // Last in array order wins.
        assert_eq!(in_progress[0].content, "last");
        // The first one demoted to pending.
        assert_eq!(stored[0].status, ChecklistStatus::Pending);
        // Summary string also reflects post-coerce counts.
        assert!(out.contains("1 in_progress"), "summary: {}", out);
    }

    #[tokio::test]
    async fn execute_empty_items_clears_list() {
        let handle = new_handle();
        // Seed the list first.
        let seed = serde_json::json!({
            "items": [{"content": "a", "status": "pending"}]
        });
        execute(&seed, &handle, &legacy_ctx()).await;
        assert_eq!(handle.lock().await.len(), 1);

        // Now empty.
        let input = serde_json::json!({"items": []});
        let (out, is_err) = execute(&input, &handle, &legacy_ctx()).await;
        assert!(!is_err);
        assert!(
            handle.lock().await.is_empty(),
            "empty items must clear the list"
        );
        assert!(out.contains("0 items") || out.contains("empty checklist"));
    }

    #[tokio::test]
    async fn execute_missing_items_key_treated_as_empty() {
        let handle = new_handle();
        // Completely missing `items` — defensive parse returns empty.
        let input = serde_json::json!({});
        let (_out, is_err) = execute(&input, &handle, &legacy_ctx()).await;
        assert!(!is_err, "missing items key is not an error");
        assert!(handle.lock().await.is_empty());
    }

    #[tokio::test]
    async fn execute_unknown_status_coerced_to_pending() {
        let handle = new_handle();
        let input = serde_json::json!({
            "items": [
                {"content": "weird", "status": "blocked"}
            ]
        });
        let (_out, is_err) = execute(&input, &handle, &legacy_ctx()).await;
        assert!(!is_err);
        let stored = handle.lock().await.clone();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].status, ChecklistStatus::Pending);
    }

    #[tokio::test]
    async fn execute_summary_counts_accurate() {
        let handle = new_handle();
        let input = serde_json::json!({
            "items": [
                {"content": "done1", "status": "done"},
                {"content": "done2", "status": "done"},
                {"content": "wip", "status": "in_progress"},
                {"content": "todo", "status": "pending"}
            ]
        });
        let (out, _) = execute(&input, &handle, &legacy_ctx()).await;
        assert!(out.contains("4 items"), "summary: {}", out);
        assert!(out.contains("2 done"), "summary: {}", out);
        assert!(out.contains("1 in_progress"), "summary: {}", out);
    }

    // ---- render_checklist ----

    #[test]
    fn render_marks_each_status_correctly() {
        let items = vec![
            item("todo", ChecklistStatus::Pending),
            item("wip", ChecklistStatus::InProgress),
            item("finished", ChecklistStatus::Done),
        ];
        let rendered = render_checklist(&items);
        assert!(rendered.contains("[ ] todo"));
        assert!(rendered.contains("[~] wip <- in progress"));
        assert!(rendered.contains("[x] finished"));
        // 1-indexed numbering.
        assert!(rendered.contains("1. "));
        assert!(rendered.contains("2. "));
        assert!(rendered.contains("3. "));
    }

    #[test]
    fn render_empty_list() {
        let rendered = render_checklist(&[]);
        assert_eq!(rendered, "(empty checklist)");
    }

    // ---- Phase 2 Step 2.6: workflow persistence ----------------------

    use crate::agent::workflow::{TaskItem, TaskJson, TaskStatus};

    /// Workflow-session `ToolContext` factory. Builds a
    /// task.json under a temp project root + returns a
    /// ctx pointing at that root with `workflow_name =
    /// Some("dev")`.
    async fn workflow_ctx_with_task(
        initial_items: Vec<TaskItem>,
        status: TaskStatus,
    ) -> (ToolContext, tempfile::TempDir) {
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let task_dir = proj_tmp
            .path()
            .join(".everlasting")
            .join("tasks")
            .join("step26-fixture");
        std::fs::create_dir_all(&task_dir).unwrap();
        let task = TaskJson {
            id: "t-step26".into(),
            title: "Step 2.6 fixture".into(),
            slug: "step26-fixture".into(),
            status,
            created_at: "2026-07-09T00:00:00Z".into(),
            updated_at: "2026-07-09T00:00:00Z".into(),
            parent: None,
            summary: "test".into(),
            items: initial_items,
            // Step 3.3: pre-archive fixture.
            completed_at: None,
            workflow_plugin: "dev".into(),
        };
        crate::agent::workflow::write_task(proj_tmp.path(), &task).unwrap();

        let ctx = ToolContext {
            tool_use_id: None,
            escalation: Default::default(),
            worktree_path: proj_tmp.path().to_path_buf(),
            cwd: proj_tmp.path().to_path_buf(),
            checklist: new_handle(),
            background_shells: crate::background_shell::default_registry(),
            db: crate::tools::test_default_pool(),
            project_id: "step26".into(),
            data_dir: proj_tmp.path().to_path_buf(),
            workflow_name: Some("dev".to_string()),
            mode: crate::db::Mode::Edit,
        };
        (ctx, proj_tmp)
    }

    fn read_persisted_items(task_dir: &Path) -> Vec<TaskItem> {
        let raw = std::fs::read_to_string(task_dir.join("task.json")).unwrap();
        let task: TaskJson = serde_json::from_str(&raw).unwrap();
        task.items
    }

    #[tokio::test]
    async fn execute_workflow_persists_items_to_task_json() {
        // Workflow session → update_checklist writes through
        // to task.json.items. The in-memory handle is also
        // updated (B12 contract preserved).
        let (ctx, proj_tmp) = workflow_ctx_with_task(vec![], TaskStatus::Planning).await;
        let task_dir = proj_tmp.path().join(".everlasting/tasks/step26-fixture");

        let handle = new_handle();
        let input = serde_json::json!({
            "items": [
                {"id": "research", "content": "调研", "status": "done"},
                {"id": "implement", "content": "实现", "status": "in_progress"},
                {"id": "test", "content": "测试", "status": "pending", "tdd": true},
            ]
        });
        let (_out, is_err) = execute(&input, &handle, &ctx).await;
        assert!(!is_err, "execute must succeed for workflow session");

        // In-memory handle updated.
        let in_mem = handle.lock().await.clone();
        assert_eq!(in_mem.len(), 3);

        // On-disk task.json updated.
        let persisted = read_persisted_items(&task_dir);
        assert_eq!(persisted.len(), 3, "all 3 items must persist");
        assert_eq!(persisted[0].id, "research");
        assert_eq!(persisted[0].status, TaskStatus::Done);
        assert_eq!(persisted[1].id, "implement");
        assert_eq!(persisted[1].status, TaskStatus::InProgress);
        assert_eq!(persisted[2].id, "test");
        assert_eq!(
            persisted[2].tdd,
            Some(true),
            "tdd flag must round-trip to task.json",
        );
    }

    #[tokio::test]
    async fn execute_non_workflow_does_not_touch_task_json() {
        // Legacy B12 contract: non-workflow caller
        // (`workflow_name = None`) mutates the in-memory
        // handle but does NOT touch task.json.
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let task_dir = proj_tmp.path().join(".everlasting/tasks/legacy-fixture");
        std::fs::create_dir_all(&task_dir).unwrap();
        let task = TaskJson {
            id: "t-legacy".into(),
            title: "Legacy".into(),
            slug: "legacy-fixture".into(),
            status: TaskStatus::InProgress,
            created_at: "2026-07-09T00:00:00Z".into(),
            updated_at: "2026-07-09T00:00:00Z".into(),
            parent: None,
            summary: "legacy".into(),
            items: vec![TaskItem {
                id: "preexisting".into(),
                content: "pre-existing item".into(),
                status: TaskStatus::Planning,
                tdd: None,
            }],
            // Step 3.3: `completed_at` is set by
            // archive_task_init; always None on the
            // legacy-fixture seed.
            completed_at: None,
            workflow_plugin: "dev".into(),
        };
        crate::agent::workflow::write_task(proj_tmp.path(), &task).unwrap();

        let ctx = ToolContext {
            tool_use_id: None,
            escalation: Default::default(),
            worktree_path: proj_tmp.path().to_path_buf(),
            cwd: proj_tmp.path().to_path_buf(),
            checklist: new_handle(),
            background_shells: crate::background_shell::default_registry(),
            db: crate::tools::test_default_pool(),
            project_id: "legacy".into(),
            data_dir: proj_tmp.path().to_path_buf(),
            workflow_name: None,
            mode: crate::db::Mode::Edit, // <- non-workflow
        };
        let handle = new_handle();
        let input = serde_json::json!({
            "items": [{"content": "x", "status": "in_progress"}]
        });
        let (_out, is_err) = execute(&input, &handle, &ctx).await;
        assert!(!is_err);

        // In-memory handle updated (B12 contract).
        let in_mem = handle.lock().await.clone();
        assert_eq!(in_mem.len(), 1);

        // On-disk task.json UNTOUCHED — still has the
        // pre-existing item from before execute().
        let persisted = read_persisted_items(&task_dir);
        assert_eq!(
            persisted.len(),
            1,
            "non-workflow must not modify task.json.items",
        );
        assert_eq!(persisted[0].id, "preexisting");
    }

    #[tokio::test]
    async fn execute_workflow_derives_id_from_content_when_missing() {
        // LLM omits `id` → writer derives one from the
        // content hash so the on-disk item still has a
        // stable key for cross-session lookups.
        let (ctx, proj_tmp) = workflow_ctx_with_task(vec![], TaskStatus::Planning).await;
        let task_dir = proj_tmp.path().join(".everlasting/tasks/step26-fixture");

        let handle = new_handle();
        let input = serde_json::json!({
            "items": [{"content": "build prototype", "status": "in_progress"}]
        });
        let (_out, _is_err) = execute(&input, &handle, &ctx).await;

        let persisted = read_persisted_items(&task_dir);
        assert_eq!(persisted.len(), 1);
        assert!(
            persisted[0].id.starts_with("auto-"),
            "missing id must derive to auto-{{hash}} (got: {})",
            persisted[0].id,
        );
        assert!(persisted[0].id.len() > 5, "hash must be non-trivial");
    }

    #[test]
    fn checklist_item_parses_id_and_tdd_from_json() {
        // The LLM-facing schema now exposes `id` + `tdd`
        // (optional). Parse must round-trip both fields.
        let input = serde_json::json!({
            "items": [
                {
                    "id": "backend-impl",
                    "content": "implement backend",
                    "status": "in_progress",
                    "tdd": true
                }
            ]
        });
        let items = parse_and_coerce(&input);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "backend-impl");
        assert_eq!(items[0].tdd, Some(true));
        assert_eq!(items[0].status, ChecklistStatus::InProgress);
    }

    #[test]
    fn checklist_item_omitted_id_and_tdd_default_cleanly() {
        // Backward compat: legacy items without `id` /
        // `tdd` must still parse (id="" / tdd=None).
        let input = serde_json::json!({
            "items": [{"content": "x", "status": "done"}]
        });
        let items = parse_and_coerce(&input);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "");
        assert_eq!(items[0].tdd, None);
        assert_eq!(items[0].status, ChecklistStatus::Done);
    }
}
