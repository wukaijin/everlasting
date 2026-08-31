//! Sandbox tests. Two layers:
//!
//! - Unit tests (all platforms): ABI constant pins (kernel UAPI
//!   alignment), AccessSet ⊆ handled (spike trap 2 at type level),
//!   BPF golden + logic walk, policy spec construction (both faces),
//!   resolve_policy matrix (P3c design §1), guidance copy (AC7).
//! - Integration tests (`#[cfg(target_os = "linux")]` + live
//!   capability check): real spawns of `sh -c` under the real
//!   ruleset — write allow/deny both faces, exec interop deny, read
//!   unrestricted, network block, AF_UNIX pass (AC1 / AC2). They
//!   skip (with a loud note, not a silent pass-fail) when the
//!   runtime kernel lacks Landlock/seccomp, mirroring the spike's
//!   own matrix harness.

use std::path::{Path, PathBuf};

use super::{Capability, SandboxSpec, DEVICE_WRITE_PATHS};
use crate::db::Mode;

// ---------------------------------------------------------------------------
// ABI constant pins (kernel UAPI alignment — spike trap 1: never trust
// distro headers; these ARE our constants, test-pinned)
// ---------------------------------------------------------------------------

#[test]
fn abi_landlock_access_fs_bits() {
    use super::landlock::bits::*;
    assert_eq!(EXECUTE, 1);
    assert_eq!(WRITE_FILE, 1 << 1);
    assert_eq!(READ_FILE, 1 << 2);
    assert_eq!(READ_DIR, 1 << 3);
    assert_eq!(REMOVE_DIR, 1 << 4);
    assert_eq!(REMOVE_FILE, 1 << 5);
    assert_eq!(MAKE_CHAR, 1 << 6);
    assert_eq!(MAKE_DIR, 1 << 7);
    assert_eq!(MAKE_REG, 1 << 8);
    assert_eq!(MAKE_SOCK, 1 << 9);
    assert_eq!(MAKE_FIFO, 1 << 10);
    assert_eq!(MAKE_BLOCK, 1 << 11);
    assert_eq!(MAKE_SYM, 1 << 12, "ABI v1 tops out at MAKE_SYM (no APPEND)");
}

#[test]
fn abi_handled_mask_is_execute_plus_write_family() {
    use super::landlock::bits::*;
    let expected = EXECUTE
        | WRITE_FILE
        | REMOVE_DIR
        | REMOVE_FILE
        | MAKE_CHAR
        | MAKE_DIR
        | MAKE_REG
        | MAKE_SOCK
        | MAKE_FIFO
        | MAKE_BLOCK
        | MAKE_SYM;
    assert_eq!(super::landlock::HANDLED_ACCESS_FS, expected);
    // Reads are deliberately NOT handled (spec: 控写不控读).
    assert_eq!(super::landlock::HANDLED_ACCESS_FS & READ_FILE, 0);
    assert_eq!(super::landlock::HANDLED_ACCESS_FS & READ_DIR, 0);
}

#[test]
fn abi_prctl_and_seccomp_constants() {
    use super::landlock::{PR_GET_SECCOMP, PR_SET_NO_NEW_PRIVS, PR_SET_SECCOMP};
    assert_eq!(PR_GET_SECCOMP, 21);
    assert_eq!(PR_SET_SECCOMP, 22);
    assert_eq!(PR_SET_NO_NEW_PRIVS, 38);
    assert_eq!(libc::SECCOMP_MODE_FILTER, 2);
    assert_eq!(libc::SECCOMP_RET_ALLOW, 0x7fff_0000);
    assert_eq!(libc::SECCOMP_RET_ERRNO, 0x0005_0000);
    assert_eq!(libc::BPF_LD, 0x00);
    assert_eq!(libc::BPF_W, 0x00);
    assert_eq!(libc::BPF_ABS, 0x20);
    assert_eq!(libc::BPF_JMP, 0x05);
    assert_eq!(libc::BPF_JEQ, 0x10);
    assert_eq!(libc::BPF_RET, 0x06);
    assert_eq!(libc::BPF_K, 0x00);
    // Landlock rule type + probe flag.
    assert_eq!(super::landlock::LANDLOCK_RULE_PATH_BENEATH, 1);
    assert_eq!(super::landlock::LANDLOCK_CREATE_RULESET_VERSION, 1);
}

#[cfg(target_os = "linux")]
#[test]
fn abi_libc_syscall_numbers_match_arch_uapi() {
    // x86_64/aarch64 UAPI: 444/445/446. The point of this test is the
    // alignment of our constants with libc's per-arch values — we
    // always call through libc::SYS_* (never hardcode), so this
    // asserts the documented UAPI numbers to catch a libc regression.
    assert_eq!(libc::SYS_landlock_create_ruleset, 444);
    assert_eq!(libc::SYS_landlock_add_rule, 445);
    assert_eq!(libc::SYS_landlock_restrict_self, 446);
    assert_eq!(libc::SYS_socket, 41);
}

