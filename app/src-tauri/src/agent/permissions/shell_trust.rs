//! ⑨ 关 Tier 4 shell command classification (A2+B7 re-grill
//! 2026-06-13; 三档分类 2026-06-14).
//!
//! Sits between the agent loop's `provider.send()` stream and
//! `tools::execute_tool`. For every `shell` tool_use, the agent
//! loop calls [`classify_prefix`], which bins the command into
//! one of three trust levels. The **caller** (`agent/
//! permissions::check` Tier 4 Shell branch) maps each level to a
//! `Decision` that depends on the session `Mode`:
//!
//! | `ShellTrust` | Plan        | Edit        | Yolo                  |
//! |--------------|-------------|-------------|-----------------------|
//! | `ReadOnly`   | Allow       | Allow       | Allow (Tier 4 bypass) |
//! | `SideEffect` | Ask (modal) | Allow       | Allow (Tier 4 bypass) |
//! | `Ask`        | Ask (modal) | Ask (modal) | Allow (Tier 2 仍兜)   |
//!
//! ## Why three tiers, not two
//!
//! The old `Allow` / `Ask` split treated `shell` as one
//! homogenous tool. But `shell` is heterogenous: `git diff`
//! (read), `git push` (write), `ENV=x && cargo check`
//! (unknowable) all ride it. A single Allow bucket meant Plan
//! mode — *defined* as read-only analysis — had to either allow
//! `git push` or forbid `git diff`, with no middle ground, and
//! because the Mode check sat at Tier 3 it also skipped the
//! modal entirely (no "let me allow this once" path). Splitting
//! `ReadOnly` out of `Allow` lets Plan run its most-needed
//! investigation commands (`git diff` / `git status` / `ls` /
//! `cat`) silently, while everything else still reaches the
//! modal so the user can opt in per call.
//!
//! ## Classification algorithm (short-circuits top-down)
//!
//! 1. **Empty / whitespace-only** → `Ask` (defensive).
//! 2. **Structural downgrade**: the command contains `|`
//!    (covers `||`), `&&`, or `;` → `Ask`. Pipes and command
//!    chains can hide side effects behind a benign first token
//!    (`git log | bash`, `git diff && cargo build`); the
//!    first-token classifier cannot soundly classify them, so
//!    they go to the user. This also closes the old hole where
//!    `git log | bash` was classified `Allow` (first token `git`)
//!    and executed `bash` — Tier 2 only catches `curl|bash` /
//!    `wget|sh`.
//! 3. **git subcommand refinement**: first token `git` → look at
//!    the second token. Known read-only subcommands (`diff`,
//!    `log`, `status`, `show`, …) → `ReadOnly`; everything else
//!    (`push`, `commit`, `reset`, `config`, `branch`, bare
//!    `git`, …) → `SideEffect` (fail-safe: unknown git
//!    subcommands are treated as writes).
//! 4. **Generic tables**: first token in
//!    [`READ_ONLY_WHITELIST`] → `ReadOnly`; in
//!    [`SIDE_EFFECT_WHITELIST`] → `SideEffect`.
//! 5. **Default**: `Ask` (asklist entries like `rm`/`sudo` plus
//!    unknown commands — "宁误弹不漏弹").
//!
//! ## Quoting / metacharacters — deliberately not handled
//!
//! Same stance as before: the first token is taken as-is.
//! `bash -c "ls"` classifies as `bash` → `Ask`. A `|` / `&&` /
//! `;` inside a quoted argument (`grep "a|b"`) is a false
//! positive that downgrades to `Ask` — safe (the user can still
//! allow it), which is the point.
//!
//! ## Path prefix
//!
//! `./foo` and `/usr/bin/foo` are reduced to the basename `foo`
//! before lookup (see [`first_token`]).
//!
//! See `.trellis/spec/backend/tool-contract.md` §"Scenario:
//! Path-based Permission" and `docs/IMPLEMENTATION.md §4` (ADR
//! 2026-06-14) for the full contract.

