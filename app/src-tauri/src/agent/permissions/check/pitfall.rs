//! Tier 1 Hooks: pre-tool pitfall recall (P3, 2026-06-29) +
//! P5 tiered recall. Split out of `agent/permissions/check.rs`
//! (2026-08-08 batch3).

use std::collections::HashSet;

use sqlx::SqlitePool;

use super::permission::{classify_tool, extract_path_arg, ToolKind};

// ---------------------------------------------------------------------------
// Tier 1 Hooks: pre-tool pitfall recall (P3, 2026-06-29)
// ---------------------------------------------------------------------------

/// Pre-tool pitfall recall — the Tier 1 Hooks side of the ⑨ layer.
///
/// **Scope**: hooks the `permissions/check.rs` Tier 1 site (currently
/// no-op per the 5-tier design) with a `find_pitfalls_by_trigger`
/// probe. When `active` pitfalls match the current `(tool_name,
/// tool_input)`, the function builds a footnote string that the
/// chat loop prepends to the `tool_result.content`. The tool
/// execution itself is NEVER blocked (this is the **active 注脚**
/// tier per spike-007 §4 + P3 PRD; the verified soft-intercept tier
/// is OUT OF SCOPE here — `P5`).
///
/// **Why separate from `check()`**: `check()` returns a `Decision`
/// (Allow/Deny/resolved-Ask); injecting a "soft footnote" would
/// pollute that contract (Deny is silent, Ask goes through
/// oneshot, neither carries text). The chat loop already has a
/// clear "after check returns Allow, before execute_tool" seam,
/// so the recall runs there as its own pure-data step. This keeps
/// `check()` 5-tier-pure and the loop structure untouched (PRD
/// hard rule).
///
/// **Behavior contract** (locked by P3 acceptance criteria):
/// 1. Resolves `(command_pattern, path)` from `tool_input` based
///    on tool kind (Path → `path`/`cwd`/`working_directory`;
///    Shell → `command`; WebFetch → `url`; other → `(None, None)`).
/// 2. Calls `db::memories::find_pitfalls_by_trigger` — exact-match
///    `tool_name`, substring `command_pattern` (when supplied by
///    the caller), `path_globs` glob match (when supplied).
/// 3. Filters to `status == 'active'` rows only (verified soft-
///    intercept is P5 scope; see spike-007 §4 tier table).
/// 4. Builds a multi-line footnote: one bullet per matching pitfall
///    with title + content. Token budget is loose (P3 doesn't cap;
///    P5 will re-derive alongside the verified soft-intercept).
/// 5. Fires `db::memories::bump_hit_count` per hit, fire-and-forget
///    on a `tokio::spawn` so the recall step stays sync-fast
///    (matches the audit-write pattern: best-effort metadata, never
///    blocks the hot path).
/// 6. Any DB error → `tracing::warn!` + return `Ok(None)` (recall
///    failure MUST NOT block tool execution; PRD acceptance
///    criterion).
///
/// **Returns**: `Ok(Some(footnote))` on active hit, `Ok(None)` on
/// miss / DB-error / out-of-scope. The chat loop prepends the
/// footnote to `content` before envelope wrapping. The footnote is
/// deliberately plain text (no `cache_control`, no Anthropic
/// metadata) — it travels inside the tool_result `content` string
/// alongside the tool's normal output.
///
/// **Wire / cross-layer**: this function is called from
/// `agent/chat_loop.rs` between `permissions::check` returning
/// `Allow` and `execute_tool`. It does NOT mutate the agent loop
/// state machine, does NOT touch cancel/audit maps, does NOT alter
/// the persisted message history (the tool_result already travels
/// through the normal path).
//
// P5 (2026-06-29): production now routes through `recall_pitfall`
// (tiered). This function is retained as the **Footnote tier**
// reference + for the P3-era test suite
// (`recall_pitfall_footnote_*` tests in `tests_check.rs`). It's the
// stable shape a future caller wanting "active-only footnote, no
// soft-block semantics" would reach for.
#[allow(dead_code)]
pub async fn recall_pitfall_footnote(
    db: &SqlitePool,
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> Result<Option<String>, sqlx::Error> {
    // Step 1: extract the relevant probe string from tool_input.
    let (command_pattern, path) = extract_probe_args(tool_name, tool_input);

    // Step 2: probe find_pitfalls_by_trigger.
    let rows = crate::db::memories::find_pitfalls_by_trigger(
        db,
        tool_name,
        command_pattern.as_deref(),
        path.as_deref(),
    )
    .await?;

    // Step 3: filter to active rows only (verified → P5).
    let active_rows: Vec<_> = rows.into_iter().filter(|r| r.status == "active").collect();

    if active_rows.is_empty() {
        return Ok(None);
    }

    // Step 5: bump_hit_count fire-and-forget per hit (best-effort,
    // never blocks the recall step).
    for row in &active_rows {
        let pool = db.clone();
        let mid = row.memory_id.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::db::memories::bump_hit_count(&pool, &mid).await {
                tracing::warn!(
                    memory_id = %mid,
                    error = %e,
                    "recall_pitfall_footnote: bump_hit_count failed (non-fatal)"
                );
            }
        });
    }

    // Step 4: build the multi-line footnote. Imperative, pitfall-
    // style phrasing per spike-007 §4 "active 注脚" tier (the
    // soft hint that doesn't interrupt execution).
    let mut out = String::from("⚠️ Memory: 此前在本项目执行类似操作时踩过坑 —\n");
    for row in &active_rows {
        // Title + content, one pitfall per line. Use the bullet
        // marker `•` so the LLM can pick the relevant one out of
        // multiple hits without losing alignment.
        out.push_str(&format!("• [{}] {}\n", row.title, row.content));
    }
    Ok(Some(out))
}

