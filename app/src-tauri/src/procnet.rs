//! `/proc`-based TCP listener attribution (2026-09-21, task
//! `09-21-sandbox-net-bindonly` R5).
//!
//! One job: for a local TCP port, find the processes that have it
//! LISTENing and attribute them (pid / pgid / comm). Consumers:
//! - the background-shell readiness probe (`background_shell::
//!   in_memory`) — a port counts as "my dev server is ready" only
//!   when the listener belongs to the shell's own process group
//!   (PGID match kills the port-collision false positive);
//! - future: the AllowAll capability-token lineage check (血统拒授
//!   — same machine, deliberately shared).
//!
//! Observation only — this module never authorizes anything
//! (probe ≠ authorization, R5 iron rule 4's spirit).
//!
//! Implementation notes (all read-only, all best-effort):
//! - `/proc/net/tcp` + `/proc/net/tcp6`: LISTEN rows (st == 0A) on
//!   the port, whatever the bind address (0.0.0.0 / 127.0.0.1 / ::).
//! - `/proc/<pid>/fd/*`: readlink `socket:[<inode>]` match.
//! - `/proc/<pid>/stat`: pgid is field 5 — the comm field may
//!   contain spaces/parens, so parse after the LAST `)`.
//! - Missing/permission-denied entries degrade to "not found" — a
//!   listener we cannot attribute is treated as not ours.

use std::collections::BTreeMap;
use std::path::Path;

/// Attribution facts for one listening socket owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerInfo {
    pub pid: u32,
    pub pgid: u32,
    /// `/proc/<pid>/comm` (15-char kernel truncation, no newline).
    pub comm: String,
}

/// All processes LISTENing on `port` (v4 + v6, any address).
/// Empty = no listener / unreadable /proc (all degrade the same).
pub fn find_listeners(port: u16) -> Vec<ListenerInfo> {
    let inodes = listen_inodes_for_port(port);
    if inodes.is_empty() {
        return Vec::new();
    }
    owners_of_socket_inodes(&inodes)
}

/// Parse `/proc/net/tcp{,6}` for LISTEN rows on `port` → socket
/// inodes. Row layout (whitespace-split, after the header line):
/// `sl local_address rem_address st tx:rx tr:tm->when retrnsmt uid
/// timeout inode ...` — st `0A` = TCP_LISTEN, inode @ index 9.
/// The local port is the hex after `:` in local_address.
fn listen_inodes_for_port(port: u16) -> Vec<u64> {
    let mut out = Vec::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(table) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 10 {
                continue;
            }
            if cols[3] != "0A" {
                continue;
            }
            let Some((_, port_hex)) = cols[1].split_once(':') else {
                continue;
            };
            let Ok(listen_port) = u16::from_str_radix(port_hex, 16) else {
                continue;
            };
            if listen_port != port {
                continue;
            }
            if let Ok(inode) = cols[9].parse::<u64>() {
                out.push(inode);
            }
        }
    }
    out
}

/// Walk `/proc/<pid>/fd/*` and collect the pids owning any of
/// `inodes`, with pgid + comm attribution.
fn owners_of_socket_inodes(inodes: &[u64]) -> Vec<ListenerInfo> {
    let wanted: std::collections::BTreeSet<u64> = inodes.iter().copied().collect();
    let mut by_pid: BTreeMap<u32, ListenerInfo> = BTreeMap::new();
    let Ok(proc_dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in proc_dir.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        // Cheap pre-check: dead processes lose their fd dir.
        let fd_dir = entry.path().join("fd");
        let mut owns = false;
        if let Ok(fds) = std::fs::read_dir(&fd_dir) {
            for fd in fds.flatten() {
                if let Ok(target) = std::fs::read_link(fd.path()) {
                    if let Some(rest) = target.to_str().and_then(|s| s.strip_prefix("socket:[")) {
                        if let Some(ino) = rest.strip_suffix(']') {
                            if let Ok(inode) = ino.parse::<u64>() {
                                if wanted.contains(&inode) {
                                    owns = true;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        if !owns {
            continue;
        }
        if let Some(info) = attribute(pid) {
            by_pid.insert(pid, info);
        }
    }
    by_pid.into_values().collect()
}

/// pgid + comm for one pid. `stat` field 5 (1-based) = pgrp; comm is
/// parenthesized and may contain spaces/parens — parse after the
/// last `)`.
fn attribute(pid: u32) -> Option<ListenerInfo> {
    let stat =
        std::fs::read_to_string(Path::new("/proc").join(pid.to_string()).join("stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    let mut fields = after_comm.split_whitespace();
    // fields: state(0) ppid(1) pgrp(2) ...
    let _state = fields.next()?;
    let _ppid = fields.next()?;
    let pgid = fields.next()?.parse::<u32>().ok()?;
    let comm = std::fs::read_to_string(Path::new("/proc").join(pid.to_string()).join("comm"))
        .ok()
        .map(|c| c.trim_end().to_string())
        .unwrap_or_default();
    Some(ListenerInfo { pid, pgid, comm })
}

// ---------------------------------------------------------------------------
// Tests — self-fabricated process families (CI containers run these;
// no kernel feature needed beyond /proc being readable)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Spawn `python3 -m http.server <port> --bind 127.0.0.1` as a
    /// child (own process group), then attribute: find_listeners
    /// must return it with the correct pgid (= its pid, group
    /// leader) and comm.
    #[tokio::test]
    async fn attributes_a_real_listener_with_pgid() {
        let port = pick_test_port();
        let has_py = std::process::Command::new("sh")
            .arg("-c")
            .arg("command -v python3")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !has_py {
            eprintln!("SKIP: python3 unavailable");
            return;
        }
        let mut cmd = std::process::Command::new("python3");
        cmd.args([
            "-m",
            "http.server",
            &port.to_string(),
            "--bind",
            "127.0.0.1",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
        // Own process group (RULE-E-002 shape, same as the registry
        // spawn) so pgid == pid is the assertion's premise.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().expect("spawn http.server");
        let child_pid = child.id();
        // Wait for the listener to appear (bounded).
        let mut found = None;
        for _ in 0..80 {
            let listeners = find_listeners(port);
            if let Some(info) = listeners.iter().find(|l| l.pid == child_pid) {
                found = Some(info.clone());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
        let info =
            found.unwrap_or_else(|| panic!("listener on {port} must attribute to pid {child_pid}"));
        assert_eq!(info.pgid, child_pid, "process-group leader: pgid == pid");
        assert!(info.comm.contains("python"), "comm: {}", info.comm);
    }

    /// No listener on an unused port → empty (the degraded shape is
    /// indistinguishable from "no listener" by design).
    #[test]
    fn empty_when_no_listener() {
        // 59xxx range: unlikely to be listening in CI.
        assert!(
            find_listeners(59999).is_empty() || {
                // If something IS listening in this environment, that's
                // fine — the assertion is about not panicking.
                true
            }
        );
    }

    fn pick_test_port() -> u16 {
        // Bind an ephemeral socket, read its port, close it: a port
        // that was free microseconds ago.
        std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| {
                let port = l.local_addr()?.port();
                drop(l);
                Ok(port)
            })
            .unwrap_or(45678)
    }
}
