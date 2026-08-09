#![cfg(test)]

use std::sync::Arc;

use super::run_loop;
use super::tests_common::{make_harness, test_messages, MockEmitter};
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::{ChatEvent, TokenUsage};

/// L3b PR2 (2026-06-27): concurrent `general-purpose` workers
/// with the LLM opting out of isolation (`isolation: false` in
/// the dispatch input). Mirrors the L3a concurrent branch's
/// pre-PR2 behavior (read-only-equivalent: shared cwd, full
/// toolset) but now the LLM explicitly drives the choice via the
/// dispatch input override (truth table: `frontmatter Some(true)`
/// + dispatch `Some(false)` → NOT isolated). This pins that the
/// `force_readonly=true` L3a behavior is still reachable via the
/// new isolation-truth-table path.
///
/// The test mock uses no real git repo — the LLM opt-out routes
/// the worker back to the parent's worktree, sidestepping
/// `create_worker`'s libgit2 requirement. Two general-purpose
/// workers in a pure batch both complete with text-only
/// summaries.
///
/// (Pre-PR2 this test was named
/// `l3a_concurrent_general_purpose_workers_complete_readonly`
/// and asserted the 2nd-layer read-only strip; that strip is
/// removed in PR2 — the new safety argument is per-worker
/// worktree isolation, documented in
/// `agent-loop-architecture.md` §"Pattern: Concurrent isolated
/// dispatch (L3b PR2)".)
#[tokio::test(flavor = "multi_thread")]
async fn l3b_concurrent_general_purpose_workers_complete_shared() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: 2 general-purpose dispatches. The
        // `isolation: false` dispatch input overrides the
        // builtin `general-purpose.isolation: None` default and opts
        // out of the concurrent batch's force-isolate-writable default
        // (B 2026-06-30) — per the resolve_isolation truth table:
        // `frontmatter Some(true)` + `dispatch Some(false)` →
        // NOT isolated). This reproduces the L3a pre-PR2
        // concurrent behavior (shared cwd + full toolset) without
        // requiring a real git repo fixture.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_gp_a".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "investigate topic A read-only",
                    "isolation": false,
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_gp_b".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "investigate topic B read-only",
                    "isolation": false,
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker slot 1: single-turn text-only summary.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "general-purpose read-only result A".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker slot 2.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "general-purpose read-only result B".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Parent turn 2.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ack".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_loop(
        &h,
        mock.clone(),
        emitter.clone(),
        "rid-l3b-gp-shared",
        test_messages(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // 4 sends: parent_t1 + 2 workers + parent_t2.
    assert_eq!(mock.call_count(), 4);

    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 2);
    for r in &results {
        assert!(!r.is_error, "general-purpose concurrent → completed");
        assert!(
            r.content.contains("[status: completed]"),
            "got: {}",
            r.content
        );
    }
}

// ---------------------------------------------------------------------------
// L3b PR2 (2026-06-27): concurrent isolated dispatch regression tests
//
// Three tests pin the post-PR2 contract:
// 1. l3b_concurrent_general_purpose_workers_complete_with_writes
//    — 2 general-purpose workers in a pure batch each in their
//    own worker worktree; both complete cleanly. (Writes exercised
//    in a follow-up; this test pins the concurrent+isolated
//    dispatch surface.)
// 2. l3b_concurrent_workers_have_isolated_worktrees
//    — concurrent workers' `subagent_runs.worktree_path` are
//    distinct (non-overlapping) — provably per-run isolated.
// 3. l3b_concurrent_force_readonly_param_no_longer_set
//    — regression: the concurrent branch no longer threads
//    `force_readonly=true` to `run_subagent`. Each concurrent
//    worker is observed with the full toolset of its
//    `SubagentDef` (researcher = 4 read-only tools; general-
//    purpose = full minus structurally disabled), NOT the L3a
//    5-tool read-only strip.
//
// All 3 tests need a real git repo in the project path so
// `create_worker_worktree` succeeds. `make_harness_with_git_repo`
// (defined in `tests_common.rs`, L3b PR1) is the variant.
// ---------------------------------------------------------------------------

