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

use super::{Face, SandboxSpec};
use crate::tools::ToolContext;

/// Per-project sandbox policy tier (P3c, design §2). Stored in
/// `projects.sandbox_policy` (TEXT + CHECK constraint, added by the
/// schema migration); the default is `ReadWrite` — the P3c behavior
/// change is intentional (PRD D2: SideEffect commands become
/// sandboxed-with-writable-worktree instead of silently unbounded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectSandboxPolicy {
    /// No sandbox for this project — classic Tier 4 approval path.
    Off,
    /// Every command sandboxes; worktree writable (default).
    ReadWrite,
    /// Every command sandboxes; worktree read-only (hard isolation
    /// for auditing third-party repos).
    ReadOnly,
}

// ---------------------------------------------------------------------------
// NetPolicy (09-21-sandbox-net-bindonly, R1/R4) — the orthogonal network
// dimension of the per-project sandbox policy.
// ---------------------------------------------------------------------------

/// Control-plane ports that a bind snapshot may never include (R4
/// clamp: `snapshot ∩ daemon_listen_ports = ∅`). The daemon port is
/// `--port flag > EVERLASTING_DAEMON_PORT env > 7456` (server.rs
/// `resolve_port`); this set always contains the default AND the env
/// override when parseable — a flag-only override lives in one
/// process's argv and is not observable here, so the write-time
/// rejection (Step 6 confirm route) is the authoritative gate and
/// this set is the structural backstop.
pub(crate) fn daemon_listen_ports() -> std::collections::BTreeSet<u16> {
    let mut set = std::collections::BTreeSet::new();
    set.insert(crate::daemon::server::DEFAULT_DAEMON_PORT);
    if let Ok(v) = std::env::var("EVERLASTING_DAEMON_PORT") {
        if let Ok(p) = v.parse::<u16>() {
            set.insert(p);
        }
    }
    set
}

/// Upper bound on ports in one bind snapshot. Not a security limit
/// (Landlock scales to far more rules) — a sanity cap against
/// runaway lists; a dev-server project needs single digits.
pub(crate) const MAX_BIND_PORTS: usize = 32;

/// The operator-confirmed port snapshot backing the BindOnly tier.
/// The ONLY durable authorization surface (R4): LLM/manifest
/// proposals are suggestions that flow through propose → operator
/// confirm → DB row, and never construct this type directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindSet {
    /// Sorted, deduplicated. `ports` is not pub to keep construction
    /// gated on the DB read path / tests.
    ports: std::collections::BTreeSet<u16>,
}

impl BindSet {
    /// Construct from raw ports (sorted + deduped internally).
    /// TEST-only by design (production BindSets come from
    /// `parse_ports` on a DB row — the operator-confirmed snapshot;
    /// nothing else may mint one).
    #[cfg(test)]
    pub(crate) fn from_iter_ports<I: IntoIterator<Item = u16>>(ports: I) -> Self {
        BindSet {
            ports: ports.into_iter().collect(),
        }
    }

    pub(crate) fn ports(&self) -> &std::collections::BTreeSet<u16> {
        &self.ports
    }

    /// Parse the snapshot table's `ports` TEXT column
    /// (`"3000,3001"`, same grammar as the `bind_only:` suffix).
    /// Fail-closed: any malformed token → None (caller degrades to
    /// Block + warn), never a partial port list.
    pub(crate) fn parse_ports(s: &str) -> Option<Self> {
        parse_port_list(s).map(|ports| BindSet { ports })
    }
}

/// Shared grammar for `bind_only:<list>` / snapshot `ports`:
/// comma-separated decimal u16 ports, at least one, ≤
/// [`MAX_BIND_PORTS`], each in 1..=65535. Rejects empty tokens,
/// signs, whitespace — the whole string must be exactly the list.
fn parse_port_list(s: &str) -> Option<std::collections::BTreeSet<u16>> {
    let mut out = std::collections::BTreeSet::new();
    for tok in s.split(',') {
        // u16::from_str rejects '+', '-', spaces — no manual guard.
        let p = tok.parse::<u16>().ok()?;
        if p == 0 {
            return None;
        }
        out.insert(p);
    }
    if out.is_empty() || out.len() > MAX_BIND_PORTS {
        return None;
    }
    Some(out)
}

