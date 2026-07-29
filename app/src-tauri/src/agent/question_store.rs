//! ⑨-b QuestionStore — in-flight `ask_user_question` + `request_mode_change`
//! oneshot registry, parallel to the A2+B7 `PermissionStore`. Split
//! out from the agent-layer god-module on 2026-06-30 as part of the
//! `ask_user_question` tool task; extended on 2026-07-07 (task
//! `07-07-request-mode-change-tool`) to host the `request_mode_change`
//! interaction under the same store + single-pending gate (a session
//! can't have both a pending question and a pending mode change
//! at the same time — both are interactive user-blocked interactions).
//!
//! ## Why a parallel store, not a PermissionStore extension?
//!
//! `PermissionStore` is a security gate; question flow is a UX
//! gate (see design §2.2 / PRD §"QuestionStore 访问边界"). They
//! share the structural shape (`Arc<Mutex<HashMap<session_id,
//! Pending*>>>` + `register` / `resolve` / IPC channel) but the
//! cancellation semantics differ by intent:
//!
//! - PermissionStore: switch session → cancel all pending asks
//!   (`cancel_session_asks`); close modal → oneshot dropped →
//!   `Deny` short-circuits the LLM loop.
//! - QuestionStore: switch session → keep oneshot alive; user
//!   must explicitly cancel or answer (`R9`-`R11`); close card →
//!   no equivalent (inline card, no modal close).
//!
//! Sharing the store type would entangle these semantics. The
//! parallel store is intentional; the implementation mirrors
//! `permissions/store.rs` 1:1 for parity (same field shape,
//! same `tokio::select!` consumption pattern in `tools/ask_user_
//! question.rs`).
//!
//! No new IPC channel here — that lives in `state.rs::ChatEventSink
//! ::emit_tool_question` (the `tool:question` Tauri event) and in
//! `commands/question.rs` (the `resolve_tool_question` +
//! `get_pending_question` commands). This module owns the
//! in-process oneshot map only.
//!
//! ## Pending state survives session switches
//!
//! `QuestionStore` deliberately does NOT cancel on session
//! switch (design §8): the agent loop's `tokio::select!` keeps
//! waiting on the oneshot until resolve / app crash. The
//! frontend session-switch path uses `get_pending_question` to
//! recover the live payload (so a switched-back session can
//! render the still-pending card). The user-facing `取消` button
//! resolves with `InteractionResponse::Cancelled`; the session
//! cancel token (user Stop / app shutdown) is handled by the
//! cancel arm dropping the receiver (`Err(RecvError)` →
//! `cancelled_by_session` tool_result). Both are explicit
//! user/chrome signals, never implicit session-switch.
//!
//! ## 2026-07-07 extension: PendingInteraction enum
//!
//! The same single-pending-per-session gate now covers the
//! `ask_user_question` tool, the `request_mode_change` tool,
//! and the `request_task_state_transition` tool (Phase 3 Step 3.1).
//! All three are user-interaction-gated, all need the
//! "switch-session keeps oneshot alive" semantic, and all want
//! the same frontend "inline card with allow/deny" UX. We
//! unify them under `PendingInteraction` (a tagged enum) so the
//! store keys on `session_id` once and the IPC
//! `get_pending_interaction` command returns a
//! `PendingInteractionEntry` (kind + payload) the frontend can
//! dispatch on. The wire-side split stays snake_case on both
//! sides (same shared-struct exemption as `ToolQuestionPayload`).
//! Mutation surface is `ask_user_question::execute_blocking`,
//! `request_mode_change::execute_blocking`, and (Step 3.1)
//! `request_task_state_transition::execute_blocking`; all three
//! build a `PendingInteraction::Question(...)`, `ModeChange(...)`,
//! or `TaskStateTransition(...)` variant before calling `register`.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, Mutex};

