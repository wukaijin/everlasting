//! L3d Subagent frontmatter loader — mtime-fenced scan of
//! user/project agent dirs.
//!
//! Mirrors the B3 `resource_loader` (commands) + B4 `skill/loader`
//! (skills) shape: read-through mtime fence, hand-rolled frontmatter
//! parser (scalar + single-line inline array, same YAGNI parser
//! philosophy — no `serde_yaml` dependency), precedence merge.
//!
//! Precedence (high → low): **project > user > builtin**. A user/
//! project `.md` whose `name` collides with a builtin **fully
//! overrides** the builtin (last-write-wins on a `HashMap` insert).
//! No reload command — freshness is decided at read time by the mtime
//! fence (Q1 decision, replacing the design PRD §7.2 `/reload-subagents`
//! command). Adding / editing / deleting a `.md` is picked up on the
//! next chat turn that calls `SubagentCache::list`.
//!
//! ## tools inheritance (Q2)
//!
//! `tools` is an **optional** frontmatter field. The parser keeps it
//! as `Option<Vec<String>>` so we can distinguish "not declared"
//! (`None`) from "declared empty" (`Some(vec![])`):
//! - When a `.md` overrides a same-named lower layer (builtin or
//!   user) and **does not declare `tools`** → inherit the lower
//!   layer's `def.tools` (so "only change the system prompt" costs
//!   nothing — the user does not need to copy the builtin tool list).
//! - When a `.md` declares `tools` (even `[]`) → use the declared
//!   list verbatim. `[]` follows the `general-purpose` convention
//!   ("empty = full set minus structural-disabled").
//! - A brand-new agent (no lower-layer collision) with no `tools`
//!   declaration → `vec![]` (full set minus structural-disabled).
//!
//! The inheritance is resolved during the precedence merge (low →
//! high insertion into the by-name map); see `merge_with_inheritance`.
//!
//! ## Per-file isolation
//!
//! A single bad `.md` (over-cap, non-UTF-8, missing `name`, illegal
//! `name` characters, malformed frontmatter) is skipped with a
//! `tracing::warn!` and never aborts the whole scan. Builtins are
//! always present regardless of `.md` failures (they come from
//! `builtin_subagents()`, an in-memory `&'static`).
//!
//! ## What this module does NOT do
//!
//! - Does NOT change `dispatch_subagent`'s `definition()` enum (still
//!   hardcoded `["researcher", "general-purpose"]` — PR3 adds the
//!   parallel `definition_with_cache` for the dynamic path but keeps
//!   the static `definition()` for unit tests).
//! - Does NOT change `dispatch.rs::run_subagent`'s `lookup_subagent`
//!   call site (PR3 does, threading `subagent_cache` through).
//! - Does NOT add `SubagentCache` to `AppState` (PR3).
//!
//! PR3 lights up all three; this module is the pure infrastructure +
//! unit tests that PR3 wires in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::RwLock;

use crate::agent::subagent::{builtin_subagents, SubagentDef};
use crate::memory::file::user_dir;

/// Subdirectory under both the user config dir and the project root
/// that holds custom subagent files (`*.md`). Matches the Claude Code
/// `.claude/agents/` convention but under our namespace.
const AGENTS_SUBDIR: &str = "agents";

/// `.everlasting/` project-local namespace (shared with B3 commands,
/// B4 skills, and shell output spillover). Agents live under `agents/`.
const PROJECT_NAMESPACE: &str = ".everlasting";

/// Single agent file size cap (defensive — an agent is a prompt +
/// frontmatter, not a content dump). Mirrors B3's
/// `MAX_COMMAND_FILE_SIZE` and B4's `MAX_SKILL_FILE_SIZE`.
const MAX_AGENT_FILE_SIZE: u64 = 64 * 1024; // 64 KiB

/// Where a subagent definition came from. `Plugin` overrides
/// `Project` (and `Project` overrides `User`, which overrides
/// `Builtin`) on a name collision.
///
/// Step 2.3 (`07-08-workflow-integration`): added `Plugin`
/// for `<project>/.everlasting/workflow/<wf>/agents/`. The
/// plugin layer is the highest-priority one when present —
/// workflow session agents can fully customize the worker
/// system prompt + tools by dropping a `.md` file alongside
/// the plugin's `workflow.json`. Non-workflow callers
/// (using the existing `list` / `lookup` methods) never
/// see the plugin layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentSource {
    Builtin,
    User,
    Project,
    Plugin,
}

impl SubagentSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Builtin => "builtin",
            Self::User => "user",
            Self::Project => "project",
            Self::Plugin => "plugin",
        }
    }
}

/// A loaded subagent definition + its source layer. Returned by
/// `SubagentCache::list` / `lookup` to callers (PR3 will use the
/// source tag to render `Available subagents: name (source: ...)`
/// in the dispatch_subagent tool description).
#[derive(Clone, Debug)]
pub struct LoadedSubagent {
    pub def: SubagentDef,
    pub source: SubagentSource,
}

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
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    /// `None` = not declared; `Some(vec)` = declared (incl. empty).
    tools: Option<Vec<String>>,
    /// L3b (2026-06-27): `None` = not declared (inherit on override);
    /// `Some(true/false)` = declared, use verbatim.
    isolation: Option<bool>,
    /// task 07-03-subagent-frontmatter-model: `None` = not declared
    /// (worker inherits the parent provider); `Some(model_id)` = the
    /// worker resolves its `Arc<dyn Provider>` from the process
    /// catalog by this `models.id`. Empty / whitespace-only is
    /// normalized to `None` at the parse site. Format is NOT
    /// validated here — a non-existent / stale id surfaces at
    /// dispatch time as a catalog miss → `warn!` + parent fallback
    /// (see `run_subagent`).
    model: Option<String>,
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
fn parse_frontmatter(content: &str) -> (Frontmatter, String) {
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
fn apply_kv(fm: &mut Frontmatter, line: &str) {
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
fn parse_isolation(raw: &str) -> bool {
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
fn parse_tools_array(raw: &str) -> Vec<String> {
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

/// Return `true` iff `name` is non-empty and contains only
/// `[a-zA-Z0-9_-]` (PRD §6 — `name` becomes a JSON schema enum value
/// and a filesystem stem, so any path/comment/quote-breaking char
/// must be rejected).
fn is_valid_agent_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// ---------------------------------------------------------------------------
// Directory scan
// ---------------------------------------------------------------------------

/// Resolve the user agents dir (`~/.config/everlasting/agents/`).
/// `None` if `user_dir()` is unresolvable on this platform. Matches
/// the B3 / B4 convention (single shared config root).
fn user_agents_dir() -> Option<PathBuf> {
    user_dir().map(|d| d.join(AGENTS_SUBDIR))
}

/// Resolve a project's agents dir (`<project>/.everlasting/agents/`).
fn project_agents_dir(project_path: &str) -> PathBuf {
    PathBuf::from(project_path)
        .join(PROJECT_NAMESPACE)
        .join(AGENTS_SUBDIR)
}

/// Resolve a workflow plugin's agents dir
/// (`<project>/.everlasting/workflow/<wf>/agents/`).
///
/// Step 2.3 of `07-08-workflow-integration`: the plugin
/// layer is the highest-priority one when the caller passes
/// a `workflow_name`. Non-workflow callers
/// (using the legacy `list` / `lookup` methods) never
/// consult this path. Mirrors
/// `skill::loader::plugin_skills_dir` — same
/// `.everlasting/workflow/<wf>/` root so plugin authors
/// memorize one directory shape per plugin.
fn plugin_agents_dir(workflow_name: &str, project_path: &str) -> PathBuf {
    PathBuf::from(project_path)
        .join(PROJECT_NAMESPACE)
        .join("workflow")
        .join(workflow_name)
        .join(AGENTS_SUBDIR)
}

/// Stat the `*.md` files in an agents dir, returning a path → mtime
/// map. A file's absence (deleted) or changed mtime invalidates the
/// cached scan. Missing dir → empty map. Identical fence shape to
/// `resource_loader::current_mtimes`.
async fn current_mtimes(dir: &Path) -> HashMap<PathBuf, Option<SystemTime>> {
    let mut map = HashMap::new();
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else {
        return map;
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let m = tokio::fs::metadata(&path)
            .await
            .ok()
            .and_then(|m| m.modified().ok());
        map.insert(path, m);
    }
    map
}

/// Scan a single agents directory. Bad files (over-cap, non-UTF-8,
/// missing/illegal `name`, IO) are skipped with a `warn!` — one bad
/// file never aborts the whole scan (mirrors memory / B3 / B4
/// failure tolerance).
///
/// Returns `LoadedAgentFile` (not the public `LoadedSubagent`) so the
/// precedence merge in `SubagentCache::list` can see the
/// `tools_declared` side-channel and apply Q2 inheritance.
async fn scan_dir(dir: &Path, source: SubagentSource) -> Vec<LoadedAgentFile> {
    let mut out = Vec::new();
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "subagent: read_dir failed");
            return out;
        }
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        match load_agent_file(&path, source).await {
            Ok(Some(file)) => out.push(file),
            Ok(None) => {} // skipped (reason logged inside)
            Err(e) => tracing::warn!(
                path = %path.display(),
                error = %e,
                "subagent: load failed"
            ),
        }
    }
    out
}

/// Load + parse one agent `.md`. Returns `Ok(None)` when the file is
/// deliberately skipped (over-cap / missing or illegal `name`); `Err`
/// for I/O failures.
///
/// **No file-stem fallback for `name`** (PRD R3 / Q2 — unlike B3
/// commands and B4 skills, an agent MUST declare its `name`
/// explicitly in frontmatter). This avoids surprises where a file
/// renamed in the editor silently changes the dispatch enum.
///
/// Returns `LoadedAgentFile` so the precedence merge can see whether
/// `tools` was declared (Q2 inheritance sentinel). The
/// `SubagentDef.tools` field is always populated: declared → the
/// parsed Vec (possibly empty); not declared → `vec![]` as a
/// placeholder (overwritten by inheritance at merge time).
async fn load_agent_file(
    path: &Path,
    source: SubagentSource,
) -> std::io::Result<Option<LoadedAgentFile>> {
    let meta = tokio::fs::metadata(path).await?;
    if meta.len() > MAX_AGENT_FILE_SIZE {
        tracing::warn!(
            path = %path.display(),
            size = meta.len(),
            max = MAX_AGENT_FILE_SIZE,
            "subagent: file exceeds size cap, skipping"
        );
        return Ok(None);
    }
    let content = tokio::fs::read_to_string(path).await?;
    let (fm, body) = parse_frontmatter(&content);

    // name: frontmatter `name` is REQUIRED (no stem fallback). Empty
    // or whitespace-only → skip + warn.
    let name = match fm.name.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(n) => n.to_string(),
        None => {
            tracing::warn!(
                path = %path.display(),
                "subagent: missing or empty `name` field, skipping (name is required)"
            );
            return Ok(None);
        }
    };
    if !is_valid_agent_name(&name) {
        tracing::warn!(
            path = %path.display(),
            name = %name,
            "subagent: `name` contains illegal characters (allowed: [a-zA-Z0-9_-]), skipping"
        );
        return Ok(None);
    }

    // description: missing → empty string + warn (degraded but loads).
    let description = match fm.description {
        Some(d) => d,
        None => {
            tracing::warn!(
                path = %path.display(),
                name = %name,
                "subagent: missing `description` field, falling back to empty string"
            );
            String::new()
        }
    };

    let tools_declared = fm.tools.is_some();
    let isolation_declared = fm.isolation.is_some();
    let def = SubagentDef {
        name,
        description,
        system_prompt: body,
        // Placeholder when not declared; overwritten by inheritance
        // at merge time (or kept vec![] for brand-new agents, which
        // follows the general-purpose convention: empty = full set
        // at filter_tools_for_subagent time).
        tools: fm.tools.unwrap_or_default(),
        // L3b (2026-06-27): placeholder `None` when not declared;
        // overwritten by inheritance at merge time (or kept `None`
        // for brand-new agents, which is the legacy shared-cwd
        // behavior).
        isolation: fm.isolation,
        // task 07-03-subagent-frontmatter-model: frontmatter `model:`
        // value (None = inherit parent provider). Empty / missing is
        // normalized to None at the parse site.
        model: fm.model,
    };

    Ok(Some(LoadedAgentFile {
        loaded: LoadedSubagent { def, source },
        tools_declared,
        isolation_declared,
    }))
}

