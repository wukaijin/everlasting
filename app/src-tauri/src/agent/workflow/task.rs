//! W1 (Workflow integration, Phase 0 Step 0.4 — 2026-07-08):
//! workflow **task** 文件态 — `.everlasting/tasks/<slug>/task.json`
//! 读 + 写 + 初始模板生成。这一层**纯文件 IO**,不读 DB 不调 LLM
//! 不开 tokio runtime(全 sync),这样 Step 0.5 的 chat_loop per-turn
//! 注入可以同步调用而不破坏现有 async 签名(同步代码包在
//! `spawn_blocking` 里)。
//!
//! ## 目录约定
//!
//! ```text
//! <project_path>/.everlasting/tasks/<slug>/
//!     ├── task.json     # 元数据 + items 列表(本文焦点)
//!     ├── prd.md        # 创建时填的 skeleton 由 create_task 写
//!     ├── design.md     # agent 用 wf-before-dev 自行追加
//!     └── progress.md   # agent 推进时 + 跨 session 续上时追加
//! ```
//!
//! ## task.json Schema
//!
//! Minimal v1:
//!
//! ```json
//! {
//!   "id": "uuid-v4-string",
//!   "title": "<string>",
//!   "slug": "<[a-z0-9-]+>",
//!   "status": "planning",
//!   "created_at": "<rfc3339>",
//!   "updated_at": "<rfc3339>",
//!   "parent": null,
//!   "summary": "",
//!   "items": []
//! }
//! ```
//!
//! `parent` is reserved for B8 DAG (Phase 3+); nullable now.
//! `summary` is the user-provided one-liner; Phase 1's
//! `wf-brainstorm` skill feeds it. Empty in v1 is fine.
//!
//! ## Phase scope
//!
//! - **Step 0.4**: types + read/write + `create_task` template fill +
//!   dir layout. NO `update_task` IPC yet — Phase 2 Step 2.6
//!   (B12 checklist sync) owns the writer path for `items`.
//! - **Step 2.6**: `update_task(task_dir, items)` writes via
//!   `write_task_partial(...).items = ...` to keep serde
//!   semantics centralized.
//!
//! ## Validation
//!
//! `slug` is constrained to `[a-z0-9-]{1,64}` ASCII.
//! Bad slugs are rejected as `Err(TaskError::InvalidSlug)` —
//! the IPC surfaces this as `AppCommandError::InvalidRequest`
//! (`commands/task.rs` wrapper). The constraint exists to
//! keep `.everlasting/tasks/<slug>/` paths portable across
//! case-insensitive filesystems (NTFS / HFS+ / APFS) and
//! shell-safe.
//!
//! Hub for the `task/` directory module (split 2026-08-08 batch3). The
//! type definitions, slug/path helpers, IO, and archive logic live in
//! the submodules below; this hub re-exports the public surface so the
//! existing `agent/workflow/mod.rs` `pub use task::{...}` and all
//! `crate::agent::workflow::task::*` callers keep resolving unchanged.

mod archive;
mod io;
mod paths;
mod types;

#[allow(unused_imports)]
pub use archive::{archive_task_init, PROJ_NS_TASKS_ARCHIVE_DIR};
#[allow(unused_imports)]
pub use io::{create_task_init, read_task, write_task};
#[allow(unused_imports)]
pub use paths::{task_dir, task_json_path, task_prd_path, validate_slug};
#[allow(unused_imports)]
pub use types::{TaskError, TaskItem, TaskJson, TaskResult, TaskStatus};
