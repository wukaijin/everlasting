//! P5 (autonomous memory quality layer, 2026-06-29): hygiene job +
//! char-trigram Jaccard dedup utility.
//!
//! See `.trellis/tasks/06-29-am-p5-quality/design.md` §6 + spike-007
//! §10 for the design lineage. This module is the **quality layer**
//! that keeps the autonomous-memory library from accumulating
//! duplicates + stale rows over time:
//!
//! - [`char_trigrams`] / [`jaccard`] — zero-dependency, language-
//!   agnostic similarity scorer (design D3). Char-trigram (not word-
//!   token) so it works on Chinese short sentences without a
//!   tokenizer. `> 0.7` is the dedup threshold.
//! - [`run_hygiene_pass`] — the event-triggered (not interval-polled)
//!   cleanup pass: dedup-merge high-Jaccard pairs (pitfall by
//!   `trigger_key`, others by char-trigram Jaccard) + age-out low-hit
//!   memories. Fire-and-forget on a `tokio::spawn` from
//!   `insert_memory` (every 10 inserts per `(scope, kind)`) and once
//!   at app startup (D4).
//!
//! # Why event-triggered (not `tokio::time::interval`)
//!
//! The project has **no existing long-running `interval` task** —
//! every `tokio::spawn` is fire-and-forget (P4 reflection, L1a bg
//! shell). Adding a resident interval would require lifecycle +
//! shutdown plumbing disproportionate to "hygiene doesn't need to be
//! real-time". The event trigger (count-mod on insert) amortises the
//! cost: a busy write session gets periodic cleanups, an idle DB
//! doesn't pay anything, and the app-startup pass covers "user
//! accumulated 200 rows then closed the app". See design D4.

use std::collections::HashSet;

use sqlx::SqlitePool;

use crate::db::memories::{
    delete_memory, list_memories, update_status, MemoryKind, MemoryScope, MemoryStatus,
};

// ---------------------------------------------------------------------------
// char-trigram Jaccard (design D3)
// ---------------------------------------------------------------------------

/// Build the char-trigram set for `s`. A "char" here is a Unicode
/// scalar value (`char`), NOT a byte — so a CJK char is one unit
/// (design D3: "char-trigram, language-agnostic"). Strings shorter
/// than 3 chars collapse to a single trigram (the whole string) so
/// very short inputs still produce a non-empty set (avoids div-by-zero
/// in [`jaccard`]).
///
/// Lowercased + trimmed so `Foo` and `foo ` produce the same set.
pub fn char_trigrams(s: &str) -> HashSet<String> {
    let normalized: String = s.trim().to_lowercase();
    let chars: Vec<char> = normalized.chars().collect();
    if chars.is_empty() {
        return HashSet::new();
    }
    if chars.len() < 3 {
        // Whole string is one "trigram" — keeps short inputs
        // comparable to themselves + lets Jaccard return 1.0 for
        // identical short strings.
        let mut set = HashSet::new();
        set.insert(normalized);
        return set;
    }
    chars
        .windows(3)
        .map(|w| w.iter().collect::<String>())
        .collect()
}

/// Jaccard similarity over the char-trigram sets of `a` and `b`.
/// Returns `0.0..=1.0`. Empty inputs (`char_trigrams` empty on both
/// sides) return `0.0` (avoid the div-by-zero; two empty strings are
/// NOT "identical" in any useful sense for dedup).
pub fn jaccard(a: &str, b: &str) -> f32 {
    let sa = char_trigrams(a);
    let sb = char_trigrams(b);
    if sa.is_empty() || sb.is_empty() {
        return 0.0;
    }
    let inter = sa.intersection(&sb).count() as f32;
    let union = sa.union(&sb).count() as f32;
    if union == 0.0 {
        return 0.0;
    }
    inter / union
}

// ---------------------------------------------------------------------------
// Hygiene pass — dedup-merge + age-out (design §6)
// ---------------------------------------------------------------------------

/// Jaccard threshold above which two non-pitfall memories are
/// considered duplicates (design D3 / spike-007 §10). Char-trigram
/// on the `content` field — the body carries the actual experience;
/// title is a short label that may legitimately repeat across
/// distinct memories.
pub const DEDUP_JACCARD_THRESHOLD: f32 = 0.7;

/// Age-out rule (design §6 / spike-007 §10): a memory in
/// `candidate`/`active` whose `last_used_at` is older than this many
/// days AND whose `hit_count < AGE_OUT_MIN_HITS` is demoted.
pub const AGE_OUT_DAYS: i64 = 30;
pub const AGE_OUT_MIN_HITS: i64 = 2;

