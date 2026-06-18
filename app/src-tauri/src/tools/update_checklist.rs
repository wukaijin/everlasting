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
#[derive(Debug, Clone, PartialEq, Eq, serde :: Serialize, serde :: Deserialize)]
pub struct ChecklistItem {
    pub content: String,
    pub status: ChecklistStatus,
}

/// The `update_checklist` tool definition registered in
/// `builtin_tools()`.
pub fn definition() -> ToolDef {
    ToolDef {
        name: "update_checklist".to_string(),
        description: Some(
            "Update your running progress checklist for this task. Pass the FULL list of \
             items every call — the new list replaces the old one atomically (not append). \
             Each item has `content` (short description) and `status` \
             (`pending` / `in_progress` / `done`). At most one item should be \
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
                            "content": {
                                "type": "string",
                                "description": "Short description of the step."
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "done"],
                                "description": "Current state of the item."
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
        parsed.push(ChecklistItem {
            content: content.to_string(),
            status,
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
pub async fn execute(input: &serde_json::Value, handle: &ChecklistHandle) -> (String, bool) {
    let new_items = parse_and_coerce(input);
    // Atomic full-replace. The lock is held only for the swap; no
    // I/O inside the critical section.
    {
        let mut guard = handle.lock().await;
        guard.clear();
        guard.extend(new_items.iter().cloned());
    }
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
        "Checklist updated ({} items, {} done, {} in_progress).\n\n{}",
        new_items.len(),
        done_count,
        in_progress_count,
        body
    );
    (summary, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(content: &str, status: ChecklistStatus) -> ChecklistItem {
        ChecklistItem {
            content: content.to_string(),
            status,
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
        let has_items = required
            .iter()
            .any(|v| v.as_str() == Some("items"));
        assert!(has_items, "items must be required");
    }

    #[test]
    fn definition_schema_status_enum_covers_three_states() {
        let schema = definition().input_schema;
        let status_enum = schema
            .pointer("/properties/items/items/properties/status/enum")
            .and_then(|v| v.as_array())
            .expect("status enum present");
        let strs: Vec<&str> = status_enum
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
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
        let (out1, is_err1) = execute(&input1, &handle).await;
        assert!(!is_err1, "{}", out1);
        assert_eq!(handle.lock().await.len(), 3);

        // Second call: 2 completely different items.
        let input2 = serde_json::json!({
            "items": [
                {"content": "x", "status": "pending"},
                {"content": "y", "status": "pending"}
            ]
        });
        let (out2, is_err2) = execute(&input2, &handle).await;
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
        let (out, is_err) = execute(&input, &handle).await;
        assert!(!is_err, "coerce must NOT error");
        let stored = handle.lock().await.clone();
        assert_eq!(stored.len(), 2);
        let in_progress: Vec<_> = stored
            .iter()
            .filter(|i| i.status == ChecklistStatus::InProgress)
            .collect();
        assert_eq!(
            in_progress.len(),
            1,
            "exactly one in_progress after coerce"
        );
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
        execute(&seed, &handle).await;
        assert_eq!(handle.lock().await.len(), 1);

        // Now empty.
        let input = serde_json::json!({"items": []});
        let (out, is_err) = execute(&input, &handle).await;
        assert!(!is_err);
        assert!(handle.lock().await.is_empty(), "empty items must clear the list");
        assert!(out.contains("0 items") || out.contains("empty checklist"));
    }

    #[tokio::test]
    async fn execute_missing_items_key_treated_as_empty() {
        let handle = new_handle();
        // Completely missing `items` — defensive parse returns empty.
        let input = serde_json::json!({});
        let (_out, is_err) = execute(&input, &handle).await;
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
        let (_out, is_err) = execute(&input, &handle).await;
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
        let (out, _) = execute(&input, &handle).await;
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
}