/// IPC wire shape — the question payload the frontend renders
/// into `<AskUserQuestionCard>`. Mirrors design §4.1 (the
/// `tool:question` event body) and PRD §R3 wire spec.
///
/// # Why snake_case (IPC `camelCase` rule exemption)
///
/// `database-guidelines.md` mandates `#[serde(rename_all =
/// "camelCase")]` on structs crossing the IPC boundary. This
/// struct (and `Question` / `QuestionOption` below) are
/// **exempt**: the same `Question` type is shared with
/// `tools::ask_user_question::AskUserQuestionInput`, which
/// deserializes the LLM's tool-use JSON. The LLM schema (see
/// `ask_user_question::definition()`'s `input_schema`) is
/// snake_case — it mirrors Claude Code's trained
/// `AskUserQuestion` schema for zero learning cost. Renaming
/// the shared `Question` to camelCase would break LLM input
/// parsing. So the entire emit chain stays snake_case on both
/// sides of the IPC (backend Serialize snake → frontend reads
/// `payload.session_id` snake). The exemption is recorded in
/// `database-guidelines.md` near the catalog checklist.
///
/// `ts` is unix-ms timestamp from the backend — lets the
/// frontend distinguish "this question is from before my
/// session switch" from "fresh question during my session"
/// without server-side ordering (the oneshot map is
/// single-entry-per-session, so ordering within a session is
/// trivially the most-recent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolQuestionPayload {
    pub session_id: String,
    pub tool_use_id: String,
    pub questions: Vec<Question>,
    /// Unix epoch ms (backend authoritative). Lets the frontend
    /// display "asked 3s ago" without re-deriving from the
    /// `ChatEvent` log.
    pub ts: i64,
}

/// One question section inside `<AskUserQuestionCard>` (v1
/// single-card multi-section, design §5.5 / PRD §R8). The
/// `Option` field uses `skip_serializing_if` so the optional
/// fields round-trip cleanly across the IPC boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub options: Vec<QuestionOption>,
    /// Always emitted (the wire default for `Option<bool>` in
    /// serde is `None` but the LLM contract requires the field
    /// to default to `false`; we serialize `None` as `false`).
    #[serde(default)]
    pub multi_select: bool,
    /// If true, the frontend renders a free-text input so the
    /// user can type their own answer instead of picking an
    /// option. Selecting an option and typing are mutually
    /// exclusive: when the user types, the answer's `options`
    /// is empty and `custom` carries the text (see
    /// `QuestionAnswer::custom`). Defaults to `false` so pre-
    /// custom-field task.json / payloads round-trip unchanged.
    /// Mirrors `multi_select`'s serde shape (`#[serde(default)]`,
    /// always emitted) for symmetry.
    #[serde(default)]
    pub allow_custom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// User's answer selection — one entry per `Question` in the
/// original `ToolQuestionPayload.questions` (the 1:1 pairing is
/// enforced by the frontend's "submit" handler; the backend
/// trusts the shape). `options` is the labels the user picked
/// (1 element for radio, N for multi_select). `multi_select`
/// is echoed back so the LLM can disambiguate without
/// re-deriving from the original schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnswer {
    pub question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    pub options: Vec<String>,
    pub multi_select: bool,
    /// Free-text answer the user typed via the `allow_custom`
    /// input. Mutually exclusive with `options`: when present,
    /// `options` is empty (`[]`) and vice versa. `Option` +
    /// `skip_serializing_if` so a non-custom answer omits the
    /// field (matches `header`'s round-trip shape) and legacy
    /// answers without it deserialize as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// IPC wire shape — the `request_mode_change` payload the frontend
/// renders into `<RequestModeChangeCard>`.
///
/// # Why snake_case (IPC `camelCase` rule exemption)
///
/// Same exemption as `ToolQuestionPayload`: the same kind is shared
/// with `tools::request_mode_change::RequestModeChangeInput` (LLM
/// tool input). The LLM schema (see `request_mode_change
/// ::definition()`'s `input_schema`) is snake_case to match Claude
/// Code's trained `request_mode_change` semantics. The entire emit
/// chain stays snake_case on both sides of the IPC.
///
/// `current_mode` is the session's `mode` at the time the LLM
/// invoked the tool (read off `loaded_session.session.mode` at the
/// blocking tool's entry). The frontend uses it to color the
/// "current mode → target mode" comparison pill.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeChangePayload {
    pub session_id: String,
    pub tool_use_id: String,
    /// "edit" | "plan" | "yolo" (validated by the tool; the wire
    /// enum is the source of truth — see
    /// `request_mode_change::definition()`).
    pub target_mode: String,
    /// Session's current mode at the time the tool was invoked.
    /// `None` is reserved for future "session pre-load not yet
    /// resolved" edge cases; production always populates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_mode: Option<String>,
    /// LLM-supplied explanation (≤500 chars). Optional; the card
    /// renders it as a sub-title under the mode name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Unix epoch ms (backend authoritative). Lets the frontend
    /// display "asked 3s ago" without re-deriving from the
    /// `ChatEvent` log.
    pub ts: i64,
}

