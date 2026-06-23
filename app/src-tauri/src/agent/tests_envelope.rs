#![cfg(test)]

use crate::agent::helpers::tool_result_envelope;
use crate::agent::thinking::{flush_pending_thinking, PendingThinking};

/// exactly the shape `{"result": <content>, "cwd": <path>}`.
/// This is the LLM-facing contract — the LLM gets the cwd so
/// it can correlate tool results with the worktree state.
/// The frontend's `extractToolResultDisplay` parses this same
/// shape; a regression here would leak the raw JSON into the
/// UI.
#[test]
fn tool_result_envelope_round_trip() {
    let path = std::path::Path::new("/data/worktrees/p1/s1");
    let env = tool_result_envelope("hello world", path);
    let parsed: serde_json::Value = serde_json::from_str(&env).expect("envelope must be JSON");
    assert_eq!(parsed["result"], "hello world");
    assert_eq!(parsed["cwd"], "/data/worktrees/p1/s1");
    // No extra top-level keys — schema discipline matters
    // because the LLM is reading this.
    assert_eq!(
        parsed.as_object().unwrap().len(),
        2,
        "envelope must have exactly 2 keys: result, cwd"
    );
}

/// Step 4 follow-up: empty / unicode / special-char content
/// all round-trip cleanly through the envelope. (Sanity — the
/// envelope is built with `serde_json::json!` which handles
/// escaping, but a hand-written string would not.)
#[test]
fn tool_result_envelope_handles_special_chars() {
    let path = std::path::Path::new("/data/wt");
    // Newline, quote, and backslash in the content.
    let content = "line 1\nline 2 with \"quote\" and \\ slash";
    let env = tool_result_envelope(content, path);
    let parsed: serde_json::Value = serde_json::from_str(&env).expect("envelope must be JSON");
    assert_eq!(parsed["result"], content);
    assert_eq!(parsed["cwd"], "/data/wt");
}

// ---------------------------------------------------------------------------
// flush_pending_thinking
// ---------------------------------------------------------------------------

/// A pending thinking block with both text and signature is
/// moved into the finalized vec on flush.
#[test]
fn flush_pending_thinking_moves_into_finalized() {
    let mut pending = Some(PendingThinking {
        text: "reasoning text".to_string(),
        signature: "sig-blob".to_string(),
    });
    let mut finalized: Vec<(String, String)> = Vec::new();
    flush_pending_thinking(&mut pending, &mut finalized);
    assert!(pending.is_none(), "pending should be cleared after flush");
    assert_eq!(finalized.len(), 1);
    assert_eq!(finalized[0].0, "reasoning text");
    assert_eq!(finalized[0].1, "sig-blob");
}

/// A no-op when pending is None (already flushed).
#[test]
fn flush_pending_thinking_noop_when_already_flushed() {
    let mut pending: Option<PendingThinking> = None;
    let mut finalized: Vec<(String, String)> = Vec::new();
    flush_pending_thinking(&mut pending, &mut finalized);
    assert!(pending.is_none());
    assert!(finalized.is_empty());
}

