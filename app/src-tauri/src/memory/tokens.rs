//! Token estimation for memory files.
//!
//! V2 1 期 uses `tiktoken-rs` with the `cl100k_base` encoding —
//! OpenAI's GPT-3.5/4 tokenizer. This is good enough for a
//! 4-file ≤2K-each memory budget (the PRD's "no hard cap" decision
//! means we're not making fine-grained budget decisions, just
//! displaying "X tokens" in the preview UI). The cost of using a
//! per-model tokenizer (e.g. cl200k for Claude) is
//! disproportionate — `tiktoken-rs` is a single encoding
//! initialised once at startup.
//!
//! Why not Anthropic's tokenizer? Anthropic does not ship an
//! official tokenizer; the community has reverse-engineered
//! approximations, but they are not part of any official API.
//! The 1-2% drift from cl100k_base is invisible at this scale
//! (the UI is `< 100` tokens for typical CLAUDE.md files).
//!
//! **Caching the encoder**: `tiktoken-rs::cl100k_base()` returns a
//! `CoreBPE` that is expensive to build (~200ms on first call).
//! We hold one instance in a `OnceCell` and clone-handle every
//! encode call. The encoder itself is `!Send` because BPE state
//! is not thread-safe — we wrap it in a `tokio::sync::Mutex` so
//! the `count_tokens` public function is `async` and the lock is
//! dropped between calls.

use std::sync::OnceLock;

use tiktoken_rs::{cl100k_base, CoreBPE};
use tokio::sync::Mutex;

/// Process-wide cl100k_base encoder, lazily initialised on the
/// first `count_tokens` call. Building the BPE table is a
/// ~200ms one-time cost; subsequent calls amortise to <1µs per
/// token.
static ENCODER: OnceLock<Mutex<CoreBPE>> = OnceLock::new();

/// Count tokens in `text` using cl100k_base.
///
/// **Async signature**: the encoder is held under a
/// `tokio::sync::Mutex` because the underlying `CoreBPE` is
/// `!Send`. The lock is held only for the duration of the encode
/// call (microseconds), so contention is negligible.
///
/// Returns 0 for the empty string. The encoder does not raise on
/// pathological inputs (no control characters, no overflow); an
/// out-of-memory situation is the only failure mode and it
/// propagates as a panic in the BPE internals — acceptable
/// because a 100 KiB file at 1 token / 4 chars is at most 25K
/// tokens, well within memory.
pub async fn count_tokens(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    let mutex = ENCODER.get_or_init(|| {
        let bpe = cl100k_base().expect("cl100k_base initialization should not fail");
        Mutex::new(bpe)
    });
    let guard = mutex.lock().await;
    let tokens = guard.encode_ordinary(text);
    tokens.len() as u32
}

/// Try-init the encoder eagerly at startup. Optional — the lazy
/// `OnceLock` initialisation in `count_tokens` covers the
/// production case. This helper exists for the test suite so
/// the test fixture does not race the lazy init.
#[cfg(test)]
pub async fn ensure_initialized() {
    let _ = count_tokens("warmup").await;
}