/// Run one hygiene pass over the entire memory library.
///
/// Two operations (design §6):
/// 1. **dedup-merge**: for each `(scope, kind)` bucket, find pairs
///    whose similarity exceeds [`DEDUP_JACCARD_THRESHOLD`] and merge
///    them — pitfall kind by exact `trigger_key` match (tool +
///    command_pattern + path_globs), other kinds by char-trigram
///    Jaccard on `content`. The merge keeps the row with the higher
///    `confidence` (tie-broken by higher `hit_count`) and deletes the
///    other; future `hit_count` accrual is unaffected (the keeper
///    already had its own count).
/// 2. **age-out**: rows in `candidate`/`active` with
///    `last_used_at > AGE_OUT_DAYS` ago AND `hit_count <
///    AGE_OUT_MIN_HITS` → [`MemoryStatus::Demoted`] with
///    `demoted_reason = "aged_out"`.
///
/// Fire-and-forget: callers (`insert_memory` event trigger, app
/// startup) spawn this on a `tokio::spawn` and never await it. All
/// errors are `tracing::warn!`-logged and swallowed — a hygiene pass
/// failure MUST NOT propagate to the caller (the write already
/// succeeded; hygiene is best-effort cleanup).
pub async fn run_hygiene_pass(pool: SqlitePool) {
    if let Err(e) = run_hygiene_pass_inner(&pool).await {
        tracing::warn!(error = %e, "memory hygiene pass failed (non-fatal)");
    }
}

async fn run_hygiene_pass_inner(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    // Load the whole library once; the table is small (v1, no global
    // scope, ≤ a few hundred rows in practice). Sorting + bucketing
    // in-memory avoids N round-trips.
    let all = list_memories(pool, None, None)
        .await
        .map_err(|e| sqlx::Error::Protocol(format!("hygiene list_memories: {e}")))?;

    dedup_pass(pool, &all).await?;
    age_out_pass(pool, &all).await?;
    Ok(())
}

/// Bucket by `(scope, kind)`, find similar pairs, delete the loser.
/// The "loser" is the row with the lower `confidence` (tie-broken by
/// lower `hit_count`, then by `memory_id` for determinism). The
/// keeper retains its own `hit_count` — design §6 chose accumulator-
/// free merge to avoid a second UPDATE per pair (the keeper's count
/// already reflects its recall hits; the loser's hits are rare and
/// the dedup is best-effort).
async fn dedup_pass(pool: &SqlitePool, all: &[crate::db::memories::MemoryRow]) -> Result<(), sqlx::Error> {
    for scope in [MemoryScope::User, MemoryScope::Project] {
        for kind in [
            MemoryKind::Pitfall,
            MemoryKind::Preference,
            MemoryKind::Fact,
            MemoryKind::Decision,
        ] {
            let bucket: Vec<&crate::db::memories::MemoryRow> = all
                .iter()
                .filter(|r| r.scope == scope.as_str() && r.kind == kind.as_str())
                .collect();
            if bucket.len() < 2 {
                continue;
            }

            // Pitfall dedup: exact trigger_key match (tool_name +
            // command_pattern + path_globs all equal). Non-pitfall
            // dedup: char-trigram Jaccard on content (design §6).
            let mut losers: HashSet<String> = HashSet::new();
            for i in 0..bucket.len() {
                if losers.contains(&bucket[i].memory_id) {
                    continue;
                }
                for j in (i + 1)..bucket.len() {
                    if losers.contains(&bucket[j].memory_id) {
                        continue;
                    }
                    let similar = if kind == MemoryKind::Pitfall {
                        trigger_key_equal(bucket[i], bucket[j])
                    } else {
                        jaccard(&bucket[i].content, &bucket[j].content) >= DEDUP_JACCARD_THRESHOLD
                    };
                    if !similar {
                        continue;
                    }
                    // Pick the keeper: higher confidence, then higher
                    // hit_count, then lower memory_id (deterministic).
                    let (keeper, loser) = pick_keeper(bucket[i], bucket[j]);
                    losers.insert(loser.memory_id.clone());
                    let _ = keeper; // keeper stays in the bucket for
                                    // further comparisons (it may
                                    // dedup against more rows).
                }
            }

            for loser_id in &losers {
                if let Err(e) = delete_memory(pool, loser_id).await {
                    tracing::warn!(
                        memory_id = %loser_id,
                        error = %e,
                        "hygiene: delete_memory failed (non-fatal)"
                    );
                }
            }
        }
    }
    Ok(())
}

/// Pitfall trigger-key equality: `tool_name` + `command_pattern` +
/// `path_globs` all equal. Both fields `None`/`null` count as equal
/// (two path-agnostic, command-agnostic pitfalls on the same tool
/// are duplicates).
fn trigger_key_equal(a: &crate::db::memories::MemoryRow, b: &crate::db::memories::MemoryRow) -> bool {
    a.tool_name == b.tool_name
        && a.command_pattern == b.command_pattern
        && a.path_globs == b.path_globs
}

