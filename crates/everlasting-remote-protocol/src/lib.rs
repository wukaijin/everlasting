//! Wire-frame types shared between `everlasting-remote` (the edge daemon
//! running on the cloud server) and the tunnel-client module inside
//! `everlasting-daemon` (running on each PC behind NAT).
//!
//! Single source of truth for the tunnel protocol (task
//! `08-11-remote-control-epic`, S1 first commit per review P1-1). Both
//! crates depend on this one; never duplicate the `Frame` definition.
//!
//! ## Frame model
//!
//! A single long-lived WSS connection carries every mobile request and SSE
//! stream between remote and PC daemon. Three frame kinds — identified by
//! the internal `kind` tag — multiplex over that connection, correlated
//! by the per-request `id`:
//!
//! - `Request`  (remote → PC): one HTTP request forwarded from a mobile client
//! - `Response` (PC → remote): non-streaming reply
//! - `Stream`   (bidirectional): streaming (SSE chunked) chunks/end/error
//!
//! See `docs/` task PRD/design for the bridging rationale and SSE
//! fan-out behavior (SseRegistry is global, not session-keyed).

use serde::{Deserialize, Serialize};

/// Path prefix marking remote-internal RPCs that arrive over WSS from a PC
/// daemon (e.g. pairing-code generation). These are NOT mobile HTTP routes
/// and never carry `/api/v1` — they are dispatched inside the WSS receive
/// loop only.
pub const INTERNAL_PREFIX: &str = "/internal/";

/// One frame on the remote ↔ PC WSS connection.
///
/// Serialized as JSON (task S1/S3 MVP). The `kind` tag is internal-tag
/// serde, so adding new variants is forward-compatible for older peers
/// (they'll fail deserialization of the unknown variant, but won't break
/// parsing of surrounding frames).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    /// remote → PC: forward one mobile HTTP request to the PC daemon's
    /// local axum server via loopback. `path` has already been stripped
    /// of the `/api/v1/proxy` prefix and of any `access_token` query
    /// (P1-2/P2-1 review revisions — token is consumed at the remote
    /// auth layer and must not leak to the PC side).
    Request {
        id: u64,
        method: String,
        path: String,
        /// Ordered list — HTTP headers are ordered and may repeat
        /// (e.g. multiple `Set-Cookie`), so a `HashMap` would lose both.
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// PC → remote: non-streaming reply, correlated by `id`.
    Response {
        id: u64,
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    },
    /// Bidirectional: streaming reply (SSE chunked) or a control signal
    /// for an in-flight stream, correlated by `id`.
    Stream { id: u64, event: StreamEvent },
}

/// Sub-event for a `Frame::Stream`. SSE chunked responses are forwarded
/// as a sequence of `Chunk`s terminated by `End` (or `Error`).
///
/// Note: SSE chunk byte boundaries do NOT necessarily align with SSE event
/// boundaries (`id:...\nevent:...\ndata:...\n\n` may be split). That is
/// fine — the remote writes chunks verbatim into the mobile's SSE HTTP
/// body and the browser `EventSource` parses `\n\n` boundaries itself.
/// The tunnel layer stays pure byte-passthrough.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamEvent {
    /// One raw segment of an SSE response body.
    Chunk { bytes: Vec<u8> },
    /// Stream completed normally (server closed the SSE connection).
    End,
    /// Stream errored (transport failure, timeout, etc.).
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JSON round-trip preserves all three frame kinds including body bytes
    /// and header ordering. This guards the wire contract — any serialization
    /// change here breaks both peers simultaneously.
    #[test]
    fn request_round_trip() {
        let original = Frame::Request {
            id: 42,
            method: "POST".to_string(),
            path: "/api/v1/sessions/list".to_string(),
            headers: vec![
                ("Content-Type".to_string(), "application/json".to_string()),
                ("X-Custom".to_string(), "value with spaces".to_string()),
            ],
            body: br#"{"projectId":"abc"}"#.to_vec(),
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let back: Frame = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, back);
    }

    #[test]
    fn response_round_trip() {
        let original = Frame::Response {
            id: 42,
            status: 200,
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: br#"{"ok":true}"#.to_vec(),
        };
        let json = serde_json::to_string(&original).expect("serialize");
        let back: Frame = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(original, back);
    }

    #[test]
    fn stream_chunk_end_error_round_trip() {
        for event in [
            StreamEvent::Chunk {
                bytes: b"id:1\nevent:chat-event\ndata:{...}\n\n".to_vec(),
            },
            StreamEvent::End,
            StreamEvent::Error {
                message: "timeout".to_string(),
            },
        ] {
            let original = Frame::Stream { id: 7, event };
            let json = serde_json::to_string(&original).expect("serialize");
            let back: Frame = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(original, back);
        }
    }

    /// Internal-tag serde emits `"kind":"request"` / `"response"` / `"stream"`.
    /// Locking the tag values here protects the wire contract — both peers
    /// must agree on these strings.
    #[test]
    fn kind_tag_values_are_snake_case() {
        let req = Frame::Request {
            id: 1,
            method: "GET".into(),
            path: "/".into(),
            headers: vec![],
            body: vec![],
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["kind"], "request");

        let resp = Frame::Response {
            id: 1,
            status: 200,
            headers: vec![],
            body: vec![],
        };
        assert_eq!(serde_json::to_value(&resp).unwrap()["kind"], "response");

        let stream = Frame::Stream {
            id: 1,
            event: StreamEvent::End,
        };
        assert_eq!(serde_json::to_value(&stream).unwrap()["kind"], "stream");
    }

    /// `StreamEvent` sub-tag uses `snake_case` too (`chunk`/`end`/`error`).
    #[test]
    fn stream_event_subtag_values() {
        assert_eq!(
            serde_json::to_value(StreamEvent::End).unwrap()["kind"],
            "end"
        );
        let err = StreamEvent::Error {
            message: "x".into(),
        };
        assert_eq!(serde_json::to_value(&err).unwrap()["kind"], "error");
        let chunk = StreamEvent::Chunk { bytes: vec![] };
        assert_eq!(serde_json::to_value(&chunk).unwrap()["kind"], "chunk");
    }

    /// Header ordering is preserved across serialization (the reason we use
    /// `Vec<(String,String)>` not `HashMap` — see header doc on `Frame::Request`).
    #[test]
    fn header_order_preserved() {
        let original = Frame::Request {
            id: 1,
            method: "GET".into(),
            path: "/".into(),
            headers: vec![
                ("Z-Last".into(), "1".into()),
                ("A-First".into(), "2".into()),
                ("M-Middle".into(), "3".into()),
            ],
            body: vec![],
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: Frame = serde_json::from_str(&json).unwrap();
        let Frame::Request { headers, .. } = back else {
            panic!("expected Request");
        };
        let keys: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["Z-Last", "A-First", "M-Middle"]);
    }

    /// Binary body bytes (including non-UTF-8) survive JSON round-trip via
    /// base64-style byte-array serialization. `Vec<u8>` serializes as a JSON
    /// array of numbers in serde_json — guards against assuming string body.
    #[test]
    fn binary_body_survives() {
        let body = vec![0u8, 255, 128, 1, 2, 3];
        let original = Frame::Response {
            id: 9,
            status: 200,
            headers: vec![],
            body,
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: Frame = serde_json::from_str(&json).unwrap();
        let Frame::Response { body, .. } = back else {
            panic!("expected Response");
        };
        assert_eq!(body, vec![0u8, 255, 128, 1, 2, 3]);
    }

    #[test]
    fn internal_prefix_is_absolute_path() {
        // Sanity: the prefix starts with '/' (it's matched against the start
        // of an HTTP path that already begins with '/'). If this constant
        // ever changes shape, the ws.rs dispatch logic must be re-checked.
        assert!(INTERNAL_PREFIX.starts_with('/'));
        assert!(INTERNAL_PREFIX.ends_with('/'));
        assert_eq!(INTERNAL_PREFIX, "/internal/");
    }
}
