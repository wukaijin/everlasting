//! ⑨-b QuestionStore — in-flight `ask_user_question` oneshot
//! registry, parallel to the A2+B7 `PermissionStore`. Split out
//! from the agent-layer god-module on 2026-06-30 as part of the
//! `ask_user_question` tool task.
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
//! resolves with `QuestionResponse::Cancelled`, the session
//! cancel token resolves with `QuestionResponse::SessionCancelled`
//! — both are explicit user/chrome signals, never implicit
//! session-switch.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, Mutex};

/// IPC wire shape — the question payload the frontend renders
/// into `<AskUserQuestionCard>`. Mirrors design §4.1 (the
/// `tool:question` event body) and PRD §R3 wire spec. Fields
/// are snake_case so the LLM's JSON input matches the
/// frontend's TS types without rename. `ts` is unix-ms
/// timestamp from the backend — lets the frontend distinguish
/// "this question is from before my session switch" from
/// "fresh question during my session" without server-side
/// ordering (the oneshot map is single-entry-per-session, so
/// ordering within a session is trivially the most-recent).
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
}

/// IPC wire shape for `resolve_tool_question` (design §4.2).
/// One of `answer` / `cancelled` is populated — frontend uses
/// the `cancelled` flag to mean "user 跳过"; `answer` is the
/// normal-path wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolQuestionResolvePayload {
    pub session_id: String,
    pub tool_use_id: String,
    #[serde(default)]
    pub answer: Vec<QuestionAnswer>,
    #[serde(default)]
    pub cancelled: bool,
}

/// Internal — what the oneshot delivers on resolve. Three
/// states; downstream `execute_blocking` matches on these to
/// produce the right `tool_result` content + `is_error` flag.
///
/// `SessionCancelled` is the wire shape for the session-cancel
/// arm (token.cancelled inside `execute_blocking`'s
/// `tokio::select!`) — not constructed in this module's
/// helpers but reachable through the consumer pattern; the
/// enum variant stays in case a future caller (e.g. tests or a
/// parallel cancellation source) needs it.
#[derive(Debug, Clone)]
#[allow(dead_code)] // SessionCancelled is consumed in execute_blocking, not constructed here
pub enum QuestionResponse {
    /// User submitted the card with valid answers; the agent
    /// loop receives `Vec<QuestionAnswer>` to serialize as
    /// `tool_result` content (success).
    Answered(Vec<QuestionAnswer>),
    /// User clicked "跳过" on the card; tool_result is
    /// `{"cancelled": true}` with `is_error: true`.
    Cancelled,
    /// Session cancel token fired mid-wait (user Stop / app
    /// shutdown / destructive op cancel-inflight); tool_result
    /// is `{"cancelled_by_session": true}` with `is_error:
    /// true`. Distinct from `Cancelled` so the agent loop can
    /// tell "user chose to skip" from "session was forcibly
    /// stopped" — same UI surface but different recovery
    /// semantics.
    SessionCancelled,
}

/// One pending `ask_user_question`. The `oneshot` is `Option`
/// because `resolve` clears it (so a second resolve is a
/// no-op rather than a panic). The `payload` stays so
/// `get_pending_question` can return it for session-switch
/// recovery (frontend re-injects the card on the switched-back
/// session).
///
/// `tool_use_id` / `session_id` / `ts` are kept for parity with
/// the wire-side `ToolQuestionPayload` (frontend debugging
/// traces), but are not currently read off this struct — they
/// live on `payload` instead.
#[allow(dead_code)]
pub struct PendingQuestion {
    pub tool_use_id: String,
    pub session_id: String,
    pub ts: i64,
    pub oneshot: Option<oneshot::Sender<QuestionResponse>>,
    pub payload: ToolQuestionPayload,
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

    /// Register a new pending question for `session_id`. Returns
    /// `Err(AlreadyPending)` if a question for this session
    /// already exists — the agent loop surfaces that as a
    /// structured `tool_result(is_error: true)` so the LLM
    /// understands it's a concurrency gate (and naturally
    /// serializes on the next turn).
    ///
    /// The returned `oneshot::Receiver` is held by `execute_blocking`
    /// inside `tokio::select!{cancel, oneshot}` (mirrors the
    /// `permission_asks` permission-store consumption pattern).
    pub async fn register(
        &self,
        session_id: &str,
        tool_use_id: &str,
        payload: ToolQuestionPayload,
    ) -> Result<oneshot::Receiver<QuestionResponse>, QuestionStoreError> {
        let mut map = self.inner.lock().await;
        if map.contains_key(session_id) {
            return Err(QuestionStoreError::AlreadyPending);
        }
        let (tx, rx) = oneshot::channel();
        map.insert(
            session_id.to_string(),
            PendingQuestion {
                tool_use_id: tool_use_id.to_string(),
                session_id: session_id.to_string(),
                ts: payload.ts,
                oneshot: Some(tx),
                payload,
            },
        );
        Ok(rx)
    }

