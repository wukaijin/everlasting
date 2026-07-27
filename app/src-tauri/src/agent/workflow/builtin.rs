//! W1-builtin (2026-07-09): app 内置 dev workflow 的编译期常量源。
//!
//! `include_str!` 在编译期把 `resources/builtin-workflow/dev/` 下的内容
//! 文件读成 `&'static str`,嵌进二进制。运行时 loader 查不到项目层
//! (`.everlasting/workflow/<name>/`)时 fallback 到这里。
//!
//! **为什么不用 Tauri bundle.resources**:本项目零 resource bundle 先例
//! (`tauri.conf.json` 无 `resources` 键),内置内容惯例全是硬编码 Rust 常量
//! (`builtin_subagents()` / `default_workflow()`)。`include_str!` 是该惯例
//! 的自然延伸,dev 构建即用,无运行时 resource_dir 解析依赖。详见
//! `.trellis/tasks/07-09-workflow-builtin-plugin/design.md §1`。

/// 内置 dev 的 workflow.json 文本(与 `default_workflow()` 常量逐字等价,
/// 但走 serde 解析路径,保证"内置层结构 == 项目层结构")。
pub const BUILTIN_DEV_WORKFLOW_JSON: &str =
    include_str!("../../../resources/builtin-workflow/dev/workflow.json");

/// (slug, SKILL.md body) —— 内置 dev 的 5 个 wf-* skill。
/// slug 必须等于 SKILL.md 所在子目录名(skill loader 用目录名做 fallback name)。
pub const BUILTIN_DEV_SKILLS: &[(&str, &str)] = &[
    (
        "wf-overview",
        include_str!("../../../resources/builtin-workflow/dev/skills/wf-overview/SKILL.md"),
    ),
    (
        "wf-brainstorm",
        include_str!("../../../resources/builtin-workflow/dev/skills/wf-brainstorm/SKILL.md"),
    ),
    (
        "wf-before-dev",
        include_str!("../../../resources/builtin-workflow/dev/skills/wf-before-dev/SKILL.md"),
    ),
    (
        "wf-check",
        include_str!("../../../resources/builtin-workflow/dev/skills/wf-check/SKILL.md"),
    ),
    (
        "wf-update-spec",
        include_str!("../../../resources/builtin-workflow/dev/skills/wf-update-spec/SKILL.md"),
    ),
];

/// (role_name, agent.md body) —— 内置 dev 的 3 个角色 agent。
/// role_name 必须等于 frontmatter 的 `name:` 字段(agent loader 要求显式 name)。
pub const BUILTIN_DEV_AGENTS: &[(&str, &str)] = &[
    (
        "researcher",
        include_str!("../../../resources/builtin-workflow/dev/agents/researcher.md"),
    ),
    (
        "implementer",
        include_str!("../../../resources/builtin-workflow/dev/agents/implementer.md"),
    ),
    (
        "checker",
        include_str!("../../../resources/builtin-workflow/dev/agents/checker.md"),
    ),
];

/// 内置 review 的 workflow.json 文本(07-26-workflow-review-plugin C3)。
/// review 是多模型评审流:4 state 带回环(intake → reviewing ⇄ revising → reported)。
pub const BUILTIN_REVIEW_WORKFLOW_JSON: &str =
    include_str!("../../../resources/builtin-workflow/review/workflow.json");

/// (slug, SKILL.md body) —— 内置 review 的 4 个 wf-* skill。
/// slug 必须等于 SKILL.md 所在子目录名(同 dev 约定)。
pub const BUILTIN_REVIEW_SKILLS: &[(&str, &str)] = &[
    (
        "wf-overview",
        include_str!("../../../resources/builtin-workflow/review/skills/wf-overview/SKILL.md"),
    ),
    (
        "wf-review-prep",
        include_str!("../../../resources/builtin-workflow/review/skills/wf-review-prep/SKILL.md"),
    ),
    (
        "wf-review-method",
        include_str!("../../../resources/builtin-workflow/review/skills/wf-review-method/SKILL.md"),
    ),
    (
        "wf-synthesize",
        include_str!("../../../resources/builtin-workflow/review/skills/wf-synthesize/SKILL.md"),
    ),
];

/// (role_name, agent.md body) —— 内置 review 的 1 个角色 agent(reviewer)。
/// reviewer.md 的 `model:` 留空 —— 由 dispatch_subagent 的 model 参数(per-dispatch
/// override)主导,这是多模型评审的核心机制。role_name 等于 frontmatter `name:` 字段。
pub const BUILTIN_REVIEW_AGENTS: &[(&str, &str)] = &[
    (
        "reviewer",
        include_str!("../../../resources/builtin-workflow/review/agents/reviewer.md"),
    ),
];

