#![cfg(test)]

// ---------------------------------------------------------------------------
// L3b PR3 (2026-06-27): merge_worker / discard_worker tool tests
//
// Tests pin the post-merge / post-discard DB state + libgit2
// state. Each test sets up a project + parent session worktree
// + worker worktree via `make_harness_with_git_repo` (so
// `create_worker` succeeds), then invokes the tool helper
// directly to avoid the full agent loop harness.
//
// The `merge_worker` tool exercises:
// - fast-forward path (no common ancestor between parent and
//   worker — actually the fast-forward path triggers when
//   the parent tip IS an ancestor of the worker tip)
// - 3-way merge path (parent has diverged)
// - conflict path (both branches modify the same file)
//
// The `discard_worker` tool exercises:
// - happy path: worktree_path set → destroy + clear
// - fail-fast on already-destroyed run
// ---------------------------------------------------------------------------

/// L3b PR3 AC1: `merge_worker` happy path. Worker makes a
/// change on a separate file; `merge_worker` fast-forwards
/// the parent branch and clears the worktree_path column.
#[tokio::test(flavor = "multi_thread")]
async fn l3b_merge_worker_happy_path_fast_forward() {
    use crate::git::worktree::worker_worktree_path;

    // Set up the harness + a parent session worktree (the
    // agent loop will operate on this).
    let h = super::tests_common::make_harness_with_git_repo().await;

    // We need a parent session worktree attached so the
    // tool can find the parent branch. We simulate the
    // attach by directly setting the worktree state.
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

    // The worker run id is a fixed UUID so we can verify
    // the row state.
    let run_id = "00000000-0000-0000-0000-000000000001";
    // Insert a subagent_runs row with worktree_path set +
    // create a worker worktree on a separate file.
    let worker_wt = worker_worktree_path(&h.app_data_dir, &h.project_id, run_id);
    crate::git::worktree::create_worker(&h.project_path, &worker_wt, &wt_path, run_id)
        .expect("create worker worktree");
    // The worker creates a new file in its worktree and
    // commits it on the worker branch.
    std::fs::write(worker_wt.join("worker_change.txt"), "from worker").unwrap();
    let add = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&worker_wt)
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = std::process::Command::new("git")
        .args(["commit", "-m", "worker commit", "--no-gpg-sign"])
        .current_dir(&worker_wt)
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "worker commit failed: {:?}",
        commit
    );

    // Insert a subagent_runs row referencing the worktree.
    crate::db::subagent_runs::insert_run_with_id(
        &h.db,
        run_id,
        &h.session_id,
        "parent-rid-l3b-mw-happy",
        "general-purpose",
        Some("write a file"),
        // 2026-07-03 (task 07-03-subagent-per-agent-model-ui): test
        // fixtures don't carry a model display (the merge_worker
        // path is the worktree path, not the model); pass `None`
        // for `model_display` (legacy compat shape).
        None,
    )
    .await
    .expect("insert_run_with_id");
    crate::db::subagent_runs::set_worktree_path(&h.db, run_id, Some(worker_wt.to_str().unwrap()))
        .await
        .expect("set_worktree_path");

    // Build the ToolContext the merge_worker tool needs.
    let ctx = crate::tools::ToolContext {
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

    // Invoke merge_worker.
    let input = serde_json::json!({"run_id": run_id});
    let (msg, is_err, _update, _exit_code) =
        crate::tools::merge_worker::execute(&input, &ctx, Some(&h.session_id)).await;
    assert!(!is_err, "merge_worker should succeed: {}", msg);
    assert!(
        msg.contains("fast-forward") || msg.contains("3-way"),
        "merge message should describe the merge kind: {}",
        msg
    );

    // The parent worktree should now have the worker's
    // file (because the parent branch was fast-forwarded).
    assert!(
        wt_path.join("worker_change.txt").exists(),
        "parent worktree should have the worker's file post-merge: {:?}",
        std::fs::read_dir(&wt_path).unwrap().collect::<Vec<_>>()
    );

    // The worker worktree + branch are destroyed; the
    // worktree_path column is cleared.
    assert!(!worker_wt.exists(), "worker worktree dir removed");
    let updated_run = crate::db::subagent_runs::get_run(&h.db, run_id)
        .await
        .expect("get_run")
        .expect("run row should exist");
    assert!(
        updated_run.worktree_path.is_none(),
        "worktree_path column cleared post-merge: {:?}",
        updated_run.worktree_path
    );
}

