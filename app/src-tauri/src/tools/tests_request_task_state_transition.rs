#![cfg(test)]

use std::sync::Arc;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use crate::agent::question_store::{
    InteractionResponse, ModeChangePayload, PendingInteraction, QuestionStore,
    TaskStateTransitionPayload,
};
use crate::agent::workflow::{TaskStatus, WorkflowDef};
use crate::state::ChatEventSink;
use crate::tools::request_task_state_transition::*;

// ----- helpers for tests -----

/// A `dyn ChatEventSink` stub that captures the latest emit
/// so tests can assert "did we publish to IPC?". Mirrors the
/// test infrastructure pattern used in
/// `tools/request_mode_change.rs::tests::CapturingSink`.
#[derive(Default)]
struct CapturingSink {
    emitted_task_state_transition: std::sync::Mutex<Vec<TaskStateTransitionPayload>>,
}

impl ChatEventSink for CapturingSink {
    fn emit_chat_event(&self, _payload: &crate::state::ChatEventPayload) {}
    fn emit_tool_call(&self, _payload: &crate::state::ToolCallPayload) {}
    fn emit_tool_result(&self, _payload: &crate::state::ToolResultPayload) {}
    fn emit_permission_ask(&self, _payload: crate::agent::permissions::PermissionAskPayload) {}
    fn emit_tool_question(&self, _payload: &crate::agent::question_store::ToolQuestionPayload) {}
    fn emit_mode_change_request(&self, _payload: &ModeChangePayload) {}
    fn emit_task_state_transition(&self, payload: &TaskStateTransitionPayload) {
        self.emitted_task_state_transition
            .lock()
            .unwrap()
            .push(payload.clone());
    }
}

fn make_sink() -> Arc<CapturingSink> {
    Arc::new(CapturingSink::default())
}

async fn fresh_db() -> SqlitePool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    crate::db::migrations::run_migrations(&pool).await.unwrap();
    pool
}

async fn seed_session(pool: &SqlitePool, session_id: &str) {
    let dir = tempfile::tempdir().expect("tempdir");
    let project_path = dir.path().to_path_buf();
    let project_id = format!("proj-{}", session_id);
    crate::db::create_project(
        pool,
        &project_id,
        project_path.to_str().unwrap(),
        false,
        None,
    )
    .await
    .expect("create_project");
    crate::db::create_session(
        pool,
        session_id,
        &project_id,
        project_path.to_str().unwrap(),
        "mock-model",
        None,
        None,
        None,
    )
    .await
    .expect("create_session");
}

fn make_valid_input(target: &str, slug: &str) -> serde_json::Value {
    serde_json::json!({
        "target_state": target,
        "slug": slug,
        "reason": "research complete; ready to implement",
    })
}

// ----- validation short-circuits -----

#[tokio::test]
async fn validation_empty_target_state_short_circuits() {
    let pool = fresh_db().await;
    let store = QuestionStore::new();
    let sink = make_sink();
    let input = serde_json::json!({"target_state": "  ", "slug": "my-feat"});
    let cancel = CancellationToken::new();
    let (content, is_error, _, _) = execute_blocking(
        &input,
        "s1",
        "tu_1",
        Some(TaskStatus::Planning),
        Some("my-feat".into()),
        &pool,
        &store,
        &(sink.clone() as Arc<dyn ChatEventSink>),
        &cancel,
        None, // workflow_def (non-workflow session in these legacy tests)
        None,
    )
    .await;
    assert!(is_error, "empty target → is_error: true");
    assert!(content.contains("schema validation failed"));
    assert!(
        sink.emitted_task_state_transition
            .lock()
            .unwrap()
            .is_empty(),
        "no IPC emit on validation failure"
    );
    assert!(store.get_payload("s1").await.is_none());
}

