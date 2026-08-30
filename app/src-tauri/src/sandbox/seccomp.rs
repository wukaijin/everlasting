//! seccomp BPF filter — network lockdown for the sandbox.
//!
//! One job: `socket(AF_INET)` and `socket(AF_INET6)` return EPERM;
//! everything else is ALLOWED (default-allow — no default-deny
//! syscall surface, design §2.4 / Codex-style minimalism). Landlock
//! owns the filesystem limits; the filter owns the network only.
//!
//! Consequences by design (spike landlock 篇 §5): DNS dies (UDP
//! socket creation is `socket(AF_INET, ...)`), curl / git fetch /
//! npm registry fail fast with "Operation not permitted". AF_UNIX is
//! untouched so docker/pnpm/X11-style socket clients keep working.
//! Landlock's own network rules need ABI v4 (kernel 6.7+) — we don't
//! gamble on kernel versions, seccomp is universally available
//! (CONFIG_SECCOMP_FILTER=y in every WSL2 kernel + CI runners).
//!
//! The program is built in the parent process; the pre_exec closure
//! hands the byte array to `prctl(PR_SET_SECCOMP,
//! SECCOMP_MODE_FILTER, &fprog)` — the kernel copies the filter out
//! of parent memory during that single call (design W2), so
//! referencing the parent-constructed `Vec` from the forked child is
//! safe and malloc-free.

/// BPF class constants (libc exposes these on linux —
/// `linux/bpf_common.h` / `linux/filter.h`; re-pinned by tests).
use libc::{
    BPF_ABS, BPF_JEQ, BPF_JMP, BPF_K, BPF_LD, BPF_RET, BPF_W, SECCOMP_MODE_FILTER,
    SECCOMP_RET_ALLOW, SECCOMP_RET_ERRNO,
};

use super::landlock::{prctl, PR_SET_SECCOMP};

/// `struct seccomp_data` field offsets (kernel UAPI
/// `linux/seccomp.h`): `{ int nr; u32 arch; u64 instruction_pointer;
/// u64 args[6]; }` → `nr` @ 0, `args[0]` low 32 bits @ 16 (little
/// endian).
const SECCOMP_DATA_NR: u32 = 0;
const SECCOMP_DATA_ARG0_LO: u32 = 16;

fn stmt(code: u32, k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt: 0,
        jf: 0,
        k,
    }
}

fn jump(code: u32, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
    libc::sock_filter {
        code: code as u16,
        jt,
        jf,
        k,
    }
}

fn ret(k: u32) -> libc::sock_filter {
    libc::sock_filter {
        code: (BPF_RET | BPF_K) as u16,
        jt: 0,
        jf: 0,
        k,
    }
}

/// Build the default network filter (~8 instructions):
///
/// ```text
/// 0: A = nr
/// 1: nr == __NR_socket ? fallthrough : -> 7 (ALLOW)
/// 2: A = args[0] low 32          (the kernel truncates family to int)
/// 3: args[0] == AF_INET  ? -> 6 (EPERM) : fallthrough
/// 4: args[0] == AF_INET6 ? -> 6 (EPERM) : fallthrough
/// 5: ALLOW                       (AF_UNIX etc.)
/// 6: ERRNO | EPERM
/// 7: ALLOW                       (every non-socket syscall)
/// ```
pub(crate) fn build_inet_block_filter() -> Vec<libc::sock_filter> {
    vec![
        stmt(BPF_LD | BPF_W | BPF_ABS, SECCOMP_DATA_NR),
        jump(
            BPF_JMP | BPF_JEQ | BPF_K,
            libc::SYS_socket as u32,
            0,
            5, // not socket → instr 7 (ALLOW)
        ),
        stmt(BPF_LD | BPF_W | BPF_ABS, SECCOMP_DATA_ARG0_LO),
        jump(
            BPF_JMP | BPF_JEQ | BPF_K,
            libc::AF_INET as u32,
            2, // match → instr 6 (EPERM)
            0,
        ),
        jump(
            BPF_JMP | BPF_JEQ | BPF_K,
            libc::AF_INET6 as u32,
            1, // match → instr 6 (EPERM)
            0,
        ),
        ret(SECCOMP_RET_ALLOW),
        ret(SECCOMP_RET_ERRNO | libc::EPERM as u32),
        ret(SECCOMP_RET_ALLOW),
    ]
}

/// Install the filter — pre_exec context ONLY (pure syscall, no
/// malloc: the `sock_fprog` header is stack-built and points at the
/// parent-constructed program; the kernel copies the program during
/// the prctl). Requires `PR_SET_NO_NEW_PRIVS` to already be set —
/// the caller (`mod.rs::pre_exec_apply`) does that first.
pub(crate) fn install_in_preexec(filter: &[libc::sock_filter]) -> std::io::Result<()> {
    let fprog = libc::sock_fprog {
        len: filter.len() as u16,
        filter: filter.as_ptr() as *mut libc::sock_filter,
    };
    let ret = unsafe {
        prctl(
            PR_SET_SECCOMP,
            SECCOMP_MODE_FILTER,
            &fprog,
            0 as libc::c_ulong,
            0,
        )
    };
    if ret != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
