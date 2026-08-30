#![cfg(test)]

use crate::tools::shell::*;
use crate::tools::ToolContext;
use std::path::PathBuf;
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

fn test_ctx(tmp: &tempfile::TempDir) -> ToolContext {
    ToolContext {
        worktree_path: tmp.path().canonicalize().unwrap(),
        cwd: tmp.path().canonicalize().unwrap(),
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db: crate::tools::test_default_pool(),
        project_id: "test-proj".to_string(),
        data_dir: tmp.path().to_path_buf(),
        workflow_name: None,
        mode: crate::db::Mode::Edit,
    }
}

fn fresh_token() -> CancellationToken {
    CancellationToken::new()
}

#[test]
fn definition_has_correct_name() {
    assert_eq!(definition().name, "shell");
}

#[test]
fn definition_documents_working_directory() {
    let schema = &definition().input_schema;
    let props = schema.get("properties").unwrap();
    assert!(props.get("working_directory").is_some());
}

/// The optional `description` field (display-only intent line, 2026-08-30)
/// must be present in the schema as a string and MUST NOT be required —
/// old sessions / prompt variants that omit it stay wire-compatible.
#[test]
fn definition_documents_optional_description() {
    let def = definition();
    let schema = &def.input_schema;
    let props = schema.get("properties").unwrap();
    let desc = props
        .get("description")
        .expect("schema exposes the optional description field");
    assert_eq!(
        desc.get("type").and_then(|t| t.as_str()),
        Some("string"),
        "description must be typed as string"
    );
    let field_desc = desc
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("");
    assert!(
        field_desc.contains("10 words"),
        "schema description should carry the brevity guidance, got: {field_desc}"
    );
    // Tool-level description carries the fill-in guidance too.
    let tool_desc = def.description.as_deref().expect("shell has a description");
    assert!(
        tool_desc.contains("description"),
        "tool description should mention the description field, got: {tool_desc}"
    );
    let required = schema.get("required").expect("required list");
    assert_eq!(
        required,
        &serde_json::json!(["command"]),
        "description must stay optional (required = [command])"
    );
}

/// AC1 / R2: `description` is display-only — `execute()` never reads it.
/// A malformed (non-string) value must leave the execution result
/// byte-identical to the same call without the field.
#[tokio::test]
async fn execute_ignores_malformed_description() {
    let tmp = tempdir().unwrap();
    let ctx = test_ctx(&tmp);
    let token = fresh_token();
    let (plain, plain_err, _, _) = execute(
        &serde_json::json!({"command": "echo hi"}),
        &ctx,
        None,
        &token,
    )
    .await;
    let (with_desc, desc_err, _, _) = execute(
        &serde_json::json!({"command": "echo hi", "description": 12345}),
        &ctx,
        None,
        &token,
    )
    .await;
    assert!(!plain_err);
    assert!(!desc_err);
    assert_eq!(
        plain, with_desc,
        "malformed description must not change execution"
    );
}

/// The timeout description must guide the LLM to raise the
/// timeout for long commands (builds / installs / large test
/// suites) instead of silently getting cut off at the 2-minute
/// default. Regression guard: a copy-edit must not drop the
/// guidance (it is the only signal the model gets for when to
/// deviate from the 120s default).
#[test]
fn definition_documents_timeout_guidance() {
    let def = definition();
    let desc = def.description.as_deref().expect("shell has a description");
    // Tool-level description carries the long-command guidance.
    assert!(
        desc.contains("300000-600000"),
        "description should suggest a longer timeout range, got: {desc}"
    );
    assert!(
        desc.contains("build") && desc.contains("install"),
        "description should name long-command examples, got: {desc}"
    );
    // Schema field description mirrors the guidance.
    let props = def.input_schema.get("properties").unwrap();
    let timeout_field = props.get("timeout").unwrap();
    let to_desc = timeout_field.get("description").unwrap().as_str().unwrap();
    assert!(
        to_desc.contains("300000-600000"),
        "schema timeout description should suggest a longer value, got: {to_desc}"
    );
}