/// Internal helper carrying the parser's None/Some distinction
/// through the scan + cache + merge steps. The public-facing
/// `SubagentDef` type holds only `Vec<String>` (the dispatch /
/// filter path doesn't care about declared-ness); the Q2 inheritance
/// decision in `merge_with_inheritance` reads `tools_declared` to
/// decide whether to pull tools up from a lower-priority layer.
///
/// L3b (2026-06-27): `isolation_declared` extends the same
/// inheritance semantics to the `isolation` field (a higher layer
/// that does not declare `isolation` inherits the lower layer's
/// value).
#[derive(Clone, Debug)]
struct LoadedAgentFile {
    loaded: LoadedSubagent,
    /// `true` iff the frontmatter declared `tools` (even empty).
    /// `false` iff `tools` was absent → eligible for inheritance.
    tools_declared: bool,
    /// `true` iff the frontmatter declared `isolation`. `false` iff
    /// `isolation` was absent → eligible for inheritance.
    isolation_declared: bool,
}

// ---------------------------------------------------------------------------
// SubagentCache — read-through with an mtime fence (B3 CommandCache shape)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct CachedScan {
    /// Raw scan of this layer (before precedence merge). Each entry
    /// carries its own `tools_declared` flag for the merge step.
    files: Vec<LoadedAgentFile>,
    /// path → mtime at scan time. Compared against `current_mtimes`
    /// on every read; any difference (changed mtime OR a file
    /// appearing/vanishing) triggers a re-scan.
    mtimes: HashMap<PathBuf, Option<SystemTime>>,
}

/// Process-wide cache of scanned agent dirs, held in `AppState` (PR3).
///
/// Freshness is decided at read time by an mtime fence (no background
/// watcher): each `list` stats the dir's `*.md` files, compares
/// against the cached mtimes, and re-scans only on a difference.
/// Builtins are NOT cached (they come from `builtin_subagents()` and
/// are merged in at `list` time — zero cost, always current).
///
/// Shape mirrors `resource_loader::CommandCache` and
/// `skill::loader::SkillCache` (NOT the design PRD §8.3
/// `parking_lot::Mutex` + `Arc::swap` — that was designed for a
/// `/reload-subagents` command; the mtime fence dissolves the need
/// for manual reload, so the simpler RwLock-on-Option shape wins).
///
/// Step 2.3: added `plugin: RwLock<HashMap<(project, wf), CachedScan>>`
/// for workflow-plugin agent layers. Keyed by `(project_path,
/// workflow_name)` because each plugin lives under its own project
/// dir. Non-workflow callers never touch this lock (the legacy
/// `list` / `lookup` methods don't read it).
pub struct SubagentCache {
    user: RwLock<Option<CachedScan>>,
    project: RwLock<HashMap<String, CachedScan>>,
    plugin: RwLock<HashMap<(String, String), CachedScan>>,
}

impl SubagentCache {
    pub fn arc() -> Arc<Self> {
        Arc::new(Self {
            user: RwLock::new(None),
            project: RwLock::new(HashMap::new()),
            plugin: RwLock::new(HashMap::new()),
        })
    }

    /// List user-layer agent files (mtime-fenced), with the
    /// `tools_declared` side-channel for the precedence merge.
    async fn list_user_files(&self) -> Vec<LoadedAgentFile> {
        let Some(dir) = user_agents_dir() else {
            return Vec::new();
        };
        let mut guard = self.user.write().await;
        let updated = read_through(&dir, SubagentSource::User, guard.as_ref()).await;
        let out = updated.files.clone();
        *guard = Some(updated);
        out
    }

    /// List project-layer agent files (mtime-fenced), keyed by
    /// project path.
    async fn list_project_files(&self, project_path: &str) -> Vec<LoadedAgentFile> {
        let dir = project_agents_dir(project_path);
        let mut guard = self.project.write().await;
        let cached = guard.get(project_path);
        let updated = read_through(&dir, SubagentSource::Project, cached).await;
        let out = updated.files.clone();
        guard.insert(project_path.to_string(), updated);
        out
    }

    /// List plugin-layer agent files (mtime-fenced), keyed by
    /// `(project_path, workflow_name)`. Step 2.3 of
    /// `07-08-workflow-integration`.
    ///
    /// Only called by `list_with_workflow` — non-workflow
    /// callers (using the legacy `list` / `lookup` methods)
    /// never touch this lock, so the cache stays cold for
    /// non-workflow sessions (no scan cost).
    async fn list_plugin_files(
        &self,
        project_path: &str,
        workflow_name: &str,
    ) -> Vec<LoadedAgentFile> {
        let dir = plugin_agents_dir(workflow_name, project_path);
        let mut guard = self.plugin.write().await;
        let key = (project_path.to_string(), workflow_name.to_string());
        let cached = guard.get(&key);
        let updated = read_through(&dir, SubagentSource::Plugin, cached).await;
        let out = updated.files.clone();
        guard.insert(key, updated);
        out
    }

