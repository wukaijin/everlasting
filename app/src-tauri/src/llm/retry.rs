//! LLM retry policy + Full Jitter backoff + first-byte-safe retry open
//! (A5+, 2026-07-04).
//!
//! Backoff is **Full Jitter** (`sleep = uniform(0, min(cap, base·2^attempt))`),
//! the AWS Architecture Blog consensus — pure exponential clusters concurrent
//! clients and is the documented loser. The server's `retry-after` advisory
//! (parsed by [`crate::llm::error::parse_retry_after`] into
//! [`LlmError::retry_after`](crate::llm::error::LlmError::retry_after))
//! overrides jitter when present.
//!
//! [`retry_open`] wraps `Provider::send` with first-byte-safe retry: it
//! re-issues the request on retryable first-byte failures (`Network` /
//! `Server` / `RateLimit`) up to `max_retries`, but **stops retrying the
//! instant any `Ok(ChatEvent)` arrives** (prd R3). Because the agent loop
//! only executes tools *after* the stream completes, retrying before the
//! first byte is provably side-effect-free — no idempotency key or dedup
//! table is needed (research §5.4).
//!
//! See `docs/research/llm-network-resilience-survey.md` for protocol facts
//! (no SSE resumption — only full re-issue is possible) and the safety
//! boundary (Claude Code's "before visible output" rule; OpenCode's
//! infinite-retry cautionary tale).

use std::pin::Pin;
use std::time::Duration;

use crate::llm::error::LlmError;
use crate::llm::provider::Provider;
use crate::llm::types::{ChatEvent, ChatMessage, ToolDef};
use futures_util::{Stream, StreamExt};
use tokio_util::sync::CancellationToken;

/// Retry tuning knobs. See [`RetryPolicy::default`] for the rationale behind
/// each value (research §6.1 / prd R4-R6).
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Max retry attempts (total requests = `max_retries + 1`).
    pub max_retries: u32,
    /// Full Jitter base — the `base` in `uniform(0, base·2^attempt)`.
    pub base: Duration,
    /// Full Jitter cap — upper bound on each computed sleep.
    pub cap: Duration,
    /// Total sleep budget across all retries — circuit breaker (prd R6).
    /// Once cumulative sleep reaches this, no further retry is attempted
    /// (avoids the OpenCode "session dies for hours" failure mode).
    pub budget: Duration,
    /// Ceiling on a server-advised `retry-after` (SDK parity — both Anthropic
    /// and OpenAI cap at 60s; longer advisories fall through to jitter).
    pub retry_after_cap: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        // prd R4/R6 recommended values (planning review 2026-07-04):
        // - max_retries=3: balances token burn vs resilience. SDK default is 2
        //   (too few for personal-use long sessions), Claude Code is 10 (too
        //   many for a non-billing-subsidized individual).
        // - base=0.5s, cap=30s: Full Jitter window. cap=30s (not SDK's 8s)
        //   gives a slow upstream time to recover.
        // - budget=60s: hard ceiling. Even with max_retries=3 + cap=30s, the
        //   worst-case 3×30s=90s would overshoot — budget clamps it.
        // - retry_after_cap=60s: matches RETRY_AFTER_CAP_SECS.
        Self {
            max_retries: 3,
            base: Duration::from_millis(500),
            cap: Duration::from_secs(30),
            budget: Duration::from_secs(60),
            retry_after_cap: Duration::from_secs(60),
        }
    }
}

impl RetryPolicy {
    /// Full Jitter sleep for attempt `n` (0-indexed): `uniform(0, min(cap, base·2^n))`.
    ///
    /// `attempt = 0` is the wait BEFORE the first retry (after the initial
    /// request failed). The exponent is clamped at 20 to avoid `pow` overflow
    /// on absurd `attempt` values (2^20 ms ≈ 18 min, far above any sane cap).
    pub fn full_jitter(&self, attempt: u32, rng: &mut fastrand::Rng) -> Duration {
        let base_ms = (self.base.as_millis().min(u64::MAX as u128)) as u64;
        let cap_ms = (self.cap.as_millis().min(u64::MAX as u128)) as u64;
        let factor = 2u64.pow(attempt.min(20));
        let upper_ms = base_ms.saturating_mul(factor).min(cap_ms);
        if upper_ms == 0 {
            return Duration::ZERO;
        }
        // fastrand::Rng::u64 accepts any RangeBounds; `0..=upper_ms` includes
        // the upper bound (Full Jitter is a closed interval).
        let ms = rng.u64(0..=upper_ms);
        Duration::from_millis(ms)
    }

