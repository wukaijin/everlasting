//! SSE (Server-Sent Events) parser.
//!
//! Stateful line-oriented parser. Feed arbitrary chunks of text (may not
//! align to line boundaries); the parser buffers — a line split across
//! chunks at any byte position is reassembled before parsing — and yields
//! complete events at the empty-line boundary.
//!
//! Per HACKING-llm.md "额外观察": the GLM compatibility layer emits a `ping`
//! heartbeat event we don't care about — the caller must tolerate unknown
//! event types and continue.

/// Maximum bytes buffered for a single event's `data` field. Guards
/// against a malicious/buggy upstream emitting a GB-sized data line
/// that would OOM the process (RULE-D-003). Over-cap lines are
/// dropped silently for the rest of the event.
const MAX_DATA_BYTES: usize = 1024 * 1024; // 1 MiB

#[derive(Debug, Default)]
pub struct SseParser {
    event_type: String,
    data_buf: String,
    /// Partial trailing line carried across `feed` calls. A TCP chunk
    /// boundary can split a line at ANY byte — mid-JSON, or even inside
    /// the `data:` prefix itself — so only complete lines (terminated
    /// by '\n') are parsed; the rest waits here. (08-31 incident: a
    /// `data:` line cut mid-JSON was parsed as complete, its
    /// continuation then dropped as malformed, silently truncating
    /// tool arguments and text deltas.)
    line_buf: String,
    /// The line being buffered has exceeded the size guard below; its
    /// remaining bytes are skipped until the terminating '\n' arrives
    /// (RULE-D-003 OOM guard, extended to the line buffer).
    oversized_line: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of text. Returns zero or more complete events found
    /// within. The chunk may end mid-line (TCP chunk boundaries don't
    /// align with line boundaries): the partial trailing line is held
    /// in `line_buf` and completed by the next call, so a `data:` line
    /// cut at any byte position is reassembled before being parsed.
    pub fn feed(&mut self, chunk: &str) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // Line buffering: accumulate into `line_buf` and consume only
        // the complete lines (everything up to and including the last
        // '\n'); the trailing partial line waits for the next feed.
        self.line_buf.push_str(chunk);
        let buffered = std::mem::take(&mut self.line_buf);
        let (complete, partial) = match buffered.rfind('\n') {
            Some(idx) => buffered.split_at(idx + 1),
            None => ("", buffered.as_str()),
        };

        let mut lines = complete;
        if self.oversized_line {
            // `complete` starts inside the dropped oversized line (the
            // flag is only ever set on an unterminated tail): swallow
            // its residue up to and including the terminating '\n'.
            // With no '\n' yet, `partial` is more of the same dropped
            // line — return WITHOUT re-buffering it. This runs before
            // the guard below so a fresh oversized partial in this
            // same feed registers its own flag instead of having it
            // consumed by the previous line's terminator.
            match lines.find('\n') {
                Some(idx) => {
                    lines = &lines[idx + 1..];
                    self.oversized_line = false;
                }
                // Still no newline (complete is empty here) — nothing
                // to process this round.
                None => return events,
            }
        }
        self.line_buf.push_str(partial);
        // RULE-D-003 OOM guard, line-buffer edition: a partial line
        // already longer than the event-level cap can never fit under
        // it once completed (at most the "data: " prefix is stripped),
        // so stop buffering its bytes; the rest of the line is skipped
        // when its '\n' finally arrives.
        if self.line_buf.len() > MAX_DATA_BYTES + "data: ".len() {
            self.oversized_line = true;
            self.line_buf.clear();
        }

        if lines.is_empty() {
            // No complete line arrived (or the oversized line's
            // terminator was this chunk's last byte).
            return events;
        }
        // `lines` ends with the '\n' that terminated its last line.
        // Drop it, or split() would yield a phantom trailing "" that
        // is not a real empty line — a chunk ending exactly after
        // "data: x\n" must NOT dispatch the event early and split a
        // multi-line data payload.
        let real_lines = &lines[..lines.len() - 1];

        for raw_line in real_lines.split('\n') {
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);

