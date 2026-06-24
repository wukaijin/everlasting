//! Agent loop ⑬ — loop detection (anti death-loop).
//!
//! ARCHITECTURE §2.2 ⑬ + §2.5.4 "⑬ 循环检测阈值" reserved this
//! gate but it had zero implementation in `chat_loop.rs`; the only
//! prior safety net was the coarse `MAX_TURNS=200` turn cap. This
//! module is the behaviour-based early detector that runs *before*
//! MAX_TURNS, catching the model stuck repeating the same tool call.
//!
//! ## Algorithm (replaces the doc's single Jaccard > 0.9)
//!
//! A single threshold cannot fit both short inputs (`read_file` has
//! just one `path` token → Jaccard twitches to 0.5 on any change)
//! and long ones (a `shell` command stays > 0.9 after tweaking a
//! flag). So we use a **two-level** scheme (see
//! `research/similarity-algorithm-and-tokenizer.md`):
//!
//! - **Level 1 — exact signature (hard trigger, zero false-positive)**:
//!   a run of `HARD_WINDOW` (3) consecutive tool calls whose
//!   normalized signatures are byte-identical. Real death-loops are
//!   almost always byte-identical, so this catches the common case
//!   (repeated `read_file` / `grep` / `shell` with the same args)
//!   with no false alarms.
//! - **Level 2 — Jaccard soft hint (tolerant of noise)**: within a
//!   `SOFT_WINDOW` (5) window, at least `SOFT_PAIR_MIN` (2) pairs of
//!   calls whose token-set Jaccard similarity exceeds
//!   `SOFT_THRESHOLD` (0.85). Handles "near-duplicate" loops, mainly
//!   long `shell` commands with minor flag drift.
//!
//! On either hit the action is **soft** (per §2.5.4 "不强制打断"):
//! [`LoopVerdict::hint_text`] produces a synthetic `tool_result` that
//! is fed back to the LLM so it can self-correct; the loop is *not*
//! terminated (MAX_TURNS remains the hard backstop).
//!
//! ## Tokenizer
//!
//! Tokenization is a plain-std `split_whitespace` + punctuation
//! stripping (`tokenize_for_jaccard`), deliberately **not** reusing
//! `memory::tokens::count_tokens` (tiktoken cl100k_base): that path
//! is `async` + holds a `tokio::sync::Mutex`, splits CJK into noise,
//! and BPE subwords add noise at this coarse threshold. The two
//! "token" concepts are physically isolated.
//!
//! ## Deviation from research caveat #3
//!
//! `edit_file` signature includes `old_string` (research said
//! "exclude old_string to allow same-file different-position edits" —
//! but excluding it makes legitimate multi-block edits to one file
//! look identical and false-trigger). Including `old_string` is what
//! actually lets same-file/different-block edits stay distinct while
//! still catching the true loop of repeatedly failing the *same*
//! `old_string`.

use std::collections::HashSet;

/// Sliding window size for Level 1 (exact-signature hard trigger):
/// 3 consecutive identical-signature calls.
pub const HARD_WINDOW: usize = 3;
/// Sliding window size for Level 2 (Jaccard soft hint).
pub const SOFT_WINDOW: usize = 5;
/// Jaccard token-set similarity threshold for Level 2.
pub const SOFT_THRESHOLD: f64 = 0.85;
/// Minimum number of similar pairs within the window to trigger Level 2.
const SOFT_PAIR_MIN: usize = 2;

/// A `(name, input)` pair extracted from a `ContentBlock::ToolUse`
/// for loop detection. The tool_use `id` is intentionally excluded —
/// it is freshly generated on every call and must never participate
/// in similarity (otherwise no two calls could ever match).
///
/// PR1 keeps this decoupled from `ContentBlock` so the detection
/// logic is pure and unit-testable in isolation; PR2 builds these
/// from the assistant turn's tool_use blocks inside `run_chat_loop`.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    pub input: serde_json::Value,
}

impl ToolCall {
    /// Convenience constructor (mainly for tests and PR2 wiring).
    pub fn new(name: impl Into<String>, input: serde_json::Value) -> Self {
        ToolCall {
            name: name.into(),
            input,
        }
    }
}

