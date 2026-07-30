//! Per-turn thinking-block accumulator.
//!
//! Step 6 (extended-thinking support): the SSE parser emits
//! `ThinkingDelta` / `SignatureDelta` / `RedactedThinkingDelta`
//! events as the model streams. The agent loop holds these in a
//! per-turn [`PendingThinking`] struct, finalizing into a
//! `ContentBlock::Thinking` as soon as the model moves on to a
//! text / tool_use block (and always flushing whatever's still
//! pending at the end of the turn).

/// Per-turn accumulator for a single in-flight thinking block. We
/// finalize into a `ContentBlock::Thinking` (or push into
/// `finalized_thinking`) as soon as the model moves on to a text /
/// tool_use block, and we always flush whatever's still pending at
/// the end of the turn.
#[derive(Default)]
pub struct PendingThinking {
    pub text: String,
    pub signature: String,
}

/// Move whatever's currently in `pending` into `finalized` as a
/// `(text, signature)` pair. Called on every `Delta` / `ToolCall`
/// event AND at the end of the turn so an unfinished thinking
/// block (signature received but no subsequent text/tool_use to
/// flush it) is still captured.
pub fn flush_pending_thinking(
    pending: &mut Option<PendingThinking>,
    finalized: &mut Vec<(String, String)>,
) {
    if let Some(p) = pending.take() {
        // We persist even if text is empty — what matters is
        // that the signature is preserved verbatim, so the LLM
        // can validate the round-trip. A thinking block whose
        // text was streamed as empty (e.g. `display: "omitted"`)
        // is still a valid block.
        finalized.push((p.text, p.signature));
    }
}

/// Flush every finalized thinking pair (text, signature) into the
/// ordered block list as `ContentBlock::Thinking`, **in finalize
/// order**. Used by the interleaved-thinking path: each time a
/// thinking block is finalized (at a thinking→text or thinking→tool
/// boundary), it is appended to `ordered_blocks` so the persisted
/// `content` array keeps the real stream order
/// (`[think → text → tool]` instead of the old hard-coded
/// `[all-think → text → all-tool]`).
///
/// `finalized` is drained in place — a thinking pair enters
/// `ordered_blocks` exactly once, at the first boundary after it was
/// finalized. If multiple thinking blocks were finalized before a
/// single boundary (rare — the model rarely emits thinking without an
/// intervening text/tool event), they land in finalize order, which
/// equals stream order.
pub fn flush_ordered_thinking(
    finalized: &mut Vec<(String, String)>,
    ordered_blocks: &mut Vec<crate::llm::types::ContentBlock>,
) {
    for (thinking, signature) in finalized.drain(..) {
        ordered_blocks.push(crate::llm::types::ContentBlock::Thinking {
            thinking,
            signature,
        });
    }
}

/// Flush the currently-accumulating text (`pending_text`) into
/// `ordered_blocks` as one `ContentBlock::Text`, if non-empty.
/// `Delta` events arrive in fragments; rather than pushing one
/// `Text` block per fragment (which would shatter the content into
/// many tiny blocks), we accumulate into `pending_text` and flush it
/// as a single `Text` block at the next non-text boundary
/// (thinking / tool / redacted / turn end). This preserves the
/// stream order — when thinking appears between two text runs, the
/// two runs become two distinct `Text` blocks straddling the
/// `Thinking` block — while avoiding block fragmentation within a
/// contiguous text run.
pub fn flush_pending_text(
    pending_text: &mut Option<String>,
    ordered_blocks: &mut Vec<crate::llm::types::ContentBlock>,
) {
    if let Some(text) = pending_text.take() {
        if !text.is_empty() {
            ordered_blocks.push(crate::llm::types::ContentBlock::Text {
                text,
                cache_control: None,
            });
        }
    }
}