/// L3b PR3 AC2: `merge_worker` conflict path. Both branches
/// modify the same file → `is_error: true` with the conflict
/// file list; the worker branch + worktree are preserved.
#[tokio::test(flavor = "multi_thread")]
async fn l3b_merge_worker_conflict_returns_error() {
    use crate::git::worktree::worker_worktree_path;

    let h = super::tests_common::make_harness_with_git_repo().await;

    // Parent session worktree (from initial commit only).
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

    // Worker worktree — also from the initial commit (NOT
    // from the parent branch, which would create a
    // fast-forward path). We have to construct the
    // worker_wt branch ourselves because `create_worker`
    // bases off `parent_wt.head()` (the parent's current
    // tip). To create a true 3-way conflict, we want
    // both branches to fork from the same base.
    //
    // Strategy: pre-create the worker branch pointing at
    // the project HEAD, then `create_worker` will fast-
    // forward it to the worker's first commit (the
    // conflict-creating change).
    let run_id = "00000000-0000-0000-0000-000000000002";
    let worker_wt = worker_worktree_path(&h.app_data_dir, &h.project_id, run_id);
    // Use `create_worker` from a synthetic parent_wt that
    // has the project HEAD (so the worker bases on the
    // initial commit, not the parent's later commit).
    let temp_parent = h.project_path.join(format!("temp_parent_{}", run_id));
    let _ = std::fs::remove_dir_all(&temp_parent);
    crate::git::worktree::create(&h.project_path, &temp_parent, &format!("temp-p-{}", run_id))
        .expect("create temp parent worktree");
    crate::git::worktree::create_worker(&h.project_path, &worker_wt, &temp_parent, run_id)
        .expect("create worker worktree");
    // Clean up the temp parent worktree (we don't need
    // it anymore; the worker branch is now based off the
    // project HEAD, which is what we want for a 3-way
    // conflict).
    let temp_branch = format!("session/temp-p-{}", run_id);
    crate::git::worktree::destroy_worker(
        &h.project_path,
        &temp_parent,
        &format!("temp-p-{}", run_id),
    )
    .expect("destroy temp worker");
    let _ = std::fs::remove_dir_all(&temp_parent);
    if let Ok(repo) = git2::Repository::open(&h.project_path) {
        if let Ok(mut b) = repo.find_branch(&temp_branch, git2::BranchType::Local) {
            let _ = b.delete();
        }
    }

    // Parent branch modifies seed.txt (commits on
    // session/<sid>).
    std::fs::write(wt_path.join("seed.txt"), "from parent").unwrap();
    let add = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&wt_path)
        .output()
        .unwrap();
    assert!(add.status.success());
    let commit = std::process::Command::new("git")
        .args(["commit", "-m", "parent edit", "--no-gpg-sign"])
        .current_dir(&wt_path)
        .output()
        .unwrap();
    assert!(
        commit.status.success(),
        "parent commit failed: {:?}",
        commit
    );

    // Worker modifies seed.txt differently (commits on
    // worker/<run_id>).
    std::fs::write(worker_wt.join("seed.txt"), "from worker").unwrap();
    let w_add = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(&worker_wt)
        .output()
        .unwrap();
    assert!(w_add.status.success());
    let w_commit = std::process::Command::new("git")
        .args(["commit", "-m", "worker edit", "--no-gpg-sign"])
        .current_dir(&worker_wt)
        .output()
        .unwrap();
    assert!(w_commit.status.success());

    crate::db::subagent_runs::insert_run_with_id(
        &h.db,
        run_id,
        &h.session_id,
        "parent-rid-l3b-mw-conflict",
        "general-purpose",
        Some("edit seed.txt"),
        // 2026-07-03 (task 07-03-subagent-per-agent-model-ui): test
        // fixtures pass `None` for `model_display` (no model
        // resolution needed for the merge_worker conflict path).
        None,
    )
    .await
    .expect("insert_run_with_id");
    crate::db::subagent_runs::set_worktree_path(&h.db, run_id, Some(worker_wt.to_str().unwrap()))
        .await
        .expect("set_worktree_path");

    let ctx = crate::tools::ToolContext {
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
        crate::tools::merge_worker::execute(&input, &ctx, Some(&h.session_id)).await;
    if !is_err {
        eprintln!("merge_worker succeeded unexpectedly: {}", msg);
    }
    assert!(is_err, "merge_worker should fail on conflict (msg={})", msg);
    assert!(
        msg.contains("merge conflict"),
        "error should describe the conflict: {}",
        msg
    );
    assert!(
        msg.contains("seed.txt"),
        "conflict list should mention seed.txt: {}",
        msg
    );

    // Worker worktree + branch are PRESERVED.
    assert!(
        worker_wt.exists(),
        "worker worktree dir preserved on conflict"
    );
    let repo = git2::Repository::open(&h.project_path).unwrap();
    assert!(
        repo.find_branch(&format!("worker/{}", run_id), git2::BranchType::Local)
            .is_ok(),
        "worker branch preserved on conflict"
    );
    // The subagent_runs row's worktree_path is still set
    // (the cleanup finalize was NOT called because the
    // merge didn't succeed).
    let run_row = crate::db::subagent_runs::get_run(&h.db, run_id)
        .await
        .expect("get_run")
        .expect("run row should exist");
    assert!(
        run_row.worktree_path.is_some(),
        "worktree_path column preserved on conflict: {:?}",
        run_row.worktree_path
    );
}

