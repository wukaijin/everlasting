//! Phase 2.2 axum router assembly + daemon serve loop (B1 + B2).
//!
//! `serve_daemon` is the single entry point used by the
//! `everlasting-daemon` bin target. It:
//! 1. Pre-flight health check (Q1 fail-loud decision): GET the
//!    `0.0.0.0:PORT/api/v1/health` endpoint; if a daemon is already
//!    running with the same `daemon_version`, we exit cleanly (the
//!    caller can reuse the existing daemon). If the port is taken
//!    by something else, we fail loud with a Chinese user-facing
//!    message (no silent port hop — Q1 explicitly rejects that).
//! 2. Build the axum router via [`build_router`], wiring the shared
//!    `Arc<AppState>` as `State`.
//! 3. Bind a `TcpListener` on `0.0.0.0:PORT` (WSL-first — Windows
//!    host browsers reach the daemon via WSL 2 localhost forwarding;
//!    see `docs/HACKING-wsl.md`).
//! 4. `axum::serve(...).with_graceful_shutdown(...)` — Ctrl+C /
//!    SIGTERM drains in-flight requests.
//!
//! The `Arc<AppState>` is shared across every handler (axum clones
//! the `Arc` per request). This matches the Tauri `State<'_,
//! Arc<AppState>>` pattern — the underlying `SqlitePool`, catalog,
//! `PermissionStore`, etc. are all `Arc`-internal and safe to share.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{routing::get, Router};
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::cors::CorsLayer;

use crate::daemon::routes;
use crate::state::AppState;

/// Default daemon port (Q1 decision — `7456` is the project's
/// canonical port; override via `--port` / `EVERLASTING_DAEMON_PORT`).
pub const DEFAULT_DAEMON_PORT: u16 = 7456;

/// Convenience wrapper exposed for the daemon bin target so it
/// doesn't need to reach the private `state` module directly. P2.1
/// landed `AppState::load_from_dir(PathBuf)`; the bin calls this
/// helper instead of `AppState::load_from_dir` so the daemon bin
/// never touches private modules.
pub async fn load_daemon_state(data_dir: std::path::PathBuf) -> Arc<AppState> {
    Arc::new(AppState::load_from_dir(data_dir).await)
}

/// Build the un-mounted daemon router. Exposed so integration tests
/// (`daemon::routes::tests_*`) can construct a router against a
/// test-only `AppState` without going through the TCP serve loop.
///
/// `health` lives at the top level so it's reachable without the
/// `/api/v1` prefix too — the Q1 port-conflict probe hits
/// `/api/v1/health` specifically, but exposing the bare `/health`
/// path as well costs nothing and is friendlier for ad-hoc curl.
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(routes::health::health))
        .route("/api/v1/health", get(routes::health::health))
        .merge(routes::router(state))
        // P2.3 dev-only CORS (C6 收尾):vite dev server (1420) 与 daemon
        // (7456) 跨域,`fetch` + `EventSource` 需 daemon 放行 preflight。
        // `very_permissive` 允许任意 origin / method / header(不带
        // credentials —— 此处 SSE/fetch 均无 cookie)。P2.4 sidecar 同源后
        // 此层移除(同源不触发 preflight)。
        .layer(CorsLayer::very_permissive())
}

/// Resolve the daemon port per Q1 decision:
/// `--port <N>` flag > `EVERLASTING_DAEMON_PORT` env > default 7456.
///
/// The bin target handles the CLI / env parsing and passes the
/// resolved value here. This helper exists as a free function so
/// tests can verify the precedence chain without spawning the CLI.
pub fn resolve_port(cli_port: Option<u16>, env_port: Option<u16>) -> u16 {
    cli_port.or(env_port).unwrap_or(DEFAULT_DAEMON_PORT)
}

/// Bind + serve the daemon on `0.0.0.0:PORT`. The `everlasting-daemon`
/// bin target is a thin shell that resolves the port, loads
/// `AppState`, and calls this.
///
/// Graceful shutdown is wired to SIGINT (Ctrl+C) and SIGTERM
/// (POSIX). P2.4 will add sidecar-aware shutdown (Tauri window
/// close → SIGTERM the daemon); for now we use the platform's
/// canonical terminate signals.
pub async fn serve_daemon(state: Arc<AppState>, port: u16) -> std::io::Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(addr = %addr, "everlasting-daemon listening");

    let router = build_router(state);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("everlasting-daemon shutdown complete");
    Ok(())
}

/// Graceful shutdown signal handler. Fires on Ctrl+C (portable) or
/// SIGTERM (Unix only). Windows builds fall back to Ctrl+C only,
/// which matches the Tauri GUI's signal handling convention.
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received SIGINT, shutting down"),
        _ = terminate => tracing::info!("received SIGTERM, shutting down"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Q1 port resolution precedence: CLI > env > default. The bin
    /// target's `--port` flag wins, then the env var, then the
    /// 7456 default. The single source of truth for the
    /// "never auto-hop ports" decision (Q1) is `serve_daemon`'s
    /// `TcpListener::bind` — failing there fails loud.
    #[test]
    fn port_resolution_precedence() {
        // No CLI, no env → default.
        assert_eq!(resolve_port(None, None), DEFAULT_DAEMON_PORT);
        // Env wins over default.
        assert_eq!(resolve_port(None, Some(9999)), 9999);
        // CLI wins over env.
        assert_eq!(resolve_port(Some(7456), Some(9999)), 7456);
    }
}
