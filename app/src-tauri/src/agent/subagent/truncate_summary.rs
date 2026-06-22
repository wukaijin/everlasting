//! Transcript truncation (4 MiB cap), status-prefix formatters
//! (`format_final_text` / `format_dispatch_result`), and the
//! worker partial-actions summary (`summarize_worker_tool_actions`).
//!
//! Extracted from `subagent.rs` (split 2026-06-23). Pure functions
//! — no sink state, no Tauri runtime. The cap semantics live next
//! to the transcript types they bound; the formatters live next to
//! the `SubagentStatus` enum that drives them.

use std::collections::HashMap;

use super::transcript::{TranscriptEntry, TranscriptKind};
use super::SubagentStatus;

// ---------------------------------------------------------------------------
// Transcript 4MB cap (B6 PR2)
// ---------------------------------------------------------------------------

/// Maximum size (in bytes) of a serialized `Vec<TranscriptEntry>`
/// that will be persisted into `subagent_runs.transcript_json`.
///
/// **4 MiB** is the B6 PR2 design decision (see PRD §"transcript
/// 大小 cap"): safely under SQLite's TEXT default cap (1 GiB) while
/// far above the 20-turn worker's realistic worst case (a busy
/// tool-use turn produces ~2-5 KiB of transcript, so 20 turns ≈
/// 100 KiB — three orders of magnitude under the cap). When the
/// transcript exceeds the cap, [`truncate_transcript_for_persistence`]
/// marks `transcript_truncated=1` and keeps a head + tail
/// representative slice so PR3's expand UI still shows both the
/// "what did the worker start with?" and "what did it end with?"
/// context.
pub const TRANSCRIPT_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Serialize-then-cap helper. Returns `(transcript, truncated)`:
///
/// - If the JSON-serialized transcript fits in `max_bytes`, the
///   original is returned and `truncated=false`.
/// - If it doesn't, the function keeps the head and tail halves
///   of the byte representation (each `max_bytes / 2` bytes),
///   parses them back into `TranscriptEntry` vectors, and returns
///   the **union** (head + tail entries) plus `truncated=true`.
///   The parsing may fail on a half-element boundary (e.g. the
///   head cut lands in the middle of a JSON value); in that case
///   the function falls back to keeping just the head bytes (no
///   parse) under the assumption that PR3's render path will
///   surface the raw bytes — a degraded but never-empty result.
///
/// The function is **pure** (no I/O) and lives next to the sink
/// so the cap semantics are co-located with the type the cap
/// bounds. PR2's `run_subagent` calls this immediately before
/// `db::subagent_runs::update_run_finished` so the DB write
/// receives a transcript that already meets the cap.
pub fn truncate_transcript_for_persistence(
    transcript: Vec<TranscriptEntry>,
    max_bytes: usize,
) -> (Vec<TranscriptEntry>, bool) {
    let json = match serde_json::to_string(&transcript) {
        Ok(s) => s,
        Err(_) => {
            // Serialization should never fail for `TranscriptEntry`
            // (its `payload_json` is already `serde_json::Value`),
            // but the safe fallback is "return as-is, mark
            // truncated" so the caller still persists SOMETHING.
            return (transcript, true);
        }
    };
    if json.len() <= max_bytes {
        return (transcript, false);
    }
    // Over cap: keep head + tail halves (each `max_bytes / 2` bytes).
    // The cap is large enough that we don't need to worry about
    // the head/tail split landing inside a single-element JSON —
    // the reparse failure falls back to keeping the head bytes as
    // a single-element vector.
    let half = max_bytes / 2;
    let head_end = half.min(json.len());
    let tail_start = json.len().saturating_sub(half);
    // Build a synthetic JSON array: `[<head_bytes_trimmed_to_array_end>..., <tail_bytes_trimmed_to_array_end>...]`
    // by attempting to find the last `}` in the head and the first
    // `{` after `tail_start`. If the parse fails, the caller
    // persists the head bytes as a single-element vector (raw
    // JSON fragment). This branch should be unreachable in
    // practice — 4 MiB of transcript contains millions of `}`
    // chars — but the defensive fallback is cheap.
    let head_trim = find_last_close_brace(&json[..head_end]);
    let tail_trim_start = find_first_open_brace(&json[tail_start..])
        .map(|i| tail_start + i)
        .unwrap_or(tail_start);
    let truncated_json = if let (Some(h), true) = (head_trim, tail_trim_start < json.len()) {
        // Concatenate head[..h] + tail[tail_trim_start..] wrapped in
        // a synthetic array. The two halves are JSON-serialized
        // TranscriptEntry values; we wrap them in a JSON array.
        format!(
            "[{}]",
            [&json[..h], &json[tail_trim_start..]].join(",")
        )
    } else if let Some(h) = head_trim {
        // Tail parse failed; keep just the head (truncated).
        format!("[{}]", &json[..h])
    } else {
        // Head parse failed too; keep the raw head bytes as a
        // single-element fallback (last resort). The shape is
        // invalid JSON but the transcript_truncated=1 flag tells
        // PR3's render to surface a degraded view.
        return (vec![make_raw_fallback_entry(&json[..head_end])], true);
    };
    match serde_json::from_str::<Vec<TranscriptEntry>>(&truncated_json) {
        Ok(parsed) => (parsed, true),
        Err(_) => (vec![make_raw_fallback_entry(&json[..head_end])], true),
    }
}