/// 内置 plugin 名清单。`list_plugins` 用它做并集发现。
/// 07-26-workflow-review-plugin C3:追加 review。
pub const BUILTIN_PLUGIN_NAMES: &[&str] = &["dev", "review"];

/// 返回内置 plugin `<name>` 的 workflow.json 文本。
/// `None` = 无此内置 plugin(交由 caller 继续降级)。
pub fn builtin_workflow_json(name: &str) -> Option<&'static str> {
    match name {
        "dev" => Some(BUILTIN_DEV_WORKFLOW_JSON),
        "review" => Some(BUILTIN_REVIEW_WORKFLOW_JSON),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::workflow::{validate, WorkflowDef};

    #[test]
    fn builtin_dev_workflow_json_validates() {
        let def: WorkflowDef =
            serde_json::from_str(BUILTIN_DEV_WORKFLOW_JSON).expect("builtin dev JSON parses");
        assert!(
            validate(&def).is_ok(),
            "builtin dev JSON must self-validate"
        );
        assert_eq!(def.name, "dev");
        assert_eq!(def.initial, "planning");
    }

    #[test]
    fn builtin_dev_skills_nonempty_with_frontmatter() {
        assert_eq!(BUILTIN_DEV_SKILLS.len(), 5);
        for (slug, body) in BUILTIN_DEV_SKILLS {
            assert!(!body.is_empty(), "skill {slug} body empty");
            assert!(
                body.starts_with("---\n") && body.contains("name:"),
                "skill {slug} must start with frontmatter fence + name field"
            );
        }
    }

    #[test]
    fn builtin_dev_agents_nonempty_with_frontmatter() {
        assert_eq!(BUILTIN_DEV_AGENTS.len(), 3);
        for (role, body) in BUILTIN_DEV_AGENTS {
            assert!(!body.is_empty(), "agent {role} body empty");
            assert!(
                body.starts_with("---\n") && body.contains("name:"),
                "agent {role} must start with frontmatter fence + name field"
            );
        }
    }

    // --- review plugin (07-26-workflow-review-plugin C3) -------------------

    #[test]
    fn builtin_review_workflow_json_validates() {
        let def: WorkflowDef = serde_json::from_str(BUILTIN_REVIEW_WORKFLOW_JSON)
            .expect("builtin review JSON parses");
        assert!(
            validate(&def).is_ok(),
            "builtin review JSON must self-validate"
        );
        assert_eq!(def.name, "review");
        assert_eq!(def.initial, "intake");
        // 4 distinct states (intake / reviewing / revising / reported).
        assert_eq!(def.states.len(), 4);
        assert_eq!(
            def.states.len(),
            def.states.iter().collect::<std::collections::HashSet<_>>().len(),
            "review states must be distinct (watch the design.md §1 typo: 4 distinct, not 'reviewing' twice)"
        );
    }

    #[test]
    fn builtin_review_skills_nonempty_with_frontmatter() {
        assert_eq!(BUILTIN_REVIEW_SKILLS.len(), 4);
        for (slug, body) in BUILTIN_REVIEW_SKILLS {
            assert!(!body.is_empty(), "skill {slug} body empty");
            // slug MUST equal the skill subdirectory name (loader requirement).
            assert!(
                body.starts_with(&format!("---\nname: {slug}\n"))
                    || body.starts_with("---\n") && body.contains(&format!("name: {slug}")),
                "skill {slug} frontmatter name field must equal its slug (subdir name)"
            );
            assert!(
                body.starts_with("---\n") && body.contains("name:"),
                "skill {slug} must start with frontmatter fence + name field"
            );
        }
    }

    #[test]
    fn builtin_review_agents_nonempty_with_frontmatter() {
        assert_eq!(BUILTIN_REVIEW_AGENTS.len(), 1);
        for (role, body) in BUILTIN_REVIEW_AGENTS {
            assert!(!body.is_empty(), "agent {role} body empty");
            assert!(
                body.starts_with("---\n") && body.contains("name:"),
                "agent {role} must start with frontmatter fence + name field"
            );
            // reviewer.md frontmatter `model:` must be empty/absent
            // (per-dispatch override drives it; see design.md §2).
            // The body declares it only as a `#` comment line, never a real key.
            assert!(
                !body.lines()
                    .take_while(|l| !l.trim().is_empty())
                    .any(|l| l.trim().starts_with("model:") && !l.trim().starts_with("#")),
                "agent {role} must NOT declare a real `model:` key in frontmatter (per-dispatch override drives it)"
            );
        }
    }
}
