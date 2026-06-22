//! Transcript types + `subagent:event` / `subagent:finished` IPC
//! payload builders for the worker run.
//!
//! Extracted from `subagent.rs` (split 2026-06-23). Pure data +
//! pure functions — no Tauri runtime, no sink state. Keeping the
//! wire shape in exactly one place lets the TS mirror lock by unit
//! test without spinning up a Tauri runtime.

use serde::{Deserialize, Serialize};

/// One entry in the worker's in-memory transcript. PR1b keeps it
/// **in memory only** — no DB writes (that's PR2's `subagent_runs`
/// table). The transcript accumulates the worker's chat-events /
/// tool calls / tool results so the parent + (future PR2/PR3) the
/// frontend can expand "what did the worker do?" after the fact.
///
/// `Serialize` / `Deserialize` are derived in PR2 so the
/// `Vec<TranscriptEntry>` can round-trip through the
/// `subagent_runs.transcript_json` column (and through the
/// `truncate_transcript_for_persistence` head+tail reparse path).
/// The shape is `{"kind": "<variant>", "payload_json": <any JSON>}` —
/// the inner `payload_json` is already a `serde_json::Value`, so
/// the `#[serde(other)]` on the kind enum (below) is irrelevant
/// here; we just need the outer struct to derive the traits.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // PR1b: in-memory only; PR2 persists, PR3 renders.
pub struct TranscriptEntry {
    pub kind: TranscriptKind,
    pub payload_json: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)] // paired with TranscriptEntry
pub enum TranscriptKind {
    ChatEvent,
    ToolCall,
    ToolResult,
    PermissionAsk,
    /// 2026-06-22 (RULE-WorkerAsk-001): the resolve outcome of a
    /// worker's `PermissionAsk`. Emitted by `SubagentBufferSink
    /// ::emit_permission_ask_resolved` AFTER the `ask_path` worker
    /// branch's `tokio::select!` returns its outcome. The entry
    /// carries `{ rid, outcome }` where `outcome ∈ {"allow", "deny",
    /// "timeout", "cancel"}`. The frontend pairs this entry to the
    /// matching `PermissionAsk` transcript entry by `rid` and
    /// surfaces the outcome as a badge on the historical card.
    ///
    /// **Transcript-only** (NOT dual-emitted on `permission:ask`
    /// IPC) — the live interaction card's disappearance is already
    /// driven by the permissions store removing the pending entry
    /// on resolve. This entry is the historical-replay record for
    /// when the drawer is reopened after the worker exits.
    PermissionAskResolved,
}

/// Build the IPC payload for the `subagent:event` Tauri channel.
/// Pure function — keeps the wire shape in exactly one place so
/// the TS mirror (`runId` / `sessionId` / `kind` / `payload` /
/// `timestamp`) can be locked by unit tests without spinning up a
/// Tauri runtime.
///
/// **Wire shape** (matches prd.md §"PR2 hotfix" decision + the
/// the `transcript_kind_str` mapping below):
/// ```json
/// {
///   "runId": "<DB row id (worker_run_id) — MUST equal summary.id>",
///   "sessionId": "<parent session_id>",
///   "kind": "chat_event" | "tool_call" | "tool_result" | "permission_ask",
///   "payload": <the original chat-event / tool-call / tool-result payload>,
///   "timestamp": "<RFC 3339>"
/// }
/// ```
///
/// The `kind` string is the snake_case of the `TranscriptKind`
/// enum variant (`#[serde(rename_all = "snake_case")]` on the
/// enum). The TS enum must stay in lockstep with this mapping —
/// `trellis-check` verifies line-by-line parity.
///
/// **`runId` contract (B6 PR3b hotfix, 2026-06-21)**: `run_id` MUST
/// be the `subagent_runs.id` DB row id (the UUID `insert_run`
/// returns as `worker_run_id`), NOT the human-readable
/// `worker_rid` (`"{parent_rid}-sub-{tool_use_id}"`). The frontend
/// `subagentRuns` store keys `liveTranscript` / `getRunCache` by
/// `event.runId`, while `ToolCallCard` opens the drawer with
/// `summary.id` (= the same DB id). If the two diverge, the drawer's
/// `transcript`/`status` computeds look up the wrong key and render
/// blank + stuck-on-running. `run_subagent` threads
/// `worker_run_id_opt` (fallback `worker_rid` only when the insert
/// failed — no DB row exists, so the drawer can't open anyway).
pub(super) fn build_subagent_event_payload(
    run_id: &str,
    session_id: &str,
    kind: TranscriptKind,
    payload: serde_json::Value,
) -> serde_json::Value {
    let kind_str = match kind {
        TranscriptKind::ChatEvent => "chat_event",
        TranscriptKind::ToolCall => "tool_call",
        TranscriptKind::ToolResult => "tool_result",
        TranscriptKind::PermissionAsk => "permission_ask",
        TranscriptKind::PermissionAskResolved => "permission_ask_resolved",
    };
    serde_json::json!({
        "runId": run_id,
        "sessionId": session_id,
        "kind": kind_str,
        "payload": payload,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })
}

