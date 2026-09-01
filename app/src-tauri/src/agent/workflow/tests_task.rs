#![cfg(test)]

use crate::agent::workflow::task::*;
use chrono::Utc;
use std::path::PathBuf;

/// Per-test scratch dir under `tempfile::tempdir()`. Auto-cleaned
/// when the `TempDir` is dropped at the end of each test, so
/// the working tree stays clean across `cargo test` runs.
fn fresh_project() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn proj(d: &tempfile::TempDir) -> PathBuf {
    d.path().to_path_buf()
}

// --- slug validation --------------------------------------------------

#[test]
fn validate_slug_accepts_lowercase_alphanumeric_with_hyphens() {
    assert!(validate_slug("dev").is_ok());
    assert!(validate_slug("my-feature").is_ok());
    assert!(validate_slug("a").is_ok());
    assert!(validate_slug("abc-123").is_ok());
    assert!(validate_slug(&"a".repeat(64)).is_ok(), "at the boundary");
}

#[test]
fn validate_slug_rejects_bad_chars_or_bounds() {
    for bad in [
        "",
        &"a".repeat(65),
        "-leading-hyphen",
        "trailing-hyphen-",
        "UPPER",
        "Mixed",
        "with space",
        "中文",
        "with_underscore",
        "with/slash",
        "with.dot",
    ] {
        assert!(
            validate_slug(bad).is_err(),
            "slug {:?} should be rejected",
            bad
        );
    }
}

// --- create + read round-trip ---------------------------------------

#[test]
fn create_task_init_writes_json_and_prd_skeleton() {
    let d = fresh_project();
    let task = create_task_init(
        &proj(&d),
        "My Feature",
        "my-feature",
        None,
        TaskStatus::Planning,
        "dev",
    )
    .expect("create");
    assert_eq!(task.title, "My Feature");
    assert_eq!(task.slug, "my-feature");
    assert_eq!(task.status, TaskStatus::Planning);
    assert!(task.summary.is_empty());
    assert!(task.items.is_empty());
    assert!(task.parent.is_none());

    // Files exist on disk.
    let json_path = task_json_path(&proj(&d), "my-feature");
    let prd_path = task_prd_path(&proj(&d), "my-feature");
    assert!(json_path.exists());
    assert!(prd_path.exists());

    // Round-trip: re-read the JSON and confirm structural identity.
    let again = read_task(&proj(&d), "my-feature").expect("read");
    assert_eq!(again.id, task.id, "id persists across read");
    assert_eq!(again.status, TaskStatus::Planning);
    assert_eq!(again.created_at, task.created_at);
    assert_eq!(again.updated_at, task.updated_at);
    assert!(again.items.is_empty());
}

#[test]
fn create_task_init_refuses_to_overwrite_existing() {
    let d = fresh_project();
    create_task_init(&proj(&d), "First", "dup", None, TaskStatus::Planning, "dev")
        .expect("first ok");
    let err = create_task_init(
        &proj(&d),
        "Second",
        "dup",
        None,
        TaskStatus::Planning,
        "dev",
    )
    .expect_err("must reject duplicate");
    assert!(matches!(err, TaskError::AlreadyExists(_)), "got {:?}", err);
}

#[test]
fn create_task_init_with_parent_records_parent_slug() {
    let d = fresh_project();
    let task = create_task_init(
        &proj(&d),
        "Sub",
        "sub-task",
        Some("parent-task"),
        TaskStatus::Planning,
        "dev",
    )
    .expect("create child");
    assert_eq!(task.parent.as_deref(), Some("parent-task"));
    let again = read_task(&proj(&d), "sub-task").expect("read child");
    assert_eq!(again.parent.as_deref(), Some("parent-task"));
}

#[test]
fn read_task_missing_returns_not_found_not_io_error() {
    let d = fresh_project();
    let err = read_task(&proj(&d), "nonexistent").expect_err("missing");
    assert!(
        matches!(err, TaskError::NotFound(_)),
        "got {:?}; the caller's 'no task yet' branch must be unambiguous",
        err
    );
}