#[tokio::test]
async fn custom_target_in_non_workflow_session_short_circuits() {
    // C0 (`07-26-taskstatus-custom-state`): validate no longer
    // rejects unknown target_state ("synth" passes validate now).
    // The gate moved into execute_blocking — a non-workflow
    // session (workflow_def = None) rejects Custom targets because
    // there's no plugin to define them. This replaces the legacy
    // "validate rejects unknown enum" test with the C0 semantics.
    let pool = fresh_db().await;
    let store = QuestionStore::new();
    let sink = make_sink();
    let input = serde_json::json!({"target_state": "synth", "slug": "my-feat"});
    let cancel = CancellationToken::new();
    let (content, is_error, _, _) = execute_blocking(
        &input,
        "s1",
        "tu_1",
        Some(TaskStatus::Planning),
        Some("my-feat".into()),
        &pool,
        &store,
        &(sink.clone() as Arc<dyn ChatEventSink>),
        &cancel,
        None, // workflow_def (non-workflow session in these legacy tests)
        None,
    )
    .await;
    assert!(is_error);
    assert!(
        content.contains("invalid_transition"),
        "non-workflow Custom rejection surfaces invalid_transition (got: {content})"
    );
    assert!(sink
        .emitted_task_state_transition
        .lock()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn validation_invalid_slug_short_circuits() {
    let pool = fresh_db().await;
    let store = QuestionStore::new();
    let sink = make_sink();
    let input = serde_json::json!({"target_state": "in_progress", "slug": "UPPER"});
    let cancel = CancellationToken::new();
    let (content, is_error, _, _) = execute_blocking(
        &input,
        "s1",
        "tu_1",
        Some(TaskStatus::Planning),
        Some("my-feat".into()),
        &pool,
        &store,
        &(sink.clone() as Arc<dyn ChatEventSink>),
        &cancel,
        None, // workflow_def (non-workflow session in these legacy tests)
        None,
    )
    .await;
    assert!(is_error);
    assert!(content.contains("schema validation failed"));
}

#[tokio::test]
async fn validation_reason_too_long_short_circuits() {
    let pool = fresh_db().await;
    let store = QuestionStore::new();
    let sink = make_sink();
    let input = serde_json::json!({
        "target_state": "in_progress",
        "slug": "my-feat",
        "reason": "x".repeat(501), // 501 > 500
    });
    let cancel = CancellationToken::new();
    let (content, is_error, _, _) = execute_blocking(
        &input,
        "s1",
        "tu_1",
        Some(TaskStatus::Planning),
        Some("my-feat".into()),
        &pool,
        &store,
        &(sink.clone() as Arc<dyn ChatEventSink>),
        &cancel,
        None, // workflow_def (non-workflow session in these legacy tests)
        None,
    )
    .await;
    assert!(is_error);
    assert!(content.contains("schema validation failed"));
    assert!(sink
        .emitted_task_state_transition
        .lock()
        .unwrap()
        .is_empty());
}

// ----- slug mismatch / missing context short-circuits -----

#[tokio::test]
async fn supplied_slug_must_match_workflow_ctx_short_circuits() {
    let pool = fresh_db().await;
    let store = QuestionStore::new();
    let sink = make_sink();
    let input = serde_json::json!({"target_state": "in_progress", "slug": "wrong-slug"});
    let cancel = CancellationToken::new();
    let (content, is_error, _, _) = execute_blocking(
        &input,
        "s1",
        "tu_1",
        Some(TaskStatus::Planning),
        Some("my-feat".into()),
        &pool,
        &store,
        &(sink.clone() as Arc<dyn ChatEventSink>),
        &cancel,
        None, // workflow_def (non-workflow session in these legacy tests)
        None,
    )
    .await;
    assert!(is_error);
    assert!(
        content.contains("slug"),
        "slug mismatch surfaces in tool_result (got {content})"
    );
    assert!(store.get_payload("s1").await.is_none());
}

#[tokio::test]
async fn empty_slug_falls_back_to_workflow_ctx() {
    let pool = fresh_db().await;
    seed_session(&pool, "s1").await;
    let store = QuestionStore::new();
    let sink = make_sink();
    let input = serde_json::json!({"target_state": "in_progress"});
    let cancel = CancellationToken::new();
    let sink_arc: Arc<dyn ChatEventSink> = sink.clone();

    let store_clone = store.clone();
    let input_clone = input.clone();
    let cancel_clone = cancel.clone();
    let pool_clone = pool.clone();
    let exec = tokio::spawn(async move {
        execute_blocking(
            &input_clone,
            "s1",
            "tu_1",
            Some(TaskStatus::Planning),
            Some("my-feat".into()),
            &pool_clone,
            &store_clone,
            &sink_arc,
            &cancel_clone,
            None, // workflow_def (non-workflow session in these legacy tests)
            None,
        )
        .await
    });

    let register_wait_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while store.get_payload("s1").await.is_none() {
        if std::time::Instant::now() > register_wait_deadline {
            panic!("executor never registered the pending state transition");
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    // Resolve.
    store
        .resolve("s1", InteractionResponse::Answered(serde_json::json!(true)))
        .await
        .expect("resolve ok");
    let (_content, is_error, _, _) = exec.await.expect("exec ok");
    assert!(!is_error);
    // Slug fell back to current_slug = "my-feat".
    let emitted = sink.emitted_task_state_transition.lock().unwrap();
    assert_eq!(emitted.len(), 1);
    assert_eq!(emitted[0].slug.as_deref(), Some("my-feat"));
}

#[tokio::test]
async fn no_current_slug_short_circuits() {
    let pool = fresh_db().await;
    let store = QuestionStore::new();
    let sink = make_sink();
    let input = serde_json::json!({"target_state": "in_progress"});
    let cancel = CancellationToken::new();
    let (content, is_error, _, _) = execute_blocking(
        &input,
        "s1",
        "tu_1",
        None, // no current state
        None, // no current slug
        &pool,
        &store,
        &(sink.clone() as Arc<dyn ChatEventSink>),
        &cancel,
        None, // workflow_def (non-workflow session in these legacy tests)
        None,
    )
    .await;
    assert!(is_error);
    assert!(
        content.contains("no active workflow task"),
        "no current_task short-circuits (got {content})"
    );
    assert!(store.get_payload("s1").await.is_none());
}

// ----- noop path -----

#[tokio::test]
async fn noop_when_target_equals_current() {
    let pool = fresh_db().await;
    let store = QuestionStore::new();
    let sink = make_sink();
    let input = make_valid_input("in_progress", "my-feat");
    let cancel = CancellationToken::new();
    let (content, is_error, _, _) = execute_blocking(
        &input,
        "s1",
        "tu_1",
        Some(TaskStatus::InProgress),
        Some("my-feat".into()),
        &pool,
        &store,
        &(sink.clone() as Arc<dyn ChatEventSink>),
        &cancel,
        None, // workflow_def (non-workflow session in these legacy tests)
        None,
    )
    .await;
    assert!(
        !is_error,
        "noop is NOT an error (matches request_mode_change noop)"
    );
    assert!(content.contains("noop"));
    // No IPC emit on noop (matches request_mode_change).
    assert!(sink
        .emitted_task_state_transition
        .lock()
        .unwrap()
        .is_empty());
    assert!(store.get_payload("s1").await.is_none());
}

// ----- happy path + register-before-emit invariant -----

#[tokio::test]
async fn happy_path_registers_emits_and_returns_allowed() {
    let pool = fresh_db().await;
    seed_session(&pool, "s1").await;
    let store = QuestionStore::new();
    let sink = make_sink();
    let input = make_valid_input("in_progress", "my-feat");
    let cancel = CancellationToken::new();
    let sink_arc: Arc<dyn ChatEventSink> = sink.clone();

    let store_clone = store.clone();
    let input_clone = input.clone();
    let cancel_clone = cancel.clone();
    let pool_clone = pool.clone();
    let exec = tokio::spawn(async move {
        execute_blocking(
            &input_clone,
            "s1",
            "tu_1",
            Some(TaskStatus::Planning),
            Some("my-feat".into()),
            &pool_clone,
            &store_clone,
            &sink_arc,
            &cancel_clone,
            None, // workflow_def (non-workflow session in these legacy tests)
            None,
        )
        .await
    });

    // Wait for register.
    let register_wait_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while store.get_payload("s1").await.is_none() {
        if std::time::Instant::now() > register_wait_deadline {
            panic!("executor never registered the pending state transition");
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    // Emit happened.
    {
        let emitted = sink.emitted_task_state_transition.lock().unwrap();
        assert_eq!(emitted.len(), 1, "emit_task_state_transition called once");
        assert_eq!(emitted[0].session_id, "s1");
        assert_eq!(emitted[0].tool_use_id, "tu_1");
        assert_eq!(emitted[0].target_state, "in_progress");
        assert_eq!(emitted[0].current_state.as_deref(), Some("planning"));
        assert_eq!(emitted[0].slug.as_deref(), Some("my-feat"));
    }

    // Resolve as Answered.
    store
        .resolve("s1", InteractionResponse::Answered(serde_json::json!(true)))
        .await
        .expect("resolve ok");

    let (content, is_error, _, _) = exec.await.expect("exec ok");
    assert!(!is_error, "allowed returns is_error: false");
    assert!(content.contains("allowed"));
    assert!(content.contains("new_state"));
    assert!(content.contains("in_progress"));
}

// ----- cancel arm -----

#[tokio::test]
async fn cancel_arm_returns_session_cancelled_marker() {
    let pool = fresh_db().await;
    seed_session(&pool, "s1").await;
    let store = QuestionStore::new();
    let sink = make_sink();
    let input = make_valid_input("in_progress", "my-feat");
    let cancel = CancellationToken::new();
    let sink_arc: Arc<dyn ChatEventSink> = sink.clone();

    let store_clone = store.clone();
    let input_clone = input.clone();
    let cancel_clone = cancel.clone();
    let pool_clone = pool.clone();
    let exec = tokio::spawn(async move {
        execute_blocking(
            &input_clone,
            "s1",
            "tu_1",
            Some(TaskStatus::Planning),
            Some("my-feat".into()),
            &pool_clone,
            &store_clone,
            &sink_arc,
            &cancel_clone,
            None, // workflow_def (non-workflow session in these legacy tests)
            None,
        )
        .await
    });

    let register_wait_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while store.get_payload("s1").await.is_none() {
        if std::time::Instant::now() > register_wait_deadline {
            panic!("executor never registered");
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    cancel.cancel();
    let (content, is_error, _, _) = exec.await.expect("exec ok");
    assert!(is_error);
    assert!(content.contains("cancelled_by_session"));
    assert!(store.get_payload("s1").await.is_none());
}

// ----- user-cancel path -----

#[tokio::test]
async fn cancelled_path_returns_user_cancel_marker() {
    let pool = fresh_db().await;
    seed_session(&pool, "s1").await;
    let store = QuestionStore::new();
    let sink = make_sink();
    let input = make_valid_input("in_progress", "my-feat");
    let cancel = CancellationToken::new();
    let sink_arc: Arc<dyn ChatEventSink> = sink.clone();

    let store_clone = store.clone();
    let input_clone = input.clone();
    let cancel_clone = cancel.clone();
    let pool_clone = pool.clone();
    let exec = tokio::spawn(async move {
        execute_blocking(
            &input_clone,
            "s1",
            "tu_1",
            Some(TaskStatus::Planning),
            Some("my-feat".into()),
            &pool_clone,
            &store_clone,
            &sink_arc,
            &cancel_clone,
            None, // workflow_def (non-workflow session in these legacy tests)
            None,
        )
        .await
    });

    let register_wait_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while store.get_payload("s1").await.is_none() {
        if std::time::Instant::now() > register_wait_deadline {
            panic!("executor never registered");
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    store
        .resolve("s1", InteractionResponse::Cancelled)
        .await
        .expect("resolve ok");
    let (content, is_error, _, _) = exec.await.expect("exec ok");
    assert!(is_error, "cancelled = is_error: true");
    assert!(content.contains("cancelled_by_user"));
}

// ----- AlreadyPending race -----

#[tokio::test]
async fn already_pending_returns_structured_error() {
    let pool = fresh_db().await;
    seed_session(&pool, "s1").await;
    let store = QuestionStore::new();
    let sink = make_sink();

    // Pre-register a different pending interaction.
    store
        .register(
            "s1",
            "tu_pre",
            PendingInteraction::TaskStateTransition(TaskStateTransitionPayload {
                session_id: "s1".into(),
                tool_use_id: "tu_pre".into(),
                target_state: "in_progress".into(),
                current_state: Some("planning".into()),
                slug: Some("my-feat".into()),
                reason: None,
                ts: 0,
            }),
        )
        .await
        .expect("pre-register ok");

    let input = make_valid_input("in_progress", "my-feat");
    let cancel = CancellationToken::new();
    let (content, is_error, _, _) = execute_blocking(
        &input,
        "s1",
        "tu_2",
        Some(TaskStatus::Planning),
        Some("my-feat".into()),
        &pool,
        &store,
        &(sink.clone() as Arc<dyn ChatEventSink>),
        &cancel,
        None, // workflow_def (non-workflow session in these legacy tests)
        None,
    )
    .await;
    assert!(is_error);
    assert!(content.contains("已有 pending"));
    assert!(sink
        .emitted_task_state_transition
        .lock()
        .unwrap()
        .is_empty());
    // Drain for isolation.
    let _ = store.remove("s1").await;
}

// ----- schema validate export pure-fn -----
#[test]
fn validate_accepts_well_formed_input() {
    let input = RequestTaskStateTransitionInput {
        target_state: "in_progress".into(),
        slug: "my-feat".into(),
        reason: Some("ok".into()),
    };
    validate(&input).expect("valid input passes");
}

#[test]
fn validate_accepts_unknown_target_state() {
    // C0 (`07-26-taskstatus-custom-state`): validate no longer
    // rejects unknown target_state strings. They flow through as
    // TaskStatus::Custom; legality is decided in execute_blocking
    // via can_transition (workflow session) or Custom rejection
    // (non-workflow session).
    let input = RequestTaskStateTransitionInput {
        target_state: "reviewing".into(),
        slug: "my-feat".into(),
        reason: None,
    };
    validate(&input).expect("unknown target_state now passes validate");
}

/// C0: a workflow session with a review-shaped WorkflowDef accepts
/// a declared transition (intake → reviewing) and rejects an
/// undeclared one (planning → done has no such edge). This is the
/// core R3 acceptance criterion — legality is plugin-driven via
/// can_transition, NOT a global hard-coded enum.
#[tokio::test]
async fn can_transition_gate_accepts_declared_and_rejects_undeclared() {
    let pool = fresh_db().await;
    seed_session(&pool, "s1").await;
    let store = QuestionStore::new();
    let sink = make_sink();
    let def = review_workflow_def();
    let cancel = CancellationToken::new();

    // Declared edge: intake → reviewing (review plugin declares it).
    // The gate is the assertion — once past it, execute_blocking
    // registers + emits + waits for resolve. We spawn it, observe
    // the emit (proof the gate passed), then cancel to unwind
    // deterministically (mirrors the cancel_arm test pattern).
    let sink_arc: Arc<dyn ChatEventSink> = sink.clone();
    let store_clone = store.clone();
    let cancel_clone = cancel.clone();
    let pool_clone = pool.clone();
    let input_declared = serde_json::json!({"target_state": "reviewing", "slug": "my-feat"});
    let exec = tokio::spawn(async move {
        execute_blocking(
            &input_declared,
            "s1",
            "tu_declared",
            Some(TaskStatus::Custom("intake".to_string())),
            Some("my-feat".into()),
            &pool_clone,
            &store_clone,
            &sink_arc,
            &cancel_clone,
            Some(&def),
            None,
        )
        .await
    });

    // Wait for register+emit — proof the declared transition
    // passed the can_transition gate (it would have
    // short-circuited with invalid_transition BEFORE register).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while store.get_payload("s1").await.is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "declared transition never registered (gate may have rejected it)"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(
        sink.emitted_task_state_transition.lock().unwrap().len(),
        1,
        "declared transition emitted to frontend"
    );
    // Cancel to unwind the spawned executor.
    cancel.cancel();
    let (declared_content, _, _, _) = exec.await.expect("exec ok");
    assert!(
        !declared_content.contains("invalid_transition"),
        "declared transition must not surface invalid_transition (got: {declared_content})"
    );
    let _ = store.remove("s1").await;

    // Undeclared edge: planning → done (review plugin has no such edge).
    // Short-circuits synchronously with invalid_transition — no
    // register, no emit, no spawn needed.
    let input = serde_json::json!({"target_state": "done", "slug": "my-feat"});
    let def2 = review_workflow_def();
    let (content, is_error, _, _) = execute_blocking(
        &input,
        "s1",
        "tu_undeclared",
        Some(TaskStatus::Custom("planning".to_string())),
        Some("my-feat".into()),
        &pool,
        &store,
        &(sink.clone() as Arc<dyn ChatEventSink>),
        &cancel,
        Some(&def2),
        None,
    )
    .await;
    assert!(
        is_error,
        "undeclared transition must short-circuit with is_error"
    );
    assert!(
        content.contains("invalid_transition"),
        "undeclared transition surfaces invalid_transition marker (got: {content})"
    );
    // Undeclared path never registers.
    assert!(store.get_payload("s1").await.is_none());
}

/// C0 design §3: a non-workflow session (workflow_def = None) rejects
/// a Custom target_state — without a plugin there's no basis for a
/// custom state. Known dev states still pass (covered by happy_path).
#[tokio::test]
async fn non_workflow_session_rejects_custom_target() {
    let pool = fresh_db().await;
    let store = QuestionStore::new();
    let sink = make_sink();
    let cancel = CancellationToken::new();
    let input = serde_json::json!({"target_state": "reviewing", "slug": "my-feat"});
    let (content, is_error, _, _) = execute_blocking(
        &input,
        "s1",
        "tu_custom",
        Some(TaskStatus::Planning),
        Some("my-feat".into()),
        &pool,
        &store,
        &(sink.clone() as Arc<dyn ChatEventSink>),
        &cancel,
        None, // non-workflow session
        None,
    )
    .await;
    assert!(is_error, "Custom target in non-workflow session is_error");
    assert!(
        content.contains("invalid_transition"),
        "non-workflow Custom rejection surfaces invalid_transition (got: {content})"
    );
    assert!(
        sink.emitted_task_state_transition
            .lock()
            .unwrap()
            .is_empty(),
        "no IPC emit on rejection"
    );
    assert!(store.get_payload("s1").await.is_none());
}

/// Minimal review-shaped WorkflowDef for the can_transition gate
/// tests. Mirrors the shape a real review workflow.json would have:
/// states intake/reviewing/reported, edge intake→reviewing. We only
/// need transitions + name + initial for the gate; the other fields
/// are empty defaults.
fn review_workflow_def() -> WorkflowDef {
    WorkflowDef {
        name: "review".to_string(),
        description: "test review workflow".to_string(),
        states: vec![
            "intake".to_string(),
            "reviewing".to_string(),
            "reported".to_string(),
        ],
        initial: "intake".to_string(),
        transitions: vec![crate::agent::workflow::Transition {
            from: "intake".to_string(),
            to: "reviewing".to_string(),
            requires_user_confirm: true,
        }],
        roles_by_state: Default::default(),
        breadcrumb: Default::default(),
        delegation_templates: Default::default(),
        coordination: crate::agent::workflow::Coordination::default(),
        gather_strategy: Default::default(),
    }
}