    /// Compute the wait before the next retry attempt.
    ///
    /// - If the server left a `retry_after` advisory (on `LlmError::RateLimit`
    ///   / `Server`), honor it over Full Jitter.
    /// - Clamp the result to `budget_remaining` so the final retry can't
    ///   overshoot the circuit breaker (prd R6). The caller passes
    ///   `budget - total_elapsed_so_far`.
    ///
    /// An explicit `Some(Duration::ZERO)` advisory (server says "retry now")
    /// is honored as zero — it's the server telling us not to wait.
    pub fn wait(
        &self,
        attempt: u32,
        advisory: Option<Duration>,
        budget_remaining: Duration,
        rng: &mut fastrand::Rng,
    ) -> Duration {
        // `retry_after` was already capped at `RETRY_AFTER_CAP_SECS` (60s)
        // inside `classify_error_response`; `retry_after_cap` is a second,
        // policy-configurable ceiling (default 60s — parity — but a stricter
        // policy can clamp further). Then Full Jitter fallback, then the
        // circuit-breaker budget.

        advisory
            .map(|a| a.min(self.retry_after_cap))
            .unwrap_or_else(|| self.full_jitter(attempt, rng))
            .min(budget_remaining)
    }
}

/// Event emitted to the frontend before each retry backoff sleep, so the UI
/// can show "↩ retrying 2/3, 2s …" instead of looking frozen (prd R8).
#[derive(Debug, Clone)]
pub struct RetryingEvent {
    /// Which retry is about to start (1-indexed: 1 = first retry).
    pub attempt: u32,
    /// `policy.max_retries` — the ceiling, for "attempt/max" display.
    pub max_attempts: u32,
    /// How long we'll sleep before the next attempt (ms).
    pub wait_ms: u64,
    /// User-facing reason (from `LlmError::user_message`).
    pub reason: String,
}

/// Abstraction over the chat-event sink so [`retry_open`] can emit retry
/// notices without depending on the agent loop's concrete sink type. The
/// agent loop implements this to forward [`RetryingEvent`] to its own
/// `chat-event` IPC channel (Step 7).
pub trait RetrySink {
    fn emit_retrying(&self, event: RetryingEvent);
}

/// Outcome of opening a (possibly retried) LLM stream.
///
/// Both success and terminal failure surface as [`OpenOutcome::Stream`]:
/// - **first byte OK** → the stream is reassembled (`first` re-prepended via
///   `once(..).chain(rest)`) so the caller's existing per-event loop sees the
///   full sequence unchanged;
/// - **first byte failed terminally** (non-retryable, or `max_retries` /
///   `budget` exhausted) → a synthetic one-item `Err(e)` stream, which the
///   caller's loop handles via its normal `had_error` arm (RULE-A-007).
///
/// This keeps the caller (`chat_loop`) change-minimal: it replaces
/// `provider.send(..)` with `retry_open(..)` and matches `Stream | Cancelled`
/// — the per-event `select!` loop is untouched.
pub enum OpenOutcome {
    Stream(Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send>>),
    /// Cancelled during retry open (prd R7). The caller sets its cancel flag
    /// and the post-loop persist handles `CANCELLED_MARKER`.
    Cancelled,
}

/// Build a one-item `Err` stream — used when retry is exhausted or the error
/// is non-retryable. The caller's per-event loop sees this as a normal
/// `Some(Err(e))` and runs its existing `had_error` path (so ERROR_MARKER +
/// partial-turn persist work without specialization).
fn once_err(err: LlmError) -> Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send>> {
    Box::pin(futures_util::stream::once(async move { Err(err) }))
}

