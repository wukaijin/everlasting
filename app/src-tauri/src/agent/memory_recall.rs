//! P2 session-start recall (per-turn, actually) — surfaces
//! candidate/active/verified `preference` / `fact` memories whose
//! FTS5 bm25 match against the user's latest message. The matched
//! memories are appended as an ephemeral block to the synthetic
//! instructions message (`messages[0]`) so the Anthropic cache
//! breakpoint on the banner block stays intact (the instruction
//! body prefix is unchanged; only the block list grows).
//!
//! # Why per-turn (not once at session start)
//!
//! spike-007 §4 layer 1 said "session 开始召回" but the PRD
//! decision 6 (brainstorm) refined it: the query is the user's
//! LATEST message, which changes every turn. So we run the FTS5
//! search every turn with the current last-user text. The FTS5
//! query is millisecond-scale; the per-turn cost is negligible
//! relative to LLM network latency.
//!
//! # Cache correctness (load-bearing)
//!
//! The instruction message lives at `messages[0]` and its FIRST
//! block (the banner) carries `cache_control: Ephemeral`. Anthropic
//! caches the byte-prefix UP TO the last breakpoint. The recall
//! block is appended AFTER the instruction body blocks in the
//! turn-scoped REQUEST clone (NOT the persisted `messages` Vec),
//! so:
//! - The persisted `messages[0]` is byte-identical across turns
//!   (the recall block is per-turn-only, like the B12 checklist
//!   injection).
//! - The banner + instruction body prefix is stable across turns
//!   → the cache window stays warm.
//!
//! No `cache_control` on the recall block itself: it changes every
//! turn (different query → different matches), so a breakpoint
//! would never hit.

use crate::db::memories::{
    bump_hit_count, search_memories_fts_recall, MemoryRow, RecallStatusFilter,
};
use crate::llm::types::{ChatMessage, ContentBlock, MessageContent, Role};
use crate::memory::tokens::count_tokens;
use sqlx::SqlitePool;

/// Hard token cap on the injected recall body (PRD decision 2 /
/// spike-006 §4.3). Memories are added in created_at DESC order
/// (newest first, per PRD decision 2); the running token sum is
/// truncated once it exceeds this cap.
pub const RECALL_TOKEN_BUDGET: u32 = 500;

/// FTS5 result limit (top-k). We over-fetch slightly then truncate
/// by token budget — bm25 ranking is the primary signal, token
/// budget is the secondary cap.
const RECALL_RESULT_LIMIT: i64 = 10;

/// Format one memory row as a single bullet line in the recall
/// block. Pure function — exposed for tests.
pub fn format_memory_line(mem: &MemoryRow) -> String {
    format!(
        "- [{}] {}: {}",
        mem.kind, mem.title, mem.content
    )
}

