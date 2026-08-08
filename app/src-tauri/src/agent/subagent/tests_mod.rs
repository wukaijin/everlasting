#![cfg(test)]

use crate::agent::subagent::*;
use crate::llm::ToolDef;

// ---- definition ----

#[test]
fn definition_has_correct_name() {
    assert_eq!(definition().name, DISPATCH_TOOL_NAME);
}

#[test]
fn definition_schema_requires_subagent_and_task() {
    let schema = definition().input_schema;
    let required = schema
        .get("required")
        .and_then(|v| v.as_array())
        .expect("required array present");
    let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(names.contains(&"subagent"));
    assert!(names.contains(&"task"));
}

#[test]
fn definition_schema_subagent_enum_covers_two() {
    let schema = definition().input_schema;
    let enum_vals: Vec<&str> = schema
        .pointer("/properties/subagent/enum")
        .and_then(|v| v.as_array())
        .expect("subagent enum present")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(enum_vals, vec!["researcher", "general-purpose"]);
}

// ---- definition_with_cache (L3d PR3) ----

#[tokio::test]
async fn definition_with_cache_enum_includes_builtins() {
    // Fresh cache + empty project dir → enum is the 2 builtins
    // (alphabetical: general-purpose, researcher).
    let tmp = tempfile::TempDir::new().unwrap();
    let cache = SubagentCache::arc();
    let project_path = tmp.path().to_string_lossy().to_string();
    let def = definition_with_cache(&cache, &project_path, None, &[]).await;
    assert_eq!(def.name, DISPATCH_TOOL_NAME);
    let enum_vals: Vec<String> = def
        .input_schema
        .pointer("/properties/subagent/enum")
        .and_then(|v| v.as_array())
        .expect("enum present")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    assert_eq!(
        enum_vals,
        vec!["general-purpose".to_string(), "researcher".to_string()]
    );
}

/// C5 (2026-07-27): the dispatch enum MUST surface the review
/// plugin's `reviewer` agent when `workflow_name = Some("review")`.
/// Before the fix, `definition_with_cache` called `cache.list`
/// (3 layers, no plugin layer), so `reviewer` never reached the
/// enum while the role gate demanded it — dead-locking the whole
/// review workflow (session 04c62fab).
#[tokio::test]
async fn definition_with_cache_review_plugin_exposes_reviewer() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cache = SubagentCache::arc();
    let project_path = tmp.path().to_string_lossy().to_string();
    let def = definition_with_cache(&cache, &project_path, Some("review"), &[]).await;
    let enum_vals: Vec<String> = def
        .input_schema
        .pointer("/properties/subagent/enum")
        .and_then(|v| v.as_array())
        .expect("enum present")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    // builtins (general-purpose, researcher) + review's reviewer.
    assert!(
        enum_vals.iter().any(|v| v == "reviewer"),
        "review plugin dispatch enum must include `reviewer`; got {:?}",
        enum_vals
    );
    // Sanity: dev's builtins are still there.
    assert!(enum_vals.iter().any(|v| v == "general-purpose"));
}

/// C5: dev plugin must still expose its roles (researcher/
/// implementer/checker) — no regression from the workflow_name
/// threading.
#[tokio::test]
async fn definition_with_cache_dev_plugin_keeps_dev_roles() {
    let tmp = tempfile::TempDir::new().unwrap();
    let cache = SubagentCache::arc();
    let project_path = tmp.path().to_string_lossy().to_string();
    let def = definition_with_cache(&cache, &project_path, Some("dev"), &[]).await;
    let enum_vals: Vec<String> = def
        .input_schema
        .pointer("/properties/subagent/enum")
        .and_then(|v| v.as_array())
        .expect("enum present")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    assert!(
        enum_vals.iter().any(|v| v == "researcher"),
        "dev plugin dispatch enum must keep `researcher`; got {:?}",
        enum_vals
    );
    // review's reviewer must NOT leak into a dev session.
    assert!(
        !enum_vals.iter().any(|v| v == "reviewer"),
        "reviewer leaked into dev session enum: {:?}",
        enum_vals
    );
}

