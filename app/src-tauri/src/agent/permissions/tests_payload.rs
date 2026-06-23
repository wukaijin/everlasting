#![cfg(test)]

use crate::agent::permissions::{PermissionAskPayload, Risk};

/// PermissionAskPayload wire shape: `path` is camelCase
/// (renamed from Rust `path`).
#[test]
fn permission_ask_payload_wire_shape_includes_path() {
    let p = PermissionAskPayload {
        rid: "test-rid".to_string(),
        session_id: "test-session".to_string(),
        tool_use_id: "tu-1".to_string(),
        tool_name: "read_file".to_string(),
        tool_input: serde_json::json!({"path": "/x"}),
        risk: Risk::High,
        reason: Some("test".to_string()),
        path: Some("/x".to_string()),
        worker_run_id: None,
    };
    let s = serde_json::to_string(&p).unwrap();
    // The wire field is `path` (camelCase == snake_case for a
    // single-word field).
    assert!(s.contains("\"path\":\"/x\""), "wire shape: {}", s);
    assert!(s.contains("\"toolName\":\"read_file\""), "camelCase: {}", s);
}

/// PermissionAskPayload: `path` is omitted when None
/// (`skip_serializing_if`).
#[test]
fn permission_ask_payload_omits_path_when_none() {
    let p = PermissionAskPayload {
        rid: "test-rid".to_string(),
        session_id: "test-session".to_string(),
        tool_use_id: "tu-2".to_string(),
        tool_name: "shell".to_string(),
        tool_input: serde_json::json!({"command": "ls"}),
        risk: Risk::High,
        reason: Some("test".to_string()),
        path: None,
        worker_run_id: None,
    };
    let s = serde_json::to_string(&p).unwrap();
    assert!(!s.contains("\"path\""), "path should be skipped: {}", s);
}

// =====================================================================
// 2.4 check (re-grill 2026-06-13): wire-shape guards for the
// "path field is populated ONLY for path tools" rule.
//
// The bug was that the previous `ask_path` implementation
// unconditionally set `path: Some(path_or_cmd.to_string())`
// in the payload, so shell / web_fetch payloads also got a
// `path` field on the wire. The frontend's `<PermissionModal>`
// `v-if="hasPath"` then rendered a misleading "path scope" row
// (with a "仓库外" badge) for shell commands and URLs.
//
// These tests lock the wire shape: shell and web_fetch must
// NOT include the `path` key in the serialized
// `PermissionAskPayload` (mimicking the `path_for_modal = None`
// pass-through in the new `ask_path` signature). Path tools
// MUST include it.
// =====================================================================

/// Wire shape: shell ask has NO `path` field (mimics
/// `ask_path(.., cmd, None, ..)` from the Tier 4 Shell branch).
/// The modal renders the command via `toolInput`, so a path
/// scope row would be wrong.
#[test]
fn permission_ask_payload_omits_path_for_shell() {
    let p = PermissionAskPayload {
        rid: "shell-rid".to_string(),
        session_id: "test-session".to_string(),
        tool_use_id: "tu-3".to_string(),
        tool_name: "shell".to_string(),
        tool_input: serde_json::json!({"command": "rm -rf /tmp/foo"}),
        risk: Risk::High,
        reason: Some(
            "The tool shell requires your confirmation (risk: 高, command: rm -rf /tmp/foo).".to_string(),
        ),
        // Mirrors the new `ask_path` body: `path_for_modal = None`
        // for shell, so the payload's `path` field is `None`.
        path: None,
        // Parent path ask (no worker context).
        worker_run_id: None,
    };
    let s = serde_json::to_string(&p).unwrap();
    assert!(
        !s.contains("\"path\""),
        "shell payload must NOT include a path field (would confuse PermissionModal): {}",
        s
    );
    // The other fields are still present and correctly camelCased.
    assert!(s.contains("\"toolName\":\"shell\""), "toolName: {}", s);
    assert!(s.contains("\"command\":\"rm -rf /tmp/foo\""), "toolInput echoed: {}", s);
    assert!(s.contains("\"reason\":"), "reason still present: {}", s);
}