// --- lenient parse (07-10-workflow-task-json-hardening R1) ------------
// task.json is a file the LLM can `write_file` directly, so the
// read side must tolerate hand-written schema drift (missing
// fields, checklist-style status values like "in_progress" /
// "pending") rather than crashing the whole workflow. Resilience
// lives on the read side, NOT by gating writes.

/// Write a raw (possibly hand-written / schema-drifting) task.json
/// for `slug`, creating the task dir. Simulates an LLM editing
/// task.json via write_file without going through create_task /
/// update_checklist — the exact pattern that crashed the workflow
/// twice during 07-10 dogfooding.
fn write_raw_task_json(project: &std::path::Path, slug: &str, body: &str) {
    let dir = task_dir(project, slug);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("task.json"), body).unwrap();
}

#[test]
fn read_task_lenient_missing_created_at_and_updated_at() {
    // Crash #1: LLM hand-wrote task.json without created_at /
    // updated_at. #[serde(default)] now fills empty strings.
    let d = fresh_project();
    write_raw_task_json(
        &proj(&d),
        "my-feat",
        r#"{"id":"t1","title":"T","slug":"my-feat","status":"planning","summary":"","items":[]}"#,
    );
    let task = read_task(&proj(&d), "my-feat").expect("lenient parse must succeed");
    assert_eq!(task.slug, "my-feat");
    assert_eq!(task.status, TaskStatus::Planning);
    assert!(
        task.created_at.is_empty(),
        "missing created_at → empty default"
    );
    assert!(
        task.updated_at.is_empty(),
        "missing updated_at → empty default"
    );
}

#[test]
fn read_task_lenient_top_level_unknown_status_becomes_custom() {
    // C0 (`07-26-taskstatus-custom-state`): unknown top-level
    // status is captured as Custom, NOT demoted to Planning.
    // This is the whole point of C0 — plugin-defined states
    // must survive a read/write round-trip.
    let d = fresh_project();
    write_raw_task_json(
        &proj(&d),
        "my-feat",
        r#"{"id":"t1","title":"T","slug":"my-feat","status":"blocked","created_at":"","updated_at":"","items":[]}"#,
    );
    let task = read_task(&proj(&d), "my-feat").expect("lenient parse");
    assert_eq!(
        task.status,
        TaskStatus::Custom("blocked".to_string()),
        "unknown top-level status → Custom (from_str_opt C0)"
    );
}

#[test]
fn read_task_lenient_item_status_in_progress_now_maps_to_in_progress() {
    // After the 2026-07-10 merge, "in_progress" is a valid
    // TaskStatus (the former Implement+Check collapsed into one).
    // Previously this was a checklist-style value that fell back
    // to Planning; now it parses correctly.
    let d = fresh_project();
    write_raw_task_json(
        &proj(&d),
        "my-feat",
        r#"{"id":"t1","title":"T","slug":"my-feat","status":"in_progress","created_at":"","updated_at":"","items":[{"id":"a","content":"do thing","status":"in_progress"}]}"#,
    );
    let task = read_task(&proj(&d), "my-feat").expect("lenient item status");
    assert_eq!(task.items.len(), 1);
    assert_eq!(
        task.items[0].status,
        TaskStatus::InProgress,
        "in_progress → InProgress (post-merge)"
    );
    assert_eq!(task.items[0].content, "do thing");
}