/// Per-project network policy (09-21-sandbox-net-bindonly design §2)
/// — the orthogonal column to [`ProjectSandboxPolicy`] (file face ×
/// net face = 3×3 combinations). Stored in `projects.sandbox_net`
/// (nullable TEXT, NULL/parse-failure → Block, fail-closed).
///
/// Enforcement points are mutually exclusive by construction
/// (R2, Step 2 `PreparedNet`): Block installs the incumbent seccomp
/// INET filter (byte-identical), BindOnly installs Landlock ABI v4
/// TCP rules instead, AllowAll installs neither — and this task
/// provides NO write surface for AllowAll (capability-token
/// precondition, PRD Out of Scope; parse supports it so the enum
/// roundtrips, the confirm route refuses it).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum NetPolicy {
    /// INET socket creation blocked via seccomp — the incumbent
    /// semantics, default for NULL/unknown/unsupported-kernel.
    #[default]
    Block,
    /// No network enforcement at all. Enum/parse support only this
    /// task — no configuration entry writes it (挂账: capability
    /// token before this becomes reachable).
    AllowAll,
    /// TCP bind allowed ONLY on the operator-confirmed snapshot
    /// ports; connect allowed on `{80,443} ∪ bind` (derived, R2).
    /// Degrades to Block when the kernel lacks Landlock ABI v4
    /// (R3, decided at `prepare()` entry).
    BindOnly(BindSet),
}

impl NetPolicy {
    /// Serialize for `projects.sandbox_net` / snapshot roundtrips.
    /// `bind_only` ports are emitted sorted (BTreeSet iteration).
    pub fn as_str(&self) -> String {
        match self {
            NetPolicy::Block => "block".to_string(),
            NetPolicy::AllowAll => "allow_all".to_string(),
            NetPolicy::BindOnly(set) => {
                let list = set
                    .ports()
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                format!("bind_only:{list}")
            }
        }
    }

    /// Inverse of [`as_str`] — exactly three accepted shapes:
    /// `block` / `allow_all` / `bind_only:<ports>` (bare `bind_only`
    /// without ports is INVALID: an empty snapshot authorizes
    /// nothing and must not round-trip as a configured tier).
    /// `None` = unrecognized → callers fail-closed to Block + warn.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "block" => Some(NetPolicy::Block),
            "allow_all" => Some(NetPolicy::AllowAll),
            _ => {
                let rest = s.strip_prefix("bind_only:")?;
                BindSet::parse_ports(rest).map(NetPolicy::BindOnly)
            }
        }
    }

    /// The connect allowlist derived from a bind snapshot (R2,
    /// write-once, no config): `{80, 443} ∪ bind_ports`, minus the
    /// daemon control-plane ports (second defensive clamp — the
    /// first rejected the snapshot at write time; 7456 must stay
    /// structurally outside the agent's reachable set even if the
    /// two checks drift).
    pub(crate) fn connect_ports(set: &BindSet) -> std::collections::BTreeSet<u16> {
        let mut out = std::collections::BTreeSet::new();
        out.insert(80);
        out.insert(443);
        out.extend(set.ports().iter().copied());
        let reserved = daemon_listen_ports();
        out.retain(|p| !reserved.contains(p));
        out
    }

    /// Bind ports after the defensive daemon-port subtraction (same
    /// clamp rationale as [`Self::connect_ports`]).
    pub(crate) fn bind_ports_clamped(set: &BindSet) -> std::collections::BTreeSet<u16> {
        let reserved = daemon_listen_ports();
        set.ports()
            .iter()
            .copied()
            .filter(|p| !reserved.contains(p))
            .collect()
    }
}

impl ProjectSandboxPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectSandboxPolicy::Off => "off",
            ProjectSandboxPolicy::ReadWrite => "readwrite",
            ProjectSandboxPolicy::ReadOnly => "readonly",
        }
    }

    /// Inverse of [`as_str`]. `None` = value outside the CHECK
    /// domain (callers degrade to `Off` + warn).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" => Some(ProjectSandboxPolicy::Off),
            "readwrite" => Some(ProjectSandboxPolicy::ReadWrite),
            "readonly" => Some(ProjectSandboxPolicy::ReadOnly),
            _ => None,
        }
    }
}

