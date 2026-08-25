#![cfg(test)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;

use crate::agent::helpers::{cancel_and_drain_all_agent_loops, cancel_inflight_for_session};
use crate::state::CancellationGuard;

/// Race a slow fake stream against a cancellation token. Mirrors
/// the per-event select! loop in `chat` (minus the SSE plumbing).
/// Asserts cancel wins when fired mid-stream.
#[tokio::test]
async fn select_loop_breaks_on_cancellation() {
    let token = CancellationToken::new();
    let cancelled_flag = Arc::new(std::sync::Mutex::new(false));
    let cancelled_flag_clone = cancelled_flag.clone();
    let token_clone = token.clone();

    // Simulate the per-event select! pattern. Each "event" is
    // tokio::time::sleep; the cancel arm races them.
    let handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                _ = token_clone.cancelled() => {
                    *cancelled_flag_clone.lock().unwrap() = true;
                    break;
                }
                _ = tokio::time::sleep(Duration::from_millis(200)) => {
                    // Stream "produced an event" — loop again.
                }
            }
        }
    });

    // Give the loop a tick to start, then cancel.
    tokio::time::sleep(Duration::from_millis(50)).await;
    token.cancel();

    // The select! arm should win within a few ms.
    tokio::time::timeout(Duration::from_millis(500), handle)
        .await
        .expect("select loop should have broken within 500ms")
        .expect("task should not have panicked");
    assert!(
        *cancelled_flag.lock().unwrap(),
        "cancelled flag should be set when select! breaks on cancel"
    );
}

#[tokio::test]
async fn cancellation_token_idempotent() {
    let token = CancellationToken::new();
    token.cancel();
    token.cancel();
    // Second cancel is a no-op; is_cancelled stays true; no panic.
    assert!(token.is_cancelled());
}

/// Mirrors the `cancel_chat` command's lookup logic, isolated
/// from the Tauri State wrapper. Tests that a missing
/// `request_id` is a silent Ok (idempotent) and a present one
/// actually flips the token.
#[tokio::test]
async fn cancel_chat_idempotent_for_missing_and_present() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Missing request_id → no-op, returns Ok.
    let missing = {
        let map = cancellations.lock().await;
        map.get("does-not-exist").cloned()
    };
    assert!(missing.is_none(), "unknown id should not be in map");

    // Present request_id → token fetched, is_cancelled flips.
    let token = CancellationToken::new();
    {
        let mut map = cancellations.lock().await;
        map.insert("rid-1".to_string(), token.clone());
    }
    let fetched = {
        let map = cancellations.lock().await;
        map.get("rid-1").cloned()
    };
    assert!(fetched.is_some());
    let t = fetched.unwrap();
    assert!(!t.is_cancelled());
    t.cancel();
    assert!(t.is_cancelled());
}

/// Concurrent requests: two `request_id`s are independent. Cancel
/// one; the other should not be affected.
#[tokio::test]
async fn two_concurrent_requests_are_independent() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let a = CancellationToken::new();
    let b = CancellationToken::new();
    {
        let mut map = cancellations.lock().await;
        map.insert("a".to_string(), a.clone());
        map.insert("b".to_string(), b.clone());
    }
    // Cancel A.
    {
        let map = cancellations.lock().await;
        let t = map.get("a").cloned();
        if let Some(t) = t {
            t.cancel();
        }
    }
    assert!(a.is_cancelled());
    assert!(!b.is_cancelled(), "B should not be affected by A's cancel");
}