/// Pick the keeper between two duplicate rows. Returns `(keeper, loser)`.
fn pick_keeper<'a>(
    a: &'a crate::db::memories::MemoryRow,
    b: &'a crate::db::memories::MemoryRow,
) -> (&'a crate::db::memories::MemoryRow, &'a crate::db::memories::MemoryRow) {
    // Higher confidence wins.
    if a.confidence > b.confidence {
        return (a, b);
    }
    if b.confidence > a.confidence {
        return (b, a);
    }
    // Tie on confidence → higher hit_count wins.
    if a.hit_count > b.hit_count {
        return (a, b);
    }
    if b.hit_count > a.hit_count {
        return (b, a);
    }
    // Tie on both → lower memory_id wins (deterministic; UUIDv7 so
    // older rows sort first, which matches "keep the original").
    if a.memory_id <= b.memory_id {
        (a, b)
    } else {
        (b, a)
    }
}

/// Demote stale low-hit candidate/active rows. A row is "stale" if
/// `last_used_at` is more than [`AGE_OUT_DAYS`] days ago AND
/// `hit_count < AGE_OUT_MIN_HITS`. Verified rows are exempt (they've
/// earned their keep; only age out unverified noise).
async fn age_out_pass(pool: &SqlitePool, all: &[crate::db::memories::MemoryRow]) -> Result<(), sqlx::Error> {
    let now = chrono::Utc::now();
    let cutoff = now - chrono::Duration::days(AGE_OUT_DAYS);
    for row in all {
        if row.status != MemoryStatus::Candidate.as_str()
            && row.status != MemoryStatus::Active.as_str()
        {
            continue;
        }
        if row.hit_count >= AGE_OUT_MIN_HITS {
            continue;
        }
        // Fall back to created_at when never recalled — a row that's
        // never been hit AND is old is the strongest age-out signal.
        let age_clock: &str = match &row.last_used_at {
            Some(s) => s,
            None => &row.created_at,
        };
        let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(age_clock) else {
            continue;
        };
        if parsed.with_timezone(&chrono::Utc) >= cutoff {
            continue;
        }
        // Demote with reason "aged_out". Illegal transitions
        // (already-demoted) are silently ignored — `update_status`
        // returns Err(Illegal) which we log + swallow.
        if let Err(e) = update_status(
            pool,
            &row.memory_id,
            MemoryStatus::Demoted,
            Some("aged_out"),
        )
        .await
        {
            // NotFound (row deleted between list + update) is benign;
            // Illegal (already-demoted) is benign. Only log the rest.
            match e {
                crate::db::memories::StatusTransitionError::NotFound(_)
                | crate::db::memories::StatusTransitionError::Illegal { .. } => {}
                other => tracing::warn!(
                    memory_id = %row.memory_id,
                    error = %other,
                    "hygiene: age-out demote failed (non-fatal)"
                ),
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- char_trigrams ---

    #[test]
    fn trigrams_identical_strings_produce_same_set() {
        let a = char_trigrams("hello world");
        let b = char_trigrams("hello world");
        assert_eq!(a, b);
    }

    #[test]
    fn trigrams_empty_string_is_empty_set() {
        assert!(char_trigrams("").is_empty());
        assert!(char_trigrams("   ").is_empty());
    }

    #[test]
    fn trigrams_short_string_collapses_to_whole_string() {
        let set = char_trigrams("hi");
        assert_eq!(set.len(), 1);
        assert!(set.contains("hi"));
    }

    #[test]
    fn trigrams_case_and_trim_normalized() {
        // "Foo " and "foo" both produce {"foo"} after trim + lower.
        assert_eq!(char_trigrams("Foo "), char_trigrams("foo"));
    }

    #[test]
    fn trigrams_generates_expected_count() {
        // "hello" has 5 chars → 3 trigrams: hel, ell, llo.
        let set = char_trigrams("hello");
        assert_eq!(set.len(), 3);
        assert!(set.contains("hel"));
        assert!(set.contains("ell"));
        assert!(set.contains("llo"));
    }

    // --- jaccard ---

    #[test]
    fn jaccard_identical_is_one() {
        let v = jaccard("PKG_CONFIG_PATH 环境变量", "PKG_CONFIG_PATH 环境变量");
        assert!((v - 1.0).abs() < 1e-6, "got {v}");
    }

    #[test]
    fn jaccard_disjoint_is_zero() {
        let v = jaccard("completely", "xyzabc123");
        assert_eq!(v, 0.0);
    }

    #[test]
    fn jaccard_empty_inputs_return_zero() {
        assert_eq!(jaccard("", ""), 0.0);
        assert_eq!(jaccard("hello", ""), 0.0);
        assert_eq!(jaccard("", "hello"), 0.0);
    }

    #[test]
    fn jaccard_chinese_overlap_above_threshold() {
        // Two short Chinese sentences sharing the key phrase
        // "PKG_CONFIG_PATH" should score above the dedup threshold.
        let v = jaccard("设置 PKG_CONFIG_PATH 环境变量", "PKG_CONFIG_PATH 环境变量的坑");
        assert!(
            v >= DEDUP_JACCARD_THRESHOLD,
            "got {v}, expected >= {DEDUP_JACCARD_THRESHOLD}"
        );
    }

    #[test]
    fn jaccard_unrelated_below_threshold() {
        let v = jaccard("用 tabs 而不是 spaces", "WSL 跑 cargo test 失败");
        assert!(v < DEDUP_JACCARD_THRESHOLD, "got {v}");
    }

    #[test]
    fn jaccard_symmetric() {
        let a = jaccard("first string here", "second string here");
        let b = jaccard("second string here", "first string here");
        assert!((a - b).abs() < 1e-6);
    }

    // --- pick_keeper / trigger_key_equal ---

    fn row(
        memory_id: &str,
        confidence: f64,
        hit_count: i64,
        tool: Option<&str>,
        cmd: Option<&str>,
        globs: Option<&str>,
    ) -> crate::db::memories::MemoryRow {
        use crate::db::memories::MemoryRow;
        MemoryRow {
            id: 0,
            memory_id: memory_id.to_string(),
            scope: "user".into(),
            project_id: None,
            kind: "pitfall".into(),
            status: "active".into(),
            title: "t".into(),
            content: "c".into(),
            tags: "[]".into(),
            tool_name: tool.map(String::from),
            command_pattern: cmd.map(String::from),
            path_globs: globs.map(String::from),
            source_session_id: None,
            source_ref: None,
            confidence,
            hit_count,
            last_used_at: None,
            created_at: "2026-06-29T00:00:00+00:00".into(),
            updated_at: "2026-06-29T00:00:00+00:00".into(),
            demoted_reason: None,
        }
    }

    #[test]
    fn trigger_key_equal_requires_all_three_fields_equal() {
        let a = row("a", 0.5, 0, Some("shell"), Some("cargo"), Some(r#"["x"]"#));
        // Identical → equal.
        let b = row("b", 0.5, 0, Some("shell"), Some("cargo"), Some(r#"["x"]"#));
        assert!(trigger_key_equal(&a, &b));
        // Different tool → not equal.
        let c = row("c", 0.5, 0, Some("edit_file"), Some("cargo"), Some(r#"["x"]"#));
        assert!(!trigger_key_equal(&a, &c));
        // Different cmd → not equal.
        let d = row("d", 0.5, 0, Some("shell"), Some("npm"), Some(r#"["x"]"#));
        assert!(!trigger_key_equal(&a, &d));
        // Different globs → not equal.
        let e = row("e", 0.5, 0, Some("shell"), Some("cargo"), Some(r#"["y"]"#));
        assert!(!trigger_key_equal(&a, &e));
    }

    #[test]
    fn trigger_key_equal_when_both_null_is_equal() {
        let a = row("a", 0.5, 0, Some("shell"), None, None);
        let b = row("b", 0.5, 0, Some("shell"), None, None);
        assert!(trigger_key_equal(&a, &b));
    }

    #[test]
    fn pick_keeper_prefers_higher_confidence() {
        let a = row("a", 0.9, 0, None, None, None);
        let b = row("b", 0.5, 5, None, None, None);
        let (keeper, loser) = pick_keeper(&a, &b);
        assert_eq!(keeper.memory_id, "a");
        assert_eq!(loser.memory_id, "b");
    }

    #[test]
    fn pick_keeper_tiebreaks_on_hit_count() {
        let a = row("a", 0.5, 1, None, None, None);
        let b = row("b", 0.5, 5, None, None, None);
        let (keeper, loser) = pick_keeper(&a, &b);
        assert_eq!(keeper.memory_id, "b");
        assert_eq!(loser.memory_id, "a");
    }

    #[test]
    fn pick_keeper_tiebreaks_on_memory_id_when_all_equal() {
        let a = row("aaa", 0.5, 1, None, None, None);
        let b = row("bbb", 0.5, 1, None, None, None);
        let (keeper, loser) = pick_keeper(&a, &b);
        assert_eq!(keeper.memory_id, "aaa");
        assert_eq!(loser.memory_id, "bbb");
    }
}
