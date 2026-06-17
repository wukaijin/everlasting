//! B4 Skill loader — mtime-fenced scan of user/project skill dirs.
//!
//! Mirrors the B3 `resource_loader` shape (read-through mtime fence,
//! hand-rolled frontmatter parser, precedence merge) with one
//! structural delta: a skill is a **directory** containing `SKILL.md`
//! (vs a command's single `*.md` file), so the scan walks subdirs.
//!
//! Precedence (high → low): **project > user**. No builtins (unlike
//! commands, which carry `/help` `/clear` `/new`). `user_dir` naming
//! matches `resource_loader` so both layers share the same config root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::Serialize;
use tokio::sync::RwLock;

use crate::llm::types::{CacheControl, ContentBlock};
use crate::memory::file::user_dir;

/// Subdirectory under both the user config dir and the project root
/// that holds skill directories (`<name>/SKILL.md`).
const SKILLS_SUBDIR: &str = "skills";

/// `.everlasting/` project-local namespace (shared with B3 commands
/// and shell output spillover). Skills live under `skills/`.
const PROJECT_NAMESPACE: &str = ".everlasting";

/// The single Markdown file inside each skill directory that carries
/// the frontmatter + instruction body. Additional files in the dir
/// (`reference.md`, `examples/`, …) are NOT scanned here — the model
/// pulls them via `read_file` on demand (L2 progressive disclosure).
const SKILL_FILENAME: &str = "SKILL.md";

/// Single SKILL.md size cap (defensive — a skill is an instruction
/// template, not a content dump). Mirrors B3's `MAX_COMMAND_FILE_SIZE`.
const MAX_SKILL_FILE_SIZE: u64 = 64 * 1024; // 64 KiB

/// Where a skill came from. `Project` overrides `User` on a name
/// collision (matches B3's project-over-user precedence, minus
/// builtins which skills don't have).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
 User,
 Project,
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

/// Frontmatter parsed from a SKILL.md (hand-rolled; scalars + a single
/// array field, same parser shape as B3 with a hand-rolled array
/// extension for `allowed-tools`). MVP fields: `name`, `description`,
/// `allowed-tools`. The array parser is a thin wrapper (~20 lines) over
/// the B3 scalar apply path: strip `[` `]`, comma split, trim, dedup.
/// See `.trellis/tasks/06-18-skill-stretches/prd.md` Stretch 1 §"parser
/// 升级决策" for the YAGNI justification (graduate to `serde_yaml_neo`
/// only when complex / multi-line fields appear).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Frontmatter {
 name: Option<String>,
 description: Option<String>,
 allowed_tools: Vec<String>,
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
// Frontmatter parser (hand-rolled, copied from B3 — scalar only)
// ---------------------------------------------------------------------------

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
fn parse_allowed_tools(raw: &str) -> Vec<String> {
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
async fn scan_skill_dir(dir: &Path, source: SkillSource) -> Vec<SkillResource> {
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
 let (fm, body) = parse_frontmatter(&content);
 // Name: frontmatter `name` wins; else the parent directory name.
 // Require non-empty (dir_name always non-empty here, but frontmatter
 // could be whitespace-only).
 let name = fm
  .name
  .clone()
  .filter(|n| !n.trim().is_empty())
  .unwrap_or_else(|| dir_name.to_string());
 if name.trim().is_empty() {
  tracing::warn!(
   path = %skill_path.display(),
   "skills: no name (frontmatter + dir name both empty), skipping"
  );
  return Ok(None);
 }
 Ok(Some(SkillResource {
  name,
  description: fm.description.unwrap_or_default(),
  body,
  path: skill_path.to_path_buf(),
  source,
  allowed_tools: fm.allowed_tools,
 }))
}

// ---------------------------------------------------------------------------
// SkillCache — read-through with an mtime fence (copied from B3)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct CachedScan {
 resources: Vec<SkillResource>,
 /// `<name>/SKILL.md` path → mtime at scan time. Compared against
 /// `current_mtimes` on every read; any difference (changed mtime OR
 /// a SKILL.md appearing/vanishing — i.e. a subdir added/removed)
 /// triggers a re-scan.
 mtimes: HashMap<PathBuf, Option<SystemTime>>,
}

/// Process-wide cache of scanned skill dirs, held in `AppState`.
///
/// Freshness is decided at read time by an mtime fence (no background
/// watcher): each read stats the dir's SKILL.md files, compares against
/// the cached mtimes, and re-scans only on a difference. Same pattern
/// as `resource_loader::CommandCache`.
pub struct SkillCache {
 user: RwLock<Option<CachedScan>>,
 project: RwLock<HashMap<String, CachedScan>>,
}