#[tokio::test]
async fn execute_echo() {
    let tmp = tempdir().unwrap();
    let (content, is_error, update, _) = execute(
        &serde_json::json!({"command": "echo hello"}),
        &test_ctx(&tmp),
        None,
        &fresh_token(),
    )
    .await;
    assert!(!is_error);
    assert!(content.contains("hello"));
    assert!(content.contains("[exit code: 0]"));
    // Update carries the validated cwd.
    assert!(update.new_cwd.is_some());
}

#[tokio::test]
async fn execute_stderr_command() {
    let tmp = tempdir().unwrap();
    let (content, is_error, _, _) = execute(
        &serde_json::json!({"command": "echo error >&2 && false"}),
        &test_ctx(&tmp),
        None,
        &fresh_token(),
    )
    .await;
    assert!(is_error);
    assert!(content.contains("error"));
}

#[tokio::test]
async fn execute_missing_command_param() {
    let tmp = tempdir().unwrap();
    let (msg, is_error, _, _) = execute(
        &serde_json::json!({}),
        &test_ctx(&tmp),
        None,
        &fresh_token(),
    )
    .await;
    assert!(is_error);
    assert!(msg.contains("Missing required parameter"));
}

#[tokio::test]
async fn execute_respects_working_directory_inside_root() {
    let tmp = tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("sub")).unwrap();
    let ctx = test_ctx(&tmp);

    let (content, is_error, update, _) = execute(
        &serde_json::json!({
            "command": "pwd",
            "working_directory": ctx.worktree_path.join("sub").to_string_lossy(),
        }),
        &ctx,
        None,
        &fresh_token(),
    )
    .await;
    assert!(!is_error, "{}", content);
    let update_cwd = update.new_cwd.expect("update carries new cwd");
    assert_eq!(
        update_cwd,
        ctx.worktree_path.join("sub").canonicalize().unwrap()
    );
}

#[tokio::test]
async fn execute_rejects_working_directory_outside_root() {
    let tmp = tempdir().unwrap();
    let ctx = test_ctx(&tmp);
    let (msg, is_error, update, _) = execute(
        &serde_json::json!({
            "command": "ls",
            "working_directory": "/etc",
        }),
        &ctx,
        None,
        &fresh_token(),
    )
    .await;
    assert!(is_error);
    assert!(
        msg.contains("outside project root") || msg.contains("rejected"),
        "expected rejection, got: {}",
        msg
    );
    // Update must be empty so the agent loop does not persist
    // a bogus cwd.
    assert!(update.new_cwd.is_none());
}

#[tokio::test]
async fn execute_rejects_nonexistent_working_directory() {
    let tmp = tempdir().unwrap();
    let ctx = test_ctx(&tmp);
    let (msg, is_error, _, _) = execute(
        &serde_json::json!({
            "command": "ls",
            "working_directory": ctx
                .worktree_path
                .join("nope")
                .to_string_lossy()
                .into_owned(),
        }),
        &ctx,
        None,
        &fresh_token(),
    )
    .await;
    assert!(is_error);
    assert!(msg.contains("rejected") || msg.contains("cannot be resolved"));
}

/// Defensive: when ctx.cwd is itself outside the project root
/// (which the agent loop should never construct), the boundary
/// check still rejects the operation. This guards against a
/// future regression where some caller passes a stale ctx.
#[tokio::test]
async fn execute_rejects_when_ctx_cwd_outside_root() {
    let tmp = tempdir().unwrap();
    let ctx = ToolContext {
        worktree_path: tmp.path().canonicalize().unwrap(),
        cwd: PathBuf::from("/etc"),
        checklist: crate::tools::update_checklist::new_handle(),
        background_shells: crate::background_shell::default_registry(),
        db: crate::tools::test_default_pool(),
        project_id: "test-proj".to_string(),
        data_dir: tmp.path().to_path_buf(),
        workflow_name: None,
        mode: crate::db::Mode::Edit,
    };
    let (msg, is_error, _, _) = execute(
        &serde_json::json!({"command": "pwd"}),
        &ctx,
        None,
        &fresh_token(),
    )
    .await;
    assert!(is_error);
    assert!(msg.contains("rejected") || msg.contains("outside"));
}

