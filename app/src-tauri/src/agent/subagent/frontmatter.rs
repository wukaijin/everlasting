//! Subagent frontmatter 纯解析器(拆分自 loader.rs, 08-07-large-file-splitting)。
//!
//! 手写 frontmatter 解析(标量 + 单行内联数组,YAGNI,无 serde_yaml 依赖)
//! —— fence 循环 + `key: value` 归一化共用 `resource_loader` 的
//! [`MdResource`] 共享层(RULE-FM-001 去重),本文件只留字段分支。

use crate::resource_loader::{parse_md_resource, parse_string_array, split_kv, MdResource};

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
// Frontmatter parser (field branches only — shared loop lives in
// `resource_loader`'s MdResource layer)
// ---------------------------------------------------------------------------

/// Parse an agent `.md` into `(frontmatter, body)` — thin wrapper over
/// the shared `parse_md_resource` loop.
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

impl MdResource for Frontmatter {
    fn apply_kv(&mut self, line: &str) {
        apply_kv(self, line)
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
/// into a deduplicated, trimmed `Vec<String>` — thin wrapper over the
/// shared `parse_string_array` (RULE-FM-001 dedup).
///
/// Tolerant (mirrors skills' `parse_allowed_tools`):
/// `not_an_array` / multi-line / nested / unbalanced brackets →
/// empty `Vec` + `warn!`. The caller wraps the result in `Some(_)`
/// so a malformed `tools: [...]` is treated as "declared empty"
/// rather than "not declared" — this is the safer default for Q2
/// (no accidental inheritance of a builtin's tool list when the
/// user clearly tried to declare their own). The empty-Vec value
/// then follows the general-purpose convention at filter time
/// (empty = full set minus structural-disabled).
pub(crate) fn parse_tools_array(raw: &str) -> Vec<String> {
    parse_string_array(raw, "subagent: `tools`")
}
