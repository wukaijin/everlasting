//! Tool definitions and execution for the agent.
//!
//! Step 2 defines 3 built-in tools: `read_file`, `write_file`, `shell`.
//! Each tool has a `definition()` (for the LLM request) and an `execute()`
//! (for the agent runtime).
//!
//! Step 3b-1 introduces [`ToolContext`]: a per-turn struct injected
//! into every tool call that carries the active project's root and
//! the session's current working directory. The boundary check
//! (`projects::boundary::assert_within_root`) is the single source of
//! truth for "is this path inside the project?" — see
//! `.trellis/spec/backend/project-cwd-boundary.md` for the contract.
//!
//! Step toolset-extension adds 4 more tools: `edit_file`, `grep`,
//! `glob`, `list_dir`. `edit_file` requires a `ReadGuard` (Tauri
//! State) and a session id; the other 3 are pure functions like the
//! step 2 tools.

pub mod discard_worker;
pub mod edit_file;
pub mod glob;
pub mod grep;
pub mod list_dir;
pub mod merge_worker;
pub mod read_file;
pub mod read_guard;
pub mod remember;
pub mod run_background_shell;
pub mod shell;
pub mod shell_kill;
pub mod shell_status;
pub mod update_checklist;
pub mod use_skill;
pub mod web_fetch;
pub mod write_file;

use std::path::PathBuf;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

use crate::background_shell::DefaultRegistry;
use crate::llm::types::ToolDef;
use crate::skill::loader::SkillCache;
use crate::tools::read_guard::ReadGuard;
use crate::tools::update_checklist::ChecklistHandle;

/// L3b PR3 (2026-06-27): test-only helper that returns a
/// process-wide in-memory SQLite pool. The pool is built on
/// first call (with the test schema migrated) and cached in
/// a `OnceLock` for reuse across tests. Used by tool-level
/// `test_ctx` helpers to populate `ToolContext.db` without
/// each test having to spin up its own pool.
///
/// The pool is `sqlite::memory:` so it's per-process; tests
/// that need a fresh DB must use a different helper (or set
/// `PRAGMA` truncate / re-migrate). For the tools' unit
/// tests the in-memory pool is fine because the tools
/// themselves don't query the DB (only `merge_worker` /
/// `discard_worker` do, and those are exercised in the
/// integration suite in `agent/tests_subagent.rs`).
#[cfg(test)]
pub(crate) fn test_default_pool() -> SqlitePool {
    use std::sync::OnceLock;

    static POOL: OnceLock<SqlitePool> = OnceLock::new();
    POOL.get_or_init(|| {
        // The init is async; we need a runtime to drive it.
        // We always create a fresh `current_thread` runtime
        // on a NEW OS thread — this sidesteps the
        // "Cannot start a runtime from within a runtime"
        // pitfall that hits `#[tokio::test]` callers
        // (which already have a current_thread or
        // multi_thread runtime in scope). The new thread
        // has no current runtime, so `block_on` works
        // without any nesting.
        let (tx, rx) = std::sync::mpsc::channel::<SqlitePool>();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test_default_pool: new current_thread runtime");
            let pool = rt.block_on(async {
                let pool = sqlx::SqlitePool::connect("sqlite::memory:")
                    .await
                    .expect("test_default_pool: in-memory connect");
                sqlx::query("PRAGMA foreign_keys = ON")
                    .execute(&pool)
                    .await
                    .expect("test_default_pool: foreign_keys");
                pool
            });
            let _ = tx.send(pool);
        });
        rx.recv()
            .expect("test_default_pool: receive from init thread")
    })
    .clone()
}

