//! L3b PR3 (2026-06-27): `merge_worker` tool.
//!
//! Merges a worker's `worker/<run_id>` branch (left behind by an
//! isolated worker run that exited with changes) into the parent
//! session's `session/<id>` branch. Reuses libgit2's three-way
//! merge API (`Repository::merge`); on conflict, returns an
//! `is_error: true` tool_result with the conflict file list and
//! leaves both branches intact (the worker branch + worktree
//! stay preserved for the user to inspect / resolve manually).
//!
//! On success, calls PR1's [`crate::git::worktree::destroy_worker`]
//! to remove the worker worktree + delete the `worker/<run_id>`
//! branch + clear the `subagent_runs.worktree_path` column. The
//! fast-forward path is preferred (the typical case after a
//! general-purpose worker that wrote to its own checkout without
//! touching the parent branch).
//!
//! Why this is a **tool** (not just a Tauri command): the LLM
//! drives the call. After a worker reports it changed `a.rs` /
//! `b.rs`, the parent LLM decides to merge the changes back. The
//! tool is the LLM's seam for that decision; the dedicated Tauri
//! command (`merge_worker_run`) exists only so the frontend
//! `<SubagentDrawer>` PR4 can expose a manual button.
//!
//! ⑨ 关 routing: `Risk::High` (per `permissions::types::risk_for_tool`).
//! The Tier 4 path branch classifies it as `ToolKind::GitMutation`
//! (tool-level grant + ask, mirroring WebFetch — the `run_id` is a
//! database key, not a filesystem path, so the modal renders no
//! path-scope row). Plan mode filters it out (`filter_tools_for_mode`
//! lists `merge_worker`/`discard_worker`).
//!
//! Concurrency: per-parent-session merge serialization is enforced in
//! [`do_merge_blocking`] via [`merge_lock_for`] (a `std::sync::Mutex`
//! keyed by `parent_session_id`). Both `spawn_blocking` call sites
//! (this tool's [`execute`] + the `merge_worker_run` IPC command) flow
//! through it, so concurrent merges into the same parent branch are
//! serialized; independent sessions still merge in parallel.
//!
//! Hub for the `merge_worker/` directory module (split 2026-08-08
//! batch3). The tool definition + execute entry live in [`execute`];
//! the merge bodies + the per-session `static LOCKS` live in [`merge`];
//! post-merge cleanup + parent-worktree attach live in [`finalize`].
//! This hub re-exports the public surface so existing
//! `crate::tools::merge_worker::*` callers keep resolving unchanged.

pub mod execute;
pub mod finalize;
pub mod merge;

#[allow(unused_imports)]
pub use execute::{definition, execute};
#[allow(unused_imports)]
pub use finalize::{ensure_parent_worktree_attached, finalize_merge};
#[allow(unused_imports)]
pub use merge::{do_merge_blocking, merge_session_into_main};
