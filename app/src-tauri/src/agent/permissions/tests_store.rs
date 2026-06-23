#![cfg(test)]

use crate::agent::permissions::{
    cancel_session_asks, new_permission_store, register_ask, PermissionResponse,
};

#[tokio::test]
async fn cancel_session_asks_isolates_by_session() {
    // RULE-B-002: cancelling one session's asks must NOT drop
    // another session's pending sender. Before the fix the body
    // was a map.clear() that wiped the whole store.
    let store = new_permission_store();
    let rx_a = register_ask(&store, "sess-a", "rid-a".to_string()).await;
    let _rx_b = register_ask(&store, "sess-b", "rid-b".to_string()).await;
    {
        let map = store.lock().await;
        assert_eq!(map.len(), 2, "both asks registered");
    }

    cancel_session_asks(&store, "sess-a").await;

    {
        let map = store.lock().await;
        assert!(
            !map.contains_key("rid-a"),
            "session A's ask must be cancelled"
        );
        assert!(
            map.contains_key("rid-b"),
            "session B's ask must survive cancel of A"
        );
        assert_eq!(map.len(), 1);
    }
    // A's sender was dropped by the cancel → its receiver resolves
    // Err, which check() treats as Deny (same as the timeout path).
    assert!(rx_a.await.is_err());
}

#[test]
fn permission_response_deny_carries_reason() {
    let resp = PermissionResponse::Deny {
        reason: "use git clean instead".to_string(),
    };
    if let PermissionResponse::Deny { reason } = resp {
        assert_eq!(reason, "use git clean instead");
    } else {
        panic!("expected Deny");
    }
    let plain = PermissionResponse::Deny {
        reason: String::new(),
    };
    if let PermissionResponse::Deny { reason } = plain {
        assert!(reason.is_empty());
    } else {
        panic!("expected Deny");
    }
}