#[test]
fn abi_struct_layouts() {
    // landlock_path_beneath_attr is packed in C (12 bytes, fields @0/@8);
    // landlock_ruleset_attr is one u64. Field OFFSETS are what the
    // kernel reads — assert them via pointer math.
    let ra = super::landlock::RulesetAttr {
        handled_access_fs: 0xdead_beef,
    };
    let ra_ptr = &ra as *const _ as *const u8;
    unsafe {
        assert_eq!(ra_ptr.add(0).cast::<u64>().read(), 0xdead_beef);
        assert_eq!(std::mem::size_of::<super::landlock::RulesetAttr>(), 8);
    }
    let pa = super::landlock::PathBeneathAttr {
        allowed_access: 0x1122_3344_5566_7788,
        parent_fd: 42,
    };
    let pa_ptr = &pa as *const _ as *const u8;
    unsafe {
        assert_eq!(pa_ptr.add(0).cast::<u64>().read(), 0x1122_3344_5566_7788);
        assert_eq!(pa_ptr.add(8).cast::<i32>().read(), 42);
    }
}

// ---------------------------------------------------------------------------
// AccessSet ⊆ handled (C5 / spike trap 2 — type-level guarantee)
// ---------------------------------------------------------------------------

#[test]
fn access_set_constants_are_subsets_of_handled() {
    use super::landlock::AccessSet;
    let handled = super::landlock::HANDLED_ACCESS_FS;
    assert_eq!(AccessSet::EXECUTE.0 & !handled, 0);
    assert_eq!(AccessSet::WRITE_FAMILY.0 & !handled, 0);
    assert_eq!(AccessSet::WRITE_FILE.0 & !handled, 0);
    // Sanity: WRITE_FAMILY really is the write side.
    assert_ne!(
        AccessSet::WRITE_FAMILY.0 & super::landlock::bits::WRITE_FILE,
        0
    );
    assert_ne!(
        AccessSet::WRITE_FAMILY.0 & super::landlock::bits::MAKE_SOCK,
        0
    );
    assert_eq!(
        AccessSet::WRITE_FAMILY.0 & super::landlock::bits::EXECUTE,
        0
    );
}

// ---------------------------------------------------------------------------
// seccomp BPF golden + logic walk
// ---------------------------------------------------------------------------

