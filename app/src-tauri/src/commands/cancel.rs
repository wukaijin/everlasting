//! `cancel_chat` Tauri command (PR5 — in-flight cancel).
//!
//! The frontend's Stop button invokes this with the current
//! `request_id`. Looks up the matching `CancellationToken` and
//! calls `.cancel()` on it; the agent loop's `tokio::select!`
//! notices on the next event boundary and bails out cleanly
//! (partial turn is persisted; a `done` event with
//! `stop_reason: "cancelled"` is emitted).
//!
//! Idempotent: a missing `request_id` is a silent no-op (the user
//! may have clicked Stop after the stream already finished).
//! Re-cancelling an already-cancelled token is also a no-op.

use std::sync::Arc;

use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub async fn cancel_chat(
    request_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let token = {
        let map = state.cancellations.lock().await;
        map.get(&request_id).cloned()
    };
    if let Some(t) = token {
        t.cancel();
        tracing::info!(request_id = %request_id, "cancel_chat: token cancelled");
    } else {
        tracing::debug!(
            request_id = %request_id,
            "cancel_chat: no active request (likely already finished)"
        );
    }
    Ok(())
}