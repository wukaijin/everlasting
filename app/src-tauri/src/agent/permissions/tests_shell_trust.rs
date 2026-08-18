#![cfg(test)]

use crate::agent::permissions::shell_trust::*;
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
        "mkdir", "touch", "cp", "mv", "ln", "tar", "cargo", "pnpm", "npm", "node", "make", "go",
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
    // cd is ReadOnly (subshell cwd only, 2026-08-18) → the compound
    // classifies by its other segments: max(ReadOnly, ReadOnly) =
    // ReadOnly. Pre-08-18 cd was unlisted → Ask, which dragged every
    // `cd <dir> && <readonly>` compound to the modal (session
    // 5df29977: 16 of its 21 asks were cd-headed).
    assert_eq!(classify_prefix("cd foo; ls"), ShellTrust::ReadOnly);
    // Unlisted token still escalates: max(Ask, ReadOnly) = Ask.
    assert_eq!(classify_prefix("rm foo; ls"), ShellTrust::Ask);
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
