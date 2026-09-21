//! Landlock ABI v1 subset — hand-written constants + ruleset builder.
//!
//! No `landlock` crate (PRD C1): the v1 UAPI surface is ~30 lines of
//! constants, all pinned against the kernel UAPI header by unit tests
//! (`tests_sandbox.rs::abi_*`). libc 0.2 supplies the syscall numbers
//! (`SYS_landlock_*`, arch-portable) but NOT the `landlock.h`
//! constants — those live here — and on linux-gnu targets not even
//! `prctl` / `PR_*` (android/l4re only), so `prctl` is declared
//! locally too.
//!
//! Trap 2 (spike): a rule requesting access bits outside the
//! ruleset's handled set fails with EINVAL — with an error message
//! that looks like "device nodes can't have rules". Eliminated at
//! the type level: [`AccessSet`] has no constructor from raw bits,
//! and its three constants are subsets of [`HANDLED_ACCESS_FS`]
//! (asserted by test).

use std::collections::HashMap;
use std::io;
use std::os::fd::RawFd;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// prctl — not exposed by libc on linux-gnu (android/l4re only)
// ---------------------------------------------------------------------------

extern "C" {
    /// glibc / musl `prctl` (variadic). Called only from the pre_exec
    /// closure (async-signal-safe: thin syscall wrapper, no malloc).
    pub(crate) fn prctl(option: libc::c_int, ...) -> libc::c_int;
}

/// `PR_GET_SECCOMP` (kernel UAPI `linux/prctl.h`) — read-only probe:
/// returns the current seccomp mode (≥0) or -1/EINVAL when the
/// kernel lacks seccomp.
pub(crate) const PR_GET_SECCOMP: libc::c_int = 21;
/// `PR_SET_SECCOMP` — installs a filter (used with
/// `SECCOMP_MODE_FILTER`).
pub(crate) const PR_SET_SECCOMP: libc::c_int = 22;
/// `PR_SET_NO_NEW_PRIVS` — must precede `landlock_restrict_self`
/// (spike trap 4); also seals the suid/sgid escalation surface.
pub(crate) const PR_SET_NO_NEW_PRIVS: libc::c_int = 38;

// ---------------------------------------------------------------------------
// landlock.h UAPI constants (ABI v1 subset)
// ---------------------------------------------------------------------------

/// `LANDLOCK_CREATE_RULESET_VERSION` flag: probe the supported ABI.
pub(crate) const LANDLOCK_CREATE_RULESET_VERSION: libc::c_uint = 1 << 0;
/// `LANDLOCK_RULE_PATH_BENEATH` — the only rule type in ABI v1.
pub(crate) const LANDLOCK_RULE_PATH_BENEATH: libc::c_int = 1;
/// `LANDLOCK_RULE_NET_PORT` — TCP port rule type, ABI v4 (kernel
/// 6.7+). Pinned by test like every other constant here (trap 1).
pub(crate) const LANDLOCK_RULE_NET_PORT: libc::c_int = 2;

/// Kernel UAPI `struct landlock_ruleset_attr` (ABI v1: one field;
/// ABI v4 appends `handled_access_net`). Offsets/size match the C
/// layout; the kernel copies the struct by pointer during
/// `landlock_create_ruleset`. The size PASSED to the syscall decides
/// how much of it the kernel reads: 8 bytes = v1 shape (net
/// unrestricted), 16 = v4 shape (net per the second field).
#[repr(C)]
pub(crate) struct RulesetAttr {
    pub handled_access_fs: u64,
    pub handled_access_net: u64,
}

impl RulesetAttr {
    /// Byte length of the ABI v1 shape (fs only) — what
    /// `landlock_create_ruleset` must be given when no net access is
    /// handled (kernels without ABI v4 reject the 16-byte shape).
    pub(crate) const FS_ONLY_SIZE: usize = 8;
}

/// Kernel UAPI `struct landlock_net_port_attr` (ABI v4): u64
/// allowed_access + u16 port, padded to 16 under `repr(C)` — field
/// offsets (allowed_access @0, port @8) identical to the packed C
/// layout; the kernel reads by offset.
#[repr(C)]
#[derive(Debug)]
pub(crate) struct NetPortAttr {
    pub allowed_access: u64,
    pub port: u16,
}

/// `LANDLOCK_ACCESS_NET_*` bits (ABI v4; the FS bits above end at
/// 1<<12 — net bits continue at 1<<13, kernel UAPI order). Pinned
/// by test (`abi_net_*`).
pub(crate) mod net_bits {
    pub const BIND_TCP: u64 = 1 << 13;
    pub const CONNECT_TCP: u64 = 1 << 14;
}

/// Handled net access set for a BindOnly ruleset: both bits. A net
/// access NOT in the handled set is unrestricted by Landlock — so
/// BindOnly must handle BOTH bind and connect (restricting only one
/// would fail-open the other).
pub(crate) const HANDLED_ACCESS_NET: u64 = net_bits::BIND_TCP | net_bits::CONNECT_TCP;

