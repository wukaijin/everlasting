#![cfg(test)]

use super::make_pool;
use super::memories::{
    insert_memory, MemoryInput, MemoryInsertError, MemoryKind, MemoryScope, MemoryStatus,
};

// ---------------------------------------------------------------------------
// P1 PR2: insert_memory + write safety net + list + delete
// ---------------------------------------------------------------------------

/// Helper: build a minimal valid `MemoryInput` with the caller's
/// overrides. Keeps the per-test boilerplate down.
fn input<'a>(
    scope: MemoryScope,
    kind: MemoryKind,
    title: &'a str,
    content: &'a str,
) -> MemoryInput {
    MemoryInput {
        scope,
        project_id: None,
        kind,
        status: MemoryStatus::Candidate,
        title: title.to_string(),
        content: content.to_string(),
        tags: "[]".to_string(),
        tool_name: None,
        command_pattern: None,
        path_globs: None,
        source_session_id: None,
        source_ref: None,
    }
}

/// Happy-path roundtrip: insert → read back → assert every column
/// landed correctly. The auto-generated `memory_id` is a UUID v7
/// (time-ordered, RFC 9562) — we assert it's a valid UUID shape.
#[tokio::test]
async fn insert_memory_happy_path_roundtrip() {
    let pool = make_pool().await;
    let mut inp = input(
        MemoryScope::Project,
        MemoryKind::Pitfall,
        "WSL cargo test fails",
        "Run with PKG_CONFIG_PATH set, or it can't find gdk-pixbuf",
    );
    inp.project_id = Some("proj-1".to_string());
    inp.status = MemoryStatus::Active;
    inp.tool_name = Some("shell".to_string());
    inp.command_pattern = Some("cargo test".to_string());
    inp.path_globs = Some(r#"["app/src-tauri/*"]"#.to_string());
    inp.tags = r#"["wsl","cargo"]"#.to_string();
    inp.source_session_id = Some("sess-1".to_string());
    inp.source_ref = Some("turn-3".to_string());

    let row = insert_memory(&pool, &inp).await.expect("insert ok");
    assert!(row.id > 0, "auto-id assigned");
    assert!(
        uuid::Uuid::parse_str(&row.memory_id).is_ok(),
        "memory_id is a UUID"
    );
    assert_eq!(row.scope, "project");
    assert_eq!(row.project_id.as_deref(), Some("proj-1"));
    assert_eq!(row.kind, "pitfall");
    assert_eq!(row.status, "active");
    assert_eq!(row.title, "WSL cargo test fails");
    assert!(row.content.starts_with("Run with PKG_CONFIG_PATH"));
    assert_eq!(row.tags, r#"["wsl","cargo"]"#);
    assert_eq!(row.tool_name.as_deref(), Some("shell"));
    assert_eq!(row.command_pattern.as_deref(), Some("cargo test"));
    assert_eq!(row.path_globs.as_deref(), Some(r#"["app/src-tauri/*"]"#));
    assert_eq!(row.source_session_id.as_deref(), Some("sess-1"));
    assert_eq!(row.source_ref.as_deref(), Some("turn-3"));
    // forward-compat defaults.
    assert_eq!(row.confidence, 0.5);
    assert_eq!(row.hit_count, 0);
    assert!(row.last_used_at.is_none());
    assert!(row.demoted_reason.is_none());
    assert!(!row.created_at.is_empty());
    assert_eq!(row.created_at, row.updated_at, "fresh row: equal ts");
}

/// scope=User with a project_id is rejected (H2: User scope is
/// global to the user, not project-bound).
#[tokio::test]
async fn insert_memory_user_scope_with_project_id_is_rejected() {
    let pool = make_pool().await;
    let mut inp = input(MemoryScope::User, MemoryKind::Fact, "t", "c");
    inp.project_id = Some("proj-x".to_string());
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::UserScopeHasProjectId(_)));
}

/// scope=Project without a project_id is rejected (H2).
#[tokio::test]
async fn insert_memory_project_scope_without_id_is_rejected() {
    let pool = make_pool().await;
    let inp = input(MemoryScope::Project, MemoryKind::Fact, "t", "c");
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::ProjectScopeMissingId));
}

/// Empty title / empty content are rejected (2.2 / B1).
#[tokio::test]
async fn insert_memory_rejects_empty_title_and_content() {
    let pool = make_pool().await;
    // Empty title.
    let inp = input(MemoryScope::User, MemoryKind::Fact, "   ", "content");
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::EmptyTitle), "empty title");
    // Empty content.
    let inp = input(MemoryScope::User, MemoryKind::Fact, "title", "");
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(
        matches!(err, MemoryInsertError::EmptyContent),
        "empty content"
    );
}

