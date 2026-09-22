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
    // landlock_ruleset_attr is two u64 (fs @0, net @8 — ABI v4 shape).
    // Field OFFSETS are what the kernel reads — assert them via pointer
    // math.
    let ra = super::landlock::RulesetAttr {
        handled_access_fs: 0xdead_beef,
        handled_access_net: 0x0bad_f00d,
    };
    let ra_ptr = &ra as *const _ as *const u8;
    unsafe {
        assert_eq!(ra_ptr.add(0).cast::<u64>().read(), 0xdead_beef);
        assert_eq!(ra_ptr.add(8).cast::<u64>().read(), 0x0bad_f00d);
        assert_eq!(std::mem::size_of::<super::landlock::RulesetAttr>(), 16);
        assert_eq!(super::landlock::RulesetAttr::FS_ONLY_SIZE, 8);
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
        tool_use_id: None,
        escalation: Default::default(),
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
    let spec = super::policy::build_spec(
        &ctx,
        Some("sess-1"),
        vec![],
        Face::ReadWrite,
        super::policy::NetPolicy::Block,
    );
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
    let spec = super::policy::build_spec(
        &ctx,
        Some("sess-1"),
        vec![],
        Face::ReadOnly,
        super::policy::NetPolicy::Block,
    );
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
    // Face rides the spec for the audit summary. 2026-09-21: the
    // additive `net=` segment (R8) precedes the enforcer segment —
    // for the Block tier the seccomp marker is unchanged; F1 appends
    // the canonical exec-root list (variable-length → prefix-pinned
    // rather than full-equality).
    let summary = spec.summary();
    assert!(
        summary.starts_with(&format!(
            "landlock:face=ro exec_roots={} writable_roots=2 extra=0 devices={}; net=block; seccomp:inet_block; exec_roots_canonical=[",
            spec.exec_allow_roots.len(),
            DEVICE_WRITE_PATHS.len()
        )),
        "{summary}"
    );
    // The canonical list carries real directories (F1/AC5) — the
    // worktree is in the face and appears canonically.
    assert!(
        summary.contains(&ctx.worktree_path.display().to_string()),
        "worktree must appear in the canonical exec list: {summary}"
    );
    // The ReadWrite face summary says rw (both spawn paths audit the
    // same shape — AC8 face observability).
    let rw = super::policy::build_spec(
        &ctx,
        Some("sess-1"),
        vec![],
        Face::ReadWrite,
        super::policy::NetPolicy::Block,
    );
    assert!(rw.summary().contains("face=rw"));
    assert!(rw.summary().contains("writable_roots=3"));
}

#[tokio::test]
async fn spec_merges_extra_writable_without_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = policy_ctx(&tmp);
    let extra = vec![PathBuf::from("/opt/data"), PathBuf::from("/tmp")];
    let spec = super::policy::build_spec(
        &ctx,
        Some("s"),
        extra,
        Face::ReadWrite,
        super::policy::NetPolicy::Block,
    );
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
    let a = super::policy::build_spec(
        &ctx,
        Some("s"),
        vec![],
        Face::ReadWrite,
        super::policy::NetPolicy::Block,
    );
    let b = super::policy::build_spec(
        &ctx,
        Some("s"),
        vec![],
        Face::ReadWrite,
        super::policy::NetPolicy::Block,
    );
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
        landlock_net: true,
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
                landlock_net: false,
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

// ---------------------------------------------------------------------------
// NetPolicy (09-21-sandbox-net-bindonly, R1/R4 — parse / derivation /
// snapshot-keyed effective read)
// ---------------------------------------------------------------------------

use super::policy::{BindSet, NetPolicy};

/// AC2: the three serialized shapes parse and roundtrip through
/// `as_str` (bind_only emits ports sorted — BTreeSet order).
#[test]
fn net_policy_parse_three_shapes_roundtrip() {
    assert_eq!(NetPolicy::parse("block"), Some(NetPolicy::Block));
    assert_eq!(NetPolicy::parse("allow_all"), Some(NetPolicy::AllowAll));
    let bind = NetPolicy::parse("bind_only:3001,3000").unwrap();
    match &bind {
        NetPolicy::BindOnly(set) => {
            assert_eq!(
                set.ports().iter().copied().collect::<Vec<_>>(),
                vec![3000, 3001]
            );
        }
        other => panic!("expected BindOnly, got {other:?}"),
    }
    assert_eq!(bind.as_str(), "bind_only:3000,3001");
    assert_eq!(NetPolicy::Block.as_str(), "block");
    assert_eq!(NetPolicy::AllowAll.as_str(), "allow_all");
}

/// AC2 / R1 fail-closed: every malformed shape → None (callers
/// degrade to Block + warn). Bare `bind_only` without ports is
/// deliberately invalid — an empty snapshot authorizes nothing.
#[test]
fn net_policy_parse_fail_closed() {
    let bad = [
        "",
        "block ",
        "BLOCK",
        "bind_only",       // no ports segment
        "bind_only:",      // empty ports
        "bind_only:0",     // port 0
        "bind_only:65536", // out of u16
        "bind_only:-1",
        "bind_only:80,,443",
        "bind_only:80, 443",
        "bind_only:80;443",
        "Bind_Only:80",
        "readwrite", // file-tier value must not parse as net tier
        "off",
    ];
    for s in bad {
        assert_eq!(NetPolicy::parse(s), None, "must fail closed: {s:?}");
    }
}