/// All built-in tools available as of step 2 + the toolset extension.
///
/// `dispatch_subagent` (B6, 2026-06-19) is **NOT** in this list as
/// of L3d PR3 (2026-06-25): its enum must reflect builtin + user +
/// project subagents merged by `SubagentCache`, so it can no longer
/// be a static member of the startup `state.tools` snapshot. Instead
/// `chat_loop.rs:957` appends `definition_with_cache(&subagent_cache,
/// project_path)` to the per-turn `turn_tool_defs` AFTER
/// `filter_tools_for_mode`. Worker agents share the same
/// `run_chat_loop` body, so the worker-nesting guard is enforced by
/// a SEPARATE mechanism: the per-turn append is gated on
/// `effective_is_worker == false` (see `chat_loop.rs:1002`), so the
/// worker's turn tool list never carries `dispatch_subagent` even
/// though it traverses the same code path.
/// `filter_tools_for_subagent` (which strips `dispatch_subagent` via
/// `STRUCTURALLY_DISABLED`) remains as defense-in-depth on the seed
/// `worker_tool_defs` list (`dispatch.rs:187`) — it strips
/// `dispatch_subagent` / `update_checklist` / the L1a shell triple
/// from the worker's initial toolset, but that filter alone would be
/// insufficient because the per-turn append runs inside the same
/// `run_chat_loop` body the worker also reaches. The
/// `effective_is_worker` gate is the load-bearing guard; the filter
/// is belt-and-suspenders.
pub fn builtin_tools() -> Vec<ToolDef> {
    vec![
        read_file::definition(),
        write_file::definition(),
        edit_file::definition(),
        shell::definition(),
        grep::definition(),
        glob::definition(),
        list_dir::definition(),
        web_fetch::definition(),
        use_skill::definition(),
        update_checklist::definition(),
        run_background_shell::definition(),
        shell_status::definition(),
        shell_kill::definition(),
        // L3b PR3 (2026-06-27): worker branch merge / discard tools.
        // Both route to `ToolKind::GitMutation` in Tier 4 (`Risk::High`,
        // tool-level grant + ask, mirroring WebFetch — the input is a
        // `run_id` UUID, not a filesystem path). Plan mode filters them
        // out via `filter_tools_for_mode`; worker subagents can't reach
        // them (`STRUCTURALLY_DISABLED`). `do_merge_blocking` serializes
        // per parent session (`merge_lock_for`).
        merge_worker::definition(),
        discard_worker::definition(),
        // P2 (2026-06-29): agent self-writes a long-term memory.
        // Silent Allow (no Tier 4 ask — epic "全自主写" decision); the
        // write-safety net in `db::memories::insert_memory` is the
        // guard. `Risk::Low` (the `_` default). Plan mode keeps it
        // (writes land in the DB, not the filesystem).
        remember::definition(),
    ]
}

/// Per-turn context passed to every tool execution. Built once per
/// agent turn (in `lib.rs::chat`) from the active project / session
/// state, then handed (immutably) to each tool call.
///
/// - `worktree_path`: canonical absolute path of the active project
///   (resolved via `boundary::assert_within_root` at turn start).
/// - `cwd`: canonical absolute path of the session's current working
///   directory. This is the cwd the agent "lives in" between shell
///   tool calls; LLM-supplied `working_directory` overrides are
///   validated against `worktree_path` and, if accepted, returned
///   through the [`ToolContextUpdate`] the shell tool can emit.
/// - `checklist`: B12 per-request checklist handle. The agent loop
///   constructs a fresh `Arc<Mutex<Vec<ChecklistItem>>>` per
///   `run_chat_loop` call and threads it through here so the
///   `update_checklist` tool can atomically mutate the Vec. The
///   same handle is read every turn (after C3 compaction, before
///   `provider.send`) to build the ephemeral checklist injection
///   block. The handle is `Clone` (it's an `Arc`) so the existing
///   per-turn `ToolContext` clone pattern is unaffected.
/// - `background_shells`: L1a cross-request registry handle. The
///   `run_background_shell` / `shell_status` / `shell_kill` tools
///   call into this handle to start / query / kill background
///   processes whose lifetimes span multiple turns (and multiple
///   `invoke("chat")` calls — see `.trellis/tasks/06-19-l1-shell-pty/
///   prd.md` Q1 decision). Held as a concrete `Arc<...>` rather
///   than `dyn` to match the codebase's pattern for the other
///   cross-request handles (`MemoryCache`, `SkillCache`,
///   `ReadGuard`).
///
/// No `Debug` derive: the registry's `Inner` carries
/// `HashMap<_, ShellEntry>` whose fields are deliberately opaque
/// (kill-tx oneshot senders, stdout/stderr buffers), and no
/// current caller needs `{:?}` formatting on the whole context.
/// Tools that need debug logging have access to the individual
/// fields (e.g. `tracing::info!(cwd = ?ctx.cwd, ...)`).
///
/// L3b PR3 (2026-06-27): added `db: SqlitePool` so the
/// `merge_worker` / `discard_worker` tools can read
/// `subagent_runs` rows (to look up the worker's worktree
/// path) without needing the pool plumbed through the tool's
/// own execute signature. `SqlitePool` is `Clone` (it's an
/// `Arc` internally) so the per-turn `ToolContext::clone()`
/// pattern is unaffected. The pool is the agent loop's
/// `db: SqlitePool` parameter; the LLM is trusted to be the
/// only one writing to `subagent_runs` here (RULE-A-016
/// isolation: worker decisions stay in `transcript`, not
/// the parent's audit table).
#[derive(Clone)]
pub struct ToolContext {
    pub worktree_path: PathBuf,
    pub cwd: PathBuf,
    pub checklist: ChecklistHandle,
    pub background_shells: DefaultRegistry,
    pub db: SqlitePool,
    /// P2 (2026-06-29): the session's `projects.id` UUID. Used by
    /// the `remember` tool to bind a project-scope memory to the
    /// same identifier the session-start recall
    /// (`memory_recall::build_recall_text`) filters by — keeping
    /// the two sides in lockstep so a written memory is
    /// immediately recallable. Distinct from `worktree_path`
    /// (a filesystem path) — the two are different concepts that
    /// happen to identify the same project.
    pub project_id: String,
    /// 06-30 follow-up: app-global data directory (resolved once
    /// by `AppState::load` from `tauri::AppHandle::app_data_dir`).
    /// Tool-layer callers that need to construct absolute
    /// paths under `<data_dir>/worktrees/...` (e.g. lazy
    /// auto-attach inside `merge_worker::ensure_parent_worktree
    /// _attached`) read it from here instead of a separate
    /// parameter. The Tauri IPC side has direct access via
    /// `State::app_data_dir`; the test path uses a tmpdir
    /// placeholder since the worktree tools under test don't
    /// depend on the data dir layout.
    pub data_dir: PathBuf,
}

