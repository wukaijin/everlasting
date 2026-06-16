//! B3 `/command` resource loader.
//!
//! Scans user (`~/.config/everlasting/commands/*.md`) and project
//! (`<project>/.everlasting/commands/*.md`) directories for custom
//! command files (frontmatter + Markdown body), with a read-through
//! mtime fence (RULE-C-001 pattern, same idea as `memory::loader`) so
//! a re-scan only happens when a file's mtime changed.
//!
//! Frontmatter parsing is **hand-rolled** (split `---` + `key: value`
//! scalars). `serde_yml` / `serde_yaml` are both deprecated (TECH.md
//! §1.4 is stale); the B3 command frontmatter is name / description /
//! argument-hint only, so a ~40-line parser suffices. Future
//! Skill / Memory / Role loaders with complex frontmatter can graduate
//! to a maintained YAML crate (`serde_yaml_neo`) — the parser lives
//! behind `parse_frontmatter` so the swap is local (TECH.md §5 shared
//! loader contract).
//!
//! Precedence (high → low): **builtin > project > user**. A custom
//! command whose name collides with a builtin is skipped with a `warn!`
//! (the user picks a different name or uses a `/custom:` prefix later).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::Serialize;
use tokio::sync::RwLock;

use crate::memory::file::user_dir;

/// Subdirectory under both the user config dir and the project root
/// that holds custom command files (`*.md`).
const COMMANDS_SUBDIR: &str = "commands";

/// `.everlasting/` project-local namespace (also used by shell output
/// spillover — see `tools::shell`). Commands live under `commands/`.
const PROJECT_NAMESPACE: &str = ".everlasting";

/// Single command file size cap (defensive — a command is a prompt
/// template, not a content dump). Mirrors memory's `MAX_FILE_SIZE`
/// philosophy but tighter.
const MAX_COMMAND_FILE_SIZE: u64 = 64 * 1024; // 64 KiB

/// Where a custom command came from. `Project` overrides `User` on a
/// name collision; builtins are a separate category (handled by the
/// frontend, never sent to the LLM as a template body).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandSource {
 User,
 Project,
}

/// A parsed custom command file.
#[derive(Clone, Debug)]
pub struct CommandResource {
 pub name: String,
 pub description: String,
 pub argument_hint: Option<String>,
 /// Markdown body — sent to the LLM as the user message when the
 /// user invokes `/name` (template interpolation is v2).
 pub body: String,
 pub path: PathBuf,
 pub source: CommandSource,
}

/// Frontmatter parsed from a command file (hand-rolled; scalars only).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Frontmatter {
 name: Option<String>,
 description: Option<String>,
 argument_hint: Option<String>,
}

/// Wire DTO for the `list_commands` IPC. The frontend renders the
/// command palette from this list. `source` is `"builtin"` / `"user"`
/// / `"project"`; `is_builtin` is a convenience flag (builtins have no
/// template body and are dispatched to existing frontend actions).
#[derive(Serialize, Clone)]
pub struct CommandInfo {
 pub name: String,
 pub description: String,
 pub argument_hint: Option<String>,
 pub source: String,
 pub is_builtin: bool,
}

/// A builtin command handled entirely by the frontend (no file, no
/// template body). The frontend's `executeCommand` dispatches these to
/// existing actions (`/clear` → `clear_session_messages`, `/new` →
/// `createSession`) or a self-listing view (`/help`).
pub struct BuiltinCommand {
 pub name: &'static str,
 pub description: &'static str,
}

/// The builtin command set for B3 MVP. `/mode` and `/model` are
/// intentionally omitted (existing ModeSelect / ModelSelect buttons
/// already cover them); `/compact` is deferred.
pub const BUILTIN_COMMANDS: &[BuiltinCommand] = &[
 BuiltinCommand {
 name: "help",
 description: "列出所有可用命令",
 },
 BuiltinCommand {
 name: "clear",
 description: "清空当前会话消息（保留会话）",
 },
 BuiltinCommand {
 name: "new",
 description: "新建会话",
 },
];

// ---------------------------------------------------------------------------
// Frontmatter parser (hand-rolled, ~40 lines)
// ---------------------------------------------------------------------------

