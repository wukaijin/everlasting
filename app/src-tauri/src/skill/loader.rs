//! B4 Skill loader — mtime-fenced scan of user/project/plugin skill dirs.
//!
//! Mirrors the B3 `resource_loader` shape (read-through mtime fence,
//! hand-rolled frontmatter parser, precedence merge) with one
//! structural delta: a skill is a **directory** containing `SKILL.md`
//! (vs a command's single `*.md` file), so the scan walks subdirs.
//!
//! Precedence (high → low): **plugin > project > user**. The
//! `plugin` layer is **only** consulted when the caller passes a
//! `workflow_name` (Step 1.1 of `07-08-workflow-integration`) —
//! non-workflow sessions go straight to project-overrides-user and
//! never touch `<project>/.everlasting/workflow/<name>/skills/`.
//! No builtins (unlike commands, which carry `/help` `/clear` `/new`).
//! `user_dir` naming matches `resource_loader` so both layers share
//! the same config root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::Serialize;
use tokio::sync::RwLock;

use crate::llm::types::{CacheControl, ContentBlock};
use crate::memory::file::user_dir;

mod frontmatter;
// Re-exported for tests_loader (the frontmatter parser's unit tests live
// in the relocated external test module) + loader's own parse_skill_content.
#[allow(unused_imports)]
pub(crate) use frontmatter::{apply_kv, parse_allowed_tools, parse_frontmatter, Frontmatter};

/// Subdirectory under both the user config dir and the project root
/// that holds skill directories (`<name>/SKILL.md`).
pub(crate) const SKILLS_SUBDIR: &str = "skills";

/// `.everlasting/` project-local namespace (shared with B3 commands
/// and shell output spillover). Skills live under `skills/`.
pub(crate) const PROJECT_NAMESPACE: &str = ".everlasting";

/// Workflow plugin namespace under `.everlasting/`. A plugin named
/// `dev` lives under `<project>/.everlasting/workflow/dev/`, and its
/// skills under `<project>/.everlasting/workflow/dev/skills/`. Step
/// 1.1 of `07-08-workflow-integration` — design §1 + §6 rollback.
const WORKFLOW_SUBDIR: &str = "workflow";

/// The single Markdown file inside each skill directory that carries
/// the frontmatter + instruction body. Additional files in the dir
/// (`reference.md`, `examples/`, …) are NOT scanned here — the model
/// pulls them via `read_file` on demand (L2 progressive disclosure).
pub(crate) const SKILL_FILENAME: &str = "SKILL.md";

/// Single SKILL.md size cap (defensive — a skill is an instruction
/// template, not a content dump). Mirrors B3's `MAX_COMMAND_FILE_SIZE`.
pub(crate) const MAX_SKILL_FILE_SIZE: u64 = 64 * 1024; // 64 KiB

/// Where a skill came from. On a name collision, the highest-priority
/// layer wins: **plugin > builtin-plugin > project > user**. The `plugin`
/// and `builtin-plugin` layers are workflow-scoped and only consulted
/// when the caller passes a `workflow_name` — see
/// `list_skill_infos_with_workflow` / `find_skill_with_workflow`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    User,
    Project,
    /// Workflow plugin skill — comes from
    /// `<project>/.everlasting/workflow/<name>/skills/`. Only
    /// consulted in workflow sessions (Step 1.1 of
    /// `07-08-workflow-integration`); non-workflow callers fall
    /// through to project-overrides-user.
    Plugin,
    /// app 内置 plugin skill(`include_str!` 常量)。
    /// 07-09-workflow-builtin-plugin: 编译期常量,优先级
    /// `Plugin > BuiltinPlugin > Project > User`。
    BuiltinPlugin,
}

/// A parsed skill directory: frontmatter + `SKILL.md` body. `body` is
/// returned to the LLM when it calls `use_skill(name)` (L1 activation).
///
/// `allowed_tools` is the **declarative** (informational) list of tools
/// the skill is designed to use — parsed from `allowed-tools:` (or
/// `allowed_tools:`) in the frontmatter. Empty Vec means "not declared".
/// The list is **not enforced** at execution time: the existing ⑨ 5-tier
/// permission layer still gates every tool call, and `use_skill` itself
/// does not consult this list. The data is surfaced in the L0 listing
/// block so the model sees a hint like `(tools: read_file, grep)` after
/// the description. See `.trellis/tasks/06-18-skill-stretches/prd.md`
/// Stretch 1 for the grill-converged decision (declarative, not enforced).
#[derive(Clone, Debug)]
pub struct SkillResource {
    pub name: String,
    pub description: String,
    /// `SKILL.md` body — sent to the LLM as the `use_skill` tool_result
    /// when the model invokes the skill (L1 activation).
    pub body: String,
    pub path: PathBuf,
    pub source: SkillSource,
    /// Skill-stated tool preferences (declarative). Deduplicated,
    /// trimmed; empty = not declared. Not consulted by ⑨ or `use_skill`.
    pub allowed_tools: Vec<String>,
}

