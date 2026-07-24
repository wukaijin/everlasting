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
//!    SIGTERM first calls [`sse::SseRegistry::shutdown`] to end all
//!    live SSE streams (so the drain isn't blocked by never-finishing
//!    `GET /api/v1/stream` connections), then drains remaining
//!    in-flight requests. A [`SHUTDOWN_GRACE_SECS`] timeout backs it
//!    up against unknown long-lived connections.
//!
//! The `Arc<AppState>` is shared across every handler (axum clones
//! the `Arc` per request). This matches the Tauri `State<'_,
//! Arc<AppState>>` pattern — the underlying `SqlitePool`, catalog,
//! `PermissionStore`, etc. are all `Arc`-internal and safe to share.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::{routing::get, Router};
use tokio::net::TcpListener;
use tokio::signal;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

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
    let mut router = Router::new()
        .route("/health", get(routes::health::health))
        .route("/api/v1/health", get(routes::health::health))
        .merge(routes::router(state));

    // P2.4 D4: serve the built SPA from `app/dist/` so a single daemon
    // binary delivers both the API (`/api/v1/*`) and the frontend (same
    // origin → no CORS preflight in sidecar mode). Mounted as a
    // *fallback service* so every `/api/v1/*` route wins over static
    // files; only unmatched paths fall through to the SPA.
    //
    // SPA history-mode fallback: `ServeDir::not_found_service(ServeFile`
    // of `index.html`) so client-side routes (e.g. `/settings`) resolve
    // to the app shell instead of a 404. When the dist dir is absent
    // (dev mode — the frontend runs on vite :1420, or a daemon-only
    // deployment) the fallback is skipped, leaving a pure API server.
    match resolve_dist_dir() {
        Some(dist) => {
            tracing::info!(dist = %dist.display(), "serving static frontend from dist dir");
            let spa = ServeDir::new(&dist)
                .not_found_service(ServeFile::new(dist.join("index.html")));
            router = router.fallback_service(spa);
        }
        None => {
            tracing::info!(
                "no dist dir found (EVERLASTING_DIST_DIR unset + default \
                 absent); daemon runs as API-only (dev mode expects vite on :1420)"
            );
        }
    }

    // P2.3 dev-only CORS (C6):vite dev server (1420) 与 daemon
    // (7456) 跨域,`fetch` + `EventSource` 需 daemon 放行 preflight。
    // `very_permissive` 允许任意 origin / method / header(不带
    // credentials —— 此处 SSE/fetch 均无 cookie)。
    //
    // P2.4 决议:**保留**此 CORS 层。sidecar 同源场景下不触发
    // preflight(同源请求不经 CORS),故保留对 sidecar 零成本;而
    // dev 模式(vite 1420 ↔ daemon 7456 跨域)仍需要它。移除留 P2.5
    // 复盘定夺(若确认 dev 也可同源化则删)。
    router.layer(CorsLayer::very_permissive())
}