    /// List all subagents (builtin + user + project) with precedence
    /// and Q2 tools-inheritance resolved.
    ///
    /// Precedence: **project > user > builtin** (last-write-wins on a
    /// by-name HashMap, inserted in low → high order). When a higher
    /// layer overrides a lower one and the higher `.md` did NOT
    /// declare `tools`, the lower layer's `def.tools` is inherited
    /// (Q2 — "only change the system prompt" costs nothing).
    pub async fn list(&self, project_path: &str) -> Vec<LoadedSubagent> {
        // Low → high precedence order. Each entry is (loaded, declared).
        let mut layers: Vec<Vec<LoadedAgentFile>> = Vec::with_capacity(3);

        // 1. Builtins (always present).
        let builtin_files: Vec<LoadedAgentFile> = builtin_subagents()
            .iter()
            .cloned()
            .map(|def| LoadedAgentFile {
                loaded: LoadedSubagent {
                    def,
                    source: SubagentSource::Builtin,
                },
                // Builtins always have a definitive tool list (even
                // general-purpose's empty Vec is "intentionally full
                // set"), so they count as "declared" — no inheritance
                // flows INTO a builtin from a lower layer (there is
                // no lower layer).
                tools_declared: true,
                // L3b (2026-06-27): same for `isolation` — builtins
                // always have a definitive value (general-purpose =
                // Some(true), researcher = None), so they count as
                // "declared" for inheritance purposes.
                isolation_declared: true,
            })
            .collect();
        layers.push(builtin_files);

        // 2. User `.md` layer.
        layers.push(self.list_user_files().await);

        // 3. Project `.md` layer.
        layers.push(self.list_project_files(project_path).await);

        merge_with_inheritance(layers)
    }

    /// Look up a single subagent by name (project > user > builtin).
    /// Returns a cloned `LoadedSubagent` (no lock leaks). PR3's
    /// `dispatch.rs` will replace `lookup_subagent(name)` with this.
    pub async fn lookup(&self, project_path: &str, name: &str) -> Option<LoadedSubagent> {
        self.list(project_path)
            .await
            .into_iter()
            .find(|l| l.def.name == name)
    }

    /// Workflow-aware variant of [`list`]. Consults the plugin
    /// layer first when `workflow_name` is `Some(non-empty)`,
    /// then project, then user, then builtin.
    ///
    /// Step 2.3 of `07-08-workflow-integration`: a workflow
    /// session dispatching `implementer` (or any other role
    /// the plugin defines) resolves to the plugin's `.md`
    /// when one exists, falling through to project > user >
    /// builtin otherwise. Non-workflow callers keep the
    /// legacy path (`list`); the dispatch site chooses
    /// which method to call based on `workflow_ctx`.
    ///
    /// `workflow_name = Some("")` is treated as `None`
    /// (matches the skills loader's contract — an empty
    /// plugin name is a misconfigured session, not a
    /// signal to scan some other plugin).
    pub async fn list_with_workflow(
        &self,
        project_path: &str,
        workflow_name: Option<&str>,
    ) -> Vec<LoadedSubagent> {
        let wf = workflow_name.filter(|n| !n.is_empty());
        let mut layers: Vec<Vec<LoadedAgentFile>> = Vec::with_capacity(4);

        // 1. Builtins (always present, lowest priority).
        let builtin_files: Vec<LoadedAgentFile> = builtin_subagents()
            .iter()
            .cloned()
            .map(|def| LoadedAgentFile {
                loaded: LoadedSubagent {
                    def,
                    source: SubagentSource::Builtin,
                },
                tools_declared: true,
                isolation_declared: true,
            })
            .collect();
        layers.push(builtin_files);

        // 2. User `.md` layer.
        layers.push(self.list_user_files().await);

        // 3. Project `.md` layer.
        layers.push(self.list_project_files(project_path).await);

        // 4. Plugin `.md` layer (highest priority when present).
        if let Some(wf) = wf {
            layers.push(self.list_plugin_files(project_path, wf).await);
        }

        merge_with_inheritance(layers)
    }

    /// Workflow-aware variant of [`lookup`]. See
    /// [`list_with_workflow`] for the precedence contract.
    pub async fn lookup_with_workflow(
        &self,
        project_path: &str,
        workflow_name: Option<&str>,
        name: &str,
    ) -> Option<LoadedSubagent> {
        self.list_with_workflow(project_path, workflow_name)
            .await
            .into_iter()
            .find(|l| l.def.name == name)
    }
}

// ---------------------------------------------------------------------------
// L3d / 2026-07-03: file-locating helper for the frontmatter writer
// (task 07-03-subagent-per-agent-model-ui, 阶段 2)
//
// The `set_subagent_model` IPC needs to translate `(source, name,
// project_path)` into a concrete file path so it can `write_frontmatter_model`
// the new `model:` line. Reusing the same path constants as the loader
// guarantees the writer writes to the file the loader will re-read on
// the next mtime-fenced scan — no parallel-path drift.
// ---------------------------------------------------------------------------

/// Resolve the on-disk path for a user- or project-layer agent's
/// frontmatter file. `builtin` agents have no file (the caller
/// routes the write to the DB override table instead, per
/// `design.md` §1 priority + §3 IPC source dispatch).
///
/// `project_path` is the canonical worktree path the loader uses
/// for its `<project>/.everlasting/agents/` scan key. For user
/// agents, the path is rooted at the platform `user_dir()` (see
/// `user_agents_dir` for the exact layout). The function returns
/// the path the writer will read-from / write-to; the writer
/// performs its own IO error handling.
///
/// This helper is a thin wrapper over the same private constants
/// the loader uses internally — the public surface keeps the
/// `AGENTS_SUBDIR` / `PROJECT_NAMESPACE` names out of the IPC
/// layer (the writer doesn't need to know which layout is in
/// effect; the helper picks the right one from `source`).
pub fn locate_agent_file(
    source: SubagentSource,
    name: &str,
    project_path: &str,
) -> std::io::Result<std::path::PathBuf> {
    if !is_valid_agent_name(name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid agent name: '{}'", name),
        ));
    }
    let path = match source {
        SubagentSource::Builtin => {
            // Builtin has no file path — the caller should have
            // routed to the DB override table instead. Return an
            // error so a misrouted caller surfaces as an IO error
            // (not a silent no-op).
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "builtin subagents have no file path; use the DB override instead",
            ));
        }
        SubagentSource::User => user_agents_dir()
            .map(|d| d.join(format!("{name}.md")))
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "user dir is not available on this platform",
                )
            })?,
        SubagentSource::Project => project_agents_dir(project_path).join(format!("{name}.md")),
        // Step 2.3: plugin source requires `workflow_name`
        // (the plugin's identifier) in addition to the
        // project path. The function signature stays
        // `(source, name, project_path)` — callers that
        // resolve a plugin agent must already know the
        // workflow_name and can pass it via a separate
        // helper, or just construct the path themselves
        // (the writer IPC for plugin agents hasn't landed
        // yet — `Plugin` is read-only for now, mirroring
        // the skills plugin layer's read-only contract).
        SubagentSource::Plugin => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "plugin subagents use plugin_agents_dir(<wf>, project_path); pass workflow_name explicitly",
            ));
        }
    };
    Ok(path)
}

// ---------------------------------------------------------------------------
// 2026-07-03 (task 07-03-subagent-per-agent-model-ui, 阶段 2): line-level
// frontmatter editor + IO wrapper
//
// The Settings UI's "set model" affordance writes the user's
// per-agent `model:` value back to the agent's `.md` file (for
// user / project agents; builtin agents go to the DB override
// table instead, see `subagent_overrides` + `set_subagent_model`
// IPC). Rather than introducing a YAML crate, the writer does
// LINE-LEVEL EDITS: it reads the file, locates the frontmatter
// block, and rewrites only the `model:` line. Body, comments,
// and other frontmatter keys are preserved verbatim.
// ---------------------------------------------------------------------------