/// Over-length title / content are rejected by the safety net
/// BEFORE hitting the DB (the error message is actionable; the
/// DB CHECK is the backstop).
#[tokio::test]
async fn insert_memory_rejects_oversize_title_and_content() {
    let pool = make_pool().await;
    let long_title: String = "t".repeat(201);
    let inp = input(MemoryScope::User, MemoryKind::Fact, &long_title, "c");
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::TitleTooLong(201)));
    let long_content: String = "c".repeat(501);
    let inp = input(MemoryScope::User, MemoryKind::Fact, "t", &long_content);
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::ContentTooLong(501)));
}

/// Sensitive-content regex (api_key/secret/password/token/bearer)
/// is rejected in BOTH title and content.
#[tokio::test]
async fn insert_memory_rejects_sensitive_content() {
    let pool = make_pool().await;
    // api_key in title.
    let inp = input(
        MemoryScope::User,
        MemoryKind::Fact,
        "the ANTHROPIC_API_KEY is sk-...",
        "regular content here",
    );
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(
        matches!(err, MemoryInsertError::SensitiveContent),
        "api_key in title"
    );
    // password in content.
    let inp = input(
        MemoryScope::User,
        MemoryKind::Fact,
        "regular title",
        "the database password is hunter2",
    );
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(
        matches!(err, MemoryInsertError::SensitiveContent),
        "password in content"
    );
    // bearer token in content.
    let inp = input(
        MemoryScope::User,
        MemoryKind::Fact,
        "regular title",
        "Authorization: bearer xyz",
    );
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(
        matches!(err, MemoryInsertError::SensitiveContent),
        "bearer in content"
    );
    // token= (query-param form) in content.
    let inp = input(
        MemoryScope::User,
        MemoryKind::Fact,
        "regular title",
        "url with token=abc123",
    );
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(
        matches!(err, MemoryInsertError::SensitiveContent),
        "token= in content"
    );
}

/// Sensitive-path components (.ssh / .aws / .gnupg / credentials /
/// id_rsa) are rejected in any path-like field.
#[tokio::test]
async fn insert_memory_rejects_sensitive_path_components() {
    let pool = make_pool().await;
    // .ssh in content.
    let inp = input(
        MemoryScope::User,
        MemoryKind::Pitfall,
        "key location",
        "the key is in /home/user/.ssh/id_ed25519",
    );
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(
        matches!(err, MemoryInsertError::SensitivePath(_)),
        ".ssh denied"
    );
    // .aws in path_globs (the JSON string carries the component).
    let mut inp = input(
        MemoryScope::User,
        MemoryKind::Pitfall,
        "aws creds",
        "be careful with the credentials file",
    );
    inp.path_globs = Some(r#"["/home/user/.aws/*"]"#.to_string());
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(
        matches!(err, MemoryInsertError::SensitivePath(_)),
        ".aws in path_globs denied"
    );
}

/// Temporary-path prefixes (/tmp/ / /var/log/) are rejected —
/// they're ephemeral and a memory referencing them is useless.
#[tokio::test]
async fn insert_memory_rejects_temporary_paths() {
    let pool = make_pool().await;
    let inp = input(
        MemoryScope::User,
        MemoryKind::Pitfall,
        "temp file",
        "the build output is in /tmp/build.log",
    );
    let err = insert_memory(&pool, &inp).await.unwrap_err();
    assert!(matches!(err, MemoryInsertError::TemporaryPath(_)));
}

/// `/home/<user>/` paths in content are generalized to `~/` so the
/// stored memory is username-agnostic (the spike-005 §4 leak rule).
#[tokio::test]
async fn insert_memory_generalizes_home_path() {
    let pool = make_pool().await;
    let inp = input(
        MemoryScope::User,
        MemoryKind::Fact,
        "the project lives at /home/alice/code/everlasting",
        "the config is at /home/alice/.config/everlasting/",
    );
    let row = insert_memory(&pool, &inp).await.expect("insert ok");
    assert!(
        !row.title.contains("/home/alice"),
        "title generalized: {}",
        row.title
    );
    assert!(
        row.title.contains("~/code/everlasting"),
        "title has ~/ prefix"
    );
    assert!(
        !row.content.contains("/home/alice"),
        "content generalized: {}",
        row.content
    );
    assert!(
        row.content.contains("~/.config/everlasting/"),
        "content has ~/.config prefix"
    );
}