/// IPC wire shape — the `request_task_state_transition` payload
/// the frontend renders into `<RequestTaskStateTransitionCard>`.
///
/// # Why snake_case (IPC `camelCase` rule exemption)
///
/// Same exemption as `ToolQuestionPayload` + `ModeChangePayload`:
/// the same kind is shared with
/// `tools::request_task_state_transition::RequestTaskStateTransitionInput`
/// (LLM tool input). The LLM schema (see
/// `request_task_state_transition::definition()`'s `input_schema`)
/// is snake_case to match the dev `WorkflowDef` state names
/// (`planning` / `implement` / `check` / `done`). The entire emit
/// chain stays snake_case on both sides of the IPC.
///
/// `current_state` is the task's `status` at the time the LLM
/// invoked the tool (read off `workflow_ctx.current_task.status`
/// at the blocking tool's entry). The frontend uses it to color
/// the "current state → target state" comparison pill (same UX
/// pattern as `<RequestModeChangeCard>`).
///
/// `slug` is the task identifier so the IPC handler
/// (`commands::question::resolve_task_state_transition`) can
/// locate `<project>/.everlasting/tasks/<slug>/task.json`
/// without an additional `WorkflowCtx` reconstruction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStateTransitionPayload {
    pub session_id: String,
    pub tool_use_id: String,
    /// Target state name — must be one of `planning` /
    /// `implement` / `check` / `done` (validated by the tool
    /// via `agent::workflow::parse_target_state`; the IPC
    /// re-validates on resolve). The wire enum mirrors
    /// `WorkflowDef::states` so a plugin with custom states
    /// would surface `to_string()` of each state — Step 3.1
    /// only wires the dev `dev` plugin's 4-state set.
    pub target_state: String,
    /// Task's current status at the time the tool was invoked.
    /// `None` is reserved for future "no current task" edge
    /// cases; production always populates a workflow session
    /// with a known current task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_state: Option<String>,
    /// Task slug so the IPC handler can locate the on-disk
    /// `task.json` to mutate (the IPC layer doesn't carry the
    /// agent loop's `WorkflowCtx`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// LLM-supplied explanation (≤500 chars). Optional; the
    /// card renders it as a sub-title under the target state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Unix epoch ms (backend authoritative). Lets the frontend
    /// display "asked 3s ago" without re-deriving from the
    /// `ChatEvent` log.
    pub ts: i64,
}

/// The interaction kinds the store gates. The `kind` tag drives
/// both the IPC dispatch (the frontend's `<AskUserQuestionCard>` /
/// `<RequestModeChangeCard>` / `<RequestTaskStateTransitionCard>`
/// / floating loop-intervention card) AND the audit kind written
/// by the resolve path. New kinds require adding a new
/// `PendingInteraction` variant + a new `as_str()` arm + updating
/// the frontend's `PendingInteraction` discriminated union.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionKind {
    Question,
    ModeChange,
    TaskStateTransition,
    /// C2+ active loop-intervention (chat_loop's ≥3 consecutive
    /// loop-detection hits). Distinct from `Question` because it is
    /// NOT driven by an `ask_user_question` tool_use block — the
    /// frontend renders it as a floating card (no tool_use anchor).
    LoopIntervention,
}

impl InteractionKind {
    // Suppress `dead_code` on non-test builds: `as_str` is only
    // referenced by `commands::question::tests::interaction_kind_round_trip`
    // (the wire-shape round-trip sanity check). `cargo check`
    // doesn't compile tests, so without this attribute the method
    // is flagged as unused; in test builds the attribute is a
    // no-op.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::ModeChange => "mode_change",
            Self::TaskStateTransition => "task_state_transition",
            Self::LoopIntervention => "loop_intervention",
        }
    }
}

