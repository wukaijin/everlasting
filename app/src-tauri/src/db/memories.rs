//! P1 (autonomous memory, 2026-06-29): storage layer for the
//! agent's self-produced, cross-session recalled experience memory.
//!
//! See `.trellis/tasks/06-29-am-p1-storage/prd.md` for the full
//! spec + spike-007 §5 for the design lineage. This module is the
//! data-layer foundation P2 (read/write closed loop) / P3 (pre-tool
//! pitfall recall) / P4 (event-driven reflection write) / P5
//! (status machine + hygiene job) build on.
//!
//! This file is the module hub: implementations live in the
//! `memories/` submodules (`types`, `validation`, `crud`, `search`,
//! `lifecycle`), re-exported here so `db::memories::X` call sites
//! (including the external `memories_tests.rs`) are unchanged after
//! the 08-08 split.
//!
//! # Dead-code policy
//!
//! `#![allow(dead_code)]` is set at the module level. This is a
//! **storage底座 (foundation) task** — P1 lands the table + CRUD +
//! write safety net + unit tests, but **no production caller wires
//! any function yet**. P2 (remember tool + recall injection) is the
//! first production consumer; P3 / P4 / P5 follow. Every `pub` item
//! in this module (3 enums + 2 structs + 7 CRUD fns + 2 error enums
//! + the safety-net helpers) is forward-compat storage with zero
//! current callers — ~25 dead-code warnings would fire otherwise.
//!
//! **Deviation from the `subagent_runs.rs` precedent** (which uses
//! per-item `#[allow(dead_code)]`): that module's PR2 landed the
//! table + CRUD **alongside** production callers in B6 PR1's
//! `dispatch.rs`, so only ~8 items (UI-read shapes, a `get_run`
//! helper, an unused flag) lacked callers and got per-item allows.
//! `memories.rs` is a pure-foundation task — **every** public
//! symbol is unused until P2, so per-item allows would be noisy
//! (~25 annotations) without adding precision over the module form.
//! The trade-off accepted here: the module-level allow could mask a
//! future drift (typo'd helper, refactor-orphaned fn), but P2 is
//! the immediate next task and will surface any orphan when it
//! becomes the first caller. When P2 lands, **replace this
//! module-level allow with per-item allows** on whatever P2 still
//! doesn't consume (mirroring `subagent_runs.rs`).

#![allow(dead_code)]

mod crud;
mod lifecycle;
mod search;
mod types;
mod validation;

// Re-export the full public surface so `db::memories::*` paths stay
// stable. The external `memories_tests.rs` (2241 lines) references
// types/functions by full path (`crate::db::memories::MemoryRow`) —
// these re-exports keep those paths working. `#[allow(unused_imports)]`
// mirrors the wire/mod.rs convention for binary-crate re-exports.
#[allow(unused_imports)]
pub use crud::{
    count_memories_by_scope_kind, count_memories_for_session, delete_memory, get_memory_by_id,
    insert_memory, list_memories,
};
#[allow(unused_imports)]
pub use lifecycle::{
    bump_hit_count, promote_if_eligible, update_memory, update_status, StatusTransitionError,
    ACTIVE_TO_VERIFIED_AGE_DAYS, ACTIVE_TO_VERIFIED_AT, CANDIDATE_TO_ACTIVE_AT,
};
#[allow(unused_imports)]
pub(crate) use search::build_recall_fts_query;
#[allow(unused_imports)]
pub use search::{
    find_pitfalls_by_trigger, find_pitfalls_by_trigger_all_status, search_memories_fts,
    search_memories_fts_recall, RecallStatusFilter,
};
#[allow(unused_imports)]
pub use types::{
    MemoryInput, MemoryInsertError, MemoryKind, MemoryRow, MemoryScope, MemoryStatus,
    MemoryUpdateError, MAX_CONTENT_LEN, MAX_TITLE_LEN,
};
#[allow(unused_imports)]
pub use validation::validate_memory_text;

// Re-export the test_helpers submodule (cfg(test)) so memories_tests
// can reach `insert_raw`. The submodule is declared in lifecycle.rs.
#[cfg(test)]
pub(crate) use lifecycle::test_helpers;
