//! `GET /api/v1/health` — daemon health probe (P2.2 B3).
//!
//! Used by:
//! - The Q1 port-conflict probe inside `everlasting-daemon` itself
//!   (a fresh daemon process hits this endpoint before binding; if
//!   the response carries the same `daemon_version`, the new
//!   process exits cleanly so the user reuses the existing daemon).
//! - The P2.4 GUI sidecar handshake — the Tauri GUI pings this
//!   endpoint after spawning the daemon, waiting up to N seconds
//!   before flipping `httpTransport` to default (Q5 layered
//!   validation: protocol version mismatch = fail loud, build
//!   version mismatch = warn only).
//! - Ad-hoc operator `curl` checks.
//!
//! Wire shape (PRD R1 + design.md §3.4) — deliberately stateless
//! (no `AppState`; see `health()` doc for the Q1 rationale):
//! ```json
//! 200 OK
//! {
//!   "daemon_id": "uuid-v4",
//!   "daemon_version": "0.1.0",
//!   "api_versions": ["v1"],
//!   "uptime_seconds": 3600
//! }
//! ```
//!
//! `daemon_id` is generated once per process at first request and
//! reused for the process lifetime. A daemon restart produces a
//! fresh id, so the GUI can detect "the daemon I was talking to
//! got restarted" (the new id won't match the cached one).

use std::sync::OnceLock;
use std::time::Instant;

use axum::{response::IntoResponse, Json};
use serde::Serialize;
use uuid::Uuid;

/// API versions this daemon speaks. P2.2 ships `v1` only; `v2` is
/// reserved for a future breaking change (PRD §4.3).
pub const SUPPORTED_API_VERSIONS: &[&str] = &["v1"];

/// Daemon version, sourced from `env!("CARGO_PKG_VERSION")`. The
/// GUI sidecar handshake (Q5) compares this against its own build
/// version to decide warn-only vs fail-loud.
pub const DAEMON_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Process start time, captured at first health-request. Used to
/// populate `uptime_seconds` in the response. Storing `Instant` is
/// fine because daemon + GUI are on the same host (no clock skew).
static START_TIME: OnceLock<Instant> = OnceLock::new();

/// Process-wide unique id, captured at first health-request.
static DAEMON_ID: OnceLock<String> = OnceLock::new();

/// Response body for `GET /api/v1/health`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    /// Process-unique id (UUID v4). Stable across requests within
    /// one process; changes on daemon restart.
    pub daemon_id: String,
    /// Crate version (from `Cargo.toml`). Matches the GUI build
    /// version when both are built from the same commit.
    pub daemon_version: String,
    /// Supported API versions, sorted chronologically. The GUI
    /// requires `"v1"` to be present (Q5 protocol-version gate).
    pub api_versions: Vec<&'static str>,
    /// Wall-clock seconds since first health-request. Approximate
    /// (process start would be more accurate but `Instant::now()`
    /// captured lazily keeps the `static` initialization simple).
    pub uptime_seconds: u64,
}

/// `GET /api/v1/health` handler.
///
/// Takes **no parameters**, intentionally stateless — the Q1
/// port-conflict probe hits this endpoint *before the new daemon
/// process has loaded its own `AppState`* (the probe is asking
/// the *other* daemon squatting on the port), and wiring
/// `State<Arc<AppState>>` into this route would require the
/// daemon's state-bearing router. Instead,
/// `daemon::server::build_router` mounts `/api/v1/health` on the
/// state-less top-level router; the Q1 probe's port-conflict
/// `reqwest::get` works against any axum instance that answers.
/// If a state-bearing variant is ever needed (e.g. a live session
/// count), add a separate `/api/v1/health/detailed` route rather
/// than changing this one.
pub async fn health() -> impl IntoResponse {
    let start = *START_TIME.get_or_init(Instant::now);
    let daemon_id = DAEMON_ID.get_or_init(|| Uuid::new_v4().to_string()).clone();
    let uptime_seconds = start.elapsed().as_secs();

    Json(HealthResponse {
        daemon_id,
        daemon_version: DAEMON_VERSION.to_string(),
        api_versions: SUPPORTED_API_VERSIONS.to_vec(),
        uptime_seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// End-to-end smoke: build a 1-route router with just `health`,
    /// fire a `GET /health`, assert the canonical 200 + JSON body
    /// fields. The point is verifying the wire shape matches
    /// design.md §3.4 so the Q1 port-conflict probe + Q5 GUI
    /// handshake contracts are honored.
    #[tokio::test]
    async fn health_returns_canonical_shape() {
        let app = axum::Router::new().route("/health", axum::routing::get(health));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("router succeeded");

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body collected");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("body is valid JSON");
        // Canonical fields (camelCase per HealthResponse's serde attr).
        assert!(json.get("daemonId").is_some(), "daemonId present");
        assert!(json.get("daemonVersion").is_some(), "daemonVersion present");
        let api_versions: Vec<String> =
            serde_json::from_value(json.get("apiVersions").cloned().unwrap_or_default())
                .expect("apiVersions deserializes");
        assert!(
            api_versions.iter().any(|v| v == "v1"),
            "api_versions contains v1 (Q5 protocol-version gate)"
        );
        assert!(
            json.get("uptimeSeconds").and_then(|v| v.as_u64()).is_some(),
            "uptimeSeconds is a non-negative integer"
        );
        // The stateless handler reports no session count at all —
        // `sessionCount` must be absent (removed with the -1
        // sentinel; no consumer ever read it).
        assert!(
            json.get("sessionCount").is_none(),
            "sessionCount is absent from the wire shape"
        );
    }

    /// `SUPPORTED_API_VERSIONS` is a compile-time constant — the
    /// Q5 protocol-version gate hard-codes `"v1"` so the GUI + daemon
    /// agree on the protocol. Adding a `v2` is a breaking change
    /// (PRD §4.3) and MUST go through a deprecation cycle.
    #[test]
    fn supported_api_versions_contains_v1() {
        assert!(SUPPORTED_API_VERSIONS.contains(&"v1"));
    }
}
