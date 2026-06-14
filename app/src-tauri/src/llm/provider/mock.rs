//! `MockProvider` — scripted stream `Provider` for Agent Loop
//! integration tests.
//!
//! # Why
//!
//! The Agent Loop (`crate::agent::chat`) is the highest-risk
//! subsystem: cancel races across three layers, C3 compaction
//! preserving tool_use/tool_result pair atomicity, MAX_TURNS
//! fallback, per-turn latency tracking, and tool execution +
//! `⑨ 关` permission dispatch all run inside a 50-turn spawn.
//! None of this is reachable from a unit test on the underlying
//! LLM provider trait — we need a real Provider impl that the
//! agent loop can dispatch through, and which the test fully
//! controls (scripted events, deterministic timing, no I/O).
//!
//! The pre-PR5 test surface was 469 single-function unit tests
//! covering protocol parsing, permission policy, and DB CRUD
//! individually. End-to-end turn-loop coverage was the gap that
//! the 2026-06-14 full-audit flagged as **P1 RULE-A-006**: a P0
//! fix lands and we have no regression protection for "turn 1
//! text → turn 2 tool_use → cancel mid-tool" because no test
//! can drive that path.
//!
//! # Design
//!
//! - **Scripted, not HTTP-mocked.** A `MockProvider` carries a
//!   `Vec<MockResponse>` script; the Nth call to `send` returns
//!   the Nth entry. This lets a single test script multi-turn
//!   flows without standing up a real HTTP server (the SSE
//!   protocol layer is already covered by 105 dedicated tests).
//! - **`#[cfg(test)]` only.** The entire module is gated on the
//!   test configuration; production binaries compile it out
//!   (the `ProviderProtocol::Mock` variant is also `#[cfg(test)]`
//!   so it never appears in the wire format).
//! - **Tracks call count.** `call_count()` is the assertion
//!   surface for "did the agent loop call `send` the expected
//!   number of times?" — the single most important turn-orchestration
//!   invariant in the audit.
//! - **Exhaustion is observable.** A `send` call past the end of
//!   the script yields a single `LlmError::InvalidRequest` then end-of-
//!   stream; the test can detect "agent loop asked for more
//!   turns than I scripted" and fail. `InvalidRequest` is chosen
//!   over `Server` so the inner message ("exhausted at call N")
//!   survives `LlmError::user_message()` (which would otherwise
//!   drop the inner text for the `Server` variant).
//!
//! # Usage
//!
//! ```ignore
//! use crate::llm::provider::mock::{MockProvider, MockResponse};
//! use crate::llm::types::{ChatEvent, TokenUsage};
//!
//! let mock = Arc::new(MockProvider::new(vec![
//!     // Turn 1: text-only response
//!     MockResponse::Events(vec![
//!         Ok(ChatEvent::Start),
//!         Ok(ChatEvent::Delta { text: "hi".into() }),
//!         Ok(ChatEvent::Done {
//!             stop_reason: Some("end_turn".into()),
//!             usage: Some(TokenUsage::default()),
//!         }),
//!     ]),
//! ]));
//! // Pass `mock` to the agent loop's `run_chat_loop` helper.
//! // After completion: assert_eq!(mock.call_count(), 1);
//! ```
//!
//! # Why not `wiremock` / `mockito`?
//!
//! The protocol layer (`AnthropicProvider` / `OpenAIProvider`)
//! already has 105 dedicated tests that exercise the SSE parser,
//! `usage` extraction, and cross-protocol wire conversion.
//! Standing up a mock HTTP server for the Agent Loop would
//! re-test the protocol layer (regression, not progress) while
//! adding ~50ms of latency per turn for socket I/O. The scripted-
//! stream model is the leanest design that gives the agent
//! loop a real `Provider` to dispatch through.

#![cfg(test)]

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_stream::stream;
use futures_util::Stream;

use super::{Provider, ProviderCapabilities};
use crate::db::ProviderProtocol;
use crate::llm::error::LlmError;
use crate::llm::types::{ChatEvent, ChatMessage, ToolDef};

/// A scripted Provider that returns a pre-canned stream per
/// `send` call. See module docstring for the design rationale.
pub struct MockProvider {
    script: Mutex<VecDeque<MockResponse>>,
    call_count: Arc<AtomicUsize>,
    capabilities: ProviderCapabilities,
}

/// One step in the mock provider's script — the response to a
/// single `send` call. The agent loop calls `send` once per
/// turn, so a 3-turn script has 3 entries.
///
/// The enum is **non-Clone by design** (LlmError holds a String
/// payload and isn't Clone — and cloning the script per `send`
/// would defeat the `Mutex<Vec<…>>` consumption model). Each
/// `MockResponse` is consumed exactly once by the script
/// mechanism; the test builds a fresh `Vec<MockResponse>` per
/// test case.
#[derive(Debug)]
pub enum MockResponse {
    /// Yield the events in order, then end the stream. Each
    /// `Result` is forwarded to the agent loop's per-event
    /// `select!` arm verbatim.
    Events(Vec<Result<ChatEvent, LlmError>>),
    /// The stream never produces an event (it polls
    /// `futures::pending`). The test must cancel via the
    /// cancellation token to break the agent loop out. Used
    /// to assert cancel-mid-stream behavior without timing
    /// dependencies.
    HangingThenCancel,
    /// Single `Err` event then end-of-stream. The agent loop
    /// treats this as a `ChatEvent::Error` with `had_error =
    /// true` and bails out.
    ErrThenEnd(LlmError),
}

