//! Shared test helpers for the `agent` test suite (split out of
//! `tests.rs` on 2026-06-23). The 5 domain test files
//! (`tests_cancellation` / `tests_envelope` / `tests_prompts` /
//! `tests_agent_loop` / `tests_subagent`) reach these via
//! `use super::tests_common::*`.

#![cfg(test)]

use std::collections::HashMap;
use std::path::Path;
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use sqlx::SqlitePool;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::agent::permissions::new_permission_store;
use crate::db;
use crate::db::test_support::test_pool;
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
    pub(crate) permission_asks: Arc<StdMutex<Vec<crate::agent::permissions::PermissionAskPayload>>>,
    /// 2026-06-30 (`ask_user_question` task): captured
    /// `ToolQuestionPayload`s emitted to the `tool:question`
    /// channel. Tests assert against this to confirm the IPC
    /// emit happened (mirrors the `permission_asks` pattern).
    pub(crate) tool_questions:
        Arc<StdMutex<Vec<crate::agent::question_store::ToolQuestionPayload>>>,
}

impl MockEmitter {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Snapshot all `tool:question` payloads (the
    /// `ToolQuestionPayload`s emitted to the `tool:question`
    /// channel). Tests assert against this to confirm the IPC
    /// emit happened. Mirrors `permission_asks` snapshot.
    /// Marked `#[allow(dead_code)]` because Phase A's
    /// happy-path tests don't yet exercise it (Phase F's
    /// `agent_loop_ask_user_question_*` tests will).
    #[allow(dead_code)]
    pub(crate) fn tool_questions_snapshot(
        &self,
    ) -> Vec<crate::agent::question_store::ToolQuestionPayload> {
        self.tool_questions.lock().unwrap().clone()
    }

    #[allow(dead_code)]
    pub(crate) fn tool_question_count(&self) -> usize {
        self.tool_questions.lock().unwrap().len()
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
    fn emit_tool_question(&self, payload: &crate::agent::question_store::ToolQuestionPayload) {
        // 2026-06-30: record the `tool:question` IPC payload so
        // tests can assert "the ask_user_question tool was
        // emitted on the channel". Mirrors `emit_permission_ask`.
        self.tool_questions.lock().unwrap().push(payload.clone());
    }
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
    /// 2026-06-30 (`ask_user_question` task): fresh
    /// `QuestionStore` per test for isolation. Threads through
    /// `run_chat_loop`'s new `question_store` parameter so the
    /// `ask_user_question` blocking tool's
    /// `register` / `resolve` / `get_payload` calls operate on a
    /// per-test registry (no cross-test leak).
    pub(crate) question_store: crate::agent::question_store::QuestionStore,
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
    /// L3b (2026-06-27): app data dir for worker worktree path
    /// computation. A fresh tempdir per test so isolated worker
    /// worktrees (when a test exercises isolation) don't collide
    /// across tests. Threads through `run_chat_loop`'s 28th
    /// parameter (`app_data_dir`). Tests that don't exercise
    /// isolation (most) never read this — the tempdir just exists
    /// alongside the project tempdir and is cleaned up on drop.
    pub(crate) app_data_dir: std::path::PathBuf,
    /// D (2026-08-14, `08-14-c7d-tools-stub-registration`): fresh
    /// stub loaded-set registry per test for isolation (no
    /// cross-test loaded-set leak). Threads through `run_chat_loop`'s
    /// trailing `stub_loaded` parameter; tests that exercise the
    /// stub interception reach into it to assert loaded-set writes.
    pub(crate) stub_loaded: std::sync::Arc<crate::tools::stub::StubRegistry>,
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
        .find(|p| p.path == project_path.to_string_lossy())
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
        None,
        None,
    )
    .await
    .expect("create_session");

    // D (2026-08-14, `08-14-c7d-tools-stub-registration`): 既有测试
    // 默认**关** stub — 它们用 MockProvider 让模型直呼
    // `update_checklist` / `request_mode_change` / background-shell
    // 等候选工具并断言真实执行,若缺省开(生产语义),直呼自愈拦截
    // 会把它们当 stub 直呼劫持成 error。stub 专项测试显式
    // `set_config_value("tools_stub_enabled", "true")` 覆盖本默认。
    db::config::set_config_value(&pool, "tools_stub_enabled", "false")
        .await
        .expect("set tools_stub_enabled=false");

    // C3 摘要压缩 PR2 (08-18-llm-context-compaction): 既有测试默认
    // **关** LLM 摘要压缩 — 摘要路径会先于主 turn 额外消费一次
    // MockProvider script 条目并折叠 messages,既有测试的
    // call_count / sent_messages 断言会全数漂移。机械丢组路径
    // (compact_messages)不受此开关影响,既有 C3 测试照常。摘要
    // 专项测试(tests_agent_loop/compaction_summary.rs)显式
    // `set_config_value("llm_compaction_enabled", "true")` 覆盖本默认
    // (同 stub 先例)。
    db::config::set_config_value(&pool, "llm_compaction_enabled", "false")
        .await
        .expect("set llm_compaction_enabled=false");

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
        // 2026-06-30: fresh `QuestionStore` per test (parallel to
        // `permission_asks`). Tests that exercise
        // `ask_user_question` (Phase F, not Phase A) reach into
        // `harness.question_store` to register / resolve.
        question_store: crate::agent::question_store::QuestionStore::new(),
        background_shells: crate::background_shell::default_registry(),
        subagent_cache: crate::agent::subagent::SubagentCache::arc(),
        // D (2026-08-14): fresh stub loaded-set registry per test.
        stub_loaded: std::sync::Arc::new(crate::tools::stub::StubRegistry::new()),
        // L3b (2026-06-27): fresh tempdir for the app data dir.
        // Worker worktrees (when a test exercises isolation) land
        // under `<app_data_dir>/worktrees/<project_uuid>/worker/
        // <run_id>`. Tests that don't exercise isolation never
        // touch this; the tempdir just exists for uniformity.
        app_data_dir: tempfile::tempdir()
            .expect("app_data_dir tempdir")
            .path()
            .to_path_buf(),
        // Move the TempDir guard INTO the harness so it lives as
        // long as the harness (i.e. the whole test). Without this
        // move, `dir` drops at the end of `make_harness` and the
        // temp directory is deleted before `run_chat_loop` can
        // canonicalize it.
        _tempdir: dir,
    }
}

