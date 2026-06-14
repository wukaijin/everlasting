//! C3 Context compression + token budget management.
//!
//! MVP implementation of the ⑤ Context-overflow degradation step
//! described in `docs/ARCHITECTURE.md §2.5.5`. When the conversation
//! history approaches the model's `context_window`, this module
//! trims the oldest messages to bring the estimated token count
//! back under a safe target.
//!
//! **Decisions (PRD ADR-lite)**:
//! - Token estimation reuses `crate::memory::tokens::count_tokens`
//!   (cl100k_base). 1-2% drift from the Anthropic tokenizer is
//!   absorbed by the conservative 0.80 trigger / 0.50 target
//!   thresholds — no need for a per-model tokenizer.
//! - Trigger threshold: `context_window * 0.80`.
//! - Trim target: `context_window * 0.50`.
//! - Protection priority (high → low):
//!   1. `messages[0..=1]` — B5 synthetic memory pair
//!      (`instructions` user message + assistant ack). Never dropped.
//!   2. Last user message — current turn input. Never dropped.
//!   3. `Thinking` / `RedactedThinking` blocks — must round-trip
//!      verbatim or the API returns 400. These are protected
//!      **by association**: they live inside assistant turns; if
//!      the whole turn is dropped, the thinking block goes with
//!      it (which is safe — Anthropic only requires the signature
//!      of thinking blocks still IN the history; a missing block
//!      is fine). We never split a turn (never keep part of a
//!      turn's blocks and drop the rest) so no signature ends up
//!      orphaned.
//!   4. Old runtime `tool_result` messages (older `user(tool_result)`).
//!   5. Old user / assistant turns (oldest first).
//! - Pair-protection: an `assistant(tool_use)` turn and its matching
//!   `user(tool_result)` turn form a pair. Anthropic rejects a
//!   `tool_use` without its `tool_result` (and vice versa) with a
//!   400, so the pair is dropped together.
//!
//! The compaction is **in-memory only** — the DB still has every
//! persisted message; we only mutate the `Vec<ChatMessage>` that
//! goes into `provider.send()` for this turn.

use serde::Serialize;

use crate::llm::{ChatMessage, ContentBlock, MessageContent, Role};
use crate::memory::tokens::count_tokens;

/// Compaction trigger: when estimated tokens reach this fraction of
/// `context_window`, compaction kicks in.
const TRIGGER_RATIO: f64 = 0.80;
/// Compaction target: after compaction, the estimated tokens should
/// be at or below this fraction of `context_window`.
const TARGET_RATIO: f64 = 0.50;
/// The number of messages at the head of the array that are
/// permanently protected (B5 synthetic memory pair: instructions
/// user message + assistant ack). See `chat.rs` lines ~355-388 for
/// the insertion site.
const PROTECTED_HEAD: usize = 2;

/// What kind of state did we leave the message list in after
/// compaction?
///
/// RULE-A-002 (2026-06-14): `StillOver` is the failure mode that
/// used to silently send an over-budget prompt to the LLM. When the
/// agent loop sees `StillOver` it MUST emit an `Error` event and
/// abort the turn (do NOT call `provider.send`) — otherwise the
/// over-budget prompt will 400 on `prompt is too long`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum DegradationKind {
    /// Either below trigger, or compacted cleanly to target. The
    /// caller may safely proceed with `provider.send`.
    None,
    /// Compaction ran but produced no droppable candidates (the
    /// middle is empty / all implicitly-protected tool_use turns).
    /// Safe to proceed — the message list is unchanged.
    NoCandidates,
    /// Compaction dropped everything it could but the list is still
    /// over the target threshold. The caller MUST NOT send this to
    /// the LLM — it would 400 on `prompt is too long`. The agent
    /// loop emits an `Error` event and aborts the turn instead.
    StillOver {
        /// Estimated tokens of the trimmed list.
        tokens_after: u32,
        /// The target threshold we tried to compact to.
        target: u32,
    },
}

/// Result of [`compact_messages`]. Always returned — even when no
/// compaction happened (in which case `dropped_count == 0` and
/// `messages` is unchanged from the input). The `degradation` field
/// tells the caller whether the trim was clean (`None` / `NoCandidates`)
/// or whether the budget was still over after exhausting all safe
/// candidates (`StillOver` — see [`DegradationKind`]).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CompactResult {
    /// The (possibly trimmed) message list to send to the LLM.
    pub messages: Vec<ChatMessage>,
    /// Number of messages removed. `0` means no compaction was
    /// needed or no safe candidates existed.
    pub dropped_count: usize,
    /// Estimated tokens before compaction.
    pub tokens_before: u32,
    /// Estimated tokens after compaction (re-estimated on the
    /// trimmed list). Equal to `tokens_before` when
    /// `dropped_count == 0`.
    pub tokens_after: u32,
    /// What happened to the budget. `None` / `NoCandidates` are
    /// safe-to-proceed; `StillOver` MUST be surfaced as an Error
    /// by the agent loop.
    pub degradation: DegradationKind,
}

