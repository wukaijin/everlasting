//! Git worktree lifecycle: create on session create, destroy on
//! session delete.
//!
//! Why worktrees (recap of `docs/ARCHITECTURE.md §3`):
//! - Different sessions can be active simultaneously (per-session
//!   independence is a first-class concern — see PR3 in
//!   `streamController`).
//! - worktree shares `.git/` but working dir is independent.
//! - Each session gets its own branch `session/<session_id>`; the
//!   user sees the diff between their worktree and the project's
//!   main branch.
//!
//! Why `git2-rs` (recap of
//! `.trellis/tasks/06-07-step-4-git-worktree-auto-commit/research/git-backend.md`):
//! - libgit2 covers `worktree add/list/find/lock/unlock/prune/validate`
//!   100% of what we need for step 4.
//! - libgit2 C API has no `worktree remove` (this is the real reason
//!   ARCH §3 warned about "worktree API not complete"). We work
//!   around it with `std::fs::remove_dir_all` + `Worktree::prune`.
//! - Branch delete is a separate libgit2 call (`Branch::delete`).
//!
//! This file is the module hub: implementations live in the
//! `worktree/` submodules (`naming`, `create`, `lifecycle`, `sweep`,
//! `check`), re-exported here so `git::worktree::X` call sites are
//! unchanged after the 08-08 split.

mod check;
mod create;
mod lifecycle;
mod naming;
mod sweep;

// Re-export the full public surface so `git::worktree::*` paths stay
// stable. `git/mod.rs` does `pub use worktree::{check_clean, destroy
// as destroy_worktree}` — both resolve through this re-export. The
// `#[allow(unused_imports)]` mirrors the wire/mod.rs convention: in a
// binary crate `pub use` re-exports without an in-crate consumer trip
// the lint even though the symbols are part of the module's API.
#[allow(unused_imports)]
pub use check::check_clean;
#[allow(unused_imports)]
pub use create::{create, create_worker};
#[allow(unused_imports)]
pub use lifecycle::{attach_session, destroy, destroy_worker};
#[allow(unused_imports)]
pub use naming::{
    branch_name, worker_branch_name, worker_worktree_path, worktree_path, SESSION_BRANCH_PREFIX,
    WORKER_BRANCH_PREFIX,
};
#[allow(unused_imports)]
pub use sweep::{
    commit_worker_changes, resolve_cleanup_period_days, sweep_stale_worker_worktrees,
    CLEANUP_PERIOD_DAYS_ENV, DEFAULT_CLEANUP_PERIOD_DAYS,
};
