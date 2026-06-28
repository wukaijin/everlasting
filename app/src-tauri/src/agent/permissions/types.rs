//! Type definitions for the ⑨ 关 permission layer: `Risk`,
//! `Decision`, `PermissionContext`, `PermissionResponse`. Split
//! out of `mod.rs` on 2026-06-23.

use std::sync::Arc;

use serde::Serialize;

use crate::db::Mode;

use super::run_grant::RunGrantCache;

// ---------------------------------------------------------------------------
// Risk enum (serialized to IPC in `permission:ask` payload)
// ---------------------------------------------------------------------------

/// Risk level for a `(tool_name, tool_input)` pair. Per-tool
/// static map; see [`risk_for_tool`]. Serializes lowercase to
/// match the PRD's TypeScript type (`"low" | "medium" | "high"
/// | "critical"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
// `Critical` is reserved for the PermissionModal "极高" 风险
// variant (3px red border + shield-x icon per UX spec). The
// MVP's per-tool static map only ever returns Low / Medium /
// High, so the variant is never constructed in PR1. The dead-
// code allow is forward-compat — PR3 (PermissionModal) reads
// the wire payload and renders the critical styling on this
// variant.
#[allow(dead_code)]
pub enum Risk {
    Low,
    Medium,
    High,
    Critical,
}

impl Risk {
    // `as_str` is intentionally not used in PR1 — the IPC payload
    // gets the lowercase string from `#[serde(rename_all = ...)]`
    // on the enum itself. Kept as a method for any future caller
    // that wants the string without going through serde.
    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            Risk::Low => "low",
            Risk::Medium => "medium",
            Risk::High => "high",
            Risk::Critical => "critical",
        }
    }

    /// Chinese label for the PermissionModal UI (per audit §6.2
    /// — unify on 中文).
    #[allow(dead_code)]
    pub fn label_cn(&self) -> &'static str {
        match self {
            Risk::Low => "低",
            Risk::Medium => "中",
            Risk::High => "高",
            Risk::Critical => "极高",
        }
    }
}

/// Per-tool risk level. Static mapping — `shell` is always High,
/// `write_file` / `edit_file` are Medium, the read-only tools
/// (`read_file` / `grep` / `glob` / `list_dir` / `web_fetch`)
/// are Low. Reserved for memory-file-driven overrides in a
/// future PR.
pub fn risk_for_tool(tool_name: &str) -> Risk {
    match tool_name {
        "shell" | "run_background_shell" => Risk::High,
        // L3b PR3 (2026-06-27): merge_worker / discard_worker rewrite
        // the parent session's git branch — High risk, same tier as
        // shell. `classify_tool` routes these to `ToolKind::GitMutation`.
        "merge_worker" | "discard_worker" => Risk::High,
        "write_file" | "edit_file" => Risk::Medium,
        // `web_fetch` is Low at the risk-permission layer; its own
        // SSRF blocklist (in `tools/web_fetch.rs`) is the relevant
        // defense for network egress.
        _ => Risk::Low,
    }
}

// ---------------------------------------------------------------------------
// Decision enum (internal — NOT serialized)
// ---------------------------------------------------------------------------

/// ⑨ 关决策结果. 内部enum (不出 IPC — frontend 只看
/// `permission:ask` event 或 `tool_use → is_error: true`).
///
/// - `Allow`: 放行,Tier 6 写 `tool_allowed` 审计
/// - `Deny { reason, critical }`: 静默拒绝 (`is_error: true`),
/// 不弹窗 (Tier 2 路径 + Tier 3 user "拒绝"/超时)
/// - `Ask { reason, risk }`: 发 `permission:ask` event + 等
/// frontend `permission_response` (120s 超时 → 自动转 Deny)
///
/// The `Ask` variant is reserved for the PermissionModal
/// (PR3) — the intermediate state when `check()` has fired
/// the IPC but not yet received the user's response. In PR1
/// `check()` collapses `Ask` into `Allow` / `Deny` internally
/// before returning, so the variant is never observed by
/// the chat loop. Kept in the type so the PermissionModal
/// wire shape stays stable.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum Decision {
    Allow,
    Deny { reason: String, critical: bool },
    Ask { reason: String, risk: Risk },
}

impl Decision {
    #[allow(dead_code)]
    pub fn is_deny(&self) -> bool {
        matches!(self, Decision::Deny { .. })
    }
    #[allow(dead_code)]
    pub fn is_ask(&self) -> bool {
        matches!(self, Decision::Ask { .. })
    }
}

