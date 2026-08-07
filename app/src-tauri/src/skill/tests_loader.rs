//! Tests for `skill::loader` (relocated from the pre-split inline
//! `#[cfg(test)] mod tests`). Pure relocation — no logic changes.
#![cfg(test)]

use std::path::{Path, PathBuf};

use crate::memory::file::set_user_dir_for_test;
use crate::skill::loader::*;

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
    write_skill(
        tmp.path(),
        "review-pr",
        "---\nname: review-pr\ndescription: d\n---\nb1",
    );
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
    assert!(scan_skill_dir(tmp.path(), SkillSource::User)
        .await
        .is_empty());
}

#[tokio::test]
async fn scan_missing_dir_returns_empty() {
    let res = scan_skill_dir(
        Path::new("/no/such/everlasting/skills/xyz"),
        SkillSource::User,
    )
    .await;
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
    write_skill(
        &user_skills,
        "shared",
        "---\nname: shared\ndescription: from-user\n---\nub",
    );

    let proj_tmp = tempfile::TempDir::new().unwrap();
    let proj_skills = proj_tmp.path().join(PROJECT_NAMESPACE).join(SKILLS_SUBDIR);
    std::fs::create_dir_all(&proj_skills).unwrap();
    write_skill(
        &proj_skills,
        "shared",
        "---\nname: shared\ndescription: from-project\n---\npb",
    );

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
    write_skill(
        &user_skills,
        "commit",
        "---\nname: commit\ndescription: d\n---\nBODY",
    );

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
    let crate::llm::types::ContentBlock::Text {
        text,
        cache_control,
    } = &blocks[0]
    else {
        panic!("expected Text block");
    };
    assert!(text.contains("- review-pr: review 一个 PR"));
    assert!(text.contains("- commit"));
    assert!(
        !text.contains("- commit: "),
        "empty description omits the colon"
    );
    assert!(
        !text.contains("tools:"),
        "empty allowed_tools must not render a suffix"
    );
    assert!(text.contains("use_skill"));
    assert_eq!(
        *cache_control,
        Some(crate::llm::types::CacheControl::Ephemeral)
    );
}

// ---- Stretch 1: `allowed-tools` array parse + L0 render ----