/// Wire DTO for the L0 skill listing + (future) UI. The listing only
/// needs `name` + `description` — the body is fetched on L1 activation.
///
/// `allowed_tools` (Stretch 1, 2026-06-18): the skill's declared tool
/// preferences (informational; not enforced). Surfaced in the L0
/// listing as `(tools: a, b)` after the description. Empty Vec = the
/// skill did not declare anything.
#[derive(Serialize, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub source: String,
    pub allowed_tools: Vec<String>,
}

// ---------------------------------------------------------------------------
// Directory scan (walks subdirs — the structural delta from B3)
// ---------------------------------------------------------------------------

/// Resolve the user skills dir (`~/.config/everlasting/skills/`).
/// `None` if `user_dir()` is unresolvable on this platform.
fn user_skills_dir() -> Option<PathBuf> {
    user_dir().map(|d| d.join(SKILLS_SUBDIR))
}

/// Resolve a project's skills dir (`<project>/.everlasting/skills/`).
fn project_skills_dir(project_path: &str) -> PathBuf {
    PathBuf::from(project_path)
        .join(PROJECT_NAMESPACE)
        .join(SKILLS_SUBDIR)
}

/// Resolve a workflow plugin's skills dir
/// (`<project>/.everlasting/workflow/<name>/skills/`).
///
/// Step 1.1 of `07-08-workflow-integration`: the plugin layer is
/// only consulted when the caller passes a `workflow_name` (the
/// chat_loop wires `workflow_ctx.workflow_def.name` here). This
/// function is **pure path arithmetic** — no I/O, no `is_dir()`
/// check, no `exists()`; missing dirs are silently empty at scan
/// time (see `scan_skill_dir`'s `NotFound` arm + `read_through`'s
/// empty-`current_mtimes` cache). That gives us the "plugin dir
/// doesn't exist → fall back to global" behavior for free (no
/// `if exists` branch needed at the call site).
///
/// `workflow_name` is taken verbatim — the design document pins
/// ASCII snake_case (`dev` / `review`) as the plugin naming
/// convention; we don't sanitize further so a typo surfaces
/// immediately as "no such skill" instead of being silently
/// rewritten.
pub fn plugin_skills_dir(workflow_name: &str, project_path: &str) -> PathBuf {
    PathBuf::from(project_path)
        .join(PROJECT_NAMESPACE)
        .join(WORKFLOW_SUBDIR)
        .join(workflow_name)
        .join(SKILLS_SUBDIR)
}

/// Stat each `<name>/SKILL.md` under `dir`, returning a path → mtime
/// map. A subdir's absence (no SKILL.md), a new subdir, a deleted
/// subdir, or a changed mtime all invalidate the cached scan. Missing
/// dir → empty map. Same fence idea as B3's `current_mtimes`, but the
/// keys are the SKILL.md paths (one per subdir) rather than the dir's
/// direct `*.md` children.
async fn current_mtimes(dir: &Path) -> HashMap<PathBuf, Option<SystemTime>> {
    let mut map = HashMap::new();
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else {
        return map;
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_path = path.join(SKILL_FILENAME);
        let m = tokio::fs::metadata(&skill_path)
            .await
            .ok()
            .and_then(|m| m.modified().ok());
        map.insert(skill_path, m);
    }
    map
}

