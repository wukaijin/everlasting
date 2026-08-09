#![cfg(test)]

use super::memories::{validate_memory_text, MemoryInsertError};

// ---------------------------------------------------------------------------
// 07-06 (am-observability-panel) A2: `validate_memory_text` is the
// shared write safety net used by both `insert_memory` (P1/P2) and
// `update_memory` (R4). The regression test below locks in the
// behavior so a future change to either consumer doesn't drift the
// helper (which P4/P5 (auto_reflect) also depend on via
// `insert_memory`).
// ---------------------------------------------------------------------------

/// Sanity: the helper accepts a normal title + content and
/// generalizes `/home/<user>/` paths to `~/`.
#[test]
fn validate_memory_text_happy_path_generalizes_home() {
    let (title, content) = validate_memory_text(
        "WSL cargo tip",
        "in /home/alice/.cargo: set PKG_CONFIG_PATH",
        None,
        None,
    )
    .expect("happy path should succeed");
    assert_eq!(title, "WSL cargo tip");
    assert!(content.contains("~/"), "home must be generalized");
    assert!(
        !content.contains("/home/alice/"),
        "raw username must not leak"
    );
}

/// Empty title → `EmptyTitle`; empty content → `EmptyContent`.
#[test]
fn validate_memory_text_rejects_empty() {
    let err = validate_memory_text("   ", "x", None, None).unwrap_err();
    assert!(matches!(err, MemoryInsertError::EmptyTitle));
    let err = validate_memory_text("t", "", None, None).unwrap_err();
    assert!(matches!(err, MemoryInsertError::EmptyContent));
}

/// Over-length content (501 chars) → `ContentTooLong` (same DB
/// CHECK rejects; helper catches earlier for a clean error).
#[test]
fn validate_memory_text_rejects_oversize() {
    let long = "x".repeat(501);
    let err = validate_memory_text("t", &long, None, None).unwrap_err();
    assert!(matches!(err, MemoryInsertError::ContentTooLong(501)));
}

/// Sensitive-content regex (spike-005 §4): `api_key=...` shape →
/// `SensitiveContent`.
#[test]
fn validate_memory_text_rejects_sensitive() {
    let err = validate_memory_text("t", "api_key=AKIA...", None, None).unwrap_err();
    assert!(matches!(err, MemoryInsertError::SensitiveContent));
}

/// Sensitive-path deny-list (`.ssh`): a path with that component
/// → `SensitivePath`.
#[test]
fn validate_memory_text_rejects_sensitive_path() {
    let err = validate_memory_text("t", "check /home/alice/.ssh/id_rsa", None, None).unwrap_err();
    assert!(matches!(err, MemoryInsertError::SensitivePath(_)));
}