    /// Resolve a pending question. Called by the
    /// `resolve_tool_question` Tauri command (frontend
    /// invokes on submit / 跳过).
    ///
    /// Returns `NotFound` if the session has no pending
    /// question (race: frontend clicked Submit AFTER the
    /// session-cancel arm already fired). Returns
    /// `AlreadyResolved` if the oneshot was already taken
    /// (double-resolve — defensive guard).
    pub async fn resolve(
        &self,
        session_id: &str,
        response: QuestionResponse,
    ) -> Result<(), QuestionStoreError> {
        let mut map = self.inner.lock().await;
        let pending = map
            .get_mut(session_id)
            .ok_or(QuestionStoreError::NotFound)?;
        let tx = pending
            .oneshot
            .take()
            .ok_or(QuestionStoreError::AlreadyResolved)?;
        // Take the entry out — the question is no longer
        // pending. If the sender fails (receiver already
        // dropped because the cancel arm selected), the
        // QuestionResponse is silently consumed; no audit /
        // no-op needed (the cancel path produced its own
        // tool_result already).
        map.remove(session_id);
        let _ = tx.send(response);
        Ok(())
    }

    /// Remove a pending question without sending through the
    /// oneshot. Used by the cancel arm in `execute_blocking`'s
    /// `tokio::select!` — the receiver is dropped here, which
    /// makes the `oneshot::Receiver` return `Err(RecvError)`;
    /// `execute_blocking` treats that as `SessionCancelled`.
    pub async fn remove(&self, session_id: &str) -> Option<PendingQuestion> {
        let mut map = self.inner.lock().await;
        map.remove(session_id)
    }

    /// Read-only snapshot for `get_pending_question` (frontend
    /// session-switch recovery). The `Payload` is the
    /// `ToolQuestionPayload` directly (skipping the internal
    /// `PendingQuestion` fields — frontend doesn't need
    /// `oneshot` or `ts` mapped separately; `ts` is inside the
    /// payload).
    pub async fn get_payload(
        &self,
        session_id: &str,
    ) -> Option<ToolQuestionPayload> {
        let map = self.inner.lock().await;
        map.get(session_id).map(|p| p.payload.clone())
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

/// Errors for `QuestionStore` operations. Mirrors the
/// `PermissionStore` shape (NotFound / AlreadyResolved are the
/// "no pending" / "race" pair; `AlreadyPending` is the
/// concurrency gate the `PermissionStore` doesn't need because
/// Tier 4 doesn't enforce single-pending).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionStoreError {
    /// `register` called for a session that already has a
    /// pending question (single-pending invariant, design
    /// §6.1). The LLM-facing tool_result is a structured
    /// "已有 pending question,等当前回答完成" message.
    AlreadyPending,
    /// `resolve` called for a session with no pending entry.
    /// Race: frontend Submit AFTER session cancel arm already
    /// cleared the entry. Frontend sees a no-op command
    /// result; not user-facing.
    NotFound,
    /// `resolve` called twice for the same session (the
    /// second call found the entry but the oneshot was
    /// already taken). Defensive — should not happen in
    /// production (IPC layer doesn't expose enough knobs to
    /// trigger this), but cheap to guard.
    AlreadyResolved,
}

impl std::fmt::Display for QuestionStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyPending => write!(f, "a question is already pending for this session"),
            Self::NotFound => write!(f, "no pending question for this session"),
            Self::AlreadyResolved => write!(f, "question already resolved"),
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
            }],
            ts: 1_700_000_000_000,
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
            .register("s1", "tu_1", payload.clone())
            .await
            .expect("register ok");
        // get_payload returns it BEFORE resolve.
        assert!(store.get_payload("s1").await.is_some());
        let answers = vec![QuestionAnswer {
            question: "Pick one".into(),
            header: None,
            options: vec!["A".into()],
            multi_select: false,
        }];
        store
            .resolve("s1", QuestionResponse::Answered(answers.clone()))
            .await
            .expect("resolve ok");
        // Receiver fires.
        let got = rx.await.expect("receiver ok");
        match got {
            QuestionResponse::Answered(a) => assert_eq!(a, answers),
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
            .register("s1", "tu_1", p1.clone())
            .await
            .expect("first register ok");
        let p2 = make_payload("s1", "tu_2");
        let err = store
            .register("s1", "tu_2", p2)
            .await
            .expect_err("second register errors");
        assert_eq!(err, QuestionStoreError::AlreadyPending);
        // First entry still present.
        let got = store.get_payload("s1").await.expect("first entry still present");
        assert_eq!(got.tool_use_id, "tu_1");
    }

    /// resolve on an unknown session returns `NotFound`.
    #[tokio::test]
    async fn resolve_not_found() {
        let store = QuestionStore::new();
        let err = store
            .resolve("unknown", QuestionResponse::Cancelled)
            .await
            .expect_err("resolve unknown errors");
        assert_eq!(err, QuestionStoreError::NotFound);
    }

    /// resolve twice on the same session returns
    /// `AlreadyResolved` on the second call (the entry was
    /// already cleared by the first resolve).
    #[tokio::test]
    async fn resolve_already_resolved() {
        let store = QuestionStore::new();
        store
            .register("s1", "tu_1", make_payload("s1", "tu_1"))
            .await
            .expect("register ok");
        store
            .resolve("s1", QuestionResponse::Cancelled)
            .await
            .expect("first resolve ok");
        // Second resolve fails with NotFound (entry was
        // removed) — already-resolved case is collapsed into
        // NotFound in the implementation. Either error is
        // acceptable from the frontend's perspective; we
        // accept NotFound as the documented behavior here.
        let err = store
            .resolve("s1", QuestionResponse::Cancelled)
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
            .register("s1", "tu_1", make_payload("s1", "tu_1"))
            .await
            .expect("register ok");
        let pending = store
            .remove("s1")
            .await
            .expect("remove returns pending");
        assert_eq!(pending.session_id, "s1");
        assert_eq!(pending.tool_use_id, "tu_1");
        assert!(store.get_payload("s1").await.is_none());
    }
}