/// Find the byte index of the last `}` in `s[..=]` that is at or
/// before `s.len()`. Returns `None` if no `}` is found.
fn find_last_close_brace(s: &str) -> Option<usize> {
    s.rfind('}').map(|i| i + 1)
}

/// Find the byte index of the first `{` in `s[i..]`. Returns
/// `None` if no `{` is found. The returned offset is relative to
/// `s`, not the caller's slice.
fn find_first_open_brace(s: &str) -> Option<usize> {
    s.find('{')
}

/// Build a single fallback `TranscriptEntry` carrying a raw JSON
/// fragment as its payload. Used when the head+tail reparse fails
/// (extremely rare; documented in [`truncate_transcript_for_persistence`]).
fn make_raw_fallback_entry(raw: &str) -> TranscriptEntry {
    TranscriptEntry {
        kind: TranscriptKind::ChatEvent,
        payload_json: serde_json::json!({
            "_truncation_fallback": true,
            "raw_head_bytes": raw,
        }),
    }
}

// ---------------------------------------------------------------------------
// Status-prefix formatter for the dispatch_subagent tool_result
// ---------------------------------------------------------------------------

/// Format the worker's final assistant text into the prefix-stripped
/// shape that lands in `subagent_runs.final_text` (and the drawer
/// Reply segment). Per PRD §"worker exit hook" + R2:
///
/// - `status: completed` → `<summary>` (empty text → `(worker produced no final text)`)
/// - `status: cancelled` → `<partial>\n\n[CANCELLED_MARKER]` (empty text → `[CANCELLED_MARKER]`)
/// - `status: error`     → `<error text>` (empty text → `(no error text captured)`)
/// - `status: incomplete` → `<partial>\n\n[INCOMPLETE_MARKER]` (empty text → `[INCOMPLETE_MARKER]`) — 2026-06-21 (R2)
///
/// **The `[status: ...]\n` prefix is intentionally NOT included** —
/// the `status` column is the source of truth for the prefix (per
/// the existing `summary` field contract; `subagent-runs-schema.md`
/// §3 "`update_run_finished` 行为"). The drawer reads `final_text`
/// for the Reply segment body; the status badge is rendered from
/// the `status` column separately.
pub fn format_final_text(status: SubagentStatus, worker_text: &str) -> String {
    match status {
        SubagentStatus::Completed => {
            if worker_text.is_empty() {
                "(worker produced no final text)".to_string()
            } else {
                worker_text.to_string()
            }
        }
        SubagentStatus::Cancelled => {
            // Reuse the same CANCELLED_MARKER the parent loop uses
            // for its own cancel path — keeps the wire shape
            // consistent across parent + worker.
            let marker = crate::agent::helpers::CANCELLED_MARKER;
            if worker_text.is_empty() {
                marker.to_string()
            } else {
                format!("{}\n\n{}", worker_text, marker)
            }
        }
        SubagentStatus::Error => {
            if worker_text.is_empty() {
                "(no error text captured)".to_string()
            } else {
                worker_text.to_string()
            }
        }
        SubagentStatus::Incomplete => {
            // 2026-06-21 (R2): max_turns soft-terminal. The
            // worker did useful work but did not cleanly finish
            // within its turn budget. Mirror the Cancelled shape
            // (suffix the marker) so the drawer's text rendering
            // sees a consistent "summary + reason marker" pattern
            // across both soft-terminal statuses. The marker
            // surfaces the budget-exhaustion reason in plain text
            // — a frontend visual differentiation is a separate
            // follow-up (out of scope for this task).
            let marker = crate::agent::helpers::INCOMPLETE_MARKER;
            if worker_text.is_empty() {
                marker.to_string()
            } else {
                format!("{}\n\n{}", worker_text, marker)
            }
        }
    }
}

