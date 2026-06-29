//! P4 (autonomous memory, 2026-06-29): event-driven auto-write.
//!
//! Implements the **failure → success** side of spike-007 §3
//! (path 2 — 旁路 reflection). When the same tool fails ≥2
//! times in a row, then succeeds, this module fires a
//! fire-and-forget LLM reflection that distills the failure +
//! success transcript fragment into a `kind=pitfall,
//! status=active` memory row. The P3 pre-tool recall picks it
//! up on the next session, closing the "踩坑→记住→下次规避"
//! loop without the agent having to call `remember` explicitly.
//!
//! # Design
//!
//! - **In-session state machine** — `FailureTracker`, an
//!   `Arc<Mutex<HashMap<tool_name, Entry>>>` created at the
//!   top of `run_chat_loop` and dropped when the loop exits.
//!   v1 accepts session-boundary reset (spike-007 §10 "扩展位").
//!   No cross-session persistence.
//! - **Mount points** — `try_record_outcome` is invoked at
//!   the two tool-emit sites in `chat_loop.rs`:
//!   * parallel-batch L2 path (~line 1792, inside the
//!     `FuturesUnordered` task) — needs the tracker via `Arc`
//!     so it's `Send` across the task boundary.
//!   * serial path (~line 2361) — uses the same tracker via
//!     a `&`.
//!   Both sites only carry the (tool_name, is_error,
//!   tool_result content) tuple; the rest of the context
//!   (session_id, project_id, request_id, provider, db) is
//!   captured by the seam's surrounding closure and passed
//!   to the public function.
//! - **Reflection call** — fire-and-forget `tokio::spawn`
//!   with a fresh `provider.send(...)` carrying a *dedicated*
//!   reflection system prompt + a single user message holding
//!   the failure + success transcript fragment. Expects
//!   JSON: `{"title": "...", "content": "...",
//!   "trigger_key": {"tool": "...", "command_pattern":
//!   "...", "path_globs": [...]}}`. On parse failure / LLM
//!   error → `tracing::warn!` + return (no panic, no retry).
//! - **Write safety net** — every write goes through P1's
//!   `db::memories::insert_memory` so the safety net
//!   (sensitive content / sensitive path / length cap /
//!   frequency control via `count_memories_for_session`)
//!   applies uniformly. P4 **does not** add its own safety
//!   filters.
//! - **Scope** — `Project` only. The reflection always runs
//!   in some chat context with a `project_id`; emitting a
//!   user-scope memory from a project-bound reflection would
//!   be a layering bug (project pitfalls leak across
//!   projects).
//!
//! # What's NOT here
//!
//! - No LLM-judge writing filter (P5 scope).
//! - No state machine promotion (`candidate → active →
//!   verified`); P4 writes directly to `active` (spike-007
//!   §3 path 2 — event-driven writes are high-confidence).
//! - No dedup / hygiene job (P5 scope).
//! - No session-end overall reflection (spike-007 §8
//!   out-of-scope).

#![allow(dead_code)] // P4 lands the data flow + tests; P5 is the next caller, will be exercised end-to-end via the remember tool + chat_loop integration tests then.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::db::memories::{
    insert_memory, MemoryInput, MemoryKind, MemoryScope, MemoryStatus,
};
use crate::llm::{ChatMessage, MessageContent, Provider, Role};

// ---------------------------------------------------------------------------
// FailureTracker — per-session in-memory state machine
// ---------------------------------------------------------------------------

/// Number of consecutive failures before the next success
/// triggers a reflection. `consecutive_failures == 2` is the
/// "≥2 失败后成功" PRD acceptance threshold (spike-007 §3 path 2
/// + P4 PRD AC #3). Lower values would over-trigger on
/// benign one-off failures (e.g. network blip on a single
/// curl).
pub const REFLECTION_FAILURE_THRESHOLD: u32 = 2;

/// What we record per-tool to feed the reflection prompt
/// when the threshold is hit. Captures both the LAST failure
/// (its content) and the success (its content) so the
/// reflection LLM has the full "what failed → what worked"
/// context to distill a pitfall from.
///
/// `last_failure_input` is also captured (separately from
/// the `last_failure_content`) so the reflection can spot
/// the *trigger* (the user's tool_input shape that produced
/// the failures) without re-parsing the success content.
#[derive(Debug, Clone, Default)]
struct TrackerEntry {
    consecutive_failures: u32,
    last_failure_input: Option<serde_json::Value>,
    last_failure_content: Option<String>,
}

