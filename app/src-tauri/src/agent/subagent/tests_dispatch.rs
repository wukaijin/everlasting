//! Subagent dispatch 单元测试(拆分自 dispatch.rs, 08-07-large-file-splitting)。

#![cfg(test)]

use std::sync::Arc;

use sqlx::SqlitePool;

use crate::agent::subagent::SubagentCache;
use crate::llm::Provider;
use crate::state::ProviderCatalog;

// 顶部 import 供 `mod tests` 的 `use super::*` 使用,lib 构建下视为未用
#[allow(unused_imports)]
use super::dispatch::*;
#[allow(unused_imports)]
use super::prep::*;
#[allow(unused_imports)]
use super::resolve::*;
#[allow(unused_imports)]
use super::worktree::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tests_common::{commit_all_for_test, init_repo_for_test};

    // -----------------------------------------------------------------------
    // resolve_isolation truth table (PRD §"已闭合" merge semantics)
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_isolation_frontmatter_true_no_override_isolates() {
        // frontmatter `isolation: worktree` + dispatch omits → isolated.
        assert!(resolve_isolation(Some(true), None));
    }

    #[test]
    fn resolve_isolation_frontmatter_true_dispatch_false_opts_out() {
        // frontmatter `isolation: worktree` + dispatch `isolation: false`
        // → NOT isolated (LLM opted out).
        assert!(!resolve_isolation(Some(true), Some(false)));
    }

    #[test]
    fn resolve_isolation_frontmatter_none_dispatch_true_opts_in() {
        // frontmatter not declared + dispatch `isolation: true`
        // → isolated (LLM opted in).
        assert!(resolve_isolation(None, Some(true)));
    }

    #[test]
    fn resolve_isolation_frontmatter_false_dispatch_false_stays_shared() {
        // frontmatter `isolation: false` + dispatch `isolation: false`
        // → NOT isolated.
        assert!(!resolve_isolation(Some(false), Some(false)));
    }

    #[test]
    fn resolve_isolation_no_default_no_override_is_legacy_shared() {
        // frontmatter not declared + dispatch omits → NOT isolated
        // (legacy shared-cwd behavior — the researcher builtin path).
        assert!(!resolve_isolation(None, None));
    }

    #[test]
    fn resolve_isolation_dispatch_input_wins_over_frontmatter() {
        // Dispatch input always wins (precedence rule).
        assert!(resolve_isolation(Some(false), Some(true)));
        assert!(!resolve_isolation(Some(true), Some(false)));
    }

    // -----------------------------------------------------------------------
    // builtin SubagentDef isolation defaults
    // -----------------------------------------------------------------------

    #[test]
    fn builtin_general_purpose_defaults_to_shared() {
        // B (2026-06-30): general-purpose ships with isolation = None
        // (shared) so a single serial dispatch reuses the parent cwd
        // (zero merge, matches Claude Code). Concurrent dispatch is
        // force-isolated in chat_loop's DispatchBatch::Concurrent branch
        // (gated by worker_is_writable) — concurrent-write safety no
        // longer relies on this default being Some(true).
        let g = super::super::lookup_subagent("general-purpose").expect("general-purpose exists");
        assert_eq!(g.isolation, None);
    }

    // -----------------------------------------------------------------------
    // worker_is_writable (B, 2026-06-30) — drives the concurrent-force-
    // isolate decision (only writable workers need a worktree when
    // dispatched concurrently).
    // -----------------------------------------------------------------------

    fn writable_def(name: &str, tools: &[&str]) -> crate::agent::subagent::SubagentDef {
        crate::agent::subagent::SubagentDef {
            name: name.to_string(),
            description: String::new(),
            system_prompt: String::new(),
            tools: tools.iter().map(|t| (*t).to_string()).collect(),
            isolation: None,
            model: None,
        }
    }

    #[test]
    fn worker_is_writable_empty_tools_inherits_full_set() {
        // Empty `tools` = inherit the full builtin set (write/shell) → writable.
        assert!(worker_is_writable(&writable_def("gp-like", &[])));
    }

    #[test]
    fn worker_is_writable_readonly_only_is_not_writable() {
        // A toolset that is exactly READONLY_TOOL_ALLOWLIST (researcher)
        // → not writable → concurrent dispatch stays shared (no write race).
        assert!(!worker_is_writable(&writable_def(
            "researcher-like",
            &["read_file", "grep", "glob", "list_dir", "web_fetch"]
        )));
    }

    #[test]
    fn worker_is_writable_with_write_tool_is_writable() {
        // A declared toolset containing a write tool → writable.
        assert!(worker_is_writable(&writable_def(
            "writer",
            &["read_file", "write_file"]
        )));
    }

    #[test]
    fn builtin_researcher_defaults_to_no_isolation() {
        // The researcher builtin ships with isolation = None (read-only
        // workers don't need a separate checkout — saves the per-
        // dispatch checkout cost).
        let r = super::super::lookup_subagent("researcher").expect("researcher exists");
        assert_eq!(r.isolation, None);
    }

    // -----------------------------------------------------------------------
    // task_with_env_hint (C, 2026-06-30)
    // -----------------------------------------------------------------------

    #[test]
    fn task_with_env_hint_isolated_appends_hint() {
        let out = task_with_env_hint("do the thing", true, "run-xyz");
        assert!(out.contains("do the thing"), "original task preserved");
        assert!(out.contains("ISOLATED git worktree"), "env hint present");
        assert!(out.contains("worker/run-xyz"), "run_id interpolated");
        assert!(out.contains("do NOT need to run"), "told not to commit");
    }

    #[test]
    fn task_with_env_hint_shared_is_unchanged() {
        let out = task_with_env_hint("do the thing", false, "run-xyz");
        assert_eq!(out, "do the thing");
    }

    // -----------------------------------------------------------------------
    // probe_worker_changes
    // -----------------------------------------------------------------------

    #[test]
    fn probe_worker_changes_empty_repo_reports_no_changes() {
        // A fresh worktree with no edits vs its base commit → no changes.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        init_repo_for_test(project);
        // Seed an empty-repo-friendly initial commit so the worker
        // worktree has a base commit to branch from (create_worker
        // resolves `base_worktree_path`'s HEAD).
        std::fs::write(project.join("seed.txt"), "seed").unwrap();
        commit_all_for_test(project, "init");

        // Create a worker worktree off the project HEAD.
        let run_id = "probe-empty";
        let worker_wt = project.join("worker_empty");
        crate::git::worktree::create_worker(project, &worker_wt, project, run_id)
            .expect("create_worker should succeed");

        let changes = probe_worker_changes(&worker_wt, run_id);
        assert!(
            !changes.has_changes,
            "empty worktree should have no changes"
        );
        assert!(changes.summary.is_empty());
    }

    #[test]
    fn probe_worker_changes_with_edits_reports_changes() {
        // A worker worktree with an edited file → reports changes.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        init_repo_for_test(project);
        // Seed a tracked file so the worker can modify it.
        std::fs::write(project.join("a.txt"), "v1").unwrap();
        commit_all_for_test(project, "init");

        let run_id = "probe-edits";
        let worker_wt = project.join("worker_edits");
        crate::git::worktree::create_worker(project, &worker_wt, project, run_id)
            .expect("create_worker should succeed");

        // Edit the tracked file in the worker's checkout.
        std::fs::write(worker_wt.join("a.txt"), "v2-from-worker").unwrap();

        let changes = probe_worker_changes(&worker_wt, run_id);
        assert!(changes.has_changes, "edited worktree should report changes");
        assert!(
            changes.summary.contains("a.txt"),
            "summary should mention the changed file: {}",
            changes.summary
        );
    }

    #[test]
    fn probe_worker_changes_with_untracked_file_reports_changes() {
        // A worker worktree that added a new (untracked) file → reports changes.
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path();
        init_repo_for_test(project);
        // Seed initial commit so create_worker has a base commit.
        std::fs::write(project.join("seed.txt"), "seed").unwrap();
        commit_all_for_test(project, "init");

        let run_id = "probe-untracked";
        let worker_wt = project.join("worker_untracked");
        crate::git::worktree::create_worker(project, &worker_wt, project, run_id)
            .expect("create_worker should succeed");

        // Add an untracked file in the worker's checkout.
        std::fs::write(worker_wt.join("new_file.txt"), "fresh").unwrap();

        let changes = probe_worker_changes(&worker_wt, run_id);
        assert!(
            changes.has_changes,
            "untracked file should count as a change"
        );
        assert!(
            changes.summary.contains("new_file.txt"),
            "summary should mention the untracked file: {}",
            changes.summary
        );
    }

    // -----------------------------------------------------------------------
    // resolve_worker_provider (task 07-03-subagent-frontmatter-model)
    // AC1 (hit swaps provider) / AC2 (None inherits) / AC3 (miss falls
    // back) / AC4 (ctx + display from model row).
    // -----------------------------------------------------------------------

    use crate::llm::provider::mock::MockProvider;
    use std::collections::HashMap;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        crate::db::run_migrations(&pool).await.unwrap();
        pool
    }

    fn mock_provider() -> Arc<dyn Provider> {
        Arc::new(MockProvider::new(vec![]))
    }

    #[tokio::test]
    async fn resolve_worker_provider_none_inherits_parent() {
        let pool = test_pool().await;
        let parent = mock_provider();
        let catalog: ProviderCatalog = HashMap::new();
        let (wp, ctx, disp) =
            resolve_worker_provider(None, &parent, 100_000, Some(&catalog), &pool).await;
        assert!(Arc::ptr_eq(&wp, &parent), "None model must inherit parent");
        assert_eq!(ctx, 100_000);
        assert!(disp.is_none());
    }

    #[tokio::test]
    async fn resolve_worker_provider_hit_swaps_provider() {
        let pool = test_pool().await;
        let parent = mock_provider();
        let worker = mock_provider();
        let mut catalog: ProviderCatalog = HashMap::new();
        catalog.insert("model-worker".to_string(), worker.clone());
        let (wp, _ctx, _disp) = resolve_worker_provider(
            Some("model-worker"),
            &parent,
            100_000,
            Some(&catalog),
            &pool,
        )
        .await;
        assert!(
            !Arc::ptr_eq(&wp, &parent),
            "catalog hit must swap away from parent"
        );
        assert!(
            Arc::ptr_eq(&wp, &worker),
            "worker provider must be the catalog entry"
        );
    }

    #[tokio::test]
    async fn resolve_worker_provider_miss_falls_back_to_parent() {
        let pool = test_pool().await;
        let parent = mock_provider();
        let catalog: ProviderCatalog = HashMap::new();
        let (wp, _ctx, disp) = resolve_worker_provider(
            Some("nonexistent-id"),
            &parent,
            100_000,
            Some(&catalog),
            &pool,
        )
        .await;
        assert!(
            Arc::ptr_eq(&wp, &parent),
            "catalog miss must fall back to parent"
        );
        assert!(disp.is_none());
    }

    #[tokio::test]
    async fn resolve_worker_provider_catalog_none_falls_back() {
        // catalog=None (tests, no AppHandle) + model=Some → parent.
        let pool = test_pool().await;
        let parent = mock_provider();
        let (wp, _, _) =
            resolve_worker_provider(Some("any-id"), &parent, 100_000, None, &pool).await;
        assert!(Arc::ptr_eq(&wp, &parent));
    }

    #[tokio::test]
    async fn resolve_worker_provider_hit_reads_ctx_and_display() {
        // AC4: hit + DB has the model row → ctx = model.context_window,
        // disp = display_name (NOT the parent's).
        let pool = test_pool().await;
        let provider_row = crate::db::providers::create_provider(
            &pool,
            "anthropic",
            "Anthropic",
            "https://api.anthropic.com",
            "sk-test",
        )
        .await
        .unwrap();
        let model_row = crate::db::models::create_model(
            &pool,
            &provider_row.id,
            "claude-test",
            "Claude Test",
            None,
            None,
            false,
            50_000,
        )
        .await
        .unwrap();
        let worker = mock_provider();
        let mut catalog: ProviderCatalog = HashMap::new();
        catalog.insert(model_row.id.clone(), worker.clone());
        let parent = mock_provider();
        let (wp, ctx, disp) =
            resolve_worker_provider(Some(&model_row.id), &parent, 100_000, Some(&catalog), &pool)
                .await;
        assert!(Arc::ptr_eq(&wp, &worker));
        assert_eq!(ctx, 50_000, "ctx must come from the model row, not parent");
        assert_eq!(disp.as_deref(), Some("Claude Test"));
    }

    // -----------------------------------------------------------------------
    // resolve_final_model (task 07-03-subagent-per-agent-model-ui, 阶段 1)
    //
    // AC1 (UI: builtin override wins) / AC2 (DB > frontmatter) /
    // AC3 (frontmatter > parent) / AC4 (都无 → parent) / AC9 (DB miss
    // 指向失效 model: catalog miss 走 parent, NOT frontmatter fallback).
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_final_model_db_wins_over_frontmatter() {
        // AC2: both DB and frontmatter declare a model → DB wins.
        let pool = test_pool().await;
        crate::db::subagent_overrides::set_subagent_model_override(
            &pool,
            "researcher",
            "model-from-db",
        )
        .await
        .unwrap();
        let got = resolve_final_model(&pool, "researcher", Some("model-from-fm"))
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some("model-from-db"));
    }

    #[tokio::test]
    async fn resolve_final_model_only_frontmatter() {
        // AC3: only frontmatter → frontmatter.
        let pool = test_pool().await;
        let got = resolve_final_model(&pool, "researcher", Some("model-from-fm"))
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some("model-from-fm"));
    }

    #[tokio::test]
    async fn resolve_final_model_only_db() {
        // Only DB → DB (frontmatter is None).
        let pool = test_pool().await;
        crate::db::subagent_overrides::set_subagent_model_override(
            &pool,
            "researcher",
            "model-from-db",
        )
        .await
        .unwrap();
        let got = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some("model-from-db"));
    }

    #[tokio::test]
    async fn resolve_final_model_neither_returns_none_for_parent_inheritance() {
        // AC4: no DB + no frontmatter → None (resolve_worker_provider
        // then inherits parent provider + ctx).
        let pool = test_pool().await;
        let got = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn resolve_final_model_dangling_db_override_still_returns_some() {
        // AC9 (priority chain invariant): `resolve_final_model` is
        // intentionally catalog-agnostic — a DB override pointing
        // at a deleted model STILL returns `Some(<deleted-id>)` so
        // the resolver can chain into `resolve_worker_provider`
        // (which logs `warn!` + falls back to parent on catalog
        // miss). The fall-back is NOT to frontmatter (per the
        // priority decision in the doc comment); it's directly to
        // parent. This test pins that behavior so a future refactor
        // doesn't silently change the catalog-miss path.
        let pool = test_pool().await;
        crate::db::subagent_overrides::set_subagent_model_override(
            &pool,
            "researcher",
            "model-deleted",
        )
        .await
        .unwrap();
        let got = resolve_final_model(&pool, "researcher", Some("model-from-fm"))
            .await
            .unwrap();
        assert_eq!(
            got.as_deref(),
            Some("model-deleted"),
            "DB override wins even when frontmatter is also set"
        );
        // The catalog-miss fallback to parent is tested in
        // `resolve_worker_provider_miss_falls_back_to_parent` above
        // (the same `model-id-not-in-catalog` case there covers the
        // downstream half of AC9).
    }

    // -----------------------------------------------------------------------
    // resolve_model_by_name_or_id (task 07-06-b6plus-b-dispatch-model-arg)
    //
    // B6+ B: the display_name→id reverse-lookup for the LLM-driven
    // dispatch path (schema `model` enum values are display_names).
    // AC1 (display_name→id) / id passthrough / miss→None.
    // -----------------------------------------------------------------------

    /// Helper: create a provider + model row, return the model row.
    async fn create_provider_and_model(
        pool: &SqlitePool,
        display_name: &str,
        model_name: &str,
        ctx: u32,
    ) -> crate::db::ModelRow {
        let provider_row = crate::db::providers::create_provider(
            pool,
            "anthropic",
            "Anthropic",
            "https://api.anthropic.com",
            "sk-test",
        )
        .await
        .unwrap();
        crate::db::models::create_model(
            pool,
            &provider_row.id,
            model_name,
            display_name,
            None,
            None,
            false,
            ctx,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn resolve_model_by_name_or_id_id_passthrough() {
        // An exact model id is returned verbatim.
        let pool = test_pool().await;
        let row = create_provider_and_model(&pool, "Claude Test", "claude-test", 50_000).await;
        let got = resolve_model_by_name_or_id(&pool, &row.id).await.unwrap();
        assert_eq!(got.as_deref(), Some(row.id.as_str()));
    }

    #[tokio::test]
    async fn resolve_model_by_name_or_id_display_name_lookup() {
        // A display_name resolves to the corresponding id.
        let pool = test_pool().await;
        let row = create_provider_and_model(&pool, "GPT-4o", "gpt-4o", 128_000).await;
        let got = resolve_model_by_name_or_id(&pool, "GPT-4o").await.unwrap();
        assert_eq!(got.as_deref(), Some(row.id.as_str()));
    }

    #[tokio::test]
    async fn resolve_model_by_name_or_id_display_name_first_match_wins() {
        // Multiple models share a display_name → first match (by
        // list_models ordering) wins. Deterministic for a given DB
        // state; the design accepts this (display_name should be
        // unique but DB does not enforce it).
        let pool = test_pool().await;
        let _row_a = create_provider_and_model(&pool, "Dup", "dup-a", 50_000).await;
        let _row_b = create_provider_and_model(&pool, "Dup", "dup-b", 60_000).await;
        let got = resolve_model_by_name_or_id(&pool, "Dup").await.unwrap();
        assert!(got.is_some(), "duplicate display_name must still resolve");
    }

    #[tokio::test]
    async fn resolve_model_by_name_or_id_miss_returns_none() {
        // Unknown display_name / id → Ok(None) (NOT an error).
        let pool = test_pool().await;
        let _row = create_provider_and_model(&pool, "Real", "real", 50_000).await;
        let got = resolve_model_by_name_or_id(&pool, "nonexistent")
            .await
            .unwrap();
        assert!(
            got.is_none(),
            "unknown input must resolve to None, not error"
        );
    }

    #[tokio::test]
    async fn resolve_model_by_name_or_id_empty_returns_none() {
        // Empty / whitespace input → None (the parser filters these,
        // but the function is defensive).
        let pool = test_pool().await;
        assert!(resolve_model_by_name_or_id(&pool, "")
            .await
            .unwrap()
            .is_none());
        assert!(resolve_model_by_name_or_id(&pool, "   ")
            .await
            .unwrap()
            .is_none());
    }

    // -----------------------------------------------------------------------
    // Priority overlay (task 07-06-b6plus-b-dispatch-model-arg)
    //
    // The overlay `final_model = dispatch_model.or(resolved_lower)` lives
    // inside `run_subagent` as a one-liner; these tests pin the priority
    // semantics by exercising the composition directly. The dispatch_model
    // arm is the reverse-lookup result; resolved_lower is
    // `resolve_final_model`. Together: dispatch > DB > frontmatter > parent.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn priority_overlay_dispatch_overrides_db_override() {
        // AC2: dispatch_model=X + DB override=Y → final=X.
        let pool = test_pool().await;
        let row_x = create_provider_and_model(&pool, "X", "x", 50_000).await;
        crate::db::subagent_overrides::set_subagent_model_override(
            &pool,
            "researcher",
            "model-from-db-y",
        )
        .await
        .unwrap();
        let dispatch_model = Some(row_x.id.clone());
        let resolved_lower = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert_eq!(final_model.as_deref(), Some(row_x.id.as_str()));
    }

    #[tokio::test]
    async fn priority_overlay_dispatch_overrides_frontmatter() {
        // AC3: dispatch_model=X + frontmatter=Y (no DB) → final=X.
        let pool = test_pool().await;
        let row_x = create_provider_and_model(&pool, "X", "x", 50_000).await;
        let dispatch_model = Some(row_x.id.clone());
        let resolved_lower = resolve_final_model(&pool, "researcher", Some("fm-y"))
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert_eq!(final_model.as_deref(), Some(row_x.id.as_str()));
    }

    #[tokio::test]
    async fn priority_overlay_none_dispatch_falls_to_db() {
        // AC4 zero-regression: no dispatch_model + DB override=Y → final=Y.
        let pool = test_pool().await;
        crate::db::subagent_overrides::set_subagent_model_override(
            &pool,
            "researcher",
            "model-from-db-y",
        )
        .await
        .unwrap();
        let dispatch_model: Option<String> = None;
        let resolved_lower = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert_eq!(final_model.as_deref(), Some("model-from-db-y"));
    }

    #[tokio::test]
    async fn priority_overlay_none_dispatch_none_db_falls_to_frontmatter() {
        // AC4: no dispatch + no DB → frontmatter.
        let pool = test_pool().await;
        let dispatch_model: Option<String> = None;
        let resolved_lower = resolve_final_model(&pool, "researcher", Some("fm-y"))
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert_eq!(final_model.as_deref(), Some("fm-y"));
    }

    #[tokio::test]
    async fn priority_overlay_all_none_inherits_parent() {
        // AC4: no dispatch + no DB + no frontmatter → None (parent).
        let pool = test_pool().await;
        let dispatch_model: Option<String> = None;
        let resolved_lower = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert!(final_model.is_none());
    }

    #[tokio::test]
    async fn priority_overlay_unknown_dispatch_display_name_becomes_none() {
        // AC7: the LLM sends a display_name that reverse-lookup misses
        // → dispatch_model=None → final falls to resolve_final_model.
        let pool = test_pool().await;
        crate::db::subagent_overrides::set_subagent_model_override(
            &pool,
            "researcher",
            "model-from-db-y",
        )
        .await
        .unwrap();
        let dispatch_model = resolve_model_by_name_or_id(&pool, "nonexistent-display")
            .await
            .unwrap();
        assert!(dispatch_model.is_none(), "miss must produce None");
        let resolved_lower = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert_eq!(
            final_model.as_deref(),
            Some("model-from-db-y"),
            "miss dispatch must degrade to the DB override, not parent"
        );
    }

    #[tokio::test]
    async fn priority_overlay_dispatch_miss_inherits_parent_when_no_lower() {
        // AC7: miss dispatch + no DB/frontmatter → None (parent).
        let pool = test_pool().await;
        let dispatch_model = resolve_model_by_name_or_id(&pool, "ghost").await.unwrap();
        assert!(dispatch_model.is_none());
        let resolved_lower = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        let final_model = dispatch_model.or(resolved_lower);
        assert!(final_model.is_none());
    }

    #[tokio::test]
    async fn priority_overlay_idempotent_across_dispatches() {
        // R3: per-dispatch override does NOT persist. Two identical
        // resolve_final_model calls (no DB write between) return the
        // same value — the dispatch overlay is per-call only.
        let pool = test_pool().await;
        let row = create_provider_and_model(&pool, "X", "x", 50_000).await;
        let resolved_1 = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        // Simulate a dispatch with model X (does not write DB/frontmatter).
        let _final_1 = Some(row.id.clone()).or(resolved_1.clone());
        let resolved_2 = resolve_final_model(&pool, "researcher", None)
            .await
            .unwrap();
        assert_eq!(resolved_1, resolved_2, "dispatch must not persist");
    }

    #[tokio::test]
    async fn dispatch_input_model_field_parsed_as_dispatch_model() {
        // §3.2: `input.model` (a display_name, as the LLM would send
        // from the schema enum) reverse-resolves to an id. This mirrors
        // the parse logic in run_subagent: input.model → raw →
        // resolve_model_by_name_or_id → dispatch_model.
        let pool = test_pool().await;
        let row = create_provider_and_model(&pool, "GPT-4o", "gpt-4o", 128_000).await;
        let input = serde_json::json!({ "model": "GPT-4o" });
        let raw: Option<&str> = input
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let dispatch_model = match raw {
            Some(r) => resolve_model_by_name_or_id(&pool, r).await.unwrap(),
            None => None,
        };
        assert_eq!(dispatch_model.as_deref(), Some(row.id.as_str()));
    }

    // ---- Step 2.4: workflow role-gate (pure helper) ----
    //
    // These tests target `check_workflow_role_gate`
    // directly — the gate logic is a pure function so
    // no LLM mocks / provider wiring are required. The
    // integration with `run_subagent` is verified by
    // the existing dispatch tests + the manual end-to-end
    // checklist in `implement.md` Step 2.4 validation.

    use crate::agent::workflow::{Coordination, TaskJson, TaskStatus, WorkflowCtx, WorkflowDef};

    fn dev_workflow_def() -> WorkflowDef {
        WorkflowDef {
            name: "dev".to_string(),
            description: "test".to_string(),
            states: vec!["planning".into(), "in_progress".into(), "done".into()],
            initial: "planning".into(),
            transitions: vec![],
            roles_by_state: HashMap::from([
                ("planning".to_string(), vec!["researcher".to_string()]),
                (
                    "in_progress".to_string(),
                    vec!["implementer".to_string(), "checker".to_string()],
                ),
                ("done".to_string(), vec![]),
            ]),
            breadcrumb: HashMap::new(),
            delegation_templates: HashMap::new(),
            coordination: Coordination::Pipeline,
            gather_strategy: HashMap::new(),
        }
    }

    fn ctx_with_status(status: TaskStatus) -> WorkflowCtx {
        let workflow_def = dev_workflow_def();
        WorkflowCtx {
            task_workflow_def: workflow_def.clone(),
            workflow_def,
            current_task: Some(TaskJson {
                id: "t1".into(),
                title: "x".into(),
                slug: "x".into(),
                status,
                created_at: "2026-07-08T00:00:00Z".into(),
                updated_at: "2026-07-08T00:00:00Z".into(),
                parent: None,
                summary: String::new(),
                items: vec![],
                // Step 3.3: pre-archive fixture.
                completed_at: None,
                workflow_plugin: "dev".into(),
            }),
        }
    }

    /// Gate denial: planning session dispatching a role
    /// not in planning's allowed list → tool_error body.
    #[test]
    fn gate_denies_role_not_allowed_in_current_state() {
        let ctx = ctx_with_status(TaskStatus::Planning);
        let input = serde_json::json!({"subagent": "general-purpose"});
        let denial = check_workflow_role_gate(Some(&ctx), "general-purpose", &input);
        let msg = denial.expect("gate must deny general-purpose in planning");
        assert!(msg.contains("Role gate denied"), "got: {msg}");
        assert!(msg.contains("general-purpose"), "must name role");
        assert!(msg.contains("planning"), "must name state");
        assert!(msg.contains("researcher"), "must enumerate allowed");
    }

    /// Gate allowance: planning session dispatching
    /// `researcher` (in planning's allowed list) → None.
    #[test]
    fn gate_allows_role_in_current_state() {
        let ctx = ctx_with_status(TaskStatus::Planning);
        let input = serde_json::json!({"subagent": "researcher"});
        let denial = check_workflow_role_gate(Some(&ctx), "researcher", &input);
        assert!(denial.is_none(), "researcher IS allowed in planning");
    }

    /// One-shot bypass: `force: true` overrides denial.
    #[test]
    fn gate_force_bypass_overrides_denial() {
        let ctx = ctx_with_status(TaskStatus::Planning);
        let input = serde_json::json!({"force": true});
        let denial = check_workflow_role_gate(Some(&ctx), "general-purpose", &input);
        assert!(
            denial.is_none(),
            "force=true must bypass the gate (got: {:?})",
            denial
        );
    }

    /// State-driven enforcement: same role, different
    /// states → different verdicts. Confirms the gate
    /// consults `current_task.status`, not just the role.
    #[test]
    fn gate_enforcement_is_state_driven() {
        let input = serde_json::json!({"subagent": "implementer"});

        // in_progress + implementer → allowed
        let ctx_impl = ctx_with_status(TaskStatus::InProgress);
        assert!(check_workflow_role_gate(Some(&ctx_impl), "implementer", &input).is_none());

        // in_progress + checker → also allowed (post-merge: both roles valid in in_progress)
        assert!(
            check_workflow_role_gate(Some(&ctx_impl), "checker", &input).is_none(),
            "checker must be allowed in in_progress post-merge"
        );

        // planning + implementer → denied
        let ctx_plan = ctx_with_status(TaskStatus::Planning);
        let denial = check_workflow_role_gate(Some(&ctx_plan), "implementer", &input);
        assert!(denial.is_some(), "implementer must be denied in planning");
    }

    /// Non-workflow short-circuit: `workflow_ctx = None`
    /// → gate does not engage (legacy dispatch
    /// behavior preserved).
    #[test]
    fn gate_short_circuits_when_no_workflow_ctx() {
        let input = serde_json::json!({"subagent": "general-purpose"});
        let denial = check_workflow_role_gate(None, "general-purpose", &input);
        assert!(
            denial.is_none(),
            "non-workflow session must short-circuit the gate",
        );
    }

    /// No-active-task short-circuit: workflow session with
    /// `current_task = None` (task bootstrap state) → no
    /// enforcement, no error.
    #[test]
    fn gate_short_circuits_when_no_current_task() {
        let workflow_def = dev_workflow_def();
        let ctx = WorkflowCtx {
            task_workflow_def: workflow_def.clone(),
            workflow_def,
            current_task: None,
        };
        let input = serde_json::json!({"subagent": "general-purpose"});
        let denial = check_workflow_role_gate(Some(&ctx), "general-purpose", &input);
        assert!(
            denial.is_none(),
            "no current_task (bootstrap state) must short-circuit the gate",
        );
    }

    /// Done-state enforcement: planning's `done` state
    /// has empty allowed roles; ANY dispatch is denied
    /// (mirrors the dev plugin's "done triggers archive"
    /// semantics — no further sub-agent work after done).
    #[test]
    fn gate_done_state_has_no_allowed_roles() {
        let ctx = ctx_with_status(TaskStatus::Done);
        let input = serde_json::json!({"subagent": "researcher"});
        let denial = check_workflow_role_gate(Some(&ctx), "researcher", &input);
        let msg = denial.expect("researcher must be denied in done");
        assert!(
            msg.contains("(none)"),
            "done's allowed list is empty: {msg}"
        );
    }

    /// C5 (2026-07-28): the role gate MUST use the task's owning
    /// plugin (`task_workflow_def`), not the session plugin. This
    /// test constructs the exact dead-lock scenario from session
    /// 04c62fab: a dev-created task (status=planning) opened in a
    /// review session. Before the fix, the gate queried review's
    /// `roles_by_state` with key "planning" → empty → denied all.
    /// After the fix, the gate uses dev's state machine and correctly
    /// allows `researcher` in `planning`.
    #[test]
    fn gate_uses_task_owning_plugin_not_session_plugin() {
        let dev_def = dev_workflow_def();
        // Minimal review workflow def: states don't include "planning",
        // so review's roles_by_state["planning"] is absent (empty).
        let review_def = crate::agent::workflow::WorkflowDef {
            name: "review".into(),
            description: String::new(),
            states: vec!["intake".into(), "reviewing".into()],
            initial: "intake".into(),
            transitions: vec![],
            roles_by_state: {
                let mut m = std::collections::HashMap::new();
                m.insert("reviewing".into(), vec!["reviewer".into()]);
                m
            },
            breadcrumb: std::collections::HashMap::new(),
            delegation_templates: std::collections::HashMap::new(),
            coordination: crate::agent::workflow::Coordination::Pipeline,
            gather_strategy: std::collections::HashMap::new(),
        };
        // Session is review, but task belongs to dev (status=planning).
        let ctx = WorkflowCtx {
            workflow_def: review_def,   // session plugin (review)
            task_workflow_def: dev_def, // task's owning plugin (dev)
            current_task: Some(TaskJson {
                id: "t1".into(),
                title: "x".into(),
                slug: "x".into(),
                status: TaskStatus::Planning,
                created_at: "2026-07-08T00:00:00Z".into(),
                updated_at: "2026-07-08T00:00:00Z".into(),
                parent: None,
                summary: String::new(),
                items: vec![],
                completed_at: None,
                workflow_plugin: "dev".into(),
            }),
        };
        let input = serde_json::json!({"subagent": "researcher"});
        // dev's planning allows researcher — gate must pass even
        // though the session plugin (review) has no "planning" entry.
        let denial = check_workflow_role_gate(Some(&ctx), "researcher", &input);
        assert!(
            denial.is_none(),
            "role gate must use task's dev plugin (planning allows researcher), \
             not session's review plugin; got denial: {:?}",
            denial
        );
    }

    // ---- Step 2.7: workflow-aware dispatch resolution wiring ----
    //
    // `run_subagent`'s lookup branch (dispatch.rs ~line 377) now
    // routes to `lookup_with_workflow` when `workflow_ctx` carries
    // a plugin name, so the plugin `.everlasting/workflow/<wf>/agents/`
    // layer wins over builtin/user/project. Before Step 2.7 the
    // dispatch path always used the legacy `lookup`, so a plugin's
    // `researcher.md` (Step 2.3) was never loaded even though the
    // role-gate (Step 2.4) correctly *allowed* the role.
    //
    // `run_subagent` itself needs 25+ args + a live provider/db, so
    // a full end-to-end dispatch test is out of scope here. Instead
    // this test pins the cache-level contract the dispatch branch
    // depends on: a plugin-only agent body is reachable via
    // `lookup_with_workflow` under the workflow name the dispatch
    // branch reads from `workflow_ctx.workflow_def.name`. If this
    // test regresses, the dispatch branch silently falls back to
    // the builtin body (the exact Step 2.7 bug class).

    #[tokio::test]
    async fn workflow_dispatch_resolves_plugin_agent_body() {
        use crate::agent::subagent::SubagentSource;

        // Plugin agent lives ONLY in the workflow plugin layer.
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let plugin_dir = proj_tmp
            .path()
            .join(".everlasting")
            .join("workflow")
            .join("dev")
            .join("agents");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("researcher.md"),
            "---\nname: researcher\ndescription: \"\"\n---\nPLUGIN_BODY_STEP27",
        )
        .unwrap();

        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        // Workflow name pulled from `WorkflowCtx.workflow_def.name`
        // exactly as dispatch.rs ~line 376 does.
        let wf_name = dev_workflow_def().name; // "dev"
        let loaded = cache
            .lookup_with_workflow(&project_path, Some(&wf_name), "researcher")
            .await
            .expect("plugin researcher must resolve via lookup_with_workflow");
        assert_eq!(
            loaded.source,
            SubagentSource::Plugin,
            "dispatch branch must see the plugin layer, not the builtin",
        );
        assert!(
            loaded.def.system_prompt.contains("PLUGIN_BODY_STEP27"),
            "dispatch branch must load the plugin body, not the builtin body",
        );
    }

    // ----- C1 (07-26-subagent-resume): build_clarification_message -----

    #[test]
    fn c1_clarification_none_when_field_absent() {
        let input = serde_json::json!({"task": "do thing"});
        assert!(build_clarification_message(&input).is_none());
    }

    #[test]
    fn c1_clarification_none_without_purpose() {
        // this_round_purpose is required — without it the clarification
        // is meaningless, so we drop it (resume proceeds with just the
        // replayed history + task, no clarification stanza).
        let input = serde_json::json!({
            "resume_clarification": {"current_state": "x"}
        });
        assert!(build_clarification_message(&input).is_none());
    }

    #[test]
    fn c1_clarification_full_stanza_with_all_fields() {
        let input = serde_json::json!({
            "resume_clarification": {
                "current_state": "PRD revised: scope trimmed to MVP",
                "changes_since_last": ["§2 scope reduced", "§4 added acceptance criteria"],
                "this_round_purpose": "verify the high-severity findings are resolved"
            }
        });
        let msg = build_clarification_message(&input).expect("present");
        match &msg.content {
            crate::llm::types::MessageContent::Text(t) => {
                assert!(t.contains("[resume clarification"));
                assert!(t.contains("**Current state:** PRD revised"));
                assert!(t.contains("**Changes since your last turn:**"));
                assert!(t.contains("- §2 scope reduced"));
                assert!(t.contains("- §4 added acceptance criteria"));
                assert!(t.contains("**This round's purpose:** verify the high-severity"));
            }
            other => panic!("expected Text, got {:?}", other),
        }
        assert_eq!(msg.role, crate::llm::types::Role::User);
    }

    #[test]
    fn c1_clarification_omits_empty_optional_sections() {
        // current_state empty + changes_since_last empty/missing →
        // those sections are dropped; only the header + purpose remain.
        let input = serde_json::json!({
            "resume_clarification": {
                "current_state": "",
                "this_round_purpose": "just check again"
            }
        });
        let msg = build_clarification_message(&input).expect("present");
        match &msg.content {
            crate::llm::types::MessageContent::Text(t) => {
                assert!(t.contains("[resume clarification"));
                assert!(!t.contains("**Current state:**"));
                assert!(!t.contains("**Changes since your last turn:**"));
                assert!(t.contains("**This round's purpose:** just check again"));
            }
            other => panic!("expected Text, got {:?}", other),
        }
    }
}
