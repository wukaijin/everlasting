#![cfg(test)]

use crate::agent::permissions::{filter_tools_for_mode, mode_system_prefix};
use crate::db::Mode;

#[test]
fn mode_as_str_round_trip() {
    for m in [Mode::Edit, Mode::Plan, Mode::Yolo, Mode::Background] {
        assert_eq!(Mode::from_str_opt(m.as_str()), m);
    }
}

#[test]
fn mode_from_str_unknown_defaults_to_chat() {
    assert_eq!(Mode::from_str_opt(""), Mode::Edit);
    assert_eq!(Mode::from_str_opt("nonsense"), Mode::Edit);
    assert_eq!(Mode::from_str_opt("PLAN"), Mode::Edit); // case-sensitive
}

#[test]
fn filter_tools_for_mode_drops_writes_in_plan_review() {
    let tools = vec![
        crate::llm::ToolDef::new_for_test("read_file"),
        crate::llm::ToolDef::new_for_test("write_file"),
        crate::llm::ToolDef::new_for_test("shell"),
        crate::llm::ToolDef::new_for_test("grep"),
    ];
    let filtered = filter_tools_for_mode(tools.clone(), Mode::Plan, false);
    let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"grep"));
    assert!(!names.contains(&"write_file"));
    assert!(!names.contains(&"shell"));

    let filtered = filter_tools_for_mode(tools.clone(), Mode::Plan, false);
    let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
    assert!(!names.contains(&"write_file"));
    assert!(!names.contains(&"shell"));
}

/// L3b PR3 (2026-06-27): merge_worker / discard_worker rewrite the
/// parent session branch, so Plan mode (read-only) must filter them
/// out alongside the write tools; Edit / Yolo keep them.
#[test]
fn filter_tools_for_mode_drops_merge_discard_in_plan() {
    let tools = vec![
        crate::llm::ToolDef::new_for_test("read_file"),
        crate::llm::ToolDef::new_for_test("merge_worker"),
        crate::llm::ToolDef::new_for_test("discard_worker"),
    ];
    let filtered = filter_tools_for_mode(tools.clone(), Mode::Plan, false);
    let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"read_file"));
    assert!(!names.contains(&"merge_worker"));
    assert!(!names.contains(&"discard_worker"));
    for m in [Mode::Edit, Mode::Yolo] {
        let filtered = filter_tools_for_mode(tools.clone(), m, false);
        assert_eq!(
            filtered.len(),
            tools.len(),
            "Mode {:?} should keep merge/discard",
            m
        );
    }
}

#[test]
fn filter_tools_for_mode_keeps_full_for_chat_yolo() {
    let tools = vec![
        crate::llm::ToolDef::new_for_test("read_file"),
        crate::llm::ToolDef::new_for_test("write_file"),
        crate::llm::ToolDef::new_for_test("shell"),
    ];
    for m in [Mode::Edit, Mode::Yolo] {
        let filtered = filter_tools_for_mode(tools.clone(), m, false);
        assert_eq!(
            filtered.len(),
            tools.len(),
            "Mode {:?} should keep all tools",
            m
        );
    }
}

#[test]
fn mode_system_prefix_is_non_empty() {
    for m in [Mode::Edit, Mode::Plan, Mode::Yolo, Mode::Background] {
        assert!(!mode_system_prefix(m).is_empty());
    }
}

/// P3c design §4: with the Plan read-only face available
/// (`plan_shell_available = true`), the shell tool family STAYS in
/// the Plan tool list — write tools / worker-rewrite tools are still
/// dropped.
#[test]
fn filter_tools_for_mode_keeps_shell_family_when_plan_face_available() {
    let tools = vec![
        crate::llm::ToolDef::new_for_test("read_file"),
        crate::llm::ToolDef::new_for_test("write_file"),
        crate::llm::ToolDef::new_for_test("shell"),
        crate::llm::ToolDef::new_for_test("run_background_shell"),
        crate::llm::ToolDef::new_for_test("merge_worker"),
    ];
    let filtered = filter_tools_for_mode(tools, Mode::Plan, true);
    let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"shell"), "Plan + face → shell stays");
    assert!(
        names.contains(&"run_background_shell"),
        "Plan + face → run_background_shell stays"
    );
    assert!(names.contains(&"read_file"));
    assert!(!names.contains(&"write_file"));
    assert!(!names.contains(&"merge_worker"));
}

/// P3c design §4 fallback matrix: `plan_shell_available = false`
/// (kill-switch off / project off / capability fail / non-Linux)
/// drops the shell family exactly as before the sandbox existed —
/// never a "Plan + approval-modal write" path.
#[test]
fn filter_tools_for_mode_falls_back_to_tool_filter_without_plan_face() {
    let tools = vec![
        crate::llm::ToolDef::new_for_test("read_file"),
        crate::llm::ToolDef::new_for_test("shell"),
        crate::llm::ToolDef::new_for_test("run_background_shell"),
        crate::llm::ToolDef::new_for_test("write_file"),
    ];
    let filtered = filter_tools_for_mode(tools, Mode::Plan, false);
    let names: Vec<&str> = filtered.iter().map(|t| t.name.as_str()).collect();
    assert!(!names.contains(&"shell"));
    assert!(!names.contains(&"run_background_shell"));
    assert!(!names.contains(&"write_file"));
    assert!(names.contains(&"read_file"));
}

/// The param is inert outside Plan (Edit / Yolo keep the full set
/// regardless of the face flag).
#[test]
fn filter_tools_for_mode_plan_face_param_inert_outside_plan() {
    let tools = vec![
        crate::llm::ToolDef::new_for_test("shell"),
        crate::llm::ToolDef::new_for_test("write_file"),
    ];
    for m in [Mode::Edit, Mode::Yolo] {
        for avail in [true, false] {
            let filtered = filter_tools_for_mode(tools.clone(), m, avail);
            assert_eq!(
                filtered.len(),
                tools.len(),
                "Mode {:?} ignores plan_shell_available={}",
                m,
                avail
            );
        }
    }
}