/// Parse a command file into `(frontmatter, body)`.
///
/// Format:
/// ```text
/// ---
/// name: commit
/// description: 用约定格式提交当前变更
/// argument-hint: [可选]
/// ---
/// <markdown body...>
/// ```
///
/// Rules:
/// - The opening `---` fence is optional; if absent, the whole file is
///   the body and `name` is derived from the file stem by the caller.
/// - Keys are single-line `key: value` scalars. Multi-line values /
///   arrays are out of scope (graduates to a YAML crate later).
/// - Values are trimmed; balanced surrounding quotes (`"` / `'`) are
///   stripped. A leading `#` line is treated as a comment (skipped).
/// - Unknown keys are ignored (forward-compat).
fn parse_frontmatter(content: &str) -> (Frontmatter, String) {
 let lines: Vec<&str> = content.lines().collect();
 let mut fm = Frontmatter::default();

 // Skip leading blank lines to find the opening fence.
 let mut idx = 0;
 while idx < lines.len() && lines[idx].trim().is_empty() {
 idx += 1;
 }

 if idx < lines.len() && lines[idx].trim() == "---" {
 // Frontmatter block: collect key:value until the closing `---`.
 idx += 1;
 while idx < lines.len() && lines[idx].trim() != "---" {
 apply_kv(&mut fm, lines[idx]);
 idx += 1;
 }
 // Consume the closing fence.
 if idx < lines.len() && lines[idx].trim() == "---" {
 idx += 1;
 }
 } else {
 // No frontmatter — body is the whole file.
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
 // Strip one layer of balanced surrounding quotes.
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
 "argument-hint" | "argument_hint" => fm.argument_hint = Some(v),
 _ => {}
 }
}

// ---------------------------------------------------------------------------
// Directory scan
// ---------------------------------------------------------------------------

/// Resolve the user commands dir (`~/.config/everlasting/commands/`).
/// `None` if `user_dir()` is unresolvable on this platform.
fn user_commands_dir() -> Option<PathBuf> {
 user_dir().map(|d| d.join(COMMANDS_SUBDIR))
}

/// Resolve a project's commands dir (`<project>/.everlasting/commands/`).
fn project_commands_dir(project_path: &str) -> PathBuf {
 PathBuf::from(project_path)
 .join(PROJECT_NAMESPACE)
 .join(COMMANDS_SUBDIR)
}

/// Stat the `*.md` files in a commands dir, returning a path → mtime
/// map. A file's absence (deleted) or changed mtime invalidates the
/// cached scan. Missing dir → empty map.
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

/// Scan a single commands directory. Bad files (over-cap, non-UTF-8,
/// no name) are skipped with a `warn!` — one bad file never aborts the
/// whole scan (mirrors memory's failure tolerance).
async fn scan_dir(dir: &Path, source: CommandSource) -> Vec<CommandResource> {
 let mut out = Vec::new();
 let mut rd = match tokio::fs::read_dir(dir).await {
 Ok(rd) => rd,
 Err(e) if e.kind() == std::io::ErrorKind::NotFound => return out,
 Err(e) => {
 tracing::warn!(dir = %dir.display(), error = %e, "commands: read_dir failed");
 return out;
 }
 };
 while let Ok(Some(entry)) = rd.next_entry().await {
 let path = entry.path();
 if path.extension().and_then(|e| e.to_str()) != Some("md") {
 continue;
 }
 match load_command_file(&path, source).await {
 Ok(Some(res)) => out.push(res),
 Ok(None) => {} // skipped (reason logged inside)
 Err(e) => tracing::warn!(
 path = %path.display(),
 error = %e,
 "commands: load failed"
 ),
 }
 }
 out
}

