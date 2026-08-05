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
//! 2. **Command substitution**: the command contains `$()` or a
//!    backtick → `Ask`. Static analysis cannot know what the
//!    substitution expands to at runtime (`echo $(rm x)` literally
//!    deletes a file at shell-expand time), so the whole thing
//!    goes to the modal.
//! 3. **Compound split (A2+ P2, 2026-07-04)**: split on top-level
//!    `;` / `&&` / `||` / `|` (quoting-aware) and classify each
//!    segment independently via [`classify_single`]; the result
//!    is the most-dangerous tier (`Ask > SideEffect > ReadOnly`).
//!    This unblocks read-only pipelines (`git diff | head` →
//!    ReadOnly) while still catching `ls; rm x` (max(ReadOnly,
//!    Ask) = Ask).
//! 4. **Per-segment** ([`classify_single`]):
//!    - git subcommand refinement (first token `git` → second-token
//!      lookup; read-only subcommands → ReadOnly, else SideEffect);
//!    - generic tables (first token in [`READ_ONLY_WHITELIST`] →
//!      ReadOnly; in [`SIDE_EFFECT_WHITELIST`] → SideEffect;
//!      otherwise Ask);
//!    - **write redirection bump** (segment contains `>` / `>>` /
//!      `&>` / `[N]>file`) bumps the segment to **at least**
//!      SideEffect. fd duplication (`2>&1` / `>&N`) and input
//!      redirection (`<` / `<<`) do NOT bump.
//!
//! ## Quoting / metacharacters
//!
//! [`split_top_level`] is quoting/escaping-aware (4-state machine:
//! Normal / Single / Double / Escaped) — a metacharacter inside
//! quotes (`grep "a|b"`) does NOT split. Command substitution
//! (`$()` / backtick) is checked BEFORE splitting (step 2), so
//! the splitter never has to recognise substitution boundaries;
//! inside quotes the check is intentionally over-eager (a literal
//! `'$()'` in single quotes still triggers Ask — fail-safe, the
//! user can allow it).
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
    /// unknown commands, or anything containing command
    /// substitution (`$()` / backtick). Goes to the modal in both
    /// Plan and Edit.
    Ask,
}

impl ShellTrust {
    /// Numeric severity used by [`max_of`] to take the most-dangerous
    /// of two tiers. `ReadOnly = 0 < SideEffect = 1 < Ask = 2`.
    /// Internal — kept as a method (not `derive(Ord)`) to avoid
    /// widening the enum's trait surface (serialization / cross-type
    /// `PartialOrd` surprises).
    fn severity(self) -> u8 {
        match self {
            ShellTrust::ReadOnly => 0,
            ShellTrust::SideEffect => 1,
            ShellTrust::Ask => 2,
        }
    }
}

/// Take the more-dangerous of two tiers (used when collapsing a
/// compound command's per-segment classifications into one tier).
/// `Ask > SideEffect > ReadOnly`.
pub(crate) fn max_of(a: ShellTrust, b: ShellTrust) -> ShellTrust {
    if a.severity() >= b.severity() {
        a
    } else {
        b
    }
}

/// Whitelist of pure-read command prefixes. These are allowed
/// silently in every mode — including Plan — because they have
/// no recoverable side effects the user would need to gate.
const READ_ONLY_WHITELIST: &[&str] = &[
    // Directory / file inspection
    "ls",   // list dir
    "cat",  // read file
    "head", // read file head
    "tail", // read file tail
    "wc",   // word count
    "stat", // file metadata
    "file", // file type
    "find", // find is overwhelmingly read-only; `-delete`
    // is technically a side effect but the user
    // recovers via worktree + git history, and Tier
    // 2 catches catastrophic patterns separately.
    "tree", // directory tree
    "less", // paging
    "more", // Search
    "grep", // grep inside repo
    "rg",   // ripgrep
    "ag",   // silver searcher
    "fd",   // fd (find alternative)
    // Diffing / dumping
    "diff", // diff a.txt b.txt
    "xxd",  // hex dump
    "od",   // octal dump
    // Text processing (read-only variants; `sed -i` is in-place
    // but the overwhelmingly common case is `sed -n`. Recovery
    // via worktree + git history.)
    "sed", // sed -n (read-only flag)
    "awk", // awk read-only
    "cut", "sort", "uniq", "tr",
    // No-op / inspection builtins
    "echo", // echo "hello" — no side effect
    "printf", "true", "false", "test", // test -f / -d etc.
    "[",    // [ -f ... ]
    "pwd",  // print working dir
    "env",  // env vars (read)
    "whoami", "date", "cal", "uname", "which", "type",
    // Structured-data readers
    "jq", // jq '.foo' data.json
    "yq", "xmllint",
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
    "mkdir", // mkdir -p (inside repo)
    "touch", // touch newfile
    "cp",    // cp src dst
    "mv",    // rename a project file (the common case)
    "ln",    // ln -s
    "tar",   // tar -xzf / -czf (project archives)
    "zip", "unzip", "gzip", "gunzip",
    // Build & test (project-local side effects: write target/,
    // node_modules/, run arbitrary code under the project)
    "cargo",  // cargo build / test / check / run / fmt
    "rustc",  // rare direct rustc invocations
    "pnpm",   // pnpm install / run / test
    "npm",    // npm install / test / run-script
    "yarn",   // yarn install / run / test
    "bun",    // bun install / test / run
    "node",   // node script.js (project-local scripts)
    "tsc",    // tsc --noEmit (typecheck, still writes .tsbuildinfo)
    "npx",    // npx <command> (project-local)
    "make",   // make <target> (project Makefile)
    "cmake",  // cmake --build
    "meson",  // meson compile
    "ninja",  // ninja <target>
    "go",     // go build / test
    "python", // python script.py
    "python3", "pytest", "rustup", // rustup update / show
    // VCS / DevOps — read/write polymorphic at the first token.
    "gh", // gh pr view (read) / gh pr merge (write)
    // Network egress: interactive mode treats a `curl`/`wget` to a
    // known endpoint as intentional. Tier 2 still catches
    // `curl ... | bash` (pipe → structural downgrade to Ask first).
    "curl", // curl https://...
    "wget", // wget -qO- ...
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
    "sudo", // sudo anything — always confirm
    "su",   // switch user
    "doas", // OpenBSD sudo
    // Dangerous file mutation (these are not in the whitelist
    // by being absent; we list them explicitly for visibility).
    "rm",    // rm <file> — confirm before delete
    "rmdir", // rmdir <dir>
    "chmod", // chmod / chown (permission change)
    "chown",
    "chgrp",
    "dd", // dd if=... of=... (catastrophic patterns caught
    // by Tier 2; this entry ensures the user sees the
    // modal for non-catastrophic dd).
    // Process control
    "kill", // kill <pid> / kill -9
    "killall",
    "pkill",
    "shutdown", // system power
    "reboot",
    "halt",
    "poweroff",
    // System / network administration
    "iptables", // firewall rules
    "ufw",
    "firewalld",
    "mount", // mount / umount
    "umount",
    "fsck",  // filesystem check
    "fdisk", // partition table
    "parted",
    "swapon",
    "swapoff",
    // Package install
    "apt", // apt install / remove
    "apt-get",
    "yum",
    "dnf",
    "pacman",
    "brew", // brew install
    "snap",
    "pip", // pip install
    "pip3",
    "gem", // gem install
    // Service control
    "systemctl", // systemctl start/stop/restart
    "service",
    "launchctl", // macOS
    "sc",        // Windows
    // Network binding / server start
    "ssh",   // ssh user@host
    "scp",   // scp src dst
    "rsync", // rsync (network copy)
    "nc",    // netcat
    "ncat",
    "socat",
    // Pipe-to-shell — Tier 2 catches `curl | bash`, but
    // `bash <(curl ...)` / explicit `eval` go through here.
    "bash", // bash -c / bash <(...)
    "sh",   // sh -c / sh <(...)
    "zsh",
    "fish",
    "eval",   // eval "string"
    "source", // source script.sh
    "exec",   // exec command
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
    "diff",     // git diff [<path>]
    "log",      // git log
    "status",   // git status
    "show",     // git show <ref>
    "blame",    // git blame <file>
    "annotate", // synonym for blame
    // Object database / refs (read-only inspection)
    "cat-file",     // git cat-file -p <ref>
    "ls-files",     // git ls-files
    "ls-tree",      // git ls-tree <ref>
    "rev-parse",    // git rev-parse <ref>
    "rev-list",     // git rev-list <ref>
    "reflog",       // git reflog
    "describe",     // git describe
    "shortlog",     // git shortlog
    "name-rev",     // git name-rev <ref>
    "for-each-ref", // git for-each-ref
    "cherry",       // git cherry (unpushed commits)
    "merge-base",   // git merge-base
    "range-diff",   // git range-diff
    // Misc read-only
    "var",     // git var GIT_AUTHOR_IDENT
    "version", // git version
    "help",    // git help <cmd>
    "grep",    // git grep <pattern> (searches tracked files)
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
/// # Compound commands (A2+ P2, 2026-07-04)
///
/// Compound commands are split on **top-level** `;` / `&&` / `||` /
/// `|` (quoting-aware) and each segment is classified independently;
/// the result is the most-dangerous tier across all segments
/// (`Ask > SideEffect > ReadOnly`). This unblocks read-only pipelines
/// (`git diff | head` → ReadOnly) while still catching
/// `ls; rm x` (max(ReadOnly, Ask) = Ask).
///
/// Command substitution (`$()` / backtick) → the whole command is
/// `Ask` (fail-safe: `echo $(rm x)` would actually delete a file;
/// the outer `echo` cannot widen the inner side effect).
///
/// Write redirection to a file (`>` / `>>` / `&>` / `[N]>file`)
/// bumps the segment to **at least** `SideEffect`. fd duplication
/// (`2>&1` / `>&N`) and input redirection (`<` / `<<`) do NOT bump
/// (no file side effect).
///
/// Examples:
///
/// ```text
/// "git diff"                -> ReadOnly   (single-segment read)
/// "git push"                -> SideEffect (single-segment write)
/// "ls -la"                  -> ReadOnly
/// "mkdir foo"               -> SideEffect
/// "rm foo"                  -> Ask        (asklist)
/// "ls | grep foo"           -> ReadOnly   (two read-only segments)
/// "git diff | head"         -> ReadOnly
/// "ls; rm x"                -> Ask        (max(ReadOnly, Ask))
/// "git diff && cargo build" -> SideEffect (max(ReadOnly, SideEffect))
/// "git diff > patch.txt"    -> SideEffect (write redirection bump)
/// "cmd 2>&1 | head"         -> ReadOnly   (fd dup does not bump)
/// "ls $(rm x)"              -> Ask        (command substitution)
/// "echo \"a;b\""            -> ReadOnly   (quoted metachar not split)
/// "ENV=noop && cargo check" -> Ask        (ENV=noop segment = Ask)
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

    // 2. Command substitution → fail-safe Ask. Static analysis
    //    cannot know what `$()` / backtick expands to at runtime
    //    (`echo $(rm x)` literally deletes a file at shell-expand
    //    time). Do NOT widen based on the outer command — route
    //    the whole thing to the modal. The check is intentionally
    //    NOT quote-aware (`'$()'` in single quotes still returns
    //    true → Ask, which is safe; the user can allow it).
    if has_command_substitution(cmd) {
        return ShellTrust::Ask;
    }

    // 3. Split on top-level `;` / `&&` / `||` / `|` (quoting-aware)
    //    and classify each segment independently; the result is the
    //    most-dangerous tier across all segments. A single-segment
    //    command (no top-level metacharacter) returns a one-element
    //    vec, so the reduction is a no-op for the common case.
    let segments = split_top_level(cmd);
    if segments.is_empty() {
        // All-whitespace compound (e.g. `;;` or `  ;  `) → defensive Ask.
        return ShellTrust::Ask;
    }
    segments
        .iter()
        .map(|seg| classify_single(seg))
        .fold(ShellTrust::ReadOnly, max_of)
}

/// `true` if `cmd` contains a structural metacharacter that should
/// defeat a Tier 4 prefix-grant short-circuit. Used by `check.rs`
/// to gate the (a) prefix-grant + worker run-grant short-circuits
/// BEFORE they fire — a user's "始终允许" on `ls` should NOT auto-allow
/// `ls; rm -rf ~/notes` even though the first token matches.
///
/// v1 deliberately NOT quote-aware (uses bare `contains`). A false
/// positive (`echo "a;b"` reports `true` → grant skipped → falls
/// through to `classify_prefix`, which re-splits accurately and
/// produces the right tier) is safe; a false negative is not, so we
/// err on the wider check. See `design.md §3.1`.
pub(crate) fn has_structural_metachar(cmd: &str) -> bool {
    cmd.contains('|') || cmd.contains("&&") || cmd.contains(';')
}

/// `true` if `cmd` contains command substitution (`$()` or backtick).
/// Static analysis cannot know what the substitution expands to at
/// runtime, so the whole command is fail-safe `Ask` regardless of
/// quoting. See `design.md §3.2`.
fn has_command_substitution(cmd: &str) -> bool {
    cmd.contains("$(") || cmd.contains('`')
}

/// Classify a single segment (no top-level `;` / `&&` / `||` / `|`).
///
/// Reuses the existing first-token algorithm (git subcommand
/// refinement + whitelist tables + default Ask) and layers the
/// write-redirection bump on top. See `design.md §3.4`.
fn classify_single(seg: &str) -> ShellTrust {
    let first = first_token(seg);
    if first.is_empty() {
        // Defensive: split_top_level filters out empty segments,
        // but a defensive Ask here keeps the function total.
        return ShellTrust::Ask;
    }

    // git subcommand refinement.
    let mut tier = if first == "git" {
        classify_git_subcommand(seg)
    } else if READ_ONLY_WHITELIST.contains(&first) {
        ShellTrust::ReadOnly
    } else if SIDE_EFFECT_WHITELIST.contains(&first) {
        ShellTrust::SideEffect
    } else {
        ShellTrust::Ask
    };

    // Write redirection to a file bumps the segment to at least
    // SideEffect (a read-only `git diff > patch.txt` is a write).
    // fd duplication (`2>&1` / `>&N`) and input redirection do NOT
    // bump (no file side effect). See `design.md §3.5`.
    if detect_write_redirect(seg) {
        tier = max_of(tier, ShellTrust::SideEffect);
    }

    tier
}

/// Split `cmd` on **top-level** `;` / `&&` / `||` / `|` into trimmed
/// segments, returning borrowed slices. Quoting / escaping is
/// respected: metacharacters inside single quotes, double quotes,
/// or after a backslash do NOT split.
///
/// Caller precondition: `cmd` has already passed
/// [`has_command_substitution`] (no `$()` / backtick), so the state
/// machine only needs to track the 4 quoting/escaping states — it
/// never has to recognise substitution boundaries.
///
/// Empty segments (e.g. `cmd ;; cmd` yields an empty segment between
/// the two `;`) are skipped. Leading / trailing whitespace on each
/// returned slice is preserved (callers that need it trimmed use
/// `first_token`, which trims itself).
///
/// State machine (design §3.3):
///
/// | State    | Trigger                          | Transition / action |
/// |----------|----------------------------------|---------------------|
/// | `Normal` | `'`                              | → `Single`          |
/// | `Normal` | `"`                              | → `Double`          |
/// | `Normal` | `\`                              | → `Escaped` (consume next char) |
/// | `Normal` | `;`                              | split point         |
/// | `Normal` | `&` and next char is `&`         | split point (consume both `&`)   |
/// | `Normal` | `\|` and next char is `\|`       | split point (consume both `\|`)  |
/// | `Normal` | `\|` (single)                    | split point (pipe)  |
/// | `Single` | `'`                              | → `Normal` (everything inside single quotes is literal, including `\`) |
/// | `Single` | any other                        | stay (no split)     |
/// | `Double` | `"`                              | → `Normal`          |
/// | `Double` | `\` and next char in `$ \` " \n` | → `Escaped` (consume next char)  |
/// | `Double` | any other                        | stay (no split)     |
/// | `Escaped`| any                              | → `Normal` (consume the char)    |
fn split_top_level(cmd: &str) -> Vec<&str> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Normal,
        Single,
        Double,
        Escaped,
    }

    let bytes = cmd.as_bytes();
    let mut segments: Vec<&str> = Vec::new();
    let mut state = State::Normal;
    let mut seg_start = 0usize;
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i];
        match state {
            State::Normal => match c {
                b'\'' => {
                    state = State::Single;
                    i += 1;
                }
                b'"' => {
                    state = State::Double;
                    i += 1;
                }
                b'\\' => {
                    state = State::Escaped;
                    i += 1;
                }
                b';' => {
                    let slice = &cmd[seg_start..i];
                    if !slice.trim().is_empty() {
                        segments.push(slice);
                    }
                    i += 1;
                    seg_start = i;
                }
                b'&' if i + 1 < bytes.len() && bytes[i + 1] == b'&' => {
                    let slice = &cmd[seg_start..i];
                    if !slice.trim().is_empty() {
                        segments.push(slice);
                    }
                    i += 2;
                    seg_start = i;
                }
                b'|' if i + 1 < bytes.len() && bytes[i + 1] == b'|' => {
                    let slice = &cmd[seg_start..i];
                    if !slice.trim().is_empty() {
                        segments.push(slice);
                    }
                    i += 2;
                    seg_start = i;
                }
                b'|' => {
                    let slice = &cmd[seg_start..i];
                    if !slice.trim().is_empty() {
                        segments.push(slice);
                    }
                    i += 1;
                    seg_start = i;
                }
                // A single `&` (not `&&`) is a bash background marker,
                // not a structural separator — leave it in the segment.
                // The first-token classifier will see it; `&` is not
                // in any whitelist, so the segment falls to Ask.
                _ => {
                    i += 1;
                }
            },
            State::Single => match c {
                b'\'' => {
                    state = State::Normal;
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            },
            State::Double => match c {
                b'"' => {
                    state = State::Normal;
                    i += 1;
                }
                b'\\' => {
                    // Inside double quotes, `\` only escapes
                    // `$` / backtick / `"` / `\` / newline; for any
                    // other char it's literal. We consume the next
                    // char regardless to keep the state machine total
                    // (a stray trailing `\` just leaves state=Escaped
                    // at end-of-string, harmless — the segment closes
                    // via the post-loop flush).
                    state = State::Escaped;
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            },
            State::Escaped => {
                state = State::Normal;
                i += 1;
            }
        }
    }
    let slice = &cmd[seg_start..bytes.len()];
    if !slice.trim().is_empty() {
        segments.push(slice);
    }
    segments
}