/// Optional per-tool update to the tool context. The shell tool uses
/// this to report a new `cwd` (e.g. after a successful `cd`); the
/// agent loop tracks the latest one and writes the final value to
/// `sessions.current_cwd` once at the end of the turn (see
/// `docs/PROPOSAL-project-binding-and-top-tabs.md` §4.4 "turn 结束
/// 一次性写").
#[derive(Debug, Clone, Default)]
pub struct ToolContextUpdate {
    pub new_cwd: Option<PathBuf>,
}

/// Execute a tool by name. Returns `(content_string, is_error,
/// ctx_update, exit_code)`.
///
/// `ctx` is injected (not held) — see `PROPOSAL §4.4` / GLM review
/// §1.2. Tools are pure functions, testable in isolation.
///
/// The `edit_file` and `read_file` tools additionally take a
/// `ReadGuard` and `session_id`; the dispatch here routes the right
/// combination of arguments. The guard is a Tauri-managed `State`
/// cloned in by `lib.rs::chat` so the dispatch signature stays
/// uniform for tools that don't need it.
///
/// C1 (Cancel): `cancel` is a `CancellationToken` from the agent
/// loop. The dispatch wraps every tool execution in `tokio::select!`
/// so cancellation interrupts even long-running tools (e.g. shell).
/// Tools that need custom cleanup (e.g. killing a child process)
/// receive the token in their own execute signature; the outer
/// select! provides a generic safety net for all tools.
///
/// **exit_code (C4 PR1, 2026-06-14)**: the 4th tuple element is
/// `Option<i32>`. Only `shell` returns `Some(code)` (the child
/// process exit status); every other tool returns `None` (they
/// don't spawn a process). The agent loop feeds this into the
/// `tool_executed` audit row so the C4 audit-log UI can color
/// non-zero exit codes without re-parsing the formatted content
/// string. **Never** use `Some(0)` as a sentinel for "no exit
/// code" — that conflates a successful shell run with the "N/A"
/// case the UI renders for path tools.
pub async fn execute_tool(
    name: &str,
    input: &serde_json::Value,
    ctx: &ToolContext,
    guard: Option<&ReadGuard>,
    session_id: Option<&str>,
    skill_cache: Option<&SkillCache>,
    cancel: CancellationToken,
) -> (String, bool, ToolContextUpdate, Option<i32>) {
    // C1: generic cancel wrapper for all tools. The `biased;` ensures
    // the cancel arm is polled first when both are ready, so a
    // cancelled request returns immediately even if the tool future
    // is also ready.
    tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            tracing::info!(tool = %name, "execute_tool: cancelled before/during tool execution");
            ("Tool execution was cancelled".to_string(), true, ToolContextUpdate::default(), None)
        }
        result = execute_tool_inner(name, input, ctx, guard, session_id, skill_cache, &cancel) => {
            result
        }
    }
}