/// Apply a `model:` line edit to an agent's frontmatter text.
/// Pure (no IO) so the line-level logic is unit-testable without
/// touching the filesystem.
///
/// Behavior:
/// - **`Some(mid)`** (set or replace `model`):
///   - If a `model:` line already exists inside the frontmatter
///     block, replace it with the new value.
///   - If `model:` is absent, insert a new `model: <id>` line as
///     the first line INSIDE the frontmatter block (right after
///     the opening `---` fence).
/// - **`None`** (clear `model`):
///   - If a `model:` line exists, delete it (along with its
///     trailing newline so the file doesn't gain a blank line).
///   - If `model:` is absent, the file is unchanged.
///
/// **Frontmatter detection**: the function looks for the first
/// `---\n` ... `---\n` block. If the input has no opening `---`
/// fence, the function returns `Err(String)` — a `.md` agent
/// without frontmatter is broken (the loader would have rejected
/// it for missing `name`); silently adding a fence would rewrite
/// the file structure in a way the user didn't ask for. The IPC
/// layer surfaces this as an `InvalidRequest` error.
///
/// **Preserves**: line ordering for all other frontmatter keys,
/// the body section, in-line `#` comments, and surrounding
/// whitespace (we only touch the targeted line; no
/// re-serialization of the rest).
pub fn apply_model_line(content: &str, model_id: Option<&str>) -> Result<String, String> {
    let lines: Vec<&str> = content.lines().collect();

    // Locate the opening `---` fence. Skip leading blank lines
    // so a file that starts with a blank line is still recognized.
    let mut open_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim() == "---" {
            open_idx = Some(i);
            break;
        }
        if !line.trim().is_empty() {
            // Non-blank, non-fence line before any `---` → no
            // frontmatter. Reject (don't auto-insert a fence —
            // see doc comment).
            return Err(format!(
                "agent file has no frontmatter fence (first non-blank line is `{}`)",
                line
            ));
        }
    }
    let open_idx = match open_idx {
        Some(i) => i,
        None => return Err("agent file has no frontmatter (no opening `---`)".to_string()),
    };

    // Find the closing `---` fence (search from `open_idx + 1`).
    let mut close_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate().skip(open_idx + 1) {
        if line.trim() == "---" {
            close_idx = Some(i);
            break;
        }
    }
    let close_idx = match close_idx {
        Some(i) => i,
        None => {
            return Err("agent file has unterminated frontmatter (no closing `---`)".to_string())
        }
    };

    // Scan frontmatter lines for an existing `model:` entry.
    // The check is a `starts_with` match against the trimmed line
    // (the inline whitespace + the colon; values can have any
    // shape). We capture the index so we can replace or remove
    // in place; the "remove" path also drops the trailing
    // newline so the file doesn't gain a blank line.
    let mut existing_idx: Option<usize> = None;
    for (i, line) in lines.iter().enumerate().take(close_idx).skip(open_idx + 1) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("model:") || trimmed.starts_with("model :") {
            existing_idx = Some(i);
            break;
        }
    }

    let mut new_lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();

    match (model_id, existing_idx) {
        // Set, no existing → insert as the FIRST line inside the
        // fence (right after the opening `---`). The line goes at
        // `open_idx + 1`, shifting the close_idx (and everything
        // after) by 1.
        (Some(mid), None) => {
            new_lines.insert(open_idx + 1, format!("model: {mid}"));
        }
        // Set, existing → replace in place. The trimmed line is
        // matched (model: or model :) for compatibility with the
        // tolerant parser in `apply_kv`; the new line uses the
        // canonical single-space form.
        (Some(mid), Some(idx)) => {
            new_lines[idx] = format!("model: {mid}");
        }
        // Clear, existing → drop the line. The trailing
        // newline is implicit (Vec<String> line-by-line →
        // `join("\n")`); the file's bytes are unchanged in the
        // resulting shape (a single line removal from the fence
        // interior is the desired contract).
        (None, Some(idx)) => {
            new_lines.remove(idx);
        }
        // Clear, no existing → no-op (return input verbatim).
        (None, None) => {}
    }

    // Re-attach the original line ending. `content.lines()` drops
    // a single trailing newline (Rust's `Lines` iterator is
    // documented to yield "as if the string ends with a final
    // line terminator"); the round-trip must preserve it so the
    // `write_frontmatter_model` no-op short-circuit
    // (`updated == content`) doesn't fire a spurious mtime
    // update on a no-op.
    let mut out = new_lines.join("\n");
    if content.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// IO wrapper around [`apply_model_line`]: read the file, apply
/// the line edit, write back atomically (`.tmp` + rename) so a
/// mid-write crash doesn't leave the agent file half-edited.
///
/// Errors:
/// - File missing → `Err(io::Error::NotFound)`.
/// - File present but malformed (no frontmatter / unterminated
///   fence / no `name` field) → `Err(io::Error::InvalidData)`
///   with the parser's message.
/// - IO error on the read or write path → `Err(io::Error)` (the
///   atomic write uses `.tmp` + `rename`, so partial writes don't
///   corrupt the agent file).
pub fn write_frontmatter_model(
    path: &std::path::Path,
    model_id: Option<&str>,
) -> std::io::Result<()> {
    let content = std::fs::read_to_string(path)?;
    let updated = apply_model_line(&content, model_id)
        .map_err(|msg| std::io::Error::new(std::io::ErrorKind::InvalidData, msg))?;
    // No-op short-circuit: if the line edit produced identical
    // bytes, skip the write. Avoids touching the file's mtime
    // unnecessarily (which would invalidate the loader's mtime
    // cache on a no-op round-trip).
    if updated == content {
        return Ok(());
    }
    // Atomic write: temp + rename. Same pattern as the rest of
    // the project (see `files.rs` for analogous usage).
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, updated.as_bytes())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Core mtime-fence read: stat the dir, compare against the cached
/// mtimes; on a full match return the cached clone, otherwise re-scan.
async fn read_through(
    dir: &Path,
    source: SubagentSource,
    cached: Option<&CachedScan>,
) -> CachedScan {
    let current = current_mtimes(dir).await;
    if let Some(c) = cached {
        if current == c.mtimes {
            return c.clone();
        }
    }
    let files = scan_dir(dir, source).await;
    CachedScan {
        files,
        mtimes: current,
    }
}