// ---------------------------------------------------------------------------
// P5 (2026-06-29, 06-29-am-p5-quality): tiered pre-tool pitfall recall
// ---------------------------------------------------------------------------

/// P5 tiered pitfall recall outcome. Supersedes [`recall_pitfall_footnote`]
/// for the soft-block tier; the legacy footnote tier is preserved as
/// [`PitfallRecall::Footnote`].
///
/// **Design §4 + D1**: a `verified` pitfall whose trigger key
/// **fully** matches the current `(tool_name, tool_input)` and that
/// has not yet soft-blocked this session produces
/// [`PitfallRecall::SoftBlock`]. The chat loop short-circuits
/// `execute_tool`, surfaces the hint as an `is_error: false`
/// `tool_result`, and records the `memory_id` in the session-scoped
/// `HashSet` so the same pitfall's next hit degrades to
/// [`PitfallRecall::Footnote`] + normal execution (the dead-loop
/// guard, D1).
///
/// All other hits (active / candidate / partial-match / second-hit
/// on an already-soft-blocked pitfall) produce
/// [`PitfallRecall::Footnote`] — the same behavior as P3's
/// [`recall_pitfall_footnote`] (the tool executes, the hint is
/// prepended to the result content). Misses produce [`PitfallRecall::None`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PitfallRecall {
    /// No pitfall matched (or DB error — recall never blocks).
    None,
    /// Soft hint: tool executes normally; this text is prepended to
    /// the `tool_result.content` (preserving the existing envelope
    /// shape). Carried by active / candidate / partial-match /
    /// second-hit-on-same-pitfall rows.
    Footnote(String),
    /// Verified + full trigger-key match + not yet soft-blocked this
    /// session. The chat loop short-circuits `execute_tool` and
    /// surfaces `hint` as an `is_error: false` tool_result. The
    /// `memory_id` is what the loop records in its session-scoped
    /// `HashSet<String>` so the next hit on the same pitfall
    /// degrades to [`PitfallRecall::Footnote`] (D1).
    SoftBlock { hint: String, memory_id: String },
}

impl PitfallRecall {
    /// Convenience: coerce into the P3-style `Option<String>` (the
    /// footnote text). `SoftBlock` returns `None` here — the chat
    /// loop handles SoftBlock via its own short-circuit branch, not
    /// the prepend-after-execute path. Kept for any caller that
    /// still wants the legacy shape.
    #[allow(dead_code)]
    pub fn into_footnote(self) -> Option<String> {
        match self {
            Self::Footnote(s) => Some(s),
            Self::None | Self::SoftBlock { .. } => None,
        }
    }
}

/// Master feature switch for the soft-block tier (design §7). When
/// `false`, [`recall_pitfall`] never returns [`PitfallRecall::SoftBlock`]
/// — every hit (including verified + full-match) degrades to
/// [`PitfallRecall::Footnote`] (i.e. the P3 behavior). Roll-back
/// lever for the soft-block tier without rippling through every
/// caller.
pub const PITFALL_SOFT_BLOCK_ENABLED: bool = true;