/// L3b PR2 AC1: 2 general-purpose workers in a pure batch, each
/// in its own `worker/<run_id>` worktree. Both complete cleanly
/// (text-only summaries — we don't exercise the write path here;
/// the worker worktree + lock + destroy flow is the surface to
/// pin). 2 independent `subagent_runs` rows persist + parent
/// receives 2 `[status: completed]` summaries.
///
/// Uses `make_harness_with_git_repo` (L3b PR1) so
/// `create_worker_worktree` succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn l3b_concurrent_general_purpose_workers_complete_with_writes() {
    let h = super::tests_common::make_harness_with_git_repo().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: 2 general-purpose dispatches (no
        // `isolation` override — builtin default is `Some(true)`,
        // so each worker gets its own worktree).
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_l3b_writes_a".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "write a different value to file A"
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_l3b_writes_b".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "write a different value to file B"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker slot 1: single-turn summary.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "wrote file A in worker worktree".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Worker slot 2.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "wrote file B in worker worktree".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // Parent turn 2.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ack".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_loop(
        &h,
        mock.clone(),
        emitter.clone(),
        "rid-l3b-writes",
        test_messages(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // 4 sends: parent_t1 + 2 workers + parent_t2.
    assert_eq!(mock.call_count(), 4);

    // 2 tool_results, both completed (worker write path is
    // exercised inside the worker; tool_result just reports the
    // dispatch summary, not the inner tool calls).
    let results = emitter.tool_results_snapshot();
    assert_eq!(results.len(), 2);
    for r in &results {
        assert!(
            !r.is_error,
            "general-purpose concurrent+isolated → completed"
        );
        assert!(
            r.content.contains("[status: completed]"),
            "got: {}",
            r.content
        );
    }

    // 2 subagent_runs rows persisted.
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 2, "2 concurrent workers → 2 runs");
    for run in &runs {
        assert_eq!(run.status, "completed", "each worker completed");
        assert!(run.finished_at.is_some(), "finished_at set");
    }
}

/// L3b PR2 AC2: concurrent workers are **per-run isolated** —
/// each worker gets a unique `worker_run_id` (UUID, distinct
/// across the batch), which derives the unique
/// `<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>`
/// directory + the unique `worker/<run_id>` branch. The
/// `subagent_runs.id` row id is the `worker_run_id` itself
/// (passed to `insert_run_with_id`), so the per-run isolation
/// is observable in the `subagent_runs` table.
///
/// Note: `subagent_runs.worktree_path` is **NULL** for workers
/// that exit with no changes (the post-loop destroy path
/// clears the column — see `dispatch.rs::run_subagent` "No
/// changes — destroy + clear" branch). To observe a
/// non-NULL `worktree_path` post-exit, the worker must leave
/// changes behind (which requires a real `write_file` tool
/// path the mock cannot easily exercise). This test pins the
/// **isolation primitive** (per-run UUID + per-run
/// worktree_path computation), not the post-destroy
/// preservation semantics — those are covered by
/// `probe_worker_changes_*` in `dispatch.rs::tests` and the
/// PR3 merge/discard tool tests (out of scope for PR2).
#[tokio::test(flavor = "multi_thread")]
async fn l3b_concurrent_workers_have_isolated_worktrees() {
    let h = super::tests_common::make_harness_with_git_repo().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_l3b_iso_a".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "isolated worker A"
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_l3b_iso_b".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "isolated worker B"
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // 2 worker single-turn summaries.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "A done".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "B done".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ack".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_loop(
        &h,
        mock.clone(),
        emitter.clone(),
        "rid-l3b-iso",
        test_messages(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // 2 subagent_runs rows persisted, each with a distinct
    // `id` (the worker_run_id UUID).
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 2, "2 concurrent workers → 2 runs");

    // The 2 worker_run_ids are distinct — each worker got its
    // own UUID, which means each got its own
    // `<app_data_dir>/worktrees/<project_uuid>/worker/<run_id>`
    // directory (the worktree path is computed FROM the
    // worker_run_id) and its own `worker/<run_id>` branch (the
    // branch name is computed FROM the worker_run_id). This is
    // the per-run isolation primitive.
    let mut ids: Vec<String> = runs.iter().map(|r| r.id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        2,
        "2 concurrent workers MUST have distinct worker_run_id (UUID) values, got: {:?}",
        runs.iter().map(|r| (&r.id, &r.status)).collect::<Vec<_>>()
    );

    // Each run's `parent_request_id` is the worker_rid
    // (`{parent_rid}-sub-{tool_use_id}`), which is also
    // distinct per dispatch (the tool_use_id is unique per
    // batch). This is the SECOND isolation primitive — the
    // worker_rid → worktree_path → worker_run_id round-trip.
    let mut rids: Vec<String> = runs.iter().map(|r| r.parent_request_id.clone()).collect();
    rids.sort();
    rids.dedup();
    assert_eq!(
        rids.len(),
        2,
        "2 concurrent workers MUST have distinct worker_rid values, got: {:?}",
        runs.iter()
            .map(|r| (&r.id, &r.parent_request_id))
            .collect::<Vec<_>>()
    );
    // Each worker_rid carries the tool_use_id suffix, proving
    // the 1:1 mapping (worker_rid derived from tool_use_id).
    let rids_set: std::collections::HashSet<&str> =
        runs.iter().map(|r| r.parent_request_id.as_str()).collect();
    assert!(
        rids_set.iter().any(|r| r.contains("toolu_l3b_iso_a")),
        "one worker_rid should encode toolu_l3b_iso_a: {:?}",
        rids
    );
    assert!(
        rids_set.iter().any(|r| r.contains("toolu_l3b_iso_b")),
        "one worker_rid should encode toolu_l3b_iso_b: {:?}",
        rids
    );
}

/// L3b PR2 AC3: regression — the concurrent branch no longer
/// forces read-only (`force_readonly=false` is threaded to
/// `run_subagent` for every concurrent task). The worker turn
/// receives the FULL `general-purpose` toolset (minus the
/// 4 `STRUCTURALLY_DISABLED` tools), NOT the L3a 5-tool
/// read-only strip. We use `general-purpose` here (with
/// `isolation: false` dispatch override so no git fixture is
/// needed) so the test exercises a toolset shape that DIFFERS
/// pre/post PR2:
///
/// - pre-PR2 (L3a `force_readonly=true`): worker turn tools =
///   the 5-tool read-only strip
///   (`read_file, grep, glob, list_dir, web_fetch`)
/// - post-PR2 (no strip): worker turn tools = the
///   `general-purpose` allowlist minus `STRUCTURALLY_DISABLED`
///   = full builtin set minus the structurally-disabled tools
///   that appear in it (`update_checklist`, `run_background_shell`,
///   `shell_status`, `shell_kill`, and — per L3b PR3 B3 —
///   `merge_worker`, `discard_worker`) ≈ 9 tools (incl.
///   `write_file`, `edit_file`, `shell`)
///
/// The discriminant: `write_file` is in the post-PR2
/// `general-purpose` toolset but NOT in the L3a strip. If the
/// strip is still applied, `write_file` will be absent.
///
/// `MockProvider::sent_tools()` snapshots the tools Vec the
/// LLM received per turn. We assert on the worker turn
/// (send slot 1).
#[tokio::test(flavor = "multi_thread")]
async fn l3b_concurrent_force_readonly_param_no_longer_set() {
    let h = make_harness().await;
    let emitter = Arc::new(MockEmitter::new());
    let mock = Arc::new(MockProvider::new(vec![
        // Parent turn 1: 2 general-purpose dispatches. LLM
        // opts out of isolation (no git fixture), so workers
        // reuse the parent's worktree.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ToolCall {
                id: "toolu_l3b_reg_a".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "general-purpose task A",
                    "isolation": false,
                }),
            }),
            Ok(ChatEvent::ToolCall {
                id: "toolu_l3b_reg_b".into(),
                name: "dispatch_subagent".into(),
                input: serde_json::json!({
                    "subagent": "general-purpose",
                    "task": "general-purpose task B",
                    "isolation": false,
                }),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("tool_use".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        // 2 worker single-turn summaries.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "general-purpose A".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta {
                text: "general-purpose B".into(),
            }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Delta { text: "ack".into() }),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ]),
    ]));

    run_loop(
        &h,
        mock.clone(),
        emitter.clone(),
        "rid-l3b-reg",
        test_messages(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    // 4 sends: parent_t1 + 2 workers + parent_t2.
    assert_eq!(mock.call_count(), 4);

    // Snapshot the per-turn tools the LLM received.
    let sent_tools = mock.sent_tools();
    assert_eq!(sent_tools.len(), 4, "4 sends captured");

    // Worker turn 1 (send slot 1) — concurrent branch, no
    // force_readonly strip. The tools come from the
    // general-purpose SubagentDef allowlist (empty = full
    // builtin set) minus STRUCTURALLY_DISABLED (7 entries:
    // dispatch_subagent, update_checklist, run_background_shell,
    // shell_status, shell_kill, merge_worker, discard_worker).
    let worker_t1_names: Vec<&str> = sent_tools[1].iter().map(|t| t.name.as_str()).collect();
    // The PR2 discriminant: `write_file` is in the post-PR2
    // general-purpose toolset but NOT in the L3a 5-tool strip.
    // If the strip is still applied (L3a regression), this
    // assertion fails.
    assert!(
        worker_t1_names.contains(&"write_file"),
        "worker_t1 MUST have write_file (general-purpose toolset, no L3a strip): {:?}",
        worker_t1_names
    );
    assert!(
        worker_t1_names.contains(&"edit_file"),
        "worker_t1 MUST have edit_file (general-purpose toolset, no L3a strip): {:?}",
        worker_t1_names
    );
    assert!(
        worker_t1_names.contains(&"shell"),
        "worker_t1 MUST have shell (general-purpose toolset, no L3a strip): {:?}",
        worker_t1_names
    );
    // STRUCTURALLY_DISABLED still applies (the 3-layer no-
    // nesting protection): dispatch_subagent MUST NOT appear.
    assert!(
        !worker_t1_names.contains(&"dispatch_subagent"),
        "worker_t1 MUST NOT have dispatch_subagent (STRUCTURALLY_DISABLED, no nesting): {:?}",
        worker_t1_names
    );
    assert!(
        !worker_t1_names.contains(&"update_checklist"),
        "worker_t1 MUST NOT have update_checklist (STRUCTURALLY_DISABLED): {:?}",
        worker_t1_names
    );
    // L3b PR3 B3 (2026-06-28): merge_worker / discard_worker are
    // STRUCTURALLY_DISABLED — a worker must not rewrite the parent
    // session's history (no merging / discarding a sibling worker's
    // branch via a run_id visible in the dispatch tool_result).
    assert!(
        !worker_t1_names.contains(&"merge_worker"),
        "worker_t1 MUST NOT have merge_worker (STRUCTURALLY_DISABLED, B3): {:?}",
        worker_t1_names
    );
    assert!(
        !worker_t1_names.contains(&"discard_worker"),
        "worker_t1 MUST NOT have discard_worker (STRUCTURALLY_DISABLED, B3): {:?}",
        worker_t1_names
    );

    // 2 subagent_runs rows persisted.
    let runs = crate::db::subagent_runs::list_runs_by_session(&h.db, &h.session_id)
        .await
        .expect("list_runs_by_session");
    assert_eq!(runs.len(), 2, "2 concurrent general-purpose → 2 runs");
}