/// Kernel UAPI `struct landlock_path_beneath_attr`
/// (`__attribute__((packed))` in the C header: u64 + s32 = 12 bytes).
/// Field offsets are identical under plain `repr(C)` (allowed_access
/// @0, parent_fd @8); the kernel reads fields by offset, so the
/// trailing padding of the wider Rust struct is never read. A packed
/// Rust struct is deliberately avoided — taking field references
/// into packed structs is a known unsafe footgun.
#[repr(C)]
pub(crate) struct PathBeneathAttr {
    pub allowed_access: u64,
    pub parent_fd: libc::c_int,
}

/// `LANDLOCK_ACCESS_FS_*` bits (ABI v1: EXECUTE..=MAKE_SYM, 1<<0
/// through 1<<12; APPEND only arrives with ABI v6 — trap 1: don't
/// trust distro headers or assume newer bits).
pub(crate) mod bits {
    pub const EXECUTE: u64 = 1 << 0;
    pub const WRITE_FILE: u64 = 1 << 1;
    // Read bits are NOT in our handled mask (控写不控读) and thus
    // unused in production — kept for ABI completeness + the UAPI
    // alignment tests.
    #[allow(dead_code)]
    pub const READ_FILE: u64 = 1 << 2;
    #[allow(dead_code)]
    pub const READ_DIR: u64 = 1 << 3;
    pub const REMOVE_DIR: u64 = 1 << 4;
    pub const REMOVE_FILE: u64 = 1 << 5;
    pub const MAKE_CHAR: u64 = 1 << 6;
    pub const MAKE_DIR: u64 = 1 << 7;
    pub const MAKE_REG: u64 = 1 << 8;
    pub const MAKE_SOCK: u64 = 1 << 9;
    pub const MAKE_FIFO: u64 = 1 << 10;
    pub const MAKE_BLOCK: u64 = 1 << 11;
    pub const MAKE_SYM: u64 = 1 << 12;
}

/// Handled access set for our ruleset (spike recipe): EXECUTE + the
/// full write family. Reads are NOT handled → unrestricted (deliberate;
/// read limiting is an open problem even upstream, and controlling it
/// would multiply the false-kill surface).
pub(crate) const HANDLED_ACCESS_FS: u64 = bits::EXECUTE
    | bits::WRITE_FILE
    | bits::REMOVE_DIR
    | bits::REMOVE_FILE
    | bits::MAKE_CHAR
    | bits::MAKE_DIR
    | bits::MAKE_REG
    | bits::MAKE_SOCK
    | bits::MAKE_FIFO
    | bits::MAKE_BLOCK
    | bits::MAKE_SYM;

// ---------------------------------------------------------------------------
// AccessSet — compile-time "rule access ⊆ handled" (trap 2 / C5)
// ---------------------------------------------------------------------------

/// Access rights a rule may request. The inner u64 is `pub(crate)`
/// for tests/summary, but there is NO constructor from raw bits:
/// every `AccessSet` value comes from one of the three constants
/// below, each a strict subset of [`HANDLED_ACCESS_FS`] by
/// construction — so any rule built through this type satisfies the
/// kernel's "rule access ⊆ handled" precondition without a runtime
/// check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AccessSet(pub(crate) u64);

impl AccessSet {
    /// Executable subtree (exec allow face).
    pub const EXECUTE: AccessSet = AccessSet(bits::EXECUTE);
    /// Writable subtree: WRITE_FILE + REMOVE_* + MAKE_* (the spike
    /// recipe for writable roots — includes MAKE_SOCK so unix
    /// sockets can be bound inside the worktree/tmp).
    pub const WRITE_FAMILY: AccessSet = AccessSet(
        bits::WRITE_FILE
            | bits::REMOVE_DIR
            | bits::REMOVE_FILE
            | bits::MAKE_CHAR
            | bits::MAKE_DIR
            | bits::MAKE_REG
            | bits::MAKE_SOCK
            | bits::MAKE_FIFO
            | bits::MAKE_BLOCK
            | bits::MAKE_SYM,
    );
    /// Per-file device write (`/dev/null` family — spike trap 3).
    pub const WRITE_FILE: AccessSet = AccessSet(bits::WRITE_FILE);
}

// ---------------------------------------------------------------------------
// NetAccessSet — the net-side twin of AccessSet (trap 2 / C5): no
// constructor from raw bits, both constants are strict subsets of
// HANDLED_ACCESS_NET, so a NET_PORT rule can never request an
// unhandled access bit (kernel EINVAL) by construction.
// ---------------------------------------------------------------------------

/// Net access rights a TCP-port rule may request (ABI v4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NetAccessSet(u64);

impl NetAccessSet {
    /// Allow BIND_TCP on the port (dev-server listen face).
    pub const BIND_TCP: NetAccessSet = NetAccessSet(net_bits::BIND_TCP);
    /// Allow CONNECT_TCP to the port (outbound face).
    pub const CONNECT_TCP: NetAccessSet = NetAccessSet(net_bits::CONNECT_TCP);

