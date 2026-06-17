//! B4 Stretch 2 — merged `/`-trigger panel (command + skill) IPC.
//!
//! The B3 `command_palette` module owns the B3 `/command` surface
//! (list + body fetch) and stays untouched (B3 zero-regression rule).
//! This module adds the *merged* panel that the frontend's
//! `<TriggerMenu>` uses: a single source-of-truth list of "things you
//! can type `/` to invoke" (builtin + custom command + skill), plus a
//! body-fetch IPC for skills (mirrors `get_command_body`).
//!
//! Cross-type name collision rules (Stretch 2 grill-converged):
//! 1. **builtin always wins** — same as B3's "custom collides with
//!    builtin → skip + warn" rule, but extended across types so a
//!    SKILL.md whose `name:` is `clear` (collides with B3's
//!    `/clear`) is also skipped.
//! 2. **skill (superset) covers custom command** — when a custom
//!    command and a skill share a name, the skill wins (the panel
//!    surfaces the skill; the command is hidden). The skill's
//!    `name` is what we route to, and `get_skill_body` returns the
//!    SKILL.md body for the user-message path.
//! 3. **project overrides user** (within type) — B3's per-type
//!    precedence is preserved.
//! 4. **unknown skill name → `get_skill_body` returns `None`** — the
//!    frontend's existing `get_command_body` path already handles a
//!    `None` body gracefully (shows a toast, no send).
//!
//! This module is intentionally **read-only** — the agent loop /
//! `use_skill` tool are not modified (Stretch 2 is a user-driven
//! path, separate from the LLM-driven `use_skill` virtual tool).
//! See `.trellis/tasks/06-18-skill-stretches/prd.md` §"Stretch 2"
//! for the full grill decision log.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::resource_loader::{BUILTIN_COMMANDS, CommandInfo, list_all as list_commands_all};
use crate::skill::loader::list_skill_infos;
use crate::state::AppState;

/// One row in the merged `/`-trigger panel.
///
/// `source` is one of `"builtin"` / `"command"` / `"skill"`. The
/// frontend's `onCommandSelect` dispatcher reads it to pick the
/// right path:
/// - `builtin` → client-side action (B3's `executeCommand`:
///   `/help` / `/clear` / `/new`)
/// - `command` → `get_command_body` → user message
/// - `skill` → `get_skill_body` → user message
#[derive(Serialize, Clone)]
pub struct PanelItem {
 pub name: String,
 pub description: String,
 pub argument_hint: Option<String>,
 /// `"builtin"` | `"command"` | `"skill"`.
 pub source: String,
 /// True only for `source == "builtin"`. Mirrors `CommandInfo`'s
 /// `is_builtin` so the frontend can keep using the same dispatcher
 /// logic (B3 PR2).
 pub is_builtin: bool,
}

