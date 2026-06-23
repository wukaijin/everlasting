#![cfg(test)]

//! Projects-domain integration tests (split from `db/tests.rs` on 2026-06-23).
//!
//! Coverage:
//! - Migrations idempotency + auto-default project seed
//! - Project CRUD (create / list / list_hidden / get / update_path /
//!   update_name / hide / unhide / list_stale_git_probe / git_metadata)

use sqlx::SqlitePool;

use crate::projects::DEFAULT_PROJECT_ID;

use super::{
 migrations::run_migrations,
 projects::{
 create_project, get_project, hide_project, list_hidden_projects, list_projects,
 list_projects_with_stale_git_probe, unhide_project, update_project_git_metadata,
 update_project_name, update_project_path,
 },
};

async fn test_pool() -> SqlitePool {
 let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
 // Mirror what `init_pool` does.
 sqlx::query("PRAGMA foreign_keys = ON")
 .execute(&pool)
 .await
 .unwrap();
 run_migrations(&pool).await.unwrap();
 pool
}
#[tokio::test]
async fn migrations_are_idempotent() {
 let pool = test_pool().await;
 // Running twice should not error.
 run_migrations(&pool).await.unwrap();
}

#[tokio::test]
async fn default_project_is_seeded() {
 let pool = test_pool().await;
 let projects = list_projects(&pool, true).await.unwrap();
 let backstop = projects
 .iter()
 .find(|p| p.id == DEFAULT_PROJECT_ID)
 .expect("default project should be seeded");
 assert!(backstop.is_legacy);
 assert_eq!(backstop.name, "Legacy / 未分类");
 assert!(!backstop.hidden);
}