/// L3b PR3 AC9: `merge_worker` with no parent worktree
/// attached → "parent session has no worktree" error.
#[tokio::test(flavor = "multi_thread")]
async fn l3b_merge_worker_no_parent_worktree_errors() {
    let h = super::tests_common::make_harness_with_git_repo().await;

    // The session has NO worktree attached (we never call
    // attach_worktree). The session's `worktree_path` is
    // NULL.
    // Insert a fake subagent_runs row (no worktree_path
    // needed for this error — the check fires BEFORE the
    // worktree_path column is read).
    let run_id = "00000000-0000-0000-0000-000000000003";
    crate::db::subagent_runs::insert_run_with_id(
        &h.db,
        run_id,
        &h.session_id,
        "parent-rid-l3b-mw-no-wt",
        "general-purpose",
        Some("test"),
        // 2026-07-03 (task 07-03-subagent-per-agent-model-ui):
        // `merge_worker` no-worktree test fixture — `model_display`
        // is irrelevant to the "no worktree" error path; pass `None`.
        None,
    )
    .await
    .expect("insert_run_with_id");

    // The parent_wt path is some made-up path (not a real
    // worktree). The libgit2 error fires when we try to
    // open the repo + find the parent branch.
    let bogus_wt = h.project_path.join(format!("missing_wt_{}", h.session_id));
    let ctx = crate::tools::ToolContext {
        worktree_path: bogus_wt,
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
        crate::tools::merge_worker::execute(&input, &ctx, Some(&h.session_id)).await;
    assert!(is_err, "merge_worker should fail when no parent worktree");
    // The error path fires on `find_branch` — it should
    // mention the branch name OR the worktree path.
    assert!(
        msg.contains("not found") || msg.contains("could not open") || msg.contains("worktree"),
        "error should explain the failure: {}",
        msg
    );
}