/// The store's gated entry shape. The `tag` is `"kind"`, the
/// `content` carries the typed payload (snake_case on both
/// sides — same shared-struct exemption as the question payload).
/// The frontend dispatches on the outer `kind` to pick the right
/// card component.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PendingInteraction {
    Question(ToolQuestionPayload),
    ModeChange(ModeChangePayload),
    TaskStateTransition(TaskStateTransitionPayload),
    /// C2+ loop-intervention. Reuses `ToolQuestionPayload` (the
    /// question/options shape is exactly what the intervention card
    /// needs: "终止 loop" vs "继续"); distinguished from `Question`
    /// so the frontend can render it as a floating card instead of
    /// anchoring it under a (non-existent) `ask_user_question`
    /// tool_use block. See the 2026-07-28 incident (session
    /// e8a1ad96…) where the prior `Question` tagging made the card
    /// unrenderable.
    LoopIntervention(ToolQuestionPayload),
}

impl PendingInteraction {
    pub fn kind(&self) -> InteractionKind {
        match self {
            Self::Question(_) => InteractionKind::Question,
            Self::ModeChange(_) => InteractionKind::ModeChange,
            Self::TaskStateTransition(_) => InteractionKind::TaskStateTransition,
            Self::LoopIntervention(_) => InteractionKind::LoopIntervention,
        }
    }
}

/// IPC surface returned by `get_pending_interaction` — the
/// `kind` is duplicated at the top level (so the frontend
/// `getPendingInteraction` IPC caller can do
/// `entry.kind === "mode_change"` without first parsing the
/// tagged enum). The `payload` carries the typed variant
/// (frontend code dispatches on `kind` and reads the matching
/// fields from `payload`). The wrapper is also `Clone`-able so
/// the test can copy a snapshot for assertions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingInteractionEntry {
    pub kind: InteractionKind,
    pub payload: PendingInteraction,
}

/// Internal — what the oneshot delivers on resolve. Unified
/// across the two pending kinds (one channel, two callers).
/// The `Answered` variant carries a generic JSON value so
/// question answers (`Vec<QuestionAnswer>`) and mode-change
/// accepts (`true` / `false`) both fit without per-kind
/// variants. The `Cancelled` variant covers both "user clicked
/// 跳过" (question) and "user clicked 拒绝" (mode change).
///
/// `execute_blocking` is responsible for the per-kind → wire
/// shape mapping (e.g. serialize a `Vec<QuestionAnswer>` into
/// the `Answered` JSON for the question case; pass `serde_json
/// ::Value::Bool(true)` for mode-change allow).
#[derive(Debug, Clone)]
pub enum InteractionResponse {
    /// User accepted the prompt. The `serde_json::Value` is
    /// either a JSON-serialized `Vec<QuestionAnswer>` (for
    /// `PendingInteraction::Question`) or a simple `true`
    /// boolean (for `PendingInteraction::ModeChange`). The
    /// tool layer builds the per-kind wire shape.
    Answered(serde_json::Value),
    /// User rejected / cancelled the prompt. tool_result is
    /// `{"cancelled": true}` (question) or
    /// `{"cancelled_by_user": true}` (mode change).
    Cancelled,
}

/// One pending `ask_user_question` OR `request_mode_change`. The
/// `oneshot` is `Option` because `resolve` clears it (so a
/// second resolve is a no-op rather than a panic). The `payload`
/// stays so `get_pending_interaction` can return it for
/// session-switch recovery (frontend re-injects the card on the
/// switched-back session).
///
/// 2026-07-07 (request_mode_change): the kind of the payload
/// determines which card component the frontend renders
/// (`AskUserQuestionCard` for Question / `RequestModeChangeCard`
/// for ModeChange). The `kind` field is duplicated at the top
/// level so the IPC consumer can do
/// `entry.kind === "mode_change"` without first parsing the
/// tagged enum.
#[allow(dead_code)]
pub struct PendingQuestion {
    pub tool_use_id: String,
    pub session_id: String,
    pub ts: i64,
    pub oneshot: Option<oneshot::Sender<InteractionResponse>>,
    pub kind: InteractionKind,
    pub payload: PendingInteraction,
}