/// Load + parse one command file. Returns `Ok(None)` when the file is
/// deliberately skipped (over-cap / no name); `Err` for I/O failures.
async fn load_command_file(
 path: &Path,
 source: CommandSource,
) -> std::io::Result<Option<CommandResource>> {
 let meta = tokio::fs::metadata(path).await?;
 if meta.len() > MAX_COMMAND_FILE_SIZE {
 tracing::warn!(
 path = %path.display(),
 size = meta.len(),
 max = MAX_COMMAND_FILE_SIZE,
 "commands: file exceeds size cap, skipping"
 );
 return Ok(None);
 }
 let content = tokio::fs::read_to_string(path).await?;
 let (fm, body) = parse_frontmatter(&content);
 // Name: frontmatter `name` wins; else the file stem. Require non-empty.
 let name = fm
 .name
 .clone()
 .filter(|n| !n.trim().is_empty())
 .unwrap_or_else(|| {
 path.file_stem()
 .and_then(|s| s.to_str())
 .unwrap_or("")
 .to_string()
 });
 if name.trim().is_empty() {
 tracing::warn!(
 path = %path.display(),
 "commands: no name (frontmatter + file stem both empty), skipping"
 );
 return Ok(None);
 }
 Ok(Some(CommandResource {
 name,
 description: fm.description.unwrap_or_default(),
 argument_hint: fm.argument_hint,
 body,
 path: path.to_path_buf(),
 source,
 }))
}

// ---------------------------------------------------------------------------
// CommandCache — read-through with an mtime fence
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct CachedScan {
 resources: Vec<CommandResource>,
 /// path → mtime at scan time. Compared against `current_mtimes` on
 /// every read; any difference (changed mtime OR a file
 /// appearing/vanishing) triggers a re-scan.
 mtimes: HashMap<PathBuf, Option<SystemTime>>,
}

/// Process-wide cache of scanned command files, held in `AppState`.
///
/// Freshness is decided at read time by an mtime fence (no background
/// watcher): each `list_*` stats the dir's `*.md` files, compares
/// against the cached mtimes, and re-scans only on a difference. A
/// write lock is held across the (cheap) scan — `list_commands` is
/// low-frequency (one `/` trigger), so contention is not a concern.
pub struct CommandCache {
 user: RwLock<Option<CachedScan>>,
 project: RwLock<HashMap<String, CachedScan>>,
}

impl CommandCache {
 pub fn arc() -> Arc<Self> {
 Arc::new(Self {
 user: RwLock::new(None),
 project: RwLock::new(HashMap::new()),
 })
 }

 /// List user-layer commands (mtime-fenced).
 pub async fn list_user(&self) -> Vec<CommandResource> {
 let Some(dir) = user_commands_dir() else {
 return Vec::new();
 };
 let mut guard = self.user.write().await;
 let updated = read_through(&dir, CommandSource::User, guard.as_ref()).await;
 let out = updated.resources.clone();
 *guard = Some(updated);
 out
 }

 /// List project-layer commands (mtime-fenced), keyed by project path.
 pub async fn list_project(&self, project_path: &str) -> Vec<CommandResource> {
 let dir = project_commands_dir(project_path);
 let mut guard = self.project.write().await;
 let cached = guard.get(project_path);
 let updated = read_through(&dir, CommandSource::Project, cached).await;
 let out = updated.resources.clone();
 guard.insert(project_path.to_string(), updated);
 out
 }
}

/// Core mtime-fence read: stat the dir, compare against the cached
/// mtimes; on a full match return the cached clone, otherwise re-scan.
async fn read_through(
 dir: &Path,
 source: CommandSource,
 cached: Option<&CachedScan>,
) -> CachedScan {
 let current = current_mtimes(dir).await;
 if let Some(c) = cached {
 if current == c.mtimes {
 return c.clone();
 }
 }
 let resources = scan_dir(dir, source).await;
 CachedScan {
 resources,
 mtimes: current,
 }
}

// ---------------------------------------------------------------------------
// list_all — merge builtin + user + project with precedence
// ---------------------------------------------------------------------------

fn resource_to_info(r: &CommandResource) -> CommandInfo {
 CommandInfo {
 name: r.name.clone(),
 description: r.description.clone(),
 argument_hint: r.argument_hint.clone(),
 source: match r.source {
 CommandSource::User => "user",
 CommandSource::Project => "project",
 }
 .to_string(),
 is_builtin: false,
 }
}

/// Look up a single custom command by name (project overrides user).
/// Returns the full resource including `body` (for template expansion)
/// and `path` (for tracing). Builtins are NOT matched here — the
/// frontend dispatches builtins to existing actions without a body.
pub async fn find_command(
 cache: &CommandCache,
 name: &str,
 project_path: Option<&str>,
) -> Option<CommandResource> {
 if let Some(pp) = project_path {
  if let Some(r) = cache.list_project(pp).await.into_iter().find(|r| r.name == name) {
   return Some(r);
  }
 }
 cache.list_user().await.into_iter().find(|r| r.name == name)
}