/// List the merged `/`-trigger panel: builtins (always) + custom
/// commands (user + project layers) + skills (user + project
/// layers), deduped by name with the cross-type priority rules
/// from the module docstring.
///
/// The output is sorted alphabetically for a stable panel display,
/// matching the B3 `list_commands` contract. The frontend renders
/// each row with a `source` chip so the user can tell at a glance
/// which type they're picking.
#[tauri::command]
pub async fn list_panel_items(
    state: State<'_, Arc<AppState>>,
    project_id: Option<String>,
) -> Result<Vec<PanelItem>, String> {
    let project_path = resolve_project_path(&state, project_id.as_deref()).await?;

    // Skills: list_skill_infos already handles project > user
    // precedence. We use it as-is (no dedup across the same-type
    // entries because the function is already project-overrides-user
    // collapsed).
    let skill_infos = list_skill_infos(&state.skill_cache, project_path.as_deref()).await;

    // Custom commands: B3 list_all returns CommandInfo with
    // is_builtin = false for the user/project ones; the same
    // function already filters out builtins, so we don't need a
    // second builtin-skip here. (Builtins are added directly below
    // from BUILTIN_COMMANDS so we have ONE source of truth.)
    let command_infos: Vec<CommandInfo> =
        list_commands_all(&state.command_cache, project_path.as_deref()).await;
    let custom_commands: Vec<CommandInfo> = command_infos
        .into_iter()
        .filter(|c| !c.is_builtin)
        .collect();

    // Build the inputs for the pure dedup helper.
    let builtins: Vec<BuiltinStub> = BUILTIN_COMMANDS
        .iter()
        .map(|b| BuiltinStub {
            name: b.name.to_string(),
            description: b.description.to_string(),
        })
        .collect();
    let commands: Vec<CommandStub> = custom_commands
        .iter()
        .map(|c| CommandStub {
            name: c.name.clone(),
            description: c.description.clone(),
            argument_hint: c.argument_hint.clone(),
        })
        .collect();
    let skills: Vec<SkillStub> = skill_infos
        .iter()
        .map(|s| SkillStub {
            name: s.name.clone(),
            description: s.description.clone(),
        })
        .collect();

    // Emit the cross-type "skill beats command" / "builtin beats
    // skill" warn! logs at the Tauri boundary (the helper is pure
    // and doesn't touch tracing). We compute the dropped names so
    // the operator can debug "why isn't my command/skill showing?".
    let skill_names: std::collections::HashSet<&str> =
        skills.iter().map(|s| s.name.as_str()).collect();
    let builtin_names: std::collections::HashSet<&str> =
        builtins.iter().map(|b| b.name.as_str()).collect();
    for c in &commands {
        if skill_names.contains(c.name.as_str()) {
            tracing::warn!(
                name = %c.name,
                "panel: custom command covered by a skill of the same name; the skill takes precedence"
            );
        }
    }
    for s in &skills {
        if builtin_names.contains(s.name.as_str()) {
            tracing::warn!(
                name = %s.name,
                "panel: skill collides with a builtin; the builtin takes precedence (skill skipped)"
            );
        }
    }

    Ok(dedup_panel(&builtins, &commands, &skills))
}

/// Fetch a skill's body for the user-message path. Mirrors
/// `get_command_body`: returns `Some(body)` when the skill exists,
/// `None` otherwise. The frontend treats `None` as a "skill
/// vanished" toast + no-send (same contract as the command path).
#[tauri::command]
pub async fn get_skill_body(
    state: State<'_, Arc<AppState>>,
    name: String,
    project_id: Option<String>,
) -> Result<Option<String>, String> {
    let project_path = resolve_project_path(&state, project_id.as_deref()).await?;
    match crate::skill::loader::find_skill(&state.skill_cache, &name, project_path.as_deref())
        .await
    {
        Some(skill) => {
            tracing::info!(
                name = %skill.name,
                path = %skill.path.display(),
                "skill body fetched"
            );
            Ok(Some(skill.body))
        }
        None => Ok(None),
    }
}

/// Resolve a project id to its path, mirroring `command_palette`'s
/// `list_commands` / `get_command_body` body so a missing project
/// surfaces the same error string the B3 IPCs do.
async fn resolve_project_path(
    state: &State<'_, Arc<AppState>>,
    project_id: Option<&str>,
) -> Result<Option<String>, String> {
    match project_id {
        Some(pid) => crate::db::get_project(&state.db, pid)
            .await
            .map(|opt| opt.map(|p| p.path))
            .map_err(|e| format!("panel: get_project failed: {}", e)),
        None => Ok(None),
    }
}

