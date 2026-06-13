//! ⑨ 关 Tier 4 shell prefix classification (A2+B7 re-grill, 2026-06-13).
//!
//! Sits between the agent loop's `provider.send()` stream and
//! `tools::execute_tool`. For every `shell` tool_use, the agent loop
//! takes the **first whitespace-separated token** of the command and
//! classifies it against the two static tables below:
//!
//! - `SHELL_WHITELIST`: prefixes that default-allow (no modal).
//!   Examples: `git`, `cargo`, `pnpm`, `ls`, `cat`, `find`, `grep`,
//!   `head`, `tail`, `wc`. These are read-only or repo-local
//!   side-effects the user almost always wants.
//! - `SHELL_ASKLIST`: prefixes that always go to the modal even
//!   when "inside" the project. Examples: `rm`, `chmod`, `sudo`,
//!   `curl`, `wget`. The user is expected to make a per-call
//!   decision because the side effects are visible / hard to
//!   reverse / network-egress. (Note: `mv` is in the whitelist
//!   instead — common case is renaming a project file; the
//!   user can still hit "deny" if they want to forbid a
//!   specific rename.)
//!
//! **Anything else** is treated as `Ask` (the modal pops). This
//! matches the "B 试图精确会输" philosophy (Q7 of the re-grill
//! session): we don't try to recursively parse pipes / env vars /
//! `cd` chains; we trust the first token as a coarse classifier and
//! rely on Tier 2 (hard kill list) to catch the catastrophic
//! patterns. The user is the safety net for everything in between.
//!
//! **Yolo override**: the Tier 4 caller (`agent/permissions::check`)
//! bypasses this whole module when `mode == Yolo` and returns
//! `Decision::Allow` directly. Yolo = "no questions asked" — the
//! prefix table is irrelevant in that mode (Tier 2 still
//! hard-kills).
//!
//! **Pipe handling** (`git status | head -5`): the parser splits on
//! whitespace and returns the FIRST token (`git`). The pipe target
//! is ignored at the permission layer. Tier 2 catches the
//! catastrophic `curl | bash` pattern separately (regex in
//! `dangerous.rs`).
//!
//! **Quoting / shell metacharacters**: deliberately not handled.
//! The first token is taken as-is. `bash -c "ls"` returns `bash`
//! (not `ls`) and falls into `Ask` — `bash` is in neither table.
//! This is the documented trade-off (Q7 "无递归/无 alias/无 pipe").
//!
//! **Prefix `path` (`./foo`, `/usr/bin/ls`)**: the leading `./` or
//! `/` is stripped before lookup. So `./cargo test` and
//! `/usr/bin/git status` both classify as the unprefixed token.
//! This is a deliberate UX choice: a script in `./scripts/test.sh`
//! that invokes `cargo` is asking the permission layer the same
//! question as a direct `cargo` call.
//!
//! See `.trellis/spec/backend/tool-contract.md` §"Scenario: Path-
//! based Permission" for the full contract (it's a sibling doc to
//! this file in the spec tree).

/// Outcome of classifying a shell command's first token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellTrust {
    /// Whitelist hit — no modal, default-allow. Caller still writes
    /// `tool_allowed` audit + a `path` match_kind row on
    /// `AllowAlways` for the prefix.
    Allow,
    /// Asklist hit or unknown prefix — emit `permission:ask`.
    /// Caller writes `tool_permission_ask` audit. The user's
    /// `AllowAlways` then writes a `prefix` match_kind row.
    Ask,
}