#[test]
fn read_task_lenient_legacy_implement_and_check_migrate_to_in_progress() {
    // Old task.json files with pre-merge "implement" / "check"
    // values silently migrate to InProgress (not demoted to
    // Planning) so dogfooded tasks don't lose their progress.
    let d = fresh_project();
    write_raw_task_json(
        &proj(&d),
        "my-feat",
        r#"{"id":"t1","title":"T","slug":"my-feat","status":"implement","created_at":"","updated_at":"","items":[]}"#,
    );
    let task = read_task(&proj(&d), "my-feat").expect("legacy implement");
    assert_eq!(task.status, TaskStatus::InProgress);

    write_raw_task_json(
        &proj(&d),
        "other",
        r#"{"id":"t2","title":"O","slug":"other","status":"check","created_at":"","updated_at":"","items":[]}"#,
    );
    let task = read_task(&proj(&d), "other").expect("legacy check");
    assert_eq!(task.status, TaskStatus::InProgress);
}

#[test]
fn read_task_lenient_item_status_pending_becomes_custom() {
    // C0 (`07-26-taskstatus-custom-state`): a checklist-style
    // item status like "pending" is captured as Custom rather
    // than demoted to Planning. Item-level status shares the
    // same TaskStatus enum + from_str_opt posture.
    let d = fresh_project();
    write_raw_task_json(
        &proj(&d),
        "my-feat",
        r#"{"id":"t1","title":"T","slug":"my-feat","status":"planning","created_at":"","updated_at":"","items":[{"id":"a","content":"x","status":"pending"}]}"#,
    );
    let task = read_task(&proj(&d), "my-feat").expect("lenient");
    assert_eq!(
        task.items[0].status,
        TaskStatus::Custom("pending".to_string()),
        "pending → Custom (C0)"
    );
}

/// 09-01-workflow-task-json-deadlock: reproduction of session
/// 2e438939's exact hand-written shape — `items[]` entries with
/// `{id, summary, tdd}` and NO `status`. The missing field used
/// to fail the WHOLE file's parse; `resolve_current_task` then
/// swallowed it every turn → "no active task" deadlock between
/// `request_task_state_transition` and `create_task`.
/// `TaskItem::status` now defaults to Planning on read.
#[test]
fn read_task_lenient_item_missing_status_defaults_planning() {
    let d = fresh_project();
    write_raw_task_json(
        &proj(&d),
        "09-01-a2-sandbox-docs-sync",
        r#"{"id":"t1","title":"T","slug":"09-01-a2-sandbox-docs-sync","status":"planning","created_at":"","updated_at":"","items":[{"id":"it-arch","summary":"ARCHITECTURE.md sync","tdd":false}]}"#,
    );
    let task = read_task(&proj(&d), "09-01-a2-sandbox-docs-sync")
        .expect("missing item status must not fail the file");
    assert_eq!(
        task.items[0].status,
        TaskStatus::Planning,
        "missing item status → Planning default"
    );
    assert_eq!(task.items[0].id, "it-arch");
}

#[test]
fn read_task_lenient_item_missing_content_defaults_empty() {
    let d = fresh_project();
    write_raw_task_json(
        &proj(&d),
        "my-feat",
        r#"{"id":"t1","title":"T","slug":"my-feat","status":"planning","created_at":"","updated_at":"","items":[{"id":"a","status":"done"}]}"#,
    );
    let task = read_task(&proj(&d), "my-feat").expect("missing content → empty default");
    assert_eq!(task.items[0].content, "", "missing content → empty string");
    assert_eq!(task.items[0].status, TaskStatus::Done);
}

#[test]
fn read_task_still_rejects_truly_malformed_json() {
    // Lenient status/defaults do NOT mean "accept any garbage" —
    // structurally broken JSON still fails so genuinely corrupt
    // files surface loudly (the `resolve_current_task` skip-on-
    // error contract depends on a real Err here).
    let d = fresh_project();
    write_raw_task_json(&proj(&d), "my-feat", "not json at all {");
    let err = read_task(&proj(&d), "my-feat").expect_err("garbage must still fail");
    assert!(matches!(err, TaskError::MalformedJson(..)), "got {:?}", err);
}

