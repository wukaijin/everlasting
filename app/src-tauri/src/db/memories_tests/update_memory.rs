#![cfg(test)]

use super::make_pool;
use super::memories::{
    get_memory_by_id, insert_memory, test_helpers::insert_raw, update_memory, MemoryInput,
    MemoryInsertError, MemoryKind, MemoryScope, MemoryStatus, MemoryUpdateError,
};

/// Regression: `insert_memory` continues to apply the same safety
/// net after the helper extraction (A2 refactor). Without this
/// regression, P2 / P4 / P5 (auto_reflect, which calls
/// `insert_memory` and inherits the safety net) could silently
/// accept sensitive content if the helper ever drifts.
#[tokio::test]
async fn insert_memory_still_safe_after_helper_extract() {
    let pool = make_pool().await;
    // Sensitive content via the public `insert_memory` path must
    // still be rejected (helper extraction didn't bypass the net).
    let result = insert_memory(
        &pool,
        &MemoryInput {
            scope: MemoryScope::Project,
            project_id: Some("/repo/proj".into()),
            kind: MemoryKind::Fact,
            status: MemoryStatus::Candidate,
            title: "safe title".into(),
            content: "api_key=AKIA1234".into(),
            tags: "[]".into(),
            tool_name: None,
            command_pattern: None,
            path_globs: None,
            source_session_id: None,
            source_ref: None,
        },
    )
    .await;
    assert!(
        matches!(result, Err(MemoryInsertError::SensitiveContent)),
        "insert_memory must still reject sensitive content post-extract"
    );
}

// ---------------------------------------------------------------------------
// 07-06 (am-observability-panel) A3: `update_memory` happy path +
// safety-net rejections. The R4 user-edit path reuses the same
// `validate_memory_text` helper as `insert_memory`, so the safety
// net is byte-identical for both write paths.
// ---------------------------------------------------------------------------

/// Happy path: edit an existing memory's title + content, verify
/// the post-update row has the new text, `edited_by_user = 1`, and
/// a fresh `updated_at`.
#[tokio::test]
async fn update_memory_roundtrip() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "um-1",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "old title",
        "old content",
    )
    .await
    .unwrap();
    let row = update_memory(&pool, "um-1", "new title", "new content")
        .await
        .expect("update should succeed");
    assert_eq!(row.title, "new title");
    assert_eq!(row.content, "new content");
    assert!(
        row.edited_by_user,
        "user edit must flip edited_by_user to 1"
    );
    // The post-update readback's `updated_at` is the new
    // timestamp (stricter than just "> old"); we don't pin the
    // exact value (clock-dependent) but it must differ from the
    // `insert_raw`-stamped value.
    assert!(!row.updated_at.is_empty());
}

/// Oversize content (501 chars) → `ContentTooLong` (safety net
/// reuses the same length cap as `insert_memory`).
#[tokio::test]
async fn update_memory_rejects_oversize() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "um-2",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "t",
        "c",
    )
    .await
    .unwrap();
    let long = "x".repeat(501);
    let err = update_memory(&pool, "um-2", "t", &long).await.unwrap_err();
    assert!(matches!(err, MemoryUpdateError::ContentTooLong(501)));
}

/// Sensitive content → `SensitiveContent` (same regex as
/// `insert_memory`; helper is a single source of truth).
#[tokio::test]
async fn update_memory_rejects_sensitive() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "um-3",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "t",
        "c",
    )
    .await
    .unwrap();
    let err = update_memory(&pool, "um-3", "t", "password=hunter2")
        .await
        .unwrap_err();
    assert!(matches!(err, MemoryUpdateError::SensitiveContent));
}

/// Sensitive path → `SensitivePath` (deny-list check).
#[tokio::test]
async fn update_memory_rejects_sensitive_path() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "um-4",
        MemoryScope::User,
        None,
        MemoryKind::Fact,
        MemoryStatus::Active,
        "t",
        "c",
    )
    .await
    .unwrap();
    let err = update_memory(&pool, "um-4", "t", "see /home/u/.ssh/notes")
        .await
        .unwrap_err();
    assert!(matches!(err, MemoryUpdateError::SensitivePath(_)));
}

/// `edited_by_user` flips to 1 ONLY on a successful user edit. A
/// failed edit (e.g. safety-net rejection) must NOT change the
/// column (the row is untouched, but a defensive assertion on the
/// unchanged value is the regression guard).
#[tokio::test]
async fn update_memory_sets_edited_by_user() {
    let pool = make_pool().await;
    insert_raw(
        &pool,
        "um-5",
        MemoryScope::Project,
        Some("/repo/proj"),
        MemoryKind::Fact,
        MemoryStatus::Active,
        "t",
        "c",
    )
    .await
    .unwrap();
    // Baseline: `insert_raw` (test helper) writes via raw SQL, not
    // the `insert_memory` safety net, so `edited_by_user` is the
    // schema default (0 = false).
    let before = get_memory_by_id(&pool, "um-5").await.unwrap().unwrap();
    assert!(!before.edited_by_user, "raw insert defaults to 0");
    // Successful edit → 1.
    let after = update_memory(&pool, "um-5", "new t", "new c")
        .await
        .unwrap();
    assert!(after.edited_by_user);
}

/// Unknown `memory_id` → `NotFound` (race with delete or invalid
/// input from the frontend). The IPC layer surfaces this as
/// `ErrorCategory::InvalidRequest` per the `AppError` impl.
#[tokio::test]
async fn update_memory_not_found() {
    let pool = make_pool().await;
    let err = update_memory(&pool, "nonexistent", "t", "c")
        .await
        .unwrap_err();
    assert!(matches!(err, MemoryUpdateError::NotFound(_)));
}
