//! Subagent frontmatter 纯解析器(拆分自 loader.rs, 08-07-large-file-splitting)。
//!
//! 手写 frontmatter 解析(标量 + 单行内联数组,YAGNI,无 serde_yaml 依赖)
//! —— 与 B3 resource_loader / B4 skill/loader 同一解析哲学。

/// Frontmatter parsed from an agent `.md` file (hand-rolled; scalars
/// + one optional inline-array field `tools`, same parser shape as
/// B4 skills' `allowed-tools`).
///
/// - `tools: Option<Vec<String>>` keeps the three-way distinction
///   needed for Q2 inheritance:
///   - `None` → field not declared → inherit on override / `vec![]`
///     on brand-new.
///   - `Some(vec)` (incl. `Some(vec![])`) → use verbatim.
/// - `model: Option<String>` (task 07-03-subagent-frontmatter-model,
///   2026-07-03; superseded the prior "warn+discard" behavior
///   once the C task added the UI to set the model): the worker
///   resolves its `Arc<dyn Provider>` from the process catalog by
///   this `models.id`. Empty / whitespace-only is normalized to
///   `None` at the parse site. Format is NOT validated here — a
///   non-existent / stale id surfaces at dispatch time as a
///   catalog miss → `warn!` + parent fallback (see
///   `resolve_worker_provider`). The new C task (2026-07-03)
///   layers a DB override on top of this frontmatter `model:`
///   via `resolve_final_model` in `agent::subagent::dispatch`.
/// - `isolation: Option<bool>` (L3b, 2026-06-27) keeps the same
///   declared/not-declared distinction so a higher layer that does
///   not declare `isolation` inherits the lower layer's value (Q2
///   inheritance extended to the new field).

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Frontmatter {
    pub(crate) name: Option<String>,
    pub(crate) description: Option<String>,
    /// `None` = not declared; `Some(vec)` = declared (incl. empty).
    pub(crate) tools: Option<Vec<String>>,
    /// L3b (2026-06-27): `None` = not declared (inherit on override);
    /// `Some(true/false)` = declared, use verbatim.
    pub(crate) isolation: Option<bool>,
    /// task 07-03-subagent-frontmatter-model: `None` = not declared
    /// (worker inherits the parent provider); `Some(model_id)` = the
    /// worker resolves its `Arc<dyn Provider>` from the process
    /// catalog by this `models.id`. Empty / whitespace-only is
    /// normalized to `None` at the parse site. Format is NOT
    /// validated here — a non-existent / stale id surfaces at
    /// dispatch time as a catalog miss → `warn!` + parent fallback
    /// (see `run_subagent`).
    pub(crate) model: Option<String>,
}

// ---------------------------------------------------------------------------
// Frontmatter parser (hand-rolled, mirrors B3 scalar path + B4 array path)
// ---------------------------------------------------------------------------

/// Parse an agent `.md` into `(frontmatter, body)`.
///
/// Format:
/// ```text
/// ---
/// name: quick-lookup
/// description: 轻量级只读代码探索
/// tools: [read_file, grep, glob, list_dir]
/// model: <models.id>              # catalog key; None = inherit parent (task 07-03)
/// ---
/// <system prompt — markdown body>
/// ```
///
/// Rules (identical to `resource_loader::parse_frontmatter` for
/// scalar fields, plus the inline-array extension for `tools`):
/// - Opening `---` fence optional; if absent the whole file is the
///   body and the caller will reject it (agent requires `name`).
/// - Scalar keys are single-line `key: value`. Multi-line values
///   remain out of scope (graduate to a maintained YAML crate only
///   if a real field needs them — same YAGNI as B3/B4).
/// - One array field is supported: `tools` — single-line `[a, b, c]`
///   only; multi-line / nested / unbalanced → `None` (treated as
///   "not declared") + `warn!`. The tolerance matches B4 skills'
///   `parse_allowed_tools` decision: a malformed `tools` never
///   aborts the rest of the agent load.
/// - Values trimmed; balanced surrounding quotes stripped; leading
///   `#` lines treated as comments.
/// - Unknown keys ignored (forward-compat). `model` is matched
///   explicitly and stored (task 07-03-subagent-frontmatter-model; was
///   Q4 warn+discard when v1 used a single provider).
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
        // `tools` is a single-line, comma-separated array. The value
        // is parsed into `Option<Vec<String>>` so the precedence
        // merge can distinguish "not declared" (None) from "declared
        // empty" (Some([])). Malformed → None + warn (tolerant parse,
        // the rest of the agent still loads).
        "tools" => fm.tools = Some(parse_tools_array(&v)),
        // L3b (2026-06-27): accept `isolation: worktree` (Claude Code
        // spelling) or `isolation: true/false` and normalize to a
        // bool. Tolerant parse — an unrecognized value is treated as
        // "not declared" + warn (the rest of the agent still loads).
        "isolation" => fm.isolation = Some(parse_isolation(&v)),
        // task 07-03-subagent-frontmatter-model: `model` is now STORED
        // (previously Q4 warn+discard, when v1 used a single provider
        // and did not switch per subagent). Empty / whitespace-only →
        // None (build site treats None as "inherit parent provider").
        // No format validation here — a non-existent / stale id
        // surfaces at dispatch time as a catalog miss → warn + parent
        // fallback (see `run_subagent`).
        "model" => {
            let trimmed = v.trim();
            fm.model = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
        }
        _ => {}
    }
}

/// Parse the `isolation` frontmatter value (L3b, 2026-06-27).
///
/// Accepts (case-insensitive):
/// - `worktree` → `true` (Claude Code spelling — `isolation: worktree`)
/// - `true` / `false` → the literal bool
///
/// Any other value → `false` + `warn!` (tolerant parse — the rest of
/// the agent still loads; the user can fix the typo and the next
/// mtime-fenced read picks it up).
pub(crate) fn parse_isolation(raw: &str) -> bool {
    let v = raw.trim().to_lowercase();
    match v.as_str() {
        "worktree" | "true" => true,
        "false" | "none" | "shared" => false,
        _ => {
            tracing::warn!(
                value = %raw,
                "subagent: `isolation` value not recognized (expected `worktree` / `true` / `false`); treating as `false`"
            );
            false
        }
    }
}

/// Parse a single-line `tools` array like `[read_file, grep, glob]`
/// into a deduplicated, trimmed `Vec<String>`.
///
/// Tolerant (mirrors `skill/loader.rs::parse_allowed_tools`):
/// `not_an_array` / multi-line / nested / unbalanced brackets →
/// empty `Vec` + `warn!`. The caller wraps the result in `Some(_)`
/// so a malformed `tools: [...]` is treated as "declared empty"
/// rather than "not declared" — this is the safer default for Q2
/// (no accidental inheritance of a builtin's tool list when the
/// user clearly tried to declare their own). The empty-Vec value
/// then follows the general-purpose convention at filter time
/// (empty = full set minus structural-disabled).
pub(crate) fn parse_tools_array(raw: &str) -> Vec<String> {
    let raw = raw.trim();
    let raw = raw
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(raw)
        .trim();
    let inner = if let Some(stripped) = raw.strip_prefix('[') {
        match stripped.strip_suffix(']') {
            Some(s) => s,
            None => {
                tracing::warn!(
                    raw = %raw,
                    "subagent: `tools` value starts with `[` but does not end with `]`; treating as empty"
                );
                return Vec::new();
            }
        }
    } else {
        tracing::warn!(
            raw = %raw,
            "subagent: `tools` is not a single-line `[a, b, c]` array; treating as empty (tolerant parse)"
        );
        return Vec::new();
    };
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