#[test]
fn write_task_is_atomic_via_tmp_rename() {
    // We can't directly observe the `tmp → final` rename from outside,
    // but we CAN confirm a partial-failure surrogate: write_task on a
    // read-only directory fails cleanly without corrupting the
    // original. We approximate that with an invalid slug — the
    // validate_slug preflight at the top of write_task short-circuits
    // before any IO.
    let d = fresh_project();
    create_task_init(&proj(&d), "Good", "good", None, TaskStatus::Planning, "dev").expect("first");
    let bad = TaskJson {
        id: "id".into(),
        title: "bad".into(),
        slug: "UPPER".into(),
        status: TaskStatus::Planning,
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
        parent: None,
        summary: String::new(),
        items: Vec::new(),
        // Step 3.3: pre-archive fixture.
        completed_at: None,
        workflow_plugin: "dev".into(),
    };
    let err = write_task(&proj(&d), &bad).expect_err("bad slug");
    assert!(matches!(err, TaskError::InvalidSlug(_)));
}

// --- schema / serde --------------------------------------------------

#[test]
fn task_json_serde_round_trip_preserves_all_fields() {
    let original = TaskJson {
        id: "id-x".into(),
        title: "T".into(),
        slug: "s".into(),
        status: TaskStatus::InProgress,
        created_at: "2026-07-08T00:00:00Z".into(),
        updated_at: "2026-07-08T01:00:00Z".into(),
        parent: Some("parent-slug".into()),
        summary: "one-line summary".into(),
        items: vec![
            TaskItem {
                id: "backend-impl".into(),
                content: "实现后端".into(),
                status: TaskStatus::InProgress,
                tdd: Some(true),
            },
            TaskItem {
                id: "frontend-impl".into(),
                content: "实现前端".into(),
                status: TaskStatus::Planning,
                tdd: None,
            },
        ],
        // Step 3.3: pre-archive serde-round-trip fixture.
        completed_at: None,
        workflow_plugin: "dev".into(),
    };
    let bytes = serde_json::to_vec_pretty(&original).unwrap();
    let parsed: TaskJson = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed, original);
}

/// C5 (2026-07-28): a pre-C5 task.json (no `workflow_plugin`
/// field) must deserialize with `workflow_plugin = "dev"` (the
/// serde default). Without this, upgrading would break every
/// existing task.json on disk — role gate / transition would
/// read an empty plugin and deny everything.
#[test]
fn task_json_pre_c5_missing_workflow_plugin_defaults_to_dev() {
    // A minimal pre-C5 task.json — no workflow_plugin key.
    let pre_c5 = r#"{
        "id": "old",
        "title": "Legacy",
        "slug": "legacy",
        "status": "in_progress",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    }"#;
    let parsed: TaskJson = serde_json::from_str(pre_c5).expect("pre-C5 task.json must parse");
    assert_eq!(
        parsed.workflow_plugin, "dev",
        "missing workflow_plugin must default to dev; got {:?}",
        parsed.workflow_plugin
    );
}

/// C5: a review-plugin task serializes `workflow_plugin: "review"`
/// (non-default → not skipped), while a dev task OMITS the field
/// (skip_serializing_if keeps dev task.json byte-identical to
/// pre-C5 for the common case).
#[test]
fn task_json_serializes_workflow_plugin_only_for_non_dev() {
    let review_task = TaskJson {
        id: "r".into(),
        title: "R".into(),
        slug: "r".into(),
        status: TaskStatus::Custom("intake".into()),
        created_at: "now".into(),
        updated_at: "now".into(),
        parent: None,
        summary: String::new(),
        items: Vec::new(),
        completed_at: None,
        workflow_plugin: "review".into(),
    };
    let json = serde_json::to_string(&review_task).unwrap();
    assert!(
        json.contains("\"workflow_plugin\":\"review\""),
        "review task must serialize workflow_plugin; got: {json}"
    );

    let dev_task = TaskJson {
        workflow_plugin: "dev".into(),
        ..review_task
    };
    let dev_json = serde_json::to_string(&dev_task).unwrap();
    assert!(
        !dev_json.contains("workflow_plugin"),
        "dev task must OMIT workflow_plugin (skip_serializing_if); got: {dev_json}"
    );
}

