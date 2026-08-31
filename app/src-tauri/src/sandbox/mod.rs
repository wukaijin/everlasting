//! P3b — execution-time sandbox (Landlock + seccomp) for ReadOnly-tier
//! shell commands (task `08-31-a2-p3b-sandbox-executor`).
//!
//! The classification layer (`agent::permissions::shell_trust`) can
//! never be perfect: variable expansion, `eval`, aliases and indirect
//! side effects are statically invisible. This module is the damage
//! limiter UNDER that layer: a command classified `ReadOnly` runs
//! under a Landlock ruleset (EXECUTE + write-family handled, reads
//! unrestricted) plus a seccomp BPF filter (blocks `socket(AF_INET /
//! AF_INET6)`, allows AF_UNIX) — so even a misjudged command is
//! capped at "worktree + tmp + spill writable, everything else
//! read-only, no outbound network, no `/init` / `/mnt/c` exec".
//! Classification semantics are untouched (PRD C2).
//!
//! # Layout (design.md §1)
//!
//! - [`SandboxSpec`] — pure data computed in the parent process
//!   (`policy::build_spec`). **Source iron rule**: only server-side
//!   paths enter this structure (session worktree / `/tmp` / spill
//!   dir / config extras). Nothing the LLM passes in `tool_input`
//!   can reach it (CVE-2025-59532).
//! - [`resolve_policy`] / [`resolve_session_policy`] — the trigger
//!   decision (P3c: capability → Yolo → project off → kill-switch →
//!   Plan → project face; pure + testable); `decide` composes it with
//!   the per-command context. The P3b ReadOnly-tier `gate` is gone:
//!   under a sandbox face EVERY command sandboxes (`classify_prefix`
//!   no longer participates in the trigger).
//! - [`prepare`] — parent-process "safe zone": opens the ruleset fd
//!   + one `O_PATH` fd per path, builds the BPF program. May
//!   allocate / open freely.
//! - [`apply`] — registers a `pre_exec` closure. The closure runs in
//!   the forked child on the async-signal-safety edge: it only
//!   issues raw syscalls (prctl / landlock_add_rule /
//!   landlock_restrict_self / one seccomp prctl) reading
//!   parent-constructed memory through an `Arc`; no malloc, no
//!   open, no locks (design.md §2.3).
//!
//! # Failure semantics
//!
//! - Capability probe fails (old kernel, WSL1, non-Linux) →
//!   fail-open: the command runs unsandboxed, one log line, no
//!   error, no hang (R5; generalization.md §3 ladder).
//! - Prepare / pre-exec failure → the spawn itself fails with a
//!   `[sandbox]`-prefixed error (fail-closed: we never half-apply a
//!   ruleset).
//! - Kill switch: `sandbox_enabled=false` in app_config → the spawn
//!   path never registers `pre_exec`, byte-identical to the
//!   pre-P3b behavior (R6).
//!
//! Spike provenance: ruleset recipe + the five implementation traps
//! come from `.trellis/tasks/08-31-a2-p3a-sandbox-spike/research/
//! wsl2-feasibility-landlock.md` (ABI v1 subset, rule-access ⊆
//! handled else EINVAL, per-file device rules, NoNewPrivs first,
//! tolerate-missing-paths). Trap 2 is eliminated at the type level
//! by [`landlock::AccessSet`], whose only constructors are subsets
//! of the handled mask.

pub mod landlock;
pub mod policy;
pub mod seccomp;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::process::Command;

use crate::db::Mode;
use crate::tools::ToolContext;

/// Device nodes that get a per-file `WRITE_FILE` rule (spike trap 3:
/// `O_RDWR` on `/dev/null` counts as WRITE_FILE, without which `git`
/// dies on its first invocation). The list is a fixed constant —
/// config only ever adds writable *directories*, never devices.
pub(crate) const DEVICE_WRITE_PATHS: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/full",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
];

/// Which writable face a sandboxed command gets (P3c, design §3).
/// Both faces keep `/tmp` + spill + extras writable; they differ in
/// the session worktree only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Worktree writable — the default face (per-project
    /// `readwrite` policy). Project-internal work is free.
    ReadWrite,
    /// Worktree READ-only (Plan mode / per-project `readonly`
    /// policy). The worktree moves out of the writable roots but
    /// STAYS on the exec face (project scripts still run).
    ReadOnly,
}

impl Face {
    /// Short token for the audit ruleset summary (`face=ro|rw`).
    pub fn as_str(self) -> &'static str {
        match self {
            Face::ReadWrite => "rw",
            Face::ReadOnly => "ro",
        }
    }
}