            if line.is_empty() {
                if !self.data_buf.is_empty() {
                    events.push(SseEvent {
                        event: std::mem::take(&mut self.event_type),
                        data: std::mem::take(&mut self.data_buf),
                    });
                }
            } else if let Some(rest) = line.strip_prefix("event: ") {
                self.event_type = rest.to_string();
            } else if let Some(rest) = line.strip_prefix("data:") {
                // SSE spec allows at most one leading space after the
                // colon. Tolerate both "data: x" and "data:x" (RULE-D-003:
                // some proxies/compat layers omit the space).
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                // Cap the buffered data at 1 MiB so a malicious or buggy
                // upstream can't OOM us with a GB-sized data field
                // (RULE-D-003). Once over cap, drop further data lines
                // for this event.
                let needs_newline = !self.data_buf.is_empty();
                let added = rest.len() + usize::from(needs_newline);
                if self.data_buf.len() + added <= MAX_DATA_BYTES {
                    if needs_newline {
                        self.data_buf.push('\n');
                    }
                    self.data_buf.push_str(rest);
                }
            } else if line.starts_with("id:") || line.starts_with("retry:") {
                // Per spec, "id:" sets Last-Event-ID and "retry:" sets
                // reconnect time. We don't use either; ignore silently.
            } else if line.starts_with(':') {
                // Comment line, ignore per SSE spec.
            }
            // Anything else is malformed; drop silently rather than panic.
        }
        events
    }

    /// Drop any partially-buffered state. Call on connection abort so a
    /// retry doesn't see leftover half-event state.
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.event_type.clear();
        self.data_buf.clear();
        self.line_buf.clear();
        self.oversized_line = false;
    }
}

