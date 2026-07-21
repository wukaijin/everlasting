//! Phase 2.2 daemon module (2026-07-21, task
//! `07-20-remote-access-daemon-split`).
//!
//! Hosts the axum HTTP stack that mirrors the 79
//! `#[tauri::command]`-equivalent handlers via REST + JSON. The
//! Tauri command surface stays the source of truth — every
//! business logic function is factored out as `xxx_inner` in
//! `commands/*.rs` and the daemon route handlers + the Tauri
//! command wrappers both call into the same `_inner` (Q0 decision,
//! design.md §5 "handler vs service").
//!
//! Layout:
//! - [`server`] — axum router assembly + graceful shutdown signal
//!   handler. The bin target (`src/bin/everlasting-daemon.rs`)
//!   calls `server::serve_daemon` after `AppState::load_from_dir`.
//! - [`error`] — `AppCommandError` → axum `IntoResponse` conversion
//!   (B6). Every handler returns `Result<Json<T>, AppCommandError>`;
//!   the `IntoResponse` impl maps the error category → HTTP status
//!   code + JSON body so the frontend sees the same wire shape on
//!   HTTP as it does on Tauri IPC.
//! - [`routes`] — per-domain handler modules. Each file mirrors one
//!   `commands/*.rs` module; handler functions call the
//!   corresponding `_inner` from the command module.
//!
//! Scope boundaries (P2.2 only — later phases add the rest):
//! - **P2.3** adds `sse.rs` (`HttpSseSink` + `HttpSseSubagentSink`
//!   + single-global `/api/v1/stream` endpoint) and the
//!   `/api/v1/sessions/{id}/snapshot` resync endpoint.
//! - **P2.4** adds `tower-http::services::ServeDir` for production
//!   single-binary serving of `app/dist/`.
//! - **P2.5** adds `tests/e2e.rs` integration harness.
//!
//! Phase 2.2 deliverable: 79 HTTP handlers reachable via curl, plus
//! `GET /api/v1/health` for Q1 port-conflict detection. The Tauri
//! command surface stays the production path until P2.4 flips
//! `httpTransport` to default.

pub mod error;
pub mod routes;
pub mod server;
pub mod sse;