/// AC7.1: small output (< 30 KB) goes inline as before.
#[tokio::test]
async fn small_output_inline() {
    let tmp = tempdir().unwrap();
    let (content, is_error, _, _) = execute(
        &serde_json::json!({"command": "echo hello world"}),
        &test_ctx(&tmp),
        None,
        &fresh_token(),
    )
    .await;
    assert!(!is_error);
    assert!(content.contains("hello world"));
    // No disk-spill path in the message.
    assert!(!content.contains("Output saved to"));
}

/// AC7.2: output > 30 KB spills to disk and the result mentions
/// the path and preview. C6: spill lands in
/// `<data_dir>/outputs/<session_id>/` (AC3: dir keyed by session id)
/// and the preview carries the unified mode-A marker.
#[tokio::test]
async fn large_output_spills_to_disk() {
    let tmp = tempdir().unwrap();
    // Generate ~40 KB of stdout.
    let (content, is_error, _, _) = execute(
        &serde_json::json!({"command": "yes line | head -c 40000"}),
        &test_ctx(&tmp),
        Some("sess-ac7"),
        &fresh_token(),
    )
    .await;
    assert!(!is_error, "{}", &content[..200.min(content.len())]);
    // Tool result should mention the saved file.
    assert!(
        content.contains("Output saved to"),
        "got: {}",
        &content[..300.min(content.len())]
    );
    assert!(content.contains("outputs/sess-ac7/"));
    assert!(content.contains("preview"));
    // Unified mode-A recovery marker (C6 R2).
    assert!(content.contains("recover: read_file with offset/limit"));
    // The actual file should exist on disk.
    // Parse the path out of the message.
    let path_line = content
        .lines()
        .find(|l| l.starts_with("Output saved to "))
        .expect("path line present");
    let path_str = path_line
        .trim_start_matches("Output saved to ")
        .split_whitespace()
        .next()
        .unwrap();
    let path = std::path::Path::new(path_str);
    assert!(
        path.starts_with(tmp.path().join("outputs").join("sess-ac7")),
        "spill must be session-keyed under data_dir, got {}",
        path.display()
    );
    assert!(
        path.exists(),
        "spill file should exist at {}",
        path.display()
    );
    // The file should contain all the original output (not just preview).
    let saved = tokio::fs::read_to_string(path).await.unwrap();
    assert!(saved.len() > crate::tools::tool_output::SPILL_THRESHOLD_BYTES);
}

/// AC7.3: the `<data_dir>/outputs/<session_id>/` directory is created
/// on demand (C6 relocation).
#[tokio::test]
async fn spill_creates_outputs_directory() {
    let tmp = tempdir().unwrap();
    let _ = execute(
        &serde_json::json!({"command": "yes x | head -c 40000"}),
        &test_ctx(&tmp),
        Some("sess-ac73"),
        &fresh_token(),
    )
    .await;
    let dir = tmp.path().join("outputs").join("sess-ac73");
    assert!(dir.exists());
    assert!(dir.is_dir());
}

/// C1: cancelling a long-running shell command kills the child
/// and returns partial output with a cancellation marker.
#[tokio::test]
async fn cancel_kills_child_process() {
    let tmp = tempdir().unwrap();
    let ctx = test_ctx(&tmp);
    let token = CancellationToken::new();
    let token_clone = token.clone();
    // Spawn a command that runs for 60 seconds.
    let handle = tokio::spawn(async move {
        execute(
            &serde_json::json!({"command": "sleep 60"}),
            &ctx,
            None,
            &token_clone,
        )
        .await
    });
    // Give the child a moment to start, then cancel.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    token.cancel();
    let (content, is_error, _, _) = handle.await.unwrap();
    assert!(is_error);
    assert!(
        content.contains("[cancelled, partial output]"),
        "expected cancel marker, got: {}",
        content
    );
}

