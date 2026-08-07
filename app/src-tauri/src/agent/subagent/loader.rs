//! L3d Subagent frontmatter loader — mtime-fenced scan of
//! user/project agent dirs.
//!
//! Mirrors the B3 `resource_loader` (commands) + B4 `skill/loader`
//! (skills) shape: read-through mtime fence, hand-rolled frontmatter
//! parser (scalar + single-line inline array, same YAGNI parser
//! philosophy — no `serde_yaml` dependency), precedence merge.
//!
//! Precedence (high → low): **project > user > builtin**. A user/
//! project `.md` whose `name` collides with a builtin **fully
//! overrides** the builtin (last-write-wins on a `HashMap` insert).
//! No reload command — freshness is decided at read time by the mtime
//! fence (Q1 decision, replacing the design PRD §7.2 `/reload-subagents`
//! command). Adding / editing / deleting a `.md` is picked up on the
//! next chat turn that calls `SubagentCache::list`.
//!
//! ## tools inheritance (Q2)
//!
//! `tools` is an **optional** frontmatter field. The parser keeps it
//! as `Option<Vec<String>>` so we can distinguish "not declared"
//! (`None`) from "declared empty" (`Some(vec![])`):
//! - When a `.md` overrides a same-named lower layer (builtin or
//!   user) and **does not declare `tools`** → inherit the lower
//!   layer's `def.tools` (so "only change the system prompt" costs
//!   nothing — the user does not need to copy the builtin tool list).
//! - When a `.md` declares `tools` (even `[]`) → use the declared
//!   list verbatim. `[]` follows the `general-purpose` convention
//!   ("empty = full set minus structural-disabled").
//! - A brand-new agent (no lower-layer collision) with no `tools`
//!   declaration → `vec![]` (full set minus structural-disabled).
//!
//! The inheritance is resolved during the precedence merge (low →
//! high insertion into the by-name map); see `merge_with_inheritance`.
//!
//! ## Per-file isolation
//!
//! A single bad `.md` (over-cap, non-UTF-8, missing `name`, illegal
//! `name` characters, malformed frontmatter) is skipped with a
//! `tracing::warn!` and never aborts the whole scan. Builtins are
//! always present regardless of `.md` failures (they come from
//! `builtin_subagents()`, an in-memory `&'static`).
//!
//! ## What this module does NOT do
//!
//! - Does NOT change `dispatch_subagent`'s `definition()` enum (still
//!   hardcoded `["researcher", "general-purpose"]` — PR3 adds the
//!   parallel `definition_with_cache` for the dynamic path but keeps
//!   the static `definition()` for unit tests).
//! - Does NOT change `dispatch.rs::run_subagent`'s `lookup_subagent`
//!   call site (PR3 does, threading `subagent_cache` through).
//! - Does NOT add `SubagentCache` to `AppState` (PR3).
//!
//! PR3 lights up all three; this module is the pure infrastructure +
//! unit tests that PR3 wires in.

use std::collections::HashMap;

use super::cache::LoadedAgentFile;
use crate::agent::subagent::SubagentDef;

/// Where a subagent definition came from. `Plugin` overrides
/// `BuiltinPlugin` (which overrides `Project` and `User` and
/// `Builtin`) on a name collision.
///
/// Step 2.3 (`07-08-workflow-integration`): added `Plugin`
/// for `<project>/.everlasting/workflow/<wf>/agents/`. The
/// plugin layer is the highest-priority one when present —
/// workflow session agents can fully customize the worker
/// system prompt + tools by dropping a `.md` file alongside
/// the plugin's `workflow.json`. Non-workflow callers
/// (using the existing `list` / `lookup` methods) never
/// see the plugin layer.
///
/// 07-09-workflow-builtin-plugin: added `BuiltinPlugin` for
/// app-bundled `include_str!` constants. Priority
/// `Plugin > BuiltinPlugin > Project > User > Builtin`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentSource {
    Builtin,
    User,
    Project,
    Plugin,
    /// app 内置 plugin agent(`include_str!` 常量)。
    /// 07-09-workflow-builtin-plugin: 编译期常量,
    /// workflow session 在项目 plugin 缺失时回退到这里。
    BuiltinPlugin,
}

