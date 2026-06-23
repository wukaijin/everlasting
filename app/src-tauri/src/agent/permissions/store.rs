//! In-flight `permission:ask` store: keyed by `rid`, scoped by
//! `session_id` for per-session cancel. Split out of `mod.rs`
//! on 2026-06-23.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{oneshot, Mutex};

use super::types::PermissionResponse;

/// A pending `permission:ask` awaiting the user's response. The
/// `session_id` binding lets `cancel_session_asks` filter by
/// session (RULE-B-002) so cancelling one session's in-flight
/// asks never drops another session's pending oneshot sender. The
/// `rid` key alone can't carry the session — the resolve-side
/// `permission_response` IPC sends only the rid — so the session
/// lives on the value (approach A, RULE-B-002).
#[derive(Debug)]
pub struct PendingAsk {
    session_id: String,
    tx: oneshot::Sender<PermissionResponse>,
}

/// In-flight permission asks, keyed by `rid` (random request id
/// emitted with the `permission:ask` event). The agent loop
/// inserts a `(rid, PendingAsk)` pair before emitting; the IPC
/// `permission_response` handler looks up by `rid` and forwards
/// the response on the inner `tx`. An entry is removed (and its
/// sender dropped) on timeout, and a whole session's entries are
/// dropped by `cancel_session_asks` (wired into `delete_session`).
pub type PermissionStore = Arc<Mutex<HashMap<String, PendingAsk>>>;

pub fn new_permission_store() -> PermissionStore {
    Arc::new(Mutex::new(HashMap::new()))
}

/// Insert a pending ask. The `rid` is a UUID string (the agent
/// loop generates it); `session_id` binds the entry to its
/// session so `cancel_session_asks` can filter (RULE-B-002). The
/// returned `oneshot::Receiver` is the future the agent loop
/// awaits in `check()`.
pub async fn register_ask(
    store: &PermissionStore,
    session_id: &str,
    rid: String,
) -> oneshot::Receiver<PermissionResponse> {
    let (tx, rx) = oneshot::channel();
    let mut map = store.lock().await;
    map.insert(rid, PendingAsk { session_id: session_id.to_string(), tx });
    rx
}

/// Resolve a pending ask. Called by the `permission_response`
/// IPC handler. Returns `true` if the rid was found and the
/// sender accepted the response; `false` if the rid was missing
/// (already timed out, or duplicate response).
pub async fn resolve_ask(
    store: &PermissionStore,
    rid: &str,
    response: PermissionResponse,
) -> bool {
    let mut map = store.lock().await;
    if let Some(ask) = map.remove(rid) {
        ask.tx.send(response).is_ok()
    } else {
        false
    }
}

/// Cancel all pending asks for a session, leaving other sessions'
/// asks intact (RULE-B-002). Called from the destructive-op
/// cancel hook — `delete_session` (`commands/sessions.rs`) wires
/// this in after the agent loop has exited. Removed `PendingAsk`
/// values drop in place, so their oneshot senders drop and the
/// awaiting `check()` receiver returns `Err(RecvError)`, which
/// `check()` treats as Deny (same as the timeout path).
pub async fn cancel_session_asks(store: &PermissionStore, session_id: &str) {
    let mut map = store.lock().await;
    // Retain only asks NOT owned by the cancelled session. retain
    // drops the removed values here → their senders drop → the
    // awaiting receiver resolves Err → check() picks the Deny path.
    map.retain(|_, ask| ask.session_id != session_id);
}