#[tokio::test]
async fn definition_with_cache_description_has_source_tags() {
    // The description must carry `Available subagents:` + per-agent
    // `(source: ...)` tags so the LLM (and the user debugging)
    // can see each subagent's provenance.
    let tmp = tempfile::TempDir::new().unwrap();
    let cache = SubagentCache::arc();
    let project_path = tmp.path().to_string_lossy().to_string();
    let def = definition_with_cache(&cache, &project_path, None, &[]).await;
    let desc = def.description.expect("description present");
    assert!(
        desc.contains("Available subagents:"),
        "description must carry the available-agents line: {}",
        desc
    );
    // Both builtins are source: builtin.
    assert!(
        desc.contains("researcher (source: builtin)"),
        "missing researcher builtin tag: {}",
        desc
    );
    assert!(
        desc.contains("general-purpose (source: builtin)"),
        "missing general-purpose builtin tag: {}",
        desc
    );
}

#[tokio::test]
async fn definition_with_cache_picks_up_user_md() {
    // mtime fence: writing a user .md between calls changes the
    // enum on the next `definition_with_cache` invocation.
    let user_tmp = tempfile::TempDir::new().unwrap();
    let user_agents = user_tmp.path().join("agents");
    std::fs::create_dir_all(&user_agents).unwrap();

    let proj_tmp = tempfile::TempDir::new().unwrap();
    let project_path = proj_tmp.path().to_string_lossy().to_string();

    let prev = crate::memory::file::set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
    let cache = SubagentCache::arc();

    // Initially only builtins.
    let def = definition_with_cache(&cache, &project_path, None, &[]).await;
    let enum_vals: Vec<String> = def
        .input_schema
        .pointer("/properties/subagent/enum")
        .and_then(|v| v.as_array())
        .expect("enum")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    assert_eq!(enum_vals.len(), 2);

    // Write a user .md → next call sees it.
    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    std::fs::write(
        user_agents.join("custom.md"),
        "---\nname: custom\ndescription: my custom agent\ntools: [read_file]\n---\nbody",
    )
    .unwrap();

    let def = definition_with_cache(&cache, &project_path, None, &[]).await;
    let enum_vals: Vec<String> = def
        .input_schema
        .pointer("/properties/subagent/enum")
        .and_then(|v| v.as_array())
        .expect("enum")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    assert_eq!(enum_vals.len(), 3);
    assert!(enum_vals.contains(&"custom".to_string()));

    // Source tag for the user agent is in the description.
    let desc = def.description.expect("description");
    assert!(
        desc.contains("custom (source: user)"),
        "missing custom user tag: {}",
        desc
    );

    crate::memory::file::set_user_dir_for_test(prev);
}

#[tokio::test]
async fn definition_with_cache_project_overrides_builtin() {
    // project > user > builtin precedence: a project .md with the
    // same name as a builtin shows source: project.
    let proj_tmp = tempfile::TempDir::new().unwrap();
    let proj_agents = proj_tmp.path().join(".everlasting").join("agents");
    std::fs::create_dir_all(&proj_agents).unwrap();
    std::fs::write(
        proj_agents.join("researcher.md"),
        "---\nname: researcher\ndescription: project researcher\n---\nCustom prompt.",
    )
    .unwrap();

    let cache = SubagentCache::arc();
    let project_path = proj_tmp.path().to_string_lossy().to_string();
    let def = definition_with_cache(&cache, &project_path, None, &[]).await;
    let desc = def.description.expect("description");
    // project researcher wins, source tag is project.
    assert!(
        desc.contains("researcher (source: project)"),
        "expected project source tag for researcher: {}",
        desc
    );
    // No source: builtin for researcher (overridden).
    assert!(
        !desc.contains("researcher (source: builtin)"),
        "builtin tag should be overridden: {}",
        desc
    );
}

