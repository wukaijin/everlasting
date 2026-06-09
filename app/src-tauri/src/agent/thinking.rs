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