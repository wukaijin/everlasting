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
    /// Network policy dimension (09-21-sandbox-net-bindonly): which
    /// network enforcer `prepare()` installs. Orthogonal to `face`
    /// (file face × net face); default/NULL/unknown → [`NetPolicy::Block`]
    /// = the incumbent seccomp INET filter, byte-identical (AC1).
    pub net: policy::NetPolicy,
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
    /// Landlock ABI ≥4 TCP-port rules (kernel 6.7+): the only
    /// mechanism that can allow listen without allowing arbitrary
    /// egress (spec §12.3 C). Probed by actually creating a
    /// net-handling ruleset (EINVAL on older kernels); the BindOnly
    /// tier degrades to Block when this is false (R3) — never to
    /// AllowAll.
    pub landlock_net: bool,
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
        // Net-rule probe (ABI ≥4): create a ruleset that handles
        // exactly the two net access bits. Success → the kernel
        // knows them (we could add NET_PORT rules); EINVAL on older
        // kernels. Read-only side effect: the scratch fd is closed
        // immediately and restrict is never applied.
        let landlock_net = landlock && {
            let attr = landlock::RulesetAttr {
                handled_access_fs: 0,
                handled_access_net: landlock::HANDLED_ACCESS_NET,
            };
            let fd = unsafe {
                libc::syscall(
                    libc::SYS_landlock_create_ruleset,
                    &attr as *const landlock::RulesetAttr,
                    std::mem::size_of::<landlock::RulesetAttr>(),
                    0 as libc::c_uint,
                )
            };
            if fd >= 0 {
                unsafe { libc::close(fd as std::os::fd::RawFd) };
                true
            } else {
                false
            }
        };
        let seccomp = unsafe { landlock::prctl(landlock::PR_GET_SECCOMP, 0, 0, 0, 0) } >= 0;
        Capability {
            landlock,
            landlock_net,
            seccomp,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // macOS / Windows: no Landlock, no seccomp → fail-open (C4).
        Capability {
            landlock: false,
            landlock_net: false,
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
            // Durable prefix-grant exemption (09-21-durable-prefix-grant,
            // consumer A / R3): the operator has previously approved this
            // command pattern for this project+worktree → the command
            // starts WITHOUT the sandbox (the whole point for long-lived
            // dev servers: no failed first attempt, no rerun). Checked
            // BEFORE the extra-writable / net reads so a hit skips those
            // queries too. Plan NEVER exempts (D3 alignment: Plan's value
            // is the deterministic read-only face — same gate as the
            // escalation trigger's `mode != Plan`). The audit row for a
            // grant-hit Skip is written by the tool layer (it matches
            // [`DURABLE_GRANT_SKIP_REASON`]) — this module stays
            // permissions-import-clean (see policy.rs's module contract).
            if ctx.mode != crate::db::Mode::Plan {
                if let Some(sid) = session_id {
                    if policy::durable_shell_grant_hit(&ctx.db, sid, &ctx.worktree_path, command)
                        .await
                        .is_some()
                    {
                        tracing::info!(
                            command_sha = %command_sha_prefix(command),
                            "sandbox: skip (durable prefix grant hit)"
                        );
                        return Decision::Skip {
                            reason: DURABLE_GRANT_SKIP_REASON,
                        };
                    }
                }
            }
            let extra = policy::read_extra_writable(&ctx.db).await;
            // Net dimension accompanies the project-face read (design
            // §1: read where sandbox_policy is read, NOT a new gate —
            // resolve_policy/5-Tier semantics untouched). Only the
            // Sandbox path pays this query; Skip/Off never does.
            let net = match session_id {
                Some(sid) => {
                    policy::read_effective_net_policy(&ctx.db, sid, &ctx.worktree_path).await
                }
                None => policy::NetPolicy::Block,
            };
            Decision::Sandbox(policy::build_spec(ctx, session_id, extra, face, net))
        }
    }
}

/// `Decision::Skip` reason for a durable prefix-grant hit. The tool
/// layer matches this constant to write the grant-hit audit row (the
/// sandbox module itself never writes audits — tool-side contract,
/// same split as `SandboxedShellExecution`).
pub const DURABLE_GRANT_SKIP_REASON: &str = "durable prefix grant";

