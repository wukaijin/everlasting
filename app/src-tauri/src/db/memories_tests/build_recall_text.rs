#![cfg(test)]

use super::make_pool;
use super::memories::{insert_memory, MemoryInput};

// ---------------------------------------------------------------------------
// 07-06 (am-observability-panel) A8: `build_recall_text_with_rows`
// returns the row details alongside the formatted text. The
// original `build_recall_text` keeps its signature (wrapper) and
// the original 4 P2 unit tests must continue to pass — covered by
// the existing `build_recall_text_*` tests above. The new
// `build_recall_text_with_rows_*` tests below lock the new shape.
// ---------------------------------------------------------------------------

/// `build_recall_text_with_rows` returns the rows on a hit.
#[tokio::test]
async fn build_recall_text_with_rows_returns_rows() {
    use crate::agent::memory_recall::build_recall_text_with_rows;
    use crate::db::memories::{MemoryKind, MemoryScope, MemoryStatus};

    let pool = make_pool().await;
    insert_memory(
        &pool,
        &MemoryInput {
            scope: MemoryScope::Project,
            project_id: Some("/repo/proj".into()),
            kind: MemoryKind::Fact,
            status: MemoryStatus::Candidate,
            title: "WSL cargo".into(),
            content: "set PKG_CONFIG_PATH for cargo in wsl".into(),
            tags: "[]".into(),
            tool_name: None,
            command_pattern: None,
            path_globs: None,
            source_session_id: Some("sess-test".into()),
            source_ref: None,
        },
    )
    .await
    .unwrap();
    let result = build_recall_text_with_rows(&pool, "/repo/proj", "cargo build in wsl").await;
    let (text, rows) = result.expect("hit expected");
    // The 4 P2 contract asserts continue to hold on `text`.
    assert!(text.contains("<relevant-memories>"));
    assert!(text.contains("WSL cargo"));
    assert!(text.contains("PKG_CONFIG_PATH"));
    assert!(text.contains("[fact]"));
    // The new shape: `rows` carries the hit row.
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "WSL cargo");
    assert_eq!(rows[0].kind, "fact");
}

/// `build_recall_text_with_rows` returns `None` on an empty query
/// (mirrors `build_recall_text`).
#[tokio::test]
async fn build_recall_text_with_rows_empty_query() {
    use crate::agent::memory_recall::build_recall_text_with_rows;
    let pool = make_pool().await;
    assert!(build_recall_text_with_rows(&pool, "/repo/proj", "")
        .await
        .is_none());
    assert!(build_recall_text_with_rows(&pool, "/repo/proj", "   ")
        .await
        .is_none());
}

/// `build_recall_text_with_rows` returns `None` when the FTS query
/// yields no matches (no DB error, no rows).
#[tokio::test]
async fn build_recall_text_with_rows_no_match() {
    use crate::agent::memory_recall::build_recall_text_with_rows;
    let pool = make_pool().await;
    // No rows inserted; query is non-empty.
    assert!(
        build_recall_text_with_rows(&pool, "/repo/proj", "completely unrelated xyz")
            .await
            .is_none()
    );
}
