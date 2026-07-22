//! P2.4 D2/D5 (2026-07-22, task `07-20-remote-access-daemon-split`):
//! spawn + lifetime-manage the `everlasting-daemon` sidecar from the
//! Tauri GUI process.
//!
//! ## Two GUI modes (D5 thin-client decision)
//!
//! The GUI runs in one of two modes, decided at setup time by
//! [`GuiMode::resolve`]:
//!
//! - **`Thin`** (the P2.4 default): the GUI does NOT load `AppState`,
//!   does NOT open a `SqlitePool`, and does NOT spawn the sweep /
//!   hygiene background tasks. It only spawns the daemon sidecar and
//!   talks to it over HTTP/SSE via `httpTransport` (same-origin — the
//!   daemon serves the SPA via `ServeDir`). The 79
//!   `invoke_handler` commands are still REGISTERED (so the
//!   capability schema / registration compiles unchanged), but they
//!   are never invoked by the frontend in this mode, so the absent
//!   `Arc<AppState>` Tauri state never triggers a panic. The
//!   `RunEvent::Exit` hook kills the sidecar.
//!
//! - **`Full`** (`?transport=tauri` query, or
//!   `EVERLASTING_GUI_FULL_STATE=1`): the legacy pre-P2.4 behavior —
//!   `AppState::load` + `app.manage` + sweep + hygiene. This is the
//!   emergency escape hatch when the daemon is broken (the webview
//!   can be relaunched with `?transport=tauri` to get a fully
//!   functional GUI backed by the in-process state). In Full mode no
//!   sidecar is spawned (the daemon would sit idle — the frontend
//!   talks Tauri IPC, not HTTP).
//!
//! ## Sidecar spawn contract
//!
//! `app.shell().sidecar("everlasting-daemon")` resolves the staged
//! binary at `src-tauri/binaries/everlasting-daemon-<target-triple>`
//! (copied there by `build.rs` from `target/<profile>/everlasting-daemon`).
//! The `--port` + `--data-dir` args MUST match the
//! `capabilities/default.json` `shell:allow-execute` scoped args
//! exactly (static segments verbatim, dynamic `--data-dir` value under
//! a `.+` regex validator).
//!
//! ## Kill semantics
//!
//! `ChildExt::kill()` (provided by `tauri-plugin-shell`) is the
//! cross-platform terminate: `TerminateProcess` on Windows, `kill`
//! (`SIGTERM` then `SIGKILL` fallback per the plugin's impl) on POSIX.
//! The daemon's own `shutdown_signal()` handler catches `SIGTERM` and
//! drains in-flight requests gracefully; if the sidecar is wedged the
//! plugin escalates to `SIGKILL`. Called from the `RunEvent::Exit`
//! hook so closing the GUI window reaps the daemon — no orphan
//! `everlasting-daemon` processes.

use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Manager};
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;

/// The canonical port the GUI expects the daemon on (Q1 default).
/// Kept in sync with `daemon::server::DEFAULT_DAEMON_PORT` by a
/// constant re-export there; duplicated here to avoid a cross-module
/// import just for a u16 literal.
pub const SIDECAR_PORT: u16 = 7456;

/// The sidecar identifier — matches `bundle.externalBin` in
/// `tauri.conf.json` (`"binaries/everlasting-daemon"`) and the `name`
/// field in the `shell:allow-execute` capability.
pub const SIDECAR_NAME: &str = "everlasting-daemon";

/// Held in Tauri state under [`SidecarHandle`] so the `RunEvent::Exit`
/// hook can reach the child without re-resolving it. `Mutex` (not a
/// bare field) because `kill()` takes `&mut` and the exit hook + any
/// future supervision task could race.
pub struct SidecarHandle {
    child: Mutex<Option<CommandChild>>,
}

impl SidecarHandle {
    /// Kill the sidecar if it's still alive. Idempotent — calling
    /// twice (e.g. `ExitRequested` + `Exit`) is safe because the
    /// inner is `take`n on first kill. Errors are logged at `warn!`
    /// (non-fatal — the GUI is exiting anyway and the OS will reap
    /// any stubborn descendant via the process group).
    pub fn kill(&self) {
        if let Some(child) = self.child.lock().ok().and_then(|mut g| g.take()) {
            if let Err(e) = child.kill() {
                tracing::warn!(error = %e, "sidecar kill failed (non-fatal on exit)");
            } else {
                tracing::info!("everlasting-daemon sidecar killed");
            }
        }
    }
}

/// Which GUI mode to run in (see module docs). Resolved once at setup
/// from the webview's URL query (`?transport=tauri`) or an env override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuiMode {
    /// P2.4 default: spawn sidecar, no `AppState`, no DB pool. Frontend
    /// uses `httpTransport`.
    Thin,
    /// Legacy: full in-process `AppState` + DB pool, no sidecar.
    /// Frontend uses `tauriTransport`. Activated via `?transport=tauri`
    /// or `EVERLASTING_GUI_FULL_STATE=1`.
    Full,
}

