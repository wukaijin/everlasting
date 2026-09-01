#![cfg(test)]

/// L3b PR3 AC4: `discard_worker` happy path. Worker makes
/// a change → `discard_worker` → branch deleted +
/// worktree_path cleared.
#[tokio::test(flavor = "multi_thread")]
async fn l3b_discard_worker_happy_path() {
    use crate::git::worktree::worker_worktree_path;

    let h = super::tests_common::make_harness_with_git_repo().await;
    let wt_path = h.project_path.join(format!("parent_wt_{}", h.session_id));
    let _ = std::fs::remove_dir_all(&wt_path);
    crate::git::worktree::create(&h.project_path, &wt_path, &h.session_id)
        .expect("create parent session worktree");
    crate::db::set_worktree_state(
        &h.db,
        &h.session_id,
        crate::db::WorktreeState::Active,
        Some(wt_path.to_str().unwrap()),
        None,
    )
    .await
    .expect("set worktree_state active");

    let run_id = "00000000-0000-0000-0000-000000000004";
    let worker_wt = worker_worktree_path(&h.app_data_dir, &h.project_id, run_id);
    crate::git::worktree::create_worker(&h.project_path, &worker_wt, &wt_path, run_id)
        .expect("create worker worktree");
    // Worker makes a change but we don't care about
    // commit (discard is independent of commit state).
    std::fs::write(worker_wt.join("discard_me.txt"), "should be gone").unwrap();

    crate::db::subagent_runs::insert_run_with_id(
        &h.db,
        run_id,
        &h.session_id,
        "parent-rid-l3b-dw-happy",
        "general-purpose",
        Some("test discard"),
        // 2026-07-03 (task 07-03-subagent-per-agent-model-ui):
        // `discard_worker` happy-path test fixture — model display
        // is not part of the discard contract; pass `None`.
        None,
    )
    .await
    .expect("insert_run_with_id");
    crate::db::subagent_runs::set_worktree_path(&h.db, run_id, Some(worker_wt.to_str().unwrap()))
        .await
        .expect("set_worktree_path");

    let ctx = crate::tools::ToolContext {
        tool_use_id: None,
        escalation: Default::default(),
        worktree_path: wt_path.clone(),
        cwd: wt_path.clone(),
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db: h.db.clone(),
        project_id: "test-proj".to_string(),
        data_dir: h.app_data_dir.clone(),
        workflow_name: None,
        mode: crate::db::Mode::Edit,
    };

    let input = serde_json::json!({"run_id": run_id});
    let (msg, is_err, _update, _exit_code) =
        crate::tools::discard_worker::execute(&input, &ctx, Some(&h.session_id)).await;
    assert!(!is_err, "discard_worker should succeed: {}", msg);
    assert!(
        msg.contains("discarded"),
        "success message should confirm discard: {}",
        msg
    );

    // Worker worktree + branch are gone.
    assert!(!worker_wt.exists(), "worker worktree dir removed");
    let repo = git2::Repository::open(&h.project_path).unwrap();
    assert!(
        repo.find_branch(&format!("worker/{}", run_id), git2::BranchType::Local)
            .is_err(),
        "worker branch deleted"
    );
    // worktree_path column is cleared.
    let updated_run = crate::db::subagent_runs::get_run(&h.db, run_id)
        .await
        .expect("get_run")
        .expect("run row should exist");
    assert!(
        updated_run.worktree_path.is_none(),
        "worktree_path column cleared post-discard: {:?}",
        updated_run.worktree_path
    );
}

/// L3b PR3 AC10: `discard_worker` fail-fast on
/// already-destroyed run. `worktree_path` is NULL → error
/// `worker already destroyed`.
#[tokio::test(flavor = "multi_thread")]
async fn l3b_discard_worker_already_destroyed_errors() {
    let h = super::tests_common::make_harness_with_git_repo().await;
    let run_id = "00000000-0000-0000-0000-000000000005";

    // Insert a subagent_runs row WITHOUT setting
    // worktree_path (so it's NULL — the "already
    // destroyed" state).
    crate::db::subagent_runs::insert_run_with_id(
        &h.db,
        run_id,
        &h.session_id,
        "parent-rid-l3b-dw-already",
        "general-purpose",
        Some("test"),
        // 2026-07-03 (task 07-03-subagent-per-agent-model-ui):
        // `discard_worker` test fixture passes `None` for the new
        // `model_display` parameter (the test does not exercise
        // model resolution).
        None,
    )
    .await
    .expect("insert_run_with_id");
    // Do NOT call set_worktree_path — the column stays
    // NULL.

    let ctx = crate::tools::ToolContext {
        tool_use_id: None,
        escalation: Default::default(),
        worktree_path: h.project_path.clone(),
        cwd: h.project_path.clone(),
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db: h.db.clone(),
        project_id: "test-proj".to_string(),
        data_dir: h.app_data_dir.clone(),
        workflow_name: None,
        mode: crate::db::Mode::Edit,
    };
    let input = serde_json::json!({"run_id": run_id});
    let (msg, is_err, _update, _exit_code) =
        crate::tools::discard_worker::execute(&input, &ctx, Some(&h.session_id)).await;
    assert!(is_err, "discard_worker should fail on already-destroyed");
    assert!(
        msg.contains("already destroyed"),
        "error should be 'worker already destroyed': {}",
        msg
    );
}