/// Sanity cap: > MAX_BIND_PORTS ports → None (runaway list guard).
#[test]
fn net_policy_port_count_cap() {
    let over: Vec<String> = (1..=33).map(|p| p.to_string()).collect();
    assert_eq!(
        NetPolicy::parse(&format!("bind_only:{}", over.join(","))),
        None
    );
    let at: Vec<String> = (1..=32).map(|p| p.to_string()).collect();
    assert!(NetPolicy::parse(&format!("bind_only:{}", at.join(","))).is_some());
}

/// AC2: connect derived set = {80,443} ∪ bind minus daemon ports;
/// bind clamps the same way. 7456 must never be reachable.
#[test]
fn net_connect_derived_set_and_daemon_clamp() {
    let set = BindSet::from_iter_ports([7456, 3000, 3001]);
    let connect = NetPolicy::connect_ports(&set);
    assert!(connect.contains(&80) && connect.contains(&443));
    assert!(connect.contains(&3000) && connect.contains(&3001));
    assert!(!connect.contains(&7456), "daemon port must be clamped out");
    let bind = NetPolicy::bind_ports_clamped(&set);
    assert_eq!(bind.iter().copied().collect::<Vec<_>>(), vec![3000, 3001]);
}

/// R4 snapshot-keyed effective read (AC2: 快照键含 worktree 路径,
/// snapshot ports WIN over the column, missing/malformed → Block):
/// 1. column=bind_only + no snapshot row → Block;
/// 2. snapshot row keyed at a DIFFERENT worktree → Block (branch /
///    re-checkout isolation);
/// 3. snapshot row at THIS worktree with different ports →
///    BindOnly(snapshot ports), snapshot is the authorization truth;
/// 4. malformed snapshot ports → Block;
/// 5. column=allow_all passes through without any snapshot;
/// 6. NULL column → Block; missing session → Block.
#[tokio::test]
async fn net_effective_policy_snapshot_keyed() {
    let pool = policy_pool("proj-net", PSP::ReadWrite, "sess-net").await;
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();

    sqlx::query("UPDATE projects SET sandbox_net = 'bind_only:3001' WHERE id = 'proj-net'")
        .execute(&pool)
        .await
        .unwrap();

    // 1. No snapshot → Block.
    assert_eq!(
        super::policy::read_effective_net_policy(&pool, "sess-net", &wt).await,
        NetPolicy::Block
    );

    // 2. Snapshot for a different worktree key → still Block.
    sqlx::query(
        "INSERT INTO project_net_snapshots (project_id, worktree_key, ports, confirmed_by, confirmed_at)          VALUES ('proj-net', '/other/worktree', '9999', 'op', 0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        super::policy::read_effective_net_policy(&pool, "sess-net", &wt).await,
        NetPolicy::Block
    );

    // 3. Snapshot at THIS worktree (canonicalized key), ports differ
    //    from the column → snapshot wins.
    let key = super::policy::worktree_key(&wt);
    sqlx::query(
        "INSERT INTO project_net_snapshots (project_id, worktree_key, ports, confirmed_by, confirmed_at)          VALUES ('proj-net', ?, '3000,3001', 'op', 0)",
    )
    .bind(&key)
    .execute(&pool)
    .await
    .unwrap();
    let eff = super::policy::read_effective_net_policy(&pool, "sess-net", &wt).await;
    match &eff {
        NetPolicy::BindOnly(set) => {
            assert_eq!(
                set.ports().iter().copied().collect::<Vec<_>>(),
                vec![3000, 3001]
            );
        }
        other => panic!("expected BindOnly from snapshot, got {other:?}"),
    }

    // 4. Malformed snapshot ports → Block (fail-closed, no partial).
    sqlx::query("UPDATE project_net_snapshots SET ports = 'oops' WHERE project_id = 'proj-net' AND worktree_key = ?")
        .bind(&key)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        super::policy::read_effective_net_policy(&pool, "sess-net", &wt).await,
        NetPolicy::Block
    );

    // 5. allow_all passes through without consulting snapshots.
    sqlx::query("UPDATE projects SET sandbox_net = 'allow_all' WHERE id = 'proj-net'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        super::policy::read_effective_net_policy(&pool, "sess-net", &wt).await,
        NetPolicy::AllowAll
    );

    // 6. NULL column → Block; unknown session → Block.
    sqlx::query("UPDATE projects SET sandbox_net = NULL WHERE id = 'proj-net'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        super::policy::read_effective_net_policy(&pool, "sess-net", &wt).await,
        NetPolicy::Block
    );
    assert_eq!(
        super::policy::read_effective_net_policy(&pool, "no-such-session", &wt).await,
        NetPolicy::Block
    );
}

/// AC1: the default spec (no session net column) carries Block — the
/// incumbent semantics — so the spec plumbing itself cannot change
/// Block-tier behavior.
#[tokio::test]
async fn net_default_spec_carries_block() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = policy_ctx(&tmp);
    let spec = super::policy::build_spec(
        &ctx,
        Some("sess-net-default"),
        vec![],
        Face::ReadWrite,
        NetPolicy::Block,
    );
    assert_eq!(spec.net, NetPolicy::Block);
}