/// CancellationGuard removes the entry on Drop. We construct a
/// guard, drop it, and verify the map is empty. The Drop runs
/// `tauri::async_runtime::spawn`, so the test is wrapped in
/// `#[tokio::test]` to provide a runtime (the guard's spawn
/// borrows the current Tokio runtime via the Tauri shim; in
/// unit tests we route through the global runtime).
#[tokio::test(flavor = "multi_thread")]
async fn cancellation_guard_removes_entry_on_drop() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let session_active_request: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    {
        let mut map = cancellations.lock().await;
        map.insert("rid-g".to_string(), CancellationToken::new());
    }
    {
        let mut s2p = session_active_request.lock().await;
        s2p.insert("sid-g".to_string(), "rid-g".to_string());
    }
    assert_eq!(cancellations.lock().await.len(), 1);
    {
        let _guard = CancellationGuard {
            cancellations: cancellations.clone(),
            session_active_request: session_active_request.clone(),
            request_id: "rid-g".to_string(),
            session_id: "sid-g".to_string(),
            skip_session_active: false,
            skip_cancellations: false,
        };
        // _guard drops at end of block.
    }
    // Give the spawned cleanup task a moment to run.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        cancellations.lock().await.is_empty(),
        "guard's Drop should have removed the cancellations entry"
    );
    assert!(
        session_active_request.lock().await.is_empty(),
        "guard's Drop should have removed the session_active_request entry"
    );
}

/// Step 4 follow-up: `cancel_inflight_for_session` cancels the
/// matching request token when the session has an in-flight
/// request, and is a silent no-op otherwise.
#[tokio::test]
async fn cancel_inflight_for_session_cancels_token() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let session_active_request: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    // Register a fake request for session "s1". No exit receiver is
    // registered (simulates a chat that predates the RULE-E-005
    // signal wiring, or one whose spawn closure already drained it).
    let token = CancellationToken::new();
    {
        let mut map = cancellations.lock().await;
        map.insert("rid-1".to_string(), token.clone());
    }
    {
        let mut s2p = session_active_request.lock().await;
        s2p.insert("s1".to_string(), "rid-1".to_string());
    }
    assert!(!token.is_cancelled());
    let exit_rx = cancel_inflight_for_session(
        &cancellations,
        &session_active_request,
        &inflight_exits,
        "s1",
    )
    .await;
    assert!(
        token.is_cancelled(),
        "matching request's token should be cancelled"
    );
    // No receiver was registered → helper returns None (caller's
    // `await_inflight_exit` becomes a no-op, but the token cancel
    // still took effect).
    assert!(
        exit_rx.is_none(),
        "no exit signal should be returned when none was registered"
    );
}

/// Step 4 follow-up: `cancel_inflight_for_session` is a no-op
/// when the session has no in-flight request.
#[tokio::test]
async fn cancel_inflight_for_session_missing_session_is_noop() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let session_active_request: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    // Nothing registered.
    let exit_rx = cancel_inflight_for_session(
        &cancellations,
        &session_active_request,
        &inflight_exits,
        "s-missing",
    )
    .await;
    // No panic, no state change. Returns None (no in-flight request).
    assert!(cancellations.lock().await.is_empty(), "nothing to cancel");
    assert!(exit_rx.is_none(), "no exit signal for a missing session");
    assert!(session_active_request.lock().await.is_empty());
    assert!(inflight_exits.lock().await.is_empty());
}

/// Step 4 follow-up: `cancel_inflight_for_session` is a no-op
/// when the session has a request_id but the matching
/// cancellation token is already gone (rare race: the
/// request finished between the map reads).
#[tokio::test]
async fn cancel_inflight_for_session_token_gone_is_noop() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let session_active_request: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    // session_active_request has the entry, but the
    // cancellations map doesn't (the request already
    // finished and the CancellationGuard cleaned up).
    {
        let mut s2p = session_active_request.lock().await;
        s2p.insert("s1".to_string(), "rid-gone".to_string());
    }
    let exit_rx = cancel_inflight_for_session(
        &cancellations,
        &session_active_request,
        &inflight_exits,
        "s1",
    )
    .await;
    // No panic; the function is best-effort. No token found → no
    // exit receiver either (returns None).
    assert!(exit_rx.is_none());
}