/// P5 tiered pre-tool pitfall recall. Replaces [`recall_pitfall_footnote`]
/// at the chat_loop call sites (parallel + serial paths). The tiering
/// (design §4 + D1):
///
/// 1. Probe [`crate::db::memories::find_pitfalls_by_trigger_all_status`]
///    (candidate + active + verified; design §3 widened the filter
///    so candidate pitfalls can be recalled + bumped + promoted).
/// 2. For each hit, classify by `(status, full-match, already-blocked)`:
///    - **`verified` + full match + NOT in `already_blocked`** →
///      [`PitfallRecall::SoftBlock`]. Exactly one SoftBlock wins per
///      call (the first qualifying hit); the loop records its
///      `memory_id` so subsequent calls for the same pitfall degrade.
///    - everything else (active / candidate / partial match / second
///      hit on an already-blocked verified pitfall) → folded into a
///      single multi-bullet [`PitfallRecall::Footnote`] (the P3
///      behavior, unchanged shape).
/// 3. `bump_hit_count` is fired per hit (best-effort spawn; same as
///    P3). The auto-promotion hook in `bump_hit_count` will pick up
///    threshold crosses (P5 Step 2).
/// 4. DB error → `warn!` + return [`PitfallRecall::None`] (recall
///    never blocks tool execution; same hard rule as P3).
///
/// **`full_match` definition**: the hit's `tool_name` matches the
/// probe (always true — the SQL filters on it), AND its
/// `command_pattern` is `Some(_)` and the probe's `command_pattern`
/// contains it, AND its `path_globs` is `Some(_)` and the probe's
/// `path` matches at least one glob. A pitfall with `command_pattern
/// = NULL` and `path_globs = NULL` is **path/command-agnostic** and
/// does NOT count as a full match for soft-block purposes (it would
/// soft-block too broadly — e.g. "always pass --offline to cargo"
/// would soft-block any cargo invocation).
///
/// 07-06 (am-observability-panel A9): production now routes
/// through [`recall_pitfall_with_hits`] (the R2b recall-event emit
/// site needs the rows). This wrapper is retained for the P3-era
/// tests in `tests_check.rs` (the 7 `p5_recall_*` cases call it
/// directly); `#[allow(dead_code)]` mirrors
/// [`recall_pitfall_footnote`]'s annotation (lib build doesn't
/// see the test consumers).
#[allow(dead_code)]
pub async fn recall_pitfall(
    db: &SqlitePool,
    tool_name: &str,
    tool_input: &serde_json::Value,
    already_blocked: &HashSet<String>,
) -> PitfallRecall {
    recall_pitfall_inner(db, tool_name, tool_input, already_blocked)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(
                error = %e,
                tool = tool_name,
                "recall_pitfall: DB error (non-fatal, returning None)"
            );
            PitfallRecall::None
        })
}

/// 07-06 (am-observability-panel D4 + A9): sibling of
/// [`recall_pitfall`] that ALSO returns the raw row set that drove
/// the recall decision. The chat loop uses the rows to emit a
/// `ChatEvent::Recall` (R2b) so the frontend's "本次召回" chip
/// shows pre-tool pitfall hits separately from FTS hits.
///
/// **Returns `(PitfallRecall, Vec<MemoryRow>)`** — the first
/// element is byte-identical to [`recall_pitfall`]'s return
/// (same classification logic, same `PitfallRecall` enum), the
/// second is the raw hit set (post the same in-memory
/// `is_full_match` filter applied to determine
/// `SoftBlock` vs `Footnote`). Empty `rows` is paired with
/// `PitfallRecall::None`; otherwise the rows are the full
/// pre-filter set (one per hit) regardless of whether they went
/// into the SoftBlock or Footnote bucket.
///
/// `PitfallRecall` enum shape is **unchanged** — the sibling
/// only ADDS the rows; P3/P4/P5 callers and tests do not break.
pub async fn recall_pitfall_with_hits(
    db: &SqlitePool,
    tool_name: &str,
    tool_input: &serde_json::Value,
    already_blocked: &HashSet<String>,
) -> (PitfallRecall, Vec<crate::db::memories::MemoryRow>) {
    recall_pitfall_inner_with_rows(db, tool_name, tool_input, already_blocked)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(
                error = %e,
                tool = tool_name,
                "recall_pitfall_with_hits: DB error (non-fatal, returning None)"
            );
            (PitfallRecall::None, Vec::new())
        })
}

