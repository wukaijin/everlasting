#![cfg(test)]

use super::make_pool;
use super::memories::{
    find_pitfalls_by_trigger, find_pitfalls_by_trigger_all_status, test_helpers::insert_raw,
    MemoryKind, MemoryScope, MemoryStatus,
};

/// `find_pitfalls_by_trigger` matches pitfall memories by `tool_name`
/// exact equality (indexed by `idx_am_pitfall`). Other tool_names
/// and non-pitfall kinds are NOT matched.
#[tokio::test]
async fn find_pitfalls_by_trigger_tool_name_exact_match() {
    let pool = make_pool().await;
    // Pitfall for `shell` tool, path-agnostic (path_globs=NULL).
    insert_raw(
        &pool,
        "pit-shell",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "cargo test needs PKG_CONFIG_PATH",
        "run with PKG_CONFIG_PATH set",
    )
    .await
    .unwrap();
    // Set the trigger key columns via a direct UPDATE (insert_raw
    // doesn't set them; production uses insert_memory).
    sqlx::query("UPDATE autonomous_memories SET tool_name='shell' WHERE memory_id='pit-shell'")
        .execute(&pool)
        .await
        .unwrap();
    // A non-pitfall (preference) with the same tool_name — excluded.
    insert_raw(
        &pool,
        "pref-shell",
        MemoryScope::User,
        None,
        MemoryKind::Preference,
        MemoryStatus::Active,
        "user prefers shell",
        "always use shell tool",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE autonomous_memories SET tool_name='shell' WHERE memory_id='pref-shell'")
        .execute(&pool)
        .await
        .unwrap();
    // A pitfall for a DIFFERENT tool — excluded.
    insert_raw(
        &pool,
        "pit-edit",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "edit_file pitfall",
        "be careful with edit_file",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE autonomous_memories SET tool_name='edit_file' WHERE memory_id='pit-edit'")
        .execute(&pool)
        .await
        .unwrap();

    // Probe for `shell` → only pit-shell (preference excluded despite same tool_name).
    let rows = find_pitfalls_by_trigger(&pool, "proj-test", "shell", None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, "pit-shell");
    // Probe for `edit_file` → only pit-edit.
    let rows = find_pitfalls_by_trigger(&pool, "proj-test", "edit_file", None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, "pit-edit");
    // Probe for an unknown tool → empty.
    let rows = find_pitfalls_by_trigger(&pool, "proj-test", "grep", None, None)
        .await
        .unwrap();
    assert!(rows.is_empty());
}

/// `path_globs=NULL` means the pitfall is path-agnostic (fires for
/// ANY path). `path_globs=Some(globs)` requires the caller-supplied
/// path to match at least one glob (M2).
#[tokio::test]
async fn find_pitfalls_path_globs_semantics() {
    let pool = make_pool().await;
    // Path-agnostic pitfall (path_globs=NULL) — fires for any path.
    insert_raw(
        &pool,
        "pit-any",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "any path",
        "fires for any path",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE autonomous_memories SET tool_name='shell' WHERE memory_id='pit-any'")
        .execute(&pool)
        .await
        .unwrap();
    // Path-bound pitfall (path_globs=["app/src-tauri/*"]).
    // `session_tool_permissions`-style glob: `*` does NOT cross `/`
    // (NOT native SQLite GLOB — native SQLite `'a/b' GLOB 'a*'` would
    // match; see the `glob_matches_path` doc comment for the
    // empirical verification). So `app/src-tauri/*` matches
    // `app/src-tauri/<single-segment>` but NOT
    // `app/src-tauri/src/lib.rs` (the `*` would have to cross the
    // `/` between `src` and `lib.rs`). spike-007 re-grill explicitly
    // standardized on no `**` recursion.
    insert_raw(
        &pool,
        "pit-bound",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "bound path",
        "only fires for app/src-tauri",
    )
    .await
    .unwrap();
    sqlx::query(
        r#"UPDATE autonomous_memories
           SET tool_name='shell', path_globs='["app/src-tauri/*"]'
           WHERE memory_id='pit-bound'"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    // Probe with NO path: path-agnostic fires; path-bound does NOT
    // (can't confirm the glob match; precision-first).
    let rows = find_pitfalls_by_trigger(&pool, "proj-test", "shell", None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "only path-agnostic fires without a path");
    assert_eq!(rows[0].memory_id, "pit-any");

    // Probe with a MATCHING path (single-segment after the prefix):
    // both fire.
    let rows = find_pitfalls_by_trigger(
        &pool,
        "proj-test",
        "shell",
        None,
        Some("app/src-tauri/Cargo.toml"),
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 2, "both fire when path matches the glob");

    // Probe with a NON-MATCHING path: only path-agnostic fires.
    // `app/src-tauri/src/lib.rs` does NOT match `app/src-tauri/*`
    // (`session_tool_permissions`-style glob: `*` doesn't cross `/`).
    let rows = find_pitfalls_by_trigger(
        &pool,
        "proj-test",
        "shell",
        None,
        Some("app/src-tauri/src/lib.rs"),
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "deep path doesn't match single-segment glob");
    assert_eq!(rows[0].memory_id, "pit-any");

    // Probe with a totally unrelated path: only path-agnostic fires.
    let rows = find_pitfalls_by_trigger(
        &pool,
        "proj-test",
        "shell",
        None,
        Some("/home/user/some-other-dir/foo"),
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "unrelated path → only path-agnostic");
    assert_eq!(rows[0].memory_id, "pit-any");
}

/// `command_pattern` is a secondary substring filter — a pitfall
/// fires only if the caller-supplied command contains the stored
/// pattern.
#[tokio::test]
async fn find_pitfalls_command_pattern_substring_filter() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "pit-cp",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "cargo test pattern",
        "the command pattern is cargo test",
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE autonomous_memories SET tool_name='shell', command_pattern='cargo test' \
         WHERE memory_id='pit-cp'",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Matching command — fires.
    let rows =
        find_pitfalls_by_trigger(&pool, "proj-test", "shell", Some("cargo test --lib"), None)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);

    // Non-matching command — does NOT fire.
    let rows = find_pitfalls_by_trigger(&pool, "proj-test", "shell", Some("cargo build"), None)
        .await
        .unwrap();
    assert!(rows.is_empty(), "non-matching command_pattern filtered");
}

/// Precision-first (2026-08-18, task 08-18-debug-session-5df29977
/// 问题1): a row whose `command_pattern` is SET is skipped when the
/// probe supplies NO command — the constraint can't be confirmed.
/// This is the 5df29977 incident regression: Path-kind tools
/// (edit_file) never extract a command probe, so two edit_file
/// pitfalls carrying error-text patterns (`Missing required
/// parameter: path`) fired on EVERY healthy edit_file call
/// (hit_count 60/15). The SQL pre-fix fell through the
/// `if let Some(cp) = command_pattern` guard entirely.
#[tokio::test]
async fn find_pitfalls_command_pattern_row_without_probe_command_is_skipped() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "pit-cp-no-probe",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "needs the command to confirm",
        "must not fire when the probe has no command",
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE autonomous_memories SET tool_name='edit_file', \
         command_pattern='Missing required parameter: path' \
         WHERE memory_id='pit-cp-no-probe'",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Probe with NO command (the edit_file / Path-kind probe shape):
    // the constrained row is skipped — both the P3-era filter and
    // the P5 all-status variant share the semantics.
    let rows = find_pitfalls_by_trigger(&pool, "proj-test", "edit_file", None, None)
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "command_pattern-constrained row must not fire without a probe command"
    );
    let rows = find_pitfalls_by_trigger_all_status(&pool, "proj-test", "edit_file", None, None)
        .await
        .unwrap();
    assert!(
        rows.is_empty(),
        "all_status variant shares the precision-first skip"
    );

    // Control: the same row DOES fire when the probe supplies a
    // matching command (e.g. a shell-kind probe containing the
    // stored substring).
    let rows = find_pitfalls_by_trigger(
        &pool,
        "proj-test",
        "edit_file",
        Some("error: Missing required parameter: path"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1, "matching probe command still recalls");
}

/// Scope/project isolation for the trigger-recall path (H2, added
/// 2026-09-02): a `scope=project` pitfall in proj-a must NOT fire
/// when probing from proj-b; user-scope pitfalls stay global. This
/// is the recall-side twin of `list_memories_project_isolation` —
/// before the fix, `find_pitfalls_by_trigger(_all_status)` had NO
/// scope filter at all, so a project pitfall (e.g. P4 reflection
/// output) footnoted / soft-blocked / fed hit_count promotion in
/// every project.
#[tokio::test]
async fn find_pitfalls_project_isolation() {
    let pool = make_pool().await;
    // proj-a project-scope pitfall (the P4 reflection write shape:
    // scope=Project + project_id bound).
    insert_raw(
        &pool,
        "pit-proj-a",
        MemoryScope::Project,
        Some("proj-a"),
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "proj-a only pitfall",
        "must not surface in proj-b sessions",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE autonomous_memories SET tool_name='shell' WHERE memory_id='pit-proj-a'")
        .execute(&pool)
        .await
        .unwrap();
    // User-scope pitfall — global by design.
    insert_raw(
        &pool,
        "pit-global",
        MemoryScope::User,
        None,
        MemoryKind::Pitfall,
        MemoryStatus::Active,
        "global pitfall",
        "surfaces in every project",
    )
    .await
    .unwrap();
    sqlx::query("UPDATE autonomous_memories SET tool_name='shell' WHERE memory_id='pit-global'")
        .execute(&pool)
        .await
        .unwrap();

    // Probe as proj-b → only the user row; proj-a's row is isolated.
    let rows = find_pitfalls_by_trigger(&pool, "proj-b", "shell", None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "proj-a pitfall isolated from proj-b");
    assert_eq!(rows[0].memory_id, "pit-global");

    // Probe as proj-a → both rows.
    let rows = find_pitfalls_by_trigger(&pool, "proj-a", "shell", None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);

    // The all_status variant (P5 production path) shares the filter.
    let rows = find_pitfalls_by_trigger_all_status(&pool, "proj-b", "shell", None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "all_status probe isolates too");
    assert_eq!(rows[0].memory_id, "pit-global");
}
