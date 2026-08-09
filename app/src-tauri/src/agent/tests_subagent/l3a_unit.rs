#![cfg(test)]

use crate::agent::chat_loop::{classify_dispatch_batch, DispatchBatch};
use crate::agent::subagent::filter_tools_readonly;

// ---------------------------------------------------------------------------
// L3a (2026-06-24): concurrent dispatch_subagent batch (read-only fan-out)
// ---------------------------------------------------------------------------

/// `filter_tools_readonly` (L3a unit guard): when applied to the
/// full `builtin_tools()` set, the result contains exactly the 5
/// read-only tools (read_file / grep / glob / list_dir / web_fetch)
/// and nothing else. This is the 2nd layer of the 3-layer read-only
/// guarantee; the unit test pins the function directly so a future
/// tool added to `builtin_tools()` does NOT silently leak into the
/// concurrent worker toolset. (`web_fetch` joined the read-only set
/// on 2026-06-25, task 06-25-subagent-web-access — it is a read-only
/// network op with its own SSRF guard in `tools/web_fetch.rs`.)
#[test]
fn l3a_filter_tools_readonly_keeps_only_five_read_tools() {
    let all = crate::tools::builtin_tools();
    let filtered = filter_tools_readonly(all);
    let names: Vec<String> = filtered.iter().map(|t| t.name.clone()).collect();
    assert_eq!(
        names.len(),
        5,
        "exactly 5 read-only tools, got: {:?}",
        names
    );
    for required in &["read_file", "grep", "glob", "list_dir", "web_fetch"] {
        assert!(
            names.iter().any(|n| n == required),
            "filter must keep {}, got: {:?}",
            required,
            names
        );
    }
    for forbidden in &[
        "write_file",
        "edit_file",
        "shell",
        "dispatch_subagent",
        "update_checklist",
    ] {
        assert!(
            !names.iter().any(|n| n == forbidden),
            "filter must NOT keep {}, got: {:?}",
            forbidden,
            names
        );
    }
}

/// `classify_dispatch_batch` (L3a unit guard): pure-batch counting
/// + limit threshold. Pins the three branches (Serial / Concurrent /
/// OverLimit) without spinning up the agent loop.
#[test]
fn l3a_classify_dispatch_batch_branches_correctly() {
    let dispatch_input = serde_json::json!({ "subagent": "researcher", "task": "x" });
    let read_input = serde_json::json!({ "path": "a.rs" });
    let tc =
        |id: &str, name: &str, input: serde_json::Value| (id.to_string(), name.to_string(), input);

    // Single dispatch → Serial.
    let single = vec![tc("t1", "dispatch_subagent", dispatch_input.clone())];
    assert!(matches!(
        classify_dispatch_batch(&single, 3),
        DispatchBatch::Serial
    ));

    // 2 dispatches, pure → Concurrent.
    let two = vec![
        tc("t1", "dispatch_subagent", dispatch_input.clone()),
        tc("t2", "dispatch_subagent", dispatch_input.clone()),
    ];
    assert!(matches!(
        classify_dispatch_batch(&two, 3),
        DispatchBatch::Concurrent { count: 2 }
    ));

    // 3 dispatches, pure, at limit → Concurrent.
    let three = vec![
        tc("t1", "dispatch_subagent", dispatch_input.clone()),
        tc("t2", "dispatch_subagent", dispatch_input.clone()),
        tc("t3", "dispatch_subagent", dispatch_input.clone()),
    ];
    assert!(matches!(
        classify_dispatch_batch(&three, 3),
        DispatchBatch::Concurrent { count: 3 }
    ));

    // 4 dispatches, pure, over limit → OverLimit.
    let four = vec![
        tc("t1", "dispatch_subagent", dispatch_input.clone()),
        tc("t2", "dispatch_subagent", dispatch_input.clone()),
        tc("t3", "dispatch_subagent", dispatch_input.clone()),
        tc("t4", "dispatch_subagent", dispatch_input.clone()),
    ];
    assert!(matches!(
        classify_dispatch_batch(&four, 3),
        DispatchBatch::OverLimit {
            count: 4,
            max_concurrent: 3
        }
    ));

    // Mixed batch (1 dispatch + 1 read_file) → Serial (fall through).
    let mixed = vec![
        tc("t1", "dispatch_subagent", dispatch_input.clone()),
        tc("t2", "read_file", read_input.clone()),
    ];
    assert!(matches!(
        classify_dispatch_batch(&mixed, 3),
        DispatchBatch::Serial
    ));

    // Pure read batch (no dispatch) → Serial (handled by L2 path above;
    // classify_dispatch_batch is only consulted in the serial-else arm).
    let read_only = vec![
        tc("t1", "read_file", read_input.clone()),
        tc("t2", "grep", read_input.clone()),
    ];
    assert!(matches!(
        classify_dispatch_batch(&read_only, 3),
        DispatchBatch::Serial
    ));
}
