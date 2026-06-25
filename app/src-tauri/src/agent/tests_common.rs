//! Shared test helpers for the `agent` test suite (split out of
//! `tests.rs` on 2026-06-23). The 5 domain test files
//! (`tests_cancellation` / `tests_envelope` / `tests_prompts` /
//! `tests_agent_loop` / `tests_subagent`) reach these via
//! `use super::tests_common::*`.

#![cfg(test)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use sqlx::SqlitePool;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::agent::permissions::new_permission_store;
use crate::db;
use crate::llm::types::{ChatEvent, ChatMessage};
use crate::llm::{MessageContent, Role};
use crate::memory::MemoryCache;
use crate::skill::loader::SkillCache;
use crate::state::{ChatEventPayload, ChatEventSink, ToolCallPayload, ToolResultPayload};
use crate::tools::read_guard::ReadGuard;

/// Test ChatEventSink that records every emitted event into
/// a `Vec` for assertion. Mirrors the production
/// `AppHandleSink` (which forwards to `tauri::AppHandle::emit`)
/// but is in-process and inspectable.
///
/// Uses `std::sync::Mutex` (not `tokio::sync::Mutex`) for the
/// internal storage: the sink is only ever called from the agent
/// loop's emit sites, which never hold the lock across an `.await`.
/// `std::sync::Mutex` lets the test code call `.lock().unwrap()`
/// synchronously without pulling in `.await` plumbing.
#[derive(Default)]
pub(crate) struct MockEmitter {
    pub(crate) chat_events: Arc<StdMutex<Vec<ChatEventPayload>>>,
    pub(crate) tool_calls: Arc<StdMutex<Vec<ToolCallPayload>>>,
    pub(crate) tool_results: Arc<StdMutex<Vec<ToolResultPayload>>>,
    pub(crate) permission_asks:
        Arc<StdMutex<Vec<crate::agent::permissions::PermissionAskPayload>>>,
}

impl MockEmitter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Snapshot all chat-event payloads recorded so far.
    pub(crate) fn chat_events(&self) -> Vec<ChatEventPayload> {
        self.chat_events.lock().unwrap().clone()
    }

    /// Count of `Done` events with `stop_reason = Some("cancelled")`
    /// — the contract the cancel path uses to signal end-of-stream.
    pub(crate) fn cancel_done_count(&self) -> usize {
        self.chat_events
            .lock()
            .unwrap()
            .iter()
            .filter(|p| {
                matches!(&p.event, ChatEvent::Done { stop_reason, .. }
                    if stop_reason.as_deref() == Some("cancelled"))
            })
            .count()
    }

    /// Count of `Done` events with `stop_reason = Some("max_turns")`.
    pub(crate) fn max_turns_done_count(&self) -> usize {
        self.chat_events
            .lock()
            .unwrap()
            .iter()
            .filter(|p| {
                matches!(&p.event, ChatEvent::Done { stop_reason, .. }
                    if stop_reason.as_deref() == Some("max_turns"))
            })
            .count()
    }

    /// Count of `Error` chat-events.
    pub(crate) fn error_event_count(&self) -> usize {
        self.chat_events
            .lock()
            .unwrap()
            .iter()
            .filter(|p| matches!(&p.event, ChatEvent::Error { .. }))
            .count()
    }

    /// Number of `tool:call` events recorded.
    pub(crate) fn tool_call_count(&self) -> usize {
        self.tool_calls.lock().unwrap().len()
    }

    /// Number of `tool:result` events recorded.
    pub(crate) fn tool_result_count(&self) -> usize {
        self.tool_results.lock().unwrap().len()
    }

    /// Snapshot all `tool:result` payloads (content + is_error) — for
    /// asserting what the agent loop fed back to the LLM (e.g. a
    /// resolved skill body, or an "is_error" self-correction nudge).
    pub(crate) fn tool_results_snapshot(&self) -> Vec<ToolResultPayload> {
        self.tool_results.lock().unwrap().clone()
    }
}

impl ChatEventSink for MockEmitter {
    fn emit_chat_event(&self, payload: &ChatEventPayload) {
        self.chat_events.lock().unwrap().push(payload.clone());
    }
    fn emit_tool_call(&self, payload: &ToolCallPayload) {
        self.tool_calls.lock().unwrap().push(payload.clone());
    }
    fn emit_tool_result(&self, payload: &ToolResultPayload) {
        self.tool_results.lock().unwrap().push(payload.clone());
    }
    fn emit_permission_ask(&self, payload: crate::agent::permissions::PermissionAskPayload) {
        self.permission_asks.lock().unwrap().push(payload);
    }
}

async fn test_pool() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    db::migrations::run_migrations(&pool).await.unwrap();
    pool
}