/// C1: cancelling before the child even starts returns the
/// cancel marker immediately.
#[tokio::test]
async fn cancel_before_spawn() {
    let tmp = tempdir().unwrap();
    let token = CancellationToken::new();
    token.cancel();
    let (content, is_error, _, _) = execute(
        &serde_json::json!({"command": "sleep 60"}),
        &test_ctx(&tmp),
        None,
        &token,
    )
    .await;
    assert!(is_error);
    assert!(
        content.contains("[cancelled, partial output]"),
        "expected cancel marker, got: {}",
        content
    );
}

/// C6 / RULE-E-009 regression (AC2, spill-success arm): >30 KB CJK
/// output used to panic at the raw 1 KB preview slice (pre-C6
/// `head_tail_preview` had no char-boundary handling — the
/// codebase's sole RULE-E-009 violator). The unified path must
/// return normally with the spill message + preview.
#[tokio::test]
async fn large_cjk_output_spill_preview_no_panic() {
    let tmp = tempdir().unwrap();
    // `yes 汉` emits 3-byte chars + newline — fixed byte offsets
    // land mid-character without boundary handling.
    let (content, is_error, _, _) = execute(
        &serde_json::json!({"command": "yes 汉 | head -c 100000"}),
        &test_ctx(&tmp),
        Some("sess-cjk"),
        &fresh_token(),
    )
    .await;
    assert!(!is_error);
    assert!(content.contains("Output saved to"));
    assert!(content.contains("recover: read_file with offset/limit"));
    // Surviving preview edges are valid chars (no U+FFFD soup from
    // a mid-char slice would also pass `contains`, but the marker
    // proves the boundary-safe path ran).
    assert!(content.contains("<truncated: omitted"));
}

/// C6 / RULE-E-009 regression (AC2, spill-failure fallback arm):
/// when the spill directory cannot be created (`outputs` occupied
/// by a regular file), the inline 25 KB head slice of CJK output
/// must not panic either.
#[tokio::test]
async fn large_cjk_output_spill_failure_inline_no_panic() {
    let tmp = tempdir().unwrap();
    tokio::fs::write(tmp.path().join("outputs"), b"not a dir")
        .await
        .unwrap();
    let (content, _is_error, _, _) = execute(
        &serde_json::json!({"command": "yes 汉 | head -c 100000"}),
        &test_ctx(&tmp),
        Some("sess-cjk2"),
        &fresh_token(),
    )
    .await;
    assert!(!content.contains("Output saved to"));
    assert!(
        content.contains("<truncated: omitted"),
        "inline fallback marker missing: {}",
        &content[..200.min(content.len())]
    );
}

/// AC7.4: `cleanup_outputs_dir` removes the spill directory and
/// everything under it. Mirrors the cleanup hook that
/// `lib.rs::delete_session` invokes on session delete (PRD §R8).
#[tokio::test]
async fn cleanup_outputs_dir_removes_spill_dir() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join(".everlasting/outputs");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(dir.join("a.txt"), "first").await.unwrap();
    tokio::fs::write(dir.join("b.txt"), "second").await.unwrap();
    // A nested file too — remove_dir_all must wipe the whole tree.
    tokio::fs::create_dir_all(dir.join("nested")).await.unwrap();
    tokio::fs::write(dir.join("nested").join("c.txt"), "third")
        .await
        .unwrap();
    assert!(dir.exists());
    assert!(dir.join("a.txt").exists());
    assert!(dir.join("nested").join("c.txt").exists());

    cleanup_outputs_dir(tmp.path()).await;

    assert!(!dir.exists(), "spill dir should be removed");
    assert!(!dir.join("a.txt").exists());
    assert!(!dir.join("nested").join("c.txt").exists());
}