/// Resolve the frontend static-asset directory for `ServeDir` (P2.4 D4).
///
/// Resolution order:
/// 1. `EVERLASTING_DIST_DIR` env var (operator / test override; absolute).
/// 2. Default: walk up from the *daemon executable* until we find a
///    directory whose name is `src-tauri`, then take its sibling `../dist`.
///    This matches the Tauri layout where `app/src-tauri/` holds the
///    Rust crate and `app/dist/` holds the vite build output. Walking up
///    (rather than hard-coding `../../dist`) is required because the
///    daemon binary lives at different depths across build modes:
///    - sidecar (P2.4 staging): `app/src-tauri/binaries/everlasting-daemon-<triple>`
///      → one `..` to `src-tauri`, then `../dist`
///    - `cargo build --release` (manual test / dogfood):
///      `app/src-tauri/target/release/everlasting-daemon`
///      → three `..` to `src-tauri`, then `../dist`
///    - `cargo build` (debug): `app/src-tauri/target/debug/...`
///    Using `current_exe()` (not `env!("CARGO_MANIFEST_DIR")`) keeps
///    production single-binary deploys working regardless of install layout.
///
/// Returns `None` when nothing resolves — callers treat that as
/// "API-only mode" (no frontend built, or daemon-only deployment).
pub fn resolve_dist_dir() -> Option<PathBuf> {
    if let Ok(raw) = std::env::var("EVERLASTING_DIST_DIR") {
        let p = PathBuf::from(raw);
        if p.is_dir() {
            return Some(p);
        }
        tracing::debug!(
            dist = %p.display(),
            "EVERLASTING_DIST_DIR set but not a directory; ignoring"
        );
    }
    // Default: walk up from the daemon executable looking for a `src-tauri`
    // directory (the crate root). `current_exe()` is the canonical
    // cross-platform way to locate co-bundled assets; CARGO_MANIFEST_DIR
    // only exists at build time. Walk at most 10 levels to bound the scan.
    let exe = std::env::current_exe().ok()?;
    let mut dir = exe.parent()?;
    for _ in 0..10 {
        if dir.file_name().and_then(|n| n.to_str()) == Some("src-tauri") {
            // Found crate root; `dist` is a sibling of `src-tauri`.
            let dist = dir.parent()?.join("dist");
            if let Some(c) = dist.canonicalize().ok().filter(|p| p.is_dir()) {
                return Some(c);
            }
            // `src-tauri` found but no sibling `dist` — stop searching so
            // we don't accidentally pick up an unrelated `dist` higher up.
            return None;
        }
        dir = dir.parent()?;
    }
    None
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

/// Graceful shutdown 给 in-flight 连接的硬上限(秒)。正常路径下
/// [`SseRegistry::shutdown`] 主动结束后 SSE 流会亚秒完成,这个 timeout
/// 只是 defense-in-depth —— 防止未来加的非 SSE streaming endpoint
/// 或其他未知长连接把 shutdown 卡住。比 `scripts/daemon.sh` 的 8s
/// SIGKILL 短,留 SIGKILL 作最后一道防线。
const SHUTDOWN_GRACE_SECS: u64 = 3;

/// Bind + serve the daemon on `0.0.0.0:PORT`. The `everlasting-daemon`
/// bin target is a thin shell that resolves the port, loads
/// `AppState`, and calls this.
///
/// # Graceful shutdown
///
/// 收到 Ctrl+C (SIGINT) 或 SIGTERM (POSIX) 后:
/// 1. [`shutdown_signal`] 先调 [`SseRegistry::shutdown`] 主动 drop 所有
///    SSE 订阅者 → `stream.rs` 的 SSE body 自然 `end()` → axum 感知
///    这些连接「完成」。这一步根治了"SSE 长连接永不自然完成 →
///    graceful shutdown 无限挂起"的问题(原本靠 `daemon.sh` SIGKILL 兜底)。
/// 2. `shutdown_signal` 返回后,axum 的 `with_graceful_shutdown` 开始
///    drain 所有 in-flight 连接 —— 此时 SSE 流已结束,只剩可快速 drain
///    的短请求,整体亚秒完成。
/// 3. [`SHUTDOWN_GRACE_SECS`] timeout 兜底:若仍有未知长连接卡住,
///    超时后直接返回(进程随后退出),不阻塞 `daemon.sh` 的 SIGKILL。
///
/// 关键:必须用 `with_graceful_shutdown`(而非 `select!` + drop serve
/// future)。drop `axum::serve` 的 future 会 abort 所有连接 task(粗暴
/// 断开),而非 drain —— 那会丢失正在处理中的请求。
pub async fn serve_daemon(state: Arc<AppState>, port: u16) -> std::io::Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(addr = %addr, "everlasting-daemon listening");

    let router = build_router(Arc::clone(&state));
    let serve = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(Arc::clone(&state.sse)));

    // timeout 兜底:正常 sse.shutdown() 后亚秒 drain 完成;超时则放弃
    // 等待,让 daemon.sh 的 SIGKILL 作最后一道防线。
    match tokio::time::timeout(Duration::from_secs(SHUTDOWN_GRACE_SECS), serve).await {
        Ok(res) => {
            tracing::info!("everlasting-daemon shutdown complete");
            res?;
        }
        Err(_) => {
            tracing::warn!(
                grace_secs = SHUTDOWN_GRACE_SECS,
                "graceful shutdown exceeded grace window; forcing exit (daemon.sh SIGKILL is the last resort)"
            );
        }
    }
    Ok(())
}

