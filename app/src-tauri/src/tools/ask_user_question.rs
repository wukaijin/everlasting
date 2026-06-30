//! `ask_user_question` — agent blocking reverse-question tool
//!
//! Subagent-clone of Claude Code's `AskUserQuestion`: lets the
//! agent ask the user 1-4 structured multi-choice questions per
//! call. The execution is **blocking** (the agent loop's turn
//! suspends until the user answers or cancels); see design §3 +
//! PRD §R3.
//!
//! ## Why this lives next to the regular tools (not in agent/.
//! permissions/ or agent/::permissions/::ask)
//!
//! - Shape-wise it's a tool — the LLM discovers it via
//!   `builtin_tools()`'s schema.
//! - Execution-wise it needs the agent loop's `tokio::select!`
//!   `cancel` arm (session cancel propagates), the `QuestionStore`
//!   oneshot (parallel to `PermissionStore`), and the
//!   `ChatEventSink::emit_tool_question` trait method. All three
//!   are already threaded through `chat_loop.rs`. Adding the
//!   tool here keeps the agent-loop "blocking tool" interception
//!   obvious (the only special case is the `tool_name ==
//!   "ask_user_question"` branch — everything else goes through
//!   `execute_tool`).
//!
//! ## Why the execute path bypasses `execute_tool`
//!
//! `execute_tool_inner` is dispatch-by-name (`match name`), but it
//! has no access to `QuestionStore` or `ChatEventSink
//! ::emit_tool_question`. Per design §7 / PRD §R1, the clean
//! path is to keep this tool out of the dispatch table and have
//! `chat_loop.rs` recognize the tool name and call
//! `execute_blocking` directly — same shape as how
//! `dispatch_subagent` is intercepted. The `execute_tool_inner`
//! `match` arm is intentionally absent (with a doc comment
//! pointing at `chat_loop.rs`).
//!
//! ## Wire shape (PRD §"Tool input schema")
//!
//! ```json
//! {
//!   "questions": [
//!     {
//!       "question": "Which database backend?",
//!       "header": "DB",
//!       "options": [
//!         {"label": "PostgreSQL", "description": "Robust, requires service"},
//!         {"label": "SQLite", "description": "Embedded, zero-config"}
//!       ],
//!       "multi_select": false
//!     }
//!   ]
//! }
//! ```
//!
//! Schema validation is **strict** — boundary violations
//! (`questions` empty / >4 / `options` empty / >4 / `header` >
//! 12 chars) are `is_error: true` and do NOT enter the blocking
//! wait. The LLM gets a structured error message in the
//! `tool_result` so it can self-correct on the next turn.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::agent::question_store::{
    Question, QuestionResponse, QuestionStore, ToolQuestionPayload,
};
use crate::llm::types::ToolDef;
use crate::state::ChatEventSink;

// ---------------------------------------------------------------------------
// Constants — schema boundaries
// ---------------------------------------------------------------------------

/// Max questions per call (PRD §R2 schema: 1..=4).
const MAX_QUESTIONS: usize = 4;
/// Min questions per call.
const MIN_QUESTIONS: usize = 1;
/// Max options per question (PRD §R2 schema: 2..=4).
const MAX_OPTIONS: usize = 4;
/// Min options per question.
const MIN_OPTIONS: usize = 2;
/// Max header chars (PRD §R2: ≤12). Header renders as a card chip.
const MAX_HEADER_LEN: usize = 12;

// ---------------------------------------------------------------------------
// LLM input shape (validation handled in execute_blocking)
// ---------------------------------------------------------------------------

/// Wire input for `ask_user_question`. Snake_case fields to
/// match the LLM's trained Claude Code schema (zero learning
/// cost).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskUserQuestionInput {
    pub questions: Vec<Question>,
}

// ---------------------------------------------------------------------------
// Tool definition — registered in builtin_tools() (low-risk
// schema; SDK-style sync execution never fires because
// chat_loop intercepts)
// ---------------------------------------------------------------------------