/// In-flight `ask_user_question` registry. Wrapped in
/// `Arc<Mutex<…>>` to match the `PermissionStore` parity so
/// `AppState` carries it identically (clone-cheap, lock on
/// mutation, snapshot on read).
#[derive(Clone)]
pub struct QuestionStore {
    inner: Arc<Mutex<HashMap<String, PendingQuestion>>>,
}

impl QuestionStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a new pending interaction (Question or
    /// ModeChange) for `session_id`. Returns
    /// `Err(AlreadyPending)` if an interaction for this
    /// session already exists — the agent loop surfaces that
    /// as a structured `tool_result(is_error: true)` so the
    /// LLM understands it's a concurrency gate (and naturally
    /// serializes on the next turn).
    ///
    /// The returned `oneshot::Receiver` is held by `execute_blocking`
    /// inside `tokio::select!{cancel, oneshot}` (mirrors the
    /// `permission_asks` permission-store consumption pattern).
    ///
    /// 2026-07-07: the `payload: PendingInteraction` parameter
    /// is the unified shape — both `ask_user_question` and
    /// `request_mode_change` wrap their respective payloads in
    /// the matching enum variant. The single-pending gate
    /// applies across BOTH kinds (a pending question blocks a
    /// pending mode change, and vice versa — only one
    /// interactive UI element can be in flight per session).
    pub async fn register(
        &self,
        session_id: &str,
        tool_use_id: &str,
        payload: PendingInteraction,
    ) -> Result<oneshot::Receiver<InteractionResponse>, QuestionStoreError> {
        let mut map = self.inner.lock().await;
        if map.contains_key(session_id) {
            return Err(QuestionStoreError::AlreadyPending);
        }
        let (tx, rx) = oneshot::channel();
        let kind = payload.kind();
        let ts = match &payload {
            PendingInteraction::Question(p) => p.ts,
            PendingInteraction::ModeChange(p) => p.ts,
            PendingInteraction::TaskStateTransition(p) => p.ts,
            PendingInteraction::LoopIntervention(p) => p.ts,
        };
        map.insert(
            session_id.to_string(),
            PendingQuestion {
                tool_use_id: tool_use_id.to_string(),
                session_id: session_id.to_string(),
                ts,
                kind,
                oneshot: Some(tx),
                payload,
            },
        );
        Ok(rx)
    }

    /// Resolve a pending interaction. Called by the
    /// `resolve_tool_question` / `resolve_mode_change` Tauri
    /// commands (frontend invokes on submit / 拒绝). Returns
    /// the `PendingInteractionEntry` that was just resolved so
    /// the caller (the IPC handler) can write the
    /// per-kind `mode_change_*` audit row. The
    /// `entry.kind` field tells the caller which path to take.
    ///
    /// Returns `NotFound` if the session has no pending
    /// interaction — covers both "never registered" and the
    /// already-resolved race (resolve removes the entry
    /// atomically with taking the oneshot, so a double-resolve
    /// finds no key and returns `NotFound`).
    pub async fn resolve(
        &self,
        session_id: &str,
        response: InteractionResponse,
    ) -> Result<PendingInteractionEntry, QuestionStoreError> {
        let mut map = self.inner.lock().await;
        let pending = map
            .get_mut(session_id)
            .ok_or(QuestionStoreError::NotFound)?;
        // Invariant: while an entry exists its `oneshot` is
        // `Some` — `register` inserts `Some(tx)` and the only
        // `take()` site is this function, which `remove`s the
        // entry in the same critical section. A second resolve
        // therefore never sees an entry (it returns `NotFound`
        // above). The `expect` pins that invariant; a panic
        // here would mean store-internal corruption.
        let tx = pending
            .oneshot
            .take()
            .expect("oneshot present while entry exists");
        let kind = pending.kind;
        let entry = PendingInteractionEntry {
            kind,
            payload: std::mem::replace(
                &mut pending.payload,
                // A sentinel placeholder — the entry is removed
                // immediately below so the placeholder is
                // unreachable. Wrapped in `Question` to satisfy
                // the enum's `Question` variant being the
                // zero-cost default; the real payload was
                // captured into the `entry` above.
                PendingInteraction::Question(ToolQuestionPayload {
                    session_id: String::new(),
                    tool_use_id: String::new(),
                    questions: Vec::new(),
                    ts: 0,
                }),
            ),
        };
        // Take the entry out — the interaction is no longer
        // pending. If the sender fails (receiver already
        // dropped because the cancel arm selected), the
        // InteractionResponse is silently consumed; no audit /
        // no-op needed (the cancel path produced its own
        // tool_result already).
        map.remove(session_id);
        let _ = tx.send(response);
        Ok(entry)
    }

    /// Remove a pending interaction without sending through
    /// the oneshot. Used by the cancel arm in
    /// `execute_blocking`'s `tokio::select!` — the sender is
    /// dropped here (it lives inside the removed
    /// `PendingQuestion`), which makes the awaiting
    /// `oneshot::Receiver` return `Err(RecvError)`;
    /// `execute_blocking` maps that to the
    /// `cancelled_by_session` tool_result.
    pub async fn remove(&self, session_id: &str) -> Option<PendingQuestion> {
        let mut map = self.inner.lock().await;
        map.remove(session_id)
    }

    /// Read-only snapshot for `get_pending_interaction`
    /// (frontend session-switch recovery). Returns
    /// `Option<PendingInteractionEntry>` — `None` if no
    /// pending interaction, `Some` with the typed
    /// `kind` + payload otherwise. The frontend's
    /// `getPendingInteraction` IPC binding is the source of
    /// truth on session switch (the Pinia cache is corrected
    /// to match).
    pub async fn get_payload(&self, session_id: &str) -> Option<PendingInteractionEntry> {
        let map = self.inner.lock().await;
        map.get(session_id).map(|p| PendingInteractionEntry {
            kind: p.kind,
            payload: p.payload.clone(),
        })
    }

    /// Back-compat shim: returns the `ToolQuestionPayload` for
    /// a pending question OR `None` for anything else
    /// (including pending mode changes). Used by the legacy
    /// `get_pending_question` IPC command (kept for backward
    /// compatibility; new code should use
    /// `get_pending_interaction` + the
    /// `PendingInteractionEntry` shape).
    pub async fn get_question_payload(&self, session_id: &str) -> Option<ToolQuestionPayload> {
        let map = self.inner.lock().await;
        match map.get(session_id) {
            Some(p) => match &p.payload {
                PendingInteraction::Question(qp) => Some(qp.clone()),
                _ => None,
            },
            None => None,
        }
    }

    /// List all pending question sessions (test-only
    /// observation; production doesn't expose this). The
    /// session-id list, not the payloads.
    #[cfg(test)]
    #[allow(dead_code)]
    pub async fn list_sessions(&self) -> Vec<String> {
        let map = self.inner.lock().await;
        map.keys().cloned().collect()
    }
}