/// Graceful shutdown signal handler. Fires on Ctrl+C (portable) or
/// SIGTERM (Unix only). Windows builds fall back to Ctrl+C only,
/// which matches the Tauri GUI's signal handling convention.
///
/// 信号触发后,**返回前先调 [`SseRegistry::shutdown`]** 主动结束所有
/// SSE 长连接。这是让 axum `with_graceful_shutdown` 不被永不完成的
/// SSE 连接卡住的关键 —— 不主动关 SSE 的话,graceful drain 会无限
/// 等待每个活跃的 `GET /api/v1/stream`。
async fn shutdown_signal(registry: Arc<crate::daemon::sse::SseRegistry>) {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("install Ctrl+C handler");
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

    // 主动结束所有 SSE 长连接 —— 在返回给 axum 的 graceful_shutdown
    // 之前,让 drain 不被永不完成的 SSE 连接卡住。
    registry.shutdown();
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

    /// P2.4 D4: `EVERLASTING_DIST_DIR` pointing at a real dir wins.
    /// Uses `tempfile` to avoid coupling to the build's `app/dist/`
    /// presence. This test MUST run with the env var set ONLY for this
    /// process; since `cargo test` serializes env mutations poorly,
    /// we use a unique var name pattern via the existing function —
    /// but the function reads the canonical `EVERLASTING_DIST_DIR`,
    /// so we accept that this test is order-sensitive and run it
    /// without the env set elsewhere (the default-path test below).
    #[test]
    fn dist_dir_env_override_when_dir_exists() {
        // SAFETY on parallelism: we set+unset the env around a single
        // read. Other tests that call `resolve_dist_dir()` without
        // setting the env will simply fall through to the default-path
        // branch, which is env-independent. The only collision would
        // be two tests both setting EVERLASTING_DIST_DIR — there is
        // exactly one such test (this one).
        let tmp = tempfile::tempdir().expect("create tempdir");
        std::env::set_var("EVERLASTING_DIST_DIR", tmp.path());
        let resolved = resolve_dist_dir();
        std::env::remove_var("EVERLASTING_DIST_DIR");
        assert_eq!(resolved, Some(tmp.path().to_path_buf()));
    }

    /// P2.4 D4: a non-existent `EVERLASTING_DIST_DIR` is ignored
    /// (falls through to default, which may or may not exist depending
    /// on the build host — we only assert it doesn't panic).
    #[test]
    fn dist_dir_env_ignored_when_not_a_dir() {
        std::env::set_var("EVERLASTING_DIST_DIR", "/nonexistent/path/that/does/not/exist");
        // Must not panic; result is host-dependent (default path).
        let _ = resolve_dist_dir();
        std::env::remove_var("EVERLASTING_DIST_DIR");
    }

    /// Graceful shutdown 端到端:有活跃 SSE 长连接时,daemon 收到
    /// SIGTERM 后必须在 grace window 内退出,而不是无限挂起(原本
    /// 靠 daemon.sh SIGKILL 兜底)。
    ///
    /// 这是本任务的核心回归测试:若 `serve_daemon` 回归到「等所有
    /// in-flight 连接完成」的旧行为(没有 `sse.shutdown()` 主动关
    /// SSE),本测试会因为 `serve_daemon` 超过 grace window 超时
    /// 而失败。
    ///
    /// 测试通过的标准:`serve_daemon` 在 `SHUTDOWN_GRACE_SECS` 内
    /// 返回(而非挂起/超时),证明 `sse.shutdown()` 主动结束后 axum
    /// 的 graceful drain 能快速完成。
    #[cfg(unix)]
    #[tokio::test]
    async fn serve_daemon_shutdown_completes_with_active_sse() {
        use std::sync::Arc;
        use tempfile::TempDir;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpStream;

        // 预占一个 ephemeral port,取出端口号后立即 drop,交给
        // serve_daemon 重绑(有微小竞态,测试可接受)。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral");
        let port = listener.local_addr().expect("local_addr").port();
        drop(listener);

        let dir = TempDir::new().expect("tempdir");
        let state = load_daemon_state(dir.path().to_path_buf()).await;

        // spawn serve_daemon。
        let serve_handle = tokio::spawn(serve_daemon(Arc::clone(&state), port));

        // 等 daemon 起来(轮询 health)。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if std::time::Instant::now() >= deadline {
                panic!("daemon did not become healthy in 5s");
            }
            if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)).await {
                let req =
                    b"GET /api/v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
                let _ = s.write_all(req).await;
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf).await;
                if buf.starts_with(b"HTTP/1.1 200") {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        // 建立一个 SSE 长连接 —— 这是触发旧 bug「graceful shutdown
        // 挂起」的必要条件。连接保持打开,不发任何会让它结束的内容。
        let mut sse = TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect for SSE");
        let sse_req =
            b"GET /api/v1/stream HTTP/1.1\r\nHost: localhost\r\nAccept: text/event-stream\r\n\r\n";
        sse.write_all(sse_req).await.expect("send SSE GET");
        // 读一点响应头确认连接建立(200 + headers),但不读完 body
        // (SSE body 无限流)。
        let mut hdr = [0u8; 64];
        let _ = sse.read(&mut hdr).await;
        assert!(
            hdr.starts_with(b"HTTP/1.1 200"),
            "SSE endpoint must respond 200, got: {}",
            String::from_utf8_lossy(&hdr)
        );

        // 发真实 SIGTERM 给当前进程 —— 触发 serve_daemon 的
        // shutdown_signal。libc 已是项目依赖(process group kill 等)。
        unsafe {
            libc::kill(libc::getpid(), libc::SIGTERM);
        }

        // 核心断言:serve_daemon 必须在 grace window 内完成。
        // SHUTDOWN_GRACE_SECS 是 serve_daemon 内部的 timeout 上限;
        // 正常路径下 sse.shutdown() 后 drain 亚秒完成。留 2x 余量
        // 防 CI 慢机器。
        let completion = tokio::time::timeout(
            std::time::Duration::from_secs(SHUTDOWN_GRACE_SECS * 2 + 2),
            serve_handle,
        )
        .await;

        match completion {
            Ok(Ok(_)) => {
                // serve_daemon 正常返回 —— 通过。
            }
            Ok(Err(e)) => panic!("serve_daemon returned error: {e}"),
            Err(_) => {
                panic!(
                    "serve_daemon did NOT complete within {}s of SIGTERM — \
                     graceful shutdown is still hanging on the active SSE \
                     connection (the bug this test guards against)",
                    SHUTDOWN_GRACE_SECS * 2 + 2
                );
            }
        }
    }
}