// ---------------------------------------------------------------------------
// PermissionContext — input to `check()`
// ---------------------------------------------------------------------------

/// Per-call context. Built once per agent-loop turn, passed
/// into each `check()` call. Cloned cheaply (a few strings +
/// a `PathBuf`).
///
/// **cwd** is the session's current working directory and is
/// the single containment anchor for the Tier 4 path-based
/// check (`is_within_root(ctx.cwd, path)` per re-grill PRD §1).
/// The project root is intentionally NOT plumbed here — the
/// permission layer's "inside the project?" question is
/// "inside the session's cwd?", which can be a subdir of the
/// project (e.g. when the user has navigated to
/// `~/repo/frontend`). The tool layer's
/// `assert_within_root(ctx.project_root, cwd)` is the source of
/// truth for the project boundary.
#[derive(Debug, Clone)]
pub struct PermissionContext {
    pub session_id: String,
    pub mode: Mode,
    pub cwd: std::path::PathBuf,
    /// B6 Subagent (2026-06-19, review #5): `true` when this context
    /// belongs to a worker agent dispatched via `dispatch_subagent`.
    /// Worker agents have no UI sink, so a Tier 4 `ask_path` /
    /// `ask_shell` decision must collapse to `Decision::Deny`
    /// (cannot surface a permission modal). Production chat sets
    /// `false`; the worker path sets `true`. The collapse is wired
    /// at the top of `ask_path`.
    pub is_worker: bool,
    /// 2026-06-22 (RULE-FrontSubagent-003 fix): when `is_worker`
    /// is `true`, this is the worker subagent's `subagent_runs.id`
    /// (DB row UUID, NOT the human-readable `worker_rid`). Used
    /// for two purposes:
    ///
    /// 1. **Permission store key** — the worker's oneshot lives
    ///    under `format!("worker:{}", worker_run_id)` so worker
    ///    asks do NOT pollute the parent's `permission_asks` map
    ///    (RULE-A-014 lineage: worker's ask must not race parent's
    ///    ask round-trip). The parent's `permission_response` IPC
    ///    handler currently keys by rid alone; worker path keeps
    ///    its rid unique by prefixing on the worker_run_id side
    ///    OR by including it in the rid — see `ask_path` worker
    ///    branch for the chosen approach.
    /// 2. **IPC payload field** — propagated to `PermissionAskPayload
    ///    .worker_run_id` so the frontend `<SubagentDrawer>` can
    ///    route the ask to the right row instead of opening a
    ///    global PermissionModal.
    ///
    /// `None` for production (parent) path — the field is left
    /// unused and the existing parent-modal UX is preserved.
    pub worker_run_id: Option<String>,
    /// 2026-06-26 (task `06-26-subagent-per-run-grant`): per-run
    /// in-memory grant cache for worker subagents. `Some(Arc<...>)`
    /// on the worker path (the `Arc` is constructed fresh in
    /// `run_subagent` per worker); `None` on the parent path
    /// (production chat + tests — never read, never written).
    ///
    /// When `Some`, `check.rs` Tier 4's three branches (Path /
    /// Shell / WebFetch) consult the cache before falling through
    /// to `ask_path`, and `ask_path`'s worker `AllowAlways` arm
    /// writes to it instead of dropping the grant. The cache dies
    /// with the worker's `run_chat_loop` invocation — it does NOT
    /// persist to `session_tool_permissions` (RULE-A-016 isolation:
    /// worker grants must not cross the privilege boundary into
    /// the parent session's grant table).
    ///
    /// See [`RunGrantCache`] for the match_kind semantics
    /// (tool / prefix / path — mirrors the DB table's three
    /// variants).
    pub run_grants: Option<Arc<RunGrantCache>>,
}

// ---------------------------------------------------------------------------
// PermissionResponse (user reply to a `permission:ask` event)
// ---------------------------------------------------------------------------

/// The user's response to a `permission:ask` event. Sent over
/// the oneshot channel from the Tauri command `permission_response`
/// (in [`crate::commands::permissions`]) to the awaiting agent
/// loop's `check()` future.
#[derive(Debug, Clone)]
pub enum PermissionResponse {
    AllowOnce,
    AllowAlways,
    /// `reason` is the user's optional feedback text (the
    /// "拒绝并说明" path). Empty string = plain deny. The agent
    /// loop surfaces this as the `tool_result(is_error)` content
    /// so the LLM learns *why* it was denied.
    Deny { reason: String },
}