impl Default for QuestionStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors for `QuestionStore` operations. Two variants:
/// `AlreadyPending` is the single-pending concurrency gate
/// (design §6.1); `NotFound` covers both "never registered" and
/// the already-resolved race (resolve removes the entry in the
/// same critical section that takes the oneshot, so a second
/// resolve finds no key). There is intentionally no
/// `AlreadyResolved` variant — the entry-removal + oneshot-take
/// happen under one lock, so the "double resolve" state is
/// unreachable. The `PermissionStore` parity is structural
/// (same `Arc<Mutex<HashMap>>` shape), not variant-for-variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionStoreError {
    /// `register` called for a session that already has a
    /// pending question (single-pending invariant, design
    /// §6.1). The LLM-facing tool_result is a structured
    /// "已有 pending question,等当前回答完成" message.
    AlreadyPending,
    /// `resolve` called for a session with no pending entry.
    /// Covers both "never registered" and the already-resolved
    /// case (resolve removes the entry atomically with the
    /// oneshot take, so a double-resolve finds no key). The
    /// frontend treats this as a no-op.
    NotFound,
}

impl std::fmt::Display for QuestionStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyPending => write!(f, "a question is already pending for this session"),
            Self::NotFound => write!(f, "no pending question for this session"),
        }
    }
}

