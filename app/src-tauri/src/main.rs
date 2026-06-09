// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tracing_subscriber::{fmt, EnvFilter};

fn main() {
    init_tracing();
    everlasting_lib::run()
}

/// Initialize the `tracing` subscriber. Called from `main` before
/// the Tauri app starts so any error during `tauri::Builder`
/// setup is captured.
///
/// Grill decision #4 (locked): extracted here from `lib.rs::run`
/// so `lib.rs::run` is a thin Tauri bootstrap and `init_tracing`
/// lives next to the platform entry point it actually depends on
/// (Windows console subsystem on/off, env-filter defaults).
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).init();
}