/// Scan a single skills directory: walk its subdirs, load each
/// `<name>/SKILL.md`. Bad files (over-cap, non-UTF-8, no name) and
/// subdirs without a SKILL.md are skipped with a `warn!` — one bad
/// skill never aborts the whole scan (mirrors memory/B3 tolerance).
pub(crate) async fn scan_skill_dir(dir: &Path, source: SkillSource) -> Vec<SkillResource> {
    let mut out = Vec::new();
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
        Err(e) => {
            tracing::warn!(dir = %dir.display(), error = %e, "skills: read_dir failed");
            return out;
        }
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(dir_name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let skill_path = path.join(SKILL_FILENAME);
        match load_skill_file(&skill_path, dir_name, source).await {
            Ok(Some(res)) => out.push(res),
            Ok(None) => {} // skipped (no SKILL.md / over-cap / no name)
            Err(e) => tracing::warn!(
             path = %skill_path.display(),
             error = %e,
             "skills: load failed"
            ),
        }
    }
    out
}

/// 纯解析:从 SKILL.md 文本 + 目录名 + source 构造 SkillResource。
/// 磁盘层(`load_skill_file`)和内置层(`builtin_plugin_skills`)共用此函数,
/// 保证 frontmatter 解析行为完全一致(07-09-workflow-builtin-plugin)。
fn parse_skill_content(
    content: &str,
    dir_name: &str,
    source: SkillSource,
) -> Option<SkillResource> {
    let (fm, body) = parse_frontmatter(content);
    // Name: frontmatter `name` wins; else the parent directory name.
    // Require non-empty (dir_name always non-empty here, but frontmatter
    // could be whitespace-only).
    let name = fm
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| dir_name.to_string());
    if name.trim().is_empty() {
        return None;
    }
    Some(SkillResource {
        name,
        description: fm.description.unwrap_or_default(),
        body,
        // 内置层无磁盘路径,用虚拟标记(tracing 可读;locate/write 对内置层不适用)。
        // 磁盘层调用方在 `load_skill_file` 中覆盖此字段。
        path: PathBuf::new(),
        source,
        allowed_tools: fm.allowed_tools,
    })
}