/// AC7.4: `cleanup_outputs_dir` is a silent no-op when the
/// spill directory doesn't exist (the session never spilled
/// anything, or it was already cleaned up). Must not panic and
/// must not log at error level.
#[tokio::test]
async fn cleanup_outputs_dir_noop_when_absent() {
    let tmp = tempdir().unwrap();
    // The `.everlasting/outputs/` directory was never created.
    // Calling cleanup must return immediately.
    cleanup_outputs_dir(tmp.path()).await;
    // Sanity: the (non-existent) dir is still non-existent.
    assert!(!tmp.path().join(".everlasting/outputs").exists());
}

/// AC7.4: `cleanup_outputs_dir` is best-effort and never panics,
/// even when invoked on a cwd that itself doesn't exist
/// (defensive: covers the race where the project was deleted
/// between session creation and session delete).
#[tokio::test]
async fn cleanup_outputs_dir_silent_on_missing_cwd() {
    // Point at a path that is guaranteed not to exist.
    let bogus = std::path::Path::new("/this/path/does/not/exist/anywhere");
    // Must not panic; tracing::warn! fires (we don't assert on
    // it — log capture is out of scope for unit tests).
    cleanup_outputs_dir(bogus).await;
}

// Suppress "tempdir import not used in some branches" — every
// test above uses it via test_ctx.
#[allow(dead_code)]
fn _unused() -> tempfile::TempDir {
    tempdir().unwrap()
}

// --- P0: timeout tests ---

/// Shell with short timeout kills a long-running command.
#[tokio::test]
async fn timeout_kills_long_command() {
    let tmp = tempdir().unwrap();
    let ctx = test_ctx(&tmp);
    let token = fresh_token();
    let (content, is_error, _, _) = execute(
        &serde_json::json!({
            "command": "sleep 60",
            "timeout": 500
        }),
        &ctx,
        None,
        &token,
    )
    .await;
    assert!(is_error);
    assert!(
        content.contains("[timeout after 500ms"),
        "expected timeout marker, got: {}",
        content
    );
    assert!(
        content.contains("partial output"),
        "expected partial output marker, got: {}",
        content
    );
}

/// Shell without timeout uses the default (120s). A fast command
/// completes normally.
#[tokio::test]
async fn no_timeout_uses_default() {
    let tmp = tempdir().unwrap();
    let (content, is_error, _, _) = execute(
        &serde_json::json!({"command": "echo hello"}),
        &test_ctx(&tmp),
        None,
        &fresh_token(),
    )
    .await;
    assert!(!is_error, "{}", content);
    assert!(content.contains("hello"));
    assert!(
        !content.contains("timeout"),
        "should not have timeout marker"
    );
}

/// timeout=0 is treated as default (120s). Fast command completes.
#[tokio::test]
async fn timeout_zero_treated_as_default() {
    let tmp = tempdir().unwrap();
    let (content, is_error, _, _) = execute(
        &serde_json::json!({
            "command": "echo ok",
            "timeout": 0
        }),
        &test_ctx(&tmp),
        None,
        &fresh_token(),
    )
    .await;
    assert!(!is_error, "{}", content);
    assert!(content.contains("ok"));
}

/// timeout=-1 is treated as default. Fast command completes.
#[tokio::test]
async fn timeout_negative_treated_as_default() {
    let tmp = tempdir().unwrap();
    let (content, is_error, _, _) = execute(
        &serde_json::json!({
            "command": "echo ok",
            "timeout": -1
        }),
        &test_ctx(&tmp),
        None,
        &fresh_token(),
    )
    .await;
    assert!(!is_error, "{}", content);
    assert!(content.contains("ok"));
}

/// timeout exceeding max is clamped. A fast command still completes.
#[tokio::test]
async fn timeout_exceeds_max_clamped() {
    let tmp = tempdir().unwrap();
    let (content, is_error, _, _) = execute(
        &serde_json::json!({
            "command": "echo clamped",
            "timeout": 999999999
        }),
        &test_ctx(&tmp),
        None,
        &fresh_token(),
    )
    .await;
    assert!(!is_error, "{}", content);
    assert!(content.contains("clamped"));
}