/// RULE-E-005 (2026-06-15): `cancel_inflight_for_session` returns a
/// "agent loop exited" signal (`oneshot::Receiver`) that the
/// destructive commands await before deleting. This test proves the
/// signal's core contract: it stays **pending** while the producer
/// (the `chat` spawn closure standing in here) is silent, and
/// **resolves** only once the producer fires (i.e. `run_chat_loop`
/// returned). Without this, `delete_worktree`'s `await` would be a
/// no-op and the race the fix targets would still be open.
///
/// Mirrors `select_loop_breaks_on_cancellation`'s spawn + flag +
/// sleep pattern (a receiver is single-consumer, so we can't poll it
/// twice — we race it in a task and assert on a shared flag).
#[tokio::test(flavor = "multi_thread")]
async fn cancel_inflight_returns_exit_signal_resolving_on_completion() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let session_active_request: Arc<Mutex<HashMap<String, String>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Stand up a full in-flight registration (token + exit receiver +
    // session→rid), matching what `chat` does on spawn.
    let token = CancellationToken::new();
    let (done_tx, done_rx) = oneshot::channel::<()>();
    {
        let mut m = cancellations.lock().await;
        m.insert("rid-1".to_string(), token.clone());
    }
    {
        let mut m = inflight_exits.lock().await;
        m.insert("rid-1".to_string(), done_rx);
    }
    {
        let mut m = session_active_request.lock().await;
        m.insert("s1".to_string(), "rid-1".to_string());
    }

    // Cancel + take the exit signal.
    let exit_rx = cancel_inflight_for_session(
        &cancellations,
        &session_active_request,
        &inflight_exits,
        "s1",
    )
    .await;
    assert!(token.is_cancelled(), "token should be cancelled");
    let exit_rx = exit_rx.expect("an in-flight request yields an exit signal");
    // The receiver was taken out of the map (single-consumer).
    assert!(
        inflight_exits.lock().await.is_empty(),
        "exit receiver should be drained from the map"
    );

    // Race the receiver in a task so we can assert on a shared flag
    // (a `oneshot::Receiver` can only be awaited once).
    let resolved = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let resolved_c = resolved.clone();
    let handle = tokio::spawn(async move {
        let _ = exit_rx.await;
        resolved_c.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    // The agent loop has NOT exited yet (producer silent) → the
    // awaiting task must stay pending.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !resolved.load(std::sync::atomic::Ordering::SeqCst),
        "exit signal must NOT resolve before the agent loop exits"
    );

    // Simulate the `chat` spawn closure finishing `run_chat_loop`.
    let _ = done_tx.send(());

    // Now the signal resolves promptly.
    tokio::time::timeout(Duration::from_millis(500), handle)
        .await
        .expect("await task should complete after the producer signals")
        .expect("task should not panic");
    assert!(
        resolved.load(std::sync::atomic::Ordering::SeqCst),
        "exit signal should resolve once the producer signals completion"
    );
}

// ---------------------------------------------------------------------------
// cancel_and_drain_all_agent_loops — daemon shutdown batch drain
// (task 07-24-daemon-agent-loop-shutdown). These tests exercise the
// batch counterpart to `cancel_inflight_for_session` +
// `await_inflight_exit`: it cancels every active token and
// concurrently awaits every exit signal under one total timeout.
// ---------------------------------------------------------------------------

/// Helper: build a `(cancellations, inflight_exits)` pair pre-seeded
/// with N fake in-flight requests, each with its own token + done
/// channel. Returns the maps plus the senders (so the test can fire
/// `done_tx.send(())` to simulate `run_chat_loop` returning) and the
/// cloned tokens (so the test can assert `is_cancelled()`).
struct SeededLoops {
    cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>>,
    tokens: Vec<CancellationToken>,
    done_txs: Vec<oneshot::Sender<()>>,
}

async fn seed_loops(n: usize) -> SeededLoops {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let mut tokens = Vec::with_capacity(n);
    let mut done_txs = Vec::with_capacity(n);
    for i in 0..n {
        let token = CancellationToken::new();
        let (tx, rx) = oneshot::channel::<()>();
        let rid = format!("rid-{i}");
        cancellations
            .lock()
            .await
            .insert(rid.clone(), token.clone());
        inflight_exits.lock().await.insert(rid, rx);
        tokens.push(token);
        done_txs.push(tx);
    }
    SeededLoops {
        cancellations,
        inflight_exits,
        tokens,
        done_txs,
    }
}

/// Cancels every active token. The core invariant: after the call,
/// every previously-registered `CancellationToken` reports
/// `is_cancelled()`.
#[tokio::test]
async fn cancel_and_drain_all_cancels_every_token() {
    let s = seed_loops(3).await;
    cancel_and_drain_all_agent_loops(&s.cancellations, &s.inflight_exits, Duration::from_secs(1))
        .await;
    for (i, t) in s.tokens.iter().enumerate() {
        assert!(
            t.is_cancelled(),
            "token {i} should be cancelled after the batch drain"
        );
    }
}

/// Empty maps → no-op, returns promptly, no panic. Guards the
/// "no active loops at shutdown" common case (must not block or
/// error when there's nothing to drain).
#[tokio::test]
async fn cancel_and_drain_all_on_empty_maps_is_noop() {
    let cancellations: Arc<Mutex<HashMap<String, CancellationToken>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    // Should return near-instantly (the empty-token early return).
    let () = tokio::time::timeout(
        Duration::from_millis(200),
        cancel_and_drain_all_agent_loops(&cancellations, &inflight_exits, Duration::from_secs(5)),
    )
    .await
    .expect("drain on empty maps must not block");
}

/// When every producer fires `done_tx.send(())` (the agent loops have
/// all exited), the concurrent drain resolves promptly — well before
/// the total timeout. This is the normal shutdown path.
#[tokio::test]
async fn cancel_and_drain_all_drains_when_producers_signal() {
    let s = seed_loops(3).await;
    // Fire all done senders (simulate the 3 agent loops exiting).
    // `.send` consumes the Sender; a `oneshot::Receiver` resolves
    // immediately even if the send happened before the await.
    let SeededLoops {
        cancellations,
        inflight_exits,
        done_txs,
        ..
    } = s;
    for tx in done_txs {
        let _ = tx.send(());
    }
    tokio::time::timeout(
        Duration::from_millis(500),
        cancel_and_drain_all_agent_loops(&cancellations, &inflight_exits, Duration::from_secs(2)),
    )
    .await
    .expect("drain must complete promptly when all producers have signaled");
    // Map should be drained of receivers.
    assert!(
        inflight_exits.lock().await.is_empty(),
        "inflight_exits should be drained"
    );
}

/// A producer that never sends (a hung / panicked agent loop whose
/// `done_tx` was dropped without `.send`) makes its receiver resolve
/// to `Err` immediately (sender dropped) — so the drain completes
/// promptly, NOT via timeout. This covers the panic-drop path.
#[tokio::test]
async fn cancel_and_drain_all_completes_when_sender_dropped() {
    let s = seed_loops(2).await;
    // Drop senders WITHOUT sending → receivers get `Err` (sender
    // dropped), resolving immediately. Drain must NOT hit timeout.
    let SeededLoops {
        cancellations,
        inflight_exits,
        done_txs,
        ..
    } = s;
    drop(done_txs);
    let start = std::time::Instant::now();
    cancel_and_drain_all_agent_loops(&cancellations, &inflight_exits, Duration::from_secs(2)).await;
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(300),
        "drain must return promptly when senders dropped (receivers resolve Err); took {:?}",
        elapsed
    );
}

