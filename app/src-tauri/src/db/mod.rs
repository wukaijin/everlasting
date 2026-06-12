//! SQLite persistence for sessions, messages, and projects.
//!
//! Tables:
//! - `projects`: one row per registered directory (the user's "work
//! environment"); sessions are scoped to a project.
//! - `sessions`: one row per conversation, scoped to a project; tracks
//! title/timestamps/model and the current working directory the
//! agent is in.
//! - `messages`: one row per message, `content` is JSON-serialized
//! `Vec<ContentBlock>` so tool_use/tool_result/thinking round-trips
//! losslessly.
//! - `providers` / `models` / `app_config` (PR1 of multi-model task):
//! user-managed LLM provider catalog. `providers` holds the
//! connection details (base_url + api_key per protocol); `models`
//! binds a model name to a provider with capability hints
//! (`supports_thinking`, `context_window`); `app_config` is a small
//! key/value store for global settings (currently only
//! `default_model_id`). `sessions.model_id` is a soft FK to
//! `models.id` — kept nullable so legacy rows from the pre-PR1 era
//! (`model TEXT` only) still load; the seed function backfills
//! `model_id` from `default_model_id` on first run.
//!
//! Schema is created idempotently by [`run_migrations`], so re-running
//! the app or upgrading doesn't error out on existing tables. New
//! columns for `sessions` (step3b-1: `project_id`, `current_cwd`; step4:
//! `worktree_path`; PR1 of multi-model: `model_id`) are added via
//! non-destructive `ALTER TABLE ... ADD COLUMN` so the upgrade from
//! any prior step preserves every existing row.
//!
//! The Auto-default project (id = `__default__`) backstops legacy
//! sessions: any pre-3b-1 row gets `project_id = '__default__'`
//! during the migration, and the user can later reassign it via
//! sqlite or a future "Manage projects" panel. See
//! `docs/PROPOSAL-project-binding-and-top-tabs.md` §3.4.
//!
//! Post-PR2 of the audit task: this module is a thin facade. The
//! actual logic lives in:
//!
//! - [`types`] — Row structs + enums (`ProviderRow`, `ModelRow`,
//! `ProjectRow`, `SessionRow`, `SessionSummary`, `MessageRow`,
//! `LoadedSession`, `WorktreeState`, `ProviderProtocol`).
//! - [`migrations`] — `init_pool`, `run_migrations`, and the
//! per-table column-probe helpers.
//! - [`projects`] — Project CRUD + the `row_to_project` helper.
//! - [`sessions`] — Session CRUD + worktree-state transitions,
//! system-event injection, `persist_turn`.
//! - [`providers`] — Provider CRUD.
//! - [`models`] — Model CRUD (denormalized join with `ProviderRow`).
//! - [`config`] — `app_config` KV + the `seed_default_providers_and_models`
//! bootstrap.
//! - [`tests`] — `#[cfg(test)]` integration tests for every CRUD
//! path.
//!
//! All public types and functions are re-exported here so the
//! pre-PR2 `db::FooRow` / `db::crud_fn(...)` paths keep working
//! without any caller change.

pub mod config;
pub mod migrations;
pub mod models;
pub mod permissions;
pub mod projects;
pub mod providers;
pub mod sessions;
pub mod tests;
pub mod types;

// Re-export every public item from the submodules so callers can
// keep using the pre-PR2 `db::FooRow` / `db::crud_fn(...)` paths.
// The `pub use types::*` covers the row structs + enums;
// the domain submodules cover their CRUD functions.
pub use config::*;
pub use migrations::*;
pub use models::*;
pub use permissions::*;
pub use projects::*;
pub use providers::*;
pub use sessions::*;
// (tests is `#[cfg(test)]`-gated internally; nothing to re-export.)
pub use types::*;