/// Per-session failure tracker. Public only inside the
/// `agent` module (chat_loop.rs reaches it via
/// `crate::agent::auto_reflect`); the public API is the
/// `try_record_outcome` free function, which takes a
/// `&Arc<Mutex<FailureTracker>>` so the chat loop can share
/// the same instance across both seam sites (parallel + serial)
/// within one `run_chat_loop` invocation.
#[derive(Debug, Default)]
pub struct FailureTracker {
    by_tool: HashMap<String, TrackerEntry>,
}

impl FailureTracker {
    /// Build a fresh empty tracker. v1 always resets on
    /// session start (no cross-session memory of which
    /// tools were "flaky yesterday" — v2 might add a
    /// persistent low-confidence hint).
    pub fn new() -> Self {
        Self::default()
    }

    /// Inspect a tool's current consecutive-failure count.
    /// Test-only — production reads through
    /// `try_record_outcome`.
    #[cfg(test)]
    pub fn count(&self, tool_name: &str) -> u32 {
        self.by_tool
            .get(tool_name)
            .map(|e| e.consecutive_failures)
            .unwrap_or(0)
    }

    /// Record one tool_use outcome. On success after ≥
    /// `REFLECTION_FAILURE_THRESHOLD` consecutive failures,
    /// return `Some(Trigger)` — the caller is then expected
    /// to fire `reflect_to_pitfall` with that `Trigger`'s
    /// captured context. Returns `None` in all other
    /// conditions (single failure / success without
    /// preceding failures / consecutive failures < threshold
    /// followed by another failure).
    ///
    /// **Side effect on success path**: clears the tool's
    /// counter so the next failure starts a fresh streak.
    /// The PRD doesn't require this — a "double success"
    /// (success → fail → success → fail → success) would
    /// also fire a reflection on the second success. The
    /// "clear on success" choice is conservative: each
    /// success is a fresh "I just made it work" anchor, and
    /// the next failure streak is its own diagnostic unit.
    async fn record(
        &mut self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        is_error: bool,
        result_content: &str,
    ) -> Option<Trigger> {
        let entry = self.by_tool.entry(tool_name.to_string()).or_default();

        if is_error {
            // Bump the counter; record the latest failure
            // context (so the most recent failure wins in
            // the reflection prompt — the older failure is
            // usually a less-informative duplicate).
            entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
            entry.last_failure_input = Some(tool_input.clone());
            entry.last_failure_content = Some(result_content.to_string());
            None
        } else {
            // Success path. Check threshold BEFORE clearing
            // so the trigger has access to the recorded
            // failure context.
            if entry.consecutive_failures >= REFLECTION_FAILURE_THRESHOLD {
                let trigger = Trigger {
                    tool_name: tool_name.to_string(),
                    failure_count: entry.consecutive_failures,
                    last_failure_input: entry
                        .last_failure_input
                        .take()
                        .unwrap_or_else(|| serde_json::Value::Null),
                    last_failure_content: entry
                        .last_failure_content
                        .take()
                        .unwrap_or_default(),
                    success_input: tool_input.clone(),
                    success_content: result_content.to_string(),
                };
                // Clear the entry so the next failure starts
                // a fresh streak. The Trigger has already
                // cloned what it needs.
                entry.consecutive_failures = 0;
                entry.last_failure_input = None;
                entry.last_failure_content = None;
                Some(trigger)
            } else {
                // Success without enough preceding failures
                // — clear the counter (a successful op
                // resets the diagnostic clock).
                entry.consecutive_failures = 0;
                entry.last_failure_input = None;
                entry.last_failure_content = None;
                None
            }
        }
    }
}

/// What `FailureTracker::record` returns on the "≥2 fails
/// then success" path. The chat loop feeds this into
/// `reflect_to_pitfall` (via a `tokio::spawn` so the main
/// loop stays sync-fast).
#[derive(Debug, Clone)]
pub struct Trigger {
    pub tool_name: String,
    pub failure_count: u32,
    pub last_failure_input: serde_json::Value,
    pub last_failure_content: String,
    pub success_input: serde_json::Value,
    pub success_content: String,
}

