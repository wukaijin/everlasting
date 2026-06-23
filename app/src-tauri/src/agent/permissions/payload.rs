//! Wire payload for the `permission:ask` IPC event. Split out
//! of `mod.rs` on 2026-06-23.

use serde::Serialize;

use super::types::Risk;

/// Wire payload for the `permission:ask` event. The frontend's
/// `usePermissionsStore.setPending` reads this and mounts the
/// `<PermissionModal>`. The modal's 3 buttons each invoke
/// `permission_response(rid, decision)` which resolves the
/// pending oneshot in [`super::store::PermissionStore`].
///
/// `path` is filled in for path-tools (read_file / write_file /
/// edit_file / list_dir / grep / glob) so the PermissionModal
/// can show a "path scope" row in the header (per re-grill
/// Q10 "保留 risk 字段作 UI 视觉,加 path 范围行"). It is
/// omitted for shell / web_fetch (the modal renders the
/// command / URL inline via `toolInput` instead).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionAskPayload {
    pub rid: String,
    /// Session this ask belongs to (per-session approval routing,
    /// 2026-06-16). The frontend keys pending asks by `sessionId`
    /// so multi-session concurrency no longer collides on the
    /// single-slot `pendingPermission`.
    pub session_id: String,
    /// The `tool_use_id` of the tool_use that triggered this ask.
    /// The frontend matches `ToolCallInfo.id === toolUseId` to
    /// render the inline approval state on the right ToolCallCard.
    pub tool_use_id: String,
    pub tool_name: String,
    pub tool_input: serde_json::Value,
    pub risk: Risk,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// 2026-06-22 (RULE-FrontSubagent-003 fix): populated by the
    /// worker path (subagent dispatch) so the frontend can route
    /// the ask to the corresponding `<SubagentDrawer>` row instead
    /// of the parent session's `<PermissionModal>`. The wire shape
    /// is `workerRunId: string` (camelCase); the field is absent
    /// for parent-path asks (the existing parent modal still owns
    /// those). `skip_serializing_if = Option::is_none` keeps the
    /// field OFF the wire for the parent path, so frontend code
    /// that destructures the payload only needs to check
    /// `payload.workerRunId !== undefined` rather than splitting
    /// on `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_run_id: Option<String>,
}
