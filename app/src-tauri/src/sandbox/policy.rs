//! `SandboxSpec` construction from server-side session state.
//!
//! **Source iron rule (CVE-2025-59532)**: every path in the spec
//! comes from server-side state — the session's validated worktree,
//! fixed constants (`/tmp`, `/dev`), the spill directory derived
//! from `data_dir + session_id`, or app_config. Nothing the LLM
//! passes in `tool_input` (command text, `working_directory`, …) has
//! any influence over this structure; there is no API surface for it.

use std::path::PathBuf;

use sqlx::SqlitePool;

use super::SandboxSpec;
use crate::tools::ToolContext;

/// app_config key for the kill switch (R6). Stored as `"true"` /
/// `"false"` literals by `set_app_config_flag`; read fail-open:
/// anything but the literal `"false"` (including a missing row)
/// means enabled (D1 default-on).
const CONFIG_ENABLED_KEY: &str = "sandbox_enabled";
/// app_config key for extra writable roots (R7). JSON array of
/// strings (tilde allowed); empty/missing → `~/.cargo` only.
const CONFIG_EXTRA_KEY: &str = "sandbox_extra_writable";

/// Kill-switch read (fail-open, D1): only the literal `"false"`
/// disables. Mirrors the reading convention of
/// `turn_complete_notify_enabled` / `scheduled_tasks_enabled`.
pub(crate) async fn sandbox_enabled(db: &SqlitePool) -> bool {
    match crate::db::config::get_config_value(db, CONFIG_ENABLED_KEY).await {
        Ok(Some(v)) => v != "false",
        _ => true,
    }
}

/// Effective extra-writable roots: `~/.cargo` (default allowlist
/// entry, R7 — cargo's first build step writes there and would
/// otherwise false-kill) + the JSON array stored in app_config.
/// Tilde entries are expanded via `boundary::resolve_path` (same
/// helper the permission layer uses for `~/...` patterns); missing
/// or malformed config values degrade to the default list.
pub(crate) async fn read_extra_writable(db: &SqlitePool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = dirs::home_dir() {
        out.push(home.join(".cargo"));
    }
    let raw = match crate::db::config::get_config_value(db, CONFIG_EXTRA_KEY).await {
        Ok(Some(v)) => v,
        _ => return out,
    };
    if let Ok(list) = serde_json::from_str::<Vec<String>>(&raw) {
        for entry in list {
            if entry.is_empty() {
                continue;
            }
            let expanded =
                crate::projects::boundary::resolve_path(&entry, PathBuf::from("/").as_path());
            if !out.contains(&expanded) {
                out.push(expanded);
            }
        }
    } else {
        tracing::warn!(
            raw_prefix = %raw.chars().take(64).collect::<String>(),
            "sandbox: malformed sandbox_extra_writable config, using defaults"
        );
    }
    out
}

/// Toolchain dirs probed for the exec face (design §2.1). The
/// existence probe is cheap (two stats) and only avoids pointless
/// fd opens at rule time; `~/.cargo/bin` is usually already covered
/// via `~/.cargo` as a writable root.
fn toolchain_exec_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = dirs::home_dir() {
        let cargo_bin = home.join(".cargo").join("bin");
        if cargo_bin.exists() {
            out.push(cargo_bin);
        }
    }
    let brew = PathBuf::from("/home/linuxbrew/.linuxbrew");
    if brew.exists() {
        out.push(brew);
    }
    out
}

/// PATH resolution happens in the parent (design §2.1): the product
/// is a plain directory list; the sandboxed child keeps its PATH env
/// for lookup but every exec is gated by the face below. Symlink
/// resolution needs no extra work — `open(O_PATH)` follows symlinks,
/// so the opened fd is the real directory.
///
/// WSL interop caveat (spike landlock 篇 §2, "显式不含 /mnt/c"):
/// WSL appends the Windows drive mounts (`/mnt/c/...`) to PATH, so
/// PATH-derived entries under `/mnt/` are dropped — otherwise the
/// exec face would silently reopen the interop escape the whole
/// deny-face exists to close. The ONE intentional exception is the
/// session worktree itself: a project checked out under `/mnt/c`
/// still gets exec via the writable-root clause below (user's own
/// code, not interop binaries).
fn path_exec_roots() -> Vec<PathBuf> {
    std::env::var("PATH")
        .map(|p| {
            p.split(':')
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .filter(|d| !d.starts_with("/mnt/"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Build the spec for one session's command.
///
/// - Writable roots: worktree + `/tmp` + spill dir + extras.
/// - Exec face: PATH dirs + `/dev` + `/tmp` + writable roots +
///   toolchain dirs. Deliberately absent: `/init`, `/mnt/c`
///   (WSL interop containment — EXECUTE deny face is the whole
///   mechanism, spike landlock 篇 §3).
///
/// The spill directory is created best-effort here so its rule can
/// open an fd; a failure is non-fatal (the rule is skipped — the
/// child never legitimately writes there anyway, spills are
/// parent-side).
pub fn build_spec(
    ctx: &ToolContext,
    session_id: Option<&str>,
    extra_writable: Vec<PathBuf>,
) -> SandboxSpec {
    let mut writable_roots: Vec<PathBuf> = vec![ctx.worktree_path.clone(), PathBuf::from("/tmp")];
    if let Some(sid) = session_id {
        let spill = crate::tools::tool_output::session_outputs_dir(&ctx.data_dir, sid);
        if let Err(e) = std::fs::create_dir_all(&spill) {
            tracing::debug!(
                error = %e,
                dir = %spill.display(),
                "sandbox: spill dir pre-creation failed; its rule will be skipped"
            );
        }
        writable_roots.push(spill);
    }
    for extra in &extra_writable {
        if !writable_roots.contains(extra) {
            writable_roots.push(extra.clone());
        }
    }

    let mut exec_allow_roots = path_exec_roots();
    // ELF interpreter roots (design gap found in implementation, spike
    // recipe had them hardcoded): a dynamically-linked binary's
    // interpreter (`/lib64/ld-linux-x86-64.so.2`, musl
    // `/lib/ld-musl-*.so.1`) is opened BY THE KERNEL during execve
    // and needs EXECUTE too. Normal user PATHs never include /lib*,
    // so PATH resolution alone would EACCES every dynamic binary.
    exec_allow_roots.push(PathBuf::from("/lib"));
    exec_allow_roots.push(PathBuf::from("/lib64"));
    exec_allow_roots.push(PathBuf::from("/usr/lib"));
    exec_allow_roots.push(PathBuf::from("/dev"));
    exec_allow_roots.push(PathBuf::from("/tmp"));
    exec_allow_roots.extend(writable_roots.iter().cloned());
    exec_allow_roots.extend(toolchain_exec_roots());
    // Textual dedup, order-stable (first occurrence wins).
    let mut seen = std::collections::HashSet::new();
    exec_allow_roots.retain(|p| seen.insert(p.clone()));

    SandboxSpec {
        writable_roots,
        exec_allow_roots,
        extra_writable,
    }
}