/// Load + parse one `<name>/SKILL.md`. Returns `Ok(None)` when the
/// skill is deliberately skipped (subdir has no SKILL.md / over-cap /
/// no name); `Err` for I/O failures other than NotFound.
async fn load_skill_file(
    skill_path: &Path,
    dir_name: &str,
    source: SkillSource,
) -> std::io::Result<Option<SkillResource>> {
    let meta = match tokio::fs::metadata(skill_path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if meta.len() > MAX_SKILL_FILE_SIZE {
        tracing::warn!(
         path = %skill_path.display(),
         size = meta.len(),
         max = MAX_SKILL_FILE_SIZE,
         "skills: SKILL.md exceeds size cap, skipping"
        );
        return Ok(None);
    }
    let content = tokio::fs::read_to_string(skill_path).await?;
    let mut res = match parse_skill_content(&content, dir_name, source) {
        Some(r) => r,
        None => {
            tracing::warn!(
                path = %skill_path.display(),
                "skills: no name (frontmatter + dir name both empty), skipping"
            );
            return Ok(None);
        }
    };
    // 磁盘层:覆盖虚拟 path 为真实磁盘路径(07-09-workflow-builtin-plugin)。
    res.path = skill_path.to_path_buf();
    Ok(Some(res))
}

/// 构造 app 内置 plugin 的 skills(07-09-workflow-builtin-plugin;
/// 07-26-workflow-review-plugin C3 扩展 review)。
/// 仅 `workflow_name == "dev" | "review"` 时返回;其他返回空。
/// 不走磁盘扫描(`tokio::fs::read_dir`)—— 内置源是 `include_str!` 内存常量,
/// 用 `parse_skill_content` 直接解析(与磁盘层同一 frontmatter parser)。
/// `path` 用虚拟标记 `<builtin>/<wf>/skills/<slug>/SKILL.md`。
fn builtin_plugin_skills(workflow_name: &str) -> Vec<SkillResource> {
    let skills: &[(&str, &str)] = match workflow_name {
        "dev" => crate::agent::workflow::BUILTIN_DEV_SKILLS,
        "review" => crate::agent::workflow::BUILTIN_REVIEW_SKILLS,
        _ => return Vec::new(),
    };
    skills
        .iter()
        .filter_map(|(slug, body)| {
            let mut res = parse_skill_content(body, slug, SkillSource::BuiltinPlugin)?;
            res.path = PathBuf::from(format!("<builtin>/{workflow_name}/skills/{slug}/SKILL.md"));
            Some(res)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// SkillCache — read-through with an mtime fence (copied from B3)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct CachedScan {
    pub(crate) resources: Vec<SkillResource>,
    /// `<name>/SKILL.md` path → mtime at scan time. Compared against
    /// `current_mtimes` on every read; any difference (changed mtime OR
    /// a SKILL.md appearing/vanishing — i.e. a subdir added/removed)
    /// triggers a re-scan.
    pub(crate) mtimes: HashMap<PathBuf, Option<SystemTime>>,
}

/// Process-wide cache of scanned skill dirs, held in `AppState`.
///
/// Freshness is decided at read time by an mtime fence (no background
/// watcher): each read stats the dir's SKILL.md files, compares against
/// the cached mtimes, and re-scans only on a difference. Same pattern
/// as `resource_loader::CommandCache`.
///
/// The `plugin` slot is keyed by `(project_path, workflow_name)` —
/// different projects with the same plugin name get separate cache
/// entries (one project's plugin skills never bleed into another).
/// A non-workflow caller simply doesn't read this slot, so the cache
/// stays cold for non-workflow sessions (no scan cost).
pub struct SkillCache {
    user: RwLock<Option<CachedScan>>,
    project: RwLock<HashMap<String, CachedScan>>,
    /// `(project_path, workflow_name)` → cached plugin scan. Step 1.1.
    plugin: RwLock<HashMap<(String, String), CachedScan>>,
}

impl SkillCache {
    pub fn arc() -> Arc<Self> {
        Arc::new(Self {
            user: RwLock::new(None),
            project: RwLock::new(HashMap::new()),
            plugin: RwLock::new(HashMap::new()),
        })
    }

    /// List user-layer skills (mtime-fenced).
    pub async fn list_user(&self) -> Vec<SkillResource> {
        let Some(dir) = user_skills_dir() else {
            return Vec::new();
        };
        let mut guard = self.user.write().await;
        let updated = read_through(&dir, SkillSource::User, guard.as_ref()).await;
        let out = updated.resources.clone();
        *guard = Some(updated);
        out
    }

    /// List project-layer skills (mtime-fenced), keyed by project path.
    pub async fn list_project(&self, project_path: &str) -> Vec<SkillResource> {
        let dir = project_skills_dir(project_path);
        let mut guard = self.project.write().await;
        let cached = guard.get(project_path);
        let updated = read_through(&dir, SkillSource::Project, cached).await;
        let out = updated.resources.clone();
        guard.insert(project_path.to_string(), updated);
        out
    }

    /// List plugin-layer skills for `(project, workflow_name)`,
    /// mtime-fenced. Missing dir → empty (silently falls through to
    /// project / user layers at the call site — design §6 rollback).
    ///
    /// `workflow_name` is the plugin's identifier (e.g. `"dev"`).
    /// Empty string is normalized to `None` here so the cache slot
    /// never gets a poisoned "" key from a stray empty-name call.
    pub async fn list_plugin(&self, project_path: &str, workflow_name: &str) -> Vec<SkillResource> {
        if workflow_name.is_empty() {
            return Vec::new();
        }
        let dir = plugin_skills_dir(workflow_name, project_path);
        let key = (project_path.to_string(), workflow_name.to_string());
        let mut guard = self.plugin.write().await;
        let cached = guard.get(&key);
        let updated = read_through(&dir, SkillSource::Plugin, cached).await;
        let out = updated.resources.clone();
        guard.insert(key, updated);
        out
    }
}

/// Core mtime-fence read: stat the dir, compare against the cached
/// mtimes; on a full match return the cached clone, otherwise re-scan.
pub(crate) async fn read_through(
    dir: &Path,
    source: SkillSource,
    cached: Option<&CachedScan>,
) -> CachedScan {
    let current = current_mtimes(dir).await;
    if let Some(c) = cached {
        if current == c.mtimes {
            return c.clone();
        }
    }
    let resources = scan_skill_dir(dir, source).await;
    CachedScan {
        resources,
        mtimes: current,
    }
}

// ---------------------------------------------------------------------------
// find_skill / list_skill_infos — precedence merge (plugin > project > user)
// ---------------------------------------------------------------------------

fn resource_to_info(r: &SkillResource) -> SkillInfo {
    SkillInfo {
        name: r.name.clone(),
        description: r.description.clone(),
        source: match r.source {
            SkillSource::User => "user",
            SkillSource::Project => "project",
            SkillSource::Plugin => "plugin",
            SkillSource::BuiltinPlugin => "builtin-plugin",
        }
        .to_string(),
        allowed_tools: r.allowed_tools.clone(),
    }
}

/// Merge user + project skills into a single listing (L0 discovery).
///
/// Precedence: **project > user** (project inserted last into the
/// by-name map so it wins on collision). Result is sorted by name for
/// a stable listing. No builtins (unlike `resource_loader::list_all`).
///
/// **Non-workflow entry point** — does NOT consult the plugin layer.
/// This is the call site used by the UI (`commands::panel`); the UI
/// is session-agnostic and must never list plugin skills (those
/// only surface in workflow sessions, per design §6 rollback +
/// design §7 risk). Workflow sessions call
/// [`list_skill_infos_with_workflow`] instead.
pub async fn list_skill_infos(cache: &SkillCache, project_path: Option<&str>) -> Vec<SkillInfo> {
    merge_skill_layers(cache, project_path, None).await
}

/// Workflow-aware variant of [`list_skill_infos`]. When
/// `workflow_name` is `Some`, the plugin layer is consulted FIRST
/// (highest precedence: plugin > project > user). When `None`,
/// behavior is identical to [`list_skill_infos`].
///
/// Step 1.1 of `07-08-workflow-integration`. Callers MUST pass
/// `Some(name)` only when the session is a workflow session —
/// `chat_loop.rs` reads `workflow_ctx.workflow_def.name` and the
/// IPC layer (`commands::panel`) keeps the non-workflow entry
/// point above so the UI doesn't accidentally surface plugin
/// skills to non-workflow sessions.
///
/// `workflow_name = Some("")` is treated as `None` defensively
/// (Step 1.1: plugin layer must only run in actual workflow
/// sessions; an empty name is the symptom of a wiring bug, not a
/// valid plugin identifier).
pub async fn list_skill_infos_with_workflow(
    cache: &SkillCache,
    project_path: Option<&str>,
    workflow_name: Option<&str>,
) -> Vec<SkillInfo> {
    let wf = workflow_name.filter(|n| !n.is_empty());
    merge_skill_layers(cache, project_path, wf).await
}

/// Shared merge core. Layer insert order is the inverse of priority
/// — later inserts win — so the priority is plugin > project > user.
/// Each layer is optional: `Some(pp)` enables project; `Some(wf)`
/// enables plugin (and `None` / `""` disables it). Returns the
/// deduplicated, name-sorted `SkillInfo` listing.
async fn merge_skill_layers(
    cache: &SkillCache,
    project_path: Option<&str>,
    workflow_name: Option<&str>,
) -> Vec<SkillInfo> {
    let mut by_name: HashMap<String, SkillResource> = HashMap::new();
    // Lowest priority first; later inserts overwrite.
    for r in cache.list_user().await {
        by_name.insert(r.name.clone(), r);
    }
    if let Some(pp) = project_path {
        for r in cache.list_project(pp).await {
            by_name.insert(r.name.clone(), r);
        }
    }
    // 07-09-workflow-builtin-plugin: 内置 plugin 层,在 project-plugin 之前插入
    // (后插覆盖 → project-plugin 优先级高于 builtin-plugin)。
    if let Some(wf) = workflow_name {
        for r in builtin_plugin_skills(wf) {
            by_name.insert(r.name.clone(), r);
        }
    }
    if let (Some(wf), Some(pp)) = (workflow_name, project_path) {
        for r in cache.list_plugin(pp, wf).await {
            by_name.insert(r.name.clone(), r);
        }
    }
    let mut infos: Vec<SkillInfo> = by_name
        .into_values()
        .map(|r| resource_to_info(&r))
        .collect();
    infos.sort_by(|a, b| a.name.cmp(&b.name));
    infos
}

/// Look up a single skill by name (project overrides user). Returns
/// the full resource including `body` (for L1 activation as the
/// `use_skill` tool_result) and `path` (for tracing).
///
/// **Non-workflow entry point** — does NOT consult the plugin layer.
/// Workflow sessions call [`find_skill_with_workflow`] instead.
pub async fn find_skill(
    cache: &SkillCache,
    name: &str,
    project_path: Option<&str>,
) -> Option<SkillResource> {
    find_skill_in_layers(cache, name, project_path, None).await
}

/// Workflow-aware variant of [`find_skill`]. Consults the plugin
/// layer first when `workflow_name` is `Some`, then project, then
/// user. `workflow_name = Some("")` is treated as `None`.
///
/// Step 1.1 of `07-08-workflow-integration`. The consumer is
/// `tools::use_skill::execute`, which now reads
/// `workflow_name` off the per-turn `ToolContext` and passes
/// it through here so workflow sessions can load plugin-layer
/// skills (`wf-overview` / `wf-brainstorm` / `wf-before-dev` /
/// `wf-check` / `wf-update-spec`) without falling back to
/// "Skill not found". The Step 1.1 `#[allow(dead_code)]`
/// was lifted in Step 1.5 (also `07-08-workflow-integration`).
pub async fn find_skill_with_workflow(
    cache: &SkillCache,
    name: &str,
    project_path: Option<&str>,
    workflow_name: Option<&str>,
) -> Option<SkillResource> {
    let wf = workflow_name.filter(|n| !n.is_empty());
    find_skill_in_layers(cache, name, project_path, wf).await
}

/// Shared resolution core: highest-priority layer first (plugin →
/// builtin-plugin → project → user), so the first hit wins.
async fn find_skill_in_layers(
    cache: &SkillCache,
    name: &str,
    project_path: Option<&str>,
    workflow_name: Option<&str>,
) -> Option<SkillResource> {
    if let (Some(wf), Some(pp)) = (workflow_name, project_path) {
        if let Some(r) = cache
            .list_plugin(pp, wf)
            .await
            .into_iter()
            .find(|r| r.name == name)
        {
            return Some(r);
        }
    }
    // 07-09-workflow-builtin-plugin: 内置 plugin 层,在 project 之前查。
    if let Some(wf) = workflow_name {
        if let Some(r) = builtin_plugin_skills(wf)
            .into_iter()
            .find(|r| r.name == name)
        {
            return Some(r);
        }
    }
    if let Some(pp) = project_path {
        if let Some(r) = cache
            .list_project(pp)
            .await
            .into_iter()
            .find(|r| r.name == name)
        {
            return Some(r);
        }
    }
    cache.list_user().await.into_iter().find(|r| r.name == name)
}

// ---------------------------------------------------------------------------
// build_skill_listing_block — L0 discovery injection
// ---------------------------------------------------------------------------

/// Build the L0 skill-listing content block: a single `Text` block
/// carrying the `{name, description}` of every available skill, with
/// `cache_control: Ephemeral` so it caches as its own breakpoint
/// (B4 brainstorm decision: independent synthetic message, decoupled
/// from the memory instructions cache window — skill add/remove does
/// not bust the memory cache).
///
/// Stretch 1 (2026-06-18): when a skill declared `allowed-tools`,
/// the listing line carries an informational `(tools: a, b)` suffix
/// right after the description. The model can read the hint, but the
/// list is **not enforced** — `use_skill` and ⑨ do not consult it.
/// Format: `- <name>: <description>  (tools: a, b)` (description
/// omitted when empty, exactly as before; tools suffix omitted when
/// `allowed_tools` is empty).
///
/// Returns an empty `Vec` when there are no skills — the caller
/// (agent loop, PR2) skips the listing message entirely, symmetric to
/// `memory::loader::build_banner` returning `""` on a fresh install.
pub fn build_skill_listing_block(infos: &[SkillInfo]) -> Vec<ContentBlock> {
    if infos.is_empty() {
        return Vec::new();
    }
    let lines: Vec<String> = infos
        .iter()
        .map(|s| {
            let allowed_suffix = match s.allowed_tools.as_slice() {
                [] => String::new(),
                tools => format!("  (tools: {})", tools.join(", ")),
            };
            if s.description.trim().is_empty() {
                format!("- {}{}", s.name, allowed_suffix)
            } else {
                format!("- {}: {}{}", s.name, s.description, allowed_suffix)
            }
        })
        .collect();
    let text = format!(
  "<available-skills>\nThese skills are available. Call the `use_skill` tool with a skill's name when the task matches its description. If the user's message explicitly invokes a skill by `/name` (e.g. `/review-pr`), call `use_skill` with that exact name first, then follow the loaded instructions to handle the rest of the message.\n{}\n</available-skills>",
  lines.join("\n")
 );
    vec![ContentBlock::Text {
        text,
        cache_control: Some(CacheControl::Ephemeral),
    }]
}