/// Wire shape: web_fetch ask has NO `path` field (mimics
/// `ask_path(.., url, None, ..)` from the Tier 4 WebFetch
/// branch). The modal renders the URL via `toolInput`, so a
/// path scope row would be wrong (URL is not a filesystem
/// path, so the in-repo / out-of-repo badge would be
/// meaningless).
#[test]
fn permission_ask_payload_omits_path_for_web_fetch() {
    let p = PermissionAskPayload {
        rid: "webfetch-rid".to_string(),
        session_id: "test-session".to_string(),
        tool_use_id: "tu-4".to_string(),
        tool_name: "web_fetch".to_string(),
        tool_input: serde_json::json!({"url": "https://example.com/api"}),
        risk: Risk::Low,
        reason: Some(
            "The tool web_fetch requires your confirmation (risk: 低, URL: https://example.com/api).".to_string(),
        ),
        // Mirrors the new `ask_path` body: `path_for_modal = None`
        // for web_fetch, so the payload's `path` field is `None`.
        path: None,
        // Parent path ask (no worker context).
        worker_run_id: None,
    };
    let s = serde_json::to_string(&p).unwrap();
    assert!(
        !s.contains("\"path\""),
        "web_fetch payload must NOT include a path field (URL is not a path): {}",
        s
    );
    assert!(s.contains("\"toolName\":\"web_fetch\""), "toolName: {}", s);
    assert!(
        s.contains("\"url\":\"https://example.com/api\""),
        "toolInput echoed: {}",
        s
    );
}

/// Wire shape: path-tool ask DOES include the `path` field
/// (mimics `ask_path(.., &path_owned, Some(&path_owned), ..)`
/// from the Tier 4 Path branch — the only branch that
/// populates `path_for_modal`). The modal reads this to
/// render the in-repo / out-of-repo badge.
#[test]
fn permission_ask_payload_includes_path_for_path_tool() {
    let p = PermissionAskPayload {
        rid: "path-rid".to_string(),
        session_id: "test-session".to_string(),
        tool_use_id: "tu-5".to_string(),
        tool_name: "read_file".to_string(),
        tool_input: serde_json::json!({"path": "/Users/me/repo/src/foo.rs"}),
        risk: Risk::Low,
        reason: Some(
            "The tool read_file requires your confirmation (risk: 低, path: /Users/me/repo/src/foo.rs).".to_string(),
        ),
        // Mirrors the new `ask_path` body for path tools: the
        // `path_for_modal` argument is `Some(&path_owned)`, so the
        // payload's `path` field is `Some(...)`.
        path: Some("/Users/me/repo/src/foo.rs".to_string()),
        // Parent path ask (no worker context).
        worker_run_id: None,
    };
    let s = serde_json::to_string(&p).unwrap();
    assert!(
        s.contains("\"path\":\"/Users/me/repo/src/foo.rs\""),
        "path tool payload MUST include the path field (modal reads it for in-repo / out-of-repo badge): {}",
        s
    );
    assert!(s.contains("\"toolName\":\"read_file\""), "toolName: {}", s);
}

/// 2026-06-16 (inline approval card): payload carries `sessionId`
/// + `toolUseId` (camelCase) so the frontend routes the ask to the
/// right session and matches it to the right ToolCallCard.
#[test]
fn permission_ask_payload_carries_session_and_tool_use_id() {
    let p = PermissionAskPayload {
        rid: "r1".to_string(),
        session_id: "sess-42".to_string(),
        tool_use_id: "tooluse_abc".to_string(),
        tool_name: "write_file".to_string(),
        tool_input: serde_json::json!({"path": "/x"}),
        risk: Risk::Medium,
        reason: None,
        path: Some("/x".to_string()),
        worker_run_id: None,
    };
    let s = serde_json::to_string(&p).unwrap();
    assert!(s.contains("\"sessionId\":\"sess-42\""), "sessionId camelCase: {}", s);
    assert!(
        s.contains("\"toolUseId\":\"tooluse_abc\""),
        "toolUseId camelCase: {}",
        s
    );
}