/// Read the project's sandbox policy for a session via
/// `sessions.project_id` join `projects` (both PK lookups — the
/// sessions side has `idx_sessions_project_id`, the join lands on
/// the projects PK). Called per shell command from
/// `sandbox::resolve_session_policy` at both consumption points
/// (design §1.1: two consumers, one truth, no cross-layer plumbing).
///
/// Fallbacks (fail-open, matching the module philosophy):
/// - session/project row missing (fresh test pools, orphaned
///   session) → `Off` (classic behavior);
/// - value outside the CHECK domain → warn + `Off`;
/// - DB error → warn + `Off` (never sandbox on an unreadable
///   policy).
pub(crate) async fn read_project_sandbox_policy(
    db: &SqlitePool,
    session_id: &str,
) -> ProjectSandboxPolicy {
    let row: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
        r#"
        SELECT p.sandbox_policy
        FROM sessions s
        JOIN projects p ON p.id = s.project_id
        WHERE s.id = ?
        "#,
    )
    .bind(session_id)
    .fetch_optional(db)
    .await;
    match row {
        Ok(Some((v,))) => match ProjectSandboxPolicy::parse(&v) {
            Some(p) => p,
            None => {
                tracing::warn!(
                    value = %v,
                    "sandbox: unknown projects.sandbox_policy value, treating as off"
                );
                ProjectSandboxPolicy::Off
            }
        },
        Ok(None) => ProjectSandboxPolicy::Off,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "sandbox: failed to read project sandbox policy, treating as off"
            );
            ProjectSandboxPolicy::Off
        }
    }
}

/// Read the project's EFFECTIVE net policy for a session (R1/R4).
/// Two sources, one truth:
///
/// 1. `projects.sandbox_net` (tier, parsed fail-closed: NULL,
///    unknown value, or DB error → Block + warn);
/// 2. for BindOnly ONLY: the operator-confirmed snapshot row keyed
///    `(project_id, worktree_key)` — the authorization truth. The
///    row's ports WIN over the column's inline ports; a missing row
///    (fresh worktree, branch switch, swept snapshot) means BindOnly
///    has nothing to enforce → degrade to Block + warn (design §2:
///    "无快照行 → BindOnly 无从谈起").
///
/// `worktree` is the session worktree the shell will run in
/// (ToolContext.worktree_path at the caller); it is canonicalized
/// best-effort so the confirm-side and read-side keys agree even
/// under symlinked checkouts.
pub(crate) async fn read_effective_net_policy(
    db: &SqlitePool,
    session_id: &str,
    worktree: &std::path::Path,
) -> NetPolicy {
    let row: Result<Option<(String, Option<String>)>, sqlx::Error> = sqlx::query_as(
        r#"
        SELECT p.id, p.sandbox_net
        FROM sessions s
        JOIN projects p ON p.id = s.project_id
        WHERE s.id = ?
        "#,
    )
    .bind(session_id)
    .fetch_optional(db)
    .await;
    let (project_id, column) = match row {
        Ok(Some(v)) => v,
        Ok(None) => return NetPolicy::Block,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "sandbox: failed to read projects.sandbox_net, treating as block"
            );
            return NetPolicy::Block;
        }
    };
    let tier = match column.as_deref().map(NetPolicy::parse) {
        // NULL column = Block default (no log noise: the default state).
        None => return NetPolicy::Block,
        // Unparseable stored value → fail-closed Block + warn (R1).
        Some(None) => {
            tracing::warn!(
                value = ?column,
                "sandbox: unknown projects.sandbox_net value, treating as block"
            );
            return NetPolicy::Block;
        }
        Some(Some(t)) => t,
    };
    match tier {
        NetPolicy::Block | NetPolicy::AllowAll => tier,
        NetPolicy::BindOnly(_) => {
            let key = worktree_key(worktree);
            let snap: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
                "SELECT ports FROM project_net_snapshots WHERE project_id = ? AND worktree_key = ?",
            )
            .bind(&project_id)
            .bind(&key)
            .fetch_optional(db)
            .await;
            match snap {
                Ok(Some((ports_text,))) => match BindSet::parse_ports(&ports_text) {
                    Some(set) => NetPolicy::BindOnly(set),
                    None => {
                        tracing::warn!(
                            worktree_key = %key,
                            "sandbox: malformed net snapshot ports, degrading to block"
                        );
                        NetPolicy::Block
                    }
                },
                Ok(None) => {
                    // Missing snapshot row for THIS worktree: the
                    // operator never confirmed ports here.
                    tracing::warn!(
                        worktree_key = %key,
                        "sandbox: sandbox_net=bind_only but no confirmed snapshot for this worktree; degrading to block"
                    );
                    NetPolicy::Block
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "sandbox: net snapshot read failed, degrading to block"
                    );
                    NetPolicy::Block
                }
            }
        }
    }
}