// ---------------------------------------------------------------------------
// NetPolicy enforcement (09-21-sandbox-net-bindonly, R2/R3 — ABI v4
// constants, three-state assembly, summary, degrade path)
// ---------------------------------------------------------------------------

/// Trap 1 discipline for the ABI v4 net constants: pinned against
/// the kernel UAPI (`linux/landlock.h`): rule type 2, bits 1<<13 /
/// 1<<14, struct layouts by size/offset.
#[test]
fn abi_net_constants() {
    use super::landlock::{
        net_bits, NetPortAttr, RulesetAttr, HANDLED_ACCESS_NET, LANDLOCK_RULE_NET_PORT,
    };
    assert_eq!(LANDLOCK_RULE_NET_PORT, 2);
    assert_eq!(net_bits::BIND_TCP, 1 << 13);
    assert_eq!(net_bits::CONNECT_TCP, 1 << 14);
    assert_eq!(HANDLED_ACCESS_NET, (1 << 13) | (1 << 14));
    // RulesetAttr: 16 bytes (u64 fs + u64 net), FS_ONLY_SIZE = 8.
    assert_eq!(std::mem::size_of::<RulesetAttr>(), 16);
    assert_eq!(RulesetAttr::FS_ONLY_SIZE, 8);
    // NetPortAttr: u64 allowed_access + u16 port, padded to 16, port
    // at offset 8 (repr(C) matches the C layout the kernel reads).
    assert_eq!(std::mem::size_of::<NetPortAttr>(), 16);
    let attr = NetPortAttr {
        allowed_access: 5,
        port: 3001,
    };
    let bytes = unsafe { std::slice::from_raw_parts(&attr as *const NetPortAttr as *const u8, 16) };
    assert_eq!(u64::from_le_bytes(bytes[0..8].try_into().unwrap()), 5);
    assert_eq!(u16::from_le_bytes(bytes[8..10].try_into().unwrap()), 3001);
}

/// Trap 2 discipline, net side: both NetAccessSet constants are
/// strict subsets of HANDLED_ACCESS_NET (no raw constructor exists).
#[test]
fn net_access_set_subsets_of_handled() {
    use super::landlock::{NetAccessSet, HANDLED_ACCESS_NET};
    assert_eq!(NetAccessSet::BIND_TCP.bits(), 1 << 13);
    assert_eq!(NetAccessSet::CONNECT_TCP.bits(), 1 << 14);
    assert_eq!(HANDLED_ACCESS_NET & NetAccessSet::BIND_TCP.bits(), 1 << 13);
    assert_eq!(
        HANDLED_ACCESS_NET & NetAccessSet::CONNECT_TCP.bits(),
        1 << 14
    );
}

/// R2/AC2: prepare() assembles exactly one net enforcer per tier —
/// and only ONE kind of net artifact ever coexists with the file
/// rules. On kernels without ABI v4 (this WSL2 6.6 = ABI 3) the
/// BindOnly tier degrades to the Block variant at prepare() entry
/// (R3) — asserted live here; on ABI ≥4 kernels the real attr arrays
/// are asserted instead. Both branches deterministic per machine.
#[cfg(target_os = "linux")]
#[test]
fn prepared_net_three_states_and_degrade() {
    let cap = Capability::probe();
    if !cap.ok() {
        eprintln!("SKIP: Landlock/seccomp unavailable on this kernel");
        return;
    }
    let wt = std::env::temp_dir();
    let spec_for = |net: super::policy::NetPolicy| super::SandboxSpec {
        face: super::Face::ReadWrite,
        net,
        writable_roots: vec![wt.clone()],
        exec_allow_roots: vec!["/usr".into(), "/bin".into(), "/lib".into(), "/lib64".into()],
        extra_writable: vec![],
    };

    // Block: the incumbent 8-instruction program inside the variant.
    let p = super::prepare(&spec_for(super::policy::NetPolicy::Block)).unwrap();
    match &p.data.net {
        super::PreparedNet::Block(bpf) => assert_eq!(bpf.len(), 8, "incumbent filter shape"),
        other => panic!("Block tier must carry the seccomp variant, got {other:?}"),
    }

    // AllowAll: unit variant, no enforcer.
    let p = super::prepare(&spec_for(super::policy::NetPolicy::AllowAll)).unwrap();
    assert!(matches!(p.data.net, super::PreparedNet::AllowAll));

    // BindOnly: degrade or real attrs, per kernel capability.
    let set = super::policy::BindSet::from_iter_ports([3000, 3001, 7456]);
    let p = super::prepare(&spec_for(super::policy::NetPolicy::BindOnly(set))).unwrap();
    if cap.landlock_net {
        match &p.data.net {
            super::PreparedNet::BindOnly { bind, connect } => {
                let bind_ports: Vec<u16> = bind.iter().map(|a| a.port).collect();
                assert_eq!(bind_ports, vec![3000, 3001], "7456 clamped out of bind");
                let connect_ports: Vec<u16> = connect.iter().map(|a| a.port).collect();
                assert_eq!(
                    connect_ports,
                    vec![80, 443, 3000, 3001],
                    "derived {{80,443}}∪bind minus daemon ports"
                );
                for a in bind.iter() {
                    assert_eq!(a.allowed_access, 1 << 13);
                }
                for a in connect.iter() {
                    assert_eq!(a.allowed_access, 1 << 14);
                }
            }
            other => panic!("BindOnly on ABI≥4 must carry net attrs, got {other:?}"),
        }
    } else {
        match &p.data.net {
            super::PreparedNet::Block(bpf) => assert_eq!(bpf.len(), 8, "degraded to Block filter"),
            other => panic!("kernel without ABI v4 must degrade to Block, got {other:?}"),
        }
    }
}