// ---------------------------------------------------------------------------
// Public entry point — chat_loop calls this at the two seams
// ---------------------------------------------------------------------------

/// Record one tool_use outcome for the in-session failure
/// tracker. On the "≥2 fails then success" pattern, this
/// function **fires a fire-and-forget reflection** via
/// `tokio::spawn` — the calling loop never blocks.
///
/// **The tracker is updated under the lock; the reflection
/// itself runs outside the lock** (so a slow LLM call
/// doesn't hold the tracker mutex). This matches the P3
/// `recall_pitfall_footnote` pattern (DB write off-lock).
///
/// **All errors are absorbed** (`tracing::warn!` + return).
/// The PRD's hard rule: "旁路 reflection 异步、不阻塞主 loop;
/// 失败一律 `tracing::warn!` + 静默吞;**不** panic /
/// `.expect()` / `unwrap()`". The chat loop's main flow
/// never observes a failure here.
///
/// `failure_tracker` is an `Arc<Mutex<FailureTracker>>` so
/// the two seam sites (parallel + serial) can share the
/// same per-session state.
pub async fn try_record_outcome(
    failure_tracker: &Arc<Mutex<FailureTracker>>,
    provider: Arc<dyn Provider>,
    pool: SqlitePool,
    request_id: &str,
    session_id: &str,
    project_id: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
    is_error: bool,
    result_content: &str,
) {
    // Step 1: update the tracker; if the pattern doesn't
    // match, return immediately. Holding the lock across
    // this check is fine — the work is O(1) (HashMap
    // entry), and the reflection spawn happens AFTER
    // `release`.
    let trigger = {
        let mut guard = failure_tracker.lock().await;
        guard
            .record(tool_name, tool_input, is_error, result_content)
            .await
    };

    let Some(trigger) = trigger else {
        return;
    };

    // Step 2: fire-and-forget reflection. We do NOT await
    // this — the chat loop's hot path stays sync-fast. All
    // failures (LLM error, JSON parse, DB error) are
    // absorbed inside the spawned task.
    let request_id = request_id.to_string();
    let session_id = session_id.to_string();
    let project_id = project_id.to_string();
    let tool_name_owned = tool_name.to_string();
    tokio::spawn(async move {
        if let Err(e) = reflect_to_pitfall(
            provider,
            pool,
            &request_id,
            &session_id,
            &project_id,
            &trigger,
        )
        .await
        {
            tracing::warn!(
                request_id = %request_id,
                session_id = %session_id,
                project_id = %project_id,
                tool = %tool_name_owned,
                failure_count = trigger.failure_count,
                error = %e,
                "P4 auto_reflect: reflection failed (non-fatal)"
            );
        }
    });
}

// ---------------------------------------------------------------------------
// Reflection LLM call + insert
// ---------------------------------------------------------------------------

/// Errors from the reflection pipeline. All variants are
/// absorbed by the caller (`try_record_outcome`'s
/// `tokio::spawn` body) — they're logged, not propagated.
/// The enum exists for testability and structured
/// `tracing::warn!` output.
#[derive(Debug, thiserror::Error)]
pub enum ReflectError {
    #[error("LLM call failed: {0}")]
    Llm(String),
    #[error("LLM returned no text content")]
    NoText,
    #[error("LLM JSON parse failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("LLM JSON missing required fields: {0}")]
    MissingField(&'static str),
    #[error("P1 insert_memory rejected: {0}")]
    Insert(String),
}

/// System prompt for the reflection call. Kept tiny + rigid
/// (JSON-only output, fixed schema) so a single streaming
/// response is cheap to validate. **Independent of the
/// main loop's `DEFAULT_BEHAVIOR_PROMPT`** — the main
/// behavior prompt would dilute the reflection's focus
/// (we want a structured pitfall, not a chatty answer).
const REFLECT_SYSTEM_PROMPT: &str = "\
You distill tool-failure-then-success patterns into reusable pitfalls for an AI agent.
Given a transcript fragment where the same tool failed N times then succeeded, \
output a single JSON object capturing the lesson. Be terse: title ≤12 words, \
content ≤1 sentence. The content should describe what to watch out for (imperative) \
and the trigger_key should be a substring or glob that, if seen again, would identify \
a similar failure context. Output JSON only — no prose, no markdown fences.";

/// User-message template for the reflection call. The
/// `{failure_content}` and `{success_content}` slots are
/// populated verbatim from the tool_result strings
/// (truncated to keep the prompt small). The trigger_key
/// field is the only optional one — set it to `null` if
/// the pattern is too general to encode as a substring.
const REFLECT_USER_TEMPLATE: &str = "\
A tool failed {failure_count} times in a row, then succeeded. \
Distill this into a reusable pitfall memory.

Tool name: {tool_name}
Tool input (the trigger): {tool_input}
Last failure output: {last_failure}
Successful call input: {success_input}
Successful call output: {success}

Output JSON of shape:
{{
  \"title\": \"<≤12 words, imperative>\",
  \"content\": \"<≤1 sentence: what to watch out for + how to avoid>\",
  \"trigger_key\": {{
    \"tool\": \"{tool_name}\",
    \"command_pattern\": \"<substring that would re-identify a similar failure, or null>\",
    \"path_globs\": [<glob strings that limit the trigger, or empty array>]
  }}
}}";