/// Format the dispatch_subagent tool_result content from the
/// worker's final state. Per PRD §"summary 回填" + research §2:
///
/// - `status: completed` → `[status: completed]\n<summary>`
/// - `status: cancelled` → `[status: cancelled]\n[CANCELLED_MARKER]\n<partial>`
/// - `status: error`     → `[status: error]\n<error text>`
/// - `status: incomplete` → `[status: incomplete]\n[INCOMPLETE_MARKER]\n<partial>` — 2026-06-21 (R2)
///
/// **RULE-BackSubagent-001 (PR2)**: for non-completed terminal states,
/// `partial_actions: Some(non-empty)` appends a `Worker partial
/// actions:\n<summary>` section after the body so the parent LLM can
/// do compensatory repair (skip already-landed writes, retry failed
/// tools). The caller passes `None` for Completed and for empty
/// summaries. The summary is NOT written to `subagent_runs.final_text`
/// — the drawer already renders the full transcript in its Tools
/// segment, so the DB body (`format_final_text`) stays unchanged.
///
/// Returns `(content, is_error)`. `is_error` is `true` for cancel
/// and error so the LLM knows the worker did not succeed; `false`
/// for completed. `incomplete` is treated like `cancelled` for
/// `is_error` purposes — the worker did not cleanly finish, so
/// the parent LLM should treat the result as a soft failure
/// (the worker may have produced useful partial output but
/// should not be treated as a successful delegation).
///
/// Implementation note: the body content is built via
/// [`format_final_text`] (the prefix-stripped shape) and then
/// wrapped with `[status: <status>]\n` — single source of truth
/// for the "what does the worker's final text look like" shape
/// shared between `format_final_text` (DB write) and this
/// function (tool_result wire).
pub fn format_dispatch_result(
    status: SubagentStatus,
    worker_text: &str,
    partial_actions: Option<&str>,
) -> (String, bool) {
    let prefix = format!("[status: {}]", status.as_str());
    let body = format_final_text(status, worker_text);
    let is_error = !matches!(status, SubagentStatus::Completed);
    let content = match partial_actions {
        // RULE-BackSubagent-001: non-completed terminal states append a
        // compact summary of the worker's executed tool_calls. The
        // caller guarantees `None` for Completed; the empty-guard keeps
        // the function total if an empty summary slips through.
        Some(actions) if !actions.is_empty() => {
            format!("{}\n{}\n\nWorker partial actions:\n{}", prefix, body, actions)
        }
        _ => format!("{}\n{}", prefix, body),
    };
    (content, is_error)
}

// ---------------------------------------------------------------------------
// RULE-BackSubagent-001: worker partial-transcript summary
//
// When a worker exits in a non-completed state (Error / Cancelled /
// Incomplete-max_turns), the parent LLM previously saw only
// `[status: error]\n<error text>` — blind to which tool_calls the
// worker had already executed (and which had landed on disk). This
// block builds a compact summary of those actions so the parent can
// do compensatory repair (skip already-landed writes, retry failed
// tools). Wired into `format_dispatch_result`'s `partial_actions`
// parameter in PR2.
// ---------------------------------------------------------------------------

/// Maximum byte size of the worker partial-actions summary appended
/// to a non-completed `dispatch_subagent` tool_result. ~25-50 tool
/// actions fit at ~40-80 chars/line. Sized to inform the parent
/// LLM's compensatory repair decisions without bloating the
/// tool_result content.
pub const PARTIAL_ACTIONS_MAX_BYTES: usize = 2 * 1024;

/// Extract the single most representative input parameter for a tool
/// call — the argument that most identifies "what the tool operated
/// on" (`path` for file/dir tools, `pattern` for search, `command`
/// for shell, `url` for web_fetch, `skill_name` for use_skill).
/// Returns an empty string for tools without a representative
/// parameter (e.g. `update_checklist`) or when the field is absent /
/// non-string. Long values are truncated so a single line can't blow
/// the cap.
fn key_param_for_tool(name: &str, input: &serde_json::Value) -> String {
    let field = match name {
        "read_file" | "write_file" | "edit_file" | "list_dir" => "path",
        "grep" | "glob" => "pattern",
        "shell" => "command",
        "web_fetch" => "url",
        "use_skill" => "skill_name",
        _ => return String::new(),
    };
    let val = match input.get(field).and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return String::new(),
    };
    truncate_chars(val, 60)
}

