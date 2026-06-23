#![cfg(test)]

use crate::agent::permissions::AuditKind;

#[test]
fn audit_kind_round_trip() {
    for k in [
        AuditKind::ToolDenied,
        AuditKind::ToolAllowed,
        AuditKind::ToolPermissionAsk,
        AuditKind::PermissionGranted,
        AuditKind::ModeChanged,
        AuditKind::YoloEntered,
        AuditKind::YoloExited,
        AuditKind::ToolDeniedYolo,
        AuditKind::PermissionTimeout,
        AuditKind::RequestCancelled,
        AuditKind::ToolExecuted,
        // D3 PR1 (2026-06-17): user message edit kind. Locked
        // alongside the other variants so a future rename breaks
        // this test instead of corrupting audit rows.
        AuditKind::EditMessage,
        // D3 PR3 (2026-06-17): user resend kind. Same wire
        // string lock as the other variants — the DB layer
        // (`record_message_resend_audit` helper) writes
        // `AuditKind::ResendMessage.as_str()` verbatim; both
        // ends of the contract must agree on this string.
        AuditKind::ResendMessage,
        // 2026-06-22 (RULE-FrontSubagent-003 fix): the 4 worker
        // ask terminal kinds. Wire strings are stable — the DB
        // layer's `record_audit` helper writes them verbatim
        // from `ask_path`'s worker branch.
        AuditKind::WorkerAskAllowed,
        AuditKind::WorkerAskDenied,
        AuditKind::WorkerAskTimedOut,
        AuditKind::WorkerAskCancelled,
    ] {
        let s = k.as_str();
        assert!(!s.is_empty());
        assert!(s.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
    }
    // C4 PR1 (2026-06-14): lock the new variant's wire string so a
    // future rename / typo here breaks the test instead of corrupting
    // audit rows the frontend can no longer dispatch on.
    assert_eq!(AuditKind::ToolExecuted.as_str(), "tool_executed");
    // D3 PR1 (2026-06-17): pin the wire string for the new edit
    // kind. The DB layer (`db::sessions::edit_user_message`) writes
    // `'edit_message'` verbatim — these two strings MUST agree.
    assert_eq!(AuditKind::EditMessage.as_str(), "edit_message");
    // D3 PR3 (2026-06-17): pin the wire string for the new resend
    // kind. The DB layer's `record_message_resend_audit` helper
    // writes `AuditKind::ResendMessage.as_str()` verbatim — both
    // ends of the contract must agree on this string.
    assert_eq!(AuditKind::ResendMessage.as_str(), "resend_message");
    // 2026-06-22 (RULE-FrontSubagent-003 fix): pin the wire
    // strings for the 4 worker ask terminal kinds. The
    // `record_audit` helper in `ask_path`'s worker branch writes
    // these strings verbatim into `session_audit_events.kind`.
    assert_eq!(AuditKind::WorkerAskAllowed.as_str(), "worker_ask_allowed");
    assert_eq!(AuditKind::WorkerAskDenied.as_str(), "worker_ask_denied");
    assert_eq!(AuditKind::WorkerAskTimedOut.as_str(), "worker_ask_timed_out");
    assert_eq!(AuditKind::WorkerAskCancelled.as_str(), "worker_ask_cancelled");
}