/// Merge builtin + user + project commands into a single wire list.
///
/// Precedence: **builtin > project > user**. A custom command whose
/// name collides with a builtin is skipped (with a `warn!`) — the user
/// renames it or uses a `/custom:` prefix (v2). Custom commands are
/// sorted alphabetically for a stable panel display.
pub async fn list_all(
 cache: &CommandCache,
 project_path: Option<&str>,
) -> Vec<CommandInfo> {
 let builtin_names: HashSet<&str> = BUILTIN_COMMANDS.iter().map(|b| b.name).collect();

 // project overrides user on name collision (project inserted last).
 let mut by_name: HashMap<String, CommandResource> = HashMap::new();
 for r in cache.list_user().await {
 by_name.insert(r.name.clone(), r);
 }
 if let Some(pp) = project_path {
 for r in cache.list_project(pp).await {
 by_name.insert(r.name.clone(), r);
 }
 }

 let mut infos: Vec<CommandInfo> = BUILTIN_COMMANDS
 .iter()
 .map(|b| CommandInfo {
 name: b.name.to_string(),
 description: b.description.to_string(),
 argument_hint: None,
 source: "builtin".to_string(),
 is_builtin: true,
 })
 .collect();

 let mut custom: Vec<CommandInfo> = by_name
 .into_iter()
 .filter_map(|(name, r)| {
 if builtin_names.contains(name.as_str()) {
 tracing::warn!(
 name = %name,
 "commands: custom command collides with builtin, skipping (rename or use /custom: prefix)"
 );
 None
 } else {
 Some(resource_to_info(&r))
 }
 })
 .collect();
 custom.sort_by(|a, b| a.name.cmp(&b.name));
 infos.extend(custom);
 infos
}

#[cfg(test)]
mod tests {
 use super::*;
 use crate::memory::file::set_user_dir_for_test;

 fn write_cmd(dir: &Path, name: &str, body: &str) -> PathBuf {
  let path = dir.join(format!("{name}.md"));
  std::fs::write(&path, body).unwrap();
  path
 }

 // ---- frontmatter parser ----

 #[test]
 fn frontmatter_full() {
  let input = "---\nname: commit\ndescription: 提交变更\nargument-hint: [msg]\n---\n看一下 diff 然后 commit。";
  let (fm, body) = parse_frontmatter(input);
  assert_eq!(fm.name.as_deref(), Some("commit"));
  assert_eq!(fm.description.as_deref(), Some("提交变更"));
  assert_eq!(fm.argument_hint.as_deref(), Some("[msg]"));
  assert_eq!(body, "看一下 diff 然后 commit。");
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
  assert!(fm.argument_hint.is_none());
  assert_eq!(body, "body");
 }

 #[test]
 fn frontmatter_strips_quotes() {
  let (fm, _) = parse_frontmatter("---\nname: \"q\"\ndescription: 's'\n---\nb");
  assert_eq!(fm.name.as_deref(), Some("q"));
  assert_eq!(fm.description.as_deref(), Some("s"));
 }

