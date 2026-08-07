//! Write safety net: sensitive-content / path deny-lists / home generalization.
//!
//! Relocated verbatim from the pre-split `memories.rs`. Applied before
//! INSERT in crud::insert_memory and before UPDATE in lifecycle::update_memory.

use super::types::{MemoryInput, MemoryInsertError, MAX_CONTENT_LEN, MAX_TITLE_LEN};

// Sensitive-content / path patterns + temp-path deny-list.
/// Sensitive-content regex. Match → reject the insert + warn.
/// Absorbed from spike-005 §4. Anchored case-insensitive;
/// `token=` is the query-param form (catches `Authorization: Bearer`
/// URL leaks), `bearer` catches the header form.
///
/// `OnceLock` would be marginally faster, but `regex::Regex::new`
/// is cheap (~µs) and only runs once per insert — the simplicity
/// of a `const &str` pattern + per-call compile wins for P1.
const SENSITIVE_PATTERN: &str = r"(?i)(api[_-]?key|secret|password|token=|bearer)";

/// Path-segment deny-list. Any path in `content` / `title` /
/// `command_pattern` / `path_globs` whose components include one
/// of these is rejected outright (the agent tried to memorize a
/// secret location). The deny-list is matched on path-component
/// equality (split on `/`), so `/home/user/.ssh/foo` matches but
/// `/home/user/.sshd-config` does NOT (false-positive avoidance).
const SENSITIVE_PATH_COMPONENTS: &[&str] = &[".ssh", ".aws", ".gnupg", "credentials", "id_rsa"];

/// Temporary-path deny-list. These paths are ephemeral (process-
/// scoped, not durable across reboots) so a memory referencing
/// them is almost certainly useless — reject.
const TEMP_PATH_PREFIXES: &[&str] = &["/tmp/", "/var/log/"];

// Safety-net helpers (generalize_home_path / find_*_path / apply_safety_net /
// validate_memory_text).
/// Generalize a `/home/<user>/...` absolute path to `~/...` so the
/// stored memory doesn't leak the local username. Only applies to
/// the `content` / `title` fields (the user-visible experience text);
/// `source_session_id` / `source_ref` are opaque ids that don't
/// carry filesystem paths.
///
/// Conservative: matches `/home/<non-empty-segment>/` and replaces
/// the prefix with `~/`. Doesn't touch `/root/` (root's home is
/// already non-identifying for a single-user dev box). Windows
/// `C:\Users\<user>\` is out of scope (WSL-first design).
fn generalize_home_path(text: &str) -> String {
    // Walk the string and replace each `/home/<seg>/` occurrence.
    // Simple scan (no regex) — the input is ≤500 chars so the
    // quadratic worst case is irrelevant.
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if text[i..].starts_with("/home/") {
            // Find the end of the username segment.
            let after_home = i + "/home/".len();
            if let Some(slash) = text[after_home..].find('/') {
                let seg_end = after_home + slash;
                let username = &text[after_home..seg_end];
                if !username.is_empty() && !username.contains('\\') {
                    out.push_str("~/");
                    i = seg_end + 1;
                    continue;
                }
            }
        }
        // Default: copy one char (preserves UTF-8 boundary).
        let ch = text[i..].chars().next().expect("non-empty slice");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Detect a sensitive path component in any path-like field of the
/// insert. Returns the first offending component (for the error
/// message) or `None` if clean.
fn find_sensitive_path(text: &str) -> Option<&'static str> {
    for component in text.split(['/', '\\']) {
        for deny in SENSITIVE_PATH_COMPONENTS {
            if component == *deny {
                return Some(deny);
            }
        }
    }
    None
}

/// Detect a temporary-path reference. Returns the matched prefix.
fn find_temporary_path(text: &str) -> Option<&'static str> {
    TEMP_PATH_PREFIXES
        .iter()
        .copied()
        .find(|p| text.contains(p))
}