/// 07-06 (am-observability-panel A9): the rows-aware
/// [`recall_pitfall_inner_with_rows`] is the single source of
/// truth. This rows-dropping variant exists only to back
/// [`recall_pitfall`] (the test-only wrapper); `#[allow(dead_code)]`
/// silences the lib-build warning for the same reason
/// [`recall_pitfall`] carries it (test consumers aren't visible
/// to `cargo build --lib`).
#[allow(dead_code)]
async fn recall_pitfall_inner(
    db: &SqlitePool,
    tool_name: &str,
    tool_input: &serde_json::Value,
    already_blocked: &HashSet<String>,
) -> Result<PitfallRecall, sqlx::Error> {
    let (recall, _rows) =
        recall_pitfall_inner_with_rows(db, tool_name, tool_input, already_blocked).await?;
    Ok(recall)
}

async fn recall_pitfall_inner_with_rows(
    db: &SqlitePool,
    tool_name: &str,
    tool_input: &serde_json::Value,
    already_blocked: &HashSet<String>,
) -> Result<(PitfallRecall, Vec<crate::db::memories::MemoryRow>), sqlx::Error> {
    let (command_pattern, path) = extract_probe_args(tool_name, tool_input);
    let rows = crate::db::memories::find_pitfalls_by_trigger_all_status(
        db,
        tool_name,
        command_pattern.as_deref(),
        path.as_deref(),
    )
    .await?;

    if rows.is_empty() {
        return Ok((PitfallRecall::None, rows));
    }

    // bump_hit_count per hit, fire-and-forget (best-effort). Same
    // pattern as the P3 footnote tier; the auto-promotion hook
    // (P5 Step 2) lives inside bump_hit_count.
    for row in &rows {
        let pool = db.clone();
        let mid = row.memory_id.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::db::memories::bump_hit_count(&pool, &mid).await {
                tracing::warn!(
                    memory_id = %mid,
                    error = %e,
                    "recall_pitfall: bump_hit_count failed (non-fatal)"
                );
            }
        });
    }

    // Tier classification (design §4 + D1).
    let mut soft_block: Option<(String, String)> = None;
    let mut footnote_rows: Vec<&crate::db::memories::MemoryRow> = Vec::new();

    for row in &rows {
        let status = row.status.as_str();
        let full = is_full_match(row, command_pattern.as_deref(), path.as_deref());
        let already = already_blocked.contains(&row.memory_id);

        if PITFALL_SOFT_BLOCK_ENABLED
            && status == "verified"
            && full
            && !already
            && soft_block.is_none()
        {
            // Verified + full match + not yet blocked this session →
            // SoftBlock. First qualifying row wins; the loop records
            // this memory_id so subsequent calls degrade.
            let hint = format_soft_block_hint(row);
            soft_block = Some((hint, row.memory_id.clone()));
            // A soft-blocked pitfall does NOT also appear in the
            // footnote — the SoftBlock replaces the footnote for
            // this hit.
            continue;
        }
        // Everything else → footnote bullet.
        footnote_rows.push(row);
    }

    if let Some((hint, memory_id)) = soft_block {
        // If we also have footnote candidates, surface them in the
        // SoftBlock hint (the LLM benefits from "and also these
        // related active pitfalls"). SoftBlock still wins (the loop
        // short-circuits execute_tool).
        let hint = if footnote_rows.is_empty() {
            hint
        } else {
            format!("{}\n{}", hint, build_footnote_body(&footnote_rows))
        };
        return Ok((PitfallRecall::SoftBlock { hint, memory_id }, rows));
    }

    if footnote_rows.is_empty() {
        return Ok((PitfallRecall::None, rows));
    }
    Ok((
        PitfallRecall::Footnote(build_footnote_body(&footnote_rows)),
        rows,
    ))
}

/// Build the multi-bullet footnote body (without the leading header).
/// Reused by both the `Footnote` tier and the SoftBlock hint when
/// there are also active/candidate rows to surface.
fn build_footnote_body(rows: &[&crate::db::memories::MemoryRow]) -> String {
    let mut out = String::from("⚠️ Memory: 此前在本项目执行类似操作时踩过坑 —\n");
    for row in rows {
        out.push_str(&format!("• [{}] {}\n", row.title, row.content));
    }
    out
}