impl SkillCache {
 pub fn arc() -> Arc<Self> {
  Arc::new(Self {
   user: RwLock::new(None),
   project: RwLock::new(HashMap::new()),
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
}

/// Core mtime-fence read: stat the dir, compare against the cached
/// mtimes; on a full match return the cached clone, otherwise re-scan.
async fn read_through(
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
// find_skill / list_skill_infos — precedence merge (project > user)
// ---------------------------------------------------------------------------

fn resource_to_info(r: &SkillResource) -> SkillInfo {
 SkillInfo {
  name: r.name.clone(),
  description: r.description.clone(),
  source: match r.source {
   SkillSource::User => "user",
   SkillSource::Project => "project",
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
pub async fn list_skill_infos(
 cache: &SkillCache,
 project_path: Option<&str>,
) -> Vec<SkillInfo> {
 let mut by_name: HashMap<String, SkillResource> = HashMap::new();
 for r in cache.list_user().await {
  by_name.insert(r.name.clone(), r);
 }
 if let Some(pp) = project_path {
  for r in cache.list_project(pp).await {
   by_name.insert(r.name.clone(), r);
  }
 }
 let mut infos: Vec<SkillInfo> = by_name.into_values().map(|r| resource_to_info(&r)).collect();
 infos.sort_by(|a, b| a.name.cmp(&b.name));
 infos
}

/// Look up a single skill by name (project overrides user). Returns
/// the full resource including `body` (for L1 activation as the
/// `use_skill` tool_result) and `path` (for tracing).
pub async fn find_skill(
 cache: &SkillCache,
 name: &str,
 project_path: Option<&str>,
) -> Option<SkillResource> {
 if let Some(pp) = project_path {
  if let Some(r) = cache.list_project(pp).await.into_iter().find(|r| r.name == name) {
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
  "<available-skills>\nThese skills are available. Call the `use_skill` tool with a skill's name when the task matches its description.\n{}\n</available-skills>",
  lines.join("\n")
 );
 vec![ContentBlock::Text {
  text,
  cache_control: Some(CacheControl::Ephemeral),
 }]
}

#[cfg(test)]
mod tests {
 use super::*;
 use crate::memory::file::set_user_dir_for_test;

 /// Write `<dir>/<name>/SKILL.md` with the given body, returning the
 /// skill dir path (parent of SKILL.md).
 fn write_skill(parent: &Path, name: &str, body: &str) -> PathBuf {
  let skill_dir = parent.join(name);
  std::fs::create_dir_all(&skill_dir).unwrap();
  let path = skill_dir.join(SKILL_FILENAME);
  std::fs::write(&path, body).unwrap();
  path
 }

 // ---- frontmatter parser ----

 #[test]
 fn frontmatter_full() {
  let input = "---\nname: review-pr\ndescription: review 一个 PR\n---\n看 diff 给反馈。";
  let (fm, body) = parse_frontmatter(input);
  assert_eq!(fm.name.as_deref(), Some("review-pr"));
  assert_eq!(fm.description.as_deref(), Some("review 一个 PR"));
  assert_eq!(body, "看 diff 给反馈。");
 }

 #[test]
 fn frontmatter_absent_whole_file_is_body() {
  let input = "no frontmatter\njust body";
  let (fm, body) = parse_frontmatter(input);
  assert!(fm.name.is_none());
  assert_eq!(body, input);
 }

 #[test]
 fn frontmatter_partial_keys() {
  let (fm, body) = parse_frontmatter("---\nname: only\n---\nbody");
  assert_eq!(fm.name.as_deref(), Some("only"));
  assert!(fm.description.is_none());
  assert_eq!(body, "body");
 }

 #[test]
 fn frontmatter_strips_quotes() {
  let (fm, _) = parse_frontmatter("---\nname: \"q\"\ndescription: 's'\n---\nb");
  assert_eq!(fm.name.as_deref(), Some("q"));
  assert_eq!(fm.description.as_deref(), Some("s"));
 }

 #[test]
 fn apply_kv_ignores_comments_blank_unknown() {
  let mut fm = Frontmatter::default();
  apply_kv(&mut fm, "# comment");
  apply_kv(&mut fm, "");
  apply_kv(&mut fm, "weird: x");
  apply_kv(&mut fm, "name: real");
  assert_eq!(fm.name.as_deref(), Some("real"));
  assert!(fm.description.is_none());
 }

 // ---- directory scan (subdir walk — the delta from B3) ----

 #[tokio::test]
 async fn scan_parses_subdirs_ignores_loose_files() {
  let tmp = tempfile::TempDir::new().unwrap();
  write_skill(tmp.path(), "review-pr", "---\nname: review-pr\ndescription: d\n---\nb1");
  write_skill(tmp.path(), "commit", "---\nname: commit\n---\nb2");
  // A loose .md file at the skills/ root is NOT a skill (skills are dirs).
  std::fs::write(tmp.path().join("stray.md"), "x").unwrap();

  let mut res = scan_skill_dir(tmp.path(), SkillSource::User).await;
  res.sort_by(|a, b| a.name.cmp(&b.name));
  assert_eq!(res.len(), 2, "loose stray.md must be ignored");
  assert_eq!(res[0].name, "commit");
  assert_eq!(res[0].description, "");
  assert_eq!(res[1].name, "review-pr");
  assert_eq!(res[1].description, "d");
 }

 #[tokio::test]
 async fn scan_name_falls_back_to_dir_name() {
  let tmp = tempfile::TempDir::new().unwrap();
  write_skill(tmp.path(), "deploy", "no frontmatter body");
  let res = scan_skill_dir(tmp.path(), SkillSource::Project).await;
  assert_eq!(res.len(), 1);
  assert_eq!(res[0].name, "deploy");
  assert_eq!(res[0].source, SkillSource::Project);
 }

 #[tokio::test]
 async fn scan_skips_subdir_without_skill_md() {
  let tmp = tempfile::TempDir::new().unwrap();
  write_skill(tmp.path(), "real", "---\nname: real\n---\nb");
  // A subdir with no SKILL.md is silently skipped.
  std::fs::create_dir_all(tmp.path().join("empty")).unwrap();
  std::fs::write(tmp.path().join("empty").join("README.md"), "x").unwrap();

  let res = scan_skill_dir(tmp.path(), SkillSource::User).await;
  assert_eq!(res.len(), 1);
  assert_eq!(res[0].name, "real");
 }

 #[tokio::test]
 async fn scan_skips_over_cap_file() {
  let tmp = tempfile::TempDir::new().unwrap();
  let skill_dir = tmp.path().join("big");
  std::fs::create_dir_all(&skill_dir).unwrap();
  let big = "x".repeat((MAX_SKILL_FILE_SIZE + 1) as usize);
  std::fs::write(skill_dir.join(SKILL_FILENAME), big).unwrap();
  assert!(scan_skill_dir(tmp.path(), SkillSource::User).await.is_empty());
 }

 #[tokio::test]
 async fn scan_missing_dir_returns_empty() {
  let res = scan_skill_dir(Path::new("/no/such/everlasting/skills/xyz"), SkillSource::User).await;
  assert!(res.is_empty());
 }

 // ---- mtime fence ----

 #[tokio::test]
 async fn read_through_re_scans_on_change() {
  let tmp = tempfile::TempDir::new().unwrap();
  let dir = tmp.path().to_path_buf();
  write_skill(&dir, "a", "---\nname: a\n---\nv1");
  let cached = read_through(&dir, SkillSource::User, None).await;
  assert_eq!(cached.resources[0].body, "v1");

  // Unchanged → cache hit (same mtimes returned).
  let hit = read_through(&dir, SkillSource::User, Some(&cached)).await;
  assert_eq!(hit.mtimes, cached.mtimes);

  // Change content + advance mtime → re-scan sees new body.
  tokio::time::sleep(std::time::Duration::from_millis(15)).await;
  std::fs::write(dir.join("a").join(SKILL_FILENAME), "---\nname: a\n---\nv2").unwrap();
  let updated = read_through(&dir, SkillSource::User, Some(&cached)).await;
  assert_eq!(updated.resources[0].body, "v2");
 }

 #[tokio::test]
 async fn read_through_re_scans_on_subdir_added() {
  let tmp = tempfile::TempDir::new().unwrap();
  let dir = tmp.path().to_path_buf();
  write_skill(&dir, "a", "---\nname: a\n---\nb");
  let cached = read_through(&dir, SkillSource::User, None).await;
  assert_eq!(cached.resources.len(), 1);

  // Add a new skill subdir → mtimes map grows → re-scan sees it.
  tokio::time::sleep(std::time::Duration::from_millis(15)).await;
  write_skill(&dir, "b", "---\nname: b\n---\nb");
  let updated = read_through(&dir, SkillSource::User, Some(&cached)).await;
  assert_eq!(updated.resources.len(), 2);
 }

 // ---- list_skill_infos precedence (project > user) ----

 #[tokio::test]
 async fn list_infos_project_overrides_user() {
  let user_tmp = tempfile::TempDir::new().unwrap();
  let user_skills = user_tmp.path().join(SKILLS_SUBDIR);
  std::fs::create_dir_all(&user_skills).unwrap();
  write_skill(&user_skills, "shared", "---\nname: shared\ndescription: from-user\n---\nub");

  let proj_tmp = tempfile::TempDir::new().unwrap();
  let proj_skills = proj_tmp.path().join(PROJECT_NAMESPACE).join(SKILLS_SUBDIR);
  std::fs::create_dir_all(&proj_skills).unwrap();
  write_skill(&proj_skills, "shared", "---\nname: shared\ndescription: from-project\n---\npb");

  let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
  let cache = SkillCache::arc();
  let project_path = proj_tmp.path().to_string_lossy().to_string();
  let infos = list_skill_infos(&cache, Some(&project_path)).await;
  set_user_dir_for_test(prev);

  let shared = infos.iter().find(|i| i.name == "shared").unwrap();
  assert_eq!(shared.description, "from-project");
  assert_eq!(shared.source, "project");
 }

 #[tokio::test]
 async fn find_skill_returns_body_for_l1_activation() {
  let user_tmp = tempfile::TempDir::new().unwrap();
  let user_skills = user_tmp.path().join(SKILLS_SUBDIR);
  std::fs::create_dir_all(&user_skills).unwrap();
  write_skill(&user_skills, "commit", "---\nname: commit\ndescription: d\n---\nBODY");

  let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
  let cache = SkillCache::arc();
  let res = find_skill(&cache, "commit", None).await;
  set_user_dir_for_test(prev);

  let res = res.expect("commit skill should resolve");
  assert_eq!(res.body, "BODY");
  assert!(res.path.ends_with(SKILL_FILENAME));
 }

 #[tokio::test]
 async fn find_skill_unknown_returns_none() {
  let user_tmp = tempfile::TempDir::new().unwrap();
  let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
  let cache = SkillCache::arc();
  let res = find_skill(&cache, "does-not-exist", None).await;
  set_user_dir_for_test(prev);
  assert!(res.is_none());
 }

 // ---- build_skill_listing_block (L0 injection) ----

 #[test]
 fn listing_empty_returns_no_blocks() {
  let blocks = build_skill_listing_block(&[]);
  assert!(blocks.is_empty(), "no skills → no listing message");
 }

 #[test]
 fn listing_renders_name_and_description() {
  let infos = vec![
   SkillInfo {
    name: "review-pr".into(),
    description: "review 一个 PR".into(),
    source: "project".into(),
    allowed_tools: vec![],
   },
   SkillInfo {
    name: "commit".into(),
    description: "".into(),
    source: "user".into(),
    allowed_tools: vec![],
   },
  ];
  let blocks = build_skill_listing_block(&infos);
  assert_eq!(blocks.len(), 1);
  let ContentBlock::Text { text, cache_control } = &blocks[0] else {
   panic!("expected Text block");
  };
  assert!(text.contains("- review-pr: review 一个 PR"));
  assert!(text.contains("- commit"));
  assert!(!text.contains("- commit: "), "empty description omits the colon");
  assert!(!text.contains("tools:"), "empty allowed_tools must not render a suffix");
  assert!(text.contains("use_skill"));
  assert_eq!(*cache_control, Some(CacheControl::Ephemeral));
 }

 // ---- Stretch 1: `allowed-tools` array parse + L0 render ----

 #[test]
 fn parse_allowed_tools_basic() {
  assert_eq!(
   parse_allowed_tools("[read_file, grep, git_diff]"),
   vec!["read_file".to_string(), "grep".to_string(), "git_diff".to_string()]
  );
 }

 #[test]
 fn parse_allowed_tools_dedup_and_trim() {
  // duplicates + extra spaces → dedup + trim, preserve first-seen order
  assert_eq!(
   parse_allowed_tools("[a, a, b , c,  b]"),
   vec!["a".to_string(), "b".to_string(), "c".to_string()]
  );
 }

 #[test]
 fn parse_allowed_tools_empty_array() {
  assert!(parse_allowed_tools("[]").is_empty());
  assert!(parse_allowed_tools("[ , , ]").is_empty(), "whitespace-only items dropped");
 }

 #[test]
 fn parse_allowed_tools_no_brackets_warns_and_empty() {
  // multi-line / nested / no brackets → empty + warn (per Stretch 1
  // tolerant parse — the rest of the skill still loads).
  assert!(parse_allowed_tools("read_file, grep").is_empty());
  assert!(parse_allowed_tools("not_an_array").is_empty());
 }

 #[test]
 fn parse_allowed_tools_unbalanced_brackets_warns() {
  // starts with `[` but does not end with `]` → empty + warn.
  assert!(parse_allowed_tools("[read_file, grep").is_empty());
 }

 #[test]
 fn parse_allowed_tools_strips_quotes() {
  // user writes `allowed-tools: "[a, b]"` (B3 scalar apply would
  // leave the value as `"[a, b]"`; our array parser strips the
  // surrounding quotes before bracket-stripping).
  assert_eq!(
   parse_allowed_tools("\"[a, b]\""),
   vec!["a".to_string(), "b".to_string()]
  );
  assert_eq!(
   parse_allowed_tools("'[c]'"),
   vec!["c".to_string()]
  );
 }

 #[test]
 fn apply_kv_allowed_tools_aliases() {
  // Both `allowed-tools` and `allowed_tools` (snake_case) accepted.
  let mut fm = Frontmatter::default();
  apply_kv(&mut fm, "allowed-tools: [a, b]");
  assert_eq!(fm.allowed_tools, vec!["a".to_string(), "b".to_string()]);
  let mut fm = Frontmatter::default();
  apply_kv(&mut fm, "allowed_tools: [c, d]");
  assert_eq!(fm.allowed_tools, vec!["c".to_string(), "d".to_string()]);
 }

 #[test]
 fn frontmatter_parses_allowed_tools() {
  // End-to-end: a real SKILL.md frontmatter with allowed-tools
  // populates `allowed_tools` on the resulting `Frontmatter`.
  let input = "---\nname: review-pr\ndescription: d\nallowed-tools: [read_file, grep]\n---\nbody";
  let (fm, body) = parse_frontmatter(input);
  assert_eq!(fm.name.as_deref(), Some("review-pr"));
  assert_eq!(fm.description.as_deref(), Some("d"));
  assert_eq!(fm.allowed_tools, vec!["read_file".to_string(), "grep".to_string()]);
  assert_eq!(body, "body");
 }

 #[test]
 fn frontmatter_missing_allowed_tools_is_empty() {
  // The MVP minimal set: a skill without `allowed-tools` MUST still
  // load — `allowed_tools` is just an empty Vec, not an error.
  let (fm, _) = parse_frontmatter("---\nname: x\ndescription: y\n---\nb");
  assert!(fm.allowed_tools.is_empty());
 }

 #[test]
 fn listing_renders_allowed_tools_suffix() {
  // When `allowed_tools` is non-empty, the listing line carries
  // `  (tools: a, b)` after the description. When empty, no suffix.
  let infos = vec![
   SkillInfo {
    name: "review-pr".into(),
    description: "review 一个 PR".into(),
    source: "project".into(),
    allowed_tools: vec!["read_file".into(), "grep".into()],
   },
   SkillInfo {
    name: "commit".into(),
    description: "".into(),
    source: "user".into(),
    allowed_tools: vec![],
   },
  ];
  let blocks = build_skill_listing_block(&infos);
  let ContentBlock::Text { text, .. } = &blocks[0] else {
   panic!("expected Text block");
  };
  assert!(
   text.contains("- review-pr: review 一个 PR  (tools: read_file, grep)"),
   "allowed-tools suffix should appear after description, got: {text}"
  );
  assert!(
   !text.contains("- commit  (tools:"),
   "empty allowed_tools must NOT render a (tools: ...) suffix, got: {text}"
  );
 }

 #[test]
 fn listing_renders_allowed_tools_with_empty_description() {
  // Edge case: a skill with an empty description but a non-empty
  // `allowed-tools`. The line should be `- name  (tools: ...)`
  // (no colon, but the suffix IS present).
  let infos = vec![SkillInfo {
   name: "minimal".into(),
   description: "".into(),
   source: "user".into(),
   allowed_tools: vec!["shell".into()],
  }];
  let blocks = build_skill_listing_block(&infos);
  let ContentBlock::Text { text, .. } = &blocks[0] else {
   panic!("expected Text block");
  };
  assert!(
   text.contains("- minimal  (tools: shell)"),
   "expected `<name>  (tools: ...)` shape, got: {text}"
  );
 }
}