/// Timeout and cancel are distinct: timeout fires first, no cancel
/// marker.
#[tokio::test]
async fn timeout_fires_before_cancel() {
    let tmp = tempdir().unwrap();
    let ctx = test_ctx(&tmp);
    let token = fresh_token();
    // Use a short timeout. Don't cancel — let timeout fire.
    let (content, is_error, _, _) = execute(
        &serde_json::json!({
            "command": "sleep 10",
            "timeout": 300
        }),
        &ctx,
        None,
        &token,
    )
    .await;
    assert!(is_error);
    assert!(
        content.contains("[timeout after 300ms"),
        "expected timeout marker, got: {}",
        content
    );
    assert!(
        !content.contains("[cancelled"),
        "should not have cancel marker when timeout fires"
    );
}

// --- P0 (RULE-E-002): process_group(0) + kill PGID ---

/// Backgrounded grandchildren get killed on cancel.
///
/// Spawns a shell command that backgrounds a `sleep 60`, captures
/// its PID to a file, then `wait`s on it. We cancel the
/// `CancellationToken` and verify the sleep's `/proc/<pid>`
/// disappears within 2s — proving `kill_and_collect` reached the
/// grandchild (not just the direct `sh` child).
#[tokio::test]
async fn cancel_kills_backgrounded_grandchildren() {
    let tmp = tempdir().unwrap();
    let ctx = test_ctx(&tmp);
    let token = CancellationToken::new();
    let token_clone = token.clone();

    // The script writes sleep's PID to a file, then `wait`s on it.
    // `$!` is the PID of the most-recent backgrounded process.
    let pid_file = tmp.path().join("sleep.pid");
    let cmd = format!("sleep 60 & echo $! > {}; wait $!", pid_file.display());

    let handle = tokio::spawn(async move {
        execute(
            &serde_json::json!({"command": cmd}),
            &ctx,
            None,
            &token_clone,
        )
        .await
    });

    // Wait for the script to write the PID file (≤ ~1s).
    let mut attempts = 0;
    while !pid_file.exists() && attempts < 50 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        attempts += 1;
    }
    assert!(pid_file.exists(), "sleep pid file should exist");

    // Give sleep a moment to fully spawn before we cancel.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    token.cancel();
    let (content, is_error, _, _) = handle.await.unwrap();
    assert!(is_error);
    assert!(
        content.contains("[cancelled, partial output]"),
        "expected cancel marker, got: {}",
        content
    );

    // Verify the backgrounded sleep is gone.
    let pid_str = std::fs::read_to_string(&pid_file).unwrap();
    let pid: i32 = pid_str
        .trim()
        .parse()
        .expect("pid file should contain an integer");
    // /proc/<pid> should not exist on Linux — best-effort probe.
    let proc_path = format!("/proc/{}", pid);
    let mut gone = false;
    for _ in 0..40 {
        if !std::path::Path::new(&proc_path).exists() {
            gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        gone,
        "sleep grandchild should be killed, but {} still exists",
        proc_path
    );
}