// ---- definition_with_cache model enum (B6+ B) ----

#[tokio::test]
async fn definition_with_cache_model_enum_from_briefs() {
    // B6+ B (task 07-06-b6plus-b-dispatch-model-arg): the `model`
    // property's enum is built from the `models: &[ModelBrief]`
    // argument — display_name values, so the LLM can discover
    // available models without guessing UUIDs.
    let cache = SubagentCache::arc();
    let project_path = std::env::temp_dir().to_string_lossy().to_string();
    let briefs = vec![
        ModelBrief {
            id: "uuid-1".into(),
            display_name: "GPT-4o".into(),
        },
        ModelBrief {
            id: "uuid-2".into(),
            display_name: "Claude Sonnet 4.5".into(),
        },
    ];
    let def = definition_with_cache(&cache, &project_path, None, &briefs).await;
    let model_enum: Vec<String> = def
        .input_schema
        .pointer("/properties/model/enum")
        .and_then(|v| v.as_array())
        .expect("model enum present")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect();
    assert_eq!(
        model_enum,
        vec!["GPT-4o".to_string(), "Claude Sonnet 4.5".to_string()],
        "model enum must mirror the briefs' display_names in order"
    );
    // `model` is NOT in `required` (optional override).
    let required: Vec<String> = def
        .input_schema
        .get("required")
        .and_then(|v| v.as_array())
        .expect("required present")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect();
    assert!(
        !required.contains(&"model".to_string()),
        "model must not be required"
    );
}

#[tokio::test]
async fn definition_with_cache_empty_briefs_yields_empty_model_enum() {
    // Empty briefs (no models / list_models failed) → empty enum,
    // not a missing property (defensive — keeps the schema shape).
    let cache = SubagentCache::arc();
    let project_path = std::env::temp_dir().to_string_lossy().to_string();
    let def = definition_with_cache(&cache, &project_path, None, &[]).await;
    let model_enum = def
        .input_schema
        .pointer("/properties/model/enum")
        .and_then(|v| v.as_array())
        .expect("model enum present even when empty");
    assert!(model_enum.is_empty(), "empty briefs → empty model enum");
}

// ---- builtin_subagents ----

#[test]
fn builtin_subagents_has_two_entries() {
    let defs = builtin_subagents();
    assert_eq!(defs.len(), 2);
}

#[test]
fn builtin_subagents_researcher_tool_allowlist() {
    let r = lookup_subagent("researcher").expect("researcher exists");
    assert_eq!(
        r.tools,
        vec![
            "read_file".to_string(),
            "grep".to_string(),
            "glob".to_string(),
            "list_dir".to_string(),
            "web_fetch".to_string(),
        ]
    );
}

#[test]
fn builtin_subagents_general_purpose_empty_allowlist() {
    let g = lookup_subagent("general-purpose").expect("general-purpose exists");
    assert!(g.tools.is_empty(), "general-purpose inherits full set");
}

#[test]
fn lookup_subagent_unknown_returns_none() {
    assert!(lookup_subagent("nope").is_none());
}

// ---- filter_tools_for_subagent ----

fn tool(name: &str) -> ToolDef {
    ToolDef {
        name: name.to_string(),
        description: None,
        input_schema: serde_json::json!({"type": "object"}),
    }
}

fn tool_names(tools: &[ToolDef]) -> Vec<String> {
    tools.iter().map(|t| t.name.clone()).collect()
}

