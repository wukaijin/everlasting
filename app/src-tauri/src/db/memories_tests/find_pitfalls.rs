#![cfg(test)]

use super::make_pool;
use super::memories::{
    find_pitfalls_by_trigger, test_helpers::insert_raw, MemoryKind, MemoryScope, MemoryStatus,
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
    let rows = find_pitfalls_by_trigger(&pool, "shell", None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, "pit-shell");
    // Probe for `edit_file` → only pit-edit.
    let rows = find_pitfalls_by_trigger(&pool, "edit_file", None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].memory_id, "pit-edit");
    // Probe for an unknown tool → empty.
    let rows = find_pitfalls_by_trigger(&pool, "grep", None, None)
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
    let rows = find_pitfalls_by_trigger(&pool, "shell", None, None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "only path-agnostic fires without a path");
    assert_eq!(rows[0].memory_id, "pit-any");

    // Probe with a MATCHING path (single-segment after the prefix):
    // both fire.
    let rows = find_pitfalls_by_trigger(&pool, "shell", None, Some("app/src-tauri/Cargo.toml"))
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "both fire when path matches the glob");

    // Probe with a NON-MATCHING path: only path-agnostic fires.
    // `app/src-tauri/src/lib.rs` does NOT match `app/src-tauri/*`
    // (`session_tool_permissions`-style glob: `*` doesn't cross `/`).
    let rows = find_pitfalls_by_trigger(&pool, "shell", None, Some("app/src-tauri/src/lib.rs"))
        .await
        .unwrap();
    assert_eq!(rows.len(), 1, "deep path doesn't match single-segment glob");
    assert_eq!(rows[0].memory_id, "pit-any");

    // Probe with a totally unrelated path: only path-agnostic fires.
    let rows =
        find_pitfalls_by_trigger(&pool, "shell", None, Some("/home/user/some-other-dir/foo"))
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
    let rows = find_pitfalls_by_trigger(&pool, "shell", Some("cargo test --lib"), None)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);

    // Non-matching command — does NOT fire.
    let rows = find_pitfalls_by_trigger(&pool, "shell", Some("cargo build"), None)
        .await
        .unwrap();
    assert!(rows.is_empty(), "non-matching command_pattern filtered");
}