/// Timeout kills pipeline descendants (`yes | head`).
///
/// `head -c 100` exits after 100 bytes, but `yes` keeps producing
/// output forever — without a process-group kill the `yes`
/// process keeps running and writing to a now-broken pipe. We
/// confirm timeout fires (so the tool returns an error to the
/// LLM) and that the backgrounded `yes` process spawned by the
/// tool is reaped along with the `sh` child.
#[tokio::test]
async fn timeout_kills_pipeline_grandchildren() {
    let tmp = tempdir().unwrap();
    let ctx = test_ctx(&tmp);
    let token = fresh_token();
    // The script writes:
    //   line 1: the sh PID ($$)
    //   line 2: the `yes` backgrounded PID ($!)
    // It then `wait`s on yes so the sh process blocks until
    // we kill the group.
    let pids_file = tmp.path().join("pids.txt");
    let cmd = format!(
        "sh -c 'echo $$ > {pids}; yes > /dev/null & echo $! >> {pids}; wait $!'",
        pids = pids_file.display()
    );
    let (content, is_error, _, _) = execute(
        &serde_json::json!({
            "command": cmd,
            "timeout": 200
        }),
        &ctx,
        None,
        &token,
    )
    .await;
    assert!(is_error, "{}", content);
    assert!(content.contains("[timeout after 200ms"), "got: {}", content);
    assert!(
        pids_file.exists(),
        "pids file should have been written: {}",
        content
    );

    // Read the recorded PIDs. sh's PID == PGID (per process_group(0)),
    // and the `yes` backgrounded child is in that group.
    let pids_raw = std::fs::read_to_string(&pids_file).unwrap();
    let mut pids_iter = pids_raw.lines();
    let sh_pid: i32 = pids_iter
        .next()
        .expect("sh pid line")
        .trim()
        .parse()
        .expect("sh pid is int");
    let yes_pid: i32 = pids_iter
        .next()
        .expect("yes pid line")
        .trim()
        .parse()
        .expect("yes pid is int");

    // Give the OS a moment to deliver the kill and reap the
    // descendants before we scan /proc.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Both /proc/<sh_pid> and /proc/<yes_pid> should be gone.
    for pid in [sh_pid, yes_pid] {
        let proc_path = format!("/proc/{}", pid);
        assert!(
            !std::path::Path::new(&proc_path).exists(),
            "process group descendant not killed: {} still exists",
            proc_path
        );
    }
}

// --- P0 (RULE-E-001): env_clear + safe allowlist ---

/// An API key set in the parent process must NOT be readable
/// by the child. This is the core invariant that closes the
/// env-leak attack surface (`env`, `printenv`, etc.).
#[tokio::test]
async fn execute_env_does_not_leak_api_key() {
    let secret = "sk-test-secret-do-not-leak-12345";
    let key = "ANTHROPIC_API_KEY";
    // SAFETY: `std::env::set_var` / `remove_var` are `unsafe` in
    // 1.74+ because the env table is process-global. We accept
    // the race risk because cargo test runs tests serially by
    // default for a single test process, and we always clean up
    // via `remove_var` even on assertion failure paths below.
    // If a parallel test ever touches this key, run with
    // `--test-threads=1`.
    unsafe {
        std::env::set_var(key, secret);
    }
    let tmp = tempdir().unwrap();
    let (content, is_error, _, _) = execute(
        &serde_json::json!({"command": "printenv ANTHROPIC_API_KEY || echo __EMPTY__"}),
        &test_ctx(&tmp),
        None,
        &fresh_token(),
    )
    .await;
    unsafe {
        std::env::remove_var(key);
    }

    assert!(!is_error, "{}", content);
    assert!(
        !content.contains(secret),
        "API key leaked through child env: {}",
        content
    );
    // Either the variable was unset (so `printenv` exited 1 and
    // the `|| echo __EMPTY__` branch ran) or it was empty. Either
    // way the secret value MUST be absent.
    assert!(
        content.contains("__EMPTY__") || content.trim().is_empty(),
        "expected empty/__EMPTY__ marker, got: {}",
        content
    );
}

/// `OPENAI_API_KEY` must be filtered out the same way — the
/// allowlist is keyed by *role* (never leak credentials), not by
/// specific provider names.
#[tokio::test]
async fn execute_env_does_not_leak_openai_key() {
    let secret = "sk-openai-secret-do-not-leak-67890";
    let key = "OPENAI_API_KEY";
    unsafe {
        std::env::set_var(key, secret);
    }
    let tmp = tempdir().unwrap();
    let (content, is_error, _, _) = execute(
        &serde_json::json!({"command": "printenv OPENAI_API_KEY || echo __EMPTY__"}),
        &test_ctx(&tmp),
        None,
        &fresh_token(),
    )
    .await;
    unsafe {
        std::env::remove_var(key);
    }

    assert!(!is_error, "{}", content);
    assert!(
        !content.contains(secret),
        "OPENAI_API_KEY leaked through child env: {}",
        content
    );
}