/// The `ask_user_question` tool schema. Name MUST match the
/// interception branch in `chat_loop.rs` — that's how the
/// agent loop dispatches to `execute_blocking` instead of
/// `execute_tool`.
pub fn definition() -> ToolDef {
    ToolDef {
        name: "ask_user_question".to_string(),
        description: Some(
            "Ask the user one to four structured multi-choice questions to gather \
             preferences or clarify ambiguous requirements. Each question should \
             have 2 to 4 options, each option may have a short description and an \
             optional markdown preview. Use this ONLY when you genuinely need \
             user input to proceed (architecture choice, ambiguous requirement, \
             preference) — NOT for routine exploration tasks (use read_file / \
             grep / shell instead). The user can answer or skip the question. \
             Plan / Edit / Yolo modes all support this — it is not a permission \
             gate, just an information request.\n\n\
             Constraints:\n\
             - questions: 1..=4\n\
             - options per question: 2..=4\n\
             - header (optional, shown as a card chip): ≤12 characters\n\
             - When the user clicks 跳过, the tool returns is_error: true with \
             {\"cancelled\": true} — continue without that input."
                .to_string(),
        ),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string", "minLength": 1 },
                            "header": {
                                "type": "string",
                                "maxLength": 12,
                                "description": "Optional short label (≤12 chars) shown as a card chip above the question"
                            },
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string", "minLength": 1 },
                                        "description": { "type": "string" },
                                        "preview": { "type": "string", "description": "Optional markdown preview (collapsible on the card)" }
                                    },
                                    "required": ["label"]
                                }
                            },
                            "multi_select": {
                                "type": "boolean",
                                "default": false,
                                "description": "If true, the user can select multiple options (checkbox); otherwise single-select (radio)"
                            }
                        },
                        "required": ["question", "options"]
                    }
                }
            },
            "required": ["questions"]
        }),
    }
}

// ---------------------------------------------------------------------------
// Validation result
// ---------------------------------------------------------------------------

/// Internal validation outcome. We reuse `Result<_, ValidationError>`
/// rather than a typed struct so the `execute_blocking` call sites
/// can early-return on the error path without pre-allocating
/// pending question state.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ValidationError {
    #[error("`questions` must have {min}..={max} entries, got {got}")]
    QuestionsOutOfRange { min: usize, max: usize, got: usize },
    #[error("question #{idx} `options` must have {min}..={max} entries, got {got}")]
    OptionsOutOfRange {
        idx: usize,
        min: usize,
        max: usize,
        got: usize,
    },
    #[error("question #{idx} `header` exceeds {max} characters (got {got})")]
    HeaderTooLong {
        idx: usize,
        max: usize,
        got: usize,
    },
    #[error("question #{idx} `question` text is empty")]
    EmptyQuestionText { idx: usize },
    #[error("question #{idx} option #{opt_idx} `label` is empty")]
    EmptyOptionLabel { idx: usize, opt_idx: usize },
}