/// Outcome of classifying a shell command. Three tiers — the
/// caller (`permissions::check` Tier 4 Shell branch) maps each to
/// a per-`Mode` `Decision`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellTrust {
    /// Pure read: `ls`, `cat`, `git diff`, … Allowed silently in
    /// **every** mode (Plan included). This is the tier that
    /// unblocks Plan-mode investigation.
    ReadOnly,
    /// Has side effects but is recoverable: `mkdir`, `mv`,
    /// `cargo build`, `git push`, … Silently allowed in Edit
    /// (matches the old whitelist behaviour); goes to the modal
    /// in Plan (Plan is read-only, so the user must opt in).
    SideEffect,
    /// Dangerous / unknown / structurally complex: `rm`, `sudo`,
    /// unknown commands, or anything containing `|` / `&&` / `;`.
    /// Goes to the modal in both Plan and Edit.
    Ask,
}

/// Whitelist of pure-read command prefixes. These are allowed
/// silently in every mode — including Plan — because they have
/// no recoverable side effects the user would need to gate.
const READ_ONLY_WHITELIST: &[&str] = &[
    // Directory / file inspection
    "ls",        // list dir
    "cat",       // read file
    "head",      // read file head
    "tail",      // read file tail
    "wc",        // word count
    "stat",      // file metadata
    "file",      // file type
    "find",      // find is overwhelmingly read-only; `-delete`
                 // is technically a side effect but the user
                 // recovers via worktree + git history, and Tier
                 // 2 catches catastrophic patterns separately.
    "tree",      // directory tree
    "less",      // paging
    "more",
    // Search
    "grep",      // grep inside repo
    "rg",        // ripgrep
    "ag",        // silver searcher
    "fd",        // fd (find alternative)
    // Diffing / dumping
    "diff",      // diff a.txt b.txt
    "xxd",       // hex dump
    "od",        // octal dump
    // Text processing (read-only variants; `sed -i` is in-place
    // but the overwhelmingly common case is `sed -n`. Recovery
    // via worktree + git history.)
    "sed",       // sed -n (read-only flag)
    "awk",       // awk read-only
    "cut",
    "sort",
    "uniq",
    "tr",
    // No-op / inspection builtins
    "echo",      // echo "hello" — no side effect
    "printf",
    "true",
    "false",
    "test",      // test -f / -d etc.
    "[",         // [ -f ... ]
    "pwd",       // print working dir
    "env",       // env vars (read)
    "whoami",
    "date",
    "cal",
    "uname",
    "which",
    "type",
    // Structured-data readers
    "jq",        // jq '.foo' data.json
    "yq",
    "xmllint",
];

/// Whitelist of prefixes with **recoverable** side effects
/// (project-local file mutation, build output, network egress to
/// an intentional endpoint). Allowed silently in Edit; surfaced
/// as a modal in Plan (the user opts in to a side effect while in
/// a read-only session).
///
/// Note: tools like `gh` are read/write polymorphic (`gh pr view`
/// vs `gh pr merge`) — the first token can't tell them apart, so
/// the whole tool sits in this tier. Plan mode surfaces a modal
/// (which the user can dismiss with "allow once"); Edit mirrors
/// the old whitelist and allows silently.
const SIDE_EFFECT_WHITELIST: &[&str] = &[
    // Project-local safe side effects (recoverable via worktree + git)
    "mkdir",     // mkdir -p (inside repo)
    "touch",     // touch newfile
    "cp",        // cp src dst
    "mv",        // rename a project file (the common case)
    "ln",        // ln -s
    "tar",       // tar -xzf / -czf (project archives)
    "zip",
    "unzip",
    "gzip",
    "gunzip",
    // Build & test (project-local side effects: write target/,
    // node_modules/, run arbitrary code under the project)
    "cargo",     // cargo build / test / check / run / fmt
    "rustc",     // rare direct rustc invocations
    "pnpm",      // pnpm install / run / test
    "npm",       // npm install / test / run-script
    "yarn",      // yarn install / run / test
    "bun",       // bun install / test / run
    "node",      // node script.js (project-local scripts)
    "tsc",       // tsc --noEmit (typecheck, still writes .tsbuildinfo)
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
    // VCS / DevOps — read/write polymorphic at the first token.
    "gh",        // gh pr view (read) / gh pr merge (write)
    // Network egress: interactive mode treats a `curl`/`wget` to a
    // known endpoint as intentional. Tier 2 still catches
    // `curl ... | bash` (pipe → structural downgrade to Ask first).
    "curl",      // curl https://...
    "wget",      // wget -qO- ...
];

