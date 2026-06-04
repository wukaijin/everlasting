//! SSE (Server-Sent Events) parser.
//!
//! Stateful line-oriented parser. Feed arbitrary chunks of text (may not
//! align to line boundaries); the parser buffers and yields complete events
//! at the empty-line boundary.
//!
//! Per HACKING-llm.md "额外观察": the GLM compatibility layer emits a `ping`
//! heartbeat event we don't care about — the caller must tolerate unknown
//! event types and continue.

#[derive(Debug, Default)]
pub struct SseParser {
    event_type: String,
    data_buf: String,
}

#[derive(Debug, Clone)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of text. Returns zero or more complete events found
    /// within. Trailing data (event opened but not closed) is buffered for
    /// the next call.
    pub fn feed(&mut self, chunk: &str) -> Vec<SseEvent> {
        let mut events = Vec::new();
        for raw_line in chunk.split('\n') {
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
            } else if let Some(rest) = line.strip_prefix("data: ") {
                if !self.data_buf.is_empty() {
                    self.data_buf.push('\n');
                }
                self.data_buf.push_str(rest);
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
}