impl MockProvider {
    /// Build a mock provider with the given script. Default
    /// capabilities (system / tools / streaming all true) match
    /// what the real Anthropic and OpenAI providers report, so
    /// the agent loop's dispatch is unaffected.
    pub fn new(script: Vec<MockResponse>) -> Self {
        Self {
            script: Mutex::new(script.into_iter().collect::<VecDeque<_>>()),
            call_count: Arc::new(AtomicUsize::new(0)),
            capabilities: ProviderCapabilities {
                supports_system_prompt: true,
                supports_tools: true,
                supports_streaming: true,
            },
        }
    }

    /// Override the capabilities reported by `capabilities()`.
    /// Most tests don't need this; it's here for the
    /// capability-gated dispatch tests that may land in a
    /// future PR.
    #[allow(dead_code)]
    pub fn with_capabilities(mut self, capabilities: ProviderCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Number of times `send` has been called so far. Tests
    /// assert on this to lock the agent loop's turn count
    /// (e.g. "exactly 1 send for a text-only response", or
    /// "exactly 2 sends for a tool_use → tool_result → text
    /// flow").
    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    /// Shared handle to the call count, for tests that want to
    /// observe progress from a concurrent cancellation task.
    pub fn call_count_handle(&self) -> Arc<AtomicUsize> {
        self.call_count.clone()
    }
}

impl Provider for MockProvider {
    fn send(
        &self,
        _system: Option<String>,
        _messages: Vec<ChatMessage>,
        _tools: Vec<ToolDef>,
    ) -> Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send + 'static>> {
        // Atomically claim the next turn slot. The fetch_add is
        // the canonical "I consumed one turn" signal; the
        // call_count() reader sees the updated value the moment
        // we return the stream.
        //
        // We hold a `Mutex` on the script for the duration of the
        // pop_front. The mutex is uncontended in the test (only
        // the agent loop calls `send` per Provider, and it's
        // single-threaded turn orchestrator), so the lock cost is
        // negligible.
        //
        // Index discipline: we consume entries strictly in order
        // (turn 1 → entry 0, turn 2 → entry 1, ...). The original
        // implementation used `Vec::remove(idx)` which SHIFTS the
        // remaining entries after each call — so on the 2nd call,
        // `remove(1)` would look past the (now size-1) Vec and hit
        // the exhaustion branch even when a 2nd entry was scripted.
        // The fix is queue-discipline: pop_front on every call,
        // and use the pre-pop state (empty queue) as the
        // exhaustion signal. `idx` is preserved only for the
        // exhaustion error message.
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        let response = {
            let mut script_guard = self.script.lock().unwrap();
            match script_guard.pop_front() {
                Some(entry) => entry,
                None => {
                    // Script exhaustion is an observable failure:
                    // the agent loop asked for more turns than the
                    // test scripted. Surface a typed error so the
                    // test can assert on it (and so production-
                    // style fail-fast semantics are preserved).
                    //
                    // Use `InvalidRequest` (not `Server`) so the
                    // inner message ("exhausted") survives
                    // `LlmError::user_message()` (which would
                    // otherwise drop it for the Server variant).
                    // The exhaustion is also semantically a
                    // "request for an unscripted turn" rather
                    // than an HTTP server failure.
                    MockResponse::ErrThenEnd(LlmError::InvalidRequest(format!(
                        "MockProvider script exhausted at call {} (script had 0 entries remaining)",
                        idx,
                    )))
                }
            }
        };

        match response {
            MockResponse::Events(events) => Box::pin(stream! {
                for e in events {
                    yield e;
                }
            }),
            MockResponse::HangingThenCancel => {
                // Pending forever. The agent loop's `select!`
                // notices the cancellation token first (we
                // poll the cancel arm `biased;`). We use
                // `futures_util::stream::pending` (the
                // generic Stream version) — it's NOT a macro
                // in the stream module, despite `pending()` in
                // the future module being a macro.
                use futures_util::stream::pending;
                let s: Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send>> =
                    Box::pin(pending::<Result<ChatEvent, LlmError>>());
                s
            }
            MockResponse::ErrThenEnd(e) => {
                let s = stream! {
                    yield Err::<ChatEvent, LlmError>(e);
                };
                Box::pin(s)
            }
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities
    }

    fn protocol(&self) -> ProviderProtocol {
        ProviderProtocol::Mock
    }
}
