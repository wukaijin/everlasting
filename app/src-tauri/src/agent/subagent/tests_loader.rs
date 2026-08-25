//! Subagent loader 单元测试(拆分自 loader.rs, 08-07-large-file-splitting)。

#![cfg(test)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::cache::*;
use super::frontmatter::*;
use super::loader::*;
use super::*;

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
                "web_search".to_string(),
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
        assert!(
            res.is_err(),
            "plugin agents are read-only; locate should refuse"
        );
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
        write_agent(
            &plugin_dir,
            "researcher",
            "---\nname: researcher\n---\nPLUGIN_RESEARCHER",
        );

        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        let with_wf = cache.list_with_workflow(&project_path, Some("dev")).await;
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
        write_agent(
            &project_agents,
            "researcher",
            "---\nname: researcher\n---\nPROJECT_BODY",
        );
        let plugin_dir = plugin_agents_dir("dev", &proj_tmp.path().to_string_lossy());
        std::fs::create_dir_all(&plugin_dir).unwrap();
        write_agent(
            &plugin_dir,
            "researcher",
            "---\nname: researcher\n---\nPLUGIN_BODY",
        );

        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        let merged = cache.list_with_workflow(&project_path, Some("dev")).await;
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
        write_agent(
            &plugin_dir,
            "researcher",
            "---\nname: researcher\n---\nPLUGIN_BODY",
        );

        let cache = SubagentCache::arc();
        let project_path = proj_tmp.path().to_string_lossy().to_string();

        let merged = cache.list_with_workflow(&project_path, Some("")).await;
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

    // --- 07-09-workflow-builtin-plugin: BuiltinPlugin agent layer ---

    #[tokio::test]
    async fn builtin_plugin_agent_loaded_for_dev_in_empty_project() {
        // 空项目 + workflow=dev → researcher 命中内置 dev 角色
        // (非 builtin researcher,语义不同)。
        let cache = SubagentCache::arc();
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let pp = proj_tmp.path().to_string_lossy().to_string();
        let l = cache
            .lookup_with_workflow(&pp, Some("dev"), "researcher")
            .await;
        let l = l.expect("builtin dev researcher should load");
        assert_eq!(l.def.name, "researcher");
        assert_eq!(l.source, SubagentSource::BuiltinPlugin);
        assert!(
            !l.def.system_prompt.is_empty(),
            "builtin dev researcher has system prompt"
        );
    }

    #[tokio::test]
    async fn project_plugin_agent_overrides_builtin() {
        // 项目 .everlasting/workflow/dev/agents/researcher.md → 项目赢
        // (Plugin > BuiltinPlugin)。
        let cache = SubagentCache::arc();
        let proj_tmp = tempfile::TempDir::new().unwrap();
        let proj = proj_tmp.path();
        let agents_dir = proj.join(".everlasting/workflow/dev/agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        write_agent(
            &agents_dir,
            "researcher",
            "---\nname: researcher\ndescription: mine\ntools: [read_file]\n---\nCUSTOM_RESEARCHER",
        );
        let pp = proj.to_string_lossy().to_string();
        let l = cache
            .lookup_with_workflow(&pp, Some("dev"), "researcher")
            .await
            .expect("project plugin researcher must win");
        assert_eq!(l.source, SubagentSource::Plugin);
        assert_eq!(l.def.system_prompt, "CUSTOM_RESEARCHER");
    }

    /// F4 (2026-08-25, AC6): **runtime** assertion that THIS repo's
    /// project-layer workflow agents actually load `web_search`.
    /// The builtin copies (`resources/builtin-workflow/**`) are only a
    /// compile-time fallback — the project layer shadows them at
    /// runtime, so a builtin-only flip would leave this repo's live
    /// researcher without the tool while grep-level checks stay green
    /// (review P1-1). Resolves through the real loader (project >
    /// builtin-plugin) against the repo root.
    #[tokio::test]
    async fn repo_workflow_agents_load_web_search() {
        let repo_root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let cache = SubagentCache::arc();
        for (workflow, agent) in [("dev", "researcher"), ("review", "reviewer")] {
            let hit = cache
                .lookup_with_workflow(repo_root, Some(workflow), agent)
                .await
                .unwrap_or_else(|| panic!("{workflow}/{agent} must resolve"));
            // 项目层副本必须赢过 builtin fallback——否则断言的是错的层。
            assert_eq!(
                hit.source,
                SubagentSource::Plugin,
                "{workflow}/{agent}: expected the repo's project-layer copy to shadow the builtin"
            );
            assert!(
                hit.def.tools.iter().any(|t| t == "web_search"),
                "{workflow}/{agent} tools must include web_search, got: {:?}",
                hit.def.tools
            );
        }
    }
}
