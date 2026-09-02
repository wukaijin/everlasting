//! P3c escalation-loop integration tests (design §5, PR3 AC2/AC6).
//!
//! Full-flow tests through `shell::execute` with a REAL sandbox:
//! a command that fails out-of-face under the sandbox triggers the
//! escalation, the mock store "user" approves / denies, and the final
//! tool_result reflects the branch. Loudly skipped (like the P3b
//! matrix) when the runtime kernel lacks Landlock+seccomp — those
//! hosts run fail-open and the escalation can never fire.
//!
//! The four AC2/AC6 branches:
//! - `approve` (AllowOnce) → rerun succeeds, card emitted once;
//! - `deny` → original failure + mode-aware guidance, no rerun;
//! - `grant-hit` (AllowAlways seeded earlier) → rerun with NO card;
//! - `second failure` → the rerun failing again does NOT re-escalate
//!   (exactly one card per tool call — the §5.1 once-per-call gate).

#![cfg(test)]

use std::sync::Arc;

use tempfile::tempdir;

use crate::agent::permissions::escalation::EscalationHandle;
use crate::agent::permissions::{
    new_permission_store, PermissionAskPayload, PermissionContext, PermissionResponse,
};
use crate::state::{ChatEventPayload, ChatEventSink, ToolCallPayload, ToolResultPayload};
use crate::tools::shell::execute;
use crate::tools::ToolContext;

/// Local ask-capturing sink (the permissions suite's `CaptureAskSink`
/// is `pub(super)` — not reachable from `tools`). Captures
/// `permission:ask` payloads; the other trait methods are no-ops.
#[derive(Default)]
struct EscAskSink {
    asks: std::sync::Mutex<Vec<PermissionAskPayload>>,
}

impl ChatEventSink for EscAskSink {
    fn emit_chat_event(&self, _p: &ChatEventPayload) {}
    fn emit_tool_call(&self, _p: &ToolCallPayload) {}
    fn emit_tool_result(&self, _p: &ToolResultPayload) {}
    fn emit_permission_ask(&self, p: PermissionAskPayload) {
        self.asks.lock().unwrap().push(p);
    }
}

/// Migrated pool + project(`readwrite`) + session row, so `decide`
/// resolves the readwrite face. Mirrors `sandbox::tests_sandbox::
/// policy_pool` (project convention: each domain owns its pool).
async fn escalation_pool(session_id: &str) -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    crate::db::migrations::run_migrations(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO projects (id, name, path, created_at, updated_at) \
         VALUES ('esc-proj', 'p', '/tmp/esc-proj', datetime('now'), datetime('now'))",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE projects SET sandbox_policy = 'readwrite' WHERE id = 'esc-proj'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO sessions (id, title, created_at, updated_at, model, project_id) \
         VALUES (?, 't', datetime('now'), datetime('now'), '', 'esc-proj')",
    )
    .bind(session_id)
    .execute(&pool)
    .await
    .unwrap();
    pool
}

fn esc_ctx(tmp: &tempfile::TempDir, db: sqlx::SqlitePool, handle: EscalationHandle) -> ToolContext {
    ToolContext {
        tool_use_id: None,
        worktree_path: tmp.path().canonicalize().unwrap(),
        cwd: tmp.path().canonicalize().unwrap(),
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db,
        project_id: "esc-proj".to_string(),
        data_dir: tmp.path().to_path_buf(),
        workflow_name: None,
        mode: crate::db::Mode::Edit,
        escalation: handle,
    }
}

/// A command that ALWAYS fails with `Permission denied` (sandboxed or
/// not): touching another process's memory. Under the sandbox it
/// fails out-of-face → escalation; approved, the rerun fails the same
/// way (real EPERM) → plain failure, no second card.
const ALWAYS_DENIED: &str = "touch /proc/1/mem";