#[test]
fn task_json_omits_none_parent_when_serializing_to_skip_semantics() {
    let t = TaskJson {
        id: "id".into(),
        title: "t".into(),
        slug: "s".into(),
        status: TaskStatus::Planning,
        created_at: "now".into(),
        updated_at: "now".into(),
        parent: None,
        summary: String::new(),
        items: Vec::new(),
        // Step 3.3: must also be skipped via
        // `skip_serializing_if`.
        completed_at: None,
        workflow_plugin: "dev".into(),
    };
    let s = serde_json::to_string(&t).unwrap();
    assert!(!s.contains("parent"), "parent=None must be skipped: {}", s);
    assert!(
        !s.contains("completed_at"),
        "completed_at=None must be skipped: {}",
        s
    );
}

#[test]
fn task_status_parser_recognizes_known_forms_lenient_for_unknowns() {
    assert_eq!(TaskStatus::from_str_opt("planning"), TaskStatus::Planning);
    assert_eq!(
        TaskStatus::from_str_opt("in_progress"),
        TaskStatus::InProgress
    );
    assert_eq!(
        TaskStatus::from_str_opt("IN_PROGRESS"),
        TaskStatus::InProgress
    );
    // Legacy pre-merge values migrate to InProgress.
    assert_eq!(
        TaskStatus::from_str_opt("implement"),
        TaskStatus::InProgress
    );
    assert_eq!(TaskStatus::from_str_opt("CHECK"), TaskStatus::InProgress);
    assert_eq!(TaskStatus::from_str_opt("Done"), TaskStatus::Done);
    // Step 3.3: "completed" parses as Completed (NOT
    // demoted to Planning) so an archived task re-read
    // by the chat loop stays correctly classified.
    assert_eq!(TaskStatus::from_str_opt("completed"), TaskStatus::Completed);
    assert_eq!(TaskStatus::from_str_opt("COMPLETED"), TaskStatus::Completed);
    // C0 (`07-26-taskstatus-custom-state`): unknown values are
    // NO LONGER demoted to Planning — they flow through as
    // Custom so plugin-defined workflow states (review's
    // intake/reviewing/...) round-trip. Empty / whitespace
    // also become Custom (callers must validate non-emptiness
    // upstream; the tool layer does).
    assert_eq!(
        TaskStatus::from_str_opt(""),
        TaskStatus::Custom("".to_string())
    );
    assert_eq!(
        TaskStatus::from_str_opt("nope"),
        TaskStatus::Custom("nope".to_string())
    );
    assert_eq!(
        TaskStatus::from_str_opt("  PLAN  "),
        TaskStatus::Custom("plan".to_string()),
        "unknown value is trimmed + lowercased before capture"
    );
    assert_eq!(
        TaskStatus::from_str_opt("reviewing"),
        TaskStatus::Custom("reviewing".to_string()),
        "review plugin state round-trips as Custom"
    );
}

/// C0: `Custom(String)` round-trips through serde as a bare JSON
/// string (NOT the derived enum shape `{"Custom": "reviewing"}`),
/// so the on-disk `task.json.status` matches the plugin's
/// `workflow.json` state names and `roles_by_state` lookups
/// succeed. The manual `Serialize` impl + lenient `Deserialize`
/// (via `from_str_opt`) are symmetric.
#[test]
fn custom_status_round_trips_as_bare_string() {
    let t = TaskStatus::Custom("reviewing".to_string());
    let json = serde_json::to_string(&t).expect("serialize");
    assert_eq!(json, r#""reviewing""#, "bare string, not {{Custom:...}}");
    let parsed: TaskStatus = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, t);

    // Dev variants still serialize to their snake_case form
    // (manual impl delegates to as_str — single source of truth).
    assert_eq!(
        serde_json::to_string(&TaskStatus::Planning).unwrap(),
        r#""planning""#
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::InProgress).unwrap(),
        r#""in_progress""#
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::Done).unwrap(),
        r#""done""#
    );
    assert_eq!(
        serde_json::to_string(&TaskStatus::Completed).unwrap(),
        r#""completed""#
    );
}