/// The loop-detection verdict for a window of tool calls.
#[derive(Debug, Clone, PartialEq)]
pub enum LoopVerdict {
    /// No loop detected.
    None,
    /// Level 1: `count` consecutive trailing calls share an identical
    /// signature. High confidence, zero false-positive.
    HardLoop { tool: String, count: usize },
    /// Level 2: `pairs` calls within the window are near-duplicates
    /// (Jaccard > [`SOFT_THRESHOLD`]); `max_jaccard` is the highest
    /// pairwise similarity seen.
    SoftLoop { pairs: usize, max_jaccard: f64 },
}

impl LoopVerdict {
    /// Whether any loop was detected.
    ///
    /// `chat_loop` currently keys off [`Self::hint_text`] (it needs the
    /// message text anyway), so this predicate is retained as a
    /// convenience for future callers and tests rather than a hot path.
    #[allow(dead_code)]
    pub fn is_loop(&self) -> bool {
        !matches!(self, LoopVerdict::None)
    }

    /// The synthetic `tool_result` text to feed back to the LLM, or
    /// `None` when no loop was detected. Level 1 is assertive, Level 2
    /// is tentative (lets the model explain intentional progress).
    pub fn hint_text(&self) -> Option<String> {
        match self {
            LoopVerdict::None => None,
            LoopVerdict::HardLoop { tool, count } => Some(format!(
                "loop detected: you have called `{}` with identical arguments {} {} \
                 in a row. Reconsider your approach or stop — repeating the same call \
                 will not make progress.",
                tool,
                count,
                if *count == 1 { "time" } else { "times" }
            )),
            LoopVerdict::SoftLoop { max_jaccard, .. } => Some(format!(
                "loop suspected: recent tool calls look very similar (Jaccard {:.2} > \
                 0.85). If this is intentional progress, briefly explain why; otherwise \
                 try a different approach.",
                max_jaccard
            )),
        }
    }
}

/// Run the two-level loop detection over a window of tool calls.
///
/// The window is whatever the caller currently holds (PR2 maintains a
/// `SOFT_WINDOW`-sized sliding window). Level 1 takes precedence: a
/// hard loop is reported even if Level 2 would also fire.
pub fn detect(window: &[ToolCall]) -> LoopVerdict {
    if window.is_empty() {
        return LoopVerdict::None;
    }

    // --- Level 1: exact-signature run ending at the last call -------
    let sigs: Vec<String> = window.iter().map(signature_of).collect();
    let last_sig = sigs.last().expect("window is non-empty");
    let tail_run = sigs
        .iter()
        .rev()
        .take_while(|s| *s == last_sig)
        .count();
    if tail_run >= HARD_WINDOW {
        return LoopVerdict::HardLoop {
            tool: window.last().expect("window is non-empty").name.clone(),
            count: tail_run,
        };
    }

    // --- Level 2: pairwise Jaccard soft hint -------------------------
    let token_sets: Vec<HashSet<String>> = window
        .iter()
        .map(|c| tokenize_for_jaccard(&serialize_for_similarity(c)))
        .collect();
    let mut pairs = 0usize;
    let mut max_j = 0.0f64;
    for i in 0..token_sets.len() {
        for j in (i + 1)..token_sets.len() {
            let sim = jaccard(&token_sets[i], &token_sets[j]);
            if sim > SOFT_THRESHOLD {
                pairs += 1;
                if sim > max_j {
                    max_j = sim;
                }
            }
        }
    }
    if pairs >= SOFT_PAIR_MIN {
        return LoopVerdict::SoftLoop {
            pairs,
            max_jaccard: max_j,
        };
    }

    LoopVerdict::None
}