 #[test]
 fn frontmatter_argument_hint_underscore_alias() {
  let (fm, _) = parse_frontmatter("---\nargument_hint: u\n---\nb");
  assert_eq!(fm.argument_hint.as_deref(), Some("u"));
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

 // ---- directory scan ----

 #[tokio::test]
 async fn scan_parses_valid_files_ignores_non_md() {
  let tmp = tempfile::TempDir::new().unwrap();
  write_cmd(tmp.path(), "commit", "---\nname: commit\ndescription: d\n---\nbody1");
  write_cmd(tmp.path(), "lint", "---\nname: lint\n---\nbody2");
  std::fs::write(tmp.path().join("readme.txt"), "x").unwrap();

  let mut res = scan_dir(tmp.path(), CommandSource::User).await;
  res.sort_by(|a, b| a.name.cmp(&b.name));
  assert_eq!(res.len(), 2);
  assert_eq!(res[0].name, "commit");
  assert_eq!(res[0].description, "d");
  assert_eq!(res[1].name, "lint");
 }

 #[tokio::test]
 async fn scan_name_falls_back_to_stem() {
  let tmp = tempfile::TempDir::new().unwrap();
  write_cmd(tmp.path(), "deploy", "no frontmatter body");
  let res = scan_dir(tmp.path(), CommandSource::Project).await;
  assert_eq!(res.len(), 1);
  assert_eq!(res[0].name, "deploy");
  assert_eq!(res[0].source, CommandSource::Project);
 }

 #[tokio::test]
 async fn scan_skips_over_cap_file() {
  let tmp = tempfile::TempDir::new().unwrap();
  let big = "x".repeat((MAX_COMMAND_FILE_SIZE + 1) as usize);
  std::fs::write(tmp.path().join("big.md"), big).unwrap();
  assert!(scan_dir(tmp.path(), CommandSource::User).await.is_empty());
 }

 #[tokio::test]
 async fn scan_missing_dir_returns_empty() {
  let res =
   scan_dir(Path::new("/no/such/everlasting/dir/xyz"), CommandSource::User).await;
  assert!(res.is_empty());
 }

 // ---- mtime fence ----

 #[tokio::test]
 async fn read_through_re_scans_on_change() {
  let tmp = tempfile::TempDir::new().unwrap();
  let dir = tmp.path().to_path_buf();
  write_cmd(&dir, "a", "---\nname: a\n---\nv1");
  let cached = read_through(&dir, CommandSource::User, None).await;
  assert_eq!(cached.resources[0].body, "v1");

  // Unchanged → cache hit (same mtimes returned).
  let hit = read_through(&dir, CommandSource::User, Some(&cached)).await;
  assert_eq!(hit.mtimes, cached.mtimes);

  // Change content + advance mtime → re-scan sees new body.
  tokio::time::sleep(std::time::Duration::from_millis(15)).await;
  std::fs::write(dir.join("a.md"), "---\nname: a\n---\nv2").unwrap();
  let updated = read_through(&dir, CommandSource::User, Some(&cached)).await;
  assert_eq!(updated.resources[0].body, "v2");
 }

 // ---- list_all precedence (builtin > project > user) ----

 #[tokio::test]
 async fn list_all_builtin_wins_over_user_collision() {
  let user_tmp = tempfile::TempDir::new().unwrap();
  let user_cmd = user_tmp.path().join(COMMANDS_SUBDIR);
  std::fs::create_dir_all(&user_cmd).unwrap();
  std::fs::write(
   user_cmd.join("clear.md"),
   "---\nname: clear\ndescription: mine\n---\nb",
  )
  .unwrap();
  std::fs::write(
   user_cmd.join("deploy.md"),
   "---\nname: deploy\ndescription: d\n---\nb",
  )
  .unwrap();

  let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
  let cache = CommandCache::arc();
  let infos = list_all(&cache, None).await;
  set_user_dir_for_test(prev);

  let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
  assert!(names.contains(&"help") && names.contains(&"clear") && names.contains(&"new"));
  assert!(names.contains(&"deploy"));
  let clear = infos.iter().find(|i| i.name == "clear").unwrap();
  assert!(clear.is_builtin);
  assert_eq!(clear.source, "builtin");
 }

 #[tokio::test]
 async fn list_all_project_overrides_user() {
  let user_tmp = tempfile::TempDir::new().unwrap();
  let user_cmd = user_tmp.path().join(COMMANDS_SUBDIR);
  std::fs::create_dir_all(&user_cmd).unwrap();
  std::fs::write(
   user_cmd.join("shared.md"),
   "---\nname: shared\ndescription: from-user\n---\nub",
  )
  .unwrap();

  let proj_tmp = tempfile::TempDir::new().unwrap();
  let proj_cmd = proj_tmp
   .path()
   .join(PROJECT_NAMESPACE)
   .join(COMMANDS_SUBDIR);
  std::fs::create_dir_all(&proj_cmd).unwrap();
  std::fs::write(
   proj_cmd.join("shared.md"),
   "---\nname: shared\ndescription: from-project\n---\npb",
  )
  .unwrap();

  let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
  let cache = CommandCache::arc();
  let project_path = proj_tmp.path().to_string_lossy().to_string();
  let infos = list_all(&cache, Some(&project_path)).await;
  set_user_dir_for_test(prev);

  let shared = infos.iter().find(|i| i.name == "shared").unwrap();
  assert_eq!(shared.description, "from-project");
  assert_eq!(shared.source, "project");
 }
}