/// C0: `Custom(s).as_str()` returns the captured string verbatim,
/// so `roles_by_state[status.as_str()]` lookups hit the plugin's
/// declared state key directly. The signature widened from
/// `&'static str` to `&str` (Custom borrows its inner String);
/// all call sites use the result transiently.
#[test]
fn custom_as_str_returns_captured_string() {
    assert_eq!(
        TaskStatus::Custom("reviewing".to_string()).as_str(),
        "reviewing"
    );
    assert_eq!(TaskStatus::Custom("intake".to_string()).as_str(), "intake");
    // Dev variants unchanged.
    assert_eq!(TaskStatus::Planning.as_str(), "planning");
    assert_eq!(TaskStatus::InProgress.as_str(), "in_progress");
    assert_eq!(TaskStatus::Done.as_str(), "done");
    assert_eq!(TaskStatus::Completed.as_str(), "completed");
}

// --- Step 3.3 — archive_task_init ------------------------------

/// Step 3.3: archiving a Done task moves the task dir
/// under `.everlasting/tasks/archive/<YYYY-MM>/<slug>/`
/// and flips `task.json` to `status = Completed` with
/// `completed_at` set.
#[test]
fn archive_task_init_moves_done_task_into_archive_tree() {
    let d = tempfile::tempdir().expect("tempdir");
    let path = d.path();
    let mut task = create_task_init(
        path,
        "My Feature",
        "my-feat",
        None,
        TaskStatus::Planning,
        "dev",
    )
    .expect("create");
    task.status = TaskStatus::Done;
    write_task(path, &task).expect("write done");

    let archived =
        archive_task_init(path, "my-feat", /* no_commit */ true).expect("archive should succeed");

    // Returned task reflects post-archive state.
    assert_eq!(archived.status, TaskStatus::Completed);
    assert!(
        archived.completed_at.is_some(),
        "completed_at must be set after archive"
    );

    // Live tree: no longer has the slug.
    let live_dir = task_dir(path, "my-feat");
    assert!(
        !live_dir.exists(),
        "live task dir must be gone after archive; got: {}",
        live_dir.display()
    );

    // Archive tree: has the moved task under <YYYY-MM>/<slug>/.
    let ym = archived
        .completed_at
        .as_deref()
        .unwrap()
        .split('T')
        .next()
        .expect("rfc3339 → YYYY-MM-DD")
        .get(..7)
        .expect("YYYY-MM slice")
        .to_string();
    let archived_dir = path
        .join(".everlasting")
        .join("tasks")
        .join(PROJ_NS_TASKS_ARCHIVE_DIR)
        .join(&ym)
        .join("my-feat");
    assert!(
        archived_dir.exists(),
        "archive dir must exist at: {}",
        archived_dir.display()
    );
    let archived_json = archived_dir.join("task.json");
    assert!(
        archived_json.exists(),
        "task.json must be at the archive dir"
    );
    let disk = read_task_at(&archived_json);
    assert_eq!(disk.status, TaskStatus::Completed);
    assert_eq!(disk.completed_at, archived.completed_at);
}

/// Step 3.3: archiving a non-Done task is refused.
/// Archiving a planning / in_progress task would
/// orphan in-flight work + the spec-distillation hint,
/// so the engine refuses upfront.
#[test]
fn archive_task_init_refuses_non_done_status() {
    for non_done in [TaskStatus::Planning, TaskStatus::InProgress] {
        let d = tempfile::tempdir().expect("tempdir");
        let path = d.path();
        let mut task = create_task_init(
            path,
            "My Feature",
            "my-feat",
            None,
            TaskStatus::Planning,
            "dev",
        )
        .expect("create");
        task.status = non_done.clone();
        write_task(path, &task).expect("write");

        let err = archive_task_init(path, "my-feat", true).expect_err("must refuse");
        assert!(
            matches!(err, TaskError::NotInDoneStatus(_)),
            "expected NotInDoneStatus for {non_done:?}, got: {err:?}"
        );
        // Live tree must still have the task.
        assert!(task_dir(path, "my-feat").exists());
    }
}

