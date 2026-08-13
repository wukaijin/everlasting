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
//!    When the daemon artifact doesn't exist yet (fresh checkout), we
//!    write a 0-byte placeholder instead of no-op-ing. This is mandatory:
//!    `tauri_build::build()` validates `bundle.externalBin` entries exist
//!    on disk *during build.rs*, before any binary is compiled — so a
//!    missing `binaries/everlasting-daemon-<triple>` deadlocks the very
//!    first `cargo build --bin everlasting-daemon` that would produce it.
//!    The placeholder breaks the deadlock; the next build.rs run copies
//!    the real artifact over it (a 0-byte dst is always overwritten).
//!
//!    `cargo:rerun-if-changed` is NOT emitted for the daemon binary
//!    (we can't point it at `target/` reliably across profiles); the
//!    mtime check + the fact that this script runs on every build is
//!    sufficient. A stale daemon binary simply won't be copied until
//!    the daemon is rebuilt.
//!
//! 3. `EVERLASTING_APP_IDENTIFIER` env injection (P2.2 path-consistency
//!    fix): reads `identifier` from `tauri.conf.json` and emits it as a
//!    compile-time env so the `everlasting-daemon` bin can resolve its
//!    data dir to the SAME path Tauri's `app_data_dir()` would (which
//!    is `dirs::data_dir().join(config.identifier)`). Without this the
//!    daemon fell back to `join("everlasting")` and opened a different
//!    SQLite file than the GUI — see `bin/everlasting-daemon.rs`
//!    `resolve_data_dir()`. tauri-build does NOT expose the identifier
//!    as a generic compile-time env (only as an Android package-name
//!    derivative), so we read it ourselves here.

use std::path::{Path, PathBuf};

fn main() {
    // Stage the daemon sidecar FIRST — `tauri_build::build()` (below)
    // validates that every `bundle.externalBin` entry exists on disk
    // under `binaries/<name>-<target-triple>`, and fails the build if
    // it's missing. So we must populate the staged copy before that
    // check runs. When the daemon artifact isn't built yet, we write a
    // 0-byte placeholder so this validation passes (see
    // `stage_daemon_sidecar` for why a no-op would deadlock the first
    // build).
    if let Err(e) = stage_daemon_sidecar() {
        println!("cargo:warning=P2.4 sidecar stage skipped: {e}");
    }

    // Inject the bundle identifier so the daemon bin's `resolve_data_dir()`
    // joins the SAME subdirectory Tauri's `app_data_dir()` uses. Placed
    // before `tauri_build::build()` so a config read failure surfaces
    // before any heavier codegen runs.
    emit_app_identifier();

    tauri_build::build();
}

/// Read `identifier` from `tauri.conf.json` (in `CARGO_MANIFEST_DIR`)
/// and emit it as `EVERLASTING_APP_IDENTIFIER` compile-time env.
///
/// This lets the daemon bin compute `<platform data dir>/<identifier>`
/// identically to Tauri's `app.path().app_data_dir()` (which is
/// `dirs::data_dir().join(config.identifier)` per tauri-2 path module),
/// so a standalone `everlasting-daemon` run opens the SAME SQLite file
/// the GUI would — the P2.1 path-consistency invariant.
///
/// `TAURI_CONFIG` env override (white-label builds) is intentionally
/// NOT handled: production builds don't override the identifier, and
/// the rare white-label case would require JSON merge logic mirroring
/// tauri-build internals — out of scope. The base file is authoritative.
fn emit_app_identifier() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let conf_path = manifest_dir.join("tauri.conf.json");
    let conf_str = match std::fs::read_to_string(&conf_path) {
        Ok(s) => s,
        Err(e) => {
            println!(
                "cargo:warning=EVERLASTING_APP_IDENTIFIER: failed to read {}: {} — \
                 daemon bin will fail to compile env!(EVERLASTING_APP_IDENTIFIER)",
                conf_path.display(),
                e
            );
            return;
        }
    };
    let conf: serde_json::Value = match serde_json::from_str(&conf_str) {
        Ok(v) => v,
        Err(e) => {
            println!(
                "cargo:warning=EVERLASTING_APP_IDENTIFIER: failed to parse {}: {} — \
                 daemon bin will fail to compile env!(EVERLASTING_APP_IDENTIFIER)",
                conf_path.display(),
                e
            );
            return;
        }
    };
    let identifier = match conf.get("identifier").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            println!(
                "cargo:warning=EVERLASTING_APP_IDENTIFIER: no `identifier` field in {} — \
                 daemon bin will fail to compile env!(EVERLASTING_APP_IDENTIFIER)",
                conf_path.display()
            );
            return;
        }
    };
    // Tell cargo to re-run build.rs when the config changes (defensive —
    // tauri_build::build() also emits this, but being explicit keeps
    // this function self-contained if called independently).
    println!("cargo:rerun-if-changed={}", conf_path.display());
    println!("cargo:rustc-env=EVERLASTING_APP_IDENTIFIER={}", identifier);
}