/// Merge per-layer scans into a single de-duplicated Vec with
/// precedence (project > user > builtin) and Q2 tools-inheritance.
///
/// Insertion order is **low → high precedence**: builtin first, then
/// user, then project. A higher layer colliding on `name` overwrites
/// the lower entry. If the higher layer did NOT declare `tools`
/// (`tools_declared == false`), it inherits the lower entry's
/// `def.tools` before overwriting. A brand-new name with no
/// declaration keeps `vec![]` (the general-purpose convention).
///
/// L3b (2026-06-27): the same inheritance rule extends to `isolation`
/// — a higher layer that does NOT declare `isolation` inherits the
/// lower layer's `def.isolation`. The two fields are independent
/// (a user can override one without touching the other).
fn merge_with_inheritance(layers: Vec<Vec<LoadedAgentFile>>) -> Vec<LoadedSubagent> {
    let mut by_name: HashMap<String, LoadedAgentFile> = HashMap::new();
    for layer in layers {
        for file in layer {
            let name = file.loaded.def.name.clone();
            let mut merged = file.loaded.clone();
            let mut tools_declared = file.tools_declared;
            let mut isolation_declared = file.isolation_declared;
            // Inherit from the lower layer for any field this layer
            // did NOT declare.
            if let Some(lower) = by_name.get(&name) {
                if !file.tools_declared {
                    merged.def.tools = lower.loaded.def.tools.clone();
                    tools_declared = false; // stays inheritable for the next layer
                }
                if !file.isolation_declared {
                    merged.def.isolation = lower.loaded.def.isolation;
                    isolation_declared = false;
                }
            }
            by_name.insert(
                name,
                LoadedAgentFile {
                    loaded: merged,
                    tools_declared,
                    isolation_declared,
                },
            );
        }
    }
    let mut out: Vec<LoadedSubagent> = by_name.into_values().map(|f| f.loaded).collect();
    out.sort_by(|a, b| a.def.name.cmp(&b.def.name));
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::file::set_user_dir_for_test;

    /// Write `<dir>/<name>.md` with the given body, returning the path.
    fn write_agent(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(format!("{name}.md"));
        std::fs::write(&path, body).unwrap();
        path
    }

    // ---- frontmatter parser ----

    #[test]
    fn frontmatter_full_with_tools_array() {
        let input = "---\nname: quick-lookup\ndescription: 轻量级\ntools: [read_file, grep, glob]\n---\nYou are a quick-lookup subagent.";
        let (fm, body) = parse_frontmatter(input);
        assert_eq!(fm.name.as_deref(), Some("quick-lookup"));
        assert_eq!(fm.description.as_deref(), Some("轻量级"));
        assert_eq!(
            fm.tools,
            Some(vec![
                "read_file".to_string(),
                "grep".to_string(),
                "glob".to_string(),
            ])
        );
        assert_eq!(body, "You are a quick-lookup subagent.");
    }

    #[test]
    fn frontmatter_tools_absent_is_none() {
        // No tools field at all → None (the Q2 sentinel for inheritance).
        let (fm, _) = parse_frontmatter("---\nname: x\ndescription: y\n---\nb");
        assert!(fm.tools.is_none());
    }

    #[test]
    fn frontmatter_tools_empty_array_is_some_empty() {
        // Explicit `tools: []` → Some(vec![]) (distinct from None).
        let (fm, _) = parse_frontmatter("---\nname: x\ntools: []\n---\nb");
        assert_eq!(fm.tools, Some(Vec::new()));
    }

    #[test]
    fn frontmatter_tools_dedup_and_trim() {
        let (fm, _) = parse_frontmatter("---\nname: x\ntools: [a, a, b , c,  b]\n---\nb");
        assert_eq!(
            fm.tools,
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string(),])
        );
    }

    #[test]
    fn frontmatter_tools_strips_quotes() {
        let (fm, _) = parse_frontmatter("---\nname: x\ntools: \"[a, b]\"\n---\nb");
        assert_eq!(fm.tools, Some(vec!["a".to_string(), "b".to_string()]));
    }

    #[test]
    fn frontmatter_tools_unbalanced_brackets_is_empty() {
        // `[a, b` (no closing]) → tolerant parse → Some(vec![]).
        let (fm, _) = parse_frontmatter("---\nname: x\ntools: [a, b\n---\nb");
        assert_eq!(fm.tools, Some(Vec::new()));
    }

    #[test]
    fn frontmatter_tools_not_an_array_is_empty() {
        // `tools: read_file, grep` (no brackets) → Some(vec![]).
        let (fm, _) = parse_frontmatter("---\nname: x\ntools: read_file, grep\n---\nb");
        assert_eq!(fm.tools, Some(Vec::new()));
    }

    // ---- frontmatter `model` field (task 07-03-subagent-frontmatter-model) ----
    // Previously Q4 warn+discard; now STORED so the worker can resolve
    // its provider from the catalog by `models.id`.

    #[test]
    fn frontmatter_model_field_is_stored() {
        let (fm, _) =
            parse_frontmatter("---\nname: x\nmodel: 550e8400-e29b-41d4-a716-446655440000\n---\nb");
        assert_eq!(
            fm.model.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
    }

    #[test]
    fn frontmatter_model_absent_is_none() {
        let (fm, _) = parse_frontmatter("---\nname: x\n---\nb");
        assert!(fm.model.is_none());
    }

    #[test]
    fn frontmatter_model_empty_is_none() {
        // `model:` with empty value → None (worker inherits parent).
        let (fm, _) = parse_frontmatter("---\nname: x\nmodel: \n---\nb");
        assert!(fm.model.is_none());
    }

    #[test]
    fn frontmatter_model_is_trimmed() {
        // Surrounding whitespace trimmed at parse time.
        let (fm, _) = parse_frontmatter("---\nname: x\nmodel:  abc-123  \n---\nb");
        assert_eq!(fm.model.as_deref(), Some("abc-123"));
    }

    #[test]
    fn frontmatter_strips_quotes_on_scalars() {
        let (fm, _) = parse_frontmatter("---\nname: \"q\"\ndescription: 's'\n---\nb");
        assert_eq!(fm.name.as_deref(), Some("q"));
        assert_eq!(fm.description.as_deref(), Some("s"));
    }

    #[test]
    fn frontmatter_partial_keys() {
        let (fm, body) = parse_frontmatter("---\nname: only\n---\nbody");
        assert_eq!(fm.name.as_deref(), Some("only"));
        assert!(fm.description.is_none());
        assert!(fm.tools.is_none());
        assert_eq!(body, "body");
    }

    #[test]
    fn frontmatter_no_fence_whole_file_is_body() {
        let input = "no frontmatter\njust body";
        let (fm, body) = parse_frontmatter(input);
        assert!(fm.name.is_none());
        assert_eq!(body, input);
    }

    #[test]
    fn frontmatter_unknown_keys_ignored() {
        let mut fm = Frontmatter::default();
        apply_kv(&mut fm, "# comment");
        apply_kv(&mut fm, "");
        apply_kv(&mut fm, "weird: x");
        apply_kv(&mut fm, "name: real");
        assert_eq!(fm.name.as_deref(), Some("real"));
        assert!(fm.description.is_none());
    }

    #[test]
    fn frontmatter_model_field_is_stored_for_dispatch() {
        // 2026-07-03 (task 07-03-subagent-per-agent-model-ui): the
        // `model` field is now STORED on `Frontmatter` (previously
        // warn+discard; see `frontmatter_model_field_is_stored`
        // above for the new contract). The dispatch path
        // (`resolve_final_model` + `resolve_worker_provider`) reads
        // it; an invalid `models.id` surfaces as a catalog miss
        // at dispatch time (defensive: format is not validated at
        // parse time, per the doc comment on `Frontmatter.model`).
        // This test pins that the field is read+stored — the
        // pre-C "warn+discard" behavior is gone.
        let (fm, _) = parse_frontmatter("---\nname: x\nmodel: claude-sonnet-4-6\n---\nb");
        assert_eq!(fm.name.as_deref(), Some("x"));
        assert_eq!(fm.model.as_deref(), Some("claude-sonnet-4-6"));
    }

    // ---- name validation ----

    #[test]
    fn valid_name_alphanumeric_dashes_underscores() {
        assert!(is_valid_agent_name("researcher"));
        assert!(is_valid_agent_name("quick-lookup"));
        assert!(is_valid_agent_name("db_migrator"));
        assert!(is_valid_agent_name("agent-123"));
    }

    #[test]
    fn invalid_name_rejects_path_chars() {
        assert!(!is_valid_agent_name("a/b"));
        assert!(!is_valid_agent_name("a\\b"));
        assert!(!is_valid_agent_name("a:b"));
        assert!(!is_valid_agent_name("a.b"));
        assert!(!is_valid_agent_name("a b"));
        assert!(!is_valid_agent_name(""));
    }

    // ---- directory scan ----

    #[tokio::test]
    async fn scan_parses_valid_files_ignores_non_md() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_agent(
            tmp.path(),
            "alpha",
            "---\nname: alpha\ndescription: d\n---\nbody1",
        );
        write_agent(tmp.path(), "beta", "---\nname: beta\n---\nbody2");
        std::fs::write(tmp.path().join("readme.txt"), "x").unwrap();

        let mut files = scan_dir(tmp.path(), SubagentSource::User).await;
        files.sort_by(|a, b| a.loaded.def.name.cmp(&b.loaded.def.name));
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].loaded.def.name, "alpha");
        assert_eq!(files[0].loaded.def.description, "d");
        assert_eq!(files[1].loaded.def.name, "beta");
        assert_eq!(files[1].loaded.def.description, "");
    }

    #[tokio::test]
    async fn scan_missing_name_skips_with_warn() {
        // No `name` in frontmatter → skip (no stem fallback).
        let tmp = tempfile::TempDir::new().unwrap();
        write_agent(tmp.path(), "noname", "---\ndescription: d\n---\nb");
        let files = scan_dir(tmp.path(), SubagentSource::User).await;
        assert!(files.is_empty());
    }

    #[tokio::test]
    async fn scan_illegal_name_skips_with_warn() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_agent(tmp.path(), "bad", "---\nname: a/b\ndescription: d\n---\nb");
        let files = scan_dir(tmp.path(), SubagentSource::User).await;
        assert!(files.is_empty());
    }

    #[tokio::test]
    async fn scan_skips_over_cap_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let big = "x".repeat((MAX_AGENT_FILE_SIZE + 1) as usize);
        std::fs::write(tmp.path().join("big.md"), big).unwrap();
        assert!(scan_dir(tmp.path(), SubagentSource::User).await.is_empty());
    }

    #[tokio::test]
    async fn scan_missing_dir_returns_empty() {
        let files = scan_dir(
            Path::new("/no/such/everlasting/agents/xyz"),
            SubagentSource::User,
        )
        .await;
        assert!(files.is_empty());
    }

    #[tokio::test]
    async fn scan_per_file_isolation_one_bad_does_not_block_others() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_agent(tmp.path(), "good", "---\nname: good\n---\nb");
        write_agent(tmp.path(), "bad", "---\nname: x/y\n---\nb");
        write_agent(tmp.path(), "good2", "---\nname: good2\n---\nb");
        let mut files = scan_dir(tmp.path(), SubagentSource::Project).await;
        files.sort_by(|a, b| a.loaded.def.name.cmp(&b.loaded.def.name));
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].loaded.def.name, "good");
        assert_eq!(files[1].loaded.def.name, "good2");
        assert_eq!(files[0].loaded.source, SubagentSource::Project);
    }

    // ---- mtime fence ----

    #[tokio::test]
    async fn read_through_re_scans_on_change() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        write_agent(&dir, "a", "---\nname: a\n---\nv1");
        let cached = read_through(&dir, SubagentSource::User, None).await;
        assert_eq!(cached.files[0].loaded.def.system_prompt, "v1");

        // Unchanged → cache hit.
        let hit = read_through(&dir, SubagentSource::User, Some(&cached)).await;
        assert_eq!(hit.mtimes, cached.mtimes);

        // Change content + advance mtime → re-scan sees new body.
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        std::fs::write(dir.join("a.md"), "---\nname: a\n---\nv2").unwrap();
        let updated = read_through(&dir, SubagentSource::User, Some(&cached)).await;
        assert_eq!(updated.files[0].loaded.def.system_prompt, "v2");
    }

    #[tokio::test]
    async fn read_through_re_scans_on_file_added() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        write_agent(&dir, "a", "---\nname: a\n---\nb");
        let cached = read_through(&dir, SubagentSource::User, None).await;
        assert_eq!(cached.files.len(), 1);

        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        write_agent(&dir, "b", "---\nname: b\n---\nb");
        let updated = read_through(&dir, SubagentSource::User, Some(&cached)).await;
        assert_eq!(updated.files.len(), 2);
    }

    #[tokio::test]
    async fn read_through_re_scans_on_file_deleted() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        write_agent(&dir, "a", "---\nname: a\n---\nb");
        write_agent(&dir, "b", "---\nname: b\n---\nb");
        let cached = read_through(&dir, SubagentSource::User, None).await;
        assert_eq!(cached.files.len(), 2);

        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        std::fs::remove_file(dir.join("a.md")).unwrap();
        let updated = read_through(&dir, SubagentSource::User, Some(&cached)).await;
        assert_eq!(updated.files.len(), 1);
        assert_eq!(updated.files[0].loaded.def.name, "b");
    }

    // ---- merge_with_inheritance (Q2) ----

    fn loaded(
        name: &str,
        tools: Vec<String>,
        declared: bool,
        source: SubagentSource,
    ) -> LoadedAgentFile {
        LoadedAgentFile {
            loaded: LoadedSubagent {
                def: SubagentDef {
                    name: name.to_string(),
                    description: String::new(),
                    system_prompt: String::new(),
                    tools,
                    isolation: None,
                    model: None,
                },
                source,
            },
            tools_declared: declared,
            isolation_declared: true,
        }
    }

    #[test]
    fn merge_user_overrides_builtin_preserves_declared_tools() {
        // builtin researcher has 5 tools; user overrides with 2 declared.
        let builtin = vec![loaded(
            "researcher",
            vec!["read_file".into(), "grep".into(), "glob".into()],
            true,
            SubagentSource::Builtin,
        )];
        let user = vec![loaded(
            "researcher",
            vec!["read_file".into()],
            true,
            SubagentSource::User,
        )];
        let merged = merge_with_inheritance(vec![builtin, user]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].def.tools, vec!["read_file".to_string()]);
        assert_eq!(merged[0].source, SubagentSource::User);
    }

    #[test]
    fn merge_user_inherits_builtin_tools_when_not_declared() {
        // Q2: user overrides researcher but does NOT declare tools →
        // inherit builtin's 3 tools. Source stays User.
        let builtin = vec![loaded(
            "researcher",
            vec!["read_file".into(), "grep".into(), "glob".into()],
            true,
            SubagentSource::Builtin,
        )];
        let user = vec![loaded("researcher", vec![], false, SubagentSource::User)];
        let merged = merge_with_inheritance(vec![builtin, user]);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].def.tools,
            vec![
                "read_file".to_string(),
                "grep".to_string(),
                "glob".to_string(),
            ]
        );
        assert_eq!(merged[0].source, SubagentSource::User);
    }

    #[test]
    fn merge_project_inherits_user_tools_when_neither_declared() {
        // Chain: builtin (declared) → user (inherits builtin) →
        // project (inherits user's inherited set).
        let builtin = vec![loaded(
            "x",
            vec!["a".into(), "b".into()],
            true,
            SubagentSource::Builtin,
        )];
        let user = vec![loaded("x", vec![], false, SubagentSource::User)];
        let project = vec![loaded("x", vec![], false, SubagentSource::Project)];
        let merged = merge_with_inheritance(vec![builtin, user, project]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].def.tools, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(merged[0].source, SubagentSource::Project);
    }

    #[test]
    fn merge_brand_new_agent_no_declaration_is_empty_vec() {
        // No lower layer to inherit from → empty Vec (general-purpose
        // convention: empty = full set at filter time).
        let user = vec![loaded("custom", vec![], false, SubagentSource::User)];
        let merged = merge_with_inheritance(vec![vec![], user]);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].def.tools.is_empty());
    }

    #[test]
    fn merge_disjoint_names_all_present() {
        let builtin = vec![loaded("researcher", vec![], true, SubagentSource::Builtin)];
        let user = vec![loaded("foo", vec![], false, SubagentSource::User)];
        let project = vec![loaded("bar", vec![], false, SubagentSource::Project)];
        let merged = merge_with_inheritance(vec![builtin, user, project]);
        let names: Vec<&str> = merged.iter().map(|l| l.def.name.as_str()).collect();
        assert_eq!(names, vec!["bar", "foo", "researcher"]); // alphabetical
    }

    #[test]
    fn merge_empty_layers_returns_empty() {
        let merged = merge_with_inheritance(vec![]);
        assert!(merged.is_empty());
    }

    // ---- SubagentCache end-to-end (mtime fence + merge) ----

    #[tokio::test]
    async fn cache_list_merges_builtin_user_project_with_precedence() {
        let user_tmp = tempfile::TempDir::new().unwrap();
        let user_agents = user_tmp.path().join(AGENTS_SUBDIR);
        std::fs::create_dir_all(&user_agents).unwrap();
        write_agent(
            &user_agents,
            "shared",
            "---\nname: shared\ndescription: from-user\n---\nub",
        );
        write_agent(
            &user_agents,
            "useronly",
            "---\nname: useronly\ntools: [read_file]\n---\nub",
        );

        let proj_tmp = tempfile::TempDir::new().unwrap();
        let proj_agents = proj_tmp.path().join(PROJECT_NAMESPACE).join(AGENTS_SUBDIR);
        std::fs::create_dir_all(&proj_agents).unwrap();
        write_agent(
            &proj_agents,
            "shared",
            "---\nname: shared\ndescription: from-project\n---\npb",
        );
        write_agent(
            &proj_agents,
            "projonly",
            "---\nname: projonly\ntools: [grep]\n---\npb",
        );

        let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();
        let merged = cache.list(&project_path).await;
        set_user_dir_for_test(prev);

        // 2 builtins + 1 useronly + 1 projonly + shared (project wins) = 5.
        assert_eq!(merged.len(), 5);
        let by_name: HashMap<&str, &LoadedSubagent> =
            merged.iter().map(|l| (l.def.name.as_str(), l)).collect();

        // Builtins present.
        assert!(by_name.contains_key("researcher"));
        assert!(by_name.contains_key("general-purpose"));
        assert_eq!(by_name["researcher"].source, SubagentSource::Builtin);

        // user / project layers.
        assert_eq!(by_name["useronly"].source, SubagentSource::User);
        assert_eq!(by_name["useronly"].def.tools, vec!["read_file".to_string()]);
        assert_eq!(by_name["projonly"].source, SubagentSource::Project);
        assert_eq!(by_name["projonly"].def.tools, vec!["grep".to_string()]);

        // Precedence: project wins on collision.
        assert_eq!(by_name["shared"].source, SubagentSource::Project);
        assert_eq!(by_name["shared"].def.description, "from-project");
    }

    #[tokio::test]
    async fn cache_list_user_overrides_builtin_inherits_tools_when_undeclared() {
        // user writes researcher.md with no tools field → inherit
        // builtin researcher's 5 tools. Source = User.
        let user_tmp = tempfile::TempDir::new().unwrap();
        let user_agents = user_tmp.path().join(AGENTS_SUBDIR);
        std::fs::create_dir_all(&user_agents).unwrap();
        write_agent(
            &user_agents,
            "researcher",
            "---\nname: researcher\ndescription: my-researcher\n---\nCustom prompt only.",
        );

        let proj_tmp = tempfile::TempDir::new().unwrap();
        let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();
        let merged = cache.list(&project_path).await;
        set_user_dir_for_test(prev);

        let r = merged
            .iter()
            .find(|l| l.def.name == "researcher")
            .expect("researcher present");
        assert_eq!(r.source, SubagentSource::User);
        assert_eq!(r.def.description, "my-researcher");
        // Inherited from builtin.
        assert_eq!(
            r.def.tools,
            vec![
                "read_file".to_string(),
                "grep".to_string(),
                "glob".to_string(),
                "list_dir".to_string(),
                "web_fetch".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn cache_list_user_overrides_builtin_with_declared_tools_uses_declared() {
        // user declares tools explicitly → use them verbatim (no
        // inheritance).
        let user_tmp = tempfile::TempDir::new().unwrap();
        let user_agents = user_tmp.path().join(AGENTS_SUBDIR);
        std::fs::create_dir_all(&user_agents).unwrap();
        write_agent(
            &user_agents,
            "researcher",
            "---\nname: researcher\ntools: [read_file, grep]\n---\nOnly 2 tools.",
        );

        let proj_tmp = tempfile::TempDir::new().unwrap();
        let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();
        let merged = cache.list(&project_path).await;
        set_user_dir_for_test(prev);

        let r = merged.iter().find(|l| l.def.name == "researcher").unwrap();
        assert_eq!(r.source, SubagentSource::User);
        assert_eq!(
            r.def.tools,
            vec!["read_file".to_string(), "grep".to_string()]
        );
    }

    #[tokio::test]
    async fn cache_list_brand_new_agent_no_tools_is_empty_vec() {
        let user_tmp = tempfile::TempDir::new().unwrap();
        let user_agents = user_tmp.path().join(AGENTS_SUBDIR);
        std::fs::create_dir_all(&user_agents).unwrap();
        write_agent(
            &user_agents,
            "custom",
            "---\nname: custom\ndescription: x\n---\nbody",
        );

        let proj_tmp = tempfile::TempDir::new().unwrap();
        let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();
        let merged = cache.list(&project_path).await;
        set_user_dir_for_test(prev);

        let c = merged.iter().find(|l| l.def.name == "custom").unwrap();
        assert_eq!(c.source, SubagentSource::User);
        assert!(
            c.def.tools.is_empty(),
            "no tools + no lower layer → empty Vec"
        );
    }

    #[tokio::test]
    async fn cache_lookup_finds_builtin() {
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();
        let r = cache.lookup(&project_path, "researcher").await;
        let r = r.expect("researcher builtin resolves");
        assert_eq!(r.source, SubagentSource::Builtin);
        assert_eq!(r.def.name, "researcher");
    }

    #[tokio::test]
    async fn cache_lookup_unknown_returns_none() {
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();
        assert!(cache
            .lookup(&project_path, "does-not-exist")
            .await
            .is_none());
    }

    #[tokio::test]
    async fn cache_lookup_project_overrides_user_and_builtin() {
        let user_tmp = tempfile::TempDir::new().unwrap();
        let user_agents = user_tmp.path().join(AGENTS_SUBDIR);
        std::fs::create_dir_all(&user_agents).unwrap();
        write_agent(
            &user_agents,
            "researcher",
            "---\nname: researcher\ndescription: user-ver\n---\nub",
        );

        let proj_tmp = tempfile::TempDir::new().unwrap();
        let proj_agents = proj_tmp.path().join(PROJECT_NAMESPACE).join(AGENTS_SUBDIR);
        std::fs::create_dir_all(&proj_agents).unwrap();
        write_agent(
            &proj_agents,
            "researcher",
            "---\nname: researcher\ndescription: project-ver\n---\npb",
        );

        let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();
        let r = cache.lookup(&project_path, "researcher").await;
        set_user_dir_for_test(prev);

        let r = r.expect("researcher resolves");
        assert_eq!(r.source, SubagentSource::Project);
        assert_eq!(r.def.description, "project-ver");
    }

    #[tokio::test]
    async fn cache_list_picks_up_new_md_on_next_call() {
        // mtime fence: writing a new .md between calls is picked up
        // without any explicit reload command.
        let user_tmp = tempfile::TempDir::new().unwrap();
        let user_agents = user_tmp.path().join(AGENTS_SUBDIR);
        std::fs::create_dir_all(&user_agents).unwrap();

        let proj_tmp = tempfile::TempDir::new().unwrap();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
        let cache = SubagentCache::arc();

        // Initially only builtins.
        let merged = cache.list(&project_path).await;
        assert_eq!(merged.len(), 2);

        // Add a user .md → next list call sees it.
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        write_agent(
            &user_agents,
            "newagent",
            "---\nname: newagent\ntools: [read_file]\n---\nb",
        );
        let merged = cache.list(&project_path).await;
        assert_eq!(merged.len(), 3);
        let new = merged.iter().find(|l| l.def.name == "newagent").unwrap();
        assert_eq!(new.source, SubagentSource::User);

        set_user_dir_for_test(prev);
    }

    #[tokio::test]
    async fn cache_list_skips_bad_md_keeps_others() {
        let user_tmp = tempfile::TempDir::new().unwrap();
        let user_agents = user_tmp.path().join(AGENTS_SUBDIR);
        std::fs::create_dir_all(&user_agents).unwrap();
        write_agent(
            &user_agents,
            "good",
            "---\nname: good\ntools: [read_file]\n---\nb",
        );
        // Bad: illegal name.
        write_agent(&user_agents, "bad", "---\nname: x/y\n---\nb");

        let proj_tmp = tempfile::TempDir::new().unwrap();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
        let cache = SubagentCache::arc();
        let merged = cache.list(&project_path).await;
        set_user_dir_for_test(prev);

        // 2 builtins + 1 good (bad skipped) = 3.
        assert_eq!(merged.len(), 3);
        assert!(merged.iter().any(|l| l.def.name == "good"));
        assert!(!merged.iter().any(|l| l.def.name == "x/y"));
    }

    // ---- SubagentSource::as_str ----

    #[test]
    fn source_as_str_matches_wire_form() {
        assert_eq!(SubagentSource::Builtin.as_str(), "builtin");
        assert_eq!(SubagentSource::User.as_str(), "user");
        assert_eq!(SubagentSource::Project.as_str(), "project");
    }

    // ---- 2026-07-03 (task 07-03-subagent-per-agent-model-ui, 阶段 2):
    // apply_model_line + write_frontmatter_model

    /// Set on a file that already declares `model:` → the value
    /// is replaced; body + other frontmatter keys are preserved.
    #[test]
    fn apply_model_line_replaces_existing() {
        let input =
            "---\nname: x\nmodel: old-uuid\ndescription: d\n---\nbody line 1\nbody line 2\n";
        let got = apply_model_line(input, Some("new-uuid")).unwrap();
        assert!(got.contains("model: new-uuid"));
        assert!(!got.contains("old-uuid"));
        // body + description preserved
        assert!(got.contains("description: d"));
        assert!(got.contains("body line 1"));
        assert!(got.contains("body line 2"));
    }

    /// Set on a file with no `model:` declaration → the field
    /// is inserted as the first line inside the frontmatter
    /// (after the opening `---` fence).
    #[test]
    fn apply_model_line_inserts_when_absent() {
        let input = "---\nname: x\ndescription: d\n---\nbody\n";
        let got = apply_model_line(input, Some("new-uuid")).unwrap();
        // The new line is the first line inside the fence
        // (right after the opening `---`). The closing fence +
        // body are still there.
        let model_idx = got.find("model: new-uuid").unwrap();
        let open_idx = got.find("---").unwrap();
        // open_idx points at the FIRST `---`; the new model
        // line must come after it.
        assert!(model_idx > open_idx, "model line must be inside the fence");
        // All other frontmatter lines still present.
        assert!(got.contains("name: x"));
        assert!(got.contains("description: d"));
        // body still present
        assert!(got.contains("body"));
    }

    /// Clear (`None`) when `model:` is declared → the line is
    /// removed; everything else is preserved.
    #[test]
    fn apply_model_line_clears_existing() {
        let input = "---\nname: x\nmodel: old-uuid\ndescription: d\n---\nbody\n";
        let got = apply_model_line(input, None).unwrap();
        assert!(!got.contains("model:"));
        assert!(!got.contains("old-uuid"));
        assert!(got.contains("name: x"));
        assert!(got.contains("description: d"));
        assert!(got.contains("body"));
    }

    /// Clear when `model:` is absent → no-op (return input
    /// unchanged).
    #[test]
    fn apply_model_line_clear_no_existing_is_noop() {
        let input = "---\nname: x\ndescription: d\n---\nbody\n";
        let got = apply_model_line(input, None).unwrap();
        assert_eq!(got, input);
    }

    /// `name:` / `description:` ordering is preserved across edits
    /// (the writer must not reorder existing keys). The new
    /// `model:` line goes AT THE TOP of the frontmatter body
    /// when inserting (so `model` is read first; matches the
    /// pre-existing file convention from the doc-comment
    /// example).
    #[test]
    fn apply_model_line_preserves_key_order() {
        let input = "---\nname: x\ndescription: d\ntools: [read_file]\n---\nbody\n";
        let got = apply_model_line(input, Some("new-uuid")).unwrap();
        // The first 3 lines after the opening fence are now:
        // model: new-uuid, name: x, description: d, tools: [...]
        // (model is inserted as the first line, the rest stays
        // in original order).
        let n_idx = got.find("name: x").unwrap();
        let d_idx = got.find("description: d").unwrap();
        let t_idx = got.find("tools: [read_file]").unwrap();
        let m_idx = got.find("model: new-uuid").unwrap();
        // model is the first key (right after the opening fence);
        // the other 3 stay in their original order.
        assert!(m_idx < n_idx);
        assert!(n_idx < d_idx);
        assert!(d_idx < t_idx);
    }

    /// `model :` (space before colon) is also accepted — matches
    /// the tolerant parser in `apply_kv` (which accepts both
    /// `model: x` and `model :x`). The writer normalizes to the
    /// canonical single-space form.
    #[test]
    fn apply_model_line_tolerates_space_before_colon() {
        let input = "---\nname: x\nmodel :old-uuid\n---\nbody\n";
        let got = apply_model_line(input, Some("new-uuid")).unwrap();
        assert!(
            got.contains("model: new-uuid"),
            "normalized to canonical form"
        );
        assert!(!got.contains("old-uuid"));
        assert!(
            !got.contains("model :"),
            "no space-before-colon form remains"
        );
    }

    /// File with no frontmatter fence → Err (the loader would
    /// have rejected this for missing `name`; we don't
    /// auto-insert a fence here — see the function doc comment
    /// for the "don't rewrite file structure" rationale).
    #[test]
    fn apply_model_line_no_fence_errors() {
        let input = "no frontmatter here\njust body\n";
        let err = apply_model_line(input, Some("new-uuid"));
        assert!(err.is_err(), "no fence → Err");
    }

    /// File with unterminated frontmatter (opening `---` but no
    /// closing one) → Err.
    #[test]
    fn apply_model_line_unterminated_fence_errors() {
        let input = "---\nname: x\nbody without closing fence\n";
        let err = apply_model_line(input, Some("new-uuid"));
        assert!(err.is_err());
    }

    /// IO wrapper: writes the new content atomically (`.tmp` +
    /// rename) and preserves the rest of the file.
    #[test]
    fn write_frontmatter_model_writes_atomically() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent.md");
        std::fs::write(&path, "---\nname: x\ndescription: d\n---\nbody\n").unwrap();
        write_frontmatter_model(&path, Some("new-uuid")).unwrap();
        let got = std::fs::read_to_string(&path).unwrap();
        assert!(got.contains("model: new-uuid"));
        assert!(got.contains("name: x"));
        assert!(got.contains("description: d"));
        assert!(got.contains("body"));
        // No `.tmp` left behind.
        assert!(!tmp.path().join("agent.md.tmp").exists());
    }

    /// IO wrapper: `None` removes the line.
    #[test]
    fn write_frontmatter_model_clears() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent.md");
        std::fs::write(&path, "---\nname: x\nmodel: old-uuid\n---\nbody\n").unwrap();
        write_frontmatter_model(&path, None).unwrap();
        let got = std::fs::read_to_string(&path).unwrap();
        assert!(!got.contains("model:"));
        assert!(got.contains("name: x"));
    }

    /// IO wrapper: no-op when there's nothing to change (the
    /// caller passes `Some(<existing value>)` and the file is
    /// already in that state). The function returns Ok(())
    /// without touching the file's mtime.
    #[test]
    fn write_frontmatter_model_noop_skips_write() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent.md");
        let original = "---\nname: x\nmodel: same-uuid\n---\nbody\n";
        std::fs::write(&path, original).unwrap();
        let original_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        // Sleep so the mtime would actually advance on a re-write.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write_frontmatter_model(&path, Some("same-uuid")).unwrap();
        let after_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(original_mtime, after_mtime, "mtime unchanged on no-op");
    }

    /// IO wrapper: missing file → Err (not a silent no-op).
    #[test]
    fn write_frontmatter_model_missing_file_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does-not-exist.md");
        let res = write_frontmatter_model(&path, Some("any"));
        assert!(res.is_err(), "missing file → Err");
    }

    /// IO wrapper: file without frontmatter fence → Err.
    #[test]
    fn write_frontmatter_model_malformed_file_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("broken.md");
        std::fs::write(&path, "no fence here\njust body\n").unwrap();
        let res = write_frontmatter_model(&path, Some("any"));
        assert!(res.is_err(), "no fence → Err (no silent rewrite)");
    }

    /// IO wrapper: builtins have no file path → the
    /// `locate_agent_file` helper returns Err so a misrouted
    /// caller surfaces a clean error (the IPC layer routes
    /// builtin writes to the DB override table instead, so
    /// this case is a defensive guard for future refactors).
    #[test]
    fn locate_agent_file_builtin_errors() {
        let res = locate_agent_file(SubagentSource::Builtin, "researcher", "/tmp/proj");
        assert!(res.is_err(), "builtin has no file path");
    }

    // ---- Step 2.3: plugin agents layer (workflow integration) ----

    /// Pure path-arithmetic test (no IO). Mirrors
    /// `plugin_skills_dir` test shape.
    #[test]
    fn plugin_agents_dir_lands_under_workflow_subdir() {
        let p = plugin_agents_dir("dev", "/tmp/proj");
        assert_eq!(
            p,
            PathBuf::from("/tmp/proj/.everlasting/workflow/dev/agents"),
            "plugin_agents_dir must resolve to `<project>/.everlasting/workflow/<wf>/agents/`",
        );
    }

    /// Plugin-source file path resolution is read-only —
    /// `locate_agent_file` returns Err on Plugin so the IPC
    /// layer's writer path doesn't accidentally write to the
    /// wrong layer (the plugin layer is currently read-only;
    /// see `commands/subagents.rs` Step 2.3 comment).
    #[test]
    fn locate_agent_file_plugin_errors() {
        let res = locate_agent_file(SubagentSource::Plugin, "researcher", "/tmp/proj");
        assert!(res.is_err(), "plugin agents are read-only; locate should refuse");
    }

    #[tokio::test]
    async fn list_with_workflow_plugin_resolves_first() {
        // Plugin agent defines `researcher` with a unique
        // body. `list_with_workflow` with `workflow_name =
        // Some("dev")` must pick up the plugin layer, NOT
        // the builtin.
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let plugin_dir = plugin_agents_dir("dev", &proj_tmp.path().to_string_lossy());
        std::fs::create_dir_all(&plugin_dir).unwrap();
        write_agent(&plugin_dir, "researcher", "---\nname: researcher\n---\nPLUGIN_RESEARCHER");

        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        let with_wf = cache
            .list_with_workflow(&project_path, Some("dev"))
            .await;
        let researcher = with_wf
            .iter()
            .find(|l| l.def.name == "researcher")
            .expect("researcher must resolve under plugin workflow");
        assert_eq!(researcher.source, SubagentSource::Plugin);
        assert!(
            researcher.def.system_prompt.contains("PLUGIN_RESEARCHER"),
            "plugin body must win over builtin",
        );

        // Without a workflow_name, the same call MUST NOT
        // see the plugin layer — the legacy path is
        // un-touched.
        let without_wf = cache.list(&project_path).await;
        let researcher_legacy = without_wf
            .iter()
            .find(|l| l.def.name == "researcher")
            .expect("researcher exists as builtin");
        assert_eq!(
            researcher_legacy.source,
            SubagentSource::Builtin,
            "non-workflow list must NOT consult the plugin layer",
        );
    }

    #[tokio::test]
    async fn list_with_workflow_plugin_overrides_project() {
        // Project layer defines `researcher`; plugin layer
        // also defines it. Plugin wins (higher priority).
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let project_agents = project_agents_dir(&proj_tmp.path().to_string_lossy());
        std::fs::create_dir_all(&project_agents).unwrap();
        write_agent(&project_agents, "researcher", "---\nname: researcher\n---\nPROJECT_BODY");
        let plugin_dir = plugin_agents_dir("dev", &proj_tmp.path().to_string_lossy());
        std::fs::create_dir_all(&plugin_dir).unwrap();
        write_agent(&plugin_dir, "researcher", "---\nname: researcher\n---\nPLUGIN_BODY");

        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        let merged = cache
            .list_with_workflow(&project_path, Some("dev"))
            .await;
        let researcher = merged
            .iter()
            .find(|l| l.def.name == "researcher")
            .expect("researcher must resolve");
        assert_eq!(researcher.source, SubagentSource::Plugin);
        assert!(
            researcher.def.system_prompt.contains("PLUGIN_BODY"),
            "plugin layer must override project on collision (got {:?})",
            researcher.def.system_prompt,
        );
    }

    #[tokio::test]
    async fn list_with_workflow_empty_name_falls_back_to_legacy() {
        // `Some("")` is normalized to `None` inside
        // `list_with_workflow` — the plugin layer must NOT
        // be consulted for empty plugin names. Same
        // contract as `find_skill_with_workflow`.
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let plugin_dir = plugin_agents_dir("dev", &proj_tmp.path().to_string_lossy());
        std::fs::create_dir_all(&plugin_dir).unwrap();
        write_agent(&plugin_dir, "researcher", "---\nname: researcher\n---\nPLUGIN_BODY");

        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        let merged = cache
            .list_with_workflow(&project_path, Some(""))
            .await;
        let researcher = merged
            .iter()
            .find(|l| l.def.name == "researcher")
            .expect("researcher exists as builtin");
        assert_eq!(
            researcher.source,
            SubagentSource::Builtin,
            "empty plugin name must NOT consult the plugin layer",
        );
    }

    #[tokio::test]
    async fn lookup_with_workflow_finds_plugin_agent() {
        // Mirror of `list_with_workflow_*` but exercises the
        // single-name lookup path (used by dispatch in
        // Step 2.4 / Phase 2 batch B).
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let plugin_dir = plugin_agents_dir("dev", &proj_tmp.path().to_string_lossy());
        std::fs::create_dir_all(&plugin_dir).unwrap();
        // `researcher` is the only name present in BOTH
        // builtins (line 471 of `agent::subagent::mod.rs`)
        // and the plugin layer — using it lets the test
        // prove "plugin wins when set, builtin wins when
        // unset" without inventing a new builtin name.
        write_agent(
            &plugin_dir,
            "researcher",
            "---\nname: researcher\n---\nPLUGIN_BODY",
        );

        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        let hit = cache
            .lookup_with_workflow(&project_path, Some("dev"), "researcher")
            .await
            .expect("plugin researcher must resolve");
        assert_eq!(hit.source, SubagentSource::Plugin);
        assert!(hit.def.system_prompt.contains("PLUGIN_BODY"));

        // Without workflow → falls back to legacy lookup
        // (builtin researcher). The plugin layer is NOT
        // consulted — the same call signature with `None`
        // is byte-equivalent to the pre-Step-2.3 lookup.
        let legacy = cache
            .lookup_with_workflow(&project_path, None, "researcher")
            .await
            .expect("builtin researcher exists");
        assert_eq!(legacy.source, SubagentSource::Builtin);
    }
}