/// Schema validation — does NOT enter the blocking wait. Returns
/// `Err(ValidationError)` for short-circuit failures (the agent
/// loop pushes the error string as `tool_result` content with
/// `is_error: true`).
fn validate(input: &AskUserQuestionInput) -> Result<(), ValidationError> {
    if !(MIN_QUESTIONS..=MAX_QUESTIONS).contains(&input.questions.len()) {
        return Err(ValidationError::QuestionsOutOfRange {
            min: MIN_QUESTIONS,
            max: MAX_QUESTIONS,
            got: input.questions.len(),
        });
    }
    for (idx, q) in input.questions.iter().enumerate() {
        if q.question.trim().is_empty() {
            return Err(ValidationError::EmptyQuestionText { idx });
        }
        if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&q.options.len()) {
            return Err(ValidationError::OptionsOutOfRange {
                idx,
                min: MIN_OPTIONS,
                max: MAX_OPTIONS,
                got: q.options.len(),
            });
        }
        for (opt_idx, opt) in q.options.iter().enumerate() {
            if opt.label.trim().is_empty() {
                return Err(ValidationError::EmptyOptionLabel { idx, opt_idx });
            }
        }
        if let Some(h) = q.header.as_ref() {
            if h.chars().count() > MAX_HEADER_LEN {
                return Err(ValidationError::HeaderTooLong {
                    idx,
                    max: MAX_HEADER_LEN,
                    got: h.chars().count(),
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// execute_blocking — the ONLY execution entry point
// ---------------------------------------------------------------------------

/// Result tuple shape — matches the agent loop's existing
/// `execute_tool` return shape so the call site at
/// `chat_loop.rs` can build the same `ContentBlock::ToolResult`.
/// The `Option<i32>` is the exit_code (always `None` here — no
/// shell spawned). `Option<bool>` is reserved for future
/// structured fields and is also always `None`.
pub type BlockingToolResult = (
    /* content */ String,
    /* is_error */ bool,
    /* tool_context_update */ crate::tools::ToolContextUpdate,
    /* exit_code */ Option<i32>,
);

/// Long-running execution. Same signature shape as `execute_tool`
/// (modulo `&QuestionStore` + `&dyn ChatEventSink` + `&str`
/// `tool_use_id` + `&str` `session_id`).
///
/// Pipeline (one straight shot, no nested abstraction):
///
/// 1. Parse + validate input. Bad input → short-circuit
///    `(error, true, _, None)`. Does NOT touch the store or
///    emit anything (no IPC, no pending state — the LLM just
///    self-corrects on the next turn).
/// 2. `QuestionStore::register(session_id, tool_use_id, payload)`.
///    Already-pending race → return structured error string
///    `(error, true, _, None)`. The first pending stays
///    untouched.
/// 3. `sink.emit_tool_question(&payload)` — fires the
///    `tool:question` Tauri event so the frontend renders the
///    card.
/// 4. `tokio::select! { biased; cancel | oneshot }` —
///    - cancel arm → `QuestionStore::remove(session_id)`,
///      returns `(cancelled_by_session, true, _, None)`.
///    - oneshot arm — `QuestionResponse` matched:
///      - `Answered(answers)` → return `(json answers, false,
///        _, None)`.
///      - `Cancelled` → return `({"cancelled": true}, true, _,
///        None)`.
///      - `Err(RecvError)` (sender dropped by the cancel arm's
///        `store.remove`) → return `({"cancelled_by_session":
///        true}, true, _, None)`.
///
/// **No retries, no timeout** — v1 keeps it simple (PRD §"Notes"
/// "无 timeout 兜底"). A user who neither answers nor skips and
/// also doesn't Stop is just stuck on a card; acceptable
/// because the agent's tokio task is blocked anyway (no LLM
/// work happening).
#[allow(clippy::too_many_arguments)]
pub async fn execute_blocking(
    input: &serde_json::Value,
    session_id: &str,
    tool_use_id: &str,
    store: &QuestionStore,
    sink: &Arc<dyn ChatEventSink>,
    cancel: &CancellationToken,
) -> BlockingToolResult {
    // ---- 1. Parse + validate ----------------------------------------
    let parsed: AskUserQuestionInput = match serde_json::from_value(input.clone()) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("ask_user_question: invalid input JSON: {}", e);
            tracing::warn!(
                session_id = %session_id,
                tool_use_id = %tool_use_id,
                error = %e,
                "ask_user_question: short-circuit on parse error"
            );
            return (msg, true, crate::tools::ToolContextUpdate::default(), None);
        }
    };
    if let Err(e) = validate(&parsed) {
        let msg = format!("ask_user_question: schema validation failed: {}", e);
        tracing::warn!(
            session_id = %session_id,
            tool_use_id = %tool_use_id,
            error = %e,
            "ask_user_question: short-circuit on schema validation"
        );
        return (msg, true, crate::tools::ToolContextUpdate::default(), None);
    }

    // ---- 2. Build the payload ---------------------------------------
    let payload = ToolQuestionPayload {
        session_id: session_id.to_string(),
        tool_use_id: tool_use_id.to_string(),
        questions: parsed.questions.clone(),
        ts: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
    };

    // ---- 3. Register + emit -----------------------------------------
    let rx = match store
        .register(session_id, tool_use_id, payload.clone())
        .await
    {
        Ok(rx) => rx,
        Err(crate::agent::question_store::QuestionStoreError::AlreadyPending) => {
            let msg =
                "已有 pending question,等当前回答完成".to_string();
            tracing::warn!(
                session_id = %session_id,
                tool_use_id = %tool_use_id,
                "ask_user_question: AlreadyPending — concurrent register"
            );
            return (msg, true, crate::tools::ToolContextUpdate::default(), None);
        }
        Err(e) => {
            // `NotFound` is not reachable from `register` —
            // defensive branch (register only ever returns
            // `AlreadyPending`). Kept for exhaustiveness.
            let msg = format!("ask_user_question: store error: {}", e);
            tracing::error!(
                session_id = %session_id,
                tool_use_id = %tool_use_id,
                error = %e,
                "ask_user_question: unexpected register error"
            );
            return (msg, true, crate::tools::ToolContextUpdate::default(), None);
        }
    };
    // Emit AFTER register — the frontend's `get_pending_question`
    // fallback (used on session reload) needs the entry to
    // exist when the event arrives. Symmetric with the
    // permission:ask flow which emits AFTER register_ask.
    sink.emit_tool_question(&payload);

    // ---- 4. Wait for resolve / cancel -------------------------------
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            // Session cancel (user Stop / app shutdown / destructive
            // op cancel-inflight). Drop the oneshot receiver so
            // the sender's resolve (if it arrives late) becomes a
            // no-op. Clean the store entry.
            store.remove(session_id).await;
            tracing::info!(
                session_id = %session_id,
                tool_use_id = %tool_use_id,
                "ask_user_question: cancelled by session token"
            );
            let content = serde_json::json!({"cancelled_by_session": true})
                .to_string();
            (content, true, crate::tools::ToolContextUpdate::default(), None)
        }
        resp = rx => {
            match resp {
                Ok(QuestionResponse::Answered(answers)) => {
                    // Standard success path. Serialize the answer
                    // list as JSON so the LLM sees a structured
                    // payload that matches the wire spec (PRD
                    // §R4).
                    let content = match serde_json::to_string(&answers) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!(
                                session_id = %session_id,
                                tool_use_id = %tool_use_id,
                                error = %e,
                                "ask_user_question: failed to serialize answers"
                            );
                            return (
                                format!("ask_user_question: serialize error: {}", e),
                                true,
                                crate::tools::ToolContextUpdate::default(),
                                None,
                            );
                        }
                    };
                    (content, false, crate::tools::ToolContextUpdate::default(), None)
                }
                Ok(QuestionResponse::Cancelled) => {
                    // User clicked 跳过 (PRD §R5).
                    let content =
                        serde_json::json!({"cancelled": true}).to_string();
                    (content, true, crate::tools::ToolContextUpdate::default(), None)
                }
                Err(_recv_err) => {
                    // Sender dropped (e.g. resolve ran on a stale
                    // session id after cancel arm cleaned the
                    // entry). Treat as session-cancelled — safe
                    // default per the permission-store parity
                    // (also uses Deny on RecvError).
                    let content = serde_json::json!({"cancelled_by_session": true})
                        .to_string();
                    (content, true, crate::tools::ToolContextUpdate::default(), None)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::question_store::{
        QuestionAnswer, QuestionOption, QuestionStoreError, ToolQuestionPayload,
    };

    // ----- helpers for tests -----

    /// A `dyn ChatEventSink` stub that captures the latest emit
    /// so tests can assert "did we publish to IPC?". Mirrors the
    /// test infrastructure pattern used elsewhere (collects
    /// emissions instead of forwarding to `tauri::AppHandle`).
    /// Note: `emit_tool_question` is sync (matches the rest of
    /// the `ChatEventSink` trait — no async fns).
    #[derive(Default)]
    struct CapturingSink {
        emitted: std::sync::Mutex<Vec<ToolQuestionPayload>>,
    }

    impl ChatEventSink for CapturingSink {
        fn emit_chat_event(
            &self,
            _payload: &crate::state::ChatEventPayload,
        ) {
        }
        fn emit_tool_call(&self, _payload: &crate::state::ToolCallPayload) {}
        fn emit_tool_result(
            &self,
            _payload: &crate::state::ToolResultPayload,
        ) {
        }
        fn emit_permission_ask(
            &self,
            _payload: crate::agent::permissions::PermissionAskPayload,
        ) {
        }
        fn emit_tool_question(&self, payload: &ToolQuestionPayload) {
            self.emitted.lock().unwrap().push(payload.clone());
        }
    }

    fn make_valid_input() -> serde_json::Value {
        serde_json::json!({
            "questions": [
                {
                    "question": "Pick a backend",
                    "header": "DB",
                    "options": [
                        {"label": "Postgres", "description": "needs service"},
                        {"label": "SQLite", "description": "embedded"}
                    ],
                    "multi_select": false
                }
            ]
        })
    }

    fn make_sink() -> Arc<CapturingSink> {
        Arc::new(CapturingSink::default())
    }

    // ----- validation short-circuits (no store, no sink touched) -----

    /// Schema-violating input never reaches register/emit. We
    /// verify by passing a closed-over capturing sink that
    /// would panic if `emit_tool_question` were called.
    #[tokio::test]
    async fn validation_empty_questions_short_circuits() {
        let store = QuestionStore::new();
        let sink = make_sink();
        let input = serde_json::json!({"questions": []});
        let cancel = CancellationToken::new();
        let (content, is_error, _, _) = execute_blocking(
            &input,
            "s1",
            "tu_1",
            &store,
            &(sink.clone() as Arc<dyn ChatEventSink>),
            &cancel,
        )
        .await;
        assert!(is_error, "validation failure returns is_error: true");
        assert!(content.contains("schema validation failed"));
        assert!(sink.emitted.lock().unwrap().is_empty(), "emit not called");
        // Store stays clean.
        assert!(store.get_payload("s1").await.is_none());
    }

    #[tokio::test]
    async fn validation_too_many_questions_short_circuits() {
        let store = QuestionStore::new();
        let sink = make_sink();
        let input = serde_json::json!({
            "questions": [
                {"question": "q1", "options": [
                    {"label": "a"}, {"label": "b"}
                ]},
                {"question": "q2", "options": [
                    {"label": "a"}, {"label": "b"}
                ]},
                {"question": "q3", "options": [
                    {"label": "a"}, {"label": "b"}
                ]},
                {"question": "q4", "options": [
                    {"label": "a"}, {"label": "b"}
                ]},
                {"question": "q5", "options": [
                    {"label": "a"}, {"label": "b"}
                ]}
            ]
        });
        let cancel = CancellationToken::new();
        let (content, is_error, _, _) = execute_blocking(
            &input,
            "s1",
            "tu_1",
            &store,
            &(sink.clone() as Arc<dyn ChatEventSink>),
            &cancel,
        )
        .await;
        assert!(is_error);
        assert!(content.contains("schema validation failed"));
        assert!(sink.emitted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn validation_options_out_of_range_short_circuits() {
        let store = QuestionStore::new();
        let sink = make_sink();
        // 1 option (need 2..=4).
        let input = serde_json::json!({
            "questions": [{
                "question": "q",
                "options": [{"label": "only"}]
            }]
        });
        let cancel = CancellationToken::new();
        let (content, is_error, _, _) = execute_blocking(
            &input,
            "s1",
            "tu_1",
            &store,
            &(sink.clone() as Arc<dyn ChatEventSink>),
            &cancel,
        )
        .await;
        assert!(is_error);
        assert!(content.contains("schema validation failed"));
        assert!(sink.emitted.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn validation_header_too_long_short_circuits() {
        let store = QuestionStore::new();
        let sink = make_sink();
        let input = serde_json::json!({
            "questions": [{
                "question": "q",
                "header": "this header is way too long for a 12-char chip",
                "options": [
                    {"label": "a"}, {"label": "b"}
                ]
            }]
        });
        let cancel = CancellationToken::new();
        let (content, is_error, _, _) = execute_blocking(
            &input,
            "s1",
            "tu_1",
            &store,
            &(sink.clone() as Arc<dyn ChatEventSink>),
            &cancel,
        )
        .await;
        assert!(is_error);
        assert!(content.contains("schema validation failed"));
        assert!(sink.emitted.lock().unwrap().is_empty());
    }

    // ----- happy path + register-before-emit invariant -----

    #[tokio::test]
    async fn happy_path_registers_emits_and_returns_answers() {
        let store = QuestionStore::new();
        let sink = make_sink();
        let input = make_valid_input();
        let cancel = CancellationToken::new();
        let sink_arc: Arc<dyn ChatEventSink> = sink.clone();

        // Spawn the executor so we can resolve from the main
        // task and complete the oneshot.
        let store_clone = store.clone();
        let input_clone = input.clone();
        let cancel_clone = cancel.clone();
        let exec = tokio::spawn(async move {
            execute_blocking(
                &input_clone,
                "s1",
                "tu_1",
                &store_clone,
                &sink_arc,
                &cancel_clone,
            )
            .await
        });

        // Wait for the executor to register + emit — poll the
        // store instead of a fixed sleep (robust against CI
        // scheduling jitter; mirrors the integration suite's
        // `spawn_resolver` wait loop).
        let register_wait_deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(5);
        while store.get_payload("s1").await.is_none() {
            if std::time::Instant::now() > register_wait_deadline {
                panic!("executor never registered the pending question");
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        // emit was called.
        let emitted = sink.emitted.lock().unwrap();
        assert_eq!(emitted.len(), 1, "emit_tool_question called once");
        assert_eq!(emitted[0].session_id, "s1");
        assert_eq!(emitted[0].tool_use_id, "tu_1");
        drop(emitted);

        // Resolve with an Answer.
        let answers = vec![QuestionAnswer {
            question: "Pick a backend".into(),
            header: Some("DB".into()),
            options: vec!["Postgres".into()],
            multi_select: false,
        }];
        store
            .resolve("s1", QuestionResponse::Answered(answers.clone()))
            .await
            .expect("resolve ok");

        let (content, is_error, _, _) = exec.await.expect("exec ok");
        assert!(!is_error, "answered returns is_error: false");
        // Content is JSON-serialized answers.
        let parsed: Vec<QuestionAnswer> =
            serde_json::from_str(&content).expect("content is valid JSON");
        assert_eq!(parsed, answers);
    }

    // ----- cancel arm → cancelled_by_session content -----

    #[tokio::test]
    async fn cancel_arm_returns_session_cancelled_marker() {
        let store = QuestionStore::new();
        let sink = make_sink();
        let input = make_valid_input();
        let cancel = CancellationToken::new();
        let sink_arc: Arc<dyn ChatEventSink> = sink.clone();

        let store_clone = store.clone();
        let input_clone = input.clone();
        let cancel_clone = cancel.clone();
        let exec = tokio::spawn(async move {
            execute_blocking(
                &input_clone,
                "s1",
                "tu_1",
                &store_clone,
                &sink_arc,
                &cancel_clone,
            )
            .await
        });

        let register_wait_deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(5);
        while store.get_payload("s1").await.is_none() {
            if std::time::Instant::now() > register_wait_deadline {
                panic!("executor never registered the pending question");
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        cancel.cancel();
        let (content, is_error, _, _) = exec.await.expect("exec ok");
        assert!(is_error);
        assert!(content.contains("cancelled_by_session"));
        // Store cleaned.
        assert!(store.get_payload("s1").await.is_none());
    }

    // ----- cancelled-by-user path -----

    #[tokio::test]
    async fn cancelled_path_returns_user_cancel_marker() {
        let store = QuestionStore::new();
        let sink = make_sink();
        let input = make_valid_input();
        let cancel = CancellationToken::new();
        let sink_arc: Arc<dyn ChatEventSink> = sink.clone();

        let store_clone = store.clone();
        let input_clone = input.clone();
        let cancel_clone = cancel.clone();
        let exec = tokio::spawn(async move {
            execute_blocking(
                &input_clone,
                "s1",
                "tu_1",
                &store_clone,
                &sink_arc,
                &cancel_clone,
            )
            .await
        });

        let register_wait_deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(5);
        while store.get_payload("s1").await.is_none() {
            if std::time::Instant::now() > register_wait_deadline {
                panic!("executor never registered the pending question");
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        store
            .resolve("s1", QuestionResponse::Cancelled)
            .await
            .expect("resolve ok");
        let (content, is_error, _, _) = exec.await.expect("exec ok");
        assert!(is_error, "cancelled = is_error: true");
        assert!(content.contains("\"cancelled\":true")
            || content.contains("\"cancelled\": true"),
            "content carries cancelled marker: {}",
            content);
    }

    // ----- AlreadyPending race -----

    #[tokio::test]
    async fn already_pending_returns_structured_error() {
        let store = QuestionStore::new();
        let sink = make_sink();
        // Pre-register.
        store
            .register(
                "s1",
                "tu_pre",
                ToolQuestionPayload {
                    session_id: "s1".into(),
                    tool_use_id: "tu_pre".into(),
                    questions: vec![Question {
                        question: "preexisting".into(),
                        header: None,
                        options: vec![
                            QuestionOption {
                                label: "a".into(),
                                description: None,
                                preview: None,
                            },
                            QuestionOption {
                                label: "b".into(),
                                description: None,
                                preview: None,
                            },
                        ],
                        multi_select: false,
                    }],
                    ts: 0,
                },
            )
            .await
            .expect("pre-register ok");

        let input = make_valid_input();
        let cancel = CancellationToken::new();
        let sink_arc: Arc<dyn ChatEventSink> = sink.clone();
        let (content, is_error, _, _) = execute_blocking(
            &input,
            "s1",
            "tu_2",
            &store,
            &sink_arc,
            &cancel,
        )
        .await;
        assert!(is_error);
        assert!(content.contains("已有 pending"));
        // No emit happened for the duplicate.
        assert!(sink.emitted.lock().unwrap().is_empty());
        // The first pending is still there.
        assert!(store.get_payload("s1").await.is_some());
        assert_eq!(
            store.get_payload("s1").await.unwrap().tool_use_id,
            "tu_pre",
            "pre-existing pending untouched"
        );

        // Drain for test isolation.
        let _ = store.remove("s1").await;
    }

    // ----- schema validate export pure-fn -----
    #[test]
    fn validate_accepts_well_formed_input() {
        let input = AskUserQuestionInput {
            questions: vec![Question {
                question: "valid".into(),
                header: Some("ok".into()),
                options: vec![
                    QuestionOption {
                        label: "a".into(),
                        description: Some("desc".into()),
                        preview: None,
                    },
                    QuestionOption {
                        label: "b".into(),
                        description: None,
                        preview: None,
                    },
                ],
                multi_select: true,
            }],
        };
        validate(&input).expect("valid input passes");
    }

    #[test]
    fn validate_rejects_empty_question_text() {
        let input = AskUserQuestionInput {
            questions: vec![Question {
                question: "   ".into(),
                header: None,
                options: vec![
                    QuestionOption {
                        label: "a".into(),
                        description: None,
                        preview: None,
                    },
                    QuestionOption {
                        label: "b".into(),
                        description: None,
                        preview: None,
                    },
                ],
                multi_select: false,
            }],
        };
        let err = validate(&input).expect_err("empty question rejected");
        assert!(matches!(err, ValidationError::EmptyQuestionText { .. }));
    }

    #[test]
    fn validate_rejects_empty_option_label() {
        let input = AskUserQuestionInput {
            questions: vec![Question {
                question: "ok".into(),
                header: None,
                options: vec![
                    QuestionOption {
                        label: "".into(),
                        description: None,
                        preview: None,
                    },
                    QuestionOption {
                        label: "ok".into(),
                        description: None,
                        preview: None,
                    },
                ],
                multi_select: false,
            }],
        };
        let err = validate(&input).expect_err("empty option label rejected");
        assert!(matches!(err, ValidationError::EmptyOptionLabel { .. }));
    }

    // ----- QuestionStoreError used here (sanity) -----

    #[test]
    fn store_error_already_pending_variant_compiles() {
        // compiles = type-stable variant referenced by execute_blocking.
        let _: QuestionStoreError = QuestionStoreError::AlreadyPending;
    }
}