/// Snapshot key = canonicalized worktree absolute path (design §2:
/// keyed by worktree so a branch switch / re-checkout cannot let an
/// old snapshot authorize ports for new code). Best-effort
/// canonicalize; the literal path is the fallback.
pub(crate) fn worktree_key(worktree: &std::path::Path) -> String {
    std::fs::canonicalize(worktree)
        .unwrap_or_else(|_| worktree.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Read side of the durable shell-prefix grants
/// (`project_shell_grants`, 09-21-durable-prefix-grant): `Some(pattern)`
/// when the operator has previously approved this command pattern for
/// this project+worktree (the matched normalized prefix — reused for
/// the grant-hit audit row), meaning the command may start WITHOUT
/// the sandbox (sandbox tier) or without the approval modal (off
/// tier). `None` = miss.
///
/// Lives HERE (not in `permissions`) because the real dependency
/// edge between the modules is permissions → sandbox
/// (`escalation.rs` imports `sandbox::SandboxBlockKind`); pulling
/// permissions from `sandbox::decide` would deepen that nominal
/// cycle. The ONLY permissions items this module may use are the
/// pure `shell_trust` leaf functions (`grant_gate` /
/// `prefix_tokens_hit` — no state, no DB) — keep it that way.
///
/// Miss semantics are fail-safe in every direction: compound command
/// (`grant_gate`), no session row / NULL project_id (test pools,
/// orphans), or any sqlx error → `None` (warn, never bubble up —
/// a grant miss only costs the sandbox exemption, never safety).
pub(crate) async fn durable_shell_grant_hit(
    db: &SqlitePool,
    session_id: &str,
    worktree: &std::path::Path,
    command: &str,
) -> Option<String> {
    if crate::agent::permissions::shell_trust::grant_gate(command) {
        return None;
    }
    let row: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
        r#"
        SELECT project_id FROM sessions WHERE id = ?
        "#,
    )
    .bind(session_id)
    .fetch_optional(db)
    .await;
    let project_id = match row {
        Ok(Some((pid,))) => pid,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "sandbox: durable grant project lookup failed, treating as miss"
            );
            return None;
        }
    };
    let key = worktree_key(worktree);
    let rows: Result<Vec<(String,)>, sqlx::Error> = sqlx::query_as(
        r#"
        SELECT prefix_tokens FROM project_shell_grants
        WHERE project_id = ? AND worktree_key = ?
        "#,
    )
    .bind(&project_id)
    .bind(&key)
    .fetch_all(db)
    .await;
    let prefixes = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "sandbox: durable grant read failed, treating as miss"
            );
            return None;
        }
    };
    prefixes.into_iter().find_map(|(stored,)| {
        crate::agent::permissions::shell_trust::prefix_tokens_hit(&stored, command)
            .then_some(stored)
    })
}
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