/// Truncate `s` to at most `max_chars` Unicode scalar values,
/// appending "..." if truncated. Operates on chars (not bytes) to
/// respect multi-byte boundaries (CJK content — see RULE-E-009
/// `floor_char_boundary` convention used elsewhere in the crate).
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let kept: String = s.chars().take(max_chars).collect();
    format!("{}...", kept)
}

/// Build a human-readable summary of the worker's executed tool
/// actions from its transcript snapshot, for inclusion in a non-
/// completed `dispatch_subagent` tool_result (RULE-BackSubagent-001).
///
/// Each `tool_call` transcript entry is paired with its `tool_result`
/// by `tool_use_id`:
/// - paired result with `is_error=false` → `ok`
/// - paired result with `is_error=true`  → `failed`
/// - no paired result (worker exited mid-execution) → `?`
///
/// `chat_event` and `permission_ask` entries are skipped (not
/// relevant to the parent's compensatory repair decisions). Lines
/// appear in transcript (chronological) order.
///
/// The summary is capped at [`PARTIAL_ACTIONS_MAX_BYTES`] using a
/// head+tail strategy: when the full summary would exceed the cap,
/// the longest affordable head + tail are kept with a
/// `... (K actions omitted) ...` marker between them (the parent
/// still sees the earliest actions — what files the worker created —
/// and the latest — its state right before exit). Returns an empty
/// string when the worker executed no tool calls (the caller treats
/// empty as "do not append the section").
pub fn summarize_worker_tool_actions(transcript: &[TranscriptEntry]) -> String {
    // Collect tool_result outcomes keyed by tool_use_id.
    let mut results: HashMap<&str, bool> = HashMap::new();
    for entry in transcript {
        if entry.kind != TranscriptKind::ToolResult {
            continue;
        }
        if let Some(id) = entry
            .payload_json
            .get("tool_use_id")
            .and_then(|v| v.as_str())
        {
            let is_error = entry
                .payload_json
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            results.insert(id, is_error);
        }
    }

    // Build one line per tool_call, in transcript order.
    let mut lines: Vec<String> = Vec::new();
    for entry in transcript {
        if entry.kind != TranscriptKind::ToolCall {
            continue;
        }
        let name = entry
            .payload_json
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let input = entry
            .payload_json
            .get("input")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let id = entry
            .payload_json
            .get("tool_use_id")
            .and_then(|v| v.as_str());
        let key_param = key_param_for_tool(name, &input);
        let status = match id.and_then(|i| results.get(i).copied()) {
            Some(false) => "ok",
            Some(true) => "failed",
            None => "?",
        };
        let line = if key_param.is_empty() {
            format!("- {}: {}", name, status)
        } else {
            format!("- {}({}): {}", name, key_param, status)
        };
        lines.push(line);
    }

    if lines.is_empty() {
        return String::new();
    }

    apply_head_tail_cap(lines, PARTIAL_ACTIONS_MAX_BYTES)
}

