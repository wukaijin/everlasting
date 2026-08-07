//! Frontmatter parser for SKILL.md (hand-rolled, copied from B3 — scalar only).
//!
//! Copied verbatim from the pre-split `loader.rs` frontmatter cluster.
//! Pure parsing, zero I/O — the ideal standalone extraction target.

/// Frontmatter parsed from a SKILL.md (hand-rolled; scalars + a single
/// array field, same parser shape as B3 with a hand-rolled array
/// extension for `allowed-tools`). MVP fields: `name`, `description`,
/// `allowed-tools`. The array parser is a thin wrapper (~20 lines) over
/// the B3 scalar apply path: strip `[` `]`, comma split, trim, dedup.
/// See `.trellis/tasks/06-18-skill-stretches/prd.md` Stretch 1 §"parser
/// 升级决策" for the YAGNI justification (graduate to `serde_yaml_neo`
/// only when complex / multi-line fields appear).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Frontmatter {
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) allowed_tools: Vec<String>,
}

/// Parse a SKILL.md into `(frontmatter, body)`.
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
/// Rules (identical to `resource_loader::parse_frontmatter` for the
/// scalar fields, plus one extension for the array field):
/// - Opening `---` fence optional; if absent the whole file is the
///   body and `name` is derived from the parent directory by the
///   caller.
/// - Scalar keys are single-line `key: value`. Multi-line values
///   are still out of scope (a `serde_yaml_neo` swap is the
///   graduate path when a real field needs them — see
///   `resource_loader.rs:9`).
/// - One array field is supported: `allowed-tools` (or its
///   snake_case alias `allowed_tools`) — single-line `[a, b, c]`
///   only; multi-line / nested → empty + `warn!` (Stretch 1
///   tolerant parse, see `parse_allowed_tools`).
/// - Values trimmed; balanced surrounding quotes stripped; leading
///   `#` lines treated as comments.
/// - Unknown keys ignored (forward-compat).
pub(crate) fn parse_frontmatter(content: &str) -> (Frontmatter, String) {
    let lines: Vec<&str> = content.lines().collect();
    let mut fm = Frontmatter::default();

    let mut idx = 0;
    while idx < lines.len() && lines[idx].trim().is_empty() {
        idx += 1;
    }

    if idx < lines.len() && lines[idx].trim() == "---" {
        idx += 1;
        while idx < lines.len() && lines[idx].trim() != "---" {
            apply_kv(&mut fm, lines[idx]);
            idx += 1;
        }
        if idx < lines.len() && lines[idx].trim() == "---" {
            idx += 1;
        }
    } else {
        idx = 0;
    }

    let body = if idx >= lines.len() {
        String::new()
    } else {
        lines[idx..].join("\n")
    };
    (fm, body)
}

/// Apply a single `key: value` line to the frontmatter struct.
pub(crate) fn apply_kv(fm: &mut Frontmatter, line: &str) {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return;
    }
    let Some((k, v)) = line.split_once(':') else {
        return;
    };
    let k = k.trim();
    let mut v = v.trim().to_string();
    if v.len() >= 2 {
        let first = v.chars().next().unwrap();
        let last = v.chars().last().unwrap();
        if (first == '"' && last == '"') || (first == '\'' && last == '\'') {
            v = v[1..v.len() - 1].to_string();
        }
    }
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

/// Parse a single-line array value like `[read_file, grep, git_diff]`
/// into a deduplicated, trimmed Vec<String>.
///
/// Tolerant: any of `[]` / `not_an_array` / multi-line / nested → empty
/// Vec + `tracing::warn!` (mirrors B3 bad-file skip). The intent is to
/// **never** abort the whole skill load because of a malformed
/// `allowed-tools` field; the L0 listing simply omits the hint.
pub(crate) fn parse_allowed_tools(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    // Strip surrounding quotes (the B3 scalar apply path already does
    // this, but a user may write `allowed-tools: "[a, b]"` and we want
    // the array-strip to be the canonical form).
    let raw = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(raw)
        .trim();
    // Detect "looks like a single-line array" — must start with `[` and
    // end with `]`. Anything else (multi-line, nested, bare word) is
    // treated as malformed: empty Vec + warn, so the rest of the skill
    // (name, description, body) still loads.
    let inner = if let Some(stripped) = raw.strip_prefix('[') {
        match stripped.strip_suffix(']') {
            Some(s) => s,
            None => {
                tracing::warn!(
                 raw = %raw,
                 "skills: `allowed-tools` value starts with `[` but does not end with `]`; ignoring"
                );
                return Vec::new();
            }
        }
    } else {
        // No brackets at all — warn and treat as not declared. The PRD
        // explicitly says "非数组格式如多行/嵌套 → 该字段空 + warn".
        tracing::warn!(
        raw = %raw,
        "skills: `allowed-tools` is not a single-line `[a, b, c]` array; ignoring (per Stretch 1 tolerant parse)"
        );
        return Vec::new();
    };
    // Split on comma, trim each item, drop empties, dedup (preserve first
    // occurrence order — stable listing for tests + L0 block).
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for part in inner.split(',') {
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        if seen.insert(t.to_string()) {
            out.push(t.to_string());
        }
    }
    out
}