/// Pure dedup function (testable without Tauri state). Given the
/// three input lists (always-non-empty builtins, custom commands,
/// skills) apply the cross-type priority rules from the module
/// docstring, returning the merged panel list sorted by name.
///
/// - builtins are kept as-is (the only type that survives a name
///   collision with anything).
/// - a custom command whose name matches a skill name is dropped
///   (skill wins).
/// - a skill whose name matches a builtin name is dropped (with a
///   `warn!`; the warn is the caller's responsibility because the
///   helper is pure).
///
/// Tests in `tests` mod below exercise each rule.
fn dedup_panel(
    builtins: &[BuiltinStub],
    commands: &[CommandStub],
    skills: &[SkillStub],
) -> Vec<PanelItem> {
    let builtin_names: std::collections::HashSet<&str> =
        builtins.iter().map(|b| b.name.as_str()).collect();
    let skill_names: std::collections::HashSet<&str> =
        skills.iter().map(|s| s.name.as_str()).collect();

    let mut items: Vec<PanelItem> = Vec::new();
    for b in builtins {
        items.push(PanelItem {
            name: b.name.clone(),
            description: b.description.clone(),
            argument_hint: None,
            source: "builtin".to_string(),
            is_builtin: true,
        });
    }
    for c in commands {
        if skill_names.contains(c.name.as_str()) {
            continue;
        }
        items.push(PanelItem {
            name: c.name.clone(),
            description: c.description.clone(),
            argument_hint: c.argument_hint.clone(),
            source: "command".to_string(),
            is_builtin: false,
        });
    }
    for s in skills {
        if builtin_names.contains(s.name.as_str()) {
            continue;
        }
        items.push(PanelItem {
            name: s.name.clone(),
            description: s.description.clone(),
            argument_hint: None,
            source: "skill".to_string(),
            is_builtin: false,
        });
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    items
}

struct BuiltinStub {
    name: String,
    description: String,
}

struct CommandStub {
    name: String,
    description: String,
    argument_hint: Option<String>,
}

struct SkillStub {
    name: String,
    description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(name: &str) -> BuiltinStub {
        BuiltinStub {
            name: name.to_string(),
            description: format!("builtin {name}"),
        }
    }
    fn c(name: &str) -> CommandStub {
        CommandStub {
            name: name.to_string(),
            description: format!("cmd {name}"),
            argument_hint: None,
        }
    }
    fn s(name: &str) -> SkillStub {
        SkillStub {
            name: name.to_string(),
            description: format!("skill {name}"),
        }
    }

    #[test]
    fn dedup_builtin_always_present() {
        // Builtins must appear even when no commands/skills exist.
        let out = dedup_panel(&[b("clear")], &[], &[]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].source, "builtin");
        assert!(out[0].is_builtin);
    }

    #[test]
    fn dedup_builtin_wins_over_skill_collision() {
        // A skill named `clear` collides with the builtin `clear` →
        // skill is dropped, builtin stays. (The Tauri-level call
        // also logs a `warn!`; the helper is pure so the warn is
        // not exercised here, but the outcome is identical.)
        let out = dedup_panel(&[b("clear")], &[], &[s("clear")]);
        let names: Vec<&str> = out.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["clear"]);
        assert_eq!(out[0].source, "builtin");
    }

    #[test]
    fn dedup_skill_wins_over_command_collision() {
        // A custom command and a skill share a name → skill takes
        // precedence (skill is the command's superset per Claude
        // Code / B4 survey §5.3). The custom command is dropped.
        let out = dedup_panel(&[], &[c("review-pr")], &[s("review-pr")]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "review-pr");
        assert_eq!(out[0].source, "skill");
    }

    #[test]
    fn dedup_no_collisions_all_three_types_present() {
        // Each type gets its own slot. Names are all distinct so
        // no dedup fires.
        let out = dedup_panel(
            &[b("clear")],
            &[c("commit")],
            &[s("review-pr")],
        );
        let names: Vec<&str> = out.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["clear", "commit", "review-pr"]);
        let sources: Vec<&str> = out.iter().map(|i| i.source.as_str()).collect();
        assert_eq!(sources, vec!["builtin", "command", "skill"]);
    }

    #[test]
    fn dedup_sorted_alphabetically() {
        // Stable alphabetical order for panel display.
        let out = dedup_panel(
            &[b("zeta")],
            &[c("alpha"), c("mike")],
            &[s("beta")],
        );
        let names: Vec<&str> = out.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta", "mike", "zeta"]);
    }

    /// The "project > user" precedence for skills is already
    /// covered by `skill::loader::list_skill_infos` tests; here we
    /// just make sure that the dedup helper preserves both rows
    /// when the names are distinct (no false-positive collision).
    #[test]
    fn dedup_distinct_skill_and_command_coexist() {
        let out = dedup_panel(
            &[],
            &[c("commit")],
            &[s("review-pr")],
        );
        assert_eq!(out.len(), 2);
        let sources: std::collections::HashSet<&str> =
            out.iter().map(|i| i.source.as_str()).collect();
        assert!(sources.contains("command"));
        assert!(sources.contains("skill"));
    }
}