/// Cap a list of summary lines to `max_bytes` using a head+tail
/// split. When the joined lines fit, they are returned as-is.
/// Otherwise the longest affordable head prefix + tail suffix are
/// kept, with a `... (K actions omitted) ...` marker between them.
///
/// Proven to stay strictly `<= max_bytes`: head + tail byte costs
/// each ≤ `half`, and the marker is bounded by the reserved
/// `marker_overhead`, so the reassembled output is `max - 5` at most.
fn apply_head_tail_cap(lines: Vec<String>, max_bytes: usize) -> String {
    let full = lines.join("\n");
    if full.len() <= max_bytes {
        return full;
    }

    // Reserve room for the omission marker. Worst-case marker length:
    // "... (NNNNNN actions omitted) ...\n" ≈ 36 bytes. Using 40 keeps
    // the reassembled result strictly <= max_bytes.
    let marker_overhead = 40;
    let usable = max_bytes.saturating_sub(marker_overhead);
    let half = usable / 2;

    // Greedy head: longest prefix whose byte cost (line + newline) ≤ half.
    let mut head_end = 0usize;
    let mut head_bytes = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let cost = line.len() + 1;
        if head_bytes + cost > half {
            break;
        }
        head_bytes += cost;
        head_end = i + 1;
    }

    // Greedy tail: longest suffix whose cost ≤ half, starting strictly
    // after head_end (no overlap).
    let mut tail_start = lines.len();
    let mut tail_bytes = 0usize;
    for i in (head_end..lines.len()).rev() {
        let cost = lines[i].len() + 1;
        if tail_bytes + cost > half {
            break;
        }
        tail_bytes += cost;
        tail_start = i;
    }

    let tail_count = lines.len() - tail_start;
    let omitted = lines.len() - head_end - tail_count;

    let mut out = String::new();
    for line in &lines[..head_end] {
        out.push_str(line);
        out.push('\n');
    }
    if omitted > 0 {
        out.push_str(&format!("... ({} actions omitted) ...\n", omitted));
    }
    for line in &lines[tail_start..] {
        out.push_str(line);
        out.push('\n');
    }
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- helpers ----

    fn tc_entry(tool_use_id: &str, name: &str, input: serde_json::Value) -> TranscriptEntry {
        TranscriptEntry {
            kind: TranscriptKind::ToolCall,
            payload_json: serde_json::json!({
                "name": name,
                "input": input,
                "tool_use_id": tool_use_id,
                "id": tool_use_id,
                "request_id": "req",
            }),
        }
    }

    fn tr_entry(tool_use_id: &str, is_error: bool) -> TranscriptEntry {
        TranscriptEntry {
            kind: TranscriptKind::ToolResult,
            payload_json: serde_json::json!({
                "tool_use_id": tool_use_id,
                "is_error": is_error,
                "content": "result body",
                "request_id": "req",
            }),
        }
    }

    fn make_entry(payload: &str) -> TranscriptEntry {
        TranscriptEntry {
            kind: TranscriptKind::ChatEvent,
            payload_json: serde_json::json!({"text": payload}),
        }
    }

    // ---- format_dispatch_result ----

    #[test]
    fn format_completed_with_summary() {
        let (content, is_error) =
            format_dispatch_result(SubagentStatus::Completed, "found 3 files", None);
        assert!(!is_error);
        assert!(content.starts_with("[status: completed]"));
        assert!(content.contains("found 3 files"));
    }

    #[test]
    fn format_completed_with_empty_text_falls_back_to_note() {
        let (content, is_error) = format_dispatch_result(SubagentStatus::Completed, "", None);
        assert!(!is_error);
        assert!(content.contains("worker produced no final text"));
    }

    #[test]
    fn format_cancelled_includes_marker() {
        let (content, is_error) =
            format_dispatch_result(SubagentStatus::Cancelled, "partial", None);
        assert!(is_error);
        assert!(content.starts_with("[status: cancelled]"));
        assert!(content.contains(crate::agent::helpers::CANCELLED_MARKER));
        assert!(content.contains("partial"));
    }

    #[test]
    fn format_cancelled_empty_text_uses_marker_alone() {
        let (content, is_error) = format_dispatch_result(SubagentStatus::Cancelled, "", None);
        assert!(is_error);
        assert!(content.contains(crate::agent::helpers::CANCELLED_MARKER));
    }

    #[test]
    fn format_error_includes_status_prefix() {
        let (content, is_error) =
            format_dispatch_result(SubagentStatus::Error, "LLM stream errored", None);
        assert!(is_error);
        assert!(content.starts_with("[status: error]"));
        assert!(content.contains("LLM stream errored"));
    }

    #[test]
    fn format_dispatch_result_appends_partial_actions_when_some() {
        let actions = "- write_file(a.rs): ok\n- grep(X): failed";
        let (content, is_error) =
            format_dispatch_result(SubagentStatus::Error, "stream died", Some(actions));
        assert!(is_error);
        assert!(content.starts_with("[status: error]\n"));
        assert!(content.contains("stream died"));
        assert!(
            content.contains("Worker partial actions:\n- write_file(a.rs): ok\n- grep(X): failed"),
            "missing partial actions section: {}",
            content
        );
    }

    #[test]
    fn format_dispatch_result_none_partial_actions_has_no_section() {
        let (content, _) = format_dispatch_result(SubagentStatus::Error, "stream died", None);
        assert!(!content.contains("Worker partial actions"));
    }

    #[test]
    fn format_dispatch_result_empty_partial_actions_has_no_section() {
        let (content, _) = format_dispatch_result(SubagentStatus::Error, "stream died", Some(""));
        assert!(!content.contains("Worker partial actions"));
    }

    // ---- format_final_text (B6 redesign PR1, 2026-06-21) ----

    #[test]
    fn format_final_text_completed_returns_plain_summary() {
        let body = format_final_text(SubagentStatus::Completed, "found 3 files");
        assert_eq!(body, "found 3 files");
        assert!(
            !body.starts_with("[status:"),
            "no status prefix — the column carries that"
        );
    }

    #[test]
    fn format_final_text_completed_empty_falls_back_to_note() {
        let body = format_final_text(SubagentStatus::Completed, "");
        assert_eq!(body, "(worker produced no final text)");
    }

    #[test]
    fn format_final_text_cancelled_appends_marker() {
        let body = format_final_text(SubagentStatus::Cancelled, "partial result");
        assert_eq!(
            body,
            format!("partial result\n\n{}", crate::agent::helpers::CANCELLED_MARKER)
        );
    }

    #[test]
    fn format_final_text_cancelled_empty_uses_marker_alone() {
        let body = format_final_text(SubagentStatus::Cancelled, "");
        assert_eq!(body, crate::agent::helpers::CANCELLED_MARKER);
    }

    #[test]
    fn format_final_text_error_returns_plain_error_text() {
        let body = format_final_text(SubagentStatus::Error, "LLM stream errored");
        assert_eq!(body, "LLM stream errored");
    }

    #[test]
    fn format_final_text_error_empty_falls_back_to_note() {
        let body = format_final_text(SubagentStatus::Error, "");
        assert_eq!(body, "(no error text captured)");
    }

    /// The wire format of `format_dispatch_result` must equal
    /// `[status: X]\n` + `format_final_text(X, ...)` — single source
    /// of truth for the prefix-stripped shape.
    #[test]
    fn format_dispatch_result_is_prefix_plus_format_final_text() {
        for (status, text) in [
            (SubagentStatus::Completed, "found 3 files"),
            (SubagentStatus::Completed, ""),
            (SubagentStatus::Cancelled, "partial"),
            (SubagentStatus::Cancelled, ""),
            (SubagentStatus::Error, "stream died"),
            (SubagentStatus::Error, ""),
        ] {
            let (wire, _is_err) = format_dispatch_result(status, text, None);
            let body = format_final_text(status, text);
            let expected = format!("[status: {}]\n{}", status.as_str(), body);
            assert_eq!(wire, expected, "drift for ({:?}, {:?})", status, text);
        }
    }

    // ---- R2 (2026-06-21) format helpers: Incomplete ----

    #[test]
    fn format_final_text_incomplete_appends_marker() {
        let body = format_final_text(SubagentStatus::Incomplete, "partial result");
        assert_eq!(
            body,
            format!("partial result\n\n{}", crate::agent::helpers::INCOMPLETE_MARKER)
        );
    }

    #[test]
    fn format_final_text_incomplete_empty_uses_marker_alone() {
        let body = format_final_text(SubagentStatus::Incomplete, "");
        assert_eq!(body, crate::agent::helpers::INCOMPLETE_MARKER);
    }

    #[test]
    fn format_incomplete_includes_status_prefix_and_is_error() {
        let (content, is_error) =
            format_dispatch_result(SubagentStatus::Incomplete, "partial", None);
        assert!(is_error, "Incomplete must set is_error=true");
        assert!(content.starts_with("[status: incomplete]"));
        assert!(content.contains(crate::agent::helpers::INCOMPLETE_MARKER));
        assert!(content.contains("partial"));
    }

    #[test]
    fn format_dispatch_result_is_prefix_plus_format_final_text_includes_incomplete() {
        for (status, text) in [
            (SubagentStatus::Completed, "found 3 files"),
            (SubagentStatus::Completed, ""),
            (SubagentStatus::Cancelled, "partial"),
            (SubagentStatus::Cancelled, ""),
            (SubagentStatus::Error, "stream died"),
            (SubagentStatus::Error, ""),
            (SubagentStatus::Incomplete, "budget exhausted mid-task"),
            (SubagentStatus::Incomplete, ""),
        ] {
            let (wire, _is_err) = format_dispatch_result(status, text, None);
            let body = format_final_text(status, text);
            let expected = format!("[status: {}]\n{}", status.as_str(), body);
            assert_eq!(wire, expected, "drift for ({:?}, {:?})", status, text);
        }
    }

    // ---- summarize_worker_tool_actions (RULE-BackSubagent-001) ----

    #[test]
    fn summarize_empty_transcript_returns_empty() {
        assert_eq!(summarize_worker_tool_actions(&[]), "");
    }

    #[test]
    fn summarize_no_tool_calls_returns_empty() {
        let transcript = vec![
            TranscriptEntry {
                kind: TranscriptKind::ChatEvent,
                payload_json: serde_json::json!({"text": "thinking"}),
            },
            TranscriptEntry {
                kind: TranscriptKind::PermissionAsk,
                payload_json: serde_json::json!({"tool": "write_file"}),
            },
        ];
        assert_eq!(summarize_worker_tool_actions(&transcript), "");
    }

    #[test]
    fn summarize_pairs_ok_failed_orphan() {
        let transcript = vec![
            tc_entry("tc-1", "write_file", serde_json::json!({"path": "a.rs"})),
            tr_entry("tc-1", false),
            tc_entry("tc-2", "shell", serde_json::json!({"command": "npm test"})),
            tr_entry("tc-2", true),
            tc_entry("tc-3", "read_file", serde_json::json!({"path": "b.rs"})),
        ];
        let out = summarize_worker_tool_actions(&transcript);
        assert_eq!(
            out,
            "- write_file(a.rs): ok\n\
             - shell(npm test): failed\n\
             - read_file(b.rs): ?"
        );
    }

    #[test]
    fn summarize_key_param_per_tool() {
        let transcript = vec![
            tc_entry("t-read", "read_file", serde_json::json!({"path": "r.rs"})),
            tr_entry("t-read", false),
            tc_entry("t-write", "write_file", serde_json::json!({"path": "w.rs", "content": "x"})),
            tr_entry("t-write", false),
            tc_entry("t-edit", "edit_file", serde_json::json!({"path": "e.rs"})),
            tr_entry("t-edit", false),
            tc_entry("t-list", "list_dir", serde_json::json!({"path": "d"})),
            tr_entry("t-list", false),
            tc_entry("t-grep", "grep", serde_json::json!({"pattern": "TODO", "path": "d"})),
            tr_entry("t-grep", false),
            tc_entry("t-glob", "glob", serde_json::json!({"pattern": "**/*.rs"})),
            tr_entry("t-glob", false),
            tc_entry("t-shell", "shell", serde_json::json!({"command": "ls"})),
            tr_entry("t-shell", false),
            tc_entry("t-web", "web_fetch", serde_json::json!({"url": "https://x"})),
            tr_entry("t-web", false),
            tc_entry("t-skill", "use_skill", serde_json::json!({"skill_name": "code-review"})),
            tr_entry("t-skill", false),
        ];
        let out = summarize_worker_tool_actions(&transcript);
        assert!(out.contains("- read_file(r.rs): ok"), "read_file path: {}", out);
        assert!(out.contains("- write_file(w.rs): ok"), "write_file path: {}", out);
        assert!(out.contains("- edit_file(e.rs): ok"), "edit_file path: {}", out);
        assert!(out.contains("- list_dir(d): ok"), "list_dir path: {}", out);
        assert!(out.contains("- grep(TODO): ok"), "grep pattern: {}", out);
        assert!(out.contains("- glob(**/*.rs): ok"), "glob pattern: {}", out);
        assert!(out.contains("- shell(ls): ok"), "shell command: {}", out);
        assert!(out.contains("- web_fetch(https://x): ok"), "web_fetch url: {}", out);
        assert!(out.contains("- use_skill(code-review): ok"), "use_skill skill_name: {}", out);
    }

    #[test]
    fn summarize_unknown_tool_no_param() {
        let transcript = vec![
            tc_entry("u-1", "update_checklist", serde_json::json!({"items": []})),
            tr_entry("u-1", false),
            tc_entry("u-2", "mystery_tool", serde_json::json!({"x": 1})),
            tr_entry("u-2", false),
        ];
        let out = summarize_worker_tool_actions(&transcript);
        assert!(out.contains("- update_checklist: ok"), "no param: {}", out);
        assert!(out.contains("- mystery_tool: ok"), "unknown tool: {}", out);
        assert!(!out.contains("update_checklist("), "no parens for update_checklist: {}", out);
    }

    #[test]
    fn summarize_skips_chat_event_and_permission_ask_interleaved() {
        let transcript = vec![
            TranscriptEntry {
                kind: TranscriptKind::ChatEvent,
                payload_json: serde_json::json!({"text": "planning"}),
            },
            tc_entry("i-1", "write_file", serde_json::json!({"path": "a.rs"})),
            tr_entry("i-1", false),
            TranscriptEntry {
                kind: TranscriptKind::PermissionAsk,
                payload_json: serde_json::json!({"tool": "shell"}),
            },
        ];
        let out = summarize_worker_tool_actions(&transcript);
        assert_eq!(out, "- write_file(a.rs): ok");
    }

    #[test]
    fn summarize_preserves_transcript_order() {
        let transcript = vec![
            tc_entry("o-1", "read_file", serde_json::json!({"path": "first.rs"})),
            tc_entry("o-2", "read_file", serde_json::json!({"path": "second.rs"})),
            tc_entry("o-3", "read_file", serde_json::json!({"path": "third.rs"})),
            tr_entry("o-1", false),
            tr_entry("o-2", false),
            tr_entry("o-3", false),
        ];
        let out = summarize_worker_tool_actions(&transcript);
        assert_eq!(
            out,
            "- read_file(first.rs): ok\n\
             - read_file(second.rs): ok\n\
             - read_file(third.rs): ok"
        );
    }

    #[test]
    fn key_param_truncates_long_values() {
        let long = "x".repeat(200);
        assert_eq!(
            key_param_for_tool("shell", &serde_json::json!({"command": long})),
            format!("{}...", "x".repeat(60))
        );
        // Multi-byte safe: CJK counts as chars not bytes.
        let cjk = "文".repeat(80);
        assert_eq!(
            key_param_for_tool("write_file", &serde_json::json!({"path": cjk})),
            format!("{}...", "文".repeat(60))
        );
    }

    #[test]
    fn summarize_under_cap_has_no_omission_marker() {
        let transcript = vec![
            tc_entry("c-1", "read_file", serde_json::json!({"path": "a.rs"})),
            tr_entry("c-1", false),
            tc_entry("c-2", "grep", serde_json::json!({"pattern": "X"})),
            tr_entry("c-2", false),
        ];
        let out = summarize_worker_tool_actions(&transcript);
        assert!(!out.contains("actions omitted"), "no marker under cap: {}", out);
    }

    #[test]
    fn summarize_head_tail_cap_when_over_budget() {
        // 60 orphan tool_calls (~50 chars each ≈ 3000 bytes) > 2 KiB cap.
        let transcript: Vec<TranscriptEntry> = (0..60)
            .map(|i| {
                tc_entry(
                    &format!("tc-{}", i),
                    "write_file",
                    serde_json::json!({"path": format!("app/src/deeply/nested/file-{}.rs", i)}),
                )
            })
            .collect();
        let out = summarize_worker_tool_actions(&transcript);

        assert!(
            out.len() <= PARTIAL_ACTIONS_MAX_BYTES,
            "output {} > cap {}",
            out.len(),
            PARTIAL_ACTIONS_MAX_BYTES
        );
        assert!(out.contains("actions omitted"), "missing marker: {}", out);
        assert!(out.contains("file-0.rs"), "head dropped: {}", out);
        assert!(out.contains("file-59.rs"), "tail dropped: {}", out);
        assert!(!out.contains("file-30.rs"), "middle not dropped: {}", out);
    }

    // ---- truncate_transcript_for_persistence (B6 PR2) ----

    #[test]
    fn truncate_under_cap_returns_original() {
        let entries: Vec<TranscriptEntry> = (0..10)
            .map(|i| make_entry(&format!("entry-{}", i)))
            .collect();
        let (out, truncated) = truncate_transcript_for_persistence(entries.clone(), 4096);
        assert!(!truncated);
        assert_eq!(out.len(), 10);
    }

    #[test]
    fn truncate_at_exact_cap_returns_original() {
        let entries: Vec<TranscriptEntry> = (0..5)
            .map(|i| make_entry(&format!("entry-{}", i)))
            .collect();
        let json = serde_json::to_string(&entries).unwrap();
        let (out, truncated) = truncate_transcript_for_persistence(entries, json.len());
        assert!(!truncated, "size == cap should NOT truncate");
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn truncate_over_cap_marks_truncated_and_keeps_entries() {
        let entries: Vec<TranscriptEntry> = (0..200)
            .map(|_| make_entry(&"x".repeat(40)))
            .collect();
        let json = serde_json::to_string(&entries).unwrap();
        assert!(json.len() > 1024, "test setup: should exceed 1KiB");
        let (out, truncated) = truncate_transcript_for_persistence(entries, 1024);
        assert!(truncated, "over cap must set truncated=true");
        assert!(out.len() < 200);
        let re = serde_json::to_string(&out).unwrap();
        assert!(re.len() < json.len());
    }

    #[test]
    fn truncate_empty_transcript_returns_empty() {
        let (out, truncated) = truncate_transcript_for_persistence(Vec::new(), 1024);
        assert!(out.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn truncate_uses_default_4mb_when_called_via_run_subagent_path() {
        assert_eq!(TRANSCRIPT_MAX_BYTES, 4 * 1024 * 1024);
    }
}