/// Per-tool semantic signature. Only the 6 high-frequency tools get a
/// custom extractor; everything else falls back to
/// `name + canonical(input)`. Two calls with the same signature are
/// considered "the same operation on the same object".
fn signature_of(call: &ToolCall) -> String {
    let name = call.name.as_str();
    let input = &call.input;
    let path = || input.get("path").and_then(|v| v.as_str()).unwrap_or("");
    match name {
        // Path-centric reads/writes: repeated ops on the same path.
        "read_file" | "write_file" | "list_dir" => format!("{}:{}", name, path()),
        "grep" | "glob" => {
            let pattern = input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("{}:{}:{}", name, pattern, path())
        }
        // edit_file includes old_string: same-file/different-block edits
        // stay distinct (no false-positive), while repeatedly failing
        // the SAME old_string (true loop) collapses. See module docs.
        "edit_file" => {
            let old = input
                .get("old_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("{}:{}:{}", name, path(), old)
        }
        "shell" | "run_background_shell" => {
            let command = input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("{}:{}", name, command)
        }
        _ => format!("{}:{}", name, canonical_json(input)),
    }
}

/// Canonical, key-sorted JSON serialization so that two semantically
/// equal inputs produce the same string regardless of map key order
/// (LLM non-determinism + serde_json's map ordering must not leak
/// into the signature).
fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", k, canonical_json(&map[k])))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(canonical_json).collect();
            format!("[{}]", parts.join(","))
        }
        _ => value.to_string(),
    }
}

/// Serialize a call for Jaccard tokenization: `name` + canonical
/// input. Key order is normalized so tokenization is stable.
fn serialize_for_similarity(call: &ToolCall) -> String {
    format!("{} {}", call.name, canonical_json(&call.input))
}

/// Tokenize for Jaccard similarity: `split_whitespace`, then
/// `trim_matches` to strip *leading/trailing* punctuation (CLI flag
/// dashes, quotes, brackets, dots, colons …) while preserving
/// *internal* `_/-/.` so identifiers (`read_file`), paths
/// (`/usr/local/x.rs`), and hyphenated words (`kebab-case`) stay
/// single semantic tokens. Lowercase, drop empties.
///
/// Implementation note: this is `trim_matches`, not a full
/// `split_on_punctuation` (PRD-known deviation from the original
/// research pseudocode). The full-split approach collapsed `--flag`
/// and `...` into single tokens — `trim_matches` peels CLI flags
/// (`--flag` → `flag`) and pure-punctuation strings (`...` → empty)
/// correctly while leaving internal punctuation intact.
fn tokenize_for_jaccard(s: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    for word in s.split_whitespace() {
        // Trim leading/trailing punctuation (CLI flag dashes, quotes,
        // brackets, dots, colons …) while preserving *internal*
        // `_/-.` so identifiers (`read_file`), paths
        // (`/usr/local/x.rs`) and hyphenated words (`kebab-case`)
        // stay single semantic tokens. A leading `/` (absolute path)
        // is intentionally kept — it is not in the trim set.
        let trimmed = word.trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\'' | '`' | '-' | '.' | ':' | '=' | ',' | ';' | '!'
                    | '?' | '(' | ')' | '[' | ']' | '{' | '}'
            )
        });
        if !trimmed.is_empty() {
            set.insert(trimmed.to_lowercase());
        }
    }
    set
}