impl std::error::Error for QuestionStoreError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_payload(session_id: &str, tool_use_id: &str) -> ToolQuestionPayload {
        ToolQuestionPayload {
            session_id: session_id.to_string(),
            tool_use_id: tool_use_id.to_string(),
            questions: vec![Question {
                question: "Pick one".into(),
                header: None,
                options: vec![
                    QuestionOption {
                        label: "A".into(),
                        description: None,
                        preview: None,
                    },
                    QuestionOption {
                        label: "B".into(),
                        description: None,
                        preview: None,
                    },
                ],
                multi_select: false,
                allow_custom: false,
            }],
            ts: 1_700_000_000_000,
        }
    }

    fn make_mode_change_payload(
        session_id: &str,
        tool_use_id: &str,
        target_mode: &str,
    ) -> ModeChangePayload {
        ModeChangePayload {
            session_id: session_id.to_string(),
            tool_use_id: tool_use_id.to_string(),
            target_mode: target_mode.to_string(),
            current_mode: Some("plan".to_string()),
            reason: Some("need to write code".to_string()),
            ts: 1_700_000_000_001,
        }
    }

    /// Happy path: register returns a receiver, resolve with
    /// `Answered` makes the receiver yield the answers. The
    /// entry is cleared on resolve (subsequent get_payload
    /// is None).
    #[tokio::test]
    async fn register_then_resolve_answered() {
        let store = QuestionStore::new();
        let payload = make_payload("s1", "tu_1");
        let rx = store
            .register("s1", "tu_1", PendingInteraction::Question(payload.clone()))
            .await
            .expect("register ok");
        // get_payload returns it BEFORE resolve.
        let entry = store
            .get_payload("s1")
            .await
            .expect("entry present before resolve");
        assert_eq!(entry.kind, InteractionKind::Question);
        match entry.payload {
            PendingInteraction::Question(p) => {
                assert_eq!(p.tool_use_id, "tu_1");
            }
            _ => panic!("expected Question payload"),
        }
        let answers = vec![QuestionAnswer {
            question: "Pick one".into(),
            header: None,
            options: vec!["A".into()],
            multi_select: false,
            custom: None,
        }];
        let entry = store
            .resolve(
                "s1",
                InteractionResponse::Answered(serde_json::to_value(&answers).unwrap()),
            )
            .await
            .expect("resolve ok");
        // The returned entry identifies what was resolved.
        assert_eq!(entry.kind, InteractionKind::Question);
        // Receiver fires.
        let got = rx.await.expect("receiver ok");
        match got {
            InteractionResponse::Answered(v) => {
                let parsed: Vec<QuestionAnswer> =
                    serde_json::from_value(v).expect("payload is Vec<QuestionAnswer>");
                assert_eq!(parsed, answers);
            }
            other => panic!("expected Answered, got {:?}", other),
        }
        // Entry cleared.
        assert!(store.get_payload("s1").await.is_none());
    }

    /// Concurrency gate: second register for the same session
    /// returns `AlreadyPending`. The first register stays
    /// usable (not overwritten).
    #[tokio::test]
    async fn register_already_pending() {
        let store = QuestionStore::new();
        let p1 = make_payload("s1", "tu_1");
        let _rx1 = store
            .register("s1", "tu_1", PendingInteraction::Question(p1))
            .await
            .expect("first register ok");
        let p2 = make_payload("s1", "tu_2");
        let err = store
            .register("s1", "tu_2", PendingInteraction::Question(p2))
            .await
            .expect_err("second register errors");
        assert_eq!(err, QuestionStoreError::AlreadyPending);
        // First entry still present.
        let got = store
            .get_payload("s1")
            .await
            .expect("first entry still present");
        assert_eq!(got.kind, InteractionKind::Question);
        match got.payload {
            PendingInteraction::Question(p) => assert_eq!(p.tool_use_id, "tu_1"),
            _ => panic!("expected Question payload"),
        }
    }

    /// Cross-kind gate: a pending question blocks a pending
    /// mode change (and vice versa). Both are "interactive
    /// user-blocked" — only one can be in flight per session
    /// (PRD §"Pending 互斥" / design §3.3).
    #[tokio::test]
    async fn register_question_then_mode_change_pending_returns_already_pending() {
        let store = QuestionStore::new();
        let _rx1 = store
            .register(
                "s1",
                "tu_1",
                PendingInteraction::Question(make_payload("s1", "tu_1")),
            )
            .await
            .expect("first register ok");
        let err = store
            .register(
                "s1",
                "tu_2",
                PendingInteraction::ModeChange(make_mode_change_payload("s1", "tu_2", "edit")),
            )
            .await
            .expect_err("mode change blocked by existing question");
        assert_eq!(err, QuestionStoreError::AlreadyPending);
        // The pending question is untouched.
        let got = store
            .get_payload("s1")
            .await
            .expect("question still present");
        assert_eq!(got.kind, InteractionKind::Question);
    }

    /// ModeChange resolve returns the typed entry so the
    /// caller can branch on the kind.
    #[tokio::test]
    async fn resolve_mode_change_returns_entry_kind_mode_change() {
        let store = QuestionStore::new();
        let payload = make_mode_change_payload("s1", "tu_mc", "edit");
        let rx = store
            .register(
                "s1",
                "tu_mc",
                PendingInteraction::ModeChange(payload.clone()),
            )
            .await
            .expect("register ok");
        let entry = store
            .resolve("s1", InteractionResponse::Answered(serde_json::json!(true)))
            .await
            .expect("resolve ok");
        assert_eq!(entry.kind, InteractionKind::ModeChange);
        match entry.payload {
            PendingInteraction::ModeChange(p) => {
                assert_eq!(p.target_mode, "edit");
                assert_eq!(p.tool_use_id, "tu_mc");
            }
            _ => panic!("expected ModeChange payload"),
        }
        // Receiver fires with the JSON value.
        let got = rx.await.expect("receiver ok");
        match got {
            InteractionResponse::Answered(v) => {
                assert_eq!(v, serde_json::json!(true));
            }
            other => panic!("expected Answered, got {:?}", other),
        }
        // Entry cleared.
        assert!(store.get_payload("s1").await.is_none());
    }

    /// resolve on an unknown session returns `NotFound`.
    #[tokio::test]
    async fn resolve_not_found() {
        let store = QuestionStore::new();
        let err = store
            .resolve("unknown", InteractionResponse::Cancelled)
            .await
            .expect_err("resolve unknown errors");
        assert_eq!(err, QuestionStoreError::NotFound);
    }

    /// resolve twice on the same session returns `NotFound` on
    /// the second call — `resolve` removes the entry atomically
    /// with taking the oneshot, so there is no separate
    /// `AlreadyResolved` state; the second call simply finds no
    /// key.
    #[tokio::test]
    async fn resolve_twice_second_call_not_found() {
        let store = QuestionStore::new();
        store
            .register(
                "s1",
                "tu_1",
                PendingInteraction::Question(make_payload("s1", "tu_1")),
            )
            .await
            .expect("register ok");
        store
            .resolve("s1", InteractionResponse::Cancelled)
            .await
            .expect("first resolve ok");
        let err = store
            .resolve("s1", InteractionResponse::Cancelled)
            .await
            .expect_err("second resolve errors");
        assert_eq!(err, QuestionStoreError::NotFound);
    }

    /// `remove` clears the entry and yields the PendingQuestion
    /// (for the cancel arm's bookkeeping — the oneshot is
    /// dropped here, which the receiver sees as
    /// `Err(RecvError)`).
    #[tokio::test]
    async fn remove_clears_entry() {
        let store = QuestionStore::new();
        store
            .register(
                "s1",
                "tu_1",
                PendingInteraction::Question(make_payload("s1", "tu_1")),
            )
            .await
            .expect("register ok");
        let pending = store.remove("s1").await.expect("remove returns pending");
        assert_eq!(pending.session_id, "s1");
        assert_eq!(pending.tool_use_id, "tu_1");
        assert!(store.get_payload("s1").await.is_none());
    }
}