impl SubagentSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::User => "user",
            Self::Project => "project",
            Self::Plugin => "plugin",
            Self::BuiltinPlugin => "builtin-plugin",
        }
    }
}

/// A loaded subagent definition + its source layer. Returned by
/// `SubagentCache::list` / `lookup` to callers (PR3 will use the
/// source tag to render `Available subagents: name (source: ...)`
/// in the dispatch_subagent tool description).
#[derive(Clone, Debug)]
pub struct LoadedSubagent {
    pub def: SubagentDef,
    pub source: SubagentSource,
}

// ---------------------------------------------------------------------------
// 2026-07-03 (task 07-03-subagent-per-agent-model-ui, 阶段 2): line-level
// frontmatter editor + IO wrapper
//
// The Settings UI's "set model" affordance writes the user's
// per-agent `model:` value back to the agent's `.md` file (for
// user / project agents; builtin agents go to the DB override
// table instead, see `subagent_overrides` + `set_subagent_model`
// IPC). Rather than introducing a YAML crate, the writer does
// LINE-LEVEL EDITS: it reads the file, locates the frontmatter
// block, and rewrites only the `model:` line. Body, comments,
// and other frontmatter keys are preserved verbatim.
// ---------------------------------------------------------------------------

/// Apply a `model:` line edit to an agent's frontmatter text.
/// Pure (no IO) so the line-level logic is unit-testable without
/// touching the filesystem.
///
/// Behavior:
/// - **`Some(mid)`** (set or replace `model`):
///   - If a `model:` line already exists inside the frontmatter
///     block, replace it with the new value.
///   - If `model:` is absent, insert a new `model: <id>` line as
///     the first line INSIDE the frontmatter block (right after
///     the opening `---` fence).
/// - **`None`** (clear `model`):
///   - If a `model:` line exists, delete it (along with its
///     trailing newline so the file doesn't gain a blank line).
///   - If `model:` is absent, the file is unchanged.
///
/// **Frontmatter detection**: the function looks for the first
/// `---\n` ... `---\n` block. If the input has no opening `---`
/// fence, the function returns `Err(String)` — a `.md` agent
/// without frontmatter is broken (the loader would have rejected
/// it for missing `name`); silently adding a fence would rewrite
/// the file structure in a way the user didn't ask for. The IPC
/// layer surfaces this as an `InvalidRequest` error.
///
/// **Preserves**: line ordering for all other frontmatter keys,
/// the body section, in-line `#` comments, and surrounding
/// whitespace (we only touch the targeted line; no
/// re-serialization of the rest).
pub fn apply_model_line(content: &str, model_id: Option<&str>) -> Result<String, String> {
    let lines: Vec<&str> = content.lines().collect();

    // Locate the opening `---` fence. Skip leading blank lines
    // so a file that starts with a blank line is still recognized.
    let mut open_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "---" {
            open_idx = Some(i);
            break;
        }
        if !line.trim().is_empty() {
            // Non-blank, non-fence line before any `---` → no
            // frontmatter. Reject (don't auto-insert a fence —
            // see doc comment).
            return Err(format!(
                "agent file has no frontmatter fence (first non-blank line is `{}`)",
                line
            ));
        }
    }
    let open_idx = match open_idx {
        Some(i) => i,
        None => return Err("agent file has no frontmatter (no opening `---`)".to_string()),
    };

    // Find the closing `---` fence (search from `open_idx + 1`).
    let mut close_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate().skip(open_idx + 1) {
        if line.trim() == "---" {
            close_idx = Some(i);
            break;
        }
    }
    let close_idx = match close_idx {
        Some(i) => i,
        None => {
            return Err("agent file has unterminated frontmatter (no closing `---`)".to_string())
        }
    };

    // Scan frontmatter lines for an existing `model:` entry.
    // The check is a `starts_with` match against the trimmed line
    // (the inline whitespace + the colon; values can have any
    // shape). We capture the index so we can replace or remove
    // in place; the "remove" path also drops the trailing
    // newline so the file doesn't gain a blank line.
    let mut existing_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate().take(close_idx).skip(open_idx + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("model:") || trimmed.starts_with("model :") {
            existing_idx = Some(i);
            break;
        }
    }

    let mut new_lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();

    match (model_id, existing_idx) {
        // Set, no existing → insert as the FIRST line inside the
        // fence (right after the opening `---`). The line goes at
        // `open_idx + 1`, shifting the close_idx (and everything
        // after) by 1.
        (Some(mid), None) => {
            new_lines.insert(open_idx + 1, format!("model: {mid}"));
        }
        // Set, existing → replace in place. The trimmed line is
        // matched (model: or model :) for compatibility with the
        // tolerant parser in `apply_kv`; the new line uses the
        // canonical single-space form.
        (Some(mid), Some(idx)) => {
            new_lines[idx] = format!("model: {mid}");
        }
        // Clear, existing → drop the line. The trailing
        // newline is implicit (Vec<String> line-by-line →
        // `join("\n")`); the file's bytes are unchanged in the
        // resulting shape (a single line removal from the fence
        // interior is the desired contract).
        (None, Some(idx)) => {
            new_lines.remove(idx);
        }
        // Clear, no existing → no-op (return input verbatim).
        (None, None) => {}
    }

    // Re-attach the original line ending. `content.lines()` drops
    // a single trailing newline (Rust's `Lines` iterator is
    // documented to yield "as if the string ends with a final
    // line terminator"); the round-trip must preserve it so the
    // `write_frontmatter_model` no-op short-circuit
    // (`updated == content`) doesn't fire a spurious mtime
    // update on a no-op.
    let mut out = new_lines.join("\n");
    if content.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// IO wrapper around [`apply_model_line`]: read the file, apply
