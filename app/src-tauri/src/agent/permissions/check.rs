//! ⑨ 关 entry point — 5-tier `check` + Tier 4 helpers (classify,
//! extract_path_arg, grant checks, sqlite_glob_match,
//! match_value_for_allow_always). Split out of `mod.rs` on
//! 2026-06-23.
//!
//! Hub for the `check/` directory module (split 2026-08-08 batch3).
//! The 5-tier `check` + Tier 4 helpers live in [`permission`];
//! the Tier 1 Hooks pre-tool pitfall recall + P5 tiered recall live
//! in [`pitfall`]. This hub re-exports the public + `pub(crate)`
//! surface so `permissions/mod.rs`'s `pub use check::{...}`,
//! `ask.rs`'s `super::check::match_value_for_allow_always`, and
//! `tests_check.rs`'s 8-symbol import all keep resolving unchanged.

pub mod permission;
pub mod pitfall;

#[allow(unused_imports)]
pub use permission::check;
#[allow(unused_imports)]
pub(crate) use permission::{
    classify_tool, extract_path_arg, match_value_for_allow_always, sqlite_glob_match, ToolKind,
};
#[allow(unused_imports)]
pub use pitfall::{
    recall_pitfall, recall_pitfall_footnote, recall_pitfall_with_hits, PitfallRecall,
};