/// Estimate the total token count of a message list using the
/// process-wide cl100k_base encoder.
///
/// This is an **approximation** — the real tokenizer used by the
/// upstream model may differ by 1-2%. The compaction thresholds
/// (0.80 / 0.50) leave enough headroom to absorb this drift.
///
/// The estimate sums the visible text of every message (`role` +
/// `content.to_text()`). Tool inputs and tool results are also
/// serialized into JSON strings so they contribute to the budget.
pub async fn estimate_messages_tokens(messages: &[ChatMessage]) -> u32 {
    // Aggregate all text into a single buffer and encode once.
    // This is cheaper than per-message encoder calls (one mutex
    // acquire) and the encoding is the dominant cost.
    let mut buf = String::new();
    for m in messages {
        match m.role {
            Role::User => buf.push_str("user\n"),
            Role::Assistant => buf.push_str("assistant\n"),
        }
        // to_text() covers plain text + Text blocks. For block
        // messages with tool_use / tool_result / thinking, we also
        // serialize the JSON of the full content so the budget
        // accounts for those blocks (otherwise a giant tool_result
        // would be invisible to the estimator).
        buf.push_str(&m.content.to_text());
        if let MessageContent::Blocks(blocks) = &m.content {
            for b in blocks {
                match b {
                    ContentBlock::Text { .. } => {
                        // already counted via to_text
                    }
                    ContentBlock::ToolUse { id, name, input } => {
                        buf.push_str(id);
                        buf.push_str(name);
                        buf.push_str(&input.to_string());
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => {
                        buf.push_str(tool_use_id);
                        buf.push_str(content);
                    }
                    ContentBlock::Thinking { thinking, signature } => {
                        buf.push_str(thinking);
                        buf.push_str(&signature);
                    }
                    ContentBlock::RedactedThinking { data } => {
                        buf.push_str(data);
                    }
                }
            }
        }
        buf.push('\n');
    }
    count_tokens(&buf).await
}

/// Compact a message list to fit within the model's context window.
///
/// Algorithm (see module docs for the protection priority):
/// 1. Estimate the current token count. If `< context_window *
///    TRIGGER_RATIO`, return the input unchanged.
/// 2. Identify the permanently protected head (`messages[0..PROTECTED_HEAD]`)
///    and the protected tail (the last message, which is the current
///    user input).
/// 3. Partition the middle (`messages[PROTECTED_HEAD .. len-1]`)
///    into "turn groups": a single message or a
///    `(assistant(tool_use), user(tool_result))` pair.
/// 4. Drop turn groups from oldest first until either the budget
///    is under target or no droppable groups remain.
/// 5. Re-estimate and return.
///
/// **Pair-protection invariant**: a `tool_use` message and its
/// matching `tool_result` message are always dropped together.
/// Splitting them produces an orphan `tool_use` (no result) or
/// orphan `tool_result` (no use), both of which Anthropic rejects
/// with a 400.
///
/// **Thinking-protection invariant**: thinking / redacted_thinking
/// blocks are never orphaned. Since they live inside assistant
/// turns and a turn is dropped atomically, no signature ends up
/// without its parent turn.
pub async fn compact_messages(
    messages: Vec<ChatMessage>,
    context_window: u32,
) -> CompactResult {
    let tokens_before = estimate_messages_tokens(&messages).await;

    // Trigger threshold not reached — nothing to do.
    let trigger = trigger_threshold(context_window);
    if (tokens_before as u64) < (trigger as u64) {
        return CompactResult {
            messages,
            dropped_count: 0,
            tokens_before,
            tokens_after: tokens_before,
            degradation: DegradationKind::None,
        };
    }

    let target = target_threshold(context_window);

    // Edge case: not enough messages to compact (need at least
    // PROTECTED_HEAD + 1 protected tail + 1 droppable middle = 4).
    // With fewer, there's nothing safe to drop.
    if messages.len() <= PROTECTED_HEAD + 1 {
        return CompactResult {
            messages,
            dropped_count: 0,
            tokens_before,
            tokens_after: tokens_before,
            degradation: DegradationKind::NoCandidates,
        };
    }

    let tail_index = messages.len() - 1;

    // Build the droppable segment index: list of `(start, end)`
    // ranges (exclusive end) covering the middle, grouped into
    // turn pairs or singletons.
    let groups = group_droppable_turns(&messages, PROTECTED_HEAD, tail_index);

    // If no droppable groups exist, we can't compact.
    if groups.is_empty() {
        return CompactResult {
            messages,
            dropped_count: 0,
            tokens_before,
            tokens_after: tokens_before,
            degradation: DegradationKind::NoCandidates,
        };
    }

    // Greedy drop: oldest group first, accumulate which indices
    // to drop, until the running estimate is under target.
    //
    // We can't re-estimate on every step cheaply (encoder call
    // is ~ms), so we estimate in batches: drop a chunk of groups
    // at a time, re-estimate, and stop as soon as we cross under
    // the target. For simplicity and correctness over micro-
    // optimisation, we re-estimate after every group — the test
    // cases use synthetic messages and production runs hit this
    // at most ~10 times per chat.
    let mut dropped_indices: Vec<bool> = vec![false; messages.len()];
    let mut dropped_count: usize = 0;

    for (start, end) in &groups {
        if (estimate_messages_tokens_iter(&messages, &dropped_indices).await as u64)
            < (target as u64)
        {
            break;
        }
        for i in *start..*end {
            if !dropped_indices[i] {
                dropped_indices[i] = true;
                dropped_count += 1;
            }
        }
    }

    if dropped_count == 0 {
        return CompactResult {
            messages,
            dropped_count: 0,
            tokens_before,
            tokens_after: tokens_before,
            degradation: DegradationKind::NoCandidates,
        };
    }

    // Rebuild the message list without the dropped indices.
    let mut out = Vec::with_capacity(messages.len() - dropped_count);
    for (i, m) in messages.into_iter().enumerate() {
        if !dropped_indices[i] {
            out.push(m);
        }
    }

    let tokens_after = estimate_messages_tokens(&out).await;

    // RULE-A-002 (2026-06-14): if we exhausted every safe droppable
    // candidate but the budget is still over the target, surface
    // this as `StillOver` so the agent loop can fail loudly
    // (emit Error + abort the turn) instead of silently sending an
    // over-budget prompt that would 400 on `prompt is too long`.
    let degradation = if (tokens_after as u64) > (target as u64) {
        tracing::error!(
            tokens_after,
            target,
            dropped_count,
            "C3 compaction: all droppable exhausted but still over target"
        );
        DegradationKind::StillOver { tokens_after, target }
    } else {
        DegradationKind::None
    };

    CompactResult {
        messages: out,
        dropped_count,
        tokens_before,
        tokens_after,
        degradation,
    }
}

/// Compute the token count at which compaction triggers.
fn trigger_threshold(context_window: u32) -> u32 {
    ((context_window as f64) * TRIGGER_RATIO) as u32
}

/// Compute the token count that compaction should bring the list
/// down to.
fn target_threshold(context_window: u32) -> u32 {
    ((context_window as f64) * TARGET_RATIO) as u32
}

/// Estimate tokens across `messages`, skipping indices flagged in
/// `dropped`. Cheaper than rebuilding the Vec on every step.
async fn estimate_messages_tokens_iter(
    messages: &[ChatMessage],
    dropped: &[bool],
) -> u32 {
    let mut buf = String::new();
    for (i, m) in messages.iter().enumerate() {
        if dropped[i] {
            continue;
        }
        match m.role {
            Role::User => buf.push_str("user\n"),
            Role::Assistant => buf.push_str("assistant\n"),
        }
        buf.push_str(&m.content.to_text());
        if let MessageContent::Blocks(blocks) = &m.content {
            for b in blocks {
                match b {
                    ContentBlock::Text { .. } => {}
                    ContentBlock::ToolUse { id, name, input } => {
                        buf.push_str(id);
                        buf.push_str(name);
                        buf.push_str(&input.to_string());
                    }
                    ContentBlock::ToolResult {
                        tool_use_id, content, ..
                    } => {
                        buf.push_str(tool_use_id);
                        buf.push_str(content);
                    }
                    ContentBlock::Thinking { thinking, signature } => {
                        buf.push_str(thinking);
                        buf.push_str(&signature);
                    }
                    ContentBlock::RedactedThinking { data } => {
                        buf.push_str(data);
                    }
                }
            }
        }
        buf.push('\n');
    }
    count_tokens(&buf).await
}

/// Walk the droppable middle segment of `messages` and group
/// consecutive messages into "turn groups" that can be dropped
/// atomically. A group is either:
/// - A **pair**: an assistant message containing at least one
///   `ToolUse` block, immediately followed by a user message
///   containing at least one `ToolResult` block. These must be
///   dropped together (Anthropic 400 on orphan tool_use /
///   orphan tool_result).
/// - A **singleton**: any other single message (plain user text,
///   assistant text-only turn, assistant turn with thinking but
///   no tool_use, etc.).
///
/// **RULE-A-001 invariant (2026-06-14)**: an
/// `assistant(tool_use)` message whose matching `tool_result` is
/// NOT in the droppable middle is **implicitly protected** —
/// `group_droppable_turns` returns NO group covering it. Dropping
/// such a message as a singleton would orphan the tool_use block
/// (Anthropic 400) and could later produce an orphan tool_result
/// if the matching id surfaces in a future turn.
///
/// The returned ranges are `(start, end)` exclusive-end indices
/// into `messages`, ordered oldest-first (so the caller can drop
/// from the front until the budget is satisfied).
fn group_droppable_turns(
    messages: &[ChatMessage],
    head: usize,
    tail_index: usize,
) -> Vec<(usize, usize)> {
    let mut groups = Vec::new();
    let mut i = head;
    while i < tail_index {
        let m = &messages[i];
        if m.role == Role::Assistant && has_tool_use(m) {
            // Look ahead: is the next message a user(tool_result)?
            // Note `i + 1 <= tail_index` (not `<`): if the
            // user(tool_result) IS the protected tail, the pair
            // cannot be dropped (the tail is permanent), and the
            // assistant(tool_use) cannot be dropped alone (would
            // orphan the tail). So in that case we skip emitting
            // any group for this assistant message entirely.
            let pair_with_next = i + 1 <= tail_index
                && messages.get(i + 1).map_or(false, |n| {
                    n.role == Role::User && has_tool_result(n)
                });
            if i + 1 < tail_index && pair_with_next {
                // The tool_result sits in the droppable middle;
                // the whole pair is one atomic group.
                groups.push((i, i + 2));
                i += 2;
            } else if pair_with_next {
                // The tool_result IS the protected tail — neither
                // side can be dropped. Skip emitting any group for
                // this assistant message (it becomes implicitly
                // protected by virtue of having no droppable
                // group). Advance past it.
                i += 1;
            } else {
                // RULE-A-001 fix (2026-06-14): an
                // assistant(tool_use) message whose matching
                // tool_result is NOT in the droppable middle
                // (history truncated by an older path, edge race,
                // etc.). We MUST NOT drop this as a singleton —
                // that would orphan the tool_use block from the
                // LLM's view (Anthropic 400 "tool_use without a
                // matching tool_result"), and if a future turn
                // boundary later lands a tool_result for this id
                // in the messages vec, the orphan tool_result
                // would also 400 ("tool_result without a matching
                // tool_use"). Treat it as implicitly protected by
                // skipping (emit no group). The pre-fix code did
                // `groups.push((i, i + 1))` here; that was the
                // orphan-tool_use bug.
                //
                // Invariant: any tool_use-bearing assistant turn
                // whose tool_result pair is not ALSO in the
                // droppable middle stays. We cannot know whether
                // the tool_result is "next" in a future turn
                // boundary that compaction crossed.
                tracing::debug!(
                    index = i,
                    "C3: protecting tool_use assistant turn (no complete pair in droppable middle)"
                );
                i += 1;
            }
        } else {
            groups.push((i, i + 1));
            i += 1;
        }
    }
    groups
}

/// Does `m` contain at least one `ToolUse` content block?
fn has_tool_use(m: &ChatMessage) -> bool {
    match &m.content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. })),
        MessageContent::Text(_) => false,
    }
}