/// L3b PR1 (2026-06-27) + PR2 (2026-06-27): variant of
/// `make_harness` that also `git init`s the project tempdir so
/// worker worktree creation succeeds. Used by tests that exercise
/// isolation (`general-purpose` with default `isolation: Some(true)`
/// → calls `create_worker` → fails without `.git/`).
///
/// The git repo is initialized with `--initial-branch=main` + a
/// test user + a seed commit so `create_worker` has a base commit
/// to branch from (the worker's `worker/<run_id>` branch is based
/// off the project HEAD). The seed file is `seed.txt` (arbitrary
/// non-empty content) so `commit_all_for_test` has something to
/// track.
pub(crate) async fn make_harness_with_git_repo() -> TestHarness {
    let harness = make_harness().await;
    init_repo_for_test(&harness.project_path);
    // Seed a tracked file + initial commit so `create_worker`
    // (which branches off HEAD) has a base commit to start from.
    std::fs::write(harness.project_path.join("seed.txt"), "seed").unwrap();
    commit_all_for_test(&harness.project_path, "init");
    harness
}

pub(crate) fn test_messages() -> Vec<ChatMessage> {
    vec![ChatMessage {
        role: Role::User,
        content: MessageContent::Text("hello".to_string()),
        speaker: None,
        attachments: None,
    }]
}

// ---------------------------------------------------------------------------
// RULE-ARGS-001 (2026-08-27): parameter-object fixtures for the migrated
// `run_chat_loop(request, deps, role)` test call sites. 旧 38 位参时代每个
// 测试手写的「默认全身像」（basic.rs 模板，与 tests_message_queue.rs 的
// QueueDriverDeps 直构习惯同源）在此收口为三个缺省组装器 —— 调用点只写
// 差异字段（design D4：单点翻译，翻译错误由对应场景测试兜底）。
//
// 默认值与旧位参逐位对应；各字段的历史契约文档（RULE-A-014 /
// RULE-A-015 / RULE-E-005 / B6 / F1 双抑制 / L1a / L3b / W1 / WP2 等）
// 原文居于 `agent/chat_loop/suite.rs` 对应字段的 doc comment，此处不重复。
//
// - deps：harness 字段逐一 clone（旧 #9–#14/#17/#18/#26/#32/#33/#36 各位），
//   token = 未注册的 `CancellationToken::new()`（旧 #15 缺省）；取消类
//   测试在调用点覆盖 `deps.token`（句柄与注册皆由测试自持，语义照旧）。
// - request：身份七件套 tool_defs / provider / context_window / rid /
//   session_id / messages / sink 必填（沿旧位参 #1–#7 阅读序，跳过恒为
//   None 的 #4 provider_id）；其余六字段缺省 None，差异在调用点对返回值
//   具名赋值（如 `request.max_turns = Some(2);`）。
// - role：parent 全集 —— `is_worker: Some(false)`（RULE-A-014 生产式
//   显式值）、skip 三兄弟 false、worker 双件 None/
//   ThreadLocalSubagentSink、四 override None、app_data_dir 取 harness
//   tempdir（旧 #30）。worker 形态（stub 自愈直呼 / RULE-A-014 回归）
//   由调用点具名覆盖，见 tests_agent_loop/stub.rs 先例。
// ---------------------------------------------------------------------------