/// The genuine timeout case: a producer that stays alive and never
/// sends (a truly hung agent loop). The drain must hit the total
/// timeout and return, not block. We keep the sender alive (leak it
/// into a spawned task) so the receiver stays pending forever.
#[tokio::test]
async fn cancel_and_drain_all_times_out_on_silent_live_producer() {
    let s = seed_loops(1).await;
    // Keep the sender alive by moving it into a detached task that
    // never sends. The receiver therefore stays pending forever,
    // so only the total timeout can end the drain.
    let SeededLoops {
        cancellations,
        inflight_exits,
        done_txs,
        ..
    } = s;
    let _keeper = tokio::spawn(async move {
        // `done_txs` is captured into this task; the task never
        // completes (pending forever), so the senders stay alive
        // and the receiver stays pending. Holding the senders via
        // capture (no explicit drop) keeps them alive for the
        // task's lifetime — exactly the "hung producer" we want.
        let _keep_alive = done_txs;
        std::future::pending::<()>().await;
    });
    let start = std::time::Instant::now();
    cancel_and_drain_all_agent_loops(&cancellations, &inflight_exits, Duration::from_millis(80))
        .await;
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(70),
        "drain must wait out the timeout when a producer stays silent; returned after {:?}",
        elapsed
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "drain must not run far past the timeout; took {:?}",
        elapsed
    );
}