/// The mutually-exclusive network enforcer for one spawn (R2). The
/// type makes "seccomp and Landlock-net both installed" unrepresentable:
/// exactly one variant is carried, and `pre_exec_apply` matches on it.
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub(crate) enum PreparedNet {
    /// seccomp INET filter — the incumbent Block semantics, byte-
    /// identical program (`seccomp::build_inet_block_filter`).
    Block(Vec<libc::sock_filter>),
    /// Landlock ABI v4 TCP-port rules: BIND on the (clamped) snapshot
    /// ports, CONNECT on the derived `{80,443} ∪ bind` set; seccomp
    /// is NOT installed (socket creation unrestricted, UDP/DNS still
    /// open — documented residual, R8 copy).
    BindOnly {
        bind: Vec<landlock::NetPortAttr>,
        connect: Vec<landlock::NetPortAttr>,
    },
    /// No net enforcement. Unreachable from configuration this task
    /// (AllowAll has no write surface); exists so the enum roundtrips.
    AllowAll,
}

/// Parent-process preparation (design §2.3 "safe zone"): creates the
/// Landlock ruleset fd, opens one `O_PATH` fd per rule path, builds
/// the net/seccomp enforcer artifacts. All of this is allowed to
/// allocate / open / take locks — it never touches the pre_exec edge.
///
/// Net-tier assembly (R2/R3):
/// - Block → incumbent seccomp program, nothing else;
/// - BindOnly → net-handling ruleset + parent-built NET_PORT attr
///   arrays (bind = clamped snapshot, connect = derived set with the
///   second daemon-port clamp); capability-gated: probe without ABI
///   v4 net rules → **degrade to Block** (warn, never AllowAll);
/// - AllowAll → neither enforcer (type/parse support only).
///
/// Fails only on kernel-side ruleset creation (e.g. handled mask
/// rejected) — a missing path is NOT an error (spike trap 5: the
/// rule is skipped and logged).
pub fn prepare(spec: &SandboxSpec) -> std::io::Result<PreparedSandbox> {
    #[cfg(target_os = "linux")]
    {
        let cap = Capability::probe();
        let net = match &spec.net {
            policy::NetPolicy::Block => PreparedNet::Block(seccomp::build_inet_block_filter()),
            policy::NetPolicy::AllowAll => PreparedNet::AllowAll,
            policy::NetPolicy::BindOnly(set) => {
                if !cap.landlock_net {
                    // R3: containment leans into containment. A BindOnly
                    // tier on a kernel without ABI v4 net rules cannot
                    // be enforced → the incumbent Block filter (NOT
                    // AllowAll: an unenforceable promise must degrade
                    // to the stricter incumbent, and the summary
                    // reports the degrade).
                    tracing::warn!(
                        bind_ports = ?set.ports(),
                        "sandbox: kernel lacks Landlock ABI v4 net rules; \
                         net=bind_only degrading to block"
                    );
                    PreparedNet::Block(seccomp::build_inet_block_filter())
                } else {
                    let bind_ports = policy::NetPolicy::bind_ports_clamped(set);
                    let connect_ports = policy::NetPolicy::connect_ports(set);
                    PreparedNet::BindOnly {
                        bind: landlock::net_port_attrs(
                            landlock::NetAccessSet::BIND_TCP,
                            &bind_ports,
                        ),
                        connect: landlock::net_port_attrs(
                            landlock::NetAccessSet::CONNECT_TCP,
                            &connect_ports,
                        ),
                    }
                }
            }
        };
        let mut builder = landlock::RulesetBuilder::new();
        if matches!(net, PreparedNet::BindOnly { .. }) {
            builder.handle_net_access(landlock::HANDLED_ACCESS_NET);
        }
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
        Ok(PreparedSandbox {
            data: Arc::new(PreparedData {
                ruleset_fd: ruleset.ruleset_fd,
                rules: ruleset.rules,
                net,
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
/// add_rule calls (file rules always; NET_PORT rules on the BindOnly
/// tier — each failure aborts the whole spawn, aligned with the
/// spike probe's `_exit(99)` semantics), then restrict_self, then —
/// on the Block tier ONLY — the seccomp filter LAST so the filter
/// never interferes with the landlock syscalls above it. The two net
/// enforcers are mutually exclusive by the match on `PreparedNet`
/// (R2): there is no code path that installs both.
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
    // 2b. Net tier rules (BindOnly only): one NET_PORT rule per
    //     (access, port) attr, same abort-on-failure semantics as
    //     the file rules. Attr structs live in parent-constructed
    //     Vecs read through the Arc (W2) — no allocation here.
    if let PreparedNet::BindOnly { bind, connect } = &data.net {
        for attr in bind.iter().chain(connect.iter()) {
            let ret = unsafe {
                libc::syscall(
                    libc::SYS_landlock_add_rule,
                    data.ruleset_fd,
                    landlock::LANDLOCK_RULE_NET_PORT,
                    attr as *const landlock::NetPortAttr,
                    0 as libc::c_uint,
                )
            };
            if ret != 0 {
                return Err(std::io::Error::last_os_error());
            }
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
    // 4. Seccomp — Block tier ONLY (R2 mutual exclusion): the kernel
    //    copies the filter program out of the parent-constructed Vec
    //    during this one prctl (W2: no malloc in the closure; the
    //    sock_fprog header is stack-built). BindOnly installs no
    //    seccomp (socket creation unrestricted; Landlock owns ports),
    //    AllowAll installs nothing.
    if let PreparedNet::Block(bpf) = &data.net {
        seccomp::install_in_preexec(bpf)?;
    }
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

/// Which net enforcer a spawn actually installs — the SERVER-SIDE
/// fact behind the R9 attribution conjunction. Derived from
/// `spec.net` + `Capability::probe()` (a BindOnly tier on a kernel
/// without ABI v4 degrades to the INET filter — this enum reports
/// what RUNS, the same truth `summary()` prints and `prepare()`
/// installs; one truth, three readers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NetEnforcement {
    /// seccomp INET filter (Block tier, or degraded BindOnly).
    InetBlock,
    /// Landlock ABI v4 TCP rules (BindOnly on a capable kernel).
    LandlockNet,
    /// No net enforcement (AllowAll tier).
    None,
}

impl SandboxSpec {
    /// The R9 attribution conjunction input: which net enforcer the
    /// spawn for THIS spec actually installs.
    pub(crate) fn net_enforcement(&self) -> NetEnforcement {
        match &self.net {
            policy::NetPolicy::Block => NetEnforcement::InetBlock,
            policy::NetPolicy::AllowAll => NetEnforcement::None,
            policy::NetPolicy::BindOnly(_) => {
                if cfg!(target_os = "linux") && Capability::probe().landlock_net {
                    NetEnforcement::LandlockNet
                } else {
                    NetEnforcement::InetBlock
                }
            }
        }
    }
}

impl SandboxSpec {
    /// One-line ruleset summary for the audit payload (design §2.6:
    /// the audit row records the shape of the ruleset, never the
    /// command text — the command is already in `tool_executed`).
    /// Root counts, not rule counts: the ruleset builder merges
    /// same-path access rights, so this stays stable without opening
    /// any fd — both spawn paths (foreground `shell` + background
    /// registry consumer) audit with the SAME shape.
    ///
    /// 2026-09-21 (R8/R9): a `net=` segment is appended. It is the
    /// SERVER-SIDE fact the listen-denial attribution requires (R9):
    /// `net=block` proves the INET filter tier was actually
    /// configured, so a `listen EPERM` string under it may be
    /// classified as a sandbox block; `net=bind_only(...)` proves it
    /// was NOT (such a string must have another cause). A BindOnly
    /// tier on a kernel without ABI v4 net rules reports the degrade
    /// (`net=bind_only->block(degraded)`) — the summary never claims
    /// an enforcement the spawn did not install.
    pub(crate) fn summary(&self) -> String {
        let base = format!(
            "landlock:face={} exec_roots={} writable_roots={} extra={} devices={}",
            self.face.as_str(),
            self.exec_allow_roots.len(),
            self.writable_roots.len(),
            self.extra_writable.len(),
            DEVICE_WRITE_PATHS.len()
        );
        let (net_segment, enforcer_segment) = match &self.net {
            policy::NetPolicy::Block => ("net=block".to_string(), "; seccomp:inet_block"),
            policy::NetPolicy::AllowAll => ("net=allow_all".to_string(), ""),
            policy::NetPolicy::BindOnly(set) => {
                let ports = set
                    .ports()
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                if cfg!(target_os = "linux") && !Capability::probe().landlock_net {
                    // Degraded at prepare(): the summary reports what
                    // the spawn actually installs, never the promise.
                    (
                        format!("net=bind_only({ports})->block(degraded)"),
                        "; seccomp:inet_block",
                    )
                } else {
                    (
                        format!("net=bind_only({ports})"),
                        "; landlock_net:bind_connect",
                    )
                }
            }
        };
        // F1 (2026-09-21, R6/AC5): the canonical exec-root list.
        // Dirs only (the face is directories by construction — no
        // file paths ever enter this list); audit-local surface, the
        // point is operator visibility of the REAL face (F0: the
        // pnpm wrapper's target dir being absent was invisible while
        // the summary carried only a count).
        let exec_list = self
            .exec_allow_roots
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(",");
        format!("{base}; {net_segment}{enforcer_segment}; exec_roots_canonical=[{exec_list}]")
    }
}

#[cfg(target_os = "linux")]
struct PreparedData {
    ruleset_fd: std::os::fd::RawFd,
    rules: Vec<(std::os::fd::RawFd, u64)>,
    net: PreparedNet,
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

/// What a sandboxed command's failure smells like (P3c design
/// §5.1 + F3 2026-09-21). Conservative substring match on canonical
/// denial strings — a miss degrades to no guidance, never to a
/// false escalation.
pub(crate) enum SandboxBlockKind {
    /// Landlock write denial (`Permission denied` /
    /// `Read-only file system`).
    Write,
    /// seccomp egress block (`Operation not permitted` — EPERM at
    /// `socket()`, before any connect attempt).
    Network,
    /// Landlock EXEC-face miss (F3): `exit_code == 126` ∧ stderr
    /// `Permission denied` — the shell found a program but execve
    /// was denied (wrapper script exec'ing a binary outside the
    /// exec roots, e.g. pnpm's `@pnpm/exe`). 126 is the strong
    /// signal that separates this from fs-write denials.
    ExecFace,
}

/// Classify a failed sandboxed command's denial (2026-09-21: stderr +
/// stdout + exit code + the R9 server-side conjunction). Order:
///
/// 1. `exit 126 ∧ stderr Permission denied` → ExecFace (most
///    specific: execve denial is fatal regardless of what else the
///    streams carry);
/// 2. write strings (stderr) → Write;
/// 3. network strings → Network, ONLY under `net == InetBlock` (R9:
///    the classification asserts a cause the spawn actually enforced
///    — an INET filter. On a BindOnly spawn (LandlockNet) the INET
///    filter was NOT installed, so a `listen EPERM` string must have
///    another cause and gets NO network attribution. Strings feed
///    UX only; this conjunction is the server-side fact that makes
///    the attribution non-forgivable by command output).
///
/// stdout participates ONLY through the strong listen/socket markers
/// in [`stream_smells_net_block`] (宁缺勿滥: a bare `Operation not
/// permitted` / `Permission denied` in stdout is never trusted).
pub(crate) fn classify_block(
    stderr: &str,
    stdout: &str,
    exit_code: Option<i32>,
    net: NetEnforcement,
) -> Option<SandboxBlockKind> {
    if exit_code == Some(126) && stderr.contains("Permission denied") {
        return Some(SandboxBlockKind::ExecFace);
    }
    if stderr.contains("Permission denied") || stderr.contains("Read-only file system") {
        return Some(SandboxBlockKind::Write);
    }
    if net != NetEnforcement::InetBlock {
        return None;
    }
    // 2026-09-22 (live E2E, task 09-21-durable-prefix-grant): raw node /
    // go dev servers crash with the listen EPERM on STDERR — libuv and
    // go print errno strings lowercase ("operation not permitted"), so
    // the historical capital-O literal missed them entirely (no card, no
    // guidance). The errno literal is now casing-robust on stderr, and
    // the three strong listen shapes run against BOTH streams — dev
    // toolchains split the report across streams arbitrarily (§12.2).
    let stderr_net_denial = stderr
        .to_ascii_lowercase()
        .contains("operation not permitted");
    if stderr_net_denial || stream_smells_net_block(stdout) || stream_smells_net_block(stderr) {
        Some(SandboxBlockKind::Network)
    } else {
        None
    }
}

/// Strong network-block markers for the listen-denial shapes, run
/// against BOTH streams by [`classify_block`] (2026-09-22: raw node
/// crashes report to stderr; vite-style wrappers to stdout). A bare
/// `Operation not permitted` in **stdout** is NOT trusted — it shows up
/// whenever a command merely echoes such text (grep, cat of a log); only
/// the listen/socket creation shapes fire:
/// - node family: `Error: listen EPERM: operation not permitted 0.0.0.0:3001`
/// - go family: `listen tcp :8080: socket: operation not permitted`
/// - python: `socket.socket()` creation → `PermissionError` with a
///   `socket.py` traceback frame
pub(crate) fn stream_smells_net_block(stream: &str) -> bool {
    stream.contains("listen EPERM")
        || (stream.contains("listen tcp") && stream.contains("operation not permitted"))
        || (stream.contains("PermissionError") && stream.contains("socket"))
}

/// Post-hoc failure guidance, mode-aware (P3c design §5.3 — replaces
/// the P3b single write-block line). When a sandboxed command failed
/// and its streams/exit code smell like a sandbox denial (see
/// [`classify_block`]), the tool appends one line so the model knows
/// WHY and what to do. Heuristic, append-only — the command's own
/// output is never rewritten. `None` = no append (宁缺勿滥).
///
/// 2026-09-21 (R7): the network/exec-face-gap variants no longer
/// point at escalation reruns — a byte-identical unsandboxed rerun
/// is structurally useless for long-running processes (dev servers)
/// and for exec-face misses (the rerun hits the same face). The
/// remediation is now「收敛到一条 operator 指令然后停」: one ask,
/// then stop — no multi-round self-diagnosis.
pub(crate) fn failure_guidance(
    stderr: &str,
    stdout: &str,
    exit_code: Option<i32>,
    net: NetEnforcement,
    mode: Mode,
) -> Option<&'static str> {
    let kind = classify_block(stderr, stdout, exit_code, net)?;
    Some(failure_guidance_for_kind(kind, mode))
}

/// Guidance for an ALREADY-classified denial kind (the P3d
/// background-escalation path: the offer carries the kind, so the
/// drain side must not re-classify from the evidence line alone).
pub(crate) fn failure_guidance_for_kind(kind: SandboxBlockKind, mode: Mode) -> &'static str {
    match kind {
        SandboxBlockKind::Write => match mode {
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
        },
        SandboxBlockKind::Network => match mode {
            Mode::Plan => {
                "[sandbox] Network is blocked inside the Plan-mode read-only sandbox — no \
                 outbound connections AND no listen (dev servers cannot start); this is by \
                 design. Ask the user to run the networked command, or switch to Edit mode."
            }
            _ => {
                "[sandbox] The failure above looks like the sandbox blocking network (no INET \
                 sockets inside the sandbox: no outbound, and no listen — dev servers cannot \
                 start). Do NOT burn turns retrying or self-diagnosing: converge to ONE \
                 operator instruction — ask the user to either switch the project's sandbox \
                 network policy (Settings → project → network, e.g. a BindOnly port snapshot \
                 for dev servers) or run the networked command themselves — then stop."
            }
        },
        SandboxBlockKind::ExecFace => {
            "[sandbox] The failure above looks like the sandbox EXEC face missing the \
             program's real location (exit 126 + Permission denied on exec — typically a \
             wrapper script exec'ing a binary outside the allowed exec roots). Retrying or \
             re-routing cannot fix this: converge to ONE operator instruction — ask the user \
             to add the tool's install directory to the sandbox exec allowlist (Settings) or \
             run the command themselves — then stop."
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "tests_sandbox.rs"]
mod tests_sandbox;
