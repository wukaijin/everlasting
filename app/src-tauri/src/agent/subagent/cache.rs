//! Subagent 目录扫描 + mtime-fence 缓存 + 定位(拆分自 loader.rs,
//! 08-07-large-file-splitting)。
//!
//! `SubagentCache` 读穿透 mtime 栅栏,扫描 user/project/plugin 三层
//! agent 目录;定位与 read-through 也在此。

/// Subdirectory under both the user config dir and the project root
/// that holds custom subagent files (`*.md`). Matches the Claude Code
/// `.claude/agents/` convention but under our namespace.
pub(crate) const AGENTS_SUBDIR: &str = "agents";
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::RwLock;

use super::frontmatter::parse_frontmatter;
use super::loader::{merge_with_inheritance, LoadedSubagent, SubagentSource};
use crate::agent::subagent::{builtin_subagents, SubagentDef};
use crate::memory::file::user_dir;

/// `.everlasting/` project-local namespace (shared with B3 commands,
/// B4 skills, and shell output spillover). Agents live under `agents/`.
pub(crate) const PROJECT_NAMESPACE: &str = ".everlasting";

/// Single agent file size cap (defensive — an agent is a prompt +
/// frontmatter, not a content dump). Mirrors B3's
/// `MAX_COMMAND_FILE_SIZE` and B4's `MAX_SKILL_FILE_SIZE`.
pub(crate) const MAX_AGENT_FILE_SIZE: u64 = 64 * 1024; // 64 KiB

/// Return `true` iff `name` is non-empty and contains only
/// `[a-zA-Z0-9_-]` (PRD §6 — `name` becomes a JSON schema enum value
/// and a filesystem stem, so any path/comment/quote-breaking char
/// must be rejected).
pub(crate) fn is_valid_agent_name(name: &str) -> bool {
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
pub(crate) fn user_agents_dir() -> Option<PathBuf> {
    user_dir().map(|d| d.join(AGENTS_SUBDIR))
}

/// Resolve a project's agents dir (`<project>/.everlasting/agents/`).
pub(crate) fn project_agents_dir(project_path: &str) -> PathBuf {
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
pub(crate) fn plugin_agents_dir(workflow_name: &str, project_path: &str) -> PathBuf {
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
pub(crate) async fn current_mtimes(dir: &Path) -> HashMap<PathBuf, Option<SystemTime>> {
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
pub(crate) async fn scan_dir(dir: &Path, source: SubagentSource) -> Vec<LoadedAgentFile> {
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

/// 纯解析:从 agent `.md` 文本构造 LoadedAgentFile(07-09-workflow-builtin-plugin)。
/// name 校验 / description fallback / tools_declared / isolation_declared 逻辑
/// 与原 `load_agent_file` 完全一致。磁盘层与内置层共用此函数,保证 frontmatter
/// 解析行为完全一致。
pub(crate) fn parse_agent_content(
    content: &str,
    source: SubagentSource,
) -> Option<LoadedAgentFile> {
    let (fm, body) = parse_frontmatter(content);

    // name: frontmatter `name` is REQUIRED (no stem fallback). Empty
    // or whitespace-only → skip + warn.
    let name = match fm.name.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(n) => n.to_string(),
        None => {
            tracing::warn!("subagent: missing or empty `name` field, skipping (name is required)");
            return None;
        }
    };
    if !is_valid_agent_name(&name) {
        tracing::warn!(
            name = %name,
            "subagent: `name` contains illegal characters (allowed: [a-zA-Z0-9_-]), skipping"
        );
        return None;
    }

    // description: missing → empty string + warn (degraded but loads).
    let description = match fm.description {
        Some(d) => d,
        None => {
            tracing::warn!(
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

    Some(LoadedAgentFile {
        loaded: LoadedSubagent { def, source },
        tools_declared,
        isolation_declared,
    })
}

/// 构造 app 内置 plugin 的 agents(07-09-workflow-builtin-plugin;
/// 07-26-workflow-review-plugin C3 扩展 review)。
/// 仅 `workflow_name == "dev" | "review"` 时返回;其他返回空。
/// 不走磁盘扫描 —— 内置源是 `include_str!` 常量,
/// 用 `parse_agent_content` 直接解析(与磁盘层同一 parser)。
pub(crate) fn builtin_plugin_agents(workflow_name: &str) -> Vec<LoadedAgentFile> {
    let agents: &[(&str, &str)] = match workflow_name {
        "dev" => crate::agent::workflow::BUILTIN_DEV_AGENTS,
        "review" => crate::agent::workflow::BUILTIN_REVIEW_AGENTS,
        _ => return Vec::new(),
    };
    agents
        .iter()
        .filter_map(|(_role, body)| parse_agent_content(body, SubagentSource::BuiltinPlugin))
        .collect()
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
///
/// 07-09-workflow-builtin-plugin: IO + size cap kept here; parse logic
/// delegated to `parse_agent_content` so the builtin-plugin layer can
/// reuse the exact same parser.
pub(crate) async fn load_agent_file(
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
    Ok(parse_agent_content(&content, source))
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
pub(crate) struct LoadedAgentFile {
    pub(crate) loaded: LoadedSubagent,
    /// `true` iff the frontmatter declared `tools` (even empty).
    /// `false` iff `tools` was absent → eligible for inheritance.
    pub(crate) tools_declared: bool,
    /// `true` iff the frontmatter declared `isolation`. `false` iff
    /// `isolation` was absent → eligible for inheritance.
    pub(crate) isolation_declared: bool,
}

// ---------------------------------------------------------------------------
// SubagentCache — read-through with an mtime fence (B3 CommandCache shape)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct CachedScan {
    /// Raw scan of this layer (before precedence merge). Each entry
    /// carries its own `tools_declared` flag for the merge step.
    pub(crate) files: Vec<LoadedAgentFile>,
    /// path → mtime at scan time. Compared against `current_mtimes`
    /// on every read; any difference (changed mtime OR a file
    /// appearing/vanishing) triggers a re-scan.
    pub(crate) mtimes: HashMap<PathBuf, Option<SystemTime>>,
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
        let mut layers: Vec<Vec<LoadedAgentFile>> = Vec::with_capacity(5);

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

        // 4. Builtin-plugin layer (07-09-workflow-builtin-plugin):
        //    app-bundled `include_str!` agents, 插在 project 之后、
        //    project-plugin 之前。后插优先 → project-plugin > builtin-plugin > project。
        if let Some(wf) = wf {
            layers.push(builtin_plugin_agents(wf));
        }

        // 5. Plugin `.md` layer (highest priority when present).
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
        // 07-09-workflow-builtin-plugin: 内置 plugin agents 是
        // `include_str!` 编译期常量,无磁盘路径可写;要覆盖只能在项目
        // plugin 目录 `<project>/.everlasting/workflow/<wf>/agents/`
        // 放同名 .md。
        SubagentSource::BuiltinPlugin => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "builtin-plugin subagents are read-only compile-time constants; to override, place a same-named .md in <project>/.everlasting/workflow/dev/agents/",
            ));
        }
    };
    Ok(path)
}

/// Core mtime-fence read: stat the dir, compare against the cached
/// mtimes; on a full match return the cached clone, otherwise re-scan.
pub(crate) async fn read_through(
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