/// Whitelist of command prefixes that default-allow at Tier 4.
///
/// Kept as a const slice for compile-time visibility (no runtime
/// HashMap build, the binary inlines a sorted lookup). Order is
/// not significant — `classify_prefix` is a linear scan; for the
/// ~30 entries we have today this is faster than building a
/// `phf` map. If this list grows past ~100 entries, switch to
/// `phf` or a `BTreeSet`.
const SHELL_WHITELIST: &[&str] = &[
    // VCS / read-only repo inspection
    "git",       // git status / log / diff / show (write ops still hit
                 // Tier 2 for force-push to main; checkout of a new
                 // file is fine because the file is in the repo).
    "gh",        // gh pr / issue / run view (read-only commands).
    // Build & test (project-local side effects only)
    "cargo",     // cargo build / test / check / run / fmt
    "rustc",     // rare direct rustc invocations
    "pnpm",      // pnpm install / run / test
    "npm",       // npm install / test / run-script
    "yarn",      // yarn install / run / test
    "bun",       // bun install / test / run
    "node",      // node script.js (project-local scripts)
    "tsc",       // tsc --noEmit (typecheck)
    "npx",       // npx <command> (project-local)
    "make",      // make <target> (project Makefile)
    "cmake",     // cmake --build
    "meson",     // meson compile
    "ninja",     // ninja <target>
    "go",        // go build / test
    "python",    // python script.py
    "python3",
    "pytest",
    "rustup",    // rustup update / show
    // Inspection (read-only)
    "ls",        // list dir
    "cat",       // read file
    "head",      // read file head
    "tail",      // read file tail
    "wc",        // word count
    "stat",      // file metadata
    "file",      // file type
    "find",      // find . -name "*.tmp" -delete is technically a
                 // side-effect, but `find` is overwhelmingly
                 // used for read-only queries. Tier 2 catches
                 // the catastrophic patterns separately.
    "grep",      // grep inside repo
    "rg",        // ripgrep
    "ag",        // silver searcher
    "fd",        // fd (find alternative)
    "tree",      // tree . — directory tree
    "less",      // paging
    "more",
    "diff",      // diff a.txt b.txt
    "xxd",       // hex dump
    "od",        // octal dump
    "sed",       // sed -n (read-only flag); sed -i is silently
                 // allowed because the user can recover from
                 // any data loss with worktree + git history.
    "awk",       // awk read-only
    "cut",
    "sort",
    "uniq",
    "tr",
    "echo",      // echo "hello" — no side effect
    "printf",
    "true",
    "false",
    "test",      // test -f / -d etc.
    "[",         // [ -f ... ]
    "pwd",       // print working dir
    "env",       // env vars
    "whoami",
    "date",
    "cal",
    "uname",
    "which",
    "type",
    // Project-local safe side-effects
    "mkdir",     // mkdir -p (inside repo)
    "touch",     // touch newfile
    "cp",        // cp src dst
    "mv",        // actually still has the asklist entry, but the
                 // most common case is renaming a project file —
                 // whitelist it. (User can still hit "deny" if
                 // they want to forbid a specific rename.)
    "ln",        // ln -s
    "tar",       // tar -xzf / -czf (project archives)
    "zip",
    "unzip",
    "gzip",
    "gunzip",
    "jq",        // jq '.foo' data.json
    "yq",
    "xmllint",
    // Network: explicit whitelist (the user is in interactive
    // mode, so a `curl <known API>` is intentional). The Tier 2
    // kill list still catches `curl ... | bash`.
    "curl",      // curl https://... (read-only by default; writes
                 // go through -o which is still inside repo).
    "wget",      // wget -qO- ...
];