/// Search + format the recall block for the given user query.
///
/// - `project_id` — the session's project UUID/path string (the
///   same value the remember tool bound; passed to
///   `search_memories_fts` for the project branch of the
///   scope=None "both layers" search).
/// - `query` — the user's latest message text. Empty / whitespace
///   → no recall (returns `None`).
///
/// Returns `Some(text)` (the formatted `<relevant-memories>` body)
/// when ≥ 1 memory matches AND fits in the token budget; `None`
/// when there are no matches or the query is empty. The caller
/// wraps the text in a `ContentBlock::Text` (no `cache_control`).
///
/// Side effect: each surfaced memory's `hit_count` is bumped
/// (best-effort; failures log `warn!` and continue — the recall
/// return value is unaffected).
pub async fn build_recall_text(
    pool: &SqlitePool,
    project_id: &str,
    query: &str,
) -> Option<String> {
    if query.trim().is_empty() {
        return None;
    }
    let rows = match search_memories_fts_recall(
        pool,
        Some(project_id),
        None, // scope=None → search user + project layers
        query,
        RECALL_RESULT_LIMIT,
        // P2 ADR-lite: candidate rows ARE surfaced. P5 (design §3)
        // DELIBERATELY keeps `IncludeCandidate` — tightening to
        // `ActiveVerifiedOnly` would sever the candidate→active
        // promotion path: candidate is promoted BY being recalled
        // (hit_count accrues on recall); exclude it from recall and
        // it can never graduate (especially preference/fact kinds,
        // which have no `trigger_key` and rely on this FTS path
        // alone). Library noise is controlled by the low promotion
        // threshold (design D2) + the hygiene job (design §6), not
        // by the recall filter.
        RecallStatusFilter::IncludeCandidate,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "recall: FTS5 search failed; skipping injection");
            return None;
        }
    };
    if rows.is_empty() {
        return None;
    }

    // Token-budget truncation (PRD decision 2: created_at DESC, then
    // accumulate tokens until budget exhausted). search_memories_fts
    // returns bm25-ranked order; we re-sort by created_at DESC per
    // the PRD decision so cache stability is predictable (same
    // created_at set → same prefix regardless of bm25 drift).
    let mut sorted: Vec<MemoryRow> = rows;
    sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    let mut lines: Vec<String> = Vec::new();
    let mut used: u32 = 0;
    // Reserve a small header budget (the `<relevant-memories>`
    // wrapper + newline overhead). ~10 tokens is generous for the
    // wrapper.
    const HEADER_BUDGET: u32 = 10;
    used += HEADER_BUDGET;
    for mem in &sorted {
        let line = format_memory_line(mem);
        let line_tokens = count_tokens(&line).await;
        if used + line_tokens > RECALL_TOKEN_BUDGET {
            break;
        }
        used += line_tokens;
        // Best-effort hit-count bump. A failure here doesn't
        // invalidate the recall — the memory still surfaces; only
        // the P5 promotion accounting is affected.
        if let Err(e) = bump_hit_count(pool, &mem.memory_id).await {
            tracing::warn!(
                error = %e,
                memory_id = %mem.memory_id,
                "recall: bump_hit_count failed (best-effort, continuing)"
            );
        }
        lines.push(line);
    }

    if lines.is_empty() {
        // The first memory alone exceeded the budget (extremely
        // unlikely with a 500-token cap and ≤500-char content, but
        // defensive). Surface at least the first one truncated.
        let mem = &sorted[0];
        let line = format_memory_line(mem);
        let _ = bump_hit_count(pool, &mem.memory_id).await;
        lines.push(line);
    }

    Some(format!(
        "<relevant-memories>\n\
         The following are your previously-remembered experiences that may be \
         relevant to the user's latest message. Treat them as EXPERIENCE hints, \
         not rules — verify against the current context before acting.\n\
         {}\n\
         </relevant-memories>",
        lines.join("\n")
    ))
}

/// Wrap the recall text as a `ContentBlock::Text` (no cache_control
/// — the block changes every turn). Pure convenience wrapper.
pub fn recall_block(text: String) -> ContentBlock {
    ContentBlock::Text {
        text,
        cache_control: None,
    }
}

/// Append the recall block to the instruction message's block list
/// in the turn-scoped request clone. If `messages` has no
/// instruction message at position 0 (no memory files loaded), the
/// recall block is wrapped in its own synthetic user message and
/// prepended (so it still surfaces — recall works even on a fresh
/// install with no CLAUDE.md).
///
/// **Mutates `turn_messages`** (the request clone), NOT the
/// persisted `messages`. The persisted Vec is byte-identical
/// across turns (the recall block is per-turn-only).
pub fn inject_recall_into_turn(
    turn_messages: &mut Vec<ChatMessage>,
    recall_text: String,
) {
    let block = recall_block(recall_text);
    if let Some(first) = turn_messages.first_mut() {
        if first.role == Role::User {
            if let MessageContent::Blocks(ref mut blocks) = first.content {
                blocks.push(block);
                return;
            }
        }
    }
    // No user instruction message at position 0 — wrap the recall
    // in its own synthetic user message and prepend.
    turn_messages.insert(
        0,
        ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![block]),
        },
    );
}