/// The resolved sandbox policy for one command (P3c, design §1) —
/// the single source of truth shared by the permission layer (Tier 4
/// shell short-circuit) and the spawn side ([`decide`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// No sandbox — classic pre-execution approval path (Tier 4:
    /// prefix-grant / three-tier classify / ask).
    Off,
    /// Every shell command runs under the sandbox with the given
    /// face; out-of-face failures escalate at execution time
    /// (foreground shell) instead of pre-execution approval.
    Face(Face),
}

/// Pure data describing what a sandboxed command may do. Built by
/// [`policy::build_spec`] in the parent, consumed by [`prepare`].
///
/// The two path lists may name the same directories (e.g. `/tmp` is
/// both executable and writable); `landlock::RulesetBuilder` merges
/// same-path access rights, mirroring the kernel's union semantics
/// without relying on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxSpec {
    /// Which writable face the spec implements (`face=` audit
    /// segment; P3c design §3).
    pub face: Face,
    /// Writable subtree roots: session worktree (ReadWrite face
    /// only) + `/tmp` + the session spill dir + config
    /// `sandbox_extra_writable` entries.
    pub writable_roots: Vec<PathBuf>,
    /// Executable subtree roots: PATH dirs + `/dev` + `/tmp` +
    /// writable roots + probed toolchain dirs. Deliberately NOT
    /// `/init` / `/mnt/c` (WSL interop containment).
    pub exec_allow_roots: Vec<PathBuf>,
    /// Config-derived extra writable roots (already `~`-expanded;
    /// kept separate from `writable_roots` for audit readability —
    /// the builder unions them into the write face).
    pub extra_writable: Vec<PathBuf>,
}

/// Outcome of the per-command sandbox decision ([`decide`] /
/// [`resolve_policy`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Run the command under the given spec.
    Sandbox(SandboxSpec),
    /// Do not sandbox. `reason` goes to tracing (debug) — never to
    /// the audit log (design §2.2: skips are not security events).
    Skip { reason: &'static str },
}

/// Cached kernel capability probe (R5). `OnceLock`-cached: the
/// kernel does not gain features mid-process, and `PR_GET_SECCOMP`
/// is cheap but not free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    pub landlock: bool,
    pub seccomp: bool,
}

impl Capability {
    /// All-available (the happy path on WSL2 / CI 24.04 runners).
    pub fn ok(self) -> bool {
        self.landlock && self.seccomp
    }

    /// Probe once, cache forever (design.md §2.2 / R5). Never
    /// panics, never blocks: two prctls/syscalls on first call.
    pub fn probe() -> Self {
        use std::sync::OnceLock;
        static CAP: OnceLock<Capability> = OnceLock::new();
        *CAP.get_or_init(|| {
            let cap = probe_once();
            if cap.ok() {
                tracing::debug!(?cap, "sandbox: capability probe ok");
            } else {
                // R5: fail-open with a one-line log. This fires once
                // per process (probe is cached) — the degrade reason
                // ("sandbox inactive: <kernel too old / WSL1>") is
                // also surfaced via get_app_config sandbox_capability.
                tracing::info!(
                    ?cap,
                    "sandbox: capability probe failed; fail-open (commands run unsandboxed)"
                );
            }
            cap
        })
    }
}

/// Non-cached probe body. Landlock: `landlock_create_ruleset(NULL, 0,
/// VERSION)` returns the ABI version (≥1) when the LSM is available.
/// Seccomp: `prctl(PR_GET_SECCOMP)` returns the current mode (≥0)
/// when compiled in, -1/EINVAL when the kernel lacks seccomp. Both
/// are read-only probes with no process-state side effects.
fn probe_once() -> Capability {
    #[cfg(target_os = "linux")]
    {
        let landlock = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                std::ptr::null::<libc::c_void>(),
                0 as libc::size_t,
                landlock::LANDLOCK_CREATE_RULESET_VERSION,
            )
        } >= 1;
        let seccomp = unsafe { landlock::prctl(landlock::PR_GET_SECCOMP, 0, 0, 0, 0) } >= 0;
        Capability { landlock, seccomp }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // macOS / Windows: no Landlock, no seccomp → fail-open (C4).
        Capability {
            landlock: false,
            seccomp: false,
        }
    }
}