/// Truncate a long tool_result for the reflection prompt.
/// 2000 chars is plenty to convey the failure shape
/// (most failures are <500 chars) and keeps the reflection
/// call's input cost bounded.
const REFLECT_TRANSCRIPT_TRUNCATE: usize = 2000;

fn truncate_for_reflect(s: &str) -> String {
    if s.len() <= REFLECT_TRANSCRIPT_TRUNCATE {
        s.to_string()
    } else {
        let mut out = s
            .chars()
            .take(REFLECT_TRANSCRIPT_TRUNCATE)
            .collect::<String>();
        out.push_str("\n...<truncated>");
        out
    }
}

/// Build the user message body for the reflection call.
fn build_reflect_user_message(trigger: &Trigger) -> String {
    let tool_input_str = serde_json::to_string(&trigger.last_failure_input)
        .unwrap_or_else(|_| "<unparseable>".to_string());
    let success_input_str = serde_json::to_string(&trigger.success_input)
        .unwrap_or_else(|_| "<unparseable>".to_string());
    REFLECT_USER_TEMPLATE
        .replace("{failure_count}", &trigger.failure_count.to_string())
        .replace("{tool_name}", &trigger.tool_name)
        .replace("{tool_input}", &tool_input_str)
        .replace("{last_failure}", &truncate_for_reflect(&trigger.last_failure_content))
        .replace("{success_input}", &success_input_str)
        .replace("{success}", &truncate_for_reflect(&trigger.success_content))
}

/// LLM's expected JSON shape. All fields are required;
/// `command_pattern` and `path_globs` are nullable /
/// empty-able. We use `Deserialize` directly to keep the
/// parse error path typed.
#[derive(Debug, Deserialize)]
struct ReflectOutput {
    title: String,
    content: String,
    trigger_key: ReflectTriggerKey,
}

#[derive(Debug, Deserialize)]
struct ReflectTriggerKey {
    tool: String,
    command_pattern: Option<String>,
    path_globs: Option<Vec<String>>,
}

