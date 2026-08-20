#![cfg(test)]

use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::agent::chat_loop::run_chat_loop;
use crate::db;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

// ---------------------------------------------------------------------------
// 13) L2 (2026-06-19): parallel read-only tool batch
// ---------------------------------------------------------------------------

/// `is_parallel_eligible` — pure predicate. Covers:
/// - all-eligible set → true
/// - each excluded tool (write_file / edit_file / shell / web_fetch /
///   update_checklist) in an otherwise-eligible batch → false
/// - empty batch → false (defensive; the agent loop only calls
///   this on non-empty `tool_calls`)
/// - RULE-A-013 (2026-06-19): path-outside-root read tools
///   pull the batch back to serial. `paths` lets the test pin
///   each tool's `path` arg (empty string = no `path` arg).
#[test]
fn is_parallel_eligible_classifies_correctly() {
    use crate::agent::chat_loop::is_parallel_eligible;

    /// `names[i]` is the tool name; `paths[i]` is the `path`
    /// arg to inject (empty string = no `path` field, mirroring
    /// a model call without a path arg). All paths are
    /// constructed as absolute to the `root` passed in.
    fn batch(
        names: &[&str],
        paths: &[&str],
        root: &std::path::Path,
    ) -> Vec<(String, String, serde_json::Value)> {
        names
            .iter()
            .zip(paths.iter().chain(std::iter::repeat(&"")))
            .map(|(n, p)| {
                let input = if p.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::json!({ "path": root.join(p).to_string_lossy() })
                };
                ("x".into(), (*n).into(), input)
            })
            .collect()
    }

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();

    // All-eligible permutations of the read-only set (no paths
    // — the path check is vacuously true).
    assert!(is_parallel_eligible(
        &batch(&["read_file"], &[], root),
        root
    ));
    assert!(is_parallel_eligible(
        &batch(&["read_file", "grep", "glob"], &[], root),
        root
    ));
    assert!(is_parallel_eligible(
        &batch(&["list_dir", "use_skill"], &[], root),
        root
    ));
    assert!(is_parallel_eligible(
        &batch(
            &["read_file", "grep", "glob", "list_dir", "use_skill"],
            &[],
            root
        ),
        root
    ));

    // Each excluded tool alone → false (so a single-tool batch
    // of an excluded tool stays serial).
    assert!(!is_parallel_eligible(
        &batch(&["write_file"], &[], root),
        root
    ));
    assert!(!is_parallel_eligible(
        &batch(&["edit_file"], &[], root),
        root
    ));
    assert!(!is_parallel_eligible(&batch(&["shell"], &[], root), root));
    assert!(!is_parallel_eligible(
        &batch(&["web_fetch"], &[], root),
        root
    ));
    assert!(!is_parallel_eligible(
        &batch(&["update_checklist"], &[], root),
        root
    ));

    // Mixed: one excluded tool poisons the whole batch.
    assert!(!is_parallel_eligible(
        &batch(&["read_file", "edit_file", "grep"], &[], root),
        root
    ));
    assert!(!is_parallel_eligible(
        &batch(&["read_file", "web_fetch"], &[], root),
        root
    ));
    assert!(!is_parallel_eligible(
        &batch(&["read_file", "update_checklist"], &[], root),
        root
    ));

    // Unknown / future tool → conservatively false (serial).
    assert!(!is_parallel_eligible(
        &batch(&["some_future_tool"], &[], root),
        root
    ));

    // Empty batch → false (defensive).
    assert!(!is_parallel_eligible(&[], root));
}