/// Minimal BPF interpreter: walks the cBPF program with a synthetic
/// seccomp_data (nr + arg0), returns the RET value. Enough to verify
/// the jump topology of the 8-instruction filter.
fn bpf_eval(prog: &[libc::sock_filter], nr: u32, arg0_lo: u32) -> u32 {
    let mut pc = 0usize;
    let mut a: u32 = 0;
    for _ in 0..10_000 {
        let ins = &prog[pc];
        match ins.code {
            0x20 => {
                // LD | W | ABS
                assert_eq!(
                    ins.code,
                    (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16
                );
                a = match ins.k {
                    0 => nr,
                    16 => arg0_lo,
                    other => panic!("unexpected ABS offset {other}"),
                };
                pc += 1;
            }
            0x15 => {
                // JMP | JEQ | K
                if a == ins.k {
                    pc = pc + 1 + ins.jt as usize;
                } else {
                    pc = pc + 1 + ins.jf as usize;
                }
            }
            0x06 => return ins.k, // RET | K
            other => panic!("unexpected BPF code {other:#x}"),
        }
    }
    panic!("BPF program did not terminate");
}

#[test]
fn bpf_golden_layout() {
    let p = super::seccomp::build_inet_block_filter();
    assert_eq!(p.len(), 8);
    // 0: LD nr
    assert_eq!(
        p[0].code,
        (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16
    );
    assert_eq!(p[0].k, 0);
    // 1: JEQ __NR_socket, jt=0, jf=5 (→ ALLOW @7)
    assert_eq!(
        p[1].code,
        (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16
    );
    assert_eq!(p[1].k, libc::SYS_socket as u32);
    assert_eq!((p[1].jt, p[1].jf), (0, 5));
    // 2: LD args[0] lo
    assert_eq!(p[2].k, 16);
    // 3/4: AF_INET / AF_INET6 checks
    assert_eq!(p[3].k, libc::AF_INET as u32);
    assert_eq!((p[3].jt, p[3].jf), (2, 0));
    assert_eq!(p[4].k, libc::AF_INET6 as u32);
    assert_eq!((p[4].jt, p[4].jf), (1, 0));
    // 5: ALLOW, 6: ERRNO|EPERM, 7: ALLOW
    assert_eq!(p[5].k, libc::SECCOMP_RET_ALLOW);
    assert_eq!(p[6].k, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32);
    assert_eq!(p[7].k, libc::SECCOMP_RET_ALLOW);
}

#[test]
fn bpf_logic_walk() {
    let p = super::seccomp::build_inet_block_filter();
    let allow = libc::SECCOMP_RET_ALLOW;
    let eperm = libc::SECCOMP_RET_ERRNO | libc::EPERM as u32;
    // TCPv4 / TCPv6 outbound → EPERM (AC2).
    assert_eq!(
        bpf_eval(&p, libc::SYS_socket as u32, libc::AF_INET as u32),
        eperm
    );
    assert_eq!(
        bpf_eval(&p, libc::SYS_socket as u32, libc::AF_INET6 as u32),
        eperm
    );
    // Low-word compare is EXACTLY kernel semantics: the kernel takes
    // the full low 32 bits as a signed `int` family and range-checks
    // it, so a garbage high word (even positive) never reaches a real
    // AF_INET socket — the filter's ALLOW there is harmless (the
    // syscall fails with EAFNOSUPPORT in the kernel regardless), and
    // low word == AF_INET is the only shape that creates a v4 socket.
    assert_eq!(
        bpf_eval(
            &p,
            libc::SYS_socket as u32,
            libc::AF_INET as u32 | (3 << 28)
        ),
        allow
    );
    // AF_UNIX (pnpm/docker/X11 patterns) passes (AC2).
    assert_eq!(
        bpf_eval(&p, libc::SYS_socket as u32, libc::AF_UNIX as u32),
        allow
    );
    assert_eq!(
        bpf_eval(&p, libc::SYS_socket as u32, libc::AF_NETLINK as u32),
        allow
    );
    // Every non-socket syscall passes (default-allow, no default-deny).
    assert_eq!(bpf_eval(&p, libc::SYS_read as u32, 0), allow);
    assert_eq!(bpf_eval(&p, libc::SYS_openat as u32, 0xdead_beef), allow);
}

// ---------------------------------------------------------------------------
// Policy: spec construction (source iron rule)
// ---------------------------------------------------------------------------

fn policy_ctx(tmp: &tempfile::TempDir) -> crate::tools::ToolContext {
    crate::tools::ToolContext {
        worktree_path: tmp.path().join("worktree"),
        cwd: tmp.path().join("worktree").join("sub"),
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db: crate::tools::test_default_pool(),
        project_id: "p".to_string(),
        data_dir: tmp.path().to_path_buf(),
        workflow_name: None,
        mode: Mode::Edit,
    }
}

#[tokio::test]
async fn spec_roots_follow_server_side_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = policy_ctx(&tmp);
    let spec = super::policy::build_spec(&ctx, Some("sess-1"), vec![], Face::ReadWrite);
    // Writable: worktree (NOT the session cwd subdir — the worktree
    // is the damage-limitation boundary) + /tmp + spill dir.
    assert_eq!(spec.writable_roots.len(), 3);
    assert!(spec.writable_roots.contains(&ctx.worktree_path));
    assert!(spec.writable_roots.contains(&PathBuf::from("/tmp")));
    assert!(spec
        .writable_roots
        .contains(&crate::tools::tool_output::session_outputs_dir(
            &ctx.data_dir,
            "sess-1"
        )));
    // Exec face covers the writable roots + /dev + /tmp + toolchain;
    // NEVER /init or /mnt/c.
    for w in &spec.writable_roots {
        assert!(
            spec.exec_allow_roots.contains(w),
            "{w:?} must be executable"
        );
    }
    assert!(spec.exec_allow_roots.contains(&PathBuf::from("/dev")));
    assert!(!spec
        .exec_allow_roots
        .iter()
        .any(|p| p == Path::new("/init")));
    assert!(!spec
        .exec_allow_roots
        .iter()
        .any(|p| p.starts_with("/mnt/c")));
}

/// P3c design §3: the ReadOnly face moves the worktree OUT of the
/// writable roots (only /tmp + spill + extras stay writable) but
/// keeps it on the EXEC face — project scripts still run.
#[tokio::test]
async fn spec_readonly_face_excludes_worktree_write_keeps_exec() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = policy_ctx(&tmp);
    let spec = super::policy::build_spec(&ctx, Some("sess-1"), vec![], Face::ReadOnly);
    // Writable: /tmp + spill only — no worktree.
    assert_eq!(spec.writable_roots.len(), 2);
    assert!(!spec.writable_roots.contains(&ctx.worktree_path));
    assert!(spec.writable_roots.contains(&PathBuf::from("/tmp")));
    assert!(spec
        .writable_roots
        .contains(&crate::tools::tool_output::session_outputs_dir(
            &ctx.data_dir,
            "sess-1"
        )));
    // Exec face still carries the worktree (explicit push — the
    // writable-roots extend no longer provides it).
    assert!(
        spec.exec_allow_roots.contains(&ctx.worktree_path),
        "readonly face must keep worktree EXECUTE"
    );
    // Face rides the spec for the audit summary.
    assert_eq!(
        spec.summary(),
        format!(
            "landlock:face=ro exec_roots={} writable_roots=2 extra=0 devices={}; seccomp:inet_block",
            spec.exec_allow_roots.len(),
            DEVICE_WRITE_PATHS.len()
        )
    );
    // The ReadWrite face summary says rw (both spawn paths audit the
    // same shape — AC8 face observability).
    let rw = super::policy::build_spec(&ctx, Some("sess-1"), vec![], Face::ReadWrite);
    assert!(rw.summary().contains("face=rw"));
    assert!(rw.summary().contains("writable_roots=3"));
}

#[tokio::test]
async fn spec_merges_extra_writable_without_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = policy_ctx(&tmp);
    let extra = vec![PathBuf::from("/opt/data"), PathBuf::from("/tmp")];
    let spec = super::policy::build_spec(&ctx, Some("s"), extra, Face::ReadWrite);
    assert!(spec.writable_roots.contains(&PathBuf::from("/opt/data")));
    // /tmp already a writable root → not duplicated.
    assert_eq!(
        spec.writable_roots
            .iter()
            .filter(|p| p.as_path() == Path::new("/tmp"))
            .count(),
        1
    );
}