/// Evaluate the sandbox policy for one command context (P3c design
/// §1 — replaces the P3b four-way `gate`). Pure — the caller supplies
/// every input, so tests can drive each branch independently.
///
/// Evaluation order is short-circuit and mirrors the staged DB reads
/// in [`resolve_session_policy`] (config reads are lazy — the
/// RULE-SBX-004 spirit: a config read must not pay for a decision
/// already settled by cheaper checks). Both checks 3 and 4 produce
/// `Off`; the body orders kill-switch before the project tiers for
/// readability, the staged I/O wrapper preserves the documented
/// read order:
///
/// 1. capability probe failed → `Off` (fail-open, unchanged);
/// 2. mode == Yolo → `Off` (恒不沙盒, unchanged);
/// 3. kill-switch == false → `Off` (global master, beats every face);
/// 4. project policy == Off → `Off` (per-project opt-out);
/// 5. mode == Plan → `Face(ReadOnly)` (session-level read-only face
///    overrides the project face — D3);
/// 6. project policy → `Face(its tier)`.
///
/// `classify_prefix` no longer participates in the trigger (P3c:
/// every command sandboxes under a face); the classification layer
/// semantics are untouched and still serve the Tier 4 path when the
/// policy resolves `Off`.
pub fn resolve_policy(
    mode: Mode,
    project_policy: policy::ProjectSandboxPolicy,
    kill_switch: bool,
    cap: Capability,
) -> Policy {
    if !cap.ok() {
        return Policy::Off;
    }
    if mode == Mode::Yolo {
        // R4: Yolo already granted full trust by the user.
        return Policy::Off;
    }
    if !kill_switch {
        // Global master switch: off = no sandbox anywhere, including
        // readonly-face projects.
        return Policy::Off;
    }
    match (project_policy, mode) {
        (policy::ProjectSandboxPolicy::Off, _) => Policy::Off,
        // Plan's value is the deterministic read-only face (D3): the
        // project face is overridden for the session, but a project
        // opt-out (checked above) still turns the whole chain off —
        // that combination falls back to the Plan tool filter.
        (_, Mode::Plan) => Policy::Face(Face::ReadOnly),
        (policy::ProjectSandboxPolicy::ReadWrite, _) => Policy::Face(Face::ReadWrite),
        (policy::ProjectSandboxPolicy::ReadOnly, _) => Policy::Face(Face::ReadOnly),
    }
}

/// Resolve the policy for one session's shell command from the DB
/// (design §1.1 "两处消费,一处真源"). Staged reads keep the config
/// queries lazy: the capability probe is cached, Yolo needs no I/O,
/// and the project-policy point query (`sessions.project_id` join
/// `projects`, both PK lookups) runs before the kill-switch config
/// read. Consumed by the Tier 4 shell short-circuit
/// (`permissions/check/permission.rs`) and by [`decide`].
///
/// Missing session/project rows (fresh test pools, degenerate
/// states) resolve `Off` — classic behavior, matching the module's
/// fail-open philosophy.
pub async fn resolve_session_policy(db: &sqlx::SqlitePool, session_id: &str, mode: Mode) -> Policy {
    let cap = Capability::probe();
    if !cap.ok() {
        return Policy::Off;
    }
    if mode == Mode::Yolo {
        return Policy::Off;
    }
    let project_policy = policy::read_project_sandbox_policy(db, session_id).await;
    if project_policy == policy::ProjectSandboxPolicy::Off {
        return Policy::Off;
    }
    let enabled = policy::sandbox_enabled(db).await;
    resolve_policy(mode, project_policy, enabled, cap)
}

/// Per-command decision entry point (shell tool family). Resolves the
/// policy via [`resolve_session_policy`] and composes a
/// [`policy::build_spec`] on the Sandbox path. The returned `Decision`
/// is consumed once by the tool and reused for the post-hoc
/// write-block guidance and the audit row (W3: no second query).
pub async fn decide(ctx: &ToolContext, command: &str, session_id: Option<&str>) -> Decision {
    let policy = match session_id {
        Some(sid) => resolve_session_policy(&ctx.db, sid, ctx.mode).await,
        None => {
            // No session context (test paths): nothing to resolve a
            // project policy from → classic unsandboxed behavior.
            Policy::Off
        }
    };
    match policy {
        Policy::Off => {
            tracing::debug!(
                command_sha = %command_sha_prefix(command),
                "sandbox: skip (policy Off)"
            );
            Decision::Skip {
                reason: "policy resolved Off",
            }
        }
        Policy::Face(face) => {
            let extra = policy::read_extra_writable(&ctx.db).await;
            Decision::Sandbox(policy::build_spec(ctx, session_id, extra, face))
        }
    }
}