/// Run the full reflection pipeline: build prompt → call
/// LLM → parse JSON → `insert_memory`. All errors are
/// surfaced as `ReflectError` (the caller's `tokio::spawn`
/// body logs them at `warn!` level).
async fn reflect_to_pitfall(
    provider: Arc<dyn Provider>,
    pool: SqlitePool,
    request_id: &str,
    session_id: &str,
    project_id: &str,
    trigger: &Trigger,
) -> Result<(), ReflectError> {
    use futures_util::StreamExt;

    let user_message = build_reflect_user_message(trigger);
    let messages = vec![ChatMessage {
        role: Role::User,
        content: MessageContent::Text(user_message),
    }];

    // Empty tool list — the reflection is a single text
    // response, not a multi-step agentic call. The LLM
    // must not be able to invoke other tools from this
    // prompt.
    let stream = provider.send(
        Some(REFLECT_SYSTEM_PROMPT.to_string()),
        messages,
        vec![],
    );

    // Drain the stream into a single text blob. We don't
    // care about `Done`'s `usage` or `Stop` reason — only
    // the accumulated text. Cancellation is implicit: if
    // the spawn is dropped (session ends), the future is
    // dropped and the stream is dropped. No
    // `CancellationToken` plumbing needed for this
    // fire-and-forget call.
    let mut text = String::new();
    let mut stream = Box::pin(stream);
    while let Some(event) = stream.next().await {
        match event {
            Ok(crate::llm::ChatEvent::Delta { text: chunk }) => {
                text.push_str(&chunk);
            }
            Ok(crate::llm::ChatEvent::Error { message, .. }) => {
                return Err(ReflectError::Llm(message));
            }
            // Thinking deltas / signatures / redacted
            // thinking — ignore (the reflection output is
            // text-only).
            Ok(_) => {}
            Err(e) => {
                return Err(ReflectError::Llm(e.to_string()));
            }
        }
    }

    if text.trim().is_empty() {
        return Err(ReflectError::NoText);
    }

    // Strip optional markdown fences the LLM might add
    // despite the "JSON only" instruction. Be lenient
    // here — production traffic is real, and a fence
    // strip costs nothing.
    let json_text = strip_code_fence(&text);

    let parsed: ReflectOutput = serde_json::from_str(json_text)
        .map_err(ReflectError::Json)?;

    if parsed.title.trim().is_empty() {
        return Err(ReflectError::MissingField("title"));
    }
    if parsed.content.trim().is_empty() {
        return Err(ReflectError::MissingField("content"));
    }
    if parsed.trigger_key.tool.trim().is_empty() {
        return Err(ReflectError::MissingField("trigger_key.tool"));
    }

    // The LLM might not respect the `command_pattern`
    // field's "substring of the original command"
    // contract — but P1's `find_pitfalls_by_trigger`
    // substring-matches against the LLM's value, so the
    // worst case is a slightly off pitfall. We pass it
    // through verbatim.

    let path_globs_json = match &parsed.trigger_key.path_globs {
        Some(globs) if !globs.is_empty() => {
            Some(serde_json::to_string(globs).unwrap_or_else(|_| "[]".to_string()))
        }
        _ => None,
    };

    // source_ref carries the tool_call_id (we don't have
    // one at this layer; use the request_id + tool_name
    // as a stable locator) so a future hygiene job can
    // trace back to the original tool_use.
    let source_ref = format!("{}:{}", request_id, trigger.tool_name);

    let input = MemoryInput {
        scope: MemoryScope::Project,
        project_id: Some(project_id.to_string()),
        kind: MemoryKind::Pitfall,
        status: MemoryStatus::Active, // P4 writes directly to active (spike-007 §3 path 2)
        title: parsed.title,
        content: parsed.content,
        tags: "[]".to_string(),
        tool_name: Some(parsed.trigger_key.tool),
        command_pattern: parsed.trigger_key.command_pattern,
        path_globs: path_globs_json,
        source_session_id: Some(session_id.to_string()),
        source_ref: Some(source_ref),
    };

    insert_memory(&pool, &input)
        .await
        .map(|_row| ())
        .map_err(|e| match e {
            crate::db::memories::MemoryInsertError::Db(db_err) => {
                ReflectError::Insert(db_err.to_string())
            }
            other => ReflectError::Insert(other.to_string()),
        })
}

