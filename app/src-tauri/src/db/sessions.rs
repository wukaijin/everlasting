//! Session CRUD + worktree-state transitions + message persistence.
//!
//! Each session is one conversation scoped to a project. The
//! `current_cwd` column tracks the directory the agent is operating
//! in; tools fall back to it when `worktree_path` is `None`. The
//! `worktree_state` tri-valued enum tracks whether the session has a
//! live worktree bound (`Active`), previously had one (`Detached`),
//! or never did (`None`).
//!
//! Hub for the `sessions/` directory module (split 2026-08-08 batch3).
//! Session CRUD + worktree-state transitions live in [`session_crud`];
//! message persistence (`persist_turn`, latency, metadata, edit) lives
//! in [`messages`]. This hub re-exports the public surface so the
//! existing `db/mod.rs` `pub use sessions::*` and all
//! `crate::db::sessions::*` callers keep resolving unchanged.

pub mod messages;
pub mod session_crud;

#[allow(unused_imports)]
pub use messages::{
    delete_in_progress_turn, edit_user_message, finalize_turn_persist, find_message_id_by_seq,
    persist_turn, record_tool_duration, recover_interrupted_messages, update_message_latency,
    update_message_metadata, upsert_in_progress_turn, MessageLatency, RecoveryReport,
};
#[allow(unused_imports)]
pub use session_crud::{
    create_session, delete_messages_by_session, delete_session, insert_compaction_summary,
    insert_system_event, list_sessions, load_session, rename_session, session_exists,
    set_session_color, set_session_metadata, set_session_plugin_name, set_session_workflow_enabled,
    set_worktree_state, touch_session, update_last_turn_usage, update_session_cwd,
    update_session_model_id,
};
