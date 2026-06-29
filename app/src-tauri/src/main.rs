// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tracing_subscriber::{fmt, EnvFilter};

fn main() {
    init_tracing();
    // WSL/WSLg does not expose vgem/DRM devices, so libEGL's Mesa
    // loader prints `libEGL warning: MESA-LOADER: failed to open vgem`
    // on every startup (the loader unconditionally probes
    // `vgem_dri.so` before falling back to other drivers — known
    // Mesa behavior, see https://gitlab.freedesktop.org/mesa/mesa).
    //
    // Set these BEFORE Tauri's GTK/WebKit init. Stacked belt-and-
    // suspenders because Mesa honors some vars only on certain
    // entry points:
    //   - EGL_PLATFORM=surfaceless     → EGL entry picks no-display
    //   - WEBKIT_DISABLE_DMABUF_RENDERER=1 → WebKit skips DMA-BUF
    //   - MESA_LOADER_DRIVER_OVERRIDE=llvmpipe → libGL skips hw probe
    //   - LIBGL_ALWAYS_SOFTWARE=1      → libGL forces swrast/llvmpipe
    //   - MESA_DEBUG=quiet             → suppress Mesa debug/loader logs
    //
    // Pure stderr-noise suppression — no functional impact (Tauri
    // already runs on the software path on WSL).
    std::env::set_var("EGL_PLATFORM", "surfaceless");
    std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    std::env::set_var("MESA_LOADER_DRIVER_OVERRIDE", "llvmpipe");
    std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
    std::env::set_var("MESA_DEBUG", "quiet");
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