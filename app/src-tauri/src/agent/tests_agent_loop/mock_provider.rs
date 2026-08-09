#![cfg(test)]

use std::sync::Arc;

use futures_util::StreamExt;

use crate::db;
use crate::llm::provider::mock::{MockProvider, MockResponse};
use crate::llm::types::ChatEvent;
use crate::llm::Provider;

// ---------------------------------------------------------------------------
// 7) Provider protocol is `Mock`
// ---------------------------------------------------------------------------

/// The `MockProvider::protocol()` returns
/// `ProviderProtocol::Mock`. This is the catalog dispatch
/// contract — the chat command's pre-flight could reject
/// unknown protocols, so we test that the protocol wire
/// format is well-formed end-to-end.
#[test]
fn mock_provider_reports_mock_protocol() {
    let mock = MockProvider::new(vec![]);
    assert_eq!(mock.protocol(), db::ProviderProtocol::Mock);
    let caps = mock.capabilities();
    assert!(caps.supports_system_prompt);
    assert!(caps.supports_tools);
    assert!(caps.supports_streaming);
}

// ---------------------------------------------------------------------------
// 8) MockProvider call count tracking
// ---------------------------------------------------------------------------

/// `call_count()` is the primary assertion surface for "did
/// the agent loop dispatch the expected number of turns?".
/// This unit test guards the counter itself (the agent-loop
/// integration tests above rely on it being accurate).
#[tokio::test]
async fn mock_provider_call_count_tracks_send_calls() {
    let mock = Arc::new(MockProvider::new(vec![
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: None,
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: None,
            }),
        ]),
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: None,
            }),
        ]),
    ]));
    assert_eq!(mock.call_count(), 0);
    let _ = mock
        .send(None, vec![], vec![])
        .collect::<Vec<_>>()
        .await
        .len();
    assert_eq!(mock.call_count(), 1);
    let _ = mock
        .send(None, vec![], vec![])
        .collect::<Vec<_>>()
        .await
        .len();
    assert_eq!(mock.call_count(), 2);
    let _ = mock
        .send(None, vec![], vec![])
        .collect::<Vec<_>>()
        .await
        .len();
    assert_eq!(mock.call_count(), 3);
}