#[tokio::test]
async fn spec_ignores_command_content_by_construction() {
    // CVE-2025-59532 iron rule: the command text (and any tool_input)
    // has no path into build_spec — no parameter exists for it. This
    // test only documents that: same ctx, wildly different commands,
    // identical specs (hash comparison would be flaky via env PATH
    // only on windows; compare directly).
    let tmp = tempfile::tempdir().unwrap();
    let ctx = policy_ctx(&tmp);
    let a = super::policy::build_spec(&ctx, Some("s"), vec![], Face::ReadWrite);
    let b = super::policy::build_spec(&ctx, Some("s"), vec![], Face::ReadWrite);
    assert_eq!(a, b);
}

#[test]
fn device_write_paths_match_spike_recipe() {
    assert_eq!(
        DEVICE_WRITE_PATHS,
        &[
            "/dev/null",
            "/dev/zero",
            "/dev/full",
            "/dev/random",
            "/dev/urandom",
            "/dev/tty"
        ]
    );
}

// ---------------------------------------------------------------------------
// Policy matrix (P3c design §1: capability → Yolo → project off →
// kill-switch → Plan → project face) + DB resolution
// ---------------------------------------------------------------------------

fn cap_ok() -> Capability {
    Capability {
        landlock: true,
        seccomp: true,
    }
}

use super::policy::ProjectSandboxPolicy as PSP;
use super::{Face, Policy};

/// The pure decision matrix, all 24 rows: mode × project tier ×
/// kill-switch × capability. Locks the evaluation order semantics —
/// especially "Plan overrides the project face but not a project
/// opt-out" and "kill-switch beats every face".
#[test]
fn resolve_policy_full_matrix() {
    let modes = [Mode::Edit, Mode::Plan, Mode::Yolo, Mode::Background];
    let tiers = [PSP::Off, PSP::ReadWrite, PSP::ReadOnly];
    for mode in modes {
        for tier in tiers {
            // Row 1: capability fail → Off everywhere (fail-open).
            let broken = Capability {
                landlock: true,
                seccomp: false,
            };
            assert_eq!(super::resolve_policy(mode, tier, true, broken), Policy::Off);
            // Row 2: Yolo → Off everywhere (恒不沙盒).
            if mode == Mode::Yolo {
                assert_eq!(
                    super::resolve_policy(mode, tier, true, cap_ok()),
                    Policy::Off
                );
                continue;
            }
            // Row 3: project off → Off (Tier 4 classic path).
            if tier == PSP::Off {
                assert_eq!(
                    super::resolve_policy(mode, tier, true, cap_ok()),
                    Policy::Off
                );
                assert_eq!(
                    super::resolve_policy(mode, tier, false, cap_ok()),
                    Policy::Off
                );
                continue;
            }
            // Row 4: kill-switch off → Off (global master beats the face).
            assert_eq!(
                super::resolve_policy(mode, tier, false, cap_ok()),
                Policy::Off
            );
            // Rows 5/6: face resolution. Plan overrides the project
            // face with the session-level read-only face (D3);
            // Edit/Background/Yolo-map take the project tier.
            let expected = match mode {
                Mode::Plan => Face::ReadOnly,
                _ => match tier {
                    PSP::ReadWrite => Face::ReadWrite,
                    _ => Face::ReadOnly,
                },
            };
            assert_eq!(
                super::resolve_policy(mode, tier, true, cap_ok()),
                Policy::Face(expected),
                "mode={mode:?} tier={tier:?}"
            );
        }
    }
}