/// Build the IPC payload for the `subagent:finished` Tauri channel —
/// a one-shot terminal signal emitted by `run_subagent` AFTER
/// `update_run_finished` commits the run's terminal state. Distinct
/// from `subagent:event` (which streams transcript entries while the
/// worker runs): `subagent:finished` carries no transcript entry,
/// only the terminal status + timestamp, so the frontend can refetch
/// `get_subagent_run` + `list_subagent_runs_by_session` and flip the
/// drawer / card from `running` to the terminal state without
/// polling.
///
/// **Wire shape**:
/// ```json
/// {
///   "runId": "<DB row id — same value subagent:event uses>",
///   "sessionId": "<parent session_id>",
///   "status": "completed" | "cancelled" | "error",
///   "finishedAt": "<RFC 3339>"
/// }
/// ```
///
/// `status` is the lowercase wire form of `SubagentStatusDb::as_str`
/// (passed in as `status_str` by the caller to keep this module free
/// of a `db::subagent_runs` type dependency). The frontend
/// `coerceStatus` parses it leniently (unknown → `running`, but the
/// only emitters are the three terminal arms in `run_subagent`).
///
/// Emitted only on the `Ok(())` arm of `update_run_finished` — a DB
/// write failure leaves the row `running`, so emitting `finished`
/// would mislead the frontend into caching a stale `running` row as
/// terminal. The emit itself is best-effort (`tracing::warn!` on
/// failure, mirroring the `subagent:event` emit policy).
pub(crate) fn build_subagent_finished_payload(
    run_id: &str,
    session_id: &str,
    status_str: &str,
    finished_at: &str,
) -> serde_json::Value {
    serde_json::json!({
        "runId": run_id,
        "sessionId": session_id,
        "status": status_str,
        "finishedAt": finished_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `build_subagent_event_payload` produces the exact wire shape
    /// documented in prd.md §"PR2 hotfix":
    /// `{ runId, sessionId, kind, payload, timestamp }`.
    #[test]
    fn build_subagent_event_payload_matches_prd_wire_shape() {
        let payload = build_subagent_event_payload(
            "rid-x",
            "sid-y",
            TranscriptKind::ChatEvent,
            serde_json::json!({"hello": "world"}),
        );
        assert_eq!(payload["runId"], "rid-x");
        assert_eq!(payload["sessionId"], "sid-y");
        assert_eq!(payload["kind"], "chat_event");
        assert_eq!(payload["payload"]["hello"], "world");
        // Timestamp is RFC 3339 — contains "T" + a timezone offset.
        let ts = payload["timestamp"].as_str().expect("timestamp is string");
        assert!(ts.contains('T'), "RFC 3339 timestamp: {ts}");
    }

    /// Every `TranscriptKind` variant maps to its snake_case wire
    /// string.
    #[test]
    fn build_subagent_event_payload_kind_strings_match_enum() {
        for (kind, expected) in [
            (TranscriptKind::ChatEvent, "chat_event"),
            (TranscriptKind::ToolCall, "tool_call"),
            (TranscriptKind::ToolResult, "tool_result"),
            (TranscriptKind::PermissionAsk, "permission_ask"),
            (TranscriptKind::PermissionAskResolved, "permission_ask_resolved"),
        ] {
            let p = build_subagent_event_payload("rid", "sid", kind, serde_json::Value::Null);
            assert_eq!(p["kind"], expected, "kind={kind:?} wire form");
        }
    }

    /// `build_subagent_finished_payload` produces the one-shot
    /// terminal signal wire shape `{ runId, sessionId, status,
    /// finishedAt }`. B6 PR3b hotfix (2026-06-21).
    #[test]
    fn build_subagent_finished_payload_matches_wire_shape() {
        let payload = build_subagent_finished_payload(
            "run-uuid-123",
            "sid-y",
            "completed",
            "2026-06-21T12:00:00+00:00",
        );
        assert_eq!(payload["runId"], "run-uuid-123");
        assert_eq!(payload["sessionId"], "sid-y");
        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["finishedAt"], "2026-06-21T12:00:00+00:00");
        // No `kind` / `payload` / `timestamp` fields — this is NOT a
        // transcript entry (distinct from subagent:event).
        assert!(payload.get("kind").is_none());
        assert!(payload.get("payload").is_none());
        assert!(payload.get("timestamp").is_none());
    }
}