/// 旧位参 #1–#7 → 单次请求值；#4 `provider_id` 测试恒传 `None`，由本
/// 构造器代置。`rid` 收具体 `String`（不用 `impl Into` —— 调用点实参
/// 已是 `.into()`/`format!` 产物，泛型会与 `"x".into()` 打成推理循环）。
pub(crate) fn chat_loop_request(
    tool_defs: Vec<crate::llm::ToolDef>,
    provider: Arc<dyn crate::llm::Provider>,
    context_window: u32,
    rid: String,
    session_id: String,
    messages: Vec<ChatMessage>,
    sink: Arc<dyn ChatEventSink>,
) -> crate::agent::chat_loop::ChatLoopRequest {
    crate::agent::chat_loop::ChatLoopRequest {
        tool_defs,
        provider,
        context_window,
        provider_id: None,
        rid,
        session_id,
        messages,
        sink,
        resend_seq: None,
        max_turns: None,
        workflow_ctx: None,
        group_chat_state: None,
        current_speaker: None,
        // F2 origin:测试默认 None(来源载体链场景在调用点具名覆盖)。
        origin: None,
    }
}

/// 旧位参 #9–#18/#26/#32/#33/#36 → AppState 派生长寿命套件的测试组装
/// （比照 [`TestHarness`] 字段来源与生产 `from_app_state` 解包序）。
pub(crate) fn chat_loop_deps(harness: &TestHarness) -> crate::agent::chat_loop::ChatLoopDeps {
    use crate::agent::chat_loop::{ChatLoopDeps, ChatLoopDepsParts};
    ChatLoopDeps::from(ChatLoopDepsParts {
        db: harness.db.clone(),
        cancellations: harness.cancellations.clone(),
        session_active_request: harness.session_active_request.clone(),
        read_guard: harness.read_guard.clone(),
        memory_cache: harness.memory_cache.clone(),
        skill_cache: harness.skill_cache.clone(),
        permission_asks: harness.permission_asks.clone(),
        // 旧 #15 缺省：孤儿令牌（不入 cancellations map）。取消类测试
        // 在调用点 `deps.token = <已注册令牌>;` 覆盖。
        token: CancellationToken::new(),
        background_shells: harness.background_shells.clone(),
        stub_loaded: harness.stub_loaded.clone(),
        question_store: harness.question_store.clone(),
        subagent_cache: harness.subagent_cache.clone(),
    })
}

/// 旧位参 #19–#25/#27–#31/#37 → production-style caller 的角色全集
/// （basic.rs 模板展开）。字段语义见 suite.rs `CallerRole` doc comments。
pub(crate) fn parent_role(harness: &TestHarness) -> crate::agent::chat_loop::CallerRole {
    crate::agent::chat_loop::CallerRole {
        is_worker: Some(false),
        skip_session_active: false,
        skip_persist: false,
        skip_cancellations: false,
        worker_catalog: None,
        worker_event_sink: Arc::new(crate::agent::subagent::ThreadLocalSubagentSink),
        system_prompt_override: None,
        worker_run_id: None,
        run_grants: None,
        worktree_override: None,
        project_main_override: None,
        app_data_dir: harness.app_data_dir.clone(),
        forced_dispatch: None,
    }
}

// ---------------------------------------------------------------------------
// Shared git-repo test helpers (L3b, 2026-06-27)
//
// Promoted from `git/worktree.rs::tests`'s private `init_repo` /
// `commit_all` so the agent-layer tests (notably
// `subagent/dispatch.rs::tests::probe_worker_changes_*`) can stand up
// a real git repo to exercise worker worktree create/destroy/probe
// without depending on git-internal test internals. Kept here under
// the `_for_test` suffix so a future git-internal helper refactor
// can't collide with these names. The git-domain tests still have
// their own private copies (kept local to avoid a cross-module test
// dependency from `git/` into `agent/`); consolidating them is a
// follow-up.
// ---------------------------------------------------------------------------

/// `git init --initial-branch=main` + configure a test user so
/// subsequent `git commit` calls succeed. Panics on any git error
/// (test setup, never an assertion failure).
pub(crate) fn init_repo_for_test(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    let init = StdCommand::new("git")
        .args(["init", "--initial-branch=main"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(init.status.success(), "git init failed: {:?}", init);
    let cfg_user = StdCommand::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(cfg_user.status.success());
    let cfg_name = StdCommand::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(cfg_name.status.success());
}

/// `git add -A` + `git commit -m <msg> --no-gpg-sign` from `path`.
/// Panics on any git error (test setup, never an assertion failure).
pub(crate) fn commit_all_for_test(path: &Path, msg: &str) {
    let add = StdCommand::new("git")
        .args(["add", "-A"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = StdCommand::new("git")
        .args(["commit", "-m", msg, "--no-gpg-sign"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(commit.status.success(), "git commit failed: {:?}", commit);
}