/// The RAW extra-writable list (RULE-SBX-002): exactly the JSON
/// array stored in app_config — no `~/.cargo` default merge, no tilde
/// expansion. This is what the settings UI edits; the effective list
/// ([`read_extra_writable`]) is display-only. Empty/missing/malformed
/// → empty list.
pub(crate) async fn read_extra_writable_raw(db: &SqlitePool) -> Vec<String> {
    match crate::db::config::get_config_value(db, CONFIG_EXTRA_KEY).await {
        Ok(Some(v)) => serde_json::from_str::<Vec<String>>(&v).unwrap_or_default(),
        _ => Vec::new(),
    }
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

/// Build the spec for one session's command under the given face
/// (P3c design §3).
///
/// - **ReadWrite face** (default): writable = worktree + `/tmp` +
///   spill dir + extras.
/// - **ReadOnly face**: writable = `/tmp` + spill dir + extras —
///   the worktree moves OUT of the writable roots but is pushed
///   back onto the EXEC face explicitly (project scripts still
///   run). Before the face split, the exec face inherited the
///   worktree indirectly via the writable-roots extend below; with
///   the worktree removed from the writable side that inheritance
///   disappears, so the explicit push is load-bearing.
///
/// Extras (`~/.cargo` etc.) stay writable under BOTH faces
/// (user-granted global dirs are orthogonal to the project face);
/// `/tmp` stays writable under both (the Plan-mode escape hatch,
/// e.g. `CARGO_TARGET_DIR=/tmp/...` investigative builds — D3).
///
/// Exec face (both): PATH dirs + `/lib` `/lib64` `/usr/lib` +
/// `/dev` + `/tmp` + writable roots + (ReadOnly: the worktree) +
/// toolchain dirs. Deliberately absent: `/init`, `/mnt/c`
/// (WSL interop containment — EXECUTE deny face is the whole
/// mechanism, spike landlock 篇 §3).
///
/// The spill directory is created best-effort here so its rule can
/// open an fd; a failure is non-fatal (the rule is skipped — the
/// child never legitimately writes there anyway, spills are
/// parent-side).
pub fn build_spec(
    ctx: &ToolContext,
    session_id: Option<&str>,
    extra_writable: Vec<PathBuf>,
    face: Face,
    net: NetPolicy,
) -> SandboxSpec {
    let mut writable_roots: Vec<PathBuf> = Vec::new();
    if face == Face::ReadWrite {
        writable_roots.push(ctx.worktree_path.clone());
    }
    writable_roots.push(PathBuf::from("/tmp"));
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
    // interpreter (`/lib64/ld-linux-x-86-64.so.2`, musl
    // `/lib/ld-musl-*.so.1`) is opened BY THE KERNEL during execve
    // and needs EXECUTE too. Normal user PATHs never include /lib*,
    // so PATH resolution alone would EACCES every dynamic binary.
    exec_allow_roots.push(PathBuf::from("/lib"));
    exec_allow_roots.push(PathBuf::from("/lib64"));
    exec_allow_roots.push(PathBuf::from("/usr/lib"));
    exec_allow_roots.push(PathBuf::from("/dev"));
    exec_allow_roots.push(PathBuf::from("/tmp"));
    // ReadOnly face: the worktree left the writable roots above, so
    // the extend below no longer carries it — re-add it for EXECUTE
    // only (the builder unions same-path access rights; in the
    // ReadWrite face this push is a dedup no-op).
    exec_allow_roots.push(ctx.worktree_path.clone());
    exec_allow_roots.extend(writable_roots.iter().cloned());
    exec_allow_roots.extend(toolchain_exec_roots());
    // F1 (09-21-sandbox-net-bindonly, R6): canonicalize every exec
    // root before it enters the spec. The KERNEL side already
    // follows symlinks at `open(O_PATH)` time (the rule lands on the
    // real directory regardless), so enforcement never depended on
    // this — the canonical form buys (a) alias-aware dedup below
    // (a PATH dir reached via two names = one rule, one fd), (b) an
    // honest exec-root list in the audit summary (the operator sees
    // the REAL face, e.g. that a wrapper's target dir is NOT in it
    // — the F0 pnpm finding), (c) stability across symlink
    // reshuffles. Un-canonicalizable roots (missing toolchain dirs)
    // stay literal — trap 5 tolerance, never an abort.
    exec_allow_roots = exec_allow_roots
        .into_iter()
        .map(|root| match std::fs::canonicalize(&root) {
            Ok(real) => real,
            Err(e) => {
                tracing::debug!(
                    path = %root.display(),
                    error = %e,
                    "sandbox: exec root not canonicalizable; kept literal (spike trap 5)"
                );
                root
            }
        })
        .collect();
    // Textual dedup AFTER canonicalization (symlink aliases now
    // merge), order-stable (first occurrence wins).
    let mut seen = std::collections::HashSet::new();
    exec_allow_roots.retain(|p| seen.insert(p.clone()));

    SandboxSpec {
        face,
        net,
        writable_roots,
        exec_allow_roots,
        extra_writable,
    }
}