/// Jaccard similarity `|A ∩ B| / |A ∪ B|` over two token sets.
/// Two empty sets are defined as identical (1.0); one empty set vs a
/// non-empty set is 0.0.
fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count();
    let union = a.len() + b.len() - inter;
    if union == 0 {
        return 1.0;
    }
    inter as f64 / union as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall::new(name, input)
    }

    // --- LoopVerdict -----------------------------------------------------

    #[test]
    fn verdict_none_is_not_loop_and_has_no_hint() {
        assert!(!LoopVerdict::None.is_loop());
        assert_eq!(LoopVerdict::None.hint_text(), None);
    }

    #[test]
    fn hard_and_soft_are_loops_with_hints() {
        let hard = LoopVerdict::HardLoop {
            tool: "read_file".into(),
            count: 3,
        };
        assert!(hard.is_loop());
        let hint = hard.hint_text().unwrap();
        assert!(hint.contains("read_file"));
        assert!(hint.contains("3 times"));

        let soft = LoopVerdict::SoftLoop {
            pairs: 2,
            max_jaccard: 0.9,
        };
        assert!(soft.is_loop());
        assert!(soft.hint_text().unwrap().contains("suspected"));
    }

    #[test]
    fn hard_hint_singular_count_says_time() {
        let hard = LoopVerdict::HardLoop {
            tool: "shell".into(),
            count: 1,
        };
        assert!(hard.hint_text().unwrap().contains("1 time"));
    }

    // --- Level 1: exact signature ---------------------------------------

    #[test]
    fn empty_window_is_none() {
        assert_eq!(detect(&[]), LoopVerdict::None);
    }

    #[test]
    fn no_repetition_is_none() {
        let w = vec![
            call("read_file", json!({"path": "/a"})),
            call("grep", json!({"pattern": "foo", "path": "/b"})),
            call("shell", json!({"command": "ls"})),
        ];
        assert_eq!(detect(&w), LoopVerdict::None);
    }

    #[test]
    fn three_identical_reads_trigger_hard() {
        let w = vec![
            call("read_file", json!({"path": "/a.rs", "limit": 10})),
            call("read_file", json!({"path": "/a.rs", "limit": 20})),
            call("read_file", json!({"path": "/a.rs", "limit": 30})),
        ];
        // signature is path-only → identical across the 3 despite different limit
        assert_eq!(
            detect(&w),
            LoopVerdict::HardLoop {
                tool: "read_file".into(),
                count: 3
            }
        );
    }

    #[test]
    fn two_identical_do_not_trigger() {
        let w = vec![
            call("read_file", json!({"path": "/a"})),
            call("read_file", json!({"path": "/a"})),
        ];
        assert_eq!(detect(&w), LoopVerdict::None);
    }

    #[test]
    fn trailing_different_call_resets_run() {
        // read A, read A, read B → the trailing run is "read B" (count 1)
        let w = vec![
            call("read_file", json!({"path": "/a"})),
            call("read_file", json!({"path": "/a"})),
            call("read_file", json!({"path": "/b"})),
        ];
        assert_eq!(detect(&w), LoopVerdict::None);
    }

    #[test]
    fn hard_triggers_at_window_of_exactly_three() {
        // exactly 3 identical signatures at the tail → trigger
        let w = vec![
            call("grep", json!({"pattern": "x", "path": "/y"})),
            call("grep", json!({"pattern": "x", "path": "/y"})),
            call("grep", json!({"pattern": "x", "path": "/y"})),
        ];
        assert!(matches!(detect(&w), LoopVerdict::HardLoop { count: 3, .. }));
    }

    #[test]
    fn hard_count_reflects_actual_run_length() {
        // 4 identical → count 4
        let w = vec![
            call("read_file", json!({"path": "/a"})),
            call("read_file", json!({"path": "/a"})),
            call("read_file", json!({"path": "/a"})),
            call("read_file", json!({"path": "/a"})),
        ];
        assert!(matches!(detect(&w), LoopVerdict::HardLoop { count: 4, .. }));
    }

    // --- edit_file old_string discrimination (deviation from research) --

    #[test]
    fn edit_same_file_different_old_string_is_not_loop() {
        // Same file, 3 different blocks → distinct signatures → no loop.
        // This is the legitimate multi-block-edit workflow.
        let w = vec![
            call(
                "edit_file",
                json!({"path": "/a.rs", "old_string": "fn old1()"}),
            ),
            call(
                "edit_file",
                json!({"path": "/a.rs", "old_string": "fn old2()"}),
            ),
            call(
                "edit_file",
                json!({"path": "/a.rs", "old_string": "fn old3()"}),
            ),
        ];
        assert_eq!(detect(&w), LoopVerdict::None);
    }

    #[test]
    fn edit_same_file_same_old_string_is_loop() {
        // Repeatedly failing the SAME old_string → identical signature → loop.
        let w = vec![
            call(
                "edit_file",
                json!({"path": "/a.rs", "old_string": "fn old()"}),
            ),
            call(
                "edit_file",
                json!({"path": "/a.rs", "old_string": "fn old()"}),
            ),
            call(
                "edit_file",
                json!({"path": "/a.rs", "old_string": "fn old()"}),
            ),
        ];
        assert!(matches!(detect(&w), LoopVerdict::HardLoop { .. }));
    }

    // --- Level 2: Jaccard soft hint -------------------------------------

    #[test]
    fn soft_loop_on_repeated_non_consecutive() {
        // cmdA and cmdB each appear twice but NOT consecutively, so
        // Level 1 (exact-signature run) does not fire — yet the two
        // identical pairs score Jaccard 1.0 > 0.85, yielding ≥ 2
        // qualifying pairs → Level 2 soft hint.
        let w = vec![
            call("shell", json!({"command": "ls -la /tmp"})),
            call("shell", json!({"command": "cat /etc/hosts"})),
            call("shell", json!({"command": "ls -la /tmp"})),
            call("shell", json!({"command": "cat /etc/hosts"})),
            call("shell", json!({"command": "echo done"})),
        ];
        match detect(&w) {
            LoopVerdict::SoftLoop { pairs, max_jaccard } => {
                assert!(pairs >= SOFT_PAIR_MIN);
                assert!((max_jaccard - 1.0).abs() < 1e-9);
            }
            other => panic!("expected SoftLoop, got {:?}", other),
        }
    }

    #[test]
    fn soft_loop_near_duplicate_above_threshold() {
        // X vs Y share 6 of 7 tokens → Jaccard 6/7 ≈ 0.857 > 0.85.
        // Window [X, Y, X]: tail run is 1 (Y breaks it) so L1 stays
        // quiet, but 3 pairs qualify (X-Y, X-X, Y-X) → soft hint.
        let x = "npm run build --mode production --env staging";
        let y = "npm run build --mode production --env production";
        let w = vec![
            call("shell", json!({"command": x})),
            call("shell", json!({"command": y})),
            call("shell", json!({"command": x})),
        ];
        assert!(matches!(detect(&w), LoopVerdict::SoftLoop { .. }));
    }

    #[test]
    fn soft_not_triggered_similar_but_below_threshold() {
        // 5 commands share 3 of 5 tokens → Jaccard 0.6 for every pair,
        // all below 0.85. No qualifying pair → no soft hint. (Proves
        // the threshold is not just "any similarity".)
        let w = vec![
            call("shell", json!({"command": "echo hello world foo"})),
            call("shell", json!({"command": "echo hello world bar"})),
            call("shell", json!({"command": "echo hello world baz"})),
            call("shell", json!({"command": "echo hello world qux"})),
            call("shell", json!({"command": "echo hello world quux"})),
        ];
        assert_eq!(detect(&w), LoopVerdict::None);
    }

    #[test]
    fn hard_takes_precedence_over_soft() {
        // 5 identical calls satisfy BOTH levels → Hard wins.
        let w = vec![
            call("read_file", json!({"path": "/a"})),
            call("read_file", json!({"path": "/a"})),
            call("read_file", json!({"path": "/a"})),
            call("read_file", json!({"path": "/a"})),
            call("read_file", json!({"path": "/a"})),
        ];
        assert!(matches!(detect(&w), LoopVerdict::HardLoop { .. }));
    }

    // --- signature_of ----------------------------------------------------

    #[test]
    fn signature_path_tools_use_path() {
        assert_eq!(
            signature_of(&call("read_file", json!({"path": "/a.rs"}))),
            "read_file:/a.rs"
        );
        assert_eq!(
            signature_of(&call("list_dir", json!({"path": "/d"}))),
            "list_dir:/d"
        );
    }

    #[test]
    fn signature_grep_uses_pattern_and_path() {
        assert_eq!(
            signature_of(&call("grep", json!({"pattern": "foo", "path": "/b"}))),
            "grep:foo:/b"
        );
        assert_eq!(
            signature_of(&call("glob", json!({"pattern": "*.rs", "path": "/c"}))),
            "glob:*.rs:/c"
        );
    }

    #[test]
    fn signature_edit_includes_old_string() {
        assert_eq!(
            signature_of(&call(
                "edit_file",
                json!({"path": "/a.rs", "old_string": "x"})
            )),
            "edit_file:/a.rs:x"
        );
    }

    #[test]
    fn signature_shell_uses_command() {
        assert_eq!(
            signature_of(&call("shell", json!({"command": "echo hi"}))),
            "shell:echo hi"
        );
    }

    #[test]
    fn signature_fallback_canonicalizes_key_order() {
        // Same fields, different key order → same fallback signature.
        let a = call("use_skill", json!({"b": 2, "a": 1}));
        let b = call("use_skill", json!({"a": 1, "b": 2}));
        assert_eq!(signature_of(&a), signature_of(&b));
    }

    // --- canonical_json --------------------------------------------------

    #[test]
    fn canonical_json_sorts_keys() {
        let a = canonical_json(&json!({"b": 1, "a": 2}));
        let b = canonical_json(&json!({"a": 2, "b": 1}));
        assert_eq!(a, b);
        assert!(a.starts_with("{a:"));
    }

    #[test]
    fn canonical_json_handles_nested_and_arrays() {
        let a = canonical_json(&json!({"outer": {"z": 1, "a": 2}, "list": [3, 4]}));
        let b = canonical_json(&json!({"list": [3, 4], "outer": {"a": 2, "z": 1}}));
        assert_eq!(a, b);
    }

    // --- tokenize_for_jaccard --------------------------------------------

    #[test]
    fn tokenize_strips_punctuation_keeps_separators() {
        let s = r#"read_file "/usr/local/x.rs" --flag"#;
        let toks = tokenize_for_jaccard(s);
        // path stays one token, read_file stays one token
        assert!(toks.contains("read_file"));
        assert!(toks.contains("/usr/local/x.rs"));
        assert!(toks.contains("flag")); // --flag → flag
        assert!(!toks.contains("--"));
    }

    #[test]
    fn tokenize_lowercases() {
        let toks = tokenize_for_jaccard("READ_FILE Read_File");
        assert!(toks.contains("read_file"));
        assert_eq!(toks.len(), 1);
    }

    #[test]
    fn tokenize_handles_cjk_as_words() {
        // CJK has no whitespace; split_whitespace keeps it as one token,
        // then punctuation-strip leaves it intact (alphanumeric is
        // unicode-aware → CJK chars kept).
        let toks = tokenize_for_jaccard("搜索 关键词");
        assert!(toks.contains("搜索"));
        assert!(toks.contains("关键词"));
    }

    #[test]
    fn tokenize_drops_empties() {
        let toks = tokenize_for_jaccard("   ... :::   ");
        assert!(toks.is_empty());
    }

    // --- jaccard ---------------------------------------------------------

    #[test]
    fn jaccard_identical_sets_is_one() {
        let a: HashSet<String> = ["x", "y", "z"].iter().map(|s| s.to_string()).collect();
        assert_eq!(jaccard(&a, &a), 1.0);
    }

    #[test]
    fn jaccard_disjoint_is_zero() {
        let a: HashSet<String> = ["x"].iter().map(|s| s.to_string()).collect();
        let b: HashSet<String> = ["y"].iter().map(|s| s.to_string()).collect();
        assert_eq!(jaccard(&a, &b), 0.0);
    }

    #[test]
    fn jaccard_partial_overlap() {
        // {a,b,c} vs {a,b,d}: inter=2, union=4 → 0.5
        let a: HashSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let b: HashSet<String> = ["a", "b", "d"].iter().map(|s| s.to_string()).collect();
        assert!((jaccard(&a, &b) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn jaccard_empty_edge_cases() {
        let empty: HashSet<String> = HashSet::new();
        let nonempty: HashSet<String> = ["a"].iter().map(|s| s.to_string()).collect();
        assert_eq!(jaccard(&empty, &empty), 1.0);
        assert_eq!(jaccard(&empty, &nonempty), 0.0);
        assert_eq!(jaccard(&nonempty, &empty), 0.0);
    }
}