impl GuiMode {
    /// Resolve the mode. Precedence (highest first):
    /// 1. `EVERLASTING_GUI_FULL_STATE=1` env (operator / CI override).
    /// 2. The main webview window's URL `?transport=tauri` query.
    /// 3. Default `Thin`.
    ///
    /// Reads the webview URL (not the dev server's) because the
    /// `?transport=` query is what the operator / developer passes to
    /// force the escape mode. In `tauri dev` the webview loads from
    /// `devUrl` (vite :1420) so the query is honored; in a packaged
    /// build it loads from `tauri://localhost` and the query still
    /// works via `?transport=tauri` in the window URL.
    pub fn resolve(app: &AppHandle) -> Self {
        if matches!(
            std::env::var("EVERLASTING_GUI_FULL_STATE").as_deref(),
            Ok("1")
        ) {
            tracing::info!("GUI mode: Full (EVERLASTING_GUI_FULL_STATE=1)");
            return Self::Full;
        }
        if let Some(window) = app.get_webview_window("main") {
            if let Ok(url) = window.url() {
                let query_transport = url
                    .query_pairs()
                    .find(|(k, _)| k == "transport")
                    .map(|(_, v)| v.to_string());
                if query_transport.as_deref() == Some("tauri") {
                    tracing::info!("GUI mode: Full (?transport=tauri)");
                    return Self::Full;
                }
            }
        }
        tracing::info!("GUI mode: Thin (default; sidecar + httpTransport)");
        Self::Thin
    }
}

/// Spawn the `everlasting-daemon` sidecar and register it in Tauri
/// state as [`SidecarHandle`]. The returned `(receiver, _)` event
/// stream is drained by a background task that logs the daemon's
/// stdout/stderr to the GUI's tracing subscriber (so daemon logs are
/// visible in the GUI's console / `RUST_LOG`).
///
/// `data_dir` is passed as `--data-dir` so the daemon opens the SAME
/// SQLite file the GUI would have opened in Full mode (P2.1 path
/// consistency). The arg MUST appear in `capabilities/default.json`'s
/// scoped `shell:allow-execute` validator (`.+`) or the spawn is
/// rejected by the capability gate.
///
/// Failures here are fatal (Q1 fail-loud): a missing sidecar binary
/// or a rejected spawn means the GUI cannot reach the agent core, so
/// we surface the error rather than silently degrading.
pub fn spawn_and_manage(app: &AppHandle, data_dir: &PathBuf) {
    let data_dir_str = data_dir.to_string_lossy().into_owned();
    let sidecar_cmd = app
        .shell()
        .sidecar(SIDECAR_NAME)
        .expect("sidecar 'everlasting-daemon' not resolved (check bundle.externalBin + binaries/ staging)")
        .args([
            "--port",
            &SIDECAR_PORT.to_string(),
            "--data-dir",
            // Must match the capability's `.+` validator positionally.
            &data_dir_str,
        ]);

    let (mut rx, child) = sidecar_cmd
        .spawn()
        .expect("failed to spawn everlasting-daemon sidecar");

    app.manage(SidecarHandle {
        child: Mutex::new(Some(child)),
    });

    // Drain the sidecar's stdout/stderr into the GUI's tracing
    // pipeline so daemon logs are co-located with GUI logs. This is
    // observability-only — the spawn already succeeded.
    tauri::async_runtime::spawn(async move {
        use tauri_plugin_shell::process::CommandEvent;
        while let Some(event) = rx.recv().await {
            match event {
                CommandEvent::Stdout(bytes) => {
                    tracing::info!(
                        target: "everlasting-daemon",
                        line = %String::from_utf8_lossy(&bytes).trim_end(),
                        "daemon stdout"
                    );
                }
                CommandEvent::Stderr(bytes) => {
                    tracing::warn!(
                        target: "everlasting-daemon",
                        line = %String::from_utf8_lossy(&bytes).trim_end(),
                        "daemon stderr"
                    );
                }
                CommandEvent::Terminated(payload) => {
                    tracing::warn!(?payload, "everlasting-daemon sidecar terminated");
                    break;
                }
                CommandEvent::Error(err) => {
                    tracing::warn!(error = %err, "everlasting-daemon sidecar event error");
                }
                _ => {}
            }
        }
        tracing::info!("everlasting-daemon sidecar event stream closed");
    });
}

/// Kill the managed sidecar (if Thin mode spawned one). Called from
/// the `RunEvent::Exit` hook. No-op in Full mode (no sidecar was
/// managed).
pub fn kill_managed(app: &AppHandle) {
    if let Some(handle) = app.try_state::<SidecarHandle>() {
        handle.kill();
    }
}