/// Fresh migrated pool + project row with the given tier + session
/// row joined to it. Owns its pool (no shared OnceLock state).
async fn policy_pool(project_id: &str, tier: PSP, session_id: &str) -> sqlx::SqlitePool {
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    crate::db::migrations::run_migrations(&pool).await.unwrap();
    sqlx::query("INSERT INTO projects (id, name, path, created_at, updated_at) VALUES (?, ?, ?, datetime('now'), datetime('now'))")
        .bind(project_id)
        .bind("p")
        .bind("/tmp/p")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE projects SET sandbox_policy = ? WHERE id = ?")
        .bind(tier.as_str())
        .bind(project_id)
        .execute(&pool)
        .await
        .unwrap();
    // project_id carries a DEFAULT ('<DEFAULT_PROJECT_ID>'), so bind
    // explicitly; worktree_path/current_cwd have NOT NULL defaults.
    sqlx::query(
        "INSERT INTO sessions (id, title, created_at, updated_at, model, project_id) \
         VALUES (?, 't', datetime('now'), datetime('now'), '', ?)",
    )
    .bind(session_id)
    .bind(project_id)
    .execute(&pool)
    .await
    .unwrap();
    pool
}

/// DB resolution: a readwrite-tier project resolves Face(ReadWrite)
/// for Edit and Face(ReadOnly) for Plan; the readonly tier maps to
/// the readonly face in both.
#[tokio::test]
async fn resolve_session_policy_follows_project_tier() {
    let pool = policy_pool("proj-rw", PSP::ReadWrite, "sess-rw").await;
    assert_eq!(
        super::resolve_session_policy(&pool, "sess-rw", Mode::Edit).await,
        Policy::Face(Face::ReadWrite)
    );
    assert_eq!(
        super::resolve_session_policy(&pool, "sess-rw", Mode::Plan).await,
        Policy::Face(Face::ReadOnly)
    );

    let pool = policy_pool("proj-ro", PSP::ReadOnly, "sess-ro").await;
    assert_eq!(
        super::resolve_session_policy(&pool, "sess-ro", Mode::Edit).await,
        Policy::Face(Face::ReadOnly)
    );
}

/// DB resolution: project `off` + kill-switch config both resolve
/// Off; the kill-switch read is SKIPPED for off projects (the staged
/// reads — SBX-004).
#[tokio::test]
async fn resolve_session_policy_off_and_kill_switch() {
    let pool = policy_pool("proj-off", PSP::Off, "sess-off").await;
    assert_eq!(
        super::resolve_session_policy(&pool, "sess-off", Mode::Edit).await,
        Policy::Off
    );

    // Kill-switch: only the literal "false" disables (fail-open read).
    let pool = policy_pool("proj-ks", PSP::ReadWrite, "sess-ks").await;
    sqlx::query("INSERT INTO app_config (key, value) VALUES ('sandbox_enabled', 'false')")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        super::resolve_session_policy(&pool, "sess-ks", Mode::Edit).await,
        Policy::Off
    );
}

/// DB resolution fallbacks: unknown session id (no join row) and
/// Yolo both resolve Off without touching anything.
#[tokio::test]
async fn resolve_session_policy_missing_session_and_yolo() {
    let pool = policy_pool("proj-x", PSP::ReadWrite, "sess-x").await;
    assert_eq!(
        super::resolve_session_policy(&pool, "no-such-session", Mode::Edit).await,
        Policy::Off
    );
    assert_eq!(
        super::resolve_session_policy(&pool, "sess-x", Mode::Yolo).await,
        Policy::Off
    );
}

