//! `projects` module — Project data model + ToolContext boundary.
//!
//! This module owns the project-level data model: a directory on disk
//! the user has registered, optionally a git repository, with sessions
//! scoped to it. Step 4 will use `ProjectRow.id` as the worktree
//! namespace (see `docs/ARCHITECTURE.md` §3 worktree path:
//! `~/.local/share/everlasting/worktrees/<project_uuid>/<session_id>`).
//!
//! Files:
//! - [`types`] — `ProjectRow` and friends (DTO for IPC)
//! - [`detector`] — `is_git_repo` probe (shell-out, no `git2` dep yet)
//! - [`boundary`] — `assert_within_root` (the 7-edge-case contract)
//! - [`store`] — façade over `db.rs` for the project-level operations
//!
//! `ToolContext` lives in `crate::tools` (where it is constructed by
//! the agent loop and consumed by the tools); `boundary` is referenced
//! by both `tools::shell` / `tools::read_file` / `tools::write_file`
//! and by the `chat` command (to canonicalize the project's root and
//! the session's `current_cwd` once, at command start).

pub mod boundary;
pub mod detector;
pub mod store;
pub mod types;

pub use types::ProjectRow;

/// Stable ID for the Auto-default project that backstops legacy
/// sessions. Per `docs/PROPOSAL-project-binding-and-top-tabs.md` §3.4
/// this ID is fixed (not a random UUID) so historical sessions can
/// be re-associated with it deterministically after a migration.
pub const DEFAULT_PROJECT_ID: &str = "__default__";