    pub(crate) fn bits(self) -> u64 {
        self.0
    }
}

/// Build the pre_exec-ready attr array for one access × port list.
/// Allocation happens in the parent safe zone; the closure only
/// reads the Vec through the Arc.
pub(crate) fn net_port_attrs(
    access: NetAccessSet,
    ports: &std::collections::BTreeSet<u16>,
) -> Vec<NetPortAttr> {
    ports
        .iter()
        .map(|&port| NetPortAttr {
            allowed_access: access.bits(),
            port,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// RulesetBuilder
// ---------------------------------------------------------------------------

/// A rule path opened with `O_PATH | O_CLOEXEC`, paired with the
/// access bits to grant beneath it.
pub(crate) struct PreparedRuleset {
    pub ruleset_fd: RawFd,
    /// (path fd, allowed access) — consumed by the pre_exec closure,
    /// closed by `PreparedData: Drop`.
    pub rules: Vec<(RawFd, u64)>,
}

/// Builds the rule set in the parent process ("safe zone" — open(2)
/// is allowed here, unlike in the pre_exec closure).
///
/// Same-path rules are merged with bitwise-OR before any fd is
/// opened (the kernel unions rules for the same object anyway, but
/// merging here keeps the rule count — and therefore the fd count —
/// minimal and the behavior independent of that kernel subtlety).
pub(crate) struct RulesetBuilder {
    rules: HashMap<PathBuf, u64>,
    order: Vec<PathBuf>,
    /// Handled net access bits (0 = ABI v1 shape: net unrestricted,
    /// the Block/AllowAll tiers). Only the BindOnly tier sets
    /// [`HANDLED_ACCESS_NET`] — and the attr size handed to the
    /// kernel follows (16 vs 8 bytes), because pre-v4 kernels reject
    /// the 16-byte shape with EINVAL.
    handled_net: u64,
}

impl RulesetBuilder {
    pub fn new() -> Self {
        Self {
            rules: HashMap::new(),
            order: Vec::new(),
            handled_net: 0,
        }
    }

    /// Make the ruleset handle (and thus restrict) net accesses.
    /// MUST be called before [`Self::build`]; setting a nonzero mask
    /// on a kernel without ABI v4 fails `build()` with EINVAL — the
    /// caller (mod.rs `prepare`) probes first and degrades to Block.
    pub fn handle_net_access(&mut self, handled: u64) -> &mut Self {
        self.handled_net = handled;
        self
    }

    /// Grant `access` beneath `path`. Missing paths are tolerated at
    /// build time (spike trap 5): they are silently skipped when the
    /// fd is opened, not when this is called.
    pub fn allow(&mut self, path: &Path, access: AccessSet) -> &mut Self {
        match self.rules.get_mut(path) {
            Some(bits) => *bits |= access.0,
            None => {
                self.order.push(path.to_path_buf());
                self.rules.insert(path.to_path_buf(), access.0);
            }
        }
        self
    }

    /// Create the ruleset fd and open one `O_PATH` fd per rule path.
    ///
    /// Fails (Err) only if the kernel rejects the handled mask —
    /// which the type system makes impossible for `AccessSet`-built
    /// rules. Missing/unopenable paths log at debug and are skipped
    /// (spike trap 5: `open(O_PATH)` failure must not abort).
    pub fn build(self) -> io::Result<PreparedRuleset> {
        let attr = RulesetAttr {
            handled_access_fs: HANDLED_ACCESS_FS,
            handled_access_net: self.handled_net,
        };
        // Size follows the handled set: 8 bytes (v1 shape) when no
        // net access is handled — byte-identical to the pre-net
        // ruleset creation on every kernel; 16 only for net tiers.
        let attr_size = if self.handled_net == 0 {
            RulesetAttr::FS_ONLY_SIZE
        } else {
            std::mem::size_of::<RulesetAttr>()
        };
        let ruleset_fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &attr as *const RulesetAttr,
                attr_size,
                0 as libc::c_uint,
            )
        };
        if ruleset_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let ruleset_fd = ruleset_fd as RawFd;

        let mut rules = Vec::with_capacity(self.order.len());
        for path in self.order {
            let access = self.rules[&path];
            let fd =
                unsafe { libc::open(path_as_cstr(&path).as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
            if fd < 0 {
                // Missing path (toolchain probe, optional device, …):
                // skip the rule, keep the sandbox usable. Logging in
                // the parent here — this is the safe zone.
                tracing::debug!(
                    path = %path.display(),
                    error = %io::Error::last_os_error(),
                    "sandbox: rule path not openable, skipped (spike trap 5 tolerance)"
                );
                continue;
            }
            rules.push((fd as RawFd, access));
        }
        Ok(PreparedRuleset { ruleset_fd, rules })
    }
}

/// NUL-terminated path bytes for `open(2)`. Allocation happens in
/// the parent (safe zone) — never in the pre_exec closure.
fn path_as_cstr(path: &Path) -> std::ffi::CString {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .unwrap_or_else(|_| std::ffi::CString::new("/").expect("slash is NUL-free"))
}