/// Strip leading/trailing markdown code fences from a
/// response string. Handles ````json ... ```` and ```` ... ````
/// variants. Conservative: only strips if the entire
/// response is wrapped in a fence; if the fence is partial
/// or inline, the JSON parse will fail and produce a
/// typed `Json` error.
fn strip_code_fence(s: &str) -> &str {
    let trimmed = s.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        // Drop the optional language tag.
        let after_lang = if let Some(newline_pos) = rest.find('\n') {
            &rest[newline_pos + 1..]
        } else {
            rest
        };
        if let Some(stripped) = after_lang.strip_suffix("```") {
            return stripped.trim();
        }
    }
    trimmed
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::memories::test_helpers::insert_raw;
    use crate::db::memories::{MemoryKind, MemoryScope, MemoryStatus};
    use crate::llm::provider::mock::{MockProvider, MockResponse};
    use serde_json::json;

    async fn make_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        crate::db::migrations::run_migrations(&pool).await.unwrap();
        pool
    }

    fn shell_input(command: &str) -> serde_json::Value {
        json!({"command": command})
    }

    /// `record()` on a brand-new tracker: a single failure
    /// does NOT trigger — the threshold is 2 (PRD AC #3).
    #[tokio::test]
    async fn single_failure_does_not_trigger() {
        let mut t = FailureTracker::new();
        let r = t
            .record("shell", &shell_input("cargo test"), true, "error A")
            .await;
        assert!(r.is_none(), "1 failure must not trigger");
        assert_eq!(t.count("shell"), 1);
    }

    /// 2 consecutive failures + success triggers with the
    /// captured context (PRD AC #1).
    #[tokio::test]
    async fn two_failures_then_success_triggers() {
        let mut t = FailureTracker::new();
        // First failure.
        let r1 = t
            .record("shell", &shell_input("cargo test"), true, "error: PKG_CONFIG_PATH")
            .await;
        assert!(r1.is_none());
        // Second failure.
        let r2 = t
            .record("shell", &shell_input("cargo test"), true, "error: PKG_CONFIG_PATH")
            .await;
        assert!(r2.is_none());
        // Success.
        let r3 = t
            .record(
                "shell",
                &shell_input("PKG_CONFIG_PATH=... cargo test"),
                false,
                "test result: ok",
            )
            .await;
        let trigger = r3.expect("success after 2 failures must trigger");
        assert_eq!(trigger.tool_name, "shell");
        assert_eq!(trigger.failure_count, 2);
        assert!(trigger.last_failure_content.contains("PKG_CONFIG_PATH"));
        assert!(trigger.success_content.contains("test result: ok"));
    }

    /// Success on first call → no trigger, counter stays
    /// at 0 (no false positive on a clean run).
    #[tokio::test]
    async fn first_call_success_does_not_trigger() {
        let mut t = FailureTracker::new();
        let r = t
            .record("shell", &shell_input("ls"), false, "")
            .await;
        assert!(r.is_none());
        assert_eq!(t.count("shell"), 0);
    }

    /// Success after exactly 1 failure does NOT trigger
    /// (PRD AC #3 — "需连续 ≥2").
    #[tokio::test]
    async fn one_failure_then_success_does_not_trigger() {
        let mut t = FailureTracker::new();
        t.record("shell", &shell_input("cargo test"), true, "err")
            .await;
        let r = t
            .record("shell", &shell_input("cargo test"), false, "ok")
            .await;
        assert!(
            r.is_none(),
            "1 failure + success must NOT trigger (threshold is 2)"
        );
    }

    /// After a trigger fires, the counter resets — a
    /// follow-up single failure + success does NOT
    /// re-trigger.
    #[tokio::test]
    async fn counter_resets_after_trigger() {
        let mut t = FailureTracker::new();
        // First failure-then-success cycle.
        t.record("shell", &shell_input("a"), true, "f1").await;
        t.record("shell", &shell_input("a"), true, "f2").await;
        let triggered = t
            .record("shell", &shell_input("a"), false, "ok1")
            .await;
        assert!(triggered.is_some());

        // After the trigger, the counter is reset.
        assert_eq!(t.count("shell"), 0);

        // A follow-up single failure + success does NOT
        // re-trigger (we're back at 0, not at 1).
        t.record("shell", &shell_input("a"), true, "f3").await;
        let r = t
            .record("shell", &shell_input("a"), false, "ok2")
            .await;
        assert!(
            r.is_none(),
            "post-trigger counter must reset (1 fail + success is below threshold)"
        );
    }

    /// Different tools have independent counters — a
    /// `shell` failure does not bleed into a `grep`
    /// counter.
    #[tokio::test]
    async fn tools_have_independent_counters() {
        let mut t = FailureTracker::new();
        t.record("shell", &shell_input("a"), true, "err").await;
        t.record("shell", &shell_input("a"), true, "err").await;
        // `grep` only failed once — should NOT trigger.
        let r = t
            .record("grep", &shell_input("b"), false, "ok")
            .await;
        assert!(r.is_none(), "grep counter is independent of shell");
        // `shell` triggers.
        let r2 = t
            .record("shell", &shell_input("a"), false, "ok")
            .await;
        assert!(r2.is_some(), "shell counter hit 2 fails + success");
    }

    fn make_text_provider(text: &str) -> Arc<MockProvider> {
        Arc::new(MockProvider::new(vec![MockResponse::Events(vec![
            Ok(crate::llm::ChatEvent::Start),
            Ok(crate::llm::ChatEvent::Delta { text: text.to_string() }),
            Ok(crate::llm::ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(crate::llm::types::TokenUsage::default()),
            }),
        ])]))
    }

    /// `try_record_outcome` with a MockProvider that
    /// returns a valid JSON: end-to-end the row is
     /// inserted (PRD AC #1 + #4).
    #[tokio::test]
    async fn try_record_outcome_writes_active_pitfall_end_to_end() {
        let pool = make_pool().await;
        let provider = make_text_provider(
            r#"{
                "title": "WSL cargo test needs PKG_CONFIG_PATH",
                "content": "set PKG_CONFIG_PATH before cargo test on WSL",
                "trigger_key": {
                    "tool": "shell",
                    "command_pattern": "cargo test",
                    "path_globs": []
                }
            }"#,
        );

        let tracker = Arc::new(Mutex::new(FailureTracker::new()));

        // 2 failures.
        try_record_outcome(
            &tracker,
            provider.clone(),
            pool.clone(),
            "req-1",
            "sess-1",
            "proj-1",
            "shell",
            &shell_input("cargo test"),
            true,
            "error: PKG_CONFIG_PATH",
        )
        .await;
        try_record_outcome(
            &tracker,
            provider.clone(),
            pool.clone(),
            "req-1",
            "sess-1",
            "proj-1",
            "shell",
            &shell_input("cargo test"),
            true,
            "error: PKG_CONFIG_PATH",
        )
        .await;
        // 1 success → should fire the reflection.
        try_record_outcome(
            &tracker,
            provider.clone(),
            pool.clone(),
            "req-1",
            "sess-1",
            "proj-1",
            "shell",
            &shell_input("PKG_CONFIG_PATH=... cargo test"),
            false,
            "test result: ok",
        )
        .await;

        // The reflection is fire-and-forget — give the
        // spawned task a moment to complete and write the
        // row.
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM autonomous_memories WHERE memory_id IS NOT NULL",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if count > 0 {
                break;
            }
        }

        // Verify the row landed with the expected shape.
        let row: (String, String, String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT kind, status, title, tool_name, command_pattern \
             FROM autonomous_memories WHERE title = 'WSL cargo test needs PKG_CONFIG_PATH'",
        )
        .fetch_one(&pool)
        .await
        .expect("pitfall row must be inserted");

        assert_eq!(row.0, "pitfall");
        assert_eq!(row.1, "active"); // P4 writes directly to active
        assert_eq!(row.2, "WSL cargo test needs PKG_CONFIG_PATH");
        assert_eq!(row.3.as_deref(), Some("shell"));
        assert_eq!(row.4.as_deref(), Some("cargo test"));
    }

    /// When the LLM emits invalid JSON, the reflection
    /// fails silently (no panic, no row written) — PRD
    /// acceptance: "旁路 reflection 异步、不阻塞主 loop;
    /// 失败一律 `tracing::warn!` + 静默吞".
    #[tokio::test]
    async fn invalid_json_from_llm_does_not_panic_or_write() {
        let pool = make_pool().await;
        let provider = make_text_provider("not json at all");

        let tracker = Arc::new(Mutex::new(FailureTracker::new()));
        try_record_outcome(
            &tracker,
            provider.clone(),
            pool.clone(),
            "req-1",
            "sess-1",
            "proj-1",
            "shell",
            &shell_input("cargo test"),
            true,
            "err",
        )
        .await;
        try_record_outcome(
            &tracker,
            provider.clone(),
            pool.clone(),
            "req-1",
            "sess-1",
            "proj-1",
            "shell",
            &shell_input("cargo test"),
            true,
            "err",
        )
        .await;
        try_record_outcome(
            &tracker,
            provider.clone(),
            pool.clone(),
            "req-1",
            "sess-1",
            "proj-1",
            "shell",
            &shell_input("cargo test"),
            false,
            "ok",
        )
        .await;

        // Wait a beat for the spawned task to (fail and)
        // return.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM autonomous_memories")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0, "no row should be written on JSON parse failure");
    }

    /// `try_record_outcome` does NOT block the caller —
    /// even when the LLM is slow / never returns, the
    /// public function returns within a few ms (it's
    /// fire-and-forget).
    #[tokio::test]
    async fn try_record_outcome_does_not_block_caller() {
        let pool = make_pool().await;
        // A provider that never produces an event (the
        // task will be dropped when the test ends). The
        // public function must still return promptly.
        let provider = Arc::new(MockProvider::new(vec![MockResponse::HangingThenCancel]));
        let tracker = Arc::new(Mutex::new(FailureTracker::new()));

        let start = std::time::Instant::now();
        try_record_outcome(
            &tracker,
            provider.clone(),
            pool.clone(),
            "req-1",
            "sess-1",
            "proj-1",
            "shell",
            &shell_input("cargo test"),
            true,
            "err",
        )
        .await;
        try_record_outcome(
            &tracker,
            provider.clone(),
            pool.clone(),
            "req-1",
            "sess-1",
            "proj-1",
            "shell",
            &shell_input("cargo test"),
            true,
            "err",
        )
        .await;
        try_record_outcome(
            &tracker,
            provider.clone(),
            pool.clone(),
            "req-1",
            "sess-1",
            "proj-1",
            "shell",
            &shell_input("cargo test"),
            false,
            "ok",
        )
        .await;
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 100,
            "try_record_outcome must not block (fire-and-forget); took {:?}",
            elapsed
        );

        // Cancel the spawned task to clean up (HangingThenCancel
        // exits on the agent-loop's cancellation token; since
        // we don't have one, drop the future by ending the
        // test). The mock's hanging future is leaked-but-harmless
        // when the test process exits.
    }

    /// `strip_code_fence` handles the common LLM
    /// responses: bare JSON, fenced JSON (with and
    /// without language tag).
    #[test]
    fn strip_code_fence_handles_common_cases() {
        assert_eq!(strip_code_fence(r#"{"a":1}"#), r#"{"a":1}"#);
        assert_eq!(
            strip_code_fence("```json\n{\"a\":1}\n```"),
            "{\"a\":1}"
        );
        assert_eq!(
            strip_code_fence("```\n{\"a\":1}\n```"),
            "{\"a\":1}"
        );
    }

    /// `truncate_for_reflect` leaves short strings alone
    /// and caps long ones with a marker.
    #[test]
    fn truncate_for_reflect_under_cap_passes_through() {
        let s = "short";
        assert_eq!(truncate_for_reflect(s), "short");
    }

    #[test]
    fn truncate_for_reflect_over_cap_appends_marker() {
        let s = "a".repeat(REFLECT_TRANSCRIPT_TRUNCATE + 100);
        let out = truncate_for_reflect(&s);
        assert!(out.ends_with("...<truncated>"));
        assert!(out.len() < s.len());
    }

    /// End-to-end: a pitfall inserted via the reflection
    /// pipeline can be recalled by the P3 helper
    /// (PRD AC #4 — closed loop).
    #[tokio::test]
    async fn reflected_pitfall_is_recallable_by_p3_helper() {
        use crate::agent::permissions::check::recall_pitfall_footnote;

        let pool = make_pool().await;
        // Insert a pitfall as if P4 had produced it.
        insert_raw(
            &pool,
            "p4-pitfall",
            MemoryScope::Project,
            Some("proj-1"),
            MemoryKind::Pitfall,
            MemoryStatus::Active,
            "WSL cargo test needs PKG_CONFIG_PATH",
            "set PKG_CONFIG_PATH before cargo test",
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE autonomous_memories SET tool_name='shell', command_pattern='cargo test' \
             WHERE memory_id='p4-pitfall'",
        )
        .execute(&pool)
        .await
        .unwrap();

        let footnote = recall_pitfall_footnote(
            &pool,
            "shell",
            &json!({"command": "cargo test --lib"}),
        )
        .await
        .expect("recall must succeed on healthy pool");
        let text = footnote.expect("P3 helper must hit the active pitfall");
        assert!(text.contains("WSL cargo test needs PKG_CONFIG_PATH"));
    }
}
