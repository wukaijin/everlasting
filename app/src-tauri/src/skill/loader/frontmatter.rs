//! Frontmatter parser for SKILL.md (field branches only — the fence
//! loop + `key: value` normalization live in `resource_loader`'s
//! shared [`MdResource`] layer, RULE-FM-001 dedup).
//!
//! Pure parsing, zero I/O.

use crate::resource_loader::{parse_md_resource, parse_string_array, split_kv, MdResource};

/// Frontmatter parsed from a SKILL.md (scalars + a single array field,
/// same parser shape as B3 with a hand-rolled array extension for
/// `allowed-tools`). MVP fields: `name`, `description`,
/// `allowed-tools`. See `.trellis/tasks/06-18-skill-stretches/prd.md`
/// Stretch 1 §"parser 升级决策" for the YAGNI justification (graduate
/// to `serde_yaml_neo` only when complex / multi-line fields appear).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Frontmatter {
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) allowed_tools: Vec<String>,
}

/// Parse a SKILL.md into `(frontmatter, body)` — thin wrapper over the
/// shared `parse_md_resource` loop.
///
/// Format:
/// ```text
/// ---
/// name: review-pr
/// description: 当用户要求 review PR / diff 时调用。
/// ---
/// <markdown body...>
/// ```
///
/// Rules (shared loop + one array extension):
/// - Opening `---` fence optional; if absent the whole file is the
///   body and `name` is derived from the parent directory by the
///   caller.
/// - Scalar keys are single-line `key: value`. Multi-line values
///   are still out of scope (a `serde_yaml_neo` swap is the
///   graduate path when a real field needs them — see
///   `resource_loader.rs`).
/// - One array field is supported: `allowed-tools` (or its
///   snake_case alias `allowed_tools`) — single-line `[a, b, c]`
///   only; multi-line / nested → empty + `warn!` (Stretch 1
///   tolerant parse, see `parse_allowed_tools`).
/// - Values trimmed; balanced surrounding quotes stripped; leading
///   `#` lines treated as comments.
/// - Unknown keys ignored (forward-compat).
pub(crate) fn parse_frontmatter(content: &str) -> (Frontmatter, String) {
    parse_md_resource(content)
}

/// Apply a single `key: value` line to the frontmatter struct.
pub(crate) fn apply_kv(fm: &mut Frontmatter, line: &str) {
    let Some((k, v)) = split_kv(line) else {
        return;
    };
    match k {
        "name" => fm.name = Some(v),
        "description" => fm.description = Some(v),
        // Stretch 1 (declared 2026-06-18): `allowed-tools` is a
        // single-line, comma-separated array. Accept `allowed_tools`
        // (snake_case) as an alias for YAML-style flexibility. The
        // parsed list lives in `SkillResource.allowed_tools` for the
        // L0 listing hint; it is NOT enforced at execution time.
        "allowed-tools" | "allowed_tools" => {
            fm.allowed_tools = parse_allowed_tools(&v);
        }
        _ => {}
    }
}

impl MdResource for Frontmatter {
    fn apply_kv(&mut self, line: &str) {
        apply_kv(self, line)
    }
}

/// Parse a single-line array value like `[read_file, grep, git_diff]`
/// into a deduplicated, trimmed Vec<String> — thin wrapper over the
/// shared `parse_string_array` (RULE-FM-001 dedup).
///
/// Tolerant: any of `[]` / `not_an_array` / multi-line / nested → empty
/// Vec + `tracing::warn!` (mirrors B3 bad-file skip). The intent is to
/// **never** abort the whole skill load because of a malformed
/// `allowed-tools` field; the L0 listing simply omits the hint.
pub(crate) fn parse_allowed_tools(raw: &str) -> Vec<String> {
    parse_string_array(raw, "skills: `allowed-tools`")
}