/// Compose the soft-block hint (imperative, "this was NOT executed"
/// phrasing per design §4). `is_error=false` is set by the chat
/// loop on the wrapping `tool_result`; the hint text itself states
/// the semantics for the LLM.
fn format_soft_block_hint(row: &crate::db::memories::MemoryRow) -> String {
    format!(
        "⚠️ 此操作因历史 verified pitfall 被暂缓、未实际执行。请重新评估，\
         调整命令后重试或确认继续。\n\
         pitfall [{}] (verified): {}\n\
         （本 session 内此坑仅暂缓 1 次；再次调用将放行 + 注脚提示。）",
        row.title, row.content
    )
}

/// "Full match" predicate for the verified soft-block tier (design
/// §4: "完全命中 = tool + command_pattern + path_globs 三者皆中").
/// The bar is high — verified soft-block short-circuits `execute_tool`,
/// so the hit must be unambiguous.
///
/// Returns `true` when ALL of:
/// - the row has **at least one** of `command_pattern` / `path_globs`
///   set to `Some(_)` (a row with both `None` is fully path/command-
///   agnostic and would soft-block too broadly — degraded to Footnote),
/// - **every** `Some(_)` field on the row matches the probe:
///   - `command_pattern=Some(cp)` → probe's `command_pattern` contains `cp`.
///   - `path_globs=Some(globs)` → probe's `path` matches at least one
///     glob (the underlying SQL already applied this filter; the row
///     being in the result set is the proof of match).
///
/// A row missing the probe field for a `Some(_)` constraint degrades
/// to Footnote (e.g. a row with `command_pattern=Some` but the tool
/// kind yields no `command_pattern` in the probe → not a full match).
fn is_full_match(
    row: &crate::db::memories::MemoryRow,
    command_pattern: Option<&str>,
    _path: Option<&str>,
) -> bool {
    let has_cmd = row.command_pattern.is_some();
    let has_path = row.path_globs.is_some();
    if !has_cmd && !has_path {
        return false; // both None → too broad, degrade
    }
    // command_pattern constraint (if set on the row).
    if has_cmd {
        match (row.command_pattern.as_deref(), command_pattern) {
            (Some(mem_cp), Some(probe_cp)) if probe_cp.contains(mem_cp) => {}
            _ => return false,
        }
    }
    // path_globs constraint: the SQL filter already enforced the
    // glob match (the row wouldn't be here otherwise). has_path
    // alone is sufficient — the probe's `path` already matched.
    true
}

/// Resolve the `(command_pattern, path)` probe arguments from a
/// tool's input JSON, dispatching by tool kind. Returns
/// `(None, None)` for tool kinds that don't carry a probe-able
/// argument (e.g. dispatch_subagent — irrelevant for pitfall
/// recall).
///
/// **Why a per-tool dispatch**: pitfall rows store their
/// `command_pattern` (substring match) and `path_globs` (glob
/// match) as separate fields. The probe must extract the right
/// fields per tool so the underlying
/// `find_pitfalls_by_trigger` SQL filter does the right thing:
/// - Shell: `command` → substring probe.
/// - Path tools: `path` (with `cwd`/`working_directory` fallback) →
///   glob probe via `path_globs`.
/// - WebFetch: `url` → substring probe (matches pitfalls that
///   trigger on a domain or URL pattern).
/// - Other / unknown: no probe (no recall possible).
///
/// **Mirrors `extract_path_arg`'s precedence** for the path key
/// (`path` > `cwd` > `working_directory`), so the recall probe
/// uses the same canonical path the Tier 4 path-glob check
/// would resolve.
fn extract_probe_args(
    tool_name: &str,
    tool_input: &serde_json::Value,
) -> (Option<String>, Option<String>) {
    match classify_tool(tool_name) {
        ToolKind::Path => {
            // path-globs probe.
            (None, extract_path_arg(tool_name, tool_input))
        }
        ToolKind::Shell => {
            // substring probe on the full command. The underlying
            // `find_pitfalls_by_trigger` will further check
            // `command_pattern` substring containment inside
            // this value (the writer sets `command_pattern` to a
            // distinctive substring like "cargo test").
            let cmd = tool_input
                .get("command")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (cmd, None)
        }
        ToolKind::WebFetch => {
            // URL substring probe. Most pitfalls store a host
            // substring (e.g. "api.example.com") that the full
            // URL contains.
            let url = tool_input
                .get("url")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (url, None)
        }
        ToolKind::GitMutation | ToolKind::Other => {
            // No probe-able field — recall returns empty.
            (None, None)
        }
    }
}