/// Inner dispatch without the cancel wrapper. Tools that need the
/// token (e.g. shell for child.kill()) receive it here.
async fn execute_tool_inner(
    name: &str,
    input: &serde_json::Value,
    ctx: &ToolContext,
    guard: Option<&ReadGuard>,
    session_id: Option<&str>,
    skill_cache: Option<&SkillCache>,
    cancel: &CancellationToken,
) -> (String, bool, ToolContextUpdate, Option<i32>) {
    match name {
        "read_file" => {
            let (out, is_err) = read_file::execute(input, ctx, guard, session_id).await;
            (out, is_err, ToolContextUpdate::default(), None)
        }
        "write_file" => {
            let (out, is_err) = write_file::execute(input, ctx).await;
            (out, is_err, ToolContextUpdate::default(), None)
        }
        "edit_file" => match (guard, session_id) {
            (Some(g), Some(sid)) => {
                let (out, is_err) = edit_file::execute(input, ctx, g, sid).await;
                (out, is_err, ToolContextUpdate::default(), None)
            }
            _ => (
                "edit_file called without a ReadGuard / session_id; this is a bug."
                    .to_string(),
                true,
                ToolContextUpdate::default(),
                None,
            ),
        },
        "shell" => {
            let (out, is_err, update, exit_code) = shell::execute(input, ctx, session_id, cancel).await;
            (out, is_err, update, exit_code)
        }
        "grep" => {
            let (out, is_err) = grep::execute(input, ctx).await;
            (out, is_err, ToolContextUpdate::default(), None)
        }
        "glob" => {
            let (out, is_err) = glob::execute(input, ctx).await;
            (out, is_err, ToolContextUpdate::default(), None)
        }
        "list_dir" => {
            let (out, is_err) = list_dir::execute(input, ctx).await;
            (out, is_err, ToolContextUpdate::default(), None)
        }
        "web_fetch" => {
            let (out, is_err) = web_fetch::execute(input, ctx).await;
            (out, is_err, ToolContextUpdate::default(), None)
        }
        "use_skill" => match skill_cache {
            Some(cache) => {
                let (out, is_err) = use_skill::execute(input, cache, ctx).await;
                (out, is_err, ToolContextUpdate::default(), None)
            }
            None => (
                "use_skill called without a SkillCache; this is a bug.".to_string(),
                true,
                ToolContextUpdate::default(),
                None,
            ),
        },
        "update_checklist" => {
            // B12: atomically replace the loop's checklist Vec via
            // the handle threaded through `ToolContext`. The handle
            // is per-request (one per `run_chat_loop` call); a
            // cancelled tool execution is caught by the outer
            // `execute_tool`'s `tokio::select!` cancel wrapper, so
            // the loop's RULE-A-004 ordering (audit AFTER cancel
            // check) automatically protects the checklist too.
            let (out, is_err) = update_checklist::execute(input, &ctx.checklist).await;
            (out, is_err, ToolContextUpdate::default(), None)
        }
        "run_background_shell" => {
            // L1a: fire-and-forget shell. Returns immediately with
            // a `shell_session_id` handle; the spawned task's
            // completion notification is drained at the start of
            // the next agent-loop turn.
            let (out, is_err, update) =
                run_background_shell::execute(input, ctx, session_id).await;
            (out, is_err, update, None)
        }
        "shell_status" => {
            // L1a: query a background shell's current state.
            // Reads-only; no process exit code.
            let (out, is_err, update) = shell_status::execute(input, ctx, session_id).await;
            (out, is_err, update, None)
        }
        "shell_kill" => {
            // L1a: SIGKILL a background shell's process group.
            // Idempotent — killing a Done shell is a no-op success.
            let (out, is_err, update) = shell_kill::execute(input, ctx, session_id).await;
            (out, is_err, update, None)
        }
        // L3b PR3 (2026-06-27): merge the worker's preserved
        // `worker/<run_id>` branch into the parent session's
        // `session/<id>` branch. Fast-forward or 3-way merge;
        // conflict → `is_error: true` with the conflict file list
        // and the worker branch + worktree preserved for the user
        // to resolve manually.
        "merge_worker" => {
            let (out, is_err, update, exit_code) =
                merge_worker::execute(input, ctx, session_id).await;
            (out, is_err, update, exit_code)
        }
        // L3b PR3 (2026-06-27): discard the worker's preserved
        // branch + worktree (no merge). Fail-fast on an
        // already-destroyed run (MVP 不做幂等).
        "discard_worker" => {
            let (out, is_err, update, exit_code) =
                discard_worker::execute(input, ctx, session_id).await;
            (out, is_err, update, exit_code)
        }
        // P2 (2026-06-29): remember tool — agent self-writes a
        // candidate-status memory. Silent Allow (no Tier 4 ask);
        // the write-safety net + per-session ≤50 cap are the
        // guards. `session_id` becomes `source_session_id` for
        // frequency-control accounting.
        "remember" => {
            let (out, is_err) = remember::execute(input, ctx, session_id).await;
            (out, is_err, ToolContextUpdate::default(), None)
        }
        _ => (
            format!("Unknown tool: {}", name),
            true,
            ToolContextUpdate::default(),
            None,
        ),
    }
}