/// Step 3.3: re-archiving a task whose archive target
/// already exists is refused with `AlreadyArchived` —
/// no clobber on partial-write retries.
#[test]
fn archive_task_init_refuses_when_target_already_exists() {
    let d = tempfile::tempdir().expect("tempdir");
    let path = d.path();
    let mut task = create_task_init(
        path,
        "My Feature",
        "my-feat",
        None,
        TaskStatus::Planning,
        "dev",
    )
    .expect("create");
    task.status = TaskStatus::Done;
    write_task(path, &task).expect("write done");

    // First archive succeeds.
    archive_task_init(path, "my-feat", true).expect("first archive");

    // Simulate a stale second-attempt by recreating the
    // live task dir + Done status + a pre-existing
    // archive target (we manually mkdir the archive
    // dir this time to provoke the conflict path on
    // the *next* archive).
    // Recreate the live task first.
    let live_dir = task_dir(path, "my-feat");
    std::fs::create_dir_all(&live_dir).expect("recreate live");
    let mut task = task.clone();
    task.status = TaskStatus::Done;
    write_task(path, &task).expect("rewrite done");

    // Pre-create a stale archive target — this is the
    // scenario we want to defend against (the first
    // archive landed; a retry found the live dir again
    // somehow and is about to clobber).
    let ym = chrono::Utc::now().format("%Y-%m").to_string();
    let stale_target = path
        .join(".everlasting")
        .join("tasks")
        .join(PROJ_NS_TASKS_ARCHIVE_DIR)
        .join(&ym)
        .join("my-feat");
    std::fs::create_dir_all(&stale_target).expect("create stale target");
    std::fs::write(stale_target.join("sentinel"), "old").expect("write sentinel");

    let err = archive_task_init(path, "my-feat", true).expect_err("must refuse");
    assert!(
        matches!(err, TaskError::AlreadyArchived(_)),
        "expected AlreadyArchived, got: {err:?}"
    );
    // The stale sentinel must survive — no clobber.
    assert!(
        stale_target.join("sentinel").exists(),
        "stale archive target must not be clobbered"
    );
}

/// Step 3.3: archiving a non-existent slug returns
/// `NotFound` (not a generic IO error).
#[test]
fn archive_task_init_missing_returns_not_found() {
    let d = tempfile::tempdir().expect("tempdir");
    let err = archive_task_init(d.path(), "ghost", true).expect_err("must refuse");
    assert!(
        matches!(err, TaskError::NotFound(_)),
        "expected NotFound for ghost slug, got: {err:?}"
    );
}

/// Step 3.3: invalid slug is rejected before touching
/// the filesystem (defends against path-traversal
/// slugs the user might type into the IPC).
#[test]
fn archive_task_init_rejects_invalid_slug() {
    let d = tempfile::tempdir().expect("tempdir");
    for bad in ["BAD", "with space", "../escape", ""] {
        let err = archive_task_init(d.path(), bad, true).expect_err("must reject");
        assert!(
            matches!(err, TaskError::InvalidSlug(_)),
            "expected InvalidSlug for {bad:?}, got: {err:?}"
        );
    }
}

/// Helper: read + parse a `task.json` from an absolute
/// path (the public `read_task` resolves via project +
/// slug; the archive post-condition checks the moved
/// file directly).
fn read_task_at(json_path: &std::path::Path) -> TaskJson {
    let bytes = std::fs::read(json_path).expect("read json");
    serde_json::from_slice(&bytes).expect("parse json")
}