/// Decode one chunk of an SSE byte stream, carrying a partial UTF-8
/// sequence across chunk boundaries.
///
/// TCP/HTTP chunking can split a multi-byte UTF-8 character across two
/// chunks. Decoding each chunk in isolation then fails with
/// "incomplete utf-8 byte sequence" and aborts a healthy turn
/// (incident `3qnzktvosvxmsycoz46` turn=25: a CJK char cut at byte
/// 4082). `carry` buffers the incomplete tail and this returns
/// `Ok(None)` so the caller waits for the next chunk; once the
/// sequence completes it returns `Ok(Some(text))` for the whole
/// accumulated buffer and clears `carry`. Genuinely invalid UTF-8 (a
/// bad byte *inside* the stream, not just a truncated tail) still
/// errors, matching pre-fix behavior.
pub(crate) fn utf8_chunk_text(
    carry: &mut Vec<u8>,
    bytes: &[u8],
) -> Result<Option<String>, std::str::Utf8Error> {
    carry.extend_from_slice(bytes);
    match String::from_utf8(std::mem::take(carry)) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.utf8_error().error_len().is_none() => {
            // Incomplete trailing sequence (e.g. 1-2 bytes of a 3-byte
            // CJK char) — hold the whole buffer for the next chunk.
            *carry = e.into_bytes();
            Ok(None)
        }
        Err(e) => Err(e.utf8_error()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_event() {
        let mut p = SseParser::new();
        let events = p.feed("event: message_start\ndata: {\"a\":1}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "message_start");
        assert_eq!(events[0].data, "{\"a\":1}");
    }

    #[test]
    fn buffers_across_chunks() {
        let mut p = SseParser::new();
        assert!(p.feed("event: ping\n").is_empty());
        let events = p.feed("data: x\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "ping");
        assert_eq!(events[0].data, "x");
    }

    #[test]
    fn handles_carriage_return() {
        let mut p = SseParser::new();
        let events = p.feed("event: ping\r\ndata: y\r\n\r\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "y");
    }

    #[test]
    fn ignores_comments() {
        let mut p = SseParser::new();
        let events = p.feed(": this is a comment\nevent: ping\ndata: z\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "ping");
    }

    // --- RULE-D-003: tolerate "data:" without a space ---

    #[test]
    fn data_field_without_space_is_tolerated() {
        // No space after "data:" — some proxies/compat layers omit it.
        let mut p = SseParser::new();
        let events = p.feed("data:no-space-here\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "no-space-here");
    }

    #[test]
    fn data_field_with_space_still_works() {
        // Regression: the standard "data: x" form must still parse.
        let mut p = SseParser::new();
        let events = p.feed("data: with-space\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "with-space");
    }

    // --- RULE-D-003: 1 MiB data_buf cap ---

    #[test]
    fn data_field_capped_at_1mib() {
        // Two 700 KB data lines = 1.4 MB > 1 MiB cap. The first fits,
        // the second overflows and is dropped — data_buf stays bounded.
        let mut p = SseParser::new();
        let big = "x".repeat(700_000);
        let chunk = format!("data: {}\ndata: {}\n\n", big, big);
        let events = p.feed(&chunk);
        assert_eq!(events.len(), 1);
        assert!(
            events[0].data.len() <= MAX_DATA_BYTES,
            "data not capped: got {} bytes",
            events[0].data.len()
        );
        // First line preserved (second dropped).
        assert!(
            events[0].data.len() >= 700_000,
            "first line lost: got {} bytes",
            events[0].data.len()
        );
    }

    #[test]
    fn single_oversized_data_line_does_not_oom() {
        // One 2 MB data line alone exceeds the 1 MiB cap → dropped
        // entirely; with no other data the event isn't emitted (the
        // empty data_buf suppresses it). The point: no panic, no
        // unbounded buffer growth.
        let mut p = SseParser::new();
        let huge = "y".repeat(2 * 1024 * 1024);
        let chunk = format!("data: {}\n\n", huge);
        let events = p.feed(&chunk);
        assert!(events.is_empty());
    }

    // --- 08-31 half-line fix: a `data:` line split at a TCP chunk
    // boundary is reassembled from `line_buf` instead of being parsed
    // truncated and its continuation dropped as malformed ---
    // (incident evidence: tool_use input={}, "Unterminated quoted
    // string", old_string not found; see
    // .trellis/tasks/08-31-sse-halfline-fix/research/)

    #[test]
    fn half_line_cut_at_any_position_yields_identical_events() {
        // Reference: the same stream fed in one piece. The payload has
        // an event line, TWO data lines (join semantics), CJK text and
        // JSON braces so cuts land in every kind of content — including
        // inside the "data:" prefix itself.
        let stream =
            "event: ping\ndata: {\"text\":\"好的方案\",\"n\":42}\ndata: second 数据 line\n\n";
        let mut reference = SseParser::new();
        let want = reference.feed(stream);
        assert_eq!(want.len(), 1);
        assert_eq!(want[0].event, "ping");
        assert_eq!(
            want[0].data,
            "{\"text\":\"好的方案\",\"n\":42}\nsecond 数据 line"
        );

        let mut cuts: Vec<usize> = stream.char_indices().map(|(i, _)| i).collect();
        cuts.push(stream.len());
        for cut in cuts {
            let mut p = SseParser::new();
            let mut got = p.feed(&stream[..cut]);
            got.extend(p.feed(&stream[cut..]));
            assert_eq!(got, want, "cut at byte {} corrupted events", cut);
        }
    }

    #[test]
    fn data_prefix_cut_across_chunks_is_reassembled() {
        // The residual "dat" matches no known field prefix — without
        // line buffering it hit the malformed-drop branch and the whole
        // data line (rest of the prefix + payload) was lost.
        let mut p = SseParser::new();
        assert!(p.feed("event: ping\ndat").is_empty());
        let events = p.feed("a: {\"ok\":true}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "ping");
        assert_eq!(events[0].data, "{\"ok\":true}");
    }

    #[test]
    fn cjk_char_cut_across_byte_chunks_parses_identically() {
        // Provider-realistic two-layer path: raw bytes →
        // utf8_chunk_text (reassembles a multi-byte char split
        // mid-sequence) → SseParser::feed (reassembles a line split
        // mid-line). Every byte position is tried, including cuts
        // INSIDE the CJK chars.
        let stream = "data: {\"text\":\"好的方案\"}\n\n";
        let bytes = stream.as_bytes();
        let mut reference = SseParser::new();
        let want = reference.feed(stream);

        for cut in 0..=bytes.len() {
            let mut carry = Vec::new();
            let mut p = SseParser::new();
            let mut got = Vec::new();
            for part in [&bytes[..cut], &bytes[cut..]] {
                if let Some(text) = utf8_chunk_text(&mut carry, part).unwrap() {
                    got.extend(p.feed(&text));
                }
            }
            assert_eq!(got, want, "byte cut at {} corrupted events", cut);
        }
    }

    #[test]
    fn consecutive_events_interleaved_cut_all_delivered() {
        let mut p = SseParser::new();
        // Event a's data line is cut mid-JSON.
        assert!(p.feed("event: a\ndata: {\"i\"").is_empty());
        // One feed carrying a's tail + terminator + b's head — with b's
        // data line cut inside the "data" prefix itself.
        let mid = p.feed(":1}\n\nevent: b\nda");
        assert_eq!(mid.len(), 1);
        assert_eq!(mid[0].event, "a");
        assert_eq!(mid[0].data, "{\"i\":1}");
        let tail = p.feed("ta: {\"i\":2}\n\n");
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].event, "b");
        assert_eq!(tail[0].data, "{\"i\":2}");
    }

    #[test]
    fn reset_discards_partial_line_and_event_state() {
        let mut p = SseParser::new();
        // Half an event name + half a data line buffered mid-line.
        assert!(p.feed("event: pi\ndata: {\"half").is_empty());
        p.reset();
        // Next feed parses from a clean slate: no leftover prefix or
        // partial line may leak into the new event.
        let events = p.feed("data: fresh\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "");
        assert_eq!(events[0].data, "fresh");
    }

    #[test]
    fn unterminated_oversized_line_dropped_and_stream_resumes() {
        // RULE-D-003 OOM guard, line-buffer edition: an unterminated
        // line past the cap stops accumulating; its late-arriving tail
        // is skipped and the following healthy event parses normally.
        let mut p = SseParser::new();
        let huge = "x".repeat(2 * 1024 * 1024);
        assert!(p.feed(&format!("data: {}", huge)).is_empty());
        let events = p.feed(" oversized tail\ndata: ok\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "ok");
    }

    #[test]
    fn consecutive_oversized_lines_both_skipped() {
        // Boundary of the OOM guard: one feed carries the terminator
        // of the first oversized line AND a second oversized line
        // still unterminated. The skip flag must survive the first
        // line's terminator (it belongs to the second line now) — its
        // residue must never parse into a phantom `data:` event.
        let mut p = SseParser::new();
        let huge = "x".repeat(3 * 1024 * 1024);
        assert!(p
            .feed(&format!("data: {}\ndata: {}", huge, huge))
            .is_empty());
        let events = p.feed("data: injected\n\ndata: ok\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "ok");
    }

    #[test]
    fn line_buffer_guard_at_exact_threshold() {
        // "data: " + exactly MAX_DATA_BYTES payload (line length
        // MAX + 6) is the largest line the guard still buffers — its
        // payload fits the event-level cap, so it must survive
        // intact; the guard only fires strictly beyond it.
        let payload = "x".repeat(MAX_DATA_BYTES);
        let mut p = SseParser::new();
        assert!(p.feed(&format!("data: {}", payload)).is_empty());
        let events = p.feed("\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data.len(), MAX_DATA_BYTES);
    }

    // --- utf8_chunk_text: carry a multibyte char split across chunks ---
    // (incident 3qnzktvosvxmsycoz46 turn=25: a CJK char cut at a chunk
    // boundary used to error the whole turn with "incomplete utf-8").

    #[test]
    fn utf8_complete_chunk_flows_through() {
        let mut carry = Vec::new();
        let text = utf8_chunk_text(&mut carry, b"hello world").unwrap();
        assert_eq!(text.unwrap(), "hello world");
        assert!(carry.is_empty());
    }

    #[test]
    fn utf8_cjk_char_split_across_chunks_is_carried() {
        let mut carry = Vec::new();
        let full = "好的".as_bytes(); // 6 bytes, 2 CJK chars
                                      // Cut after the first byte of "的": "好" (3B) + 1 byte.
        assert!(utf8_chunk_text(&mut carry, &full[..4]).unwrap().is_none());
        assert_eq!(carry.len(), 4, "partial buffer held for next chunk");
        let text = utf8_chunk_text(&mut carry, &full[4..]).unwrap();
        assert_eq!(text.unwrap(), "好的");
        assert!(carry.is_empty());
    }

    #[test]
    fn utf8_char_fed_one_byte_at_a_time_accumulates() {
        let mut carry = Vec::new();
        let ch = "好".as_bytes(); // 3 bytes
        assert!(utf8_chunk_text(&mut carry, &ch[..1]).unwrap().is_none());
        assert!(utf8_chunk_text(&mut carry, &ch[1..2]).unwrap().is_none());
        let text = utf8_chunk_text(&mut carry, &ch[2..]).unwrap();
        assert_eq!(text.unwrap(), "好");
        assert!(carry.is_empty());
    }

    #[test]
    fn utf8_ascii_prefix_with_partial_tail_is_held() {
        let mut carry = Vec::new();
        let full = "head好的tail".as_bytes();
        // Cut after the first byte of "好" (index 4 + 1).
        assert!(utf8_chunk_text(&mut carry, &full[..5]).unwrap().is_none());
        let text = utf8_chunk_text(&mut carry, &full[5..]).unwrap();
        assert_eq!(text.unwrap(), "head好的tail");
        assert!(carry.is_empty());
    }

    #[test]
    fn utf8_invalid_bytes_still_error() {
        let mut carry = Vec::new();
        assert!(utf8_chunk_text(&mut carry, b"ab\xffcd").is_err());
        assert!(carry.is_empty(), "no garbage retained on hard error");
    }
}