/// Open a Provider stream with first-byte-safe retry.
///
/// Re-issues `provider.send(...)` on retryable first-byte failures
/// (`Network` / `Server` / `RateLimit`) up to `policy.max_retries` times,
/// sleeping per Full Jitter (or the server's `retry_after` advisory when
/// present), bounded by `policy.budget`. Emits a [`RetryingEvent`] before
/// each sleep. **Once any `Ok(ChatEvent)` arrives, retry stops** — the
/// stream is live and the caller owns it.
///
/// Cancellation (prd R7): both the first-byte await and the backoff sleep
/// race against `token.cancelled()` (biased select, cancel wins); either
/// returns [`OpenOutcome::Cancelled`] immediately.
#[allow(clippy::too_many_arguments)]
pub async fn retry_open(
    provider: &dyn Provider,
    system: Option<String>,
    messages: Vec<ChatMessage>,
    tools: Vec<ToolDef>,
    policy: &RetryPolicy,
    token: &CancellationToken,
    sink: &impl RetrySink,
    rng: &mut fastrand::Rng,
) -> OpenOutcome {
    let mut attempt: u32 = 0;
    let mut total_elapsed = Duration::ZERO;
    loop {
        if token.is_cancelled() {
            return OpenOutcome::Cancelled;
        }
        let mut stream = provider.send(system.clone(), messages.clone(), tools.clone());
        // Race the first byte against cancellation.
        let first = tokio::select! {
            biased;
            _ = token.cancelled() => return OpenOutcome::Cancelled,
            item = stream.next() => item,
        };
        let err = match first {
            Some(Ok(ev)) => {
                // First byte OK — re-prepend the event so the caller's
                // per-event loop sees the full sequence unchanged.
                return OpenOutcome::Stream(Box::pin(
                    futures_util::stream::once(async move { Ok(ev) }).chain(stream),
                ));
            }
            Some(Err(e)) => e,
            None => LlmError::Network("stream ended before any event".into()),
        };
        // First byte failed. Decide whether to retry.
        if !err.is_retryable() || attempt >= policy.max_retries {
            return OpenOutcome::Stream(once_err(err));
        }
        let budget_remaining = policy.budget.checked_sub(total_elapsed).unwrap_or_default();
        if budget_remaining == Duration::ZERO {
            // Circuit breaker tripped (prd R6).
            return OpenOutcome::Stream(once_err(err));
        }
        let wait = policy.wait(attempt, err.retry_after(), budget_remaining, rng);
        sink.emit_retrying(RetryingEvent {
            attempt: attempt + 1,
            max_attempts: policy.max_retries,
            wait_ms: u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
            reason: err.user_message(),
        });
        attempt += 1;
        total_elapsed += wait;
        // Race the backoff sleep against cancellation (prd R7).
        tokio::select! {
            biased;
            _ = token.cancelled() => return OpenOutcome::Cancelled,
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::mock::{MockProvider, MockResponse};
    use crate::llm::types::TokenUsage;
    use std::sync::Mutex;
    use std::time::Instant;

    fn policy() -> RetryPolicy {
        RetryPolicy::default()
    }

    /// Tiny policy for retry_open tests so backoff sleeps are sub-10ms and
    /// the suite stays fast.
    fn fast_policy() -> RetryPolicy {
        RetryPolicy {
            max_retries: 3,
            base: Duration::from_millis(1),
            cap: Duration::from_millis(5),
            budget: Duration::from_millis(20),
            retry_after_cap: Duration::from_secs(60),
        }
    }

    /// Collecting sink so tests can assert on emitted retry notices.
    #[derive(Default)]
    struct MockSink {
        events: Mutex<Vec<RetryingEvent>>,
    }
    impl RetrySink for MockSink {
        fn emit_retrying(&self, event: RetryingEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    fn retrying_count(sink: &MockSink) -> usize {
        sink.events.lock().unwrap().len()
    }

    // -------- Step 2: RetryPolicy / full_jitter / wait --------

    #[test]
    fn default_policy_matches_research_defaults() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_retries, 3);
        assert_eq!(p.base, Duration::from_millis(500));
        assert_eq!(p.cap, Duration::from_secs(30));
        assert_eq!(p.budget, Duration::from_secs(60));
        assert_eq!(p.retry_after_cap, Duration::from_secs(60));
    }

    #[test]
    fn full_jitter_attempt_zero_window_is_base() {
        let p = policy();
        let mut rng = fastrand::Rng::with_seed(0);
        for _ in 0..100 {
            let wait = p.full_jitter(0, &mut rng);
            assert!(wait <= Duration::from_millis(500), "wait {:?} > base", wait);
        }
    }

    #[test]
    fn full_jitter_large_attempt_caps_at_cap() {
        let p = policy();
        let mut rng = fastrand::Rng::with_seed(1);
        for _ in 0..100 {
            let wait = p.full_jitter(100, &mut rng);
            assert!(wait <= Duration::from_secs(30), "wait {:?} > cap", wait);
        }
    }

    #[test]
    fn full_jitter_never_negative() {
        let p = policy();
        let mut rng = fastrand::Rng::with_seed(2);
        for attempt in 0..20 {
            assert!(p.full_jitter(attempt, &mut rng) >= Duration::ZERO);
        }
    }

    #[test]
    fn full_jitter_deterministic_with_seed() {
        let p = policy();
        let mut a = fastrand::Rng::with_seed(42);
        let mut b = fastrand::Rng::with_seed(42);
        for attempt in 0..6 {
            assert_eq!(
                p.full_jitter(attempt, &mut a),
                p.full_jitter(attempt, &mut b),
                "divergence at attempt {}",
                attempt
            );
        }
    }

    #[test]
    fn full_jitter_zero_base_does_not_panic() {
        let p = RetryPolicy {
            base: Duration::ZERO,
            ..policy()
        };
        let mut rng = fastrand::Rng::with_seed(0);
        assert_eq!(p.full_jitter(0, &mut rng), Duration::ZERO);
    }

    #[test]
    fn wait_advisory_overrides_jitter() {
        let p = policy();
        let mut rng = fastrand::Rng::with_seed(0);
        let wait = p.wait(
            0,
            Some(Duration::from_secs(5)),
            Duration::from_secs(60),
            &mut rng,
        );
        assert_eq!(wait, Duration::from_secs(5));
    }

    #[test]
    fn wait_falls_back_to_jitter_without_advisory() {
        let p = policy();
        let mut rng = fastrand::Rng::with_seed(0);
        let wait = p.wait(0, None, Duration::from_secs(60), &mut rng);
        assert!(wait <= Duration::from_millis(500));
    }

    #[test]
    fn wait_clamps_to_budget_remaining() {
        let p = policy();
        let mut rng = fastrand::Rng::with_seed(0);
        let wait = p.wait(
            0,
            Some(Duration::from_secs(5)),
            Duration::from_secs(2),
            &mut rng,
        );
        assert_eq!(wait, Duration::from_secs(2));
    }

    #[test]
    fn wait_explicit_zero_advisory_honored() {
        let p = policy();
        let mut rng = fastrand::Rng::with_seed(0);
        let wait = p.wait(0, Some(Duration::ZERO), Duration::from_secs(60), &mut rng);
        assert_eq!(wait, Duration::ZERO);
    }

    // -------- Step 3: retry_open integration --------

    fn ok_turn() -> MockResponse {
        // A minimal successful stream: Start, then Done.
        MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::Done {
                stop_reason: Some("end_turn".into()),
                usage: Some(TokenUsage::default()),
            }),
        ])
    }

    // LlmError is non-Clone by design (mock.rs), so scripts that need the
    // same error shape N times build N fresh instances via these helpers.
    fn server_503() -> LlmError {
        LlmError::Server {
            status: 503,
            message: "x".into(),
            retry_after: None,
        }
    }
    fn rate_limit_advisory(d: Duration) -> LlmError {
        LlmError::RateLimit {
            message: "x".into(),
            retry_after: Some(d),
        }
    }

    #[tokio::test]
    async fn retry_open_succeeds_after_retryable_failure() {
        // First send: 503 (retryable). Second send: success.
        let mock = MockProvider::new(vec![
            MockResponse::ErrThenEnd(LlmError::Server {
                status: 503,
                message: "service unavailable".into(),
                retry_after: None,
            }),
            ok_turn(),
        ]);
        let sink = MockSink::default();
        let mut rng = fastrand::Rng::with_seed(0);
        let token = CancellationToken::new();
        let outcome = retry_open(
            &mock,
            None,
            vec![],
            vec![],
            &fast_policy(),
            &token,
            &sink,
            &mut rng,
        )
        .await;
        let mut stream = match outcome {
            OpenOutcome::Stream(s) => s,
            _ => panic!("expected Stream after retry"),
        };
        let first = stream.next().await.unwrap().expect("Ok(Start)");
        assert!(matches!(first, ChatEvent::Start));
        assert_eq!(mock.call_count(), 2); // initial fail + 1 retry
        assert_eq!(retrying_count(&sink), 1);
    }

    #[tokio::test]
    async fn retry_open_non_retryable_error_does_not_retry() {
        // Auth is deterministic — no retry.
        let mock = MockProvider::new(vec![MockResponse::ErrThenEnd(LlmError::Auth(
            "bad key".into(),
        ))]);
        let sink = MockSink::default();
        let mut rng = fastrand::Rng::with_seed(0);
        let token = CancellationToken::new();
        let outcome = retry_open(
            &mock,
            None,
            vec![],
            vec![],
            &fast_policy(),
            &token,
            &sink,
            &mut rng,
        )
        .await;
        let mut stream = match outcome {
            OpenOutcome::Stream(s) => s,
            _ => panic!("expected Stream (terminal Err)"),
        };
        let first = stream.next().await.unwrap().expect_err("Err(Auth)");
        assert!(matches!(first, LlmError::Auth(_)));
        assert_eq!(mock.call_count(), 1);
        assert_eq!(retrying_count(&sink), 0);
    }

    #[tokio::test]
    async fn retry_open_max_retries_exhausted() {
        // 4 consecutive 503s with max_retries=3 → total 4 sends, final Err.
        let mock = MockProvider::new(vec![
            MockResponse::ErrThenEnd(server_503()),
            MockResponse::ErrThenEnd(server_503()),
            MockResponse::ErrThenEnd(server_503()),
            MockResponse::ErrThenEnd(server_503()),
        ]);
        let sink = MockSink::default();
        let mut rng = fastrand::Rng::with_seed(0);
        let token = CancellationToken::new();
        let outcome = retry_open(
            &mock,
            None,
            vec![],
            vec![],
            &fast_policy(),
            &token,
            &sink,
            &mut rng,
        )
        .await;
        let mut stream = match outcome {
            OpenOutcome::Stream(s) => s,
            _ => panic!("expected Stream (terminal Err)"),
        };
        let first = stream.next().await.unwrap().expect_err("Err(Server)");
        assert!(matches!(first, LlmError::Server { .. }));
        assert_eq!(mock.call_count(), 4); // 1 initial + 3 retries
        assert_eq!(retrying_count(&sink), 3);
    }

    #[tokio::test]
    async fn retry_open_first_byte_after_no_retry() {
        // Stream emits Ok(Start) then Err — first byte is OK, so retry_open
        // returns Ok and never re-issues. The mid-stream Err is left in `rest`
        // for the agent loop's existing had_error path (prd R3).
        let mock = MockProvider::new(vec![MockResponse::Events(vec![
            Ok(ChatEvent::Start),
            Err(LlmError::Server {
                status: 503,
                message: "mid-stream drop".into(),
                retry_after: None,
            }),
        ])]);
        let sink = MockSink::default();
        let mut rng = fastrand::Rng::with_seed(0);
        let token = CancellationToken::new();
        let outcome = retry_open(
            &mock,
            None,
            vec![],
            vec![],
            &fast_policy(),
            &token,
            &sink,
            &mut rng,
        )
        .await;
        let mut stream = match outcome {
            OpenOutcome::Stream(s) => s,
            _ => panic!("expected Stream (first byte succeeded)"),
        };
        // First event (re-prepended): Ok(Start).
        let first = stream.next().await.unwrap().expect("Ok(Start)");
        assert!(matches!(first, ChatEvent::Start));
        // Next item is the mid-stream Err — carried through, NOT retried.
        let next = stream.next().await;
        assert!(matches!(next, Some(Err(LlmError::Server { .. }))));
        assert_eq!(mock.call_count(), 1); // NO retry — first byte was OK
        assert_eq!(retrying_count(&sink), 0);
    }

    #[tokio::test]
    async fn retry_open_honors_retry_after_advisory() {
        // advisory 15ms > fast_policy cap 5ms → wait must be the 15ms advisory,
        // not a jitter value ≤ 5ms. Proves advisory overrides jitter.
        let mock = MockProvider::new(vec![
            MockResponse::ErrThenEnd(LlmError::RateLimit {
                message: "slow down".into(),
                retry_after: Some(Duration::from_millis(15)),
            }),
            ok_turn(),
        ]);
        let sink = MockSink::default();
        let mut rng = fastrand::Rng::with_seed(0);
        let token = CancellationToken::new();
        let start = Instant::now();
        let outcome = retry_open(
            &mock,
            None,
            vec![],
            vec![],
            &fast_policy(),
            &token,
            &sink,
            &mut rng,
        )
        .await;
        let elapsed = start.elapsed();
        assert!(matches!(outcome, OpenOutcome::Stream(_)));
        assert!(
            elapsed >= Duration::from_millis(15),
            "advisory 15ms not honored, elapsed {:?}",
            elapsed
        );
        let evs = sink.events.lock().unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].wait_ms, 15);
        assert_eq!(evs[0].attempt, 1);
        assert_eq!(evs[0].max_attempts, 3);
        assert!(evs[0].reason.contains("频繁") || evs[0].reason.contains("请求"));
    }

    #[tokio::test]
    async fn retry_open_cancelled_during_backoff_sleep() {
        // Large base/cap → backoff sleep ~10s. Cancel after 50ms must return
        // Cancelled immediately (prd R7), with only the initial failed send.
        let mock = MockProvider::new(vec![MockResponse::ErrThenEnd(LlmError::Server {
            status: 503,
            message: "x".into(),
            retry_after: None,
        })]);
        let sink = MockSink::default();
        let mut rng = fastrand::Rng::with_seed(0);
        let token = CancellationToken::new();
        let policy = RetryPolicy {
            base: Duration::from_secs(10),
            cap: Duration::from_secs(10),
            budget: Duration::from_secs(60),
            max_retries: 3,
            retry_after_cap: Duration::from_secs(60),
        };
        let cancel_token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_token.cancel();
        });
        let start = Instant::now();
        let outcome = retry_open(
            &mock,
            None,
            vec![],
            vec![],
            &policy,
            &token,
            &sink,
            &mut rng,
        )
        .await;
        let elapsed = start.elapsed();
        assert!(matches!(outcome, OpenOutcome::Cancelled));
        assert_eq!(mock.call_count(), 1); // only the initial failed send
        assert!(
            elapsed < Duration::from_secs(2),
            "cancel should return promptly, elapsed {:?}",
            elapsed
        );
        assert_eq!(retrying_count(&sink), 1); // retrying was emitted before sleep
    }

    #[tokio::test]
    async fn retry_open_budget_breaker_stops_retry() {
        // budget=5ms, each err carries retry_after=10s advisory → first retry
        // wait clamped to 5ms (budget_remaining), total_elapsed becomes 5ms,
        // second failure sees budget_remaining=0 → Err without retry.
        let mock = MockProvider::new(vec![
            MockResponse::ErrThenEnd(rate_limit_advisory(Duration::from_secs(10))),
            MockResponse::ErrThenEnd(rate_limit_advisory(Duration::from_secs(10))),
        ]);
        let sink = MockSink::default();
        let mut rng = fastrand::Rng::with_seed(0);
        let token = CancellationToken::new();
        let policy = RetryPolicy {
            budget: Duration::from_millis(5),
            max_retries: 5,
            ..fast_policy()
        };
        let outcome = retry_open(
            &mock,
            None,
            vec![],
            vec![],
            &policy,
            &token,
            &sink,
            &mut rng,
        )
        .await;
        let mut stream = match outcome {
            OpenOutcome::Stream(s) => s,
            _ => panic!("expected Stream (terminal Err)"),
        };
        let first = stream.next().await.unwrap().expect_err("Err(RateLimit)");
        assert!(matches!(first, LlmError::RateLimit { .. }));
        assert_eq!(mock.call_count(), 2); // initial + 1 retry, then budget tripped
        assert_eq!(retrying_count(&sink), 1); // only the first retry emitted a notice
    }

    #[tokio::test]
    async fn retry_open_immediate_cancel_before_first_send() {
        // Token already cancelled when retry_open starts → Cancelled without
        // any send (the loop's top cancel check fires first).
        let mock = MockProvider::new(vec![]);
        let sink = MockSink::default();
        let mut rng = fastrand::Rng::with_seed(0);
        let token = CancellationToken::new();
        token.cancel();
        let outcome = retry_open(
            &mock,
            None,
            vec![],
            vec![],
            &fast_policy(),
            &token,
            &sink,
            &mut rng,
        )
        .await;
        assert!(matches!(outcome, OpenOutcome::Cancelled));
        assert_eq!(mock.call_count(), 0);
        assert_eq!(retrying_count(&sink), 0);
    }

    // -------- Step 8: budget / max_retries circuit-breaker edges (R6) --------

    /// Budget fires BEFORE max_retries: with `budget = 12ms` and a
    /// 10s advisory on each failure, the first retry's wait clamps
    /// to budget_remaining (12ms), `total_elapsed` becomes 12ms,
    /// and the second failure sees `budget_remaining == 0` →
    /// returns Err without emitting another retrying notice. Total:
    /// 2 sends, 1 retrying event. Proves R6's "budget circuit-
    /// breaker trips independent of max_retries" (max_retries=10
    /// here would otherwise allow 10 retries).
    #[tokio::test]
    async fn retry_open_budget_breaks_before_max_retries() {
        // max_retries=10 (generous) so budget is the only ceiling.
        let policy = RetryPolicy {
            max_retries: 10,
            base: Duration::from_millis(1),
            cap: Duration::from_millis(5),
            budget: Duration::from_millis(12),
            retry_after_cap: Duration::from_secs(60),
        };
        // Each failure carries a 10s advisory so wait() clamps to
        // budget_remaining rather than jittering into sub-ms territory.
        let mock = MockProvider::new(vec![
            MockResponse::ErrThenEnd(rate_limit_advisory(Duration::from_secs(10))),
            MockResponse::ErrThenEnd(rate_limit_advisory(Duration::from_secs(10))),
            MockResponse::ErrThenEnd(rate_limit_advisory(Duration::from_secs(10))),
            MockResponse::ErrThenEnd(rate_limit_advisory(Duration::from_secs(10))),
        ]);
        let sink = MockSink::default();
        let mut rng = fastrand::Rng::with_seed(0);
        let token = CancellationToken::new();
        let outcome = retry_open(
            &mock,
            None,
            vec![],
            vec![],
            &policy,
            &token,
            &sink,
            &mut rng,
        )
        .await;
        let mut stream = match outcome {
            OpenOutcome::Stream(s) => s,
            _ => panic!("expected Stream (terminal Err after budget trip)"),
        };
        let first = stream.next().await.unwrap().expect_err("Err(RateLimit)");
        assert!(matches!(first, LlmError::RateLimit { .. }));
        // 1 initial send + 1 retry (the retry's 10s advisory
        // clamped to the 12ms budget, so total_elapsed = 12ms).
        // The 2nd retry sees budget_remaining=0 and stops without
        // emitting a notice. Budget tripped at attempt=1 even
        // though max_retries=10.
        assert_eq!(
            mock.call_count(),
            2,
            "budget should trip after 1 retry, got {}",
            mock.call_count()
        );
        assert_eq!(retrying_count(&sink), 1);
    }

    /// max_retries fires BEFORE budget: with the default fast
    /// policy (max_retries=3, budget=20ms, cap=5ms), 4 consecutive
    /// 503s exhaust max_retries. The cumulative sleep (3 jitters
    /// ≤ 5ms each) is well under the 20ms budget — max_retries is
    /// the binding constraint. Proves the two ceilings are
    /// independent and either can trip first.
    #[tokio::test]
    async fn retry_open_max_retries_breaks_before_budget() {
        let policy = fast_policy(); // max_retries=3, budget=20ms
        let mock = MockProvider::new(vec![
            MockResponse::ErrThenEnd(server_503()),
            MockResponse::ErrThenEnd(server_503()),
            MockResponse::ErrThenEnd(server_503()),
            MockResponse::ErrThenEnd(server_503()),
        ]);
        let sink = MockSink::default();
        let mut rng = fastrand::Rng::with_seed(0);
        let token = CancellationToken::new();
        let outcome = retry_open(
            &mock,
            None,
            vec![],
            vec![],
            &policy,
            &token,
            &sink,
            &mut rng,
        )
        .await;
        let mut stream = match outcome {
            OpenOutcome::Stream(s) => s,
            _ => panic!("expected Stream (terminal Err)"),
        };
        let first = stream.next().await.unwrap().expect_err("Err(Server)");
        assert!(matches!(first, LlmError::Server { .. }));
        // 1 initial + 3 retries = 4 sends (max_retries exhausted).
        assert_eq!(mock.call_count(), 4);
        assert_eq!(retrying_count(&sink), 3);
    }

    /// wait() clamps advisory to budget_remaining: when the next
    /// retry's advisory would overshoot the remaining budget, the
    /// wait is truncated. This is the "circuit-breaker clamp" that
    /// prevents the final retry's backoff from blowing past the
    /// budget ceiling — prd R6 + research §6.1.
    #[test]
    fn wait_clamps_advisory_to_remaining_budget_so_total_does_not_overshoot() {
        let p = policy();
        let mut rng = fastrand::Rng::with_seed(0);
        // 30s advisory, only 5s remaining → wait must clamp to 5s
        // (NOT the 30s advisory, NOT the policy's 60s cap).
        let wait = p.wait(
            0,
            Some(Duration::from_secs(30)),
            Duration::from_secs(5),
            &mut rng,
        );
        assert_eq!(wait, Duration::from_secs(5));
    }

    /// Budget exhausted between retries: when budget_remaining
    /// hits exactly zero, retry_open returns Stream(Err) on the
    /// next failure WITHOUT emitting another retrying event — even
    /// though attempt < max_retries. This is the precise boundary
    /// the circuit breaker enforces.
    #[tokio::test]
    async fn retry_open_zero_budget_remaining_stops_without_emitting_retry_notice() {
        // budget=10ms, single 10s advisory → first retry sleep
        // clamps to 10ms (= budget), total_elapsed becomes 10ms.
        // Second failure sees budget_remaining = 0 → immediate Err.
        let policy = RetryPolicy {
            max_retries: 5,
            base: Duration::from_millis(1),
            cap: Duration::from_millis(5),
            budget: Duration::from_millis(10),
            retry_after_cap: Duration::from_secs(60),
        };
        let mock = MockProvider::new(vec![
            MockResponse::ErrThenEnd(rate_limit_advisory(Duration::from_secs(10))),
            MockResponse::ErrThenEnd(rate_limit_advisory(Duration::from_secs(10))),
            MockResponse::ErrThenEnd(rate_limit_advisory(Duration::from_secs(10))),
        ]);
        let sink = MockSink::default();
        let mut rng = fastrand::Rng::with_seed(0);
        let token = CancellationToken::new();
        let outcome = retry_open(
            &mock,
            None,
            vec![],
            vec![],
            &policy,
            &token,
            &sink,
            &mut rng,
        )
        .await;
        let mut stream = match outcome {
            OpenOutcome::Stream(s) => s,
            _ => panic!("expected Stream (terminal Err)"),
        };
        let first = stream.next().await.unwrap().expect_err("Err(RateLimit)");
        assert!(matches!(first, LlmError::RateLimit { .. }));
        // 1 initial + 1 retry (the 1st advisory clamped to budget
        // 10ms). The 2nd retry sees budget_remaining=0 and stops.
        assert_eq!(mock.call_count(), 2);
        assert_eq!(retrying_count(&sink), 1);
    }
}