#[test]
fn filter_researcher_keeps_only_read_tools_and_strips_disabled() {
    let def = lookup_subagent("researcher").unwrap();
    let all = vec![
        tool("read_file"),
        tool("grep"),
        tool("glob"),
        tool("list_dir"),
        tool("write_file"),
        tool("edit_file"),
        tool("shell"),
        tool("web_fetch"),
        tool("use_skill"),
        tool("update_checklist"),
        tool("dispatch_subagent"),
        tool("run_background_shell"),
        tool("shell_status"),
        tool("shell_kill"),
    ];
    let filtered = filter_tools_for_subagent(all, def);
    let names = tool_names(&filtered);
    assert!(names.contains(&"read_file".to_string()));
    assert!(names.contains(&"grep".to_string()));
    assert!(names.contains(&"glob".to_string()));
    assert!(names.contains(&"list_dir".to_string()));
    // web_fetch is now in the researcher allowlist (06-25-subagent-web-access).
    assert!(names.contains(&"web_fetch".to_string()));
    // Read-only — no writes.
    assert!(!names.contains(&"write_file".to_string()));
    assert!(!names.contains(&"edit_file".to_string()));
    assert!(!names.contains(&"shell".to_string()));
    // Structural-disabled ALWAYS stripped.
    assert!(!names.contains(&"update_checklist".to_string()));
    assert!(!names.contains(&"dispatch_subagent".to_string()));
    assert!(!names.contains(&"run_background_shell".to_string()));
    assert!(!names.contains(&"shell_status".to_string()));
    assert!(!names.contains(&"shell_kill".to_string()));
}

#[test]
fn filter_general_purpose_keeps_full_set_minus_disabled() {
    let def = lookup_subagent("general-purpose").unwrap();
    let all = vec![
        tool("read_file"),
        tool("write_file"),
        tool("edit_file"),
        tool("shell"),
        tool("grep"),
        tool("glob"),
        tool("list_dir"),
        tool("web_fetch"),
        tool("use_skill"),
        tool("update_checklist"),
        tool("dispatch_subagent"),
        tool("run_background_shell"),
        tool("shell_status"),
        tool("shell_kill"),
    ];
    let filtered = filter_tools_for_subagent(all, def);
    let names = tool_names(&filtered);
    // general-purpose keeps the full write/shell/web_fetch set.
    assert!(names.contains(&"write_file".to_string()));
    assert!(names.contains(&"edit_file".to_string()));
    assert!(names.contains(&"shell".to_string()));
    assert!(names.contains(&"web_fetch".to_string()));
    // Structural-disabled still stripped.
    assert!(!names.contains(&"update_checklist".to_string()));
    assert!(!names.contains(&"dispatch_subagent".to_string()));
    assert!(!names.contains(&"run_background_shell".to_string()));
    assert!(!names.contains(&"shell_status".to_string()));
    assert!(!names.contains(&"shell_kill".to_string()));
}

#[test]
fn filter_strips_structurally_disabled_even_if_allowlist_lists_them() {
    // Defensive: build a synthetic SubagentDef that explicitly
    // allows dispatch_subagent + the L1a trio. The filter MUST
    // still strip them (structural-disabled wins over the
    // allowlist).
    let synthetic = SubagentDef {
        name: "synthetic".to_string(),
        description: String::new(),
        system_prompt: String::new(),
        tools: vec![
            "read_file".to_string(),
            "dispatch_subagent".to_string(),
            "update_checklist".to_string(),
            "run_background_shell".to_string(),
            "shell_status".to_string(),
            "shell_kill".to_string(),
            "ask_user_question".to_string(),
        ],
        isolation: None,
        model: None,
    };
    let all = vec![
        tool("read_file"),
        tool("dispatch_subagent"),
        tool("update_checklist"),
        tool("run_background_shell"),
        tool("shell_status"),
        tool("shell_kill"),
        tool("ask_user_question"),
    ];
    let filtered = filter_tools_for_subagent(all, &synthetic);
    let names = tool_names(&filtered);
    // ask_user_question (06-30 task) is structurally disabled for
    // workers — workers have no UI sink and would hang the oneshot
    // forever. Assert it explicitly alongside the other disabled
    // tools so AC3 has a named coverage point.
    assert_eq!(names, vec!["read_file".to_string()]);
}