#[tokio::test]
async fn create_and_list_project() {
 let pool = test_pool().await;
 let dir = std::env::temp_dir().join("everlasting_test_create_proj");
 let _ = std::fs::create_dir_all(&dir);
 let path_str = dir.to_str().unwrap();

 let p1 = create_project(&pool, "a", path_str, false, None).await.unwrap();
 // Duplicate path → unique violation → Err.
 let dup = create_project(&pool, "b", path_str, false, None).await;
 assert!(dup.is_err(), "duplicate path should fail");

 let p2 = create_project(&pool, "c", "/tmp/everlasting_test_other", true, None)
 .await
 .unwrap();
 let list = list_projects(&pool, false).await.unwrap();
 let ids: Vec<&str> = list.iter().map(|p| p.id.as_str()).collect();
 assert!(ids.contains(&p1.id.as_str()));
 assert!(ids.contains(&p2.id.as_str()));
 assert!(ids.contains(&DEFAULT_PROJECT_ID));

 let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn hide_and_unhide_project() {
 let pool = test_pool().await;
 let p = create_project(&pool, "x", "/tmp/everlasting_test_hide", false, None)
 .await
 .unwrap();

 hide_project(&pool, &p.id).await.unwrap();
 let visible = list_projects(&pool, false).await.unwrap();
 assert!(!visible.iter().any(|q| q.id == p.id));
 let hidden = list_hidden_projects(&pool).await.unwrap();
 assert!(hidden.iter().any(|q| q.id == p.id));

 unhide_project(&pool, &p.id).await.unwrap();
 let visible = list_projects(&pool, false).await.unwrap();
 assert!(visible.iter().any(|q| q.id == p.id));
}

#[tokio::test]
async fn update_project_name_works() {
 let pool = test_pool().await;
 let p = create_project(&pool, "old", "/tmp/everlasting_test_rename", false, None)
 .await
 .unwrap();
 let p2 = update_project_name(&pool, &p.id, "new").await.unwrap();
 assert_eq!(p2.name, "new");
 // updated_at should have advanced.
 assert_ne!(p2.updated_at, p.updated_at);
}

#[tokio::test]
async fn update_project_path_reprobes_git_flag() {
 let pool = test_pool().await;
 let p = create_project(&pool, "p", "/tmp/everlasting_test_repath", false, None)
 .await
 .unwrap();
 assert!(!p.is_git_repo);
 assert!(p.git_branch.is_none());

 let p2 =
 update_project_path(&pool, &p.id, "/tmp/everlasting_test_repath2", true, None)
 .await
 .unwrap();
 assert!(p2.is_git_repo);
 assert_eq!(p2.path, "/tmp/everlasting_test_repath2");
}

#[tokio::test]
async fn list_projects_with_stale_git_probe_filters_correctly() {
 // Pre-PR2 row: should appear.
 let pool = test_pool().await;
 let p_stale =
 create_project(&pool, "stale", "/tmp/everlasting_test_stale", false, None)
 .await
 .unwrap();
 assert!(!p_stale.is_git_repo);

 // Already-probed row: should NOT appear.
 let p_fresh = create_project(
 &pool,
 "fresh",
 "/tmp/everlasting_test_fresh",
 true,
 Some("main".to_string()),
 )
 .await
 .unwrap();
 assert!(p_fresh.is_git_repo);

 // Hidden stale row: should NOT appear (we skip hidden
 // projects — see the function's docstring).
 let p_hidden =
 create_project(&pool, "hidden", "/tmp/everlasting_test_hidden", false, None)
 .await
 .unwrap();
 hide_project(&pool, &p_hidden.id).await.unwrap();

 let stale = list_projects_with_stale_git_probe(&pool).await.unwrap();
 let ids: Vec<&str> = stale.iter().map(|p| p.id.as_str()).collect();
 assert!(ids.contains(&p_stale.id.as_str()));
 assert!(!ids.contains(&p_fresh.id.as_str()));
 assert!(!ids.contains(&p_hidden.id.as_str()));
}

#[tokio::test]
async fn update_project_git_metadata_round_trip() {
 let pool = test_pool().await;
 // Start from a non-git row, write git metadata, reload, verify.
 let p = create_project(&pool, "p", "/tmp/everlasting_test_metaupd", false, None)
 .await
 .unwrap();
 assert!(!p.is_git_repo);

 update_project_git_metadata(&pool, &p.id, true, Some("feature/x"))
 .await
 .unwrap();
 let reloaded = get_project(&pool, &p.id).await.unwrap().unwrap();
 assert!(reloaded.is_git_repo);
 assert_eq!(reloaded.git_branch.as_deref(), Some("feature/x"));

 // Setting `git_branch = None` (e.g. for a non-git repo) is
 // distinct from "empty string".
 update_project_git_metadata(&pool, &p.id, false, None).await.unwrap();
 let reloaded = get_project(&pool, &p.id).await.unwrap().unwrap();
 assert!(!reloaded.is_git_repo);
 assert!(reloaded.git_branch.is_none());
}

#[tokio::test]
async fn create_project_persists_git_branch() {
 let pool = test_pool().await;
 // Branch string survives a round-trip through the DB; the
 // detached-HEAD literal "HEAD" is also accepted.
 let p = create_project(
 &pool,
 "branchy",
 "/tmp/everlasting_test_branch",
 true,
 Some("feature/pr2".to_string()),
 )
 .await
 .unwrap();
 assert_eq!(p.git_branch.as_deref(), Some("feature/pr2"));

 let reloaded = get_project(&pool, &p.id).await.unwrap().unwrap();
 assert_eq!(reloaded.git_branch.as_deref(), Some("feature/pr2"));

 let detached = create_project(
 &pool,
 "detached",
 "/tmp/everlasting_test_detached",
 true,
 Some("HEAD".to_string()),
 )
 .await
 .unwrap();
 assert_eq!(detached.git_branch.as_deref(), Some("HEAD"));
}

#[tokio::test]
async fn update_project_path_reprobes_git_branch() {
 let pool = test_pool().await;
 let p = create_project(
 &pool,
 "rebranch",
 "/tmp/everlasting_test_rebranch",
 true,
 Some("main".to_string()),
 )
 .await
 .unwrap();
 assert_eq!(p.git_branch.as_deref(), Some("main"));

 // Re-probe with a different branch.
 let p2 = update_project_path(
 &pool,
 &p.id,
 "/tmp/everlasting_test_rebranch2",
 true,
 Some("develop".to_string()),
 )
 .await
 .unwrap();
 assert_eq!(p2.git_branch.as_deref(), Some("develop"));

 // Re-probe to a non-git path → branch cleared.
 let p3 = update_project_path(
 &pool,
 &p.id,
 "/tmp/everlasting_test_rebranch3",
 false,
 None,
 )
 .await
 .unwrap();
 assert!(!p3.is_git_repo);
 assert!(p3.git_branch.is_none());
}