/// Reference list of command prefixes whose side effects the user
/// should always make a per-call decision about. Kept as a
/// curated reference — `classify_prefix` does NOT branch on it;
/// anything not in the two whitelist tables already falls through
/// to `Ask`. The list is still useful for:
/// - the size/overlap tests below (catch accidental dual-list
///   additions and mass-adds);
/// - a future PR that wants different modal reason text for
///   asklist-vs-unknown.
///
/// The `dead_code` allow is intentional.
#[allow(dead_code)]
const SHELL_ASKLIST: &[&str] = &[
    // Privilege escalation
    "sudo",      // sudo anything — always confirm
    "su",        // switch user
    "doas",      // OpenBSD sudo
    // Dangerous file mutation (these are not in the whitelist
    // by being absent; we list them explicitly for visibility).
    "rm",        // rm <file> — confirm before delete
    "rmdir",     // rmdir <dir>
    "chmod",     // chmod / chown (permission change)
    "chown",
    "chgrp",
    "dd",        // dd if=... of=... (catastrophic patterns caught
                 // by Tier 2; this entry ensures the user sees the
                 // modal for non-catastrophic dd).
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

/// git subcommands that are pure reads. Used only when the first
/// token is exactly `git`. Any git subcommand NOT in this list is
/// treated as [`ShellTrust::SideEffect`] (fail-safe — we'd rather
/// over-gate a write than silently allow one in Plan mode).
///
/// Subcommands that are read-with-no-args / write-with-args
/// (`branch`, `tag`, `stash`, `remote`, `config`) are
/// deliberately NOT listed here — the classifier can't tell
/// `git branch` (read) from `git branch x` (write), so the whole
/// subcommand goes to `SideEffect`. The user still gets a modal
/// in Plan mode for these.
const GIT_READONLY_SUBCOMMANDS: &[&str] = &[
    // The high-frequency investigation set — these are the
    // commands Plan mode most needs to run.
    "diff",        // git diff [<path>]
    "log",         // git log
    "status",      // git status
    "show",        // git show <ref>
    "blame",       // git blame <file>
    "annotate",    // synonym for blame
    // Object database / refs (read-only inspection)
    "cat-file",    // git cat-file -p <ref>
    "ls-files",    // git ls-files
    "ls-tree",     // git ls-tree <ref>
    "rev-parse",   // git rev-parse <ref>
    "rev-list",    // git rev-list <ref>
    "reflog",      // git reflog
    "describe",    // git describe
    "shortlog",    // git shortlog
    "name-rev",    // git name-rev <ref>
    "for-each-ref", // git for-each-ref
    "cherry",      // git cherry (unpushed commits)
    "merge-base",  // git merge-base
    "range-diff",  // git range-diff
    // Misc read-only
    "var",         // git var GIT_AUTHOR_IDENT
    "version",     // git version
    "help",        // git help <cmd>
    "grep",        // git grep <pattern> (searches tracked files)
];

/// Classify a shell command into one of three trust tiers. See
/// the module docs for the full algorithm and the per-Mode
/// behaviour matrix.
///
/// **Pre-processing** for the first token:
/// 1. Trim leading/trailing ASCII whitespace.
/// 2. Strip a single leading `./` or `/` (path prefix) and take
///    the basename.
///
/// Examples:
///
/// ```text
/// "git diff"                -> ReadOnly   (git read subcommand)
/// "git push"                -> SideEffect (git write subcommand)
/// "ls -la"                  -> ReadOnly
/// "mkdir foo"               -> SideEffect
/// "rm foo"                  -> Ask        (asklist)
/// "git status | head -5"    -> Ask        (structural downgrade)
/// "ENV=noop && cargo check" -> Ask        (structural downgrade)
/// "bash -c ls"              -> Ask        (bash -> asklist)
/// "./cargo test"            -> SideEffect (./ stripped)
/// "/usr/bin/git diff"       -> ReadOnly   (/ stripped + git sub)
/// "nonsense-cmd"            -> Ask        (unknown)
/// ""                        -> Ask        (defensive)
/// ```
pub fn classify_prefix(cmd: &str) -> ShellTrust {
    // 1. Empty first token → Ask (defensive).
    let first = first_token(cmd);
    if first.is_empty() {
        return ShellTrust::Ask;
    }

    // 2. Structural downgrade: pipe / logical chain / sequence.
    //    The first-token classifier can't soundly classify a
    //    pipeline — `git log | bash` would otherwise be `ReadOnly`
    //    because the first token is `git`, yet `bash` runs. Route
    //    any structurally-complex command to the modal. A `|` /
    //    `&&` / `;` inside a quoted argument is a false positive,
    //    but downgrading to Ask is safe (the user can allow it).
    //
    //    `cmd.contains('|')` also covers `||` (logical or).
    if cmd.contains('|') || cmd.contains("&&") || cmd.contains(';') {
        return ShellTrust::Ask;
    }

    // 3. git subcommand refinement — git is the most common
    //    multi-valent tool, and "git diff" vs "git push" is the
    //    exact Plan-mode pain point this tier split exists to
    //    resolve.
    if first == "git" {
        return classify_git_subcommand(cmd);
    }

    // 4. Generic tables.
    if READ_ONLY_WHITELIST.contains(&first) {
        return ShellTrust::ReadOnly;
    }
    if SIDE_EFFECT_WHITELIST.contains(&first) {
        return ShellTrust::SideEffect;
    }

    // 5. Default: asklist entries + unknown → Ask.
    ShellTrust::Ask
}

/// Classify a `git <subcommand>` invocation. Reads the second
/// whitespace token of `cmd` (the subcommand after `git`).
///
/// Global git flags that precede the subcommand (`--no-pager`,
/// `-C <path>`, `-c k=v`) push the real subcommand out of slot 2,
/// so such invocations fall through to the `SideEffect` default —
/// the user sees a modal in Plan mode (fail-safe).
fn classify_git_subcommand(cmd: &str) -> ShellTrust {
    let mut tokens = cmd.split_whitespace();
    let _git = tokens.next(); // consume "git" (or "/usr/bin/git")
    let sub = tokens.next().unwrap_or("");
    if GIT_READONLY_SUBCOMMANDS.contains(&sub) {
        ShellTrust::ReadOnly
    } else {
        // Unknown / write subcommand (push, commit, reset, checkout,
        // config, branch, tag, add, merge, rebase, …) or bare `git`.
        ShellTrust::SideEffect
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
    let stripped = first.strip_prefix("./").unwrap_or(first);
    // Take everything after the last `/`. If no `/`, the
    // whole string is the basename.
    match stripped.rfind('/') {
        Some(idx) => &stripped[idx + 1..],
        None => stripped,
    }
}

/// Public crate-internal accessor for the first-token extraction.
/// Used by the `permissions::check` Tier 4 "始终允许" path to
/// compute the `match_value` for a shell-prefix grant.
pub(crate) fn first_token_for_allow_always(cmd: &str) -> String {
    first_token(cmd).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // first_token
    // -----------------------------------------------------------------

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
        assert_eq!(first_token("git"), "git");
    }

    // -----------------------------------------------------------------
    // Three-tier classification — generic tables
    // -----------------------------------------------------------------

    #[test]
    fn classify_readonly_known() {
        for prefix in [
            "ls", "cat", "head", "tail", "find", "grep", "rg",
            "diff", "tree", "pwd", "env", "jq", "sed", "awk",
        ] {
            assert_eq!(
                classify_prefix(prefix),
                ShellTrust::ReadOnly,
                "expected ReadOnly for: {}",
                prefix
            );
        }
    }

    #[test]
    fn classify_sideeffect_known() {
        for prefix in [
            "mkdir", "touch", "cp", "mv", "ln", "tar",
            "cargo", "pnpm", "npm", "node", "make", "go",
            "gh", "curl", "wget", "rustup", "pytest",
        ] {
            assert_eq!(
                classify_prefix(prefix),
                ShellTrust::SideEffect,
                "expected SideEffect for: {}",
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
    fn classify_empty_and_whitespace_is_ask() {
        assert_eq!(classify_prefix(""), ShellTrust::Ask);
        assert_eq!(classify_prefix("   "), ShellTrust::Ask);
        assert_eq!(classify_prefix("\t\n"), ShellTrust::Ask);
    }

    // -----------------------------------------------------------------
    // git subcommand refinement
    // -----------------------------------------------------------------

    #[test]
    fn classify_git_readonly_subcommands() {
        for sub in [
            "diff", "log", "status", "show", "blame", "annotate",
            "cat-file", "ls-files", "ls-tree", "rev-parse",
            "rev-list", "reflog", "describe", "grep",
        ] {
            assert_eq!(
                classify_prefix(&format!("git {}", sub)),
                ShellTrust::ReadOnly,
                "expected ReadOnly for: git {}",
                sub
            );
        }
    }

    #[test]
    fn classify_git_write_subcommands_are_sideeffect() {
        // Write / mutating subcommands fall through to SideEffect.
        for sub in [
            "push", "commit", "reset", "checkout", "merge", "rebase",
            "add", "cherry-pick", "revert", "rm", "mv", "fetch",
            "pull", "init", "clone", "stash", "tag", "branch",
            "config", "gc", "clean",
        ] {
            assert_eq!(
                classify_prefix(&format!("git {} foo", sub)),
                ShellTrust::SideEffect,
                "expected SideEffect for: git {}",
                sub
            );
        }
    }

    #[test]
    fn classify_git_with_path_args_still_readonly() {
        // Read-only subcommands keep their tier with extra args.
        assert_eq!(classify_prefix("git diff HEAD~1"), ShellTrust::ReadOnly);
        assert_eq!(classify_prefix("git log --oneline -5"), ShellTrust::ReadOnly);
        assert_eq!(classify_prefix("git status --short"), ShellTrust::ReadOnly);
    }

    #[test]
    fn classify_bare_git_is_sideeffect() {
        // `git` alone (no subcommand) → conservative SideEffect.
        assert_eq!(classify_prefix("git"), ShellTrust::SideEffect);
    }

    #[test]
    fn classify_git_global_flag_falls_to_sideeffect() {
        // A global flag like --no-pager pushes the subcommand out
        // of slot 2 → conservative SideEffect (modal in Plan).
        assert_eq!(
            classify_prefix("git --no-pager diff"),
            ShellTrust::SideEffect
        );
    }

    // -----------------------------------------------------------------
    // Structural downgrade (pipe / chain / sequence)
    // -----------------------------------------------------------------

    #[test]
    fn classify_pipe_downgrades_to_ask() {
        // Even a ReadOnly prefix piping to anything → Ask.
        // This closes the old `git log | bash` → Allow hole.
        assert_eq!(classify_prefix("git log | bash"), ShellTrust::Ask);
        assert_eq!(classify_prefix("git status | head -5"), ShellTrust::Ask);
        assert_eq!(classify_prefix("ls | grep foo"), ShellTrust::Ask);
        assert_eq!(classify_prefix("cat x | head"), ShellTrust::Ask);
    }

    #[test]
    fn classify_logical_and_downgrades_to_ask() {
        assert_eq!(
            classify_prefix("git diff && cargo build"),
            ShellTrust::Ask
        );
        // The exact pain-point the user raised: env-prefix + chain.
        assert_eq!(
            classify_prefix("ENV=noop && cargo check"),
            ShellTrust::Ask
        );
        assert_eq!(classify_prefix("ls && echo done"), ShellTrust::Ask);
    }

    #[test]
    fn classify_logical_or_downgrades_to_ask() {
        assert_eq!(classify_prefix("cargo fmt || true"), ShellTrust::Ask);
        assert_eq!(classify_prefix("git diff || echo nope"), ShellTrust::Ask);
    }

    #[test]
    fn classify_sequence_downgrades_to_ask() {
        assert_eq!(classify_prefix("cd foo; ls"), ShellTrust::Ask);
        assert_eq!(classify_prefix("echo a; echo b"), ShellTrust::Ask);
    }

    // -----------------------------------------------------------------
    // Path prefix stripping
    // -----------------------------------------------------------------

    #[test]
    fn classify_strips_path_prefix() {
        // ReadOnly via basename.
        assert_eq!(classify_prefix("/usr/bin/git diff"), ShellTrust::ReadOnly);
        assert_eq!(classify_prefix("./ls -la"), ShellTrust::ReadOnly);
        // SideEffect via basename.
        assert_eq!(classify_prefix("./cargo test"), ShellTrust::SideEffect);
        assert_eq!(classify_prefix("/usr/bin/mkdir foo"), ShellTrust::SideEffect);
        // Ask via basename.
        assert_eq!(classify_prefix("./rm foo"), ShellTrust::Ask);
    }

    #[test]
    fn classify_bash_c_uses_bash() {
        // `bash -c "ls"` → token `bash` → Ask (no recursive parse).
        assert_eq!(classify_prefix("bash -c \"ls\""), ShellTrust::Ask);
    }

    #[test]
    fn classify_sudo_prefix_uses_sudo() {
        // `sudo rm foo` → first token `sudo` → Ask.
        assert_eq!(classify_prefix("sudo rm foo"), ShellTrust::Ask);
    }

    #[test]
    fn classify_find_delete_is_readonly() {
        // `find . -name "*.tmp" -delete` is technically a side
        // effect, but `find` stays in the read-only tier (the
        // user recovers via worktree + git history; Tier 2 catches
        // the catastrophic patterns).
        assert_eq!(
            classify_prefix("find . -name \"*.tmp\" -delete"),
            ShellTrust::ReadOnly
        );
    }

    // -----------------------------------------------------------------
    // Table invariants
    // -----------------------------------------------------------------

    #[test]
    fn read_only_has_no_overlap_with_side_effect() {
        // A prefix must not appear in both whitelists.
        for ro in READ_ONLY_WHITELIST {
            assert!(
                !SIDE_EFFECT_WHITELIST.contains(ro),
                "prefix '{}' is in both READ_ONLY and SIDE_EFFECT",
                ro
            );
        }
    }

    #[test]
    fn whitelists_have_no_overlap_with_asklist() {
        for w in READ_ONLY_WHITELIST.iter().chain(SIDE_EFFECT_WHITELIST.iter()) {
            assert!(
                !SHELL_ASKLIST.contains(w),
                "prefix '{}' is in a whitelist AND the asklist",
                w
            );
        }
    }

    #[test]
    fn git_readonly_subcommands_dont_leak_into_whitelists() {
        // Defensive: "git" itself must NOT be in the generic
        // whitelists (it's handled by the subcommand path).
        assert!(!READ_ONLY_WHITELIST.contains(&"git"));
        assert!(!SIDE_EFFECT_WHITELIST.contains(&"git"));
    }

    #[test]
    fn read_only_size_is_in_target_range() {
        assert!(
            (10..=80).contains(&READ_ONLY_WHITELIST.len()),
            "READ_ONLY size out of range: {}",
            READ_ONLY_WHITELIST.len()
        );
    }

    #[test]
    fn side_effect_size_is_in_target_range() {
        assert!(
            (10..=60).contains(&SIDE_EFFECT_WHITELIST.len()),
            "SIDE_EFFECT size out of range: {}",
            SIDE_EFFECT_WHITELIST.len()
        );
    }

    #[test]
    fn asklist_size_is_in_target_range() {
        assert!(
            (5..=80).contains(&SHELL_ASKLIST.len()),
            "asklist size out of range: {}",
            SHELL_ASKLIST.len()
        );
    }
}