/// Parent-process preparation (design §2.3 "safe zone"): creates the
/// Landlock ruleset fd, opens one `O_PATH` fd per rule path, builds
/// the BPF program. All of this is allowed to allocate / open / take
/// locks — it never touches the pre_exec edge.
///
/// Fails only on kernel-side ruleset creation (e.g. handled mask
/// rejected) — a missing path is NOT an error (spike trap 5: the
/// rule is skipped and logged).
pub fn prepare(spec: &SandboxSpec) -> std::io::Result<PreparedSandbox> {
    #[cfg(target_os = "linux")]
    {
        let mut builder = landlock::RulesetBuilder::new();
        for root in &spec.exec_allow_roots {
            builder.allow(root, landlock::AccessSet::EXECUTE);
        }
        for root in spec.writable_roots.iter().chain(spec.extra_writable.iter()) {
            builder.allow(root, landlock::AccessSet::WRITE_FAMILY);
        }
        for dev in DEVICE_WRITE_PATHS {
            builder.allow(std::path::Path::new(dev), landlock::AccessSet::WRITE_FILE);
        }
        let ruleset = builder.build()?;
        let bpf = seccomp::build_inet_block_filter();
        Ok(PreparedSandbox {
            data: Arc::new(PreparedData {
                ruleset_fd: ruleset.ruleset_fd,
                rules: ruleset.rules,
                bpf,
            }),
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = spec;
        // Unreachable in practice: the policy never resolves a face when
        // the probe fails, and the probe always fails off-Linux. The
        // stub exists so the tool layer needs no cfg.
        Ok(PreparedSandbox {
            data: Arc::new(PreparedData),
        })
    }
}

/// Register the pre_exec application on a command. The closure body
/// is syscall-only (see module docs); failures surface from
/// `cmd.spawn()` as an io::Error, which the tool layer reports with
/// a `[sandbox]` prefix (fail-closed, design §2.3).
pub fn apply(cmd: &mut Command, prepared: &PreparedSandbox) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let data = Arc::clone(&prepared.data);
        unsafe {
            cmd.pre_exec(move || pre_exec_apply(&data));
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (cmd, prepared);
        Ok(())
    }
}

/// The syscall-only pre_exec body (Linux). Order is load-bearing:
/// NoNewPrivs FIRST (spike trap 4 — restrict_self returns EACCES
/// without it; it also kills the suid escalation surface), then all
/// add_rule calls (each failure aborts the whole spawn, aligned with
/// the spike probe's `_exit(99)` semantics), then restrict_self, then
/// the seccomp filter LAST so the filter never interferes with the
/// landlock syscalls above it.
#[cfg(target_os = "linux")]
fn pre_exec_apply(data: &PreparedData) -> std::io::Result<()> {
    // 1. PR_SET_NO_NEW_PRIVS — required before restrict_self; also
    //    blocks suid/sgid privilege gain inside the sandbox.
    if unsafe { landlock::prctl(landlock::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // 2. Add one PATH_BENEATH rule per (fd, access) pair. Stack-only
    //    attr struct; the fd numbers were opened in the parent and
    //    are valid in the forked child until exec.
    for (fd, access) in &data.rules {
        let attr = landlock::PathBeneathAttr {
            allowed_access: *access,
            parent_fd: *fd,
        };
        let ret = unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                data.ruleset_fd,
                landlock::LANDLOCK_RULE_PATH_BENEATH,
                &attr,
                0 as libc::c_uint,
            )
        };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    // 3. Restrict: from here the child can never regain the dropped
    //    rights (irreversible for the process tree — spike §1).
    if unsafe {
        libc::syscall(
            libc::SYS_landlock_restrict_self,
            data.ruleset_fd,
            0 as libc::c_uint,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // 4. Seccomp: the kernel copies the filter program out of
    //    `data.bpf` during this one prctl (W2: referencing
    //    parent-constructed memory is safe — no malloc in the
    //    closure; the sock_fprog header is stack-built).
    seccomp::install_in_preexec(&data.bpf)?;
    Ok(())
}

/// Parent-owned sandbox artifacts for ONE spawn. `Arc<PreparedData>`
/// is captured by the pre_exec closure ('static requirement) and
/// read through by reference in the forked child; `Drop` closes the
/// fds in the parent after `spawn()` returns (std guarantees the
/// child has already exec'd or died by then — the parent-side close
/// cannot race the child's use).
pub struct PreparedSandbox {
    data: Arc<PreparedData>,
}

impl SandboxSpec {
    /// One-line ruleset summary for the audit payload (design §2.6:
    /// the audit row records the shape of the ruleset, never the
    /// command text — the command is already in `tool_executed`).
    /// Root counts, not rule counts: the ruleset builder merges
    /// same-path access rights, so this stays stable without opening
    /// any fd — both spawn paths (foreground `shell` + background
    /// registry consumer) audit with the SAME shape.
    pub(crate) fn summary(&self) -> String {
        format!(
            "landlock:face={} exec_roots={} writable_roots={} extra={} devices={}; seccomp:inet_block",
            self.face.as_str(),
            self.exec_allow_roots.len(),
            self.writable_roots.len(),
            self.extra_writable.len(),
            DEVICE_WRITE_PATHS.len()
        )
    }
}

#[cfg(target_os = "linux")]
struct PreparedData {
    ruleset_fd: std::os::fd::RawFd,
    rules: Vec<(std::os::fd::RawFd, u64)>,
    bpf: Vec<libc::sock_filter>,
}

#[cfg(not(target_os = "linux"))]
struct PreparedData;

#[cfg(target_os = "linux")]
impl Drop for PreparedData {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.ruleset_fd);
            for (fd, _) in &self.rules {
                libc::close(*fd);
            }
        }
    }
}

/// Stable short hash of the command text for audit correlation
/// (design §2.6: audit row carries a command hash, not the command —
/// the full text is already stored by `tool_executed`).
pub(crate) fn command_sha_prefix(command: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(command.as_bytes());
    let out = h.finalize();
    out.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// What a sandboxed command's failure stderr smells like (P3c design
/// §5.1). Conservative substring match on the canonical denial
/// strings — a miss degrades to no guidance, never to a false
/// escalation.
pub(crate) enum SandboxBlockKind {
    /// Landlock write denial (`Permission denied` /
    /// `Read-only file system`).
    Write,
    /// seccomp egress block (`Operation not permitted` — EPERM at
    /// `socket()`, before any connect attempt).
    Network,
}

/// Classify a failed sandboxed command's stderr. Order matters: the
/// write strings are checked first (a stderr carrying both is a
/// write failure with noise). Used by the guidance text (§5.3) and
/// by the escalation trigger (§5.1) so both share one heuristic.
pub(crate) fn classify_block(stderr: &str) -> Option<SandboxBlockKind> {
    if stderr.contains("Permission denied") || stderr.contains("Read-only file system") {
        Some(SandboxBlockKind::Write)
    } else if stderr.contains("Operation not permitted") {
        Some(SandboxBlockKind::Network)
    } else {
        None
    }
}

/// Post-hoc failure guidance, mode-aware (P3c design §5.3 — replaces
/// the P3b single write-block line). When a sandboxed command failed
/// and its stderr smells like a sandbox denial, the tool appends one
/// line so the model knows WHY and what to do. Heuristic,
/// append-only — the command's own output is never rewritten.
/// `None` = no append (宁缺勿滥: only the canonical denial strings
/// trigger it).
///
/// Variants:
/// - Edit + write: an escalation card may appear for this command;
///   otherwise adjust `sandbox_extra_writable` / the project tier.
/// - Plan + write (D3): blocked BY DESIGN — propose a diff, ask the
///   user to switch to Edit, or use /tmp (no escalation exists).
/// - Network (both modes): the sandbox has no egress; Edit names the
///   escalation card, Plan states the design intent.
pub(crate) fn failure_guidance(stderr: &str, mode: Mode) -> Option<&'static str> {
    match classify_block(stderr) {
        Some(SandboxBlockKind::Write) => Some(match mode {
            Mode::Plan => {
                "[sandbox] The write above was blocked by the Plan-mode read-only sandbox — \
                 this is by design. Propose the change as a diff and ask the user to switch \
                 to Edit mode, or write intermediate artifacts to /tmp (e.g. \
                 CARGO_TARGET_DIR=/tmp/build) — there is no approval card in Plan mode."
            }
            _ => {
                "[sandbox] The failure above looks like a sandbox write block (writable roots: \
                 the session worktree, /tmp, and the app outputs dir). Approve the escalation \
                 card for this command if one appears; otherwise ask the user to add the path \
                 to `sandbox_extra_writable` in Settings or change the project's sandbox policy."
            }
        }),
        Some(SandboxBlockKind::Network) => Some(match mode {
            Mode::Plan => {
                "[sandbox] Outbound network is blocked inside the Plan-mode read-only sandbox — \
                 this is by design. Ask the user to run the networked command, or switch to \
                 Edit mode."
            }
            _ => {
                "[sandbox] The failure above looks like the sandbox blocking outbound network \
                 (no egress inside the sandbox). Approve the escalation card for this command \
                 if one appears, or ask the user to change the project's sandbox policy."
            }
        }),
        None => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests_sandbox.rs"]
mod tests_sandbox;