/// Wait for the first ask payload and resolve it with `decision`
/// (mirrors the frontend `permission_response` IPC handler).
/// `resolve_ask` returns false while the payload has been emitted but
/// `register_ask` has not yet inserted the oneshot (ask.rs emits
/// before registering) — retry until the resolve lands, else parallel
/// test load makes the 120s timeout win the race (observed flaky).
fn spawn_resolver(
    sink: Arc<EscAskSink>,
    store: crate::agent::permissions::PermissionStore,
    decision: PermissionResponse,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        for _ in 0..1200 {
            let rid = {
                let asks = sink.asks.lock().unwrap();
                asks.first().map(|a| a.rid.clone())
            };
            if let Some(rid) = rid {
                if crate::agent::permissions::resolve_ask(&store, &rid, decision.clone()).await {
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
}

fn count_denial_lines(content: &str) -> usize {
    content.matches("Permission denied").count()
}

/// AC2 approve path: the sandboxed failure surfaces an Ask card;
/// AllowOnce → the exact command reruns unsandboxed. For a command
/// that is legitimately denied by the OS either way (/proc/1/mem),
/// the rerun still fails — but with the card approved, the result is
/// the rerun's PLAIN failure: no guidance line appended (design: the
/// rerun "按普通失败返回"), and exactly ONE card for the call.
#[tokio::test(flavor = "multi_thread")]
async fn escalation_approve_reruns_once_without_second_card() {
    if !crate::sandbox::Capability::probe().ok() {
        eprintln!("SKIP: Landlock/seccomp unavailable (fail-open runtime, no escalation)");
        return;
    }
    if unsafe { libc::geteuid() } == 0 {
        // The probe command's premise is "the OS denies /proc/1/mem
        // even unsandboxed" — false for root, whose rerun then exits 0
        // and breaks the plain-failure assertion. Same loud-SKIP
        // discipline as the kernel probe.
        eprintln!("SKIP: running as root — /proc/1/mem is writable, probe premise void");
        return;
    }
    let tmp = tempdir().unwrap();
    let pool = escalation_pool("esc-sess-1").await;
    let store = new_permission_store();
    let capture = Arc::new(EscAskSink::default());
    let sink: Arc<dyn ChatEventSink> = capture.clone();
    let root = tmp.path().canonicalize().unwrap();
    let perm_ctx = PermissionContext {
        session_id: "esc-sess-1".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let handle = EscalationHandle::new(
        sink.clone(),
        store.clone(),
        perm_ctx,
        pool.clone(),
        tokio_util::sync::CancellationToken::new(),
        "tu-esc-approve".to_string(),
    );
    let ctx = esc_ctx(&tmp, pool, handle);
    spawn_resolver(capture.clone(), store, PermissionResponse::AllowOnce);

    let (content, is_error, _, exit) = execute(
        &serde_json::json!({ "command": ALWAYS_DENIED }),
        &ctx,
        Some("esc-sess-1"),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await;

    assert!(is_error, "the rerun legitimately fails: {content}");
    assert_eq!(exit, Some(1));
    // Exactly one card for the whole call (§5.1 once-per-call gate).
    let cards = capture.asks.lock().unwrap().len();
    assert_eq!(cards, 1, "exactly one escalation card, got {cards}");
    // The card carries the cause + command + stderr evidence.
    {
        let asks = capture.asks.lock().unwrap();
        let reason = asks[0].reason.as_deref().unwrap_or("");
        assert!(reason.contains("out-of-face write"), "{reason}");
        assert!(reason.contains(ALWAYS_DENIED), "{reason}");
        assert!(reason.contains("Permission denied"), "{reason}");
    }
    // Rerun result = plain failure (no [sandbox] guidance append).
    assert!(
        !content.contains("sandbox write block"),
        "approved rerun must not append sandbox guidance: {content}"
    );
    assert!(
        count_denial_lines(&content) >= 1,
        "the rerun's own denial is visible: {content}"
    );
}

/// AC2 deny path: Deny on the card → the ORIGINAL sandboxed failure
/// is returned with the mode-aware guidance appended; no rerun.
#[tokio::test(flavor = "multi_thread")]
async fn escalation_deny_returns_failure_with_guidance() {
    if !crate::sandbox::Capability::probe().ok() {
        eprintln!("SKIP: Landlock/seccomp unavailable (fail-open runtime, no escalation)");
        return;
    }
    let tmp = tempdir().unwrap();
    let pool = escalation_pool("esc-sess-2").await;
    let store = new_permission_store();
    let capture = Arc::new(EscAskSink::default());
    let sink: Arc<dyn ChatEventSink> = capture.clone();
    let root = tmp.path().canonicalize().unwrap();
    let perm_ctx = PermissionContext {
        session_id: "esc-sess-2".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let handle = EscalationHandle::new(
        sink.clone(),
        store.clone(),
        perm_ctx,
        pool.clone(),
        tokio_util::sync::CancellationToken::new(),
        "tu-esc-deny".to_string(),
    );
    // A command the rerun WOULD have made succeed — denied means the
    // write never happens: `echo hi > $HOME/esc_denied.txt`.
    let command = format!(
        "echo hi > {}/esc_denied.txt",
        std::env::var("HOME").unwrap()
    );
    let ctx = esc_ctx(&tmp, pool, handle);
    spawn_resolver(
        capture.clone(),
        store,
        PermissionResponse::Deny {
            reason: "no".into(),
        },
    );

    let (content, is_error, _, _) = execute(
        &serde_json::json!({ "command": command }),
        &ctx,
        Some("esc-sess-2"),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // 2026-09-02 exit-code semantics: the deny leaves a non-zero
    // exit, which is no longer a tool error — the guidance text in
    // the content is the signal (assertions below).
    assert!(!is_error);
    assert_eq!(capture.asks.lock().unwrap().len(), 1, "one card");
    // Failure + guidance (the deny branch keeps the sandbox framing).
    assert!(
        content.contains("Permission denied"),
        "original sandboxed failure: {content}"
    );
    assert!(
        content.contains("escalation") || content.contains("sandbox_extra_writable"),
        "mode-aware guidance appended after denial: {content}"
    );
    // The denied write never landed.
    let home_target = std::env::var("HOME").unwrap();
    assert!(
        !std::path::Path::new(&home_target)
            .join("esc_denied.txt")
            .exists(),
        "denied escalation must NOT write the file"
    );
}

/// AC6 grant-hit path: an earlier AllowAlways on the `echo` prefix →
/// the rerun happens with NO card at all (direct unsandboxed rerun).
#[tokio::test(flavor = "multi_thread")]
async fn escalation_prefix_grant_reruns_without_card() {
    if !crate::sandbox::Capability::probe().ok() {
        eprintln!("SKIP: Landlock/seccomp unavailable (fail-open runtime, no escalation)");
        return;
    }
    let tmp = tempdir().unwrap();
    let pool = escalation_pool("esc-sess-3").await;
    // Seed what ask_path's AllowAlways would have written earlier.
    sqlx::query(
        "INSERT INTO session_tool_permissions (session_id, tool_name, match_kind, match_value) \
         VALUES ('esc-sess-3', 'shell', 'prefix', 'echo')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let store = new_permission_store();
    let capture = Arc::new(EscAskSink::default());
    let sink: Arc<dyn ChatEventSink> = capture.clone();
    let root = tmp.path().canonicalize().unwrap();
    let perm_ctx = PermissionContext {
        session_id: "esc-sess-3".to_string(),
        mode: crate::db::Mode::Edit,
        cwd: root.clone(),
        is_worker: false,
        worker_run_id: None,
        run_grants: None,
        worktree_path: root.clone(),
        project_main_path: root.clone(),
        turn_seq: None,
    };
    let handle = EscalationHandle::new(
        sink.clone(),
        store.clone(),
        perm_ctx,
        pool.clone(),
        tokio_util::sync::CancellationToken::new(),
        "tu-esc-grant".to_string(),
    );
    let ctx = esc_ctx(&tmp, pool, handle);
    let (content, is_error, _, exit) = execute(
        &serde_json::json!({ "command": "echo grant-hit > /tmp/everlasting_esc_grant.txt" }),
        &ctx,
        Some("esc-sess-3"),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await;

    assert!(!is_error, "grant-hit rerun succeeds: {content}");
    assert_eq!(exit, Some(0));
    assert_eq!(capture.asks.lock().unwrap().len(), 0, "grant hit → NO card");
    let written = std::fs::read_to_string("/tmp/everlasting_esc_grant.txt")
        .expect("rerun wrote the file unsandboxed");
    assert!(written.contains("grant-hit"));
    let _ = std::fs::remove_file("/tmp/everlasting_esc_grant.txt");
}

/// §5.1 once-per-call gate, direct proof: the rerun of
/// `touch /proc/1/mem` fails with the SAME denial signature — the
/// gate must NOT loop into a second card (covered transitively by
/// `escalation_approve_reruns_once_without_second_card`; this test
/// pins the pure helpers: compound commands never grant-hit).
#[tokio::test]
async fn escalation_compound_command_never_grant_hits() {
    let pool = escalation_pool("esc-sess-4").await;
    sqlx::query(
        "INSERT INTO session_tool_permissions (session_id, tool_name, match_kind, match_value) \
         VALUES ('esc-sess-4', 'shell', 'prefix', 'echo')",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Single-segment `echo hi` → hit.
    assert!(
        crate::agent::permissions::escalation::prefix_grant_hit(&pool, "esc-sess-4", "echo hi")
            .await
    );
    // Compound: the `echo` grant must NOT cover `echo a; rm b`.
    assert!(
        !crate::agent::permissions::escalation::prefix_grant_hit(
            &pool,
            "esc-sess-4",
            "echo a; rm b"
        )
        .await
    );
    // Different prefix: no hit.
    assert!(
        !crate::agent::permissions::escalation::prefix_grant_hit(&pool, "esc-sess-4", "cargo test")
            .await
    );
}

/// No handle (test-path ToolContext) → no card, failure + guidance
/// only (the degradation contract).
#[tokio::test(flavor = "multi_thread")]
async fn escalation_without_handle_degrades_to_guidance() {
    if !crate::sandbox::Capability::probe().ok() {
        eprintln!("SKIP: Landlock/seccomp unavailable (fail-open runtime, no escalation)");
        return;
    }
    let tmp = tempdir().unwrap();
    let pool = escalation_pool("esc-sess-5").await;
    let ctx = esc_ctx(&tmp, pool, EscalationHandle::default());
    let (content, is_error, _, _) = execute(
        &serde_json::json!({ "command": "echo hi > $HOME/esc_no_handle.txt" }),
        &ctx,
        Some("esc-sess-5"),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await;
    // 2026-09-02 exit-code semantics: guidance-only degradation keeps
    // is_error false — the "sandbox write block" text is the signal.
    assert!(!is_error);
    assert!(
        content.contains("sandbox write block"),
        "guidance appended without a handle: {content}"
    );
}