/// Build a fresh AppState-equivalent for a test: in-memory DB +
/// empty cache + cancel maps. The test passes a pre-built
/// `Arc<MockProvider>` to `run_chat_loop` directly, bypassing
/// the catalog.
///
/// `project_id` / `project_path` are kept on the harness for
/// readability (callers can see what session they're talking to
/// via the named fields) even though no test reads them back —
/// the values are also stored in the DB row the harness inserts.
///
/// **Lifetime invariant**: the harness owns the `tempfile::TempDir`
/// guard (`_tempdir`) for the entire test. Without it, `make_harness`
/// returning would drop the guard and delete the on-disk directory
/// before `run_chat_loop`'s pre-flight `assert_within_root` could
/// `canonicalize()` it — that path (chat_loop.rs:173) returns Err
/// on a missing directory, the agent loop short-circuits with an
/// Error emit, `provider.send` is never called, and `call_count`
/// stays 0. The 6 FAILED + 1 hung test symptom in the first run
/// was exactly this regression. The leading underscore on
/// `_tempdir` is intentional — the value is never read, only
/// kept alive by being a struct field.
#[allow(dead_code)]
pub(crate) struct TestHarness {
    pub(crate) db: SqlitePool,
    pub(crate) project_id: String,
    pub(crate) project_path: std::path::PathBuf,
    pub(crate) session_id: String,
    pub(crate) cancellations: Arc<AsyncMutex<HashMap<String, CancellationToken>>>,
    pub(crate) session_active_request: Arc<AsyncMutex<HashMap<String, String>>>,
    pub(crate) read_guard: ReadGuard,
    pub(crate) memory_cache: Arc<MemoryCache>,
    pub(crate) skill_cache: Arc<SkillCache>,
    pub(crate) permission_asks: crate::agent::permissions::PermissionStore,
    /// L1a (2026-06-19): cross-request background-shell registry.
    /// Each test gets a fresh registry so concurrent tests can't
    /// see each other's shells. Threads through `run_chat_loop`'s
    /// new 15th parameter and is the same handle `ToolContext`
    /// hands to the 3 L1a tools.
    pub(crate) background_shells: crate::background_shell::DefaultRegistry,
    /// L3d (2026-06-25): subagent cache. Each test gets a fresh
    /// cache so the mtime fence + scan state can't leak across
    /// tests. Threads through `run_chat_loop`'s 25th parameter and
    /// is what `definition_with_cache` + `run_subagent` consult to
    /// resolve builtin + user + project subagents.
    pub(crate) subagent_cache: Arc<crate::agent::subagent::SubagentCache>,
    /// TempDir guard — kept alive for the duration of the test so
    /// the project_path directory remains on disk while the agent
    /// loop's pre-flight canonicalizes it. See struct docstring.
    pub(crate) _tempdir: tempfile::TempDir,
}

pub(crate) async fn make_harness() -> TestHarness {
    let pool = test_pool().await;
    // Create a project in the default "Legacy" bucket (the
    // migration's seed). We use a fresh path in the tempdir
    // so the worktree assertion (assert_within_root) succeeds
    // even though the path doesn't exist on disk for the
    // text-only / tool-execution-skipping tests.
    let dir = tempfile::tempdir().expect("tempdir");
    let project_path = dir.path().to_path_buf();
    db::create_project(
        &pool,
        "test-project",
        project_path.to_str().unwrap(),
        false,
        None,
    )
    .await
    .expect("create_project");
    // The project id is generated server-side; re-fetch.
    let projects = db::list_projects(&pool, false)
        .await
        .expect("list_projects");
    let project_id = projects
        .iter()
        .find(|p| p.path == project_path.to_string_lossy().to_string())
        .map(|p| p.id.clone())
        .expect("project should be present after create");

    let session_id = uuid::Uuid::new_v4().to_string();
    db::create_session(
        &pool,
        &session_id,
        &project_id,
        project_path.to_str().unwrap(),
        "mock-model",
        None,
    )
    .await
    .expect("create_session");

    TestHarness {
        db: pool,
        project_id,
        project_path,
        session_id,
        cancellations: Arc::new(AsyncMutex::new(HashMap::new())),
        session_active_request: Arc::new(AsyncMutex::new(HashMap::new())),
        read_guard: ReadGuard::new(),
        memory_cache: MemoryCache::arc(),
        skill_cache: SkillCache::arc(),
        permission_asks: new_permission_store(),
        background_shells: crate::background_shell::default_registry(),
        subagent_cache: crate::agent::subagent::SubagentCache::arc(),
        // Move the TempDir guard INTO the harness so it lives as
        // long as the harness (i.e. the whole test). Without this
        // move, `dir` drops at the end of `make_harness` and the
        // temp directory is deleted before `run_chat_loop` can
        // canonicalize it.
        _tempdir: dir,
    }
}

pub(crate) fn test_messages() -> Vec<ChatMessage> {
    vec![ChatMessage {
        role: Role::User,
        content: MessageContent::Text("hello".to_string()),
    }]
}
