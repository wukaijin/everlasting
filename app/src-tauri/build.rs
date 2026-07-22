//! Build script.
//!
//! 1. `tauri_build::build()` — the standard Tauri codegen (capabilities,
//!    context, etc.).
//!
//! 2. P2.4 D1.3 (2026-07-22, task `07-20-remote-access-daemon-split`):
//!    copy the freshly built `everlasting-daemon` binary into
//!    `src-tauri/binaries/everlasting-daemon-<target-triple>` so Tauri's
//!    `externalBin` / `Command::new_sidecar("everlasting-daemon")`
//!    resolves it. Tauri looks up sidecars by
//!    `binaries/<name>-<target-triple>` (the `-<triple>` suffix is how
//!    Tauri picks the right binary per host platform at bundle time).
//!
//!    This runs on EVERY `cargo build` (both `everlasting` lib/bin and
//!    `everlasting-daemon` bin targets), so we only copy when the
//!    daemon artifact is newer than the staged copy (incremental —
//!    avoids re-copying 100+MB on every GUI rebuild when the daemon
//!    hasn't changed).
//!
//!    `cargo:rerun-if-changed` is NOT emitted for the daemon binary
//!    (we can't point it at `target/` reliably across profiles); the
//!    mtime check + the fact that this script runs on every build is
//!    sufficient. A stale daemon binary simply won't be copied until
//!    the daemon is rebuilt.

use std::path::PathBuf;

fn main() {
    // Stage the daemon sidecar FIRST — `tauri_build::build()` (below)
    // validates that every `bundle.externalBin` entry exists on disk
    // under `binaries/<name>-<target-triple>`, and fails the build if
    // it's missing. So we must populate the staged copy before that
    // check runs. A missing daemon artifact (first GUI-only build) is
    // handled as a no-op here; if tauri_build then fails, the error is
    // accurate ("build the daemon first: cargo build --bin
    // everlasting-daemon").
    if let Err(e) = stage_daemon_sidecar() {
        println!("cargo:warning=P2.4 sidecar stage skipped: {e}");
    }

    tauri_build::build();
}

/// Copy `target/<profile>/everlasting-daemon` →
/// `src-tauri/binaries/everlasting-daemon-<target-triple>`.
///
/// Returns `Ok(())` on success or if the daemon artifact doesn't exist
/// yet (treated as a no-op, not an error, so a GUI-only first build
/// works). Returns `Err` only on unexpected I/O failures during copy.
fn stage_daemon_sidecar() -> std::io::Result<()> {
    let target_triple = std::env::var("TARGET").unwrap_or_default();
    if target_triple.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "TARGET env var not set",
        ));
    }

    // Resolve the manifest dir (where this build.rs runs) and walk up
    // to the workspace target dir. `CARGO_MANIFEST_DIR` is the
    // `src-tauri/` dir; the cargo target dir is exposed via
    // `OUT_DIR`'s ancestor or `CARGO_TARGET_DIR`. We use
    // `CARGO_TARGET_DIR` if set, else fall back to
    // `<manifest_dir>/target`.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join("target"));

    // Profile: `debug` or `release`. Derived from `OUT_DIR`'s path
    // segment (the 3rd-from-last component is the profile name in
    // `<target>/<profile>/<crate-hash>/out/`). Fall back to `debug`.
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());

    // Windows sidecar binaries need the `.exe` suffix; Tauri's
    // `externalBin` lookup adds it automatically, but we must add it
    // to the source artifact path when reading.
    let is_windows = target_triple.contains("windows");
    let exe_name = if is_windows { "everlasting-daemon.exe" } else { "everlasting-daemon" };
    let src = target_dir.join(&profile).join(exe_name);

    if !src.exists() {
        // Daemon not built yet — common on a fresh GUI-only checkout.
        // No-op (warned by caller).
        return Ok(());
    }

    let staged_name = format!("everlasting-daemon-{}", target_triple);
    let binaries_dir = manifest_dir.join("binaries");
    std::fs::create_dir_all(&binaries_dir)?;
    let dst = binaries_dir.join(&staged_name);

    // Incremental: skip the copy when dst exists and is at least as
    // new as src (100MB+ copy avoidance on every GUI rebuild).
    if let (Ok(src_meta), Ok(dst_meta)) = (std::fs::metadata(&src), std::fs::metadata(&dst)) {
        if let (Ok(src_mtime), Ok(dst_mtime)) =
            (src_meta.modified(), dst_meta.modified())
        {
            if dst_mtime >= src_mtime {
                return Ok(());
            }
        }
    }

    std::fs::copy(&src, &dst)?;
    // On Unix, ensure the staged sidecar is executable (fs::copy
    // preserves source perms, but be defensive — a fresh `binaries/`
    // dir copy from a non-exec source shouldn't strip the bit).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&dst, perms)?;
    }
    Ok(())
}