/// `true` if the segment writes to a file via redirection.
///
/// Bumps the segment to at least `SideEffect` in [`classify_single`].
/// Recognises:
///
/// | Form                        | Write? | Example          |
/// |-----------------------------|--------|------------------|
/// | `>file` / `> file`          | yes    | `git diff > x`   |
/// | `>>file` (append)           | yes    | `echo hi >> log` |
/// | `&>file` / `&>>` (bash)     | yes    | `cmd &> f`       |
/// | `[N]>file` (e.g. `2>err`)   | yes    | `make 2>err.log` |
/// | `>&N` / `[N]>&M` (fd dup)   | **no** | `cmd 2>&1`       |
/// | `<` / `<<` / `<<<` (input)  | **no** | `cat < /etc/x`   |
///
/// Quoting / escaping aware — a `>` inside quotes (`echo ">"`) is
/// NOT a redirection. See `design.md §3.5`.
fn detect_write_redirect(seg: &str) -> bool {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Normal,
        Single,
        Double,
        Escaped,
    }

    let bytes = seg.as_bytes();
    let mut state = State::Normal;
    let mut i = 0usize;

    while i < bytes.len() {
        let c = bytes[i];
        match state {
            State::Normal => match c {
                b'\'' => {
                    state = State::Single;
                    i += 1;
                }
                b'"' => {
                    state = State::Double;
                    i += 1;
                }
                b'\\' => {
                    state = State::Escaped;
                    i += 1;
                }
                b'>' => {
                    // We have a `>`. Classify the surrounding bytes:
                    //   - `[N]>&M` / `>&M` (fd duplication) → NOT a write
                    //   - `&>` / `&>>` (bash整体重定向) → write
                    //   - `>>` (append) → write
                    //   - bare `>` → write
                    // `&` immediately AFTER `>` followed by a digit is
                    // fd dup (`2>&1`). `&` immediately BEFORE `>` is
                    // bash overall redirect (`&>`).
                    //
                    // Check backward for a preceding `&` (bash overall
                    // redirect form `&>`) — that's a write, not a dup.
                    // Then check forward: `>&<digit>` is a dup (NOT a
                    // write); `>>` is append (write); otherwise bare `>`
                    // is a write.
                    let prev_is_amp = i > 0 && bytes[i - 1] == b'&';
                    let next_is_amp = i + 1 < bytes.len() && bytes[i + 1] == b'&';
                    if next_is_amp {
                        // `>&` — fd duplication form (e.g. `2>&1`).
                        // NOT a write to a file.
                        i += 2;
                        continue;
                    }
                    let next_is_gt = i + 1 < bytes.len() && bytes[i + 1] == b'>';
                    if next_is_gt {
                        // `>>` append → write. The previous-char check
                        // for `&` doesn't apply here (`&>>` is also a
                        // write).
                        return true;
                    }
                    // Bare `>`. If the previous char was `&` (i.e. the
                    // user wrote `&>` at the bash overall-redirect
                    // form), this is a write. We don't have to special-
                    // case it — bare `>` is a write regardless.
                    let _ = prev_is_amp;
                    return true;
                }
                _ => {
                    i += 1;
                }
            },
            State::Single => match c {
                b'\'' => {
                    state = State::Normal;
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            },
            State::Double => match c {
                b'"' => {
                    state = State::Normal;
                    i += 1;
                }
                b'\\' => {
                    state = State::Escaped;
                    i += 1;
                }
                _ => {
                    i += 1;
                }
            },
            State::Escaped => {
                state = State::Normal;
                i += 1;
            }
        }
    }
    false
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
            "ls", "cat", "head", "tail", "find", "grep", "rg", "diff", "tree", "pwd", "env", "jq",
            "sed", "awk",
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
            "mkdir", "touch", "cp", "mv", "ln", "tar", "cargo", "pnpm", "npm", "node", "make",
            "go", "gh", "curl", "wget", "rustup", "pytest",
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
            "rm", "sudo", "kill", "shutdown", "reboot", "chmod", "chown", "dd", "ssh", "bash", "sh",
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
            "diff",
            "log",
            "status",
            "show",
            "blame",
            "annotate",
            "cat-file",
            "ls-files",
            "ls-tree",
            "rev-parse",
            "rev-list",
            "reflog",
            "describe",
            "grep",
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
            "push",
            "commit",
            "reset",
            "checkout",
            "merge",
            "rebase",
            "add",
            "cherry-pick",
            "revert",
            "rm",
            "mv",
            "fetch",
            "pull",
            "init",
            "clone",
            "stash",
            "tag",
            "branch",
            "config",
            "gc",
            "clean",
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
        assert_eq!(
            classify_prefix("git log --oneline -5"),
            ShellTrust::ReadOnly
        );
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
    // Compound classification (A2+ P1+P2, 2026-07-04)
    //
    // design.md §5 断言重判表 — the old "one-size-fits-all structural
    // downgrade" tests have been re-judged per the new "split + take
    // max" semantics. Pipe / chain / sequence tests now expect the
    // most-dangerous segment tier (not blanket Ask).
    // -----------------------------------------------------------------

    #[test]
    fn classify_pipe_compound_uses_segment_max() {
        // Two read-only segments → ReadOnly (was: Ask under one-size-
        // fits-all downgrade). This is the R2 Plan-mode win.
        assert_eq!(classify_prefix("ls | grep foo"), ShellTrust::ReadOnly);
        assert_eq!(
            classify_prefix("git status | head -5"),
            ShellTrust::ReadOnly
        );
        assert_eq!(classify_prefix("cat x | head"), ShellTrust::ReadOnly);
        assert_eq!(classify_prefix("git diff | head"), ShellTrust::ReadOnly);
        assert_eq!(classify_prefix("cat x | wc -l"), ShellTrust::ReadOnly);
        // ReadOnly | Ask (bash) → Ask (still caught — kill-list /
        // Tier 2 also backs this up).
        assert_eq!(classify_prefix("git log | bash"), ShellTrust::Ask);
    }

    #[test]
    fn classify_logical_and_compound_uses_segment_max() {
        // git diff (ReadOnly) + cargo build (SideEffect) → SideEffect.
        // (Was: Ask under old blanket downgrade. Plan now sees a
        // modal — correct, the chain has a side effect.)
        assert_eq!(
            classify_prefix("git diff && cargo build"),
            ShellTrust::SideEffect
        );
        // Two ReadOnly segments → ReadOnly.
        assert_eq!(classify_prefix("ls && echo done"), ShellTrust::ReadOnly);
        // ENV=noop: first_token returns the whole `ENV=noop` token
        // (no whitespace inside), which is NOT in any whitelist → Ask.
        // max(Ask, SideEffect) = Ask.
        assert_eq!(classify_prefix("ENV=noop && cargo check"), ShellTrust::Ask);
    }

    #[test]
    fn classify_logical_or_compound_uses_segment_max() {
        // cargo fmt (SideEffect) || true (ReadOnly) → SideEffect.
        assert_eq!(classify_prefix("cargo fmt || true"), ShellTrust::SideEffect);
        // git diff (ReadOnly) || echo nope (ReadOnly) → ReadOnly.
        assert_eq!(
            classify_prefix("git diff || echo nope"),
            ShellTrust::ReadOnly
        );
    }

    #[test]
    fn classify_sequence_compound_uses_segment_max() {
        // cd is NOT in any whitelist → Ask. max(Ask, ReadOnly) = Ask.
        assert_eq!(classify_prefix("cd foo; ls"), ShellTrust::Ask);
        // Two ReadOnly segments → ReadOnly.
        assert_eq!(classify_prefix("echo a; echo b"), ShellTrust::ReadOnly);
    }

    // -----------------------------------------------------------------
    // Write redirection bump (A2+ P1, R3)
    // -----------------------------------------------------------------

    #[test]
    fn classify_write_redirect_bumps_to_sideeffect() {
        // A read-only prefix + write redirect → SideEffect (was: ReadOnly,
        // silently writing files in Plan mode — the R3 hole).
        assert_eq!(
            classify_prefix("git diff > patch.txt"),
            ShellTrust::SideEffect
        );
        assert_eq!(classify_prefix("echo hi >> log"), ShellTrust::SideEffect);
        // `echo` is ReadOnly; `&>` writes both stdout+stderr to f.
        assert_eq!(classify_prefix("echo hi &> f"), ShellTrust::SideEffect);
        // [N]>file form. `make` is SideEffect already; result stays
        // SideEffect.
        assert_eq!(classify_prefix("make 2>err.log"), ShellTrust::SideEffect);
        // Already-SideEffect prefix + redirect stays SideEffect.
        assert_eq!(classify_prefix("cargo build > log"), ShellTrust::SideEffect);
    }

    #[test]
    fn classify_fd_dup_and_input_redirect_do_not_bump() {
        // fd duplication (no file side effect) does NOT bump.
        // `echo 2>&1 | head` — both segments ReadOnly, the `2>&1` is
        // fd dup so it doesn't bump → whole command ReadOnly.
        assert_eq!(classify_prefix("echo hi 2>&1 | head"), ShellTrust::ReadOnly);
        assert_eq!(classify_prefix("echo hi 1>&2"), ShellTrust::ReadOnly);
        // Input redirection (read) does NOT bump.
        assert_eq!(classify_prefix("cat < /etc/hostname"), ShellTrust::ReadOnly);
        assert_eq!(classify_prefix("wc -l < file"), ShellTrust::ReadOnly);
    }

    #[test]
    fn classify_redirect_in_quotes_is_literal() {
        // `>` inside quotes is not a redirection.
        assert_eq!(classify_prefix("echo \">\""), ShellTrust::ReadOnly);
        assert_eq!(classify_prefix("echo \"a > b\""), ShellTrust::ReadOnly);
    }

    // -----------------------------------------------------------------
    // Command substitution → Ask (R4 fail-safe)
    // -----------------------------------------------------------------

    #[test]
    fn classify_command_substitution_is_ask() {
        assert_eq!(classify_prefix("ls $(rm x)"), ShellTrust::Ask);
        assert_eq!(classify_prefix("ls `rm x`"), ShellTrust::Ask);
        // Even `echo $(date)` (semantically read-only) is Ask in v1 —
        // fail-safe, never widen based on the outer echo.
        assert_eq!(classify_prefix("echo $(date)"), ShellTrust::Ask);
    }

    // -----------------------------------------------------------------
    // Quoting / escaping corner cases (splitter precision)
    // -----------------------------------------------------------------

    #[test]
    fn classify_quoted_metachar_does_not_split() {
        // Quoted `;` does not split — single segment `echo "a;b"` → echo
        // is ReadOnly.
        assert_eq!(classify_prefix("echo \"a;b\""), ShellTrust::ReadOnly);
        // Single-quoted variant.
        assert_eq!(classify_prefix("echo 'a;b'"), ShellTrust::ReadOnly);
        // Quoted `|` does not split.
        assert_eq!(classify_prefix("grep \"a|b\" f"), ShellTrust::ReadOnly);
        // Quoted `&&` does not split.
        assert_eq!(classify_prefix("echo \"a && b\""), ShellTrust::ReadOnly);
    }

    #[test]
    fn classify_escaped_metachar_does_not_split() {
        // `\;` outside quotes is an escaped `;` — does not split.
        assert_eq!(classify_prefix("echo a\\;b"), ShellTrust::ReadOnly);
        // `\|` outside quotes is an escaped `|`.
        assert_eq!(classify_prefix("echo a\\|b"), ShellTrust::ReadOnly);
    }

    #[test]
    fn classify_empty_segments_skipped() {
        // `a ;; b` has an empty segment between the two `;` —
        // splitter skips it. Two real segments, both ReadOnly.
        assert_eq!(classify_prefix("echo a ;; echo b"), ShellTrust::ReadOnly);
        // Leading / trailing separators with empty segments.
        assert_eq!(classify_prefix("; ls"), ShellTrust::ReadOnly);
        assert_eq!(classify_prefix("ls ;"), ShellTrust::ReadOnly);
    }

    // -----------------------------------------------------------------
    // split_top_level unit tests
    // -----------------------------------------------------------------

    #[test]
    fn split_top_level_single_segment_returns_one_slice() {
        assert_eq!(split_top_level("ls -la"), vec!["ls -la"]);
        assert_eq!(split_top_level("git diff"), vec!["git diff"]);
    }

    #[test]
    fn split_top_level_pipe_splits() {
        assert_eq!(split_top_level("ls | grep foo"), vec!["ls ", " grep foo"]);
    }

    #[test]
    fn split_top_level_logical_operators_split() {
        assert_eq!(
            split_top_level("git diff && cargo build"),
            vec!["git diff ", " cargo build"]
        );
        assert_eq!(
            split_top_level("cargo fmt || true"),
            vec!["cargo fmt ", " true"]
        );
    }

    #[test]
    fn split_top_level_sequence_splits() {
        assert_eq!(split_top_level("echo a; echo b"), vec!["echo a", " echo b"]);
    }

    #[test]
    fn split_top_level_quoted_metachar_does_not_split() {
        assert_eq!(split_top_level("echo \"a;b\""), vec!["echo \"a;b\""]);
        assert_eq!(split_top_level("grep 'a|b' f"), vec!["grep 'a|b' f"]);
    }

    #[test]
    fn split_top_level_double_quote_backslash_escapes_selectively() {
        // Inside double quotes `\` only escapes `$`, backtick, `"`,
        // `\`, newline. For `;` it's literal — but since we're inside
        // double quotes we don't split anyway. The state machine just
        // has to consume the next char and return to Double/Normal
        // without misclassifying.
        assert_eq!(split_top_level("echo \"a\\;b\""), vec!["echo \"a\\;b\""]);
    }

    #[test]
    fn split_top_level_escaped_metachar_does_not_split() {
        // `\;` outside quotes — backslash escapes the `;`.
        assert_eq!(split_top_level("echo a\\;b"), vec!["echo a\\;b"]);
        assert_eq!(split_top_level("echo a\\|b"), vec!["echo a\\|b"]);
    }

    #[test]
    fn split_top_level_empty_segments_skipped() {
        assert_eq!(
            split_top_level("echo a ;; echo b"),
            vec!["echo a ", " echo b"]
        );
        // Whitespace-only segments also skipped.
        assert_eq!(split_top_level("ls   ;   ;   pwd"), vec!["ls   ", "   pwd"]);
    }

    #[test]
    fn split_top_level_combined_operators() {
        // Mixing operators in one command.
        assert_eq!(
            split_top_level("a && b | c ; d"),
            vec!["a ", " b ", " c ", " d"]
        );
    }

    // -----------------------------------------------------------------
    // has_command_substitution
    // -----------------------------------------------------------------

    #[test]
    fn has_command_substitution_detects_dollar_paren() {
        assert!(has_command_substitution("ls $(rm x)"));
        assert!(has_command_substitution("echo $(date)"));
        assert!(has_command_substitution("a $(b) c"));
    }

    #[test]
    fn has_command_substitution_detects_backtick() {
        assert!(has_command_substitution("ls `rm x`"));
        assert!(has_command_substitution("a `b` c"));
    }

    #[test]
    fn has_command_substitution_negative_cases() {
        assert!(!has_command_substitution("ls"));
        assert!(!has_command_substitution("echo a; echo b"));
        assert!(!has_command_substitution("git diff | head"));
        // `$var` (variable expansion) does NOT trigger — different
        // syntax. (Static unknown, v1 leaves it to fail-safe else-
        // where; if a `$var` does expand to something dangerous the
        // kill-list / Modal still backs us up.)
        assert!(!has_command_substitution("echo $var"));
        // Escaped `\$\(` is literal — but fail-safe: we still detect
        // the `$(` substring and return true. (Acceptable: user can
        // allow once; better safe.)
        assert!(has_command_substitution("echo \\$(rm x)"));
    }

    // -----------------------------------------------------------------
    // has_structural_metachar (grant short-circuit gate)
    // -----------------------------------------------------------------

    #[test]
    fn has_structural_metachar_detects_pipe_and_chain() {
        assert!(has_structural_metachar("ls | grep foo"));
        assert!(has_structural_metachar("a || b"));
        assert!(has_structural_metachar("a && b"));
        assert!(has_structural_metachar("a ; b"));
    }

    #[test]
    fn has_structural_metachar_negative_cases() {
        assert!(!has_structural_metachar("ls -la"));
        assert!(!has_structural_metachar("git diff"));
        // Quoted metachar STILL reports true (v1 not quote-aware —
        // false positive is safe: grant skips, classify_prefix re-
        // splits accurately). This is by design.
        assert!(has_structural_metachar("echo \"a;b\""));
    }

    // -----------------------------------------------------------------
    // detect_write_redirect
    // -----------------------------------------------------------------

    #[test]
    fn detect_write_redirect_basic_forms() {
        assert!(detect_write_redirect("git diff > patch.txt"));
        assert!(detect_write_redirect("echo hi >> log"));
        assert!(detect_write_redirect("cmd &> f"));
        assert!(detect_write_redirect("make 2>err.log"));
        // bare `>` with space.
        assert!(detect_write_redirect("git diff >  patch.txt"));
    }

    #[test]
    fn detect_write_redirect_fd_dup_not_a_write() {
        assert!(!detect_write_redirect("cmd 2>&1"));
        assert!(!detect_write_redirect("echo hi 1>&2"));
        assert!(!detect_write_redirect("cmd >&2"));
    }

    #[test]
    fn detect_write_redirect_input_not_a_write() {
        assert!(!detect_write_redirect("cat < /etc/hostname"));
        assert!(!detect_write_redirect("wc -l < file"));
        // heredoc and here-string are reads.
        assert!(!detect_write_redirect("cat << EOF"));
        assert!(!detect_write_redirect("cat <<< word"));
    }

    #[test]
    fn detect_write_redirect_in_quotes_is_literal() {
        assert!(!detect_write_redirect("echo \">\""));
        assert!(!detect_write_redirect("echo \"a > b\""));
        assert!(!detect_write_redirect("echo '>'"));
    }

    #[test]
    fn detect_write_redirect_escaped_is_literal() {
        assert!(!detect_write_redirect("echo a\\>b"));
    }

    #[test]
    fn detect_write_redirect_no_redirect() {
        assert!(!detect_write_redirect("ls -la"));
        assert!(!detect_write_redirect("git diff"));
        assert!(!detect_write_redirect("cargo build 2>&1 | tee log"));
        // `2>&1 | tee log` — the `>` here is part of `2>&1` (fd dup),
        // not a file write. No `>file` anywhere.
    }

    // -----------------------------------------------------------------
    // ShellTrust::severity + max_of
    // -----------------------------------------------------------------

    #[test]
    fn shell_trust_severity_is_monotonic() {
        assert!(ShellTrust::ReadOnly.severity() < ShellTrust::SideEffect.severity());
        assert!(ShellTrust::SideEffect.severity() < ShellTrust::Ask.severity());
    }

    #[test]
    fn max_of_returns_more_dangerous_tier() {
        assert_eq!(
            max_of(ShellTrust::ReadOnly, ShellTrust::ReadOnly),
            ShellTrust::ReadOnly
        );
        assert_eq!(
            max_of(ShellTrust::ReadOnly, ShellTrust::SideEffect),
            ShellTrust::SideEffect
        );
        assert_eq!(
            max_of(ShellTrust::SideEffect, ShellTrust::ReadOnly),
            ShellTrust::SideEffect
        );
        assert_eq!(
            max_of(ShellTrust::ReadOnly, ShellTrust::Ask),
            ShellTrust::Ask
        );
        assert_eq!(
            max_of(ShellTrust::Ask, ShellTrust::ReadOnly),
            ShellTrust::Ask
        );
        assert_eq!(
            max_of(ShellTrust::SideEffect, ShellTrust::Ask),
            ShellTrust::Ask
        );
        assert_eq!(
            max_of(ShellTrust::Ask, ShellTrust::SideEffect),
            ShellTrust::Ask
        );
        assert_eq!(
            max_of(ShellTrust::SideEffect, ShellTrust::SideEffect),
            ShellTrust::SideEffect
        );
        assert_eq!(max_of(ShellTrust::Ask, ShellTrust::Ask), ShellTrust::Ask);
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
        assert_eq!(
            classify_prefix("/usr/bin/mkdir foo"),
            ShellTrust::SideEffect
        );
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
        for w in READ_ONLY_WHITELIST
            .iter()
            .chain(SIDE_EFFECT_WHITELIST.iter())
        {
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