/// Walk up from `dir` to the Cargo workspace root (the nearest ancestor
/// whose `Cargo.toml` declares a `[workspace]` section) and return its
/// `target/` directory. Returns `None` when no workspace root is found
/// (standalone crate outside a workspace) — the caller then falls back
/// to the pre-workspace `<manifest_dir>/target` layout.
///
/// Added 2026-08-11 (task 08-11-remote-daemon-core workspace flip):
/// cargo stopped writing member artifacts next to the member manifest
/// (`app/src-tauri/target`) and now uses the workspace root `target/`.
/// The previous `<manifest_dir>/target` fallback silently missed the
/// daemon binary → staged a 0-byte sidecar on clean checkouts.
fn workspace_target_dir(mut dir: &Path) -> Option<PathBuf> {
    loop {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.is_file() {
            let content = std::fs::read_to_string(&cargo_toml).ok()?;
            if content.contains("[workspace]") {
                return Some(dir.join("target"));
            }
        }
        dir = dir.parent()?;
    }
}

/// Copy `target/<profile>/everlasting-daemon` →
/// `src-tauri/binaries/everlasting-daemon-<target-triple>`.
///
/// Returns `Ok(())` on success, when the daemon artifact doesn't exist
/// yet (a 0-byte placeholder is written so `externalBin` validation can
/// pass — see the deadlock note above), or when the staged copy is
/// already a fresh real binary. Returns `Err` only on unexpected I/O
/// failures during copy.
fn stage_daemon_sidecar() -> std::io::Result<()> {
    let target_triple = std::env::var("TARGET").unwrap_or_default();
    if target_triple.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "TARGET env var not set",
        ));
    }

    // Resolve the cargo target dir. `CARGO_MANIFEST_DIR` is the
    // `src-tauri/` dir; the cargo target dir is exposed via
    // `CARGO_TARGET_DIR`, else (2026-08-11 workspace flip) it lives at
    // the workspace root `<workspace>/target` — **not** `<manifest_dir>/target`.
    // `workspace_target_dir` walks up to the nearest ancestor whose
    // Cargo.toml declares `[workspace]`; a standalone (non-workspace)
    // build falls back to the old `<manifest_dir>/target` layout.
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let target_dir = match std::env::var("CARGO_TARGET_DIR") {
        Ok(v) => PathBuf::from(v),
        Err(_) => {
            workspace_target_dir(&manifest_dir).unwrap_or_else(|| manifest_dir.join("target"))
        }
    };

    // Profile: `debug` or `release`. Derived from `OUT_DIR`'s path
    // segment (the 3rd-from-last component is the profile name in
    // `<target>/<profile>/<crate-hash>/out/`). Fall back to `debug`.
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());

    // Windows sidecar binaries need the `.exe` suffix; Tauri's
    // `externalBin` lookup adds it automatically, but we must add it
    // to the source artifact path when reading.
    let is_windows = target_triple.contains("windows");
    let exe_name = if is_windows {
        "everlasting-daemon.exe"
    } else {
        "everlasting-daemon"
    };
    let src = target_dir.join(&profile).join(exe_name);

    let staged_name = format!("everlasting-daemon-{}", target_triple);
    let binaries_dir = manifest_dir.join("binaries");
    std::fs::create_dir_all(&binaries_dir)?;
    let dst = binaries_dir.join(&staged_name);

    // Daemon source artifact not built yet. Two sub-cases:
    //
    //   a) dst already holds a real binary (built earlier, e.g. a
    //      warm rust-cache) → leave it, nothing to do.
    //   b) dst is missing or is a 0-byte placeholder → write a 0-byte
    //      placeholder so tauri_build's externalBin existence check
    //      passes and the build can proceed. WITHOUT this, a fresh
    //      checkout is a hard deadlock: `externalBin` is validated in
    //      build.rs *before* any binary is compiled, so even
    //      `cargo build --bin everlasting-daemon` (which produces the
    //      very artifact we'd copy) can't start. The placeholder breaks
    //      the deadlock; the next build.rs run after the daemon is
    //      compiled copies the real binary over it (see below — the
    //      0-byte size is what lets the real artifact win despite the
    //      placeholder's fresh mtime).
    if !src.exists() {
        let needs_placeholder = match std::fs::metadata(&dst) {
            Ok(m) => m.len() == 0, // existing placeholder, keep it idempotent
            Err(_) => true,        // nothing staged yet
        };
        if needs_placeholder {
            std::fs::write(&dst, [])?;
        }
        return Ok(());
    }

    // src exists. Decide whether to (over)write dst with the real binary.
    // Skip only when dst already holds a real (non-placeholder) binary that
    // is at least as new as src — the 100MB+ copy avoidance that matters for
    // fast incremental GUI rebuilds. A 0-byte dst is always a placeholder
    // (see above) and must be overwritten unconditionally; relying on mtime
    // alone would let a freshly-touched placeholder shadow a real artifact.
    let dst_is_real_and_fresh = match std::fs::metadata(&dst) {
        Ok(dst_meta) if dst_meta.len() > 0 => {
            match (
                std::fs::metadata(&src).and_then(|m| m.modified()),
                dst_meta.modified(),
            ) {
                (Ok(src_mtime), Ok(dst_mtime)) => dst_mtime >= src_mtime,
                _ => false,
            }
        }
        _ => false, // missing or 0-byte placeholder → must copy
    };
    if dst_is_real_and_fresh {
        return Ok(());
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