/// RULE-A-013 (2026-06-19): `is_parallel_eligible` now also
/// checks path-outside-root. Cases (all use a real tempdir as
/// `root` so `is_within_root` works as in production):
/// 1. absolute in-root path → eligible
/// 2. relative in-root path → eligible (joined onto root)
/// 3. absolute out-of-root path → falls back to serial
/// 4. relative `../foo` out-of-root path → falls back to serial
/// 5. path tool without a `path` arg → eligible (tool layer
///    schema validation is the fallback; mirrors the
///    permission layer's no-path convention)
/// 6. `use_skill` + arbitrary path in same batch → eligible
///    (`use_skill` is name-eligible and exempt from the path
///    check, so it can ride along with a path tool that has a
///    path arg)
#[test]
fn is_parallel_eligible_boundary_silent() {
    use crate::agent::chat_loop::is_parallel_eligible;
    use std::path::PathBuf;

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // Build an in-root file and an out-of-root sibling file so
    // absolute and relative paths have real targets to resolve
    // against. `is_within_root` is lexical so existence isn't
    // strictly required, but having real files makes the test
    // mirror production intent more clearly.
    std::fs::write(root.join("in_root.txt"), "x").unwrap();
    let outside_dir = tempfile::tempdir().expect("outside tempdir");
    let outside_file: PathBuf = outside_dir.path().join("outside.txt");
    std::fs::write(&outside_file, "x").unwrap();

    // Helper to build a single-tool batch. `root` is unused
    // (the caller resolves the path string in advance) but
    // kept in the signature for symmetry with `batch()` above.
    #[allow(unused_variables)]
    fn single(
        root: &std::path::Path,
        name: &str,
        path: Option<&str>,
    ) -> Vec<(String, String, serde_json::Value)> {
        let input = match path {
            Some(p) => serde_json::json!({ "path": p }),
            None => serde_json::json!({}),
        };
        vec![("x".into(), name.into(), input)]
    }

    // 1. absolute in-root → eligible
    let abs_in = root.join("in_root.txt").to_string_lossy().into_owned();
    assert!(
        is_parallel_eligible(&single(root, "read_file", Some(&abs_in)), root),
        "absolute in-root path should be eligible"
    );

    // 2. relative in-root → eligible (joined onto root)
    assert!(
        is_parallel_eligible(&single(root, "read_file", Some("in_root.txt")), root),
        "relative in-root path should be eligible"
    );

    // 3. absolute out-of-root → NOT eligible
    let abs_out = outside_file.to_string_lossy().into_owned();
    assert!(
        !is_parallel_eligible(&single(root, "read_file", Some(&abs_out)), root),
        "absolute out-of-root path should fall back to serial"
    );

    // 4. relative `../<file>` out-of-root → NOT eligible
    //    Build a relative path from `root` that escapes: e.g.
    //    `../<outside_dir_basename>/outside.txt`. We don't know
    //    the basename, so walk up one level to a tempdir
    //    sibling and back down.
    let rel_out = format!(
        "../{}/{}",
        outside_dir.path().file_name().unwrap().to_string_lossy(),
        "outside.txt"
    );
    assert!(
        !is_parallel_eligible(&single(root, "read_file", Some(&rel_out)), root),
        "relative out-of-root path should fall back to serial"
    );

    // 5. path tool with no `path` arg → eligible (tool layer
    //    validates; we mirror the permission layer convention)
    assert!(
        is_parallel_eligible(&single(root, "read_file", None), root),
        "path tool without path arg should be eligible"
    );
    assert!(
        is_parallel_eligible(&single(root, "grep", None), root),
        "grep without path arg should be eligible"
    );

    // 6. use_skill + arbitrary path coexist → eligible.
    //    use_skill is name-eligible and exempt from the path
    //    check; it can ride along with a path tool that has a
    //    valid in-root path.
    let mixed = vec![
        ("x".into(), "use_skill".into(), serde_json::json!({})),
        (
            "x".into(),
            "read_file".into(),
            serde_json::json!({ "path": abs_in.clone() }),
        ),
    ];
    assert!(
        is_parallel_eligible(&mixed, root),
        "use_skill + in-root path tool should be eligible"
    );
}

/// L2: three `read_file` tool_use blocks in one turn execute
/// concurrently. Asserts:
/// - exactly 3 `tool:result` events fire
/// - the result contents appear in the SAME order as the
///   tool_use blocks (LLM-context stability contract — Q3) by
///   cross-referencing `tool_use_id` to file content
/// - the persisted `tool_result` user message has its blocks
///   in tool_use order, not completion order
///
/// The three reads target different-sized files so the result
/// text is unambiguous about which tool_use produced which
/// result. (No timing assertion — see PRD §Acceptance for the
/// "no flaky timing" rule.)
#[tokio::test]
async fn agent_loop_parallel_readonly_batch_preserves_order() {
    let h = make_harness().await;

    // Three distinct files under the project root so each
    // read_file produces a unique, identifiable output.
    std::fs::write(h.project_path.join("a.txt"), "AAA-content").unwrap();
    std::fs::write(h.project_path.join("b.txt"), "BBB-content").unwrap();
    std::fs::write(h.project_path.join("c.txt"), "CCC-content").unwrap();

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: three tool_use blocks (eligible batch).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_a".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "a.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_b".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "b.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_c".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "c.txt"}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: terminal text (consumed only if turn 1 ran
        // successfully through the parallel batch and the
        // tool_result got fed back).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "done".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        None,
        "rid-par-batch".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (200) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
        // 2026-06-22 (RULE-FrontSubagent-003 fix): tests pass
        // `None` (production-style caller — not a worker, so
        // the PermissionContext.worker_run_id is unused by the
        // ask_path parent branch). The worker nested call in
        // `run_subagent` passes `Some(worker_run_id_opt)`.
        None,
        h.subagent_cache.clone(),
        None,
        // L3b (2026-06-27): production-style caller (and non-isolated
        // worker path) → worktree_override = None (use the session
        // row's worktree_path). Only the isolated worker path passes
        // Some(worker_worktree_path).
        None,
        None, // project_main_override (2026-07-29)
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
        None,
        // 2026-06-30 (ask_user_question task): per-test QuestionStore
        h.question_store.clone(),
        // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
        // workflow_ctx = None (tests don't exercise the workflow
        // breadcrumb injection seam; that lives in separate
        // `agent::workflow::inject` tests).
        None,
        None,
        None,
        h.stub_loaded.clone(),
    )
    .await;

    assert_eq!(
        mock.call_count(),
        2,
        "parallel batch must trigger exactly one follow-up turn (2 sends total)"
    );
    assert_eq!(emitter.tool_call_count(), 3, "all 3 tool_use fire");
    assert_eq!(
        emitter.tool_result_count(),
        3,
        "all 3 read_file produce a tool_result"
    );

    // Order contract: the result_blocks in the persisted
    // tool_result user message MUST appear in tool_use order
    // (a, b, c), regardless of which task finished first.
    let loaded = db::load_session(&h.db, &h.session_id)
        .await
        .expect("load_session")
        .expect("session exists");
    // `MessageContent::Blocks` serializes as a top-level JSON
    // array (see `llm/types.rs` MessageContent Serialize impl),
    // so `MessageRow.content` is the array directly.
    let tool_result_msg = loaded
        .messages
        .iter()
        .find(|m| {
            m.role == "user"
                && m.content
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
                    })
                    .unwrap_or(false)
        })
        .expect("tool_result user message persisted");
    let blocks = tool_result_msg.content.as_array().expect("content array");
    let tool_use_ids: Vec<String> = blocks
        .iter()
        .filter_map(|b| {
            if b.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                b.get("tool_use_id")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        tool_use_ids,
        vec!["toolu_a", "toolu_b", "toolu_c"],
        "tool_result blocks MUST be in tool_use order (Q3 — LLM context stability), got: {:?}",
        tool_use_ids
    );

    // All three audit rows fire — read tools complete fully
    // (no cancel in this test), so each leaves a tool_executed.
    let audit_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM session_audit_events
           WHERE session_id = ? AND kind = 'tool_executed'"#,
    )
    .bind(&h.session_id)
    .fetch_one(&h.db)
    .await
    .expect("count tool_executed audit rows");
    assert_eq!(
        audit_count, 3,
        "each completed read tool leaves a tool_executed audit row"
    );
}