/// the line edit, write back atomically (`.tmp` + rename) so a
/// mid-write crash doesn't leave the agent file half-edited.
///
/// Errors:
/// - File missing → `Err(io::Error::NotFound)`.
/// - File present but malformed (no frontmatter / unterminated
///   fence / no `name` field) → `Err(io::Error::InvalidData)`
///   with the parser's message.
/// - IO error on the read or write path → `Err(io::Error)` (the
///   atomic write uses `.tmp` + `rename`, so partial writes don't
///   corrupt the agent file).
pub fn write_frontmatter_model(
    path: &std::path::Path,
    model_id: Option<&str>,
) -> std::io::Result<()> {
    let content = std::fs::read_to_string(path)?;
    let updated = apply_model_line(&content, model_id)
        .map_err(|msg| std::io::Error::new(std::io::ErrorKind::InvalidData, msg))?;
    // No-op short-circuit: if the line edit produced identical
    // bytes, skip the write. Avoids touching the file's mtime
    // unnecessarily (which would invalidate the loader's mtime
    // cache on a no-op round-trip).
    if updated == content {
        return Ok(());
    }
    // Atomic write: temp + rename. Same pattern as the rest of
    // the project (see `files.rs` for analogous usage).
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, updated.as_bytes())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub(crate) fn merge_with_inheritance(layers: Vec<Vec<LoadedAgentFile>>) -> Vec<LoadedSubagent> {
    let mut by_name: HashMap<String, LoadedAgentFile> = HashMap::new();
    for layer in layers {
        for file in layer {
            let name = file.loaded.def.name.clone();
            let mut merged = file.loaded.clone();
            let mut tools_declared = file.tools_declared;
            let mut isolation_declared = file.isolation_declared;
            // Inherit from the lower layer for any field this layer
            // did NOT declare.
            if let Some(lower) = by_name.get(&name) {
                if !file.tools_declared {
                    merged.def.tools = lower.loaded.def.tools.clone();
                    tools_declared = false; // stays inheritable for the next layer
                }
                if !file.isolation_declared {
                    merged.def.isolation = lower.loaded.def.isolation;
                    isolation_declared = false;
                }
            }
            by_name.insert(
                name,
                LoadedAgentFile {
                    loaded: merged,
                    tools_declared,
                    isolation_declared,
                },
            );
        }
    }
    let mut out: Vec<LoadedSubagent> = by_name.into_values().map(|f| f.loaded).collect();
    out.sort_by(|a, b| a.def.name.cmp(&b.def.name));
    out
}