/// PATH must still be inherited so commands resolve. We use
/// `which sh` as a proxy: if PATH is missing the shell itself
/// would be unresolvable and we'd see exit code 127 / "not found".
#[tokio::test]
async fn execute_preserves_path() {
    let tmp = tempdir().unwrap();
    let (content, is_error, _, _) = execute(
        &serde_json::json!({"command": "command -v sh"}),
        &test_ctx(&tmp),
        None,
        &fresh_token(),
    )
    .await;
    assert!(!is_error, "{}", content);
    // `command -v sh` writes the resolved path to stdout. We only
    // care that the output is non-empty — the exact path depends
    // on the host PATH and may legitimately differ across
    // Linux/macOS.
    let stdout_line = content
        .lines()
        .find(|l| !l.starts_with("[exit code") && !l.is_empty())
        .unwrap_or("");
    assert!(
        !stdout_line.trim().is_empty(),
        "PATH should be inherited enough to resolve `sh`, got: {}",
        content
    );
}

/// The optional allowlist variables (HOME / LANG / TERM) are
/// re-injected when present in the parent. They may be empty in
/// CI (e.g. a `LANG=` build environment), in which case they
/// are simply not set in the child — the contract is "no
/// leakage", not "guaranteed presence".
#[tokio::test]
async fn execute_optional_env_vars_do_not_error() {
    let tmp = tempdir().unwrap();
    // `env` prints every variable in the child, one per line.
    // We only assert the command ran successfully — the count
    // and contents vary by host.
    let (content, is_error, _, _) = execute(
        &serde_json::json!({"command": "env | wc -l"}),
        &test_ctx(&tmp),
        None,
        &fresh_token(),
    )
    .await;
    assert!(!is_error, "{}", content);
    // The line containing the count (before "[exit code: 0]")
    // should be a valid integer >= 0.
    let count_line = content
        .lines()
        .find(|l| !l.starts_with("[exit code"))
        .unwrap_or("");
    let count: u32 = count_line
        .trim()
        .parse()
        .expect("wc -l should print a number");
    // Allowlist alone (PATH + SAFE_ENV_VARS) is at most 10 vars.
    // Anything more would mean an extra var leaked through. We
    // pick a tight upper bound (20) to leave headroom for
    // shell-internal vars (`_`, `OLDPWD`, etc.) without
    // masking a real leak.
    assert!(
        count <= 20,
        "child env has {} vars — possible leak: {}",
        count,
        content
    );
}

/// `apply_safe_env` is hard to test structurally (a
/// `tokio::process::Command` is opaque once built), so this
/// test is a defense-in-depth guard: it asserts that
/// [`SAFE_ENV_VARS`] does not contain any obviously-bad name,
/// catching future PRs that add a credential to the allowlist.
/// The behavioral guarantee lives in
/// `execute_env_does_not_leak_api_key` / `..._openai_key` —
/// those tests actually spawn a child and confirm the secret
/// is absent from `printenv`.
#[test]
fn apply_safe_env_clears_and_reinjects() {
    for forbidden in &[
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "*_TOKEN",
        "*_SECRET",
        "AWS_SECRET_ACCESS_KEY",
    ] {
        assert!(
            !SAFE_ENV_VARS.contains(forbidden),
            "SAFE_ENV_VARS must not contain {}",
            forbidden
        );
    }
    // Sanity: SAFE_ENV_VARS is non-empty (any positive set means
    // we re-inject at least one var; an empty list would still
    // satisfy the negative assertion above but indicates the
    // table was emptied by mistake).
    assert!(!SAFE_ENV_VARS.is_empty());
}
