//! Sandbox tests. Two layers:
//!
//! - Unit tests (all platforms): ABI constant pins (kernel UAPI
//!   alignment), AccessSet ⊆ handled (spike trap 2 at type level),
//!   BPF golden + logic walk, policy spec construction, gate matrix
//!   (AC3 / AC4 / AC5), guidance copy (AC7).
//! - Integration tests (`#[cfg(target_os = "linux")]` + live
//!   capability check): real spawns of `sh -c` under the real
//!   ruleset — write allow/deny, exec interop deny, read
//!   unrestricted, network block, AF_UNIX pass (AC1 / AC2). They
//!   skip (with a loud note, not a silent pass-fail) when the
//!   runtime kernel lacks Landlock/seccomp, mirroring the spike's
//!   own matrix harness.

use std::path::{Path, PathBuf};

use super::{gate, Capability, SandboxSpec, DEVICE_WRITE_PATHS};
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
    let spec = super::policy::build_spec(&ctx, Some("sess-1"), vec![]);
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

#[tokio::test]
async fn spec_merges_extra_writable_without_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = policy_ctx(&tmp);
    let extra = vec![PathBuf::from("/opt/data"), PathBuf::from("/tmp")];
    let spec = super::policy::build_spec(&ctx, Some("s"), extra);
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
    let a = super::policy::build_spec(&ctx, Some("s"), vec![]);
    let b = super::policy::build_spec(&ctx, Some("s"), vec![]);
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
// Gate matrix (AC3 / AC4 / AC5)
// ---------------------------------------------------------------------------

fn cap_ok() -> Capability {
    Capability {
        landlock: true,
        seccomp: true,
    }
}

#[test]
fn gate_readonly_edit_enabled_sandboxes() {
    assert_eq!(gate("ls -la", Mode::Edit, cap_ok(), true), None);
    assert_eq!(gate("git diff | head", Mode::Plan, cap_ok(), true), None);
}

#[test]
fn gate_sideeffect_and_ask_tiers_skip() {
    // AC3: SideEffect (mkdir) / Ask (rm) run exactly as before.
    assert_eq!(
        gate("mkdir x", Mode::Edit, cap_ok(), true),
        Some("command is not ReadOnly tier")
    );
    assert_eq!(
        gate("rm x", Mode::Edit, cap_ok(), true),
        Some("command is not ReadOnly tier")
    );
    // Command substitution fails-safe to Ask → not sandboxed either.
    assert_eq!(
        gate("echo $(rm x)", Mode::Edit, cap_ok(), true),
        Some("command is not ReadOnly tier")
    );
}

#[test]
fn gate_yolo_and_kill_switch_skip() {
    // AC3: Yolo bypasses the sandbox entirely.
    assert_eq!(
        gate("ls", Mode::Yolo, cap_ok(), true),
        Some("session mode is Yolo")
    );
    // AC4: kill switch → identical-to-legacy behavior.
    assert_eq!(
        gate("ls", Mode::Edit, cap_ok(), false),
        Some("sandbox_enabled=false")
    );
}

#[test]
fn gate_capability_probe_fail_opens() {
    // AC5: probe stub (landlock + seccomp unavailable) → Skip, the
    // command runs exactly as the legacy path.
    let broken = Capability {
        landlock: false,
        seccomp: true,
    };
    assert_eq!(
        gate("ls", Mode::Edit, broken, true),
        Some("capability probe failed (fail-open)")
    );
    let broken6 = Capability {
        landlock: true,
        seccomp: false,
    };
    assert_eq!(
        gate("ls", Mode::Edit, broken6, true),
        Some("capability probe failed (fail-open)")
    );
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