/// decide() end-to-end: readwrite tier sandboxes a SideEffect command
/// (the P3c behavior change — pre-P3c it skipped), off tier keeps the
/// legacy skip, and a None session id skips (no policy to resolve).
#[tokio::test]
async fn decide_sandboxes_all_tiers_under_readwrite() {
    use crate::sandbox::Decision;
    let tmp = tempfile::tempdir().unwrap();
    let mut ctx = policy_ctx(&tmp);
    let pool = policy_pool("proj-d", PSP::ReadWrite, "sess-d").await;
    ctx.db = pool;

    // SideEffect-tier command (pre-P3c: Skip) → now sandboxed.
    let d = super::decide(&ctx, "mkdir x", Some("sess-d")).await;
    assert!(matches!(d, Decision::Sandbox(_)), "got: {d:?}");
    // Ask-tier command → sandboxed too (Tier 4 short-circuits the
    // modal upstream; the spawn side must not re-skip it).
    let d = super::decide(&ctx, "rm x", Some("sess-d")).await;
    assert!(matches!(d, Decision::Sandbox(_)), "got: {d:?}");

    // Off tier → legacy skip.
    let pool = policy_pool("proj-d-off", PSP::Off, "sess-d-off").await;
    ctx.db = pool;
    let d = super::decide(&ctx, "mkdir x", Some("sess-d-off")).await;
    assert!(matches!(d, Decision::Skip { .. }), "got: {d:?}");

    // No session context → skip (cannot resolve a project policy).
    let d = super::decide(&ctx, "ls", None).await;
    assert!(matches!(d, Decision::Skip { .. }), "got: {d:?}");
}

#[test]
fn capability_ok_requires_both() {
    assert!(cap_ok().ok());
    assert!(!Capability {
        landlock: true,
        seccomp: false
    }
    .ok());
    assert!(!Capability {
        landlock: false,
        seccomp: true
    }
    .ok());
}

// ---------------------------------------------------------------------------
// Audit hash + write-block guidance copy (AC7)
// ---------------------------------------------------------------------------

#[test]
fn command_sha_prefix_is_stable_12_hex() {
    let a = super::command_sha_prefix("git status");
    let b = super::command_sha_prefix("git status");
    let c = super::command_sha_prefix("git status --short");
    assert_eq!(a, b);
    assert_ne!(a, c);
    assert_eq!(a.len(), 12);
    assert!(a.chars().all(|ch| ch.is_ascii_hexdigit()));
}

#[test]
fn guidance_fires_on_permission_denied_and_rofs() {
    let text = super::write_block_guidance("touch /etc/foo\nPermission denied").expect("fires");
    // Pinned copy points (design §2.5): what happened + both escape
    // hatches (authorized non-read-only command / config allowlist).
    assert!(text.contains("[sandbox]"));
    assert!(text.contains("sandbox_extra_writable"));
    assert!(text.contains("non-read-only"));
    assert!(text.contains("worktree"));
    assert!(super::write_block_guidance("Read-only file system").is_some());
    // Heuristic must stay quiet on unrelated failures (宁缺勿滥).
    assert!(super::write_block_guidance("command not found").is_none());
    assert!(super::write_block_guidance("fatal: not a git repository").is_none());
    assert!(super::write_block_guidance("").is_none());
}

// ---------------------------------------------------------------------------
// Integration: real spawns under the real ruleset (Linux only)
// ---------------------------------------------------------------------------

/// Capability check + skip macro: integration tests need a live
/// Landlock+seccomp kernel. The skip is loud (eprintln) so CI logs
/// show WHY a matrix row vanished instead of silently shrinking.
#[cfg(target_os = "linux")]
macro_rules! require_sandbox {
    () => {{
        if !super::Capability::probe().ok() {
            eprintln!("SKIP: Landlock/seccomp unavailable on this kernel (fail-open runtime)");
            return;
        }
    }};
}

#[cfg(target_os = "linux")]
fn integration_spec(worktree: &Path) -> SandboxSpec {
    SandboxSpec {
        face: super::Face::ReadWrite,
        writable_roots: vec![worktree.to_path_buf(), PathBuf::from("/tmp")],
        exec_allow_roots: vec![
            PathBuf::from("/usr"),
            PathBuf::from("/bin"),
            PathBuf::from("/lib"),
            PathBuf::from("/lib64"),
            PathBuf::from("/dev"),
            PathBuf::from("/tmp"),
            worktree.to_path_buf(),
        ],
        extra_writable: vec![],
    }
}

/// ReadOnly face spec (P3c design §3): worktree OUT of the writable
/// roots, ON the exec face. Built via `build_spec` so the test pins
/// the real constructor, not a hand-rolled copy.
#[cfg(target_os = "linux")]
fn integration_readonly_spec(ctx: &crate::tools::ToolContext) -> SandboxSpec {
    super::policy::build_spec(ctx, Some("integ-ro"), vec![], super::Face::ReadOnly)
}