/// Apply the write safety net to the caller-supplied fields.
/// Returns `Ok((generalized_title, generalized_content))` on
/// success, or the first rejection encountered. Path generalization
/// (`/home/<user>/` → `~/`) is applied to `title` + `content` on
/// the success path so the stored memory is username-agnostic.
///
/// `tags` / `path_globs` / `command_pattern` are NOT generalized
/// (they're structured fields the caller controls; path_globs is
/// a glob the recall path matches against, so generalizing it would
/// break the match). They ARE checked for sensitive-path
/// components (`/home/user/.ssh` in path_globs is still rejected).
pub(crate) fn apply_safety_net(input: &MemoryInput) -> Result<(String, String), MemoryInsertError> {
    let (title, content) = validate_memory_text(
        &input.title,
        &input.content,
        input.command_pattern.as_deref(),
        input.path_globs.as_deref(),
    )?;
    Ok((title, content))
}

/// Write-safety-net validator (07-06, am-observability-panel D2).
/// Single source of truth for the title / content / path-globs
/// write rules — shared by `insert_memory` (P1/P2 write path) and
/// `update_memory` (R4 user-edit path). Keeps the safety net from
/// drifting between the two entry points.
///
/// Returns `Ok((generalized_title, generalized_content))` on
/// success, or the first rejection encountered. Path generalization
/// (`/home/<user>/` → `~/`) is applied to `title` + `content` on
/// the success path so the stored memory is username-agnostic.
///
/// `command_pattern` and `path_globs` are optional structured
/// fields — pass `None` for the `update_memory` path (no structured
/// field edits in R4; title + content are the only editable text).
pub fn validate_memory_text(
    title: &str,
    content: &str,
    command_pattern: Option<&str>,
    path_globs: Option<&str>,
) -> Result<(String, String), MemoryInsertError> {
    // 1. Empty-value rejection (B1/2.2).
    let title_trimmed = title.trim();
    if title_trimmed.is_empty() {
        return Err(MemoryInsertError::EmptyTitle);
    }
    let content_trimmed = content.trim();
    if content_trimmed.is_empty() {
        return Err(MemoryInsertError::EmptyContent);
    }

    // 2. Length caps (B1) — DB CHECK is the backstop; reject early
    //    so the error message is actionable.
    if title.chars().count() > MAX_TITLE_LEN {
        return Err(MemoryInsertError::TitleTooLong(title.chars().count()));
    }
    if content.chars().count() > MAX_CONTENT_LEN {
        return Err(MemoryInsertError::ContentTooLong(content.chars().count()));
    }

    // 3. Sensitive-content regex (spike-005 §4). Anchored on
    //    title + content (the free-form text the LLM produces).
    let sensitive_re = regex::Regex::new(SENSITIVE_PATTERN).expect("sensitive pattern compiles");
    if sensitive_re.is_match(title) || sensitive_re.is_match(content) {
        return Err(MemoryInsertError::SensitiveContent);
    }

    // 4. Sensitive-path deny-list (2.3). Check every path-like
    //    field; reject on the first hit.
    for field in [
        title,
        content,
        command_pattern.unwrap_or(""),
        path_globs.unwrap_or(""),
    ] {
        if let Some(deny) = find_sensitive_path(field) {
            return Err(MemoryInsertError::SensitivePath(deny.to_string()));
        }
    }

    // 5. Temporary-path deny-list.
    for field in [
        title,
        content,
        command_pattern.unwrap_or(""),
        path_globs.unwrap_or(""),
    ] {
        if let Some(prefix) = find_temporary_path(field) {
            return Err(MemoryInsertError::TemporaryPath(prefix.to_string()));
        }
    }

    // 6. Path generalization (`/home/<user>/` → `~/`). Applied
    //    AFTER the deny-list checks (a path under `/home/<user>/.ssh`
    //    is rejected by step 4 before reaching here).
    let title = generalize_home_path(title);
    let content = generalize_home_path(content);

    Ok((title, content))
}