/// The cache-control marker used by the instruction banner. Re-
/// exported here so tests can assert the recall block does NOT
/// carry it (per-turn-mutating blocks must not be cache
/// breakpoints).
#[cfg(test)]
pub(crate) const TEST_CACHE_CONTROL_MARKER: Option<crate::llm::types::CacheControl> =
    Some(crate::llm::types::CacheControl::Ephemeral);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::memories::{
        insert_memory, MemoryInput, MemoryKind, MemoryScope, MemoryStatus,
    };

    async fn make_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        crate::db::migrations::run_migrations(&pool).await.unwrap();
        pool
    }

    fn mem_input(title: &str, content: &str, kind: MemoryKind) -> MemoryInput {
        MemoryInput {
            scope: MemoryScope::Project,
            project_id: Some("/repo/proj".into()),
            kind,
            status: MemoryStatus::Candidate,
            title: title.into(),
            content: content.into(),
            tags: "[]".into(),
            tool_name: None,
            command_pattern: None,
            path_globs: None,
            source_session_id: Some("sess-test".into()),
            source_ref: None,
        }
    }

    #[test]
    fn format_memory_line_includes_kind_title_content() {
        let inp = mem_input("tabs", "user prefers tabs", MemoryKind::Preference);
        let row = MemoryRow {
            id: 1,
            memory_id: "m1".into(),
            scope: "project".into(),
            project_id: Some("/repo/proj".into()),
            kind: "preference".into(),
            status: "candidate".into(),
            title: inp.title.clone(),
            content: inp.content.clone(),
            tags: "[]".into(),
            tool_name: None,
            command_pattern: None,
            path_globs: None,
            source_session_id: Some("s".into()),
            source_ref: None,
            confidence: 0.5,
            hit_count: 0,
            last_used_at: None,
            created_at: "2026-06-29T00:00:00Z".into(),
            updated_at: "2026-06-29T00:00:00Z".into(),
            demoted_reason: None,
        };
        let line = format_memory_line(&row);
        assert!(line.contains("[preference]"));
        assert!(line.contains("tabs"));
        assert!(line.contains("user prefers tabs"));
    }

    #[tokio::test]
    async fn build_recall_text_returns_none_for_empty_query() {
        let pool = make_pool().await;
        assert!(build_recall_text(&pool, "/repo/proj", "").await.is_none());
        assert!(build_recall_text(&pool, "/repo/proj", "   ").await.is_none());
    }

    #[tokio::test]
    async fn build_recall_text_returns_none_when_no_matches() {
        let pool = make_pool().await;
        insert_memory(&pool, &mem_input("tabs", "user prefers tabs", MemoryKind::Preference))
            .await
            .unwrap();
        // Unrelated query → no match.
        assert!(build_recall_text(&pool, "/repo/proj", "completely unrelated topic xyz")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn build_recall_text_surfaces_candidate_match() {
        let pool = make_pool().await;
        insert_memory(
            &pool,
            &mem_input("WSL cargo", "set PKG_CONFIG_PATH for cargo in wsl", MemoryKind::Fact),
        )
        .await
        .unwrap();
        let text = build_recall_text(&pool, "/repo/proj", "cargo build in wsl")
            .await
            .expect("match found");
        assert!(text.contains("<relevant-memories>"));
        assert!(text.contains("WSL cargo"));
        assert!(text.contains("PKG_CONFIG_PATH"));
        assert!(text.contains("[fact]"));
        // hit_count bumped.
        let rows = crate::db::memories::list_memories(
            &pool,
            Some(MemoryScope::Project),
            Some("/repo/proj"),
        )
        .await
        .unwrap();
        assert_eq!(rows[0].hit_count, 1);
    }

    #[tokio::test]
    async fn build_recall_text_truncates_at_token_budget() {
        let pool = make_pool().await;
        // Insert many candidate memories with overlapping keywords
        // so they all match. Each content is ~30 tokens; with a
        // 500-token budget we should get ~15 but the loop stops
        // when budget is exhausted.
        for i in 0..30 {
            let inp = MemoryInput {
                title: format!("memory number {}", i),
                content: format!(
                    "this is memory number {} about cargo build configuration in wsl",
                    i
                ),
                ..mem_input(
                    &format!("t{}", i),
                    "placeholder",
                    MemoryKind::Fact,
                )
            };
            insert_memory(&pool, &inp).await.unwrap();
        }
        let text = build_recall_text(&pool, "/repo/proj", "cargo build wsl")
            .await
            .expect("matches found");
        // Count the bullet lines — must be < 30 (truncated).
        let bullet_count = text.matches("\n- ").count() + 1; // +1 for the first bullet
        assert!(
            bullet_count < 30,
            "expected truncation, got {} bullets",
            bullet_count
        );
        assert!(bullet_count >= 1);
    }

    #[tokio::test]
    async fn build_recall_text_isolates_projects() {
        let pool = make_pool().await;
        // Memory in proj-A. (Avoid the word "secret" — the
        // write-safety net's sensitive regex matches it.)
        insert_memory(
            &pool,
            &MemoryInput {
                scope: MemoryScope::Project,
                project_id: Some("/repo/proj-a".into()),
                kind: MemoryKind::Fact,
                status: MemoryStatus::Candidate,
                title: "proj-a config note".into(),
                content: "proj-a cargo config detail".into(),
                tags: "[]".into(),
                tool_name: None,
                command_pattern: None,
                path_globs: None,
                source_session_id: None,
                source_ref: None,
            },
        )
        .await
        .unwrap();
        // Searching proj-B for the same keyword must NOT surface it.
        let text = build_recall_text(&pool, "/repo/proj-b", "cargo config").await;
        assert!(text.is_none(), "proj-a memory isolated from proj-B");
    }

    #[test]
    fn inject_recall_appends_to_instruction_message_blocks() {
        let mut msgs = vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: "banner".into(),
                    cache_control: TEST_CACHE_CONTROL_MARKER,
                },
                ContentBlock::Text {
                    text: "instruction body".into(),
                    cache_control: None,
                },
            ]),
        }];
        inject_recall_into_turn(&mut msgs, "recall body".into());
        // Still 1 message; blocks grew by 1.
        assert_eq!(msgs.len(), 1);
        if let MessageContent::Blocks(blocks) = &msgs[0].content {
            assert_eq!(blocks.len(), 3);
            // Banner block unchanged (cache_control preserved).
            assert!(matches!(
                &blocks[0],
                ContentBlock::Text { text, cache_control } if text == "banner" && cache_control.is_some()
            ));
            // Recall block has no cache_control.
            assert!(matches!(
                &blocks[2],
                ContentBlock::Text { text, cache_control } if text == "recall body" && cache_control.is_none()
            ));
        } else {
            panic!("expected Blocks variant");
        }
    }

    #[test]
    fn inject_recall_prepends_when_no_instruction_message() {
        let mut msgs: Vec<ChatMessage> = vec![];
        inject_recall_into_turn(&mut msgs, "recall body".into());
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, Role::User);
    }

    #[test]
    fn inject_recall_prepends_when_first_message_is_text() {
        // If messages[0] is a plain Text (not Blocks), we can't
        // append — prepend a new synthetic message instead.
        let mut msgs = vec![ChatMessage {
            role: Role::User,
            content: MessageContent::Text("hello".into()),
        }];
        inject_recall_into_turn(&mut msgs, "recall body".into());
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::User);
        // The recall message is now first.
        if let MessageContent::Blocks(blocks) = &msgs[0].content {
            assert_eq!(blocks.len(), 1);
        } else {
            panic!("expected Blocks");
        }
    }
}