/// Spawn `sh -c script` under the sandbox, return (exit, stderr).
#[cfg(target_os = "linux")]
async fn run_sandboxed(spec: &SandboxSpec, script: &str, cwd: &Path) -> (i32, String) {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c").arg(script).current_dir(cwd);
    let prepared = super::prepare(spec).expect("prepare (parent zone)");
    super::apply(&mut cmd, &prepared).expect("apply (register pre_exec)");
    let out = cmd.output().await.expect("child runs");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn integration_write_faces_and_read_freedom() {
    require_sandbox!();
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let spec = integration_spec(&wt);

    // AC1: worktree + /tmp writable…
    let (code, err) = run_sandboxed(&spec, "echo hi > ./out.txt", &wt).await;
    assert_eq!(code, 0, "worktree write must succeed: {err}");
    let (code, err) = run_sandboxed(&spec, "echo hi > /tmp/everlasting_sbx_w.txt", &wt).await;
    assert_eq!(code, 0, "/tmp write must succeed: {err}");
    // …/dev/null (device per-file rule) works…
    let (code, err) = run_sandboxed(&spec, "echo hi > /dev/null", &wt).await;
    assert_eq!(code, 0, "/dev/null write must succeed: {err}");
    // …reads anywhere are unrestricted (incl. redirect to /dev/null)…
    let (code, err) = run_sandboxed(&spec, "cat /etc/passwd > /dev/null", &wt).await;
    assert_eq!(code, 0, "reads must be unrestricted: {err}");
    // …home + /usr/local writes are denied.
    let (code, err) = run_sandboxed(&spec, "echo hi > $HOME/everlasting_sbx_denied.txt", &wt).await;
    assert_ne!(code, 0, "$HOME write must be denied");
    assert!(err.contains("Permission denied"), "got: {err}");
    let (code, err) = run_sandboxed(
        &spec,
        "echo hi > /usr/local/everlasting_sbx_denied.txt",
        &wt,
    )
    .await;
    assert_ne!(code, 0, "/usr/local write must be denied");
    assert!(err.contains("Permission denied"), "got: {err}");
}

/// P3c (design §3, AC2 face semantics): under the ReadOnly face the
/// worktree write is DENIED (Landlock) while executing a project
/// script from the worktree still WORKS (EXECUTE face kept).
///
/// The worktree must live OUTSIDE every writable root for the denial
/// row to carry signal — tempfile hands out `/tmp`-based dirs and
/// `/tmp` stays writable under this face, so the worktree is created
/// under `$HOME` instead (best-effort cleanup; a panic may leave a
/// `.everlasting-sbx-ro-*` dir behind — acceptable for a test).
#[cfg(target_os = "linux")]
#[tokio::test]
async fn integration_readonly_face_blocks_worktree_write_keeps_exec() {
    require_sandbox!();
    let home = match dirs::home_dir() {
        Some(h) if !h.starts_with("/tmp") => h,
        _ => {
            eprintln!("SKIP-row: no usable $HOME outside /tmp for the readonly-face worktree");
            return;
        }
    };
    let base = home.join(format!(".everlasting-sbx-ro-{}", std::process::id()));
    let wt = base.join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let ctx = policy_ctx(&tempfile::tempdir().unwrap());
    // build_spec takes the worktree from ctx — point it at wt. The
    // data_dir (spill root) stays on the discarded tempdir so the
    // spill rule never collides with the home-side worktree.
    let ctx = crate::tools::ToolContext {
        worktree_path: wt.clone(),
        ..ctx
    };
    let spec = integration_readonly_spec(&ctx);
    // The executable itself must be executable; put a script in the
    // worktree and exec it DIRECTLY (`./script.sh` → execve on the
    // worktree file needs the EXECUTE face; `sh ./script.sh` would
    // only read it and prove nothing about exec).
    let script = wt.join("script.sh");
    std::fs::write(&script, "#!/bin/sh\necho ran\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    // Worktree write → denied by the missing writable rule.
    let (code, err) = run_sandboxed(&spec, "echo hi > ./blocked.txt", &wt).await;
    assert_ne!(code, 0, "readonly face must deny worktree writes");
    assert!(err.contains("Permission denied"), "got: {err}");

    // /tmp write (escape hatch) → still allowed.
    let (code, err) = run_sandboxed(&spec, "echo hi > /tmp/everlasting_sbx_ro.txt", &wt).await;
    assert_eq!(code, 0, "/tmp write must survive the readonly face: {err}");

    // Executing a project script from the (read-only) worktree →
    // allowed by the explicit exec push.
    let (code, err) = run_sandboxed(&spec, "./script.sh", &wt).await;
    assert_eq!(
        code, 0,
        "project script exec must survive readonly face: {err}"
    );

    // Best-effort cleanup (the test process is unsandboxed).
    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn integration_interop_exec_denied() {
    require_sandbox!();
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let spec = integration_spec(&wt);

    // AC1: /init (WSL interop entrypoint) — exec must be denied
    // wherever it exists (WSL2). On a plain Linux CI runner /init
    // usually doesn't exist; the matrix row then carries no signal.
    if Path::new("/init").exists() {
        let (code, err) = run_sandboxed(&spec, "exec /init", &wt).await;
        assert_ne!(code, 0, "/init exec must be denied");
        assert!(
            err.contains("Permission denied"),
            "/init exec should fail with EACCES, got: {err}"
        );
    } else {
        eprintln!("SKIP-row: /init does not exist on this host (not WSL2)");
    }

    // AC1: /mnt/c Windows PEs — WSL-only row.
    if Path::new("/mnt/c").is_dir() {
        if Path::new("/mnt/c/Windows/System32/whoami.exe").exists() {
            let (code, err) =
                run_sandboxed(&spec, "exec /mnt/c/Windows/System32/whoami.exe", &wt).await;
            assert_ne!(code, 0, ".exe exec must be denied");
            assert!(err.contains("Permission denied"), "got: {err}");
        } else {
            eprintln!("SKIP-row: no System32/whoami.exe on this host");
        }
    } else {
        eprintln!("SKIP-row: /mnt/c does not exist on this host (not WSL)");
    }
}

#[cfg(target_os = "linux")]
const SOCK_PATH: &str = "/tmp/everlasting_sbx_afunix.sock";

#[cfg(target_os = "linux")]
#[tokio::test]
async fn integration_seccomp_blocks_inet_allows_af_unix() {
    require_sandbox!();
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let spec = integration_spec(&wt);

    // AC2: bash /dev/tcp → EPERM at socket() (bash prints
    // strerror(EPERM) = "Operation not permitted"; a bare refused
    // connect would say "Connection refused" — the filter fires
    // BEFORE any connection attempt).
    let has_bash = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("command -v bash")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if has_bash {
        let mut cmd = tokio::process::Command::new("bash");
        cmd.arg("-c")
            .arg("echo > /dev/tcp/127.0.0.1/9")
            .current_dir(&wt);
        let prepared = super::prepare(&spec).expect("prepare");
        super::apply(&mut cmd, &prepared).expect("apply");
        let out = cmd.output().await.expect("child runs");
        let err = String::from_utf8_lossy(&out.stderr);
        assert_ne!(out.status.code().unwrap_or(-1), 0, "network must fail");
        assert!(
            err.contains("Operation not permitted") && !err.contains("Connection refused"),
            "socket() must EPERM (not a refused connect), got: {err}"
        );
    } else {
        eprintln!("SKIP-row: bash unavailable (cannot exercise /dev/tcp)");
    }

    // AC2: AF_UNIX keeps working — bind a unix socket in /tmp (also
    // exercises MAKE_SOCK in the writable face). Prefer python3,
    // fall back to perl; skip the row when neither exists.
    let probe = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("command -v python3 || command -v perl")
        .output()
        .await
        .expect("probe");
    let helper = String::from_utf8_lossy(&probe.stdout).trim().to_string();
    let script = match helper.rsplit('/').next().unwrap_or("") {
        "python3" => format!(
            // rm -f first: bind() on an existing socket path is
            // EADDRINUSE, and a previous run may have left the file.
            "rm -f {sock} && python3 -c 'import socket; \
             s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); \
             s.bind(\"{sock}\"); s.close()'",
            sock = SOCK_PATH
        ),
        "perl" => format!(
            "rm -f {sock} && perl -e 'use Socket; socket(S, PF_UNIX, SOCK_STREAM, 0) or die $!; \
             bind(S, sockaddr_un(\"{sock}\")) or die $!;'",
            sock = SOCK_PATH
        ),
        _ => {
            eprintln!("SKIP-row: neither python3 nor perl available");
            return;
        }
    };
    let (code, err) = run_sandboxed(&spec, &script, &wt).await;
    assert_eq!(code, 0, "AF_UNIX bind must succeed: {err}");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn integration_plain_git_works_in_worktree() {
    require_sandbox!();
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let spec = integration_spec(&wt);
    // git reads its config + writes nothing: the classic read-only
    // session workload must be untouched (spike matrix row 3 analog
    // — /dev/null device rule is what git's diff plumbing needs).
    let has_git = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("command -v git")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !has_git {
        eprintln!("SKIP-row: git unavailable");
        return;
    }
    let (code, err) = run_sandboxed(&spec, "git init -q . && git status --porcelain", &wt).await;
    assert_eq!(code, 0, "git in worktree must work: {err}");
}