/// R8/AC7: summary() net segment — three tiers + the degrade suffix.
/// The degraded form is asserted on kernels without ABI v4, the
/// plain form on kernels with it (both deterministic per machine).
#[test]
fn summary_net_segment_three_states() {
    let spec_for = |net: super::policy::NetPolicy| super::SandboxSpec {
        face: super::Face::ReadWrite,
        net,
        writable_roots: vec![],
        exec_allow_roots: vec![],
        extra_writable: vec![],
    };
    let block = spec_for(super::policy::NetPolicy::Block).summary();
    assert!(block.contains("net=block"), "{block}");
    assert!(block.contains("seccomp:inet_block"), "{block}");
    let allow = spec_for(super::policy::NetPolicy::AllowAll).summary();
    assert!(allow.contains("net=allow_all"), "{allow}");
    assert!(!allow.contains("seccomp:inet_block"), "{allow}");

    let set = super::policy::BindSet::from_iter_ports([3000, 3001]);
    let bind = spec_for(super::policy::NetPolicy::BindOnly(set)).summary();
    if cfg!(target_os = "linux") && !Capability::probe().landlock_net {
        assert!(
            bind.contains("net=bind_only(3000,3001)->block(degraded)"),
            "{bind}"
        );
        assert!(bind.contains("seccomp:inet_block"), "{bind}");
    } else {
        assert!(bind.contains("net=bind_only(3000,3001)"), "{bind}");
        assert!(bind.contains("landlock_net:bind_connect"), "{bind}");
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

/// Durable prefix-grant exemption (09-21-durable-prefix-grant,
/// consumer A): a Face tier + a matching `project_shell_grants` row
/// → `Skip` with the durable reason = unsandboxed START (no failed
/// first attempt); a different prefix / a foreign worktree key stays
/// sandboxed; Plan NEVER exempts; Face(ReadOnly) under Edit mode is
/// deliberately bypassed by an operator approval (PRD AC anchor —
/// approval outranks the project face default).
#[tokio::test]
async fn decide_durable_grant_hit_skips_sandbox() {
    use crate::sandbox::Decision;
    let tmp = tempfile::tempdir().unwrap();
    let mut ctx = policy_ctx(&tmp);
    let pool = policy_pool("proj-grant", PSP::ReadWrite, "sess-grant").await;
    let key = super::policy::worktree_key(&ctx.worktree_path);
    sqlx::query(
        "INSERT INTO project_shell_grants (project_id, worktree_key, prefix_tokens, tool_name) \
         VALUES ('proj-grant', ?, 'pnpm --filter @jjh/web dev', 'shell')",
    )
    .bind(&key)
    .execute(&pool)
    .await
    .unwrap();
    ctx.db = pool;

    // Flag-extending invocation still hits the prefix → unsandboxed.
    let d = super::decide(
        &ctx,
        "pnpm --filter @jjh/web dev --port 3000",
        Some("sess-grant"),
    )
    .await;
    assert!(
        matches!(
            d,
            Decision::Skip {
                reason: super::DURABLE_GRANT_SKIP_REASON
            }
        ),
        "got: {d:?}"
    );
    // Compound (newline form) NEVER hits, even with a matching prefix.
    let d = super::decide(
        &ctx,
        "pnpm --filter @jjh/web dev\ndevil",
        Some("sess-grant"),
    )
    .await;
    assert!(matches!(d, Decision::Sandbox(_)), "got: {d:?}");
    // Different prefix → sandboxed (that is the whole point: `pnpm
    // install` stays under the sandbox).
    let d = super::decide(&ctx, "pnpm install", Some("sess-grant")).await;
    assert!(matches!(d, Decision::Sandbox(_)), "got: {d:?}");

    // Plan NEVER exempts (D3: Plan's value is the deterministic
    // read-only face) — same command that just skipped.
    ctx.mode = Mode::Plan;
    let d = super::decide(&ctx, "pnpm --filter @jjh/web dev", Some("sess-grant")).await;
    assert!(matches!(d, Decision::Sandbox(_)), "got: {d:?}");
    ctx.mode = Mode::Edit;

    // Foreign worktree key (isolated-worker semantics) → miss.
    let pool_iso = policy_pool("proj-iso", PSP::ReadWrite, "sess-iso").await;
    sqlx::query(
        "INSERT INTO project_shell_grants (project_id, worktree_key, prefix_tokens, tool_name) \
         VALUES ('proj-iso', '/elsewhere-entirely', 'pnpm --filter @jjh/web dev', 'shell')",
    )
    .execute(&pool_iso)
    .await
    .unwrap();
    ctx.db = pool_iso;
    let d = super::decide(&ctx, "pnpm --filter @jjh/web dev", Some("sess-iso")).await;
    assert!(matches!(d, Decision::Sandbox(_)), "got: {d:?}");

    // Face(ReadOnly) + Edit: an explicit operator approval bypasses
    // the readonly face — INTENTIONAL semantics (approval outranks
    // the project face default), pinned here against accidental
    // "fixes".
    let pool_ro = policy_pool("proj-grant-ro", PSP::ReadOnly, "sess-grant-ro").await;
    let key_ro = super::policy::worktree_key(&ctx.worktree_path);
    sqlx::query(
        "INSERT INTO project_shell_grants (project_id, worktree_key, prefix_tokens, tool_name) \
         VALUES ('proj-grant-ro', ?, 'pnpm --filter @jjh/web dev', 'shell')",
    )
    .bind(&key_ro)
    .execute(&pool_ro)
    .await
    .unwrap();
    ctx.db = pool_ro;
    let d = super::decide(&ctx, "pnpm --filter @jjh/web dev", Some("sess-grant-ro")).await;
    assert!(
        matches!(
            d,
            Decision::Skip {
                reason: super::DURABLE_GRANT_SKIP_REASON
            }
        ),
        "got: {d:?}"
    );
}

#[test]
fn capability_ok_requires_both() {
    assert!(cap_ok().ok());
    assert!(!Capability {
        landlock: true,
        landlock_net: false,
        seccomp: false
    }
    .ok());
    assert!(!Capability {
        landlock: false,
        landlock_net: false,
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

// New-signature shorthands for guidance/classify tests: the Block
// tier's default conjunction inputs (exit None + INET filter).
fn fg(stderr: &str, stdout: &str, mode: Mode) -> Option<&'static str> {
    super::failure_guidance(stderr, stdout, None, super::NetEnforcement::InetBlock, mode)
}

fn cb(stderr: &str, stdout: &str) -> Option<super::SandboxBlockKind> {
    super::classify_block(stderr, stdout, None, super::NetEnforcement::InetBlock)
}

#[test]
fn guidance_edit_write_variant_pins_copy_points() {
    let text = fg("touch /etc/foo\nPermission denied", "", Mode::Edit).expect("fires");
    // Pinned copy points (design §5.3): what happened + escalation
    // card + both config escape hatches.
    assert!(text.contains("[sandbox]"));
    assert!(text.contains("sandbox_extra_writable"));
    assert!(text.contains("escalation"));
    assert!(text.contains("worktree"));
    assert!(fg("Read-only file system", "", Mode::Edit).is_some());
    // Heuristic must stay quiet on unrelated failures (宁缺勿滥).
    assert!(fg("command not found", "", Mode::Edit).is_none());
    assert!(fg("fatal: not a git repository", "", Mode::Edit).is_none());
    assert!(fg("", "", Mode::Edit).is_none());
}

/// P3c design §5.3 (D3): the Plan variant is mode-aware — by-design
/// wording, diff proposal, /tmp escape hatch, and an EXPLICIT
/// no-card statement (Plan has no escalation exit).
#[test]
fn guidance_plan_write_variant_is_mode_aware() {
    let text = fg("Permission denied", "", Mode::Plan).expect("fires");
    assert!(text.contains("Plan"));
    assert!(text.contains("by design"));
    assert!(text.contains("diff"));
    assert!(text.contains("Edit mode"));
    assert!(text.contains("/tmp"));
    assert!(text.contains("no approval card"));
    assert!(!text.contains("sandbox_extra_writable"));
}

/// P3c design §5.3: the network feature (`Operation not permitted`,
/// seccomp EPERM at socket()) gets its own guidance — never mixed
/// into the write text. Edit names the escalation path; Plan states
/// the design intent.
#[test]
fn guidance_network_variant_separate_from_write() {
    let edit =
        fg("bash: /dev/tcp: Operation not permitted", "", Mode::Edit).expect("network fires");
    assert!(edit.contains("network"));
    // 2026-09-21 R7: the network variant no longer points at the
    // escalation-rerun path — the remediation is converge-then-stop.
    assert!(edit.contains("ONE"));
    assert!(edit.contains("operator instruction"));
    assert!(edit.contains("then stop"));
    let plan = fg("Operation not permitted", "", Mode::Plan).expect("plan network fires");
    assert!(plan.contains("Plan"));
    assert!(plan.contains("by design"));
    // Write strings must NOT route to the network text and vice versa.
    let write = fg("Permission denied", "", Mode::Edit).unwrap();
    assert!(!write.contains("network"));
    assert!(!edit.contains("Permission denied"));
}

/// 2026-09-21 临时修复:listen 类网络拦截报在 stdout(stderr 全空)。
/// 三个实证形态必须命中 Network;宁缺勿滥锚:stdout 里的裸
/// "Operation not permitted" / "Permission denied"(grep、cat 日志的
/// 常见内容)不触发;stderr 的 Write 特征仍优先于 stdout 特征。
#[test]
fn classify_block_reads_stdout_for_listen_denials() {
    use super::SandboxBlockKind;
    let net = |kind: Option<SandboxBlockKind>| matches!(kind, Some(SandboxBlockKind::Network));
    // vite / node family (DB 实证 jjh-mono 23a8184b):
    let vite = "error when starting dev server:\nError: listen EPERM: operation not permitted 0.0.0.0:3001";
    assert!(net(cb("", vite)));
    // go family:
    assert!(net(cb(
        "",
        "listen tcp :8080: socket: operation not permitted"
    )));
    // python: socket() creation denied (traceback carries socket.py):
    let py = "  File \"/usr/lib/python3.10/socket.py\", line 232\nPermissionError: [Errno 1] Operation not permitted";
    assert!(net(cb("", py)));

    // 宁缺勿滥: stdout 里裸的拒绝字符串不认 —— 它们太常作为
    // 普通输出出现(日志、grep 结果)。Write 识别保持 stderr-only。
    assert!(cb("", "grep: Operation not permitted").is_none());
    assert!(cb("", "cat: Permission denied").is_none());
    // stderr 特征优先: stderr 是写拒绝时,即使 stdout 带 listen
    // 噪声也归 Write。
    let kind = cb("mv: Permission denied", vite);
    assert!(matches!(kind, Some(SandboxBlockKind::Write)));
}

/// 2026-09-22(09-21-durable-prefix-grant live E2E 实证):裸 node 脚本
/// dev server 的 listen EPERM 打在 **stderr** 且 libuv 的 errno 文案是
/// 小写(`operation not permitted`)——旧分类只认 stderr 大写 O 字面量
/// + 只喂 stdout 强特征,整条升级链哑火(无卡、无指引)。三条修复锚:
/// stderr errno 字面量大小写不敏感 / listen 强特征两流都喂 / 宁缺勿滥
/// 方向不变(stdout 裸字面量依旧不认)。
#[test]
fn classify_block_reads_stderr_for_listen_denials() {
    use super::SandboxBlockKind;
    let net = |kind: Option<SandboxBlockKind>| matches!(kind, Some(SandboxBlockKind::Network));
    // 裸 node 崩溃形态(live E2E session 9883f423 逐字节实证,含
    // 小写 "operation not permitted" + "listen EPERM" 双锚):
    let node_crash = "node:events:487\n      throw er; // Unhandled 'error' event\n      ^\n\nError: listen EPERM: operation not permitted 0.0.0.0:3987\n    at Server.setupListenHandle [as _listen2] (node:net:1986:21)\n\nNode.js v24.15.0";
    assert!(
        net(cb(node_crash, "")),
        "raw-node stderr crash must classify Network"
    );
    // 无 listen 形状、仅小写 errno 字面量的 stderr(libuv/go strerror
    // 形态,如 curl/openssl 外联被拒)同样命中:
    assert!(net(cb("curl: (7) operation not permitted", "")));
    // 大写 O(libc strerror)形态回归锚 —— 修复不得丢:
    assert!(net(cb("some-tool: Operation not permitted", "")));
    // 宁缺勿滥方向不变:stdout 里裸的小写字面量依旧不认:
    assert!(cb("", "grep: operation not permitted").is_none());
}

/// 2026-09-21:listen 场景的 guidance 变体要点破「无 listen,dev
/// server 起不来」——原文案只讲 outbound,会诱导模型去改绑
/// 127.0.0.1(jjh-mono session 实证过的无效尝试)。
#[test]
fn guidance_network_variant_names_listen() {
    let edit = fg(
        "",
        "Error: listen EPERM: operation not permitted 0.0.0.0:3001",
        Mode::Edit,
    )
    .expect("stdout listen fires");
    assert!(edit.contains("listen"));
    // R7: converge-then-stop remediation (rerun pointers removed).
    assert!(edit.contains("operator instruction"));
    let plan = fg("", "Error: listen EPERM", Mode::Plan).expect("plan stdout listen fires");
    assert!(plan.contains("Plan"));
    assert!(plan.contains("listen"));
}

/// F3 (AC6, 2026-09-21): exit 126 ∧ stderr "Permission denied" →
/// ExecFace (the exec-face-miss class). The exit code is the strong
/// signal — non-126 Permission denied stays a Write classification,
/// and stdout-only strings never fire the exec class (宁缺勿滥).
#[test]
fn classify_exec_face_exit_126() {
    use super::SandboxBlockKind;
    let pnpm126 = "/root/.local/share/pnpm/bin/pnpm: 39: exec: /root/.local/share/pnpm/bin/../global/v11/x/@pnpm/exe/pnpm: Permission denied";
    assert!(matches!(
        cb_stderr_exit(pnpm126, "", Some(126)),
        Some(SandboxBlockKind::ExecFace)
    ));
    // Non-126 with the same stderr = fs-write denial, not exec.
    assert!(matches!(
        cb_stderr_exit(pnpm126, "", Some(1)),
        Some(SandboxBlockKind::Write)
    ));
    // 126 without the Permission denied string is nothing.
    assert!(cb_stderr_exit("command not found", "", Some(126)).is_none());
    // stdout-only Permission denied + 126 does NOT fire (write/exec
    // classification stays stderr-only).
    assert!(cb_stderr_exit("", "cat: Permission denied", Some(126)).is_none());
}

fn cb_stderr_exit(
    stderr: &str,
    stdout: &str,
    exit: Option<i32>,
) -> Option<super::SandboxBlockKind> {
    super::classify_block(stderr, stdout, exit, super::NetEnforcement::InetBlock)
}

/// R9 (AC6): network attribution requires the INET filter to have
/// actually been installed (server-side conjunction). Under
/// LandlockNet (BindOnly) a `listen EPERM` string is NOT a sandbox
/// INET block — it must have another cause, so no classification.
#[test]
fn classify_network_requires_inet_block_enforcement() {
    let vite = "Error: listen EPERM: operation not permitted 0.0.0.0:3001";
    // Block tier: fires (incumbent semantics).
    assert!(matches!(
        cb("", vite),
        Some(super::SandboxBlockKind::Network)
    ));
    // BindOnly tier (Landlock installed, no seccomp): never Network.
    assert!(super::classify_block("", vite, None, super::NetEnforcement::LandlockNet).is_none());
    assert!(super::classify_block(
        "curl: (7) Operation not permitted",
        "",
        None,
        super::NetEnforcement::LandlockNet
    )
    .is_none());
    // AllowAll: nothing is enforced → nothing is attributable.
    assert!(super::classify_block("", vite, None, super::NetEnforcement::None).is_none());
    // Write/exec classes are Landlock-file facts — they fire under
    // every net tier (the conjunction scopes the NETWORK kind only).
    assert!(matches!(
        super::classify_block(
            "Permission denied",
            "",
            None,
            super::NetEnforcement::LandlockNet
        ),
        Some(super::SandboxBlockKind::Write)
    ));
    assert!(matches!(
        super::classify_block(
            "exec: x: Permission denied",
            "",
            Some(126),
            super::NetEnforcement::LandlockNet
        ),
        Some(super::SandboxBlockKind::ExecFace)
    ));
}

/// R7 (AC6): the ExecFace guidance names the class, forbids
/// retry-spending, and converges to ONE operator instruction.
#[test]
fn guidance_exec_face_converges_to_operator() {
    let text = super::failure_guidance_for_kind(super::SandboxBlockKind::ExecFace, Mode::Edit);
    assert!(text.contains("EXEC face"));
    assert!(text.contains("exit 126"));
    assert!(text.contains("ONE"));
    assert!(text.contains("operator instruction"));
    assert!(text.contains("then stop"));
}

/// net_enforcement(): Block → InetBlock; BindOnly → LandlockNet on
/// capable kernels / InetBlock (degraded) otherwise; AllowAll →
/// None. Mirrors prepare()'s decision and summary()'s print (one
/// truth, three readers).
#[test]
fn net_enforcement_matches_prepare_decision() {
    let spec_for = |net: super::policy::NetPolicy| super::SandboxSpec {
        face: super::Face::ReadWrite,
        net,
        writable_roots: vec![],
        exec_allow_roots: vec![],
        extra_writable: vec![],
    };
    assert_eq!(
        spec_for(super::policy::NetPolicy::Block).net_enforcement(),
        super::NetEnforcement::InetBlock
    );
    assert_eq!(
        spec_for(super::policy::NetPolicy::AllowAll).net_enforcement(),
        super::NetEnforcement::None
    );
    let bind = spec_for(super::policy::NetPolicy::BindOnly(
        super::policy::BindSet::from_iter_ports([3001]),
    ));
    let cap = Capability::probe();
    if cap.landlock_net {
        assert_eq!(bind.net_enforcement(), super::NetEnforcement::LandlockNet);
    } else {
        assert_eq!(bind.net_enforcement(), super::NetEnforcement::InetBlock);
    }
}

// ---------------------------------------------------------------------------
/// F1 (AC5, 2026-09-21): exec roots are canonicalized — a symlinked
/// PATH dir enters the spec as its REAL target; an alias pair
/// (symlink + real dir) dedups to ONE entry; a missing dir stays
/// literal (trap 5 tolerance). [F0 record: canonicalize does NOT
/// bring the pnpm wrapper's exec target into the face — the target
/// lives outside every PATH dir; the fix for that class is F2,
/// which stays 挂账.]
#[cfg(unix)]
#[test]
fn exec_roots_canonicalized_and_alias_deduped() {
    let tmp = tempfile::tempdir().unwrap();
    let real = tmp.path().join("real-bin");
    std::fs::create_dir_all(&real).unwrap();
    let alias = tmp.path().join("alias-bin");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let missing = tmp.path().join("not-there");

    // Drive the canonicalize step the same way build_spec does.
    let mut roots = vec![alias.clone(), real.clone(), missing.clone()];
    roots = roots
        .into_iter()
        .map(|r| std::fs::canonicalize(&r).unwrap_or(r))
        .collect();
    let mut seen = std::collections::HashSet::new();
    roots.retain(|p| seen.insert(p.clone()));
    // The alias resolved to the real dir and the two collapsed.
    assert_eq!(roots, vec![real.canonicalize().unwrap(), missing]);
}

/// AC5: the summary exec segment lists canonical roots (dirs only).
#[test]
fn summary_exec_segment_lists_canonical_roots() {
    let wt = if let Ok(c) = std::fs::canonicalize("/tmp") {
        c
    } else {
        PathBuf::from("/tmp")
    };
    let spec = super::SandboxSpec {
        face: super::Face::ReadWrite,
        net: super::policy::NetPolicy::Block,
        writable_roots: vec![wt.clone()],
        exec_allow_roots: vec![wt.clone(), PathBuf::from("/usr")],
        extra_writable: vec![],
    };
    let s = spec.summary();
    assert!(s.contains("exec_roots_canonical=["), "{s}");
    assert!(s.contains("/usr"), "{s}");
    // Count and list agree on membership: /tmp appears in the list.
    assert!(s.contains(&wt.display().to_string()), "{s}");
}

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
        net: super::policy::NetPolicy::Block,
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
    super::policy::build_spec(
        ctx,
        Some("integ-ro"),
        vec![],
        super::Face::ReadOnly,
        super::policy::NetPolicy::Block,
    )
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

/// AC3 (09-21-sandbox-net-bindonly): true-kernel BindOnly matrix —
/// declared-port bind succeeds, undeclared bind denied, connect
/// outside `{80,443}∪bind` denied, the daemon port 7456 unreachable
/// even though socket creation itself is unrestricted (no seccomp).
/// LOUD SKIP on kernels without Landlock ABI v4 net rules (this
/// WSL2 6.6 = ABI 3): the matrix is only meaningful where the
/// enforcer exists.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn integration_bind_only_net_matrix() {
    require_sandbox!();
    if !Capability::probe().landlock_net {
        eprintln!(
            "SKIP: Landlock ABI v4 net rules unavailable (probe landlock_net=false); \
             BindOnly live behavior needs kernel >= 6.7"
        );
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    let has_py = tokio::process::Command::new("sh")
        .arg("-c")
        .arg("command -v python3")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !has_py {
        eprintln!("SKIP-row: python3 unavailable for the socket probe");
        return;
    }
    let spec = super::SandboxSpec {
        face: super::Face::ReadWrite,
        net: super::policy::NetPolicy::BindOnly(super::policy::BindSet::from_iter_ports([43123])),
        writable_roots: vec![wt.clone(), PathBuf::from("/tmp")],
        exec_allow_roots: vec![
            PathBuf::from("/usr"),
            PathBuf::from("/bin"),
            PathBuf::from("/lib"),
            PathBuf::from("/lib64"),
            PathBuf::from("/dev"),
            PathBuf::from("/tmp"),
            wt.clone(),
        ],
        extra_writable: vec![],
    };
    // One python process per row keeps verdicts independent. All
    // verdicts land on STDERR (run_sandboxed captures stderr only):
    // `python3 -c "<one-liner>"` on success prints VERDICT, on
    // failure its traceback lands on stderr too; `echo exit=$? >&2`
    // reports the shell-visible exit code.
    fn py(code: &str) -> String {
        format!("python3 -c \"{code}\"; echo \"exit=$?\" >&2")
    }
    async fn probe(spec: &SandboxSpec, script: &str, wt: &Path) -> String {
        let (code, err) = run_sandboxed(spec, script, wt).await;
        assert_eq!(code, 0, "sh wrapper must run: {err}");
        err
    }
    // 1. Declared port bind: LISTEN succeeds (exit=0 + VERDICT).
    let out = probe(
        &spec,
        &py("import socket; s=socket.socket(); \\\n\
         s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); \\\n\
         s.bind(('127.0.0.1', 43123)); s.listen(1); print('VERDICT bind-declared ok')"),
        &wt,
    )
    .await;
    assert!(out.contains("VERDICT bind-declared ok"), "{out}");
    // 2. Undeclared port bind: denied before TCP (no listener was
    //    ever created, so a "Connection refused" would be impossible
    //    anyway — the guard documents the deny-before-TCP shape).
    let out = probe(
        &spec,
        &py("import socket; s=socket.socket(); \\\n\
         s.bind(('127.0.0.1', 43199))"),
        &wt,
    )
    .await;
    assert!(out.contains("exit=1"), "{out}");
    assert!(
        !out.contains("Connection refused"),
        "undeclared bind must be denied before TCP, got: {out}"
    );
    // 3. Connect to the daemon port 7456: denied (not in derived
    //    set {80,443,43123}). A "Connection refused" here would mean
    //    Landlock ALLOWED the connect (it reached the TCP stack).
    let out = probe(
        &spec,
        &py("import socket; s=socket.socket(); \\\n\
         s.connect(('127.0.0.1', 7456))"),
        &wt,
    )
    .await;
    assert!(
        out.contains("exit=1"),
        "connect to daemon port must fail, got: {out}"
    );
    assert!(
        !out.contains("Connection refused"),
        "a refusal means Landlock ALLOWED the connect (reached TCP); must be denied earlier: {out}"
    );
    // 4. Connect to a port IN the derived set (43123 ∈ bind snapshot)
    //    with nothing listening: ECONNREFUSED proves Landlock
    //    permitted the connect itself (deny would exit 1 without a
    //    TCP-level refusal).
    let out = probe(
        &spec,
        &py("import socket; s=socket.socket(); \\\n\
         s.connect(('127.0.0.1', 43123))"),
        &wt,
    )
    .await;
    assert!(
        out.contains("Connection refused"),
        "connect to whitelisted port should be permitted (ECONNREFUSED expected), got: {out}"
    );
}
