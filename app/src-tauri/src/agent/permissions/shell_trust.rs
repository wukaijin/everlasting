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
    pub(crate) fn severity(self) -> u8 {
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
pub(crate) const READ_ONLY_WHITELIST: &[&str] = &[
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
pub(crate) const SIDE_EFFECT_WHITELIST: &[&str] = &[
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
pub(crate) const SHELL_ASKLIST: &[&str] = &[
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
pub(crate) fn has_command_substitution(cmd: &str) -> bool {
    cmd.contains("$(") || cmd.contains('`')
}

/// Classify a single segment (no top-level `;` / `&&` / `||` / `|`).
///
/// Reuses the existing first-token algorithm (git subcommand
/// refinement + whitelist tables + default Ask) and layers the
/// write-redirection bump on top. See `design.md §3.4`.
pub(crate) fn classify_single(seg: &str) -> ShellTrust {
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
pub(crate) fn split_top_level(cmd: &str) -> Vec<&str> {
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
pub(crate) fn detect_write_redirect(seg: &str) -> bool {
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
pub(crate) fn classify_git_subcommand(cmd: &str) -> ShellTrust {
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
pub(crate) fn first_token(cmd: &str) -> &str {
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