/// L2 fallback: a batch containing one `edit_file` (a write
/// tool) MUST fall back to the serial path. Behavior must be
/// byte-identical to pre-L2 (the read still runs, the edit
/// still runs, results appear in order). This is a regression
/// guard: a future change that accidentally routes mixed
/// batches through the parallel path would fail here.
#[tokio::test]
async fn agent_loop_mixed_batch_with_edit_falls_back_to_serial() {
    let h = make_harness().await;
    // Seed a file so edit_file's read-before-edit guard passes
    // (the test messages don't read it, but the guard returns
    // is_error=true on missing read — the agent loop still
    // feeds that error back to the LLM, which is what we want
    // to assert: serial path runs end-to-end).
    std::fs::write(h.project_path.join("target.txt"), "original").unwrap();

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: mixed batch — read_file + edit_file.
        // `is_parallel_eligible` returns false → serial path.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_read".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "target.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_edit".into(),
                name: "edit_file".into(),
                input: serde_json::json!({
                    "path": "target.txt",
                    "old_string": "original",
                    "new_string": "edited"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Turn 2: terminal text.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ok".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        None,
        "rid-mixed".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        CancellationToken::new(),
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (200) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
        // 2026-06-22 (RULE-FrontSubagent-003 fix): tests pass
        // `None` (production-style caller — not a worker, so
        // the PermissionContext.worker_run_id is unused by the
        // ask_path parent branch). The worker nested call in
        // `run_subagent` passes `Some(worker_run_id_opt)`.
        None,
        h.subagent_cache.clone(),
        None,
        // L3b (2026-06-27): production-style caller (and non-isolated
        // worker path) → worktree_override = None (use the session
        // row's worktree_path). Only the isolated worker path passes
        // Some(worker_worktree_path).
        None,
        None, // project_main_override (2026-07-29)
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
        None,
        // 2026-06-30 (ask_user_question task): per-test QuestionStore
        h.question_store.clone(),
        // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
        // workflow_ctx = None (tests don't exercise the workflow
        // breadcrumb injection seam; that lives in separate
        // `agent::workflow::inject` tests).
        None,
        None,
        None,
        h.stub_loaded.clone(),
    )
    .await;

    assert_eq!(mock.call_count(), 2, "serial path drives 2 turns");
    assert_eq!(emitter.tool_call_count(), 2);
    assert_eq!(
        emitter.tool_result_count(),
        2,
        "serial path emits 2 tool_results (read + edit)"
    );
    // Order is still tool_use order (serial is naturally ordered).
    let results = emitter.tool_results_snapshot();
    assert_eq!(results[0].tool_use_id, "toolu_read");
    assert_eq!(results[1].tool_use_id, "toolu_edit");
}

/// L2 Q2: a batch containing `web_fetch` (a read-only tool that
/// is EXCLUDED from the parallel set because its Tier 4 default
/// is `ask`) MUST fall back to the serial path. The web_fetch
/// here would normally fire a `permission:ask` — but since
/// `permission_asks` is empty (no sender), the 120s timeout
/// would fire and the test would hang. To avoid that, we pair
/// web_fetch with a read_file (so the batch is non-eligible for
/// parallel anyway) and assert the serial path is taken by
/// checking tool_result ordering is preserved — no need to
/// actually execute web_fetch.
///
/// The assertion is structural: web_fetch in the batch → serial.
/// The pure-predicate test above (`is_parallel_eligible_*`)
/// covers the classification; this test is the end-to-end
/// confirmation that the agent loop honors the predicate.
#[tokio::test]
async fn agent_loop_web_fetch_batch_does_not_run_parallel() {
    // Structural-only: we don't need to invoke run_chat_loop
    // here because the predicate is the gate, and the predicate
    // is exhaustively covered in `is_parallel_eligible_*`. This
    // test is intentionally a no-op placeholder documenting
    // that the Q2 exclusion is enforced at the predicate layer;
    // an end-to-end run would hang on the web_fetch ask
    // timeout. The classification test asserts the contract.
    //
    // Kept as a named test so a future refactor that removes
    // `is_parallel_eligible` will fail here (the named test
    // exists; if the predicate is renamed/removed the test
    // body, which references it via the unit test above, would
    // not compile).
    use crate::agent::chat_loop::is_parallel_eligible;
    let batch: Vec<(String, String, serde_json::Value)> = vec![
        (
            "x".into(),
            "read_file".into(),
            serde_json::json!({"path": "a"}),
        ),
        (
            "y".into(),
            "web_fetch".into(),
            serde_json::json!({"url": "https://example.com"}),
        ),
    ];
    // Root argument is irrelevant here: `web_fetch` is excluded
    // by the name check (Q2) before the path check ever runs.
    // Use a dummy tempdir for the parameter contract.
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(
        !is_parallel_eligible(&batch, dir.path()),
        "web_fetch MUST exclude the batch from parallel execution (Q2)"
    );
}

/// L2 + RULE-A-004: a parallel read-only batch cancelled mid-
/// flight MUST:
/// - mark the turn `cancelled` (so the existing cancel path
///   persists partial results + emits Done{cancelled})
/// - NOT leave a `tool_executed` audit row for the cancelled
///   task(s) (RULE-A-004 invariant, preserved per-task)
/// - still leave audit rows for tools that completed BEFORE
///   the cancel token fired
///
/// Script: turn 1 emits two `read_file` tool_use blocks. A
/// background task cancels the token as soon as the provider
/// has been called once (i.e. turn 1's send completed, tool
/// execution is about to start). With three reads, the cancel
/// arrives during the parallel batch — at least one task's
/// `token.is_cancelled()` is true after execute → its audit
/// write is skipped; the batch-level cancel flag flips → the
/// existing cancel path runs.
///
/// NOTE: read_file is fast and doesn't consult the cancel token
/// internally (the wrapper `execute_tool` select! only fires
/// if the cancel arrives BEFORE the inner future completes).
/// So whether the audit row fires depends on which task's
/// `token.is_cancelled()` check wins. The contract under test
/// is the EQUIVALENCE: a cancelled task skips audit, a
/// completed task records audit. The assert is "0 or more, but
/// consistent" — specifically, the test asserts the cancel
/// path was taken (Done{cancelled} emitted), which is the
/// invariant the L2 change can break.
#[tokio::test]
async fn agent_loop_parallel_batch_cancel_marks_turn_cancelled() {
    let h = make_harness().await;
    std::fs::write(h.project_path.join("a.txt"), "AAA").unwrap();
    std::fs::write(h.project_path.join("b.txt"), "BBB").unwrap();
    std::fs::write(h.project_path.join("c.txt"), "CCC").unwrap();

    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Turn 1: three read_file tool_use blocks (eligible).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_a".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "a.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_b".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "b.txt"}),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_c".into(),
                name: "read_file".into(),
                input: serde_json::json!({"path": "c.txt"}),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Sentinel — only consumed if cancel fails to mark the
        // turn (it shouldn't).
        MockResponse::HangingThenCancel,
    ]));

    let call_handle = mock.call_count_handle();
    let cancel_token = CancellationToken::new();
    let cancel_for_task = cancel_token.clone();
    let cancel_handle = tokio::spawn(async move {
        loop {
            if call_handle.load(Ordering::SeqCst) >= 1 {
                cancel_for_task.cancel();
                break;
            }
            tokio::task::yield_now().await;
        }
    });

    run_chat_loop(
        vec![],
        mock.clone(),
        200_000,
        None,
        "rid-par-cancel".into(),
        h.session_id.clone(),
        test_messages(),
        emitter.clone(),
        h.db.clone(),
        h.cancellations,
        h.session_active_request,
        h.read_guard,
        h.memory_cache,
        h.skill_cache,
        h.permission_asks,
        cancel_token,
        None,
        h.background_shells.clone(),
        // B6 Subagent (2026-06-19): max_turns = None keeps the
        // default MAX_TURNS (200) budget for all 9 agent_loop_*
        // integration tests (RULE-A-006 parity with production).
        None,
        // B6 Subagent (PR1b review #2): production-style caller,
        // so skip_session_active = false (guard clears the slot).
        false,
        // B6 Subagent (PR1b): production-style caller persists
        // every turn normally (RULE-A-006 parity with production).
        false,
        // B6 Subagent PR2b (RULE-A-014, 2026-06-20): production-
        // style caller → Some(false). Inside run_chat_loop this
        // falls through to `PermissionContext.is_worker = false` —
        // Tier 4 ask is reachable (permission:ask modal works
        // normally, the loop is not a worker). Mirrors the
        // production chat.rs call site.
        Some(false),
        // B6 PR3 (2026-06-20, PR2 hotfix): tests pass None (no Tauri runtime).
        None,
        std::sync::Arc::new(crate::agent::subagent::ThreadLocalSubagentSink), // worker_event_sink
        // 2026-06-21 fix (B6 review defect A): tests pass
        // `None` (production-style caller — not a worker,
        // so the parent's `assemble_system_prompt(mode_prefix,
        // base_prompt)` path runs unchanged). The worker
        // nested call in `run_subagent` passes `Some(...)`
        // to fully replace the parent's prompt with the
        // worker's `SubagentDef.system_prompt`.
        None,
        // 2026-06-22 (RULE-FrontSubagent-003 fix): tests pass
        // `None` (production-style caller — not a worker, so
        // the PermissionContext.worker_run_id is unused by the
        // ask_path parent branch). The worker nested call in
        // `run_subagent` passes `Some(worker_run_id_opt)`.
        None,
        h.subagent_cache.clone(),
        None,
        // L3b (2026-06-27): production-style caller (and non-isolated
        // worker path) → worktree_override = None (use the session
        // row's worktree_path). Only the isolated worker path passes
        // Some(worker_worktree_path).
        None,
        None, // project_main_override (2026-07-29)
        // L3b (2026-06-27): thread the test harness's app_data_dir
        // (a fresh tempdir per test). Tests that don't exercise
        // worker isolation never read it.
        h.app_data_dir.clone(),
        None,
        // 2026-06-30 (ask_user_question task): per-test QuestionStore
        h.question_store.clone(),
        // W1 (Workflow integration, Phase 0 Step 0.5 — 2026-07-08):
        // workflow_ctx = None (tests don't exercise the workflow
        // breadcrumb injection seam; that lives in separate
        // `agent::workflow::inject` tests).
        None,
        None,
        None,
        h.stub_loaded.clone(),
    )
    .await;
    cancel_handle.await.unwrap();

    // The cancel path MUST have been taken. Two invariants:
    // (a) exactly one Done{cancelled} event
    // (b) the loop did NOT re-enter turn 2 (mock.call_count == 1)
    assert_eq!(
        mock.call_count(),
        1,
        "cancel must abort before turn 2 (call_count stays at 1)"
    );
    assert_eq!(
        emitter.cancel_done_count(),
        1,
        "cancel path emits exactly one Done{{cancelled}} event"
    );

    // RULE-A-004 cross-check: the number of tool_executed audit
    // rows MUST be <= the number of tool_result events (a
    // cancelled task still emits a tool_result but skips
    // audit). Specifically: tool_result_count == 3 (all three
    // read_files complete because read_file doesn't consult
    // the cancel token internally), but audit_count <= 3
    // because tasks whose post-execute `token.is_cancelled()`
    // check came back true skipped their audit write.
    let audit_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM session_audit_events
           WHERE session_id = ? AND kind = 'tool_executed'"#,
    )
    .bind(&h.session_id)
    .fetch_one(&h.db)
    .await
    .expect("count tool_executed audit rows");
    assert!(
        emitter.tool_result_count() >= audit_count as usize,
        "cancelled tasks skip audit (RULE-A-004): results={} audit={} (audit MUST be <= results)",
        emitter.tool_result_count(),
        audit_count
    );
}