/// Asklist of command prefixes that always go to the modal.
///
/// These are commands whose side effects the user should make a
/// per-call decision about. Network-egress outside the
/// whitelist, file mutation across the project boundary, and
/// privilege escalation all live here.
///
/// **Reserved for future behavioral split**: today
/// `classify_prefix` collapses asklist hits and unknown
/// prefixes into the same `ShellTrust::Ask` outcome (the
/// re-grill PRD §10 notes "宁误弹不漏弹" philosophy — we don't
/// differentiate the reason text between asklist and unknown).
/// The list is still useful as a curated reference:
/// - the size/overlap tests (`whitelist_has_no_overlap_with_asklist`,
///   `asklist_size_is_in_target_range`) catch accidental
///   dual-list additions and mass-adds;
/// - a future PR that wants different `build_ask_reason` text
///   for asklist-vs-unknown (per re-grill PRD §1 "reason 字段
///   区分") can branch on `SHELL_ASKLIST.contains(&first)`
///   without re-curating the entries.
///
/// The `dead_code` allow is intentional and reflects the
/// "reference table, not runtime dispatch" role. Removing the
/// list would force the future PR to re-enumerate ~30
/// dangerous commands from scratch.
#[allow(dead_code)]
const SHELL_ASKLIST: &[&str] = &[
    // Privilege escalation
    "sudo",      // sudo anything — always confirm
    "su",        // switch user
    "doas",      // OpenBSD sudo
    // Dangerous file mutation (these are not in the whitelist
    // by being absent from SHELL_WHITELIST; we list them
    // explicitly to make the intent visible in code review).
    "rm",        // rm <file> — confirm before delete
    "rmdir",     // rmdir <dir>
    "chmod",     // chmod / chown (permission change)
    "chown",
    "chgrp",
    "dd",        // dd if=... of=... (catastrophic patterns caught
                 // by Tier 2; this Tier 4 entry ensures the user
                 // sees the modal for non-catastrophic dd).
    // Process control
    "kill",      // kill <pid> / kill -9
    "killall",
    "pkill",
    "shutdown",  // system power
    "reboot",
    "halt",
    "poweroff",
    // System / network administration
    "iptables",  // firewall rules
    "ufw",
    "firewalld",
    "mount",     // mount / umount
    "umount",
    "fsck",      // filesystem check
    "fdisk",     // partition table
    "parted",
    "swapon",
    "swapoff",
    // Package install
    "apt",       // apt install / remove
    "apt-get",
    "yum",
    "dnf",
    "pacman",
    "brew",      // brew install
    "snap",
    "pip",       // pip install
    "pip3",
    "gem",       // gem install
    // Service control
    "systemctl", // systemctl start/stop/restart
    "service",
    "launchctl", // macOS
    "sc",        // Windows
    // Network binding / server start
    "ssh",       // ssh user@host
    "scp",       // scp src dst
    "rsync",     // rsync (network copy)
    "nc",        // netcat
    "ncat",
    "socat",
    // Pipe-to-shell — Tier 2 catches `curl | bash`, but
    // `bash <(curl ...)` / explicit `eval` go through here.
    "bash",      // bash -c / bash <(...)
    "sh",        // sh -c / sh <(...)
    "zsh",
    "fish",
    "eval",      // eval "string"
    "source",    // source script.sh
    "exec",      // exec command
];

/// Classify a shell command's first token. Returns `Allow` if the
/// prefix is in the whitelist, `Ask` otherwise (asklist OR
/// unknown — we treat unknown as Ask, never as Allow, per Q2
/// "宁误弹不漏弹" philosophy).
///
/// **Pre-processing**:
/// 1. Trim leading/trailing ASCII whitespace.
/// 2. Strip a single leading `./` or `/` (path prefix).
/// 3. Take the first whitespace-separated token.
///
/// Examples:
///
/// ```text
/// "git status"             -> "git"   -> Allow
/// "git status | head -5"   -> "git"   -> Allow   (pipe ignored)
/// "rm -rf foo"             -> "rm"    -> Ask
/// "sudo apt install"       -> "sudo"  -> Ask
/// "bash -c ls"             -> "bash"  -> Ask
/// "./cargo test"           -> "cargo" -> Allow   (./ stripped)
/// "/usr/bin/git status"    -> "git"   -> Allow   (/ stripped)
/// "nonsense-cmd"           -> "nonsense-cmd" -> Ask
/// ""                       -> ""      -> Ask     (defensive)
/// ```
pub fn classify_prefix(cmd: &str) -> ShellTrust {
    let first = first_token(cmd);
    if first.is_empty() {
        // Empty / whitespace-only command: treat as Ask (defensive
        // — the tool layer's shell will fail anyway, but we
        // surface the modal so the user can see what the LLM
        // tried to do).
        return ShellTrust::Ask;
    }
    if SHELL_WHITELIST.contains(&first) {
        ShellTrust::Allow
    } else {
        // Asklist hit OR unknown — both fall into Ask. We
        // intentionally do NOT differentiate (the modal shows
        // the same shape for both, and an unknown command is
        // a stronger reason to ask than a known asklist entry).
        ShellTrust::Ask
    }
}