#[test]
fn parse_allowed_tools_basic() {
    assert_eq!(
        parse_allowed_tools("[read_file, grep, git_diff]"),
        vec![
            "read_file".to_string(),
            "grep".to_string(),
            "git_diff".to_string()
        ]
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
    assert!(
        parse_allowed_tools("[ , , ]").is_empty(),
        "whitespace-only items dropped"
    );
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
    assert_eq!(parse_allowed_tools("'[c]'"), vec!["c".to_string()]);
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
    assert_eq!(
        fm.allowed_tools,
        vec!["read_file".to_string(), "grep".to_string()]
    );
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
    let crate::llm::types::ContentBlock::Text { text, .. } = &blocks[0] else {
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
    let crate::llm::types::ContentBlock::Text { text, .. } = &blocks[0] else {
        panic!("expected Text block");
    };
    assert!(
        text.contains("- minimal  (tools: shell)"),
        "expected `<name>  (tools: ...)` shape, got: {text}"
    );
}

// ---- Step 1.1 plugin layer (workflow skills) ----

/// Write a skill under
/// `<project>/.everlasting/workflow/<wf>/skills/<name>/SKILL.md`.
/// Mirrors `write_skill` (arbitrary parent) but pins the parent
/// to the plugin-skills layout.
fn write_plugin_skill(project: &Path, wf: &str, name: &str, body: &str) {
    let dir = plugin_skills_dir(wf, &project.to_string_lossy());
    std::fs::create_dir_all(dir.join(name)).unwrap();
    std::fs::write(dir.join(name).join(SKILL_FILENAME), body).unwrap();
}

#[test]
fn plugin_skills_dir_lands_under_workflow_subdir() {
    // Pure path arithmetic — no I/O, no existence check. The dir
    // doesn't need to exist; the resolver must produce the same
    // shape regardless of disk state (used by both warm-path
    // cache hits AND the silent-fallback-to-global contract).
    let p = plugin_skills_dir("dev", "/tmp/proj");
    assert_eq!(
        p,
        PathBuf::from("/tmp/proj/.everlasting/workflow/dev/skills"),
        "plugin_skills_dir must resolve to `<project>/.everlasting/workflow/<wf>/skills/`"
    );
}

#[tokio::test]
async fn list_infos_plugin_overrides_project_and_user() {
    let user_tmp = tempfile::TempDir::new().unwrap();
    let user_skills = user_tmp.path().join(SKILLS_SUBDIR);
    std::fs::create_dir_all(&user_skills).unwrap();
    write_skill(
        &user_skills,
        "shared",
        "---\nname: shared\ndescription: from-user\n---\nub",
    );
    write_skill(
        &user_skills,
        "user-only",
        "---\nname: user-only\ndescription: u-only\n---\nub",
    );

    let proj_tmp = tempfile::TempDir::new().unwrap();
    let proj_skills = proj_tmp.path().join(PROJECT_NAMESPACE).join(SKILLS_SUBDIR);
    std::fs::create_dir_all(&proj_skills).unwrap();
    write_skill(
        &proj_skills,
        "shared",
        "---\nname: shared\ndescription: from-project\n---\npb",
    );
    write_plugin_skill(
        proj_tmp.path(),
        "dev",
        "shared",
        "---\nname: shared\ndescription: from-plugin\n---\nplb",
    );
    write_plugin_skill(
        proj_tmp.path(),
        "dev",
        "wf-only",
        "---\nname: wf-only\ndescription: p-only\n---\nplb",
    );

    let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
    let cache = SkillCache::arc();
    let project_path = proj_tmp.path().to_string_lossy().to_string();
    let infos = list_skill_infos_with_workflow(&cache, Some(&project_path), Some("dev")).await;
    set_user_dir_for_test(prev);

    let by_name: std::collections::HashMap<&str, &SkillInfo> =
        infos.iter().map(|i| (i.name.as_str(), i)).collect();
    assert_eq!(by_name["shared"].description, "from-plugin");
    assert_eq!(by_name["shared"].source, "plugin");
    assert!(by_name.contains_key("user-only"));
    assert_eq!(by_name["user-only"].source, "user");
    assert!(by_name.contains_key("wf-only"));
    assert_eq!(by_name["wf-only"].source, "plugin");
}
// ---- plugin layer tests Step 1.1 ----

#[tokio::test]
async fn list_infos_with_workflow_none_falls_back_to_non_workflow_path() {
    // Defensive: passing None workflow_name MUST produce the
    // byte-identical result to list_skill_infos (the
    // non-workflow entry point). This is the
    // non-workflow session = no plugin layer guarantee.
    let user_tmp = tempfile::TempDir::new().unwrap();
    let proj_tmp = tempfile::TempDir::new().unwrap();
    write_plugin_skill(
        proj_tmp.path(),
        "dev",
        "wf-only",
        "---\nname: wf-only\ndescription: hidden\n---\nx",
    );
    let user_skills = user_tmp.path().join(SKILLS_SUBDIR);
    std::fs::create_dir_all(&user_skills).unwrap();
    write_skill(
        &user_skills,
        "global",
        "---\nname: global\ndescription: visible\n---\ng",
    );

    let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
    let cache = SkillCache::arc();
    let project_path = proj_tmp.path().to_string_lossy().to_string();

    let with_wf = list_skill_infos_with_workflow(&cache, Some(&project_path), Some("dev")).await;
    let without_wf = list_skill_infos_with_workflow(&cache, Some(&project_path), None).await;
    let baseline = list_skill_infos(&cache, Some(&project_path)).await;
    set_user_dir_for_test(prev);

    assert!(
        with_wf.iter().any(|i| i.name == "wf-only"),
        "plugin skill must surface when workflow_name=Some"
    );
    assert!(
        !without_wf.iter().any(|i| i.name == "wf-only"),
        "plugin skill must NOT surface when workflow_name=None"
    );
    // The non-workflow entry point and the workflow=None path
    // must produce IDENTICAL listings. PartialEq isn't derived
    // on SkillInfo (it's a Serialize wire DTO), so compare by
    // hand against the keys + sources that matter.
    let names_no_wf: Vec<&str> = without_wf.iter().map(|i| i.name.as_str()).collect();
    let names_baseline: Vec<&str> = baseline.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        names_no_wf, names_baseline,
        "workflow_name=None listing names must match the non-workflow entry point"
    );
    for (a, b) in without_wf.iter().zip(baseline.iter()) {
        assert_eq!(a.source, b.source, "source must match");
        assert_eq!(a.description, b.description, "description must match");
    }
}

#[tokio::test]
async fn list_infos_with_workflow_empty_string_treated_as_none() {
    // Defensive: an empty workflow_name is a wiring bug, NOT a
    // valid plugin identifier. Treat it as None.
    let user_tmp = tempfile::TempDir::new().unwrap();
    let proj_tmp = tempfile::TempDir::new().unwrap();
    write_plugin_skill(
        proj_tmp.path(),
        "dev",
        "wf-only",
        "---\nname: wf-only\ndescription: hidden\n---\nx",
    );
    let user_skills = user_tmp.path().join(SKILLS_SUBDIR);
    std::fs::create_dir_all(&user_skills).unwrap();
    write_skill(
        &user_skills,
        "global",
        "---\nname: global\ndescription: g\n---\ng",
    );

    let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
    let cache = SkillCache::arc();
    let project_path = proj_tmp.path().to_string_lossy().to_string();
    let infos = list_skill_infos_with_workflow(&cache, Some(&project_path), Some("")).await;
    set_user_dir_for_test(prev);

    assert!(
        !infos.iter().any(|i| i.name == "wf-only"),
        "empty workflow_name must NOT enable the plugin layer"
    );
}

#[tokio::test]
async fn list_infos_plugin_missing_dir_falls_back_silently() {
    // Design section 6 rollback: plugin dir absent → no warn,
    // no error, just transparent fallthrough to project / user /
    // builtin-plugin (07-09: 内置 plugin 层也是 fallback 的一部分,
    // 不需要 project 有 workflow 目录就出现)。
    let user_tmp = tempfile::TempDir::new().unwrap();
    let proj_tmp = tempfile::TempDir::new().unwrap();
    // Deliberately do NOT create .everlasting/workflow/dev/skills/.
    let user_skills = user_tmp.path().join(SKILLS_SUBDIR);
    std::fs::create_dir_all(&user_skills).unwrap();
    write_skill(
        &user_skills,
        "global",
        "---\nname: global\ndescription: g\n---\ng",
    );
    let proj_skills = proj_tmp.path().join(PROJECT_NAMESPACE).join(SKILLS_SUBDIR);
    std::fs::create_dir_all(&proj_skills).unwrap();
    write_skill(
        &proj_skills,
        "project-only",
        "---\nname: project-only\ndescription: p\n---\np",
    );

    let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
    let cache = SkillCache::arc();
    let project_path = proj_tmp.path().to_string_lossy().to_string();
    let infos = list_skill_infos_with_workflow(&cache, Some(&project_path), Some("dev")).await;
    set_user_dir_for_test(prev);

    let names: Vec<&str> = infos.iter().map(|i| i.name.as_str()).collect();
    assert!(names.contains(&"global"));
    assert!(names.contains(&"project-only"));
    // 07-09-workflow-builtin-plugin: 内置 dev plugin 提供 5 个 wf-* skills
    // (即使项目无 workflow 目录)。校验它们都在(没出现 phantom source path)。
    for slug in [
        "wf-overview",
        "wf-brainstorm",
        "wf-before-dev",
        "wf-check",
        "wf-update-spec",
    ] {
        assert!(
            names.contains(&slug),
            "builtin {slug} should appear (got {names:?})"
        );
    }
    // 来源校验:wf-* 来源是 builtin-plugin,user/project 来源不变。
    for info in &infos {
        if info.name.starts_with("wf-") {
            assert_eq!(
                info.source, "builtin-plugin",
                "wf-* source must be builtin-plugin (got {})",
                info.source
            );
        }
    }
}

#[tokio::test]
async fn find_skill_with_workflow_resolves_plugin_layer_first() {
    // L1 counterpart of the listing test: when wf-* skill is
    // only present in the plugin layer, find_skill_with_workflow
    // must return the plugin body; without the workflow_name,
    // it must miss.
    let user_tmp = tempfile::TempDir::new().unwrap();
    let proj_tmp = tempfile::TempDir::new().unwrap();
    write_plugin_skill(
        proj_tmp.path(),
        "dev",
        "wf-check",
        "---\nname: wf-check\ndescription: p\n---\nPLUGIN_BODY",
    );

    let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
    let cache = SkillCache::arc();
    let project_path = proj_tmp.path().to_string_lossy().to_string();

    let with_wf =
        find_skill_with_workflow(&cache, "wf-check", Some(&project_path), Some("dev")).await;
    let without_wf = find_skill_with_workflow(&cache, "wf-check", Some(&project_path), None).await;
    let baseline = find_skill(&cache, "wf-check", Some(&project_path)).await;
    set_user_dir_for_test(prev);

    let with_wf = with_wf.expect("plugin skill must resolve under workflow_name=Some");
    assert_eq!(with_wf.body, "PLUGIN_BODY");
    assert_eq!(with_wf.source, SkillSource::Plugin);
    assert!(
        without_wf.is_none(),
        "non-workflow call must NOT see the plugin layer"
    );
    assert!(
        baseline.is_none(),
        "non-workflow baseline must NOT see the plugin layer"
    );
}

#[tokio::test]
async fn find_skill_with_workflow_plugin_overrides_project_layer() {
    // Plugin layer takes priority even when project has the
    // same skill name.
    let user_tmp = tempfile::TempDir::new().unwrap();
    let proj_tmp = tempfile::TempDir::new().unwrap();
    let proj_skills = proj_tmp.path().join(PROJECT_NAMESPACE).join(SKILLS_SUBDIR);
    std::fs::create_dir_all(&proj_skills).unwrap();
    write_skill(
        &proj_skills,
        "shared",
        "---\nname: shared\ndescription: from-project\n---\nPROJECT_BODY",
    );
    write_plugin_skill(
        proj_tmp.path(),
        "dev",
        "shared",
        "---\nname: shared\ndescription: from-plugin\n---\nPLUGIN_BODY",
    );

    let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
    let cache = SkillCache::arc();
    let project_path = proj_tmp.path().to_string_lossy().to_string();
    let resolved =
        find_skill_with_workflow(&cache, "shared", Some(&project_path), Some("dev")).await;
    set_user_dir_for_test(prev);

    let resolved = resolved.expect("shared must resolve under plugin override");
    assert_eq!(resolved.body, "PLUGIN_BODY");
    assert_eq!(resolved.source, SkillSource::Plugin);
}

#[tokio::test]
async fn find_skill_with_workflow_project_layer_when_no_plugin_match() {
    // Plugin layer consulted first; on a name miss it falls
    // through to project, then user. This is the
    // project-only skill is still visible to a workflow session
    // guarantee — workflow sessions don't lose access to
    // non-plugin skills.
    let user_tmp = tempfile::TempDir::new().unwrap();
    let proj_tmp = tempfile::TempDir::new().unwrap();
    let proj_skills = proj_tmp.path().join(PROJECT_NAMESPACE).join(SKILLS_SUBDIR);
    std::fs::create_dir_all(&proj_skills).unwrap();
    write_skill(
        &proj_skills,
        "review-pr",
        "---\nname: review-pr\ndescription: r\n---\nPROJECT_BODY",
    );
    // Plugin dir exists but contains an unrelated skill.
    write_plugin_skill(
        proj_tmp.path(),
        "dev",
        "wf-check",
        "---\nname: wf-check\ndescription: c\n---\nC",
    );

    let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
    let cache = SkillCache::arc();
    let project_path = proj_tmp.path().to_string_lossy().to_string();
    let resolved =
        find_skill_with_workflow(&cache, "review-pr", Some(&project_path), Some("dev")).await;
    set_user_dir_for_test(prev);

    let resolved = resolved.expect("project-only skill must still resolve");
    assert_eq!(resolved.body, "PROJECT_BODY");
    assert_eq!(resolved.source, SkillSource::Project);
}

#[tokio::test]
async fn find_skill_with_workflow_no_project_path_falls_back_to_user() {
    // project_path=None skips the plugin + project layers (they
    // both need a project root) and goes straight to user.
    let user_tmp = tempfile::TempDir::new().unwrap();
    let user_skills = user_tmp.path().join(SKILLS_SUBDIR);
    std::fs::create_dir_all(&user_skills).unwrap();
    write_skill(
        &user_skills,
        "commit",
        "---\nname: commit\ndescription: c\n---\nUSER_BODY",
    );

    let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
    let cache = SkillCache::arc();
    let resolved = find_skill_with_workflow(&cache, "commit", None, Some("dev")).await;
    set_user_dir_for_test(prev);

    let resolved = resolved.expect("user skill must resolve when no project path");
    assert_eq!(resolved.body, "USER_BODY");
    assert_eq!(resolved.source, SkillSource::User);
}

// --- 07-09-workflow-builtin-plugin: BuiltinPlugin skill layer -----

#[tokio::test]
async fn builtin_plugin_skill_loaded_for_dev_in_empty_project() {
    // 空项目目录 + workflow=dev → wf-brainstorm 命中内置层。
    let user_tmp = tempfile::TempDir::new().unwrap();
    let proj_tmp = tempfile::TempDir::new().unwrap();
    let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
    let cache = SkillCache::arc();
    let pp = proj_tmp.path().to_string_lossy().to_string();
    let r = find_skill_with_workflow(&cache, "wf-brainstorm", Some(&pp), Some("dev")).await;
    set_user_dir_for_test(prev);
    let r = r.expect("builtin wf-brainstorm should load");
    assert_eq!(r.source, SkillSource::BuiltinPlugin);
    assert!(!r.body.is_empty());
    assert!(r.path.to_string_lossy().contains("<builtin>"));
}

#[tokio::test]
async fn project_plugin_overrides_builtin_skill() {
    // 项目 .everlasting/workflow/dev/skills/wf-brainstorm/SKILL.md → 项目赢(Plugin > BuiltinPlugin)。
    let user_tmp = tempfile::TempDir::new().unwrap();
    let proj_tmp = tempfile::TempDir::new().unwrap();
    let dir = proj_tmp
        .path()
        .join(".everlasting/workflow/dev/skills/wf-brainstorm");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: wf-brainstorm\ndescription: mine\n---\nCUSTOM",
    )
    .unwrap();
    let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
    let cache = SkillCache::arc();
    let pp = proj_tmp.path().to_string_lossy().to_string();
    let r = find_skill_with_workflow(&cache, "wf-brainstorm", Some(&pp), Some("dev")).await;
    set_user_dir_for_test(prev);
    let r = r.expect("project plugin wf-brainstorm must win");
    assert_eq!(
        r.source,
        SkillSource::Plugin,
        "project plugin wins over builtin"
    );
    assert_eq!(r.body, "CUSTOM");
}

#[tokio::test]
async fn builtin_plugin_beats_project_layer_skill() {
    // 项目普通 .everlasting/skills/wf-brainstorm → 内置赢(BuiltinPlugin > Project)。
    let user_tmp = tempfile::TempDir::new().unwrap();
    let proj_tmp = tempfile::TempDir::new().unwrap();
    let dir = proj_tmp
        .path()
        .join(PROJECT_NAMESPACE)
        .join("skills/wf-brainstorm");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: wf-brainstorm\ndescription: mine\n---\nSHOULD_NOT_WIN",
    )
    .unwrap();
    let prev = set_user_dir_for_test(Some(user_tmp.path().to_path_buf()));
    let cache = SkillCache::arc();
    let pp = proj_tmp.path().to_string_lossy().to_string();
    let r = find_skill_with_workflow(&cache, "wf-brainstorm", Some(&pp), Some("dev")).await;
    set_user_dir_for_test(prev);
    let r = r.expect("builtin wf-brainstorm should win over project-layer same name");
    assert_eq!(
        r.source,
        SkillSource::BuiltinPlugin,
        "builtin beats project-layer skill"
    );
    assert_ne!(r.body, "SHOULD_NOT_WIN");
}