/// Does `m` contain at least one `ToolResult` content block?
fn has_tool_result(m: &ChatMessage) -> bool {
    match &m.content {
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. })),
        MessageContent::Text(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ChatMessage, ContentBlock, MessageContent, Role};

    /// Helper: build a user-text message.
    fn user(text: impl Into<String>) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Text(text.into()),
        }
    }

    /// Helper: build an assistant-text message.
    fn assistant(text: impl Into<String>) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Text(text.into()),
        }
    }

    /// Helper: build an assistant turn carrying a `tool_use`.
    fn assistant_tool_use(id: &str, name: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Text {
                    text: format!("calling {}", name),
                    cache_control: None,
                },
                ContentBlock::ToolUse {
                    id: id.to_string(),
                    name: name.to_string(),
                    input: serde_json::json!({"path": "/tmp"}),
                },
            ]),
        }
    }

    /// Helper: build a user turn carrying a `tool_result`.
    fn user_tool_result(id: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: "result body".to_string(),
                is_error: false,
            }]),
        }
    }

    /// Helper: build an assistant turn carrying a `thinking` block +
    /// text. Used to verify thinking-protection (no orphaning).
    fn assistant_with_thinking(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Thinking {
                    thinking: format!("secret reasoning for {}", text),
                    signature: format!("sig_for_{}", text),
                },
                ContentBlock::Text {
                    text: text.to_string(),
                    cache_control: None,
                },
            ]),
        }
    }

    /// Helper: a large token-padding string. cl100k_base encodes
    /// ASCII at ~4 chars/token; this string is large enough that
    /// several of them comfortably exceed a 1000-token budget.
    fn big_pad(n_chars: usize) -> String {
        "the quick brown fox jumps over the lazy dog. "
            .repeat(n_chars / 45 + 1)
            .chars()
            .take(n_chars)
            .collect()
    }

    // -----------------------------------------------------------------------
    // Case 1: no trigger — messages under 80% threshold are untouched.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn case_1_under_trigger_no_compaction() {
        let messages = vec![
            user("memory synthetic"),
            assistant("ack"),
            user("hello"),
            assistant("hi there"),
            user("what's up?"),
        ];
        let before = messages.clone();
        // context_window = 200_000 → trigger at 160_000. Our tiny
        // messages are well under.
        let result = compact_messages(messages, 200_000).await;
        assert_eq!(result.dropped_count, 0, "nothing should be dropped");
        assert_eq!(
            result.messages, before,
            "messages must be returned unchanged"
        );
        assert_eq!(
            result.tokens_before, result.tokens_after,
            "tokens unchanged when nothing dropped"
        );
    }

    // -----------------------------------------------------------------------
    // Case 2: trigger reached → trim down to target.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn case_2_trigger_brings_tokens_under_target() {
        // Build a synthetic conversation: 2 protected head + N big
        // middle pairs + 1 protected tail. Pick sizes so that the
        // total comfortably exceeds 80% of a 1000-token window but
        // dropping a few pairs brings it under 50%.
        let mut messages = Vec::new();
        messages.push(user("B5 memory instructions go here"));
        messages.push(assistant("Understood."));
        // Each turn pair ~ 100+ tokens. 10 of them → >1000 tokens,
        // well above 800 (0.80 * 1000) and well above the trim
        // target 500 (0.50 * 1000).
        for _ in 0..10 {
            messages.push(user(big_pad(800)));
            messages.push(assistant(big_pad(800)));
        }
        messages.push(user("current question"));

        let result = compact_messages(messages, 1000).await;

        // Triggered.
        assert!(
            result.tokens_before >= trigger_threshold(1000),
            "tokens_before ({}) should be >= trigger ({})",
            result.tokens_before,
            trigger_threshold(1000)
        );
        assert!(
            result.dropped_count > 0,
            "compaction should have dropped at least one message"
        );
        // The tail question must survive.
        assert_eq!(
            result.messages.last().map(|m| m.content.to_text()),
            Some("current question".to_string()),
            "current user message must survive compaction"
        );
        // The head pair must survive.
        assert!(
            result.messages.len() >= 3,
            "at least head[0..=1] + the tail user message should survive"
        );
        assert_eq!(result.messages[0].content.to_text(), "B5 memory instructions go here");
        assert_eq!(result.messages[1].content.to_text(), "Understood.");
    }

    // -----------------------------------------------------------------------
    // Case 3: pair protection — tool_use + tool_result stay paired.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn case_3_tool_use_tool_result_pair_intact_or_dropped_together() {
        let mut messages = Vec::new();
        messages.push(user("B5 memory"));
        messages.push(assistant("ack"));
        // First turn pair.
        messages.push(assistant_tool_use("tu_1", "read_file"));
        messages.push(user_tool_result("tu_1"));
        // Padding turns to push the budget over.
        for _ in 0..6 {
            messages.push(user(big_pad(800)));
            messages.push(assistant(big_pad(800)));
        }
        // Second turn pair (also droppable but it sits between
        // the head and the padding, so it'll be the FIRST
        // candidate to drop).
        messages.push(assistant_tool_use("tu_2", "grep"));
        messages.push(user_tool_result("tu_2"));
        messages.push(user("current question"));

        let result = compact_messages(messages, 1000).await;
        assert!(result.dropped_count > 0, "should compact something");

        // Walk the survivors and verify every tool_use has a
        // matching tool_result immediately after, and every
        // tool_result has a matching tool_use immediately before.
        for i in 0..result.messages.len() {
            let m = &result.messages[i];
            if m.role == Role::Assistant && has_tool_use(m) {
                // The next message must be a user(tool_result) for
                // the same id(s).
                let next = result.messages.get(i + 1);
                assert!(
                    matches!(next, Some(n) if n.role == Role::User && has_tool_result(n)),
                    "assistant(tool_use) at index {} must be immediately followed by user(tool_result) — got {:?}",
                    i,
                    next.map(|n| &n.role)
                );
            }
            if m.role == Role::User && has_tool_result(m) {
                let prev = if i == 0 { None } else { result.messages.get(i - 1) };
                assert!(
                    matches!(prev, Some(p) if p.role == Role::Assistant && has_tool_use(p)),
                    "user(tool_result) at index {} must be immediately preceded by assistant(tool_use) — got {:?}",
                    i,
                    prev.map(|n| &n.role)
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Case 4: B5 synthetic head never dropped.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn case_4_b5_synthetic_head_protected() {
        let mut messages = Vec::new();
        messages.push(user("B5_INSTRUCTIONS_MARKER"));
        messages.push(assistant("ack"));
        for _ in 0..10 {
            messages.push(user(big_pad(800)));
            messages.push(assistant(big_pad(800)));
        }
        messages.push(user("tail"));

        let result = compact_messages(messages, 1000).await;
        assert!(result.dropped_count > 0);
        // Head pair must be present.
        assert!(
            result.messages.len() >= 2,
            "head pair must survive (got {} messages)",
            result.messages.len()
        );
        assert_eq!(result.messages[0].content.to_text(), "B5_INSTRUCTIONS_MARKER");
        assert_eq!(result.messages[1].content.to_text(), "ack");
    }

    // -----------------------------------------------------------------------
    // Case 5: thinking protection — assistant turns with thinking
    // blocks are dropped atomically (no orphan signature).
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn case_5_thinking_blocks_atomic_drop() {
        let mut messages = Vec::new();
        messages.push(user("B5 memory"));
        messages.push(assistant("ack"));
        // Several assistant turns with thinking blocks.
        for i in 0..6 {
            messages.push(assistant_with_thinking(&format!("turn {}", i)));
            messages.push(user(big_pad(600)));
        }
        messages.push(user("current question"));

        let result = compact_messages(messages, 1000).await;
        assert!(result.dropped_count > 0, "should drop something");

        // Every Thinking block that survives must be intact
        // (signature matches the thinking text).
        for m in &result.messages {
            if let MessageContent::Blocks(blocks) = &m.content {
                for b in blocks {
                    if let ContentBlock::Thinking { thinking, signature } = b {
                        // The signature we constructed is
                        // "sig_for_<visible text>"; check it's still
                        // there (i.e. not split off).
                        assert!(
                            signature.starts_with("sig_for_"),
                            "signature must be intact: got {}",
                            signature
                        );
                        assert!(
                            thinking.starts_with("secret reasoning for "),
                            "thinking must be intact: got {}",
                            thinking
                        );
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Case 6: not enough messages — early return preserves the input.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn case_6_too_few_messages_to_compact() {
        // Only head + tail = 3 messages (head=2 + 1 tail = 3, but
        // we require head + tail + at least one droppable middle).
        let messages = vec![
            user("B5 memory"),
            assistant("ack"),
            user("hi"),
        ];
        // Force trigger by setting an absurdly small context_window.
        let result = compact_messages(messages, 10).await;
        assert_eq!(result.dropped_count, 0, "not enough messages to compact");
        assert_eq!(result.messages.len(), 3);
    }

    // -----------------------------------------------------------------------
    // Case 7: MAX_TURNS safety — at the max-turns boundary we still
    // preserve the contract (no panic, head + tail intact).
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn case_7_long_history_at_max_turns_compacts_safely() {
        // Simulate the worst-case agent loop: 50 turns (the new
        // MAX_TURNS cap) each producing a user/assistant pair.
        let mut messages = Vec::new();
        messages.push(user("B5 memory"));
        messages.push(assistant("ack"));
        for i in 0..100 {
            if i % 2 == 0 {
                messages.push(user(big_pad(300)));
            } else {
                messages.push(assistant(big_pad(300)));
            }
        }
        messages.push(user("tail"));

        let result = compact_messages(messages, 4000).await;
        // No panic, head + tail intact.
        assert!(result.messages.len() >= 3);
        assert_eq!(result.messages[0].content.to_text(), "B5 memory");
        assert_eq!(result.messages.last().unwrap().content.to_text(), "tail");
    }

    // -----------------------------------------------------------------------
    // Threshold helpers
    // -----------------------------------------------------------------------

    #[test]
    fn trigger_threshold_matches_prd_ratio() {
        assert_eq!(trigger_threshold(200_000), 160_000);
        assert_eq!(trigger_threshold(1000), 800);
    }

    #[test]
    fn target_threshold_matches_prd_ratio() {
        assert_eq!(target_threshold(200_000), 100_000);
        assert_eq!(target_threshold(1000), 500);
    }

    // -----------------------------------------------------------------------
    // Group detection
    // -----------------------------------------------------------------------

    #[test]
    fn group_droppable_turns_identifies_tool_pair() {
        let messages = vec![
            user("B5"),
            assistant("ack"),
            assistant_tool_use("t1", "read"),
            user_tool_result("t1"),
            assistant("done"),
            user("tail"),
        ];
        // head = 2, tail = 5 (last index).
        let groups = group_droppable_turns(&messages, 2, 5);
        // Expect:
        //   (2, 4) — the tool_use + tool_result pair
        //   (4, 5) — the singleton assistant("done")
        assert_eq!(groups, vec![(2, 4), (4, 5)]);
    }

    #[test]
    fn group_droppable_turns_singleton_for_plain_turns() {
        let messages = vec![
            user("B5"),
            assistant("ack"),
            user("q1"),
            assistant("a1"),
            user("tail"),
        ];
        let groups = group_droppable_turns(&messages, 2, 4);
        assert_eq!(groups, vec![(2, 3), (3, 4)]);
    }

    #[test]
    fn group_droppable_turns_protects_orphan_tool_use() {
        // RULE-A-001 (2026-06-14): assistant(tool_use) without a
        // following user(tool_result) is IMPLICITLY PROTECTED —
        // no group covers it. Pre-fix code emitted a singleton
        // here, which orphaned the tool_use block when dropped.
        let messages = vec![
            user("B5"),
            assistant("ack"),
            assistant_tool_use("t1", "read"),
            assistant("done"),
            user("tail"),
        ];
        let groups = group_droppable_turns(&messages, 2, 4);
        // Only the singleton assistant("done") at index 3 is
        // droppable. The assistant_tool_use at index 2 is skipped
        // (no group emitted), so it survives compaction intact.
        assert_eq!(groups, vec![(3, 4)]);
    }

    /// RULE-A-001 (2026-06-14): middle segment with an
    /// assistant(tool_use) followed by TWO plain user(text)
    /// messages — the assistant's tool_result is not present in
    /// the droppable middle. The assistant(tool_use) MUST be
    /// skipped (no group emitted), so it stays in the messages
    /// list intact. The two plain user(text) messages are normal
    /// singletons.
    #[test]
    fn group_protects_orphan_tool_use() {
        let messages = vec![
            user("B5"),
            assistant("ack"),
            assistant_tool_use("tu_orphan", "read_file"),
            user("first plain reply"),
            user("second plain reply"),
            user("tail"),
        ];
        // head = 2, tail = 5.
        let groups = group_droppable_turns(&messages, 2, 5);
        // The assistant_tool_use at index 2 is implicitly protected
        // (skipped). The two plain user(text) at indices 3 and 4 are
        // droppable singletons.
        assert_eq!(
            groups,
            vec![(3, 4), (4, 5)],
            "orphan tool_use assistant turn must NOT appear in any group"
        );
        // Sanity: index 2 is not the start of any group.
        assert!(
            !groups.iter().any(|(s, _)| *s == 2),
            "the implicitly-protected tool_use turn must not start a group"
        );
    }

    /// RULE-A-001 (2026-06-14): a complete
    /// `(assistant(tool_use), user(tool_result))` pair sitting in
    /// the droppable middle is one atomic group (start, end = i+2).
    #[test]
    fn group_drops_complete_pair() {
        let messages = vec![
            user("B5"),
            assistant("ack"),
            user("padding q"),
            assistant("padding a"),
            assistant_tool_use("tu_complete", "grep"),
            user_tool_result("tu_complete"),
            user("tail"),
        ];
        // head = 2, tail = 6.
        let groups = group_droppable_turns(&messages, 2, 6);
        // Expect the two padding singletons (2,3) (3,4) and then
        // the pair (4,6) covering both the assistant(tool_use) and
        // the user(tool_result).
        assert_eq!(groups, vec![(2, 3), (3, 4), (4, 6)]);
    }

    /// RULE-A-001 (2026-06-14): an `assistant(tool_use)` whose
    /// following message is NOT a user(tool_result) but the tail
    /// is plain user(text). The assistant(tool_use) MUST be
    /// skipped (no group emitted), so it's protected alongside
    /// the protected tail.
    #[test]
    fn group_protects_tool_use_at_tail() {
        let messages = vec![
            user("B5"),
            assistant("ack"),
            user("padding"),
            assistant_tool_use("tu_tail_adjacent", "read_file"),
            user("tail plain text"),
        ];
        // head = 2, tail = 4.
        let groups = group_droppable_turns(&messages, 2, 4);
        // Only the padding user(text) at index 2 is droppable.
        // The assistant_tool_use at index 3 is implicitly protected
        // — its tool_result is NOT in the middle (the next message
        // is plain user text, not a tool_result), and the tail at
        // index 4 is also plain text (not a tool_result).
        assert_eq!(groups, vec![(2, 3)]);
    }

    // -----------------------------------------------------------------------
    // Encoder warm-up
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn estimate_tokens_returns_nonzero_for_real_text() {
        crate::memory::tokens::ensure_initialized().await;
        let tokens = estimate_messages_tokens(&[user("hello world this is a test")]).await;
        assert!(tokens > 0, "should return > 0 tokens for real text");
    }

    // -----------------------------------------------------------------------
    // Regression: pair at tail boundary — if the last two messages
    // form an assistant(tool_use) + user(tool_result) pair and
    // aggressive compaction is required, the pair must NOT be split.
    // The PROTECTED_TAIL covers the user(tool_result); the
    // assistant(tool_use) is implicitly protected because the pair
    // spans the tail boundary (`i+1 == tail_index` branch — neither
    // side is droppable). RULE-A-001 (2026-06-14) keeps this
    // invariant intact.
    //
    // Note: this scenario produces `DegradationKind::StillOver`
    // because the budget is still over target after the padding is
    // dropped but the tail pair can't be touched. The agent loop
    // MUST surface `StillOver` as an Error (RULE-A-002). This test
    // only asserts pair integrity; the `StillOver` signal is
    // covered by `compact_emits_still_over_degradation` below.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn regression_pair_at_tail_split_under_pressure() {
        // Construct a scenario where ALL middle messages must be
        // dropped AND the algorithm would consider dropping the
        // final assistant(tool_use). Padding size is tuned so the
        // total barely exceeds target after middle drops.
        let mut messages = Vec::new();
        messages.push(user("B5"));
        messages.push(assistant("ack"));
        // Just enough padding that compaction triggers, but small
        // enough that after dropping all middle pairs the estimate
        // is still over target (forcing the algorithm to consider
        // the tail-adjacent pair).
        for _ in 0..4 {
            messages.push(user(big_pad(800)));
            messages.push(assistant(big_pad(800)));
        }
        // Final pair just before the protected tail.
        messages.push(assistant_tool_use("tu_final", "grep"));
        messages.push(user_tool_result("tu_final"));

        // context_window = 10 — target = 5. Head + final pair
        // alone (4 messages) total > 5 tokens; padding must be
        // dropped but the algorithm will then consider the tail
        // pair's assistant(tool_use) singleton droppable.
        let result = compact_messages(messages, 10).await;
        // Walk survivors and check pair integrity.
        for i in 0..result.messages.len() {
            let m = &result.messages[i];
            if m.role == Role::Assistant && has_tool_use(m) {
                let next = result.messages.get(i + 1);
                assert!(
                    matches!(next, Some(n) if n.role == Role::User && has_tool_result(n)),
                    "assistant(tool_use) at index {} must be immediately followed by user(tool_result) — got {:?}",
                    i,
                    next.map(|n| &n.role)
                );
            }
            if m.role == Role::User && has_tool_result(m) {
                let prev = if i == 0 { None } else { result.messages.get(i - 1) };
                assert!(
                    matches!(prev, Some(p) if p.role == Role::Assistant && has_tool_use(p)),
                    "user(tool_result) at index {} must be immediately preceded by assistant(tool_use) — got {:?}",
                    i,
                    prev.map(|n| &n.role)
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // RULE-A-002 (2026-06-14): compact_messages surfaces
    // `DegradationKind::StillOver` when it runs out of safe droppable
    // candidates but the budget is still over the target. The agent
    // loop is expected to turn this into an Error event (covered by
    // `agent/tests.rs::agent_loop_*` integration tests), NOT to
    // silently send the over-budget prompt.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn compact_emits_still_over_degradation() {
        // Construct a messages list where the only droppable middle
        // message is small but the protected tail is HUGE, so that
        // after dropping the middle the budget is still over the
        // (artificially small) target.
        //
        // Layout: head[2 small] + middle[1 small droppable] +
        //         tail[1 huge user text].
        // context_window = 1000 → trigger = 800, target = 500.
        // The huge tail alone is well over 500 tokens.
        let mut messages = Vec::new();
        messages.push(user("B5 memory instructions"));
        messages.push(assistant("ack"));
        // One droppable middle user(text) message — small but
        // still needs to be the FIRST candidate so it gets dropped.
        messages.push(user("droppable middle"));
        // Protected tail: huge user text (> target 500 tokens).
        messages.push(user(big_pad(8_000)));

        let result = compact_messages(messages, 1000).await;

        // The middle "droppable" message must have been dropped.
        assert!(
            result.dropped_count >= 1,
            "expected the middle message to be dropped (got dropped_count={})",
            result.dropped_count
        );
        // The huge tail must survive (it's protected).
        assert!(
            result
                .messages
                .iter()
                .any(|m| m.role == Role::User && m.content.to_text().contains("lazy dog")),
            "the huge protected tail must survive compaction"
        );
        // The degradation signal MUST be StillOver — the budget is
        // still over target after exhausting the only droppable
        // candidate.
        match &result.degradation {
            DegradationKind::StillOver {
                tokens_after,
                target,
            } => {
                assert!(
                    *tokens_after > *target,
                    "tokens_after ({}) must be > target ({})",
                    tokens_after,
                    target
                );
                assert_eq!(*target, target_threshold(1000));
            }
            other => panic!(
                "expected DegradationKind::StillOver, got {:?}",
                other
            ),
        }
    }

    /// Companion: when no compaction is needed (under the trigger),
    /// the result's `degradation` is `None`. Sanity-checks the
    /// "happy path" branch so the enum wiring is exercised both
    /// ways in context.rs unit tests.
    #[tokio::test]
    async fn compact_emits_none_when_under_trigger() {
        let messages = vec![
            user("B5 memory"),
            assistant("ack"),
            user("hello"),
            assistant("hi"),
            user("current"),
        ];
        // context_window = 200_000 → trigger = 160_000. The tiny
        // messages are well under the trigger so compaction is a
        // no-op.
        let result = compact_messages(messages, 200_000).await;
        assert_eq!(result.dropped_count, 0);
        assert_eq!(result.degradation, DegradationKind::None);
    }
}