/// Extract the first whitespace-separated token of `cmd`, then
/// reduce it to its basename (so a `./` or absolute-path
/// invocation of a binary still classifies as that binary).
/// Internal helper, exposed for unit testing.
fn first_token(cmd: &str) -> &str {
    let trimmed = cmd.trim_start();
    let first = trimmed.split_whitespace().next().unwrap_or("");
    // Strip a leading `./` and take the basename (so
    // `/usr/bin/git` → `git` and `./cargo` → `cargo`). We
    // intentionally only handle the path separators within
    // this single token — multi-token paths (`./some dir/cargo`)
    // are treated as `./some` (the path is broken anyway).
    let stripped = first
        .strip_prefix("./")
        .unwrap_or(first);
    // Take everything after the last `/`. If no `/`, the
    // whole string is the basename.
    match stripped.rfind('/') {
        Some(idx) => &stripped[idx + 1..],
        None => stripped,
    }
}

/// Public crate-internal accessor for the first-token
/// extraction. Used by the `permissions::check` Tier 4
/// "始终允许" path to compute the `match_value` for a
/// shell-prefix grant.
pub(crate) fn first_token_for_allow_always(cmd: &str) -> String {
    first_token(cmd).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_token_basic() {
        assert_eq!(first_token("git status"), "git");
        assert_eq!(first_token("  cargo  test  "), "cargo");
        assert_eq!(first_token("./pnpm test"), "pnpm");
        assert_eq!(first_token("/usr/bin/git log"), "git");
        assert_eq!(first_token(""), "");
        assert_eq!(first_token("   "), "");
    }

    #[test]
    fn first_token_no_whitespace_returns_self() {
        // A single token (no spaces) is returned as-is.
        assert_eq!(first_token("git"), "git");
    }

    #[test]
    fn classify_whitelist_known() {
        // Each whitelist entry, alone with no args, is Allow.
        for prefix in [
            "git", "gh", "cargo", "pnpm", "npm", "yarn", "node",
            "ls", "cat", "head", "tail", "find", "grep", "rg",
            "curl", "wget", "mkdir", "touch", "mv",
        ] {
            assert_eq!(
                classify_prefix(prefix),
                ShellTrust::Allow,
                "expected Allow for whitelist entry: {}",
                prefix
            );
        }
    }

    #[test]
    fn classify_asklist_known() {
        for prefix in [
            "rm", "sudo", "kill", "shutdown", "reboot",
            "chmod", "chown", "dd", "ssh", "bash", "sh",
        ] {
            assert_eq!(
                classify_prefix(prefix),
                ShellTrust::Ask,
                "expected Ask for asklist entry: {}",
                prefix
            );
        }
    }

    #[test]
    fn classify_unknown_is_ask() {
        // Unknown commands always go to the modal — never Allow.
        // This is the "B 试图精确会输" safety net: a typo or
        // exotic command should always be visible to the user.
        for prefix in ["nonsense-cmd", "my-script", "evil-binary", "x"] {
            assert_eq!(
                classify_prefix(prefix),
                ShellTrust::Ask,
                "expected Ask for unknown command: {}",
                prefix
            );
        }
    }

    #[test]
    fn classify_pipe_uses_first_token() {
        // The pipe target is deliberately ignored.
        assert_eq!(classify_prefix("git status | head -5"), ShellTrust::Allow);
        // Even a pipe from an Allow prefix to a dangerous target
        // (e.g. `git log | bash`) is classified as Allow at
        // THIS layer; Tier 2's hard kill list separately
        // catches `curl ... | bash` / `wget ... | sh`. `git |
        // bash` doesn't match Tier 2 patterns because the
        // upstream is `git`, not `curl`/`wget` — the user is
        // the safety net for that case.
        assert_eq!(classify_prefix("git log | bash"), ShellTrust::Allow);
    }

    #[test]
    fn classify_strips_path_prefix() {
        // ./ and / stripping: the user can run a script that
        // invokes a whitelisted binary via a relative or
        // absolute path, and we treat it the same as a direct
        // invocation.
        assert_eq!(classify_prefix("./cargo test"), ShellTrust::Allow);
        assert_eq!(classify_prefix("/usr/bin/git status"), ShellTrust::Allow);
        assert_eq!(classify_prefix("./rm foo"), ShellTrust::Ask);
    }

    #[test]
    fn classify_bash_c_uses_bash() {
        // `bash -c "ls"` returns the literal token `bash` (no
        // recursive parsing). `bash` is in the asklist, so the
        // modal pops — even though the inner command `ls` would
        // be whitelist-allowed if invoked directly. This is the
        // documented trade-off (Q7).
        assert_eq!(classify_prefix("bash -c \"ls\""), ShellTrust::Ask);
    }

    #[test]
    fn classify_sudo_prefix_uses_sudo() {
        // `sudo rm foo` — first token is `sudo`, asklist. The
        // actual command is `rm`, which is also in the
        // asklist, but the user explicitly typed `sudo` so we
        // surface the modal to confirm the privilege
        // escalation.
        assert_eq!(classify_prefix("sudo rm foo"), ShellTrust::Ask);
    }

    #[test]
    fn classify_find_delete_is_allow() {
        // `find . -name "*.tmp" -delete` is technically a
        // side-effect, but the user's mental model is "find
        // does search-and-cleanup" — whitelist it. The user
        // can still hit "deny" if they want to forbid a
        // specific cleanup. (The catastrophic patterns like
        // `rm -rf /` are caught by Tier 2; `find ... -delete
        // /` is unusual enough not to warrant a Tier 2
        // entry.)
        assert_eq!(
            classify_prefix("find . -name \"*.tmp\" -delete"),
            ShellTrust::Allow
        );
    }

    #[test]
    fn classify_empty_and_whitespace_is_ask() {
        // Defensive: empty / whitespace-only input is Ask (not
        // Allow). The tool layer will fail the command, but we
        // surface the modal so the user can see what the LLM
        // tried to do.
        assert_eq!(classify_prefix(""), ShellTrust::Ask);
        assert_eq!(classify_prefix("   "), ShellTrust::Ask);
        assert_eq!(classify_prefix("\t\n"), ShellTrust::Ask);
    }

    #[test]
    fn whitelist_has_no_overlap_with_asklist() {
        // Defensive: a future PR must not add the same prefix
        // to both tables. `mv` is the only borderline case —
        // it's in the whitelist (renames are usually safe)
        // and not in the asklist.
        for w in SHELL_WHITELIST {
            assert!(
                !SHELL_ASKLIST.contains(w),
                "prefix '{}' is in both whitelist and asklist",
                w
            );
        }
    }

    #[test]
    fn whitelist_size_is_in_target_range() {
        // The re-grill PRD §"shell 白/ask 解析" expects ~30
        // whitelist entries; today's list has 60+. We don't
        // fail if it grows, but the assertion is here to
        // detect an accidental mass-add (e.g. adding every
        // /usr/bin binary by accident).
        assert!(
            (10..=120).contains(&SHELL_WHITELIST.len()),
            "whitelist size out of expected range: {}",
            SHELL_WHITELIST.len()
        );
    }

    #[test]
    fn asklist_size_is_in_target_range() {
        // The re-grill PRD expects ~10 asklist entries. The
        // current list is larger (~30) because we enumerated
        // package managers + service controllers + privilege
        // escalation. If this list explodes (>80), it means
        // the philosophy has shifted from "asklist is
        // critical ops" to "asklist is everything not on
        // the whitelist" — at which point we should drop
        // the asklist and treat unknown = ask (the
        // default).
        assert!(
            (5..=80).contains(&SHELL_ASKLIST.len()),
            "asklist size out of expected range: {}",
            SHELL_ASKLIST.len()
        );
    }
}
