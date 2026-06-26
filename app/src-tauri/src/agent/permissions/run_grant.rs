//! Per-run grant cache for worker subagents (task
//! `06-26-subagent-per-run-grant`, 2026-06-26).
//!
//! A worker (dispatched via `dispatch_subagent`) shares its parent
//! session's id but must NOT pollute the parent's
//! `session_tool_permissions` table when the user clicks "always
//! allow" on a worker `permission:ask`. The fix is an in-memory
//! grant cache scoped to the worker's run: it lives as long as the
//! worker's `Arc<RunGrantCache>` handle is alive (= the worker's
//! `run_chat_loop` call), and dies when the worker exits.
//!
//! ## Data model
//!
//! The cache mirrors the three `match_kind` variants of
//! `session_tool_permissions`, just with storage = memory and
//! scope = single run:
//!
//! - `tool`   — whole-tool grant (e.g. `web_fetch`, value `None`)
//! - `prefix` — shell first-token grant (e.g. `cargo`, value = first token)
//! - `path`   — repo-outside path glob (e.g. `/tmp/notes/*`, value = glob)
//!
//! ## Write path
//!
//! `grant_for_run` reuses `check::match_value_for_allow_always` to
//! compute `(kind, value)` for a given `(tool_name, tool_input,
//! path_or_cmd)` — exactly the same rule the parent path uses to
//! persist a `session_tool_permissions` row. This guarantees
//! identical matching semantics across the run cache and the DB
//! table (single source of truth for the rule).
//!
//! ## Read path
//!
//! `has_run_grant` mirrors the DB query semantics of
//! `check::check_tool_grant` / `check::check_prefix_grant` /
//! `check::check_path_grant` (run cache = in-memory image of the
//! same semantics):
//!
//! - `tool`   — `tool_name` exact equality (value field ignored)
//! - `prefix` — `match_value == candidate` (candidate = shell first token)
//! - `path`   — `sqlite_glob_match(match_value, candidate)` for each row
//!   (candidate = absolute path being checked)
//!
//! ## Concurrency
//!
//! Workers execute turns serially (one tool_use → one tool_result at
//! a time per worker), so the cache is logically single-writer. The
//! `Mutex` is still there as a defensive guard; performance is a
//! non-issue because the cache is read at most a few times per turn.
//!
//! ## Isolation
//!
//! Each worker `run_chat_loop` invocation gets its own
//! `Arc<RunGrantCache>` (constructed in `run_subagent`), so
//! concurrent workers (L3a) have isolated caches — a grant on one
//! worker's `cargo` does NOT silently authorize another worker's
//! `cargo`. The parent path (`run_grants: None`) never touches the
//! cache, preserving the existing session-level "always allow"
//! behavior unchanged.

use std::sync::{Arc, Mutex};

use super::check::{match_value_for_allow_always, sqlite_glob_match};

/// A single in-memory grant row. Mirrors the three `match_kind`
/// variants of `session_tool_permissions` (tool / prefix / path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunGrant {
    pub tool_name: String,
    /// `"tool"` / `"prefix"` / `"path"` — same wire values as
    /// `session_tool_permissions.match_kind`.
    pub match_kind: String,
    /// `None` for `tool` match_kind (whole-tool grant); the
    /// first-token string for `prefix`; the glob string for `path`.
    pub match_value: Option<String>,
}

/// Per-run in-memory grant cache. Constructed once per worker
/// `run_chat_loop` invocation; dies when the `Arc` handle is dropped
/// (always at worker exit).
///
/// Cheap to clone (`Arc<Mutex<Vec<...>>>`). The
/// `PermissionContext.run_grants` field carries an
/// `Option<Arc<RunGrantCache>>`; `None` = parent path (do not read,
/// do not write).
#[derive(Clone, Default, Debug)]
pub struct RunGrantCache {
    grants: Arc<Mutex<Vec<RunGrant>>>,
}

impl RunGrantCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a grant for the current run. Computes the
    /// `(match_kind, match_value)` pair using the SAME rule as the
    /// parent path's `session_tool_permissions` persistence
    /// (`check::match_value_for_allow_always`) — identical
    /// semantics, in-memory storage. Called from the worker branch
    /// of `ask_path` when the user clicks "always allow" on a worker
    /// ask.
    ///
    /// `path_or_cmd` is the same string the parent path passes to
    /// `match_value_for_allow_always` — the full path / command /
    /// URL the user is approving.
    pub fn grant_for_run(
        &self,
        tool_name: &str,
        tool_input: &serde_json::Value,
        path_or_cmd: &str,
    ) {
        let (kind, value) = match_value_for_allow_always(tool_name, tool_input, path_or_cmd);
        let grant = RunGrant {
            tool_name: tool_name.to_string(),
            match_kind: kind.to_string(),
            match_value: value,
        };
        let mut grants = self
            .grants
            .lock()
            .expect("RunGrantCache mutex poisoned (grant_for_run)");
        // Defensive: avoid pushing a byte-equal duplicate. Cheap
        // because the cache is per-run and bounded by the number of
        // distinct tools × kinds a worker exercises (typically < 10).
        if !grants.iter().any(|g| {
            g.tool_name == grant.tool_name
                && g.match_kind == grant.match_kind
                && g.match_value == grant.match_value
        }) {
            grants.push(grant);
        }
    }

    /// Query whether this run cache holds a grant matching the given
    /// `(tool_name, kind, candidate)`. Used by `check.rs` Tier 4
    /// three branches before falling through to `ask_path`.
    ///
    /// `kind` selects the matching rule:
    /// - `"tool"`   — `tool_name` equality (candidate ignored)
    /// - `"prefix"` — `match_value == candidate` (exact-eq; candidate
    ///   is the shell's first token)
    /// - `"path"`   — `sqlite_glob_match(match_value, candidate)`
    ///   for each path-kind row; candidate is the absolute path
    ///   being checked
    ///
    /// Returns `false` for unknown kinds (defensive — no fallthrough
    /// to a permissive default).
    pub fn has_run_grant(&self, tool_name: &str, kind: &str, candidate: &str) -> bool {
        let grants = match self.grants.lock() {
            Ok(g) => g,
            // Mutex poisoned: fail CLOSED (deny) — never silently
            // grant permission when the cache state is suspect.
            // Matches the project's "fail-safe on permission grant"
            // posture.
            Err(_) => return false,
        };
        match kind {
            "tool" => grants.iter().any(|g| {
                g.match_kind == "tool" && g.tool_name == tool_name
            }),
            "prefix" => {
                if candidate.is_empty() {
                    return false;
                }
                grants.iter().any(|g| {
                    g.match_kind == "prefix"
                        && g.tool_name == tool_name
                        && g.match_value.as_deref() == Some(candidate)
                })
            }
            "path" => {
                // Candidate is the absolute path being checked. Match
                // each path-kind row's glob against it using the SAME
                // matcher `check::check_path_grant` uses (sqlite GLOB
                // semantics — `*` does not cross `/`).
                grants.iter().any(|g| {
                    if g.match_kind != "path" || g.tool_name != tool_name {
                        return false;
                    }
                    match g.match_value.as_deref() {
                        Some(glob) => sqlite_glob_match(glob, candidate),
                        None => false,
                    }
                })
            }
            _ => false,
        }
    }

    /// Number of grants currently held. Test-only helper for
    /// asserting "AllowOnce did not write a grant" / "AllowAlways
    /// wrote exactly one grant".
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.grants
            .lock()
            .map(|g| g.len())
            .unwrap_or(0)
    }

    /// Whether the cache currently holds any grant. Test-only.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
