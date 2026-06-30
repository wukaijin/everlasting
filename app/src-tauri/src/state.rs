//! Process-wide application state.
//!
//! Lives in its own module (post-PR1 of the audit task) so `lib.rs` is
//! a thin entry point and the per-module `commands/*` / `agent/*`
//! files can `use crate::state::{AppState, ...}` without dragging
//! the whole god-module in.
//!
//! Contents:
//! - [`AppState`] — owned by `tauri::State<Arc<AppState>>`; carries
//!   DB pool, LLM config + tool registry, in-flight cancellation
//!   map, session→active-request map, read guard, and the
//!   catalog-resolved `ProviderCatalog` (PR1 grill decision #3).
//! - [`CancellationGuard`] — RAII guard that removes the
//!   `(request_id → token)` AND `(session_id → request_id)` entries
//!   on Drop (grill decision #1: lives here next to AppState).
//! - [`ChatEventPayload`] / [`ToolCallPayload`] / [`ToolResultPayload`]
//!   — Tauri event payloads emitted by the agent loop and the
//!   `chat-event` / `tool:call` / `tool:result` channels.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Mutex, RwLock, oneshot};
use tokio_util::sync::CancellationToken;

use crate::agent::permissions::PermissionAskPayload;
use crate::agent::permissions::PermissionStore;
use crate::agent::question_store::QuestionStore;
use crate::agent::subagent::SubagentCache;
use crate::llm::{ChatEvent, LlmConfig, Provider, ToolDef};
use crate::memory::MemoryCache;
use crate::resource_loader::CommandCache;
use crate::skill::loader::SkillCache;
use crate::tools::read_guard::ReadGuard;

/// Catalog of `Arc<dyn Provider>` instances, keyed by
/// `models.id` (the catalog `models.id`, NOT `model.model_name`).
///
/// Built once at `AppState::load` time by walking
/// `db::list_providers` and constructing a `Provider` per row
/// (using `build_provider` from the LLM module). The chat command
/// looks up `provider: Arc<dyn Provider>` by the resolved
/// `model_id` and clones the `Arc` into its spawn task.
///
/// The catalog is shared across every concurrent chat — providers
/// are stateless w.r.t. requests (per-turn state lives in the
/// `Pin<Box<dyn Stream<...>>>` returned by `Provider::send`), so
/// sharing the `Arc<dyn Provider>` is safe and avoids re-running
/// `build_provider` for every chat invocation.
///
/// Grill decision #3 (locked): the chat command no longer calls
/// `resolve_chat_provider(db)` on every chat — it does a single
/// `catalog.get(&model_id)` instead. Failures during `AppState::load`
/// (e.g. a malformed protocol) degrade gracefully: a `warn!` log
/// and the entry is skipped, so a single bad row doesn't prevent
/// the whole catalog from loading.
pub type ProviderCatalog = HashMap<String, Arc<dyn Provider>>;

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// Process-wide state, owned by `tauri::State<Arc<AppState>>`.
///
/// Grill decision #2 (locked): the `catalog` field sits immediately
/// after `db`, so the "data plane" (DB + provider catalog) is
/// grouped together. All other fields keep their pre-PR1 order to
/// keep the diff small.
pub struct AppState {
    /// Legacy cold-start LLM config (env-derived). Kept for the
    /// `get_llm_config` IPC fallback path and any future
    /// "no-catalog" mode; the chat command itself reads from the
    /// `catalog` instead.
    pub config: LlmConfig,
    /// Static list of tool definitions registered with the agent.
    pub tools: Vec<ToolDef>,
    /// SQLite connection pool (single process, but sqlx pools the
    /// connections internally).
    pub db: SqlitePool,
    /// Grill decision #3: pre-built provider catalog keyed by
    /// `models.id`. The chat command does a single lookup here
    /// instead of re-running `build_provider` on every chat.
    ///
    /// Wrapped in `RwLock` so provider/model CRUD commands can
    /// rebuild the catalog on config changes without restarting
    /// the app. Read-heavy (every chat), write-rare (user saves
    /// config).
    pub catalog: Arc<RwLock<ProviderCatalog>>,
    /// RULE-E-006: Tauri-resolved app data dir. Same root as the
    /// SQLite db above; worktree storage lives under
    /// `<app_data_dir>/worktrees/<project_uuid>/<session_uuid>`.
    /// Replaces the old env-based `git::data_dir()` whose `/tmp`
    /// fallback risked data loss on reboot.
    ///
    /// Lives in the data-plane group (right after `catalog`) so
    /// the data-root invariants are colocated, while preserving
    /// Grill decision #2 (`catalog` immediately after `db`).
    pub app_data_dir: std::path::PathBuf,
    /// Active chat request cancellation tokens, keyed by `request_id`.
    /// The frontend's Stop button calls `cancel_chat(request_id)`
    /// which looks up the token and calls `.cancel()`. The agent
    /// loop is wrapped in `tokio::select!` and listens for
    /// cancellation between events. The entry is removed by the
    /// spawn task on every exit path (normal / error / cancel).
    pub cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    /// Per-session → active request id, for the destructive
    /// in-flight cancel hook (`delete_session`, `detach_worktree`,
    /// `delete_worktree`). When the user invokes any of these, we
    /// look up the session's active `request_id` and cancel it
    /// BEFORE the destructive work runs, so the LLM can't write
    /// to a half-deleted worktree.
    pub session_active_request: Arc<Mutex<HashMap<String, String>>>,
    /// RULE-E-005 (2026-06-15): request_id → "agent loop exited"
    /// signal. The `chat` command creates a `oneshot` pair per
    /// spawn, stores the `Receiver` here keyed by `request_id`,
    /// and moves the `Sender` into the spawn closure. When
    /// `run_chat_loop` returns (the agent loop has fully exited —
    /// including any in-flight tool that was already dispatched
    /// when cancel fired), the closure `.send(())`s, resolving
    /// the receiver. The destructive commands (`delete_session` /
    /// `detach_worktree` / `delete_worktree`) `await` the receiver
    /// (via `cancel_inflight_for_session`) with a defensive
    /// timeout BEFORE doing the destructive work, closing the race
    /// where the loop writes into a just-deleted worktree. The
    /// entry is taken out of the map by `cancel_inflight_for_session`
    /// (single-consumer receiver); if no destructive op drains it,
    /// the spawn closure removes it after `.send`.
    pub inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>>,
    /// Per-session read fingerprints. The `edit_file` tool consults
    /// this guard to ensure the LLM (a) read the file in the current
    /// session and (b) the file hasn't been modified on disk since.
    /// Lives in process state, not persisted.
    pub read_guard: ReadGuard,
    /// B5 Memory cache (V2 1 期, 2026-06-10; mtime fence
    /// 2026-06-15). Holds the 4 fixed memory files (2 layers
    /// × 2 sources) for the duration of the process. Each
    /// `load_for_session` stats every file's `mtime` and
    /// reloads on change — no background watcher. Lives in
    /// process state, not persisted.
    pub memory_cache: Arc<MemoryCache>,
    /// B3 /command cache (2026-06-16). Scanned user + project command
    /// files with a read-through mtime fence (RULE-C-001 pattern,
    /// same idea as `memory_cache`). Lives in process state, not
    /// persisted.
    pub command_cache: Arc<CommandCache>,
    /// B4 skill cache (2026-06-18). Scanned user + project skill
    /// directories with a read-through mtime fence (same pattern as
    /// `command_cache` / `memory_cache`). Independent loader — a
    /// deliberate copy of the B3 `resource_loader` shape (see
    /// `skill/loader.rs` doc). PR1: load layer only; PR2 wires the
    /// `use_skill` tool + L0 listing injection.
    pub skill_cache: Arc<SkillCache>,
    /// L3d (2026-06-25): user + project custom subagent definitions
    /// (`~/.config/everlasting/agents/*.md` and
    /// `<project>/.everlasting/agents/*.md`). Read-through mtime
    /// fence (same pattern as `memory_cache` / `command_cache` /
    /// `skill_cache`): every `chat_loop` turn calls
    /// `definition_with_cache(&subagent_cache, project_path)` to
    /// build the dynamic `dispatch_subagent` ToolDef enum, and
    /// `run_subagent` calls `cache.lookup(project_path, name)` to
    /// resolve the worker. Adding / editing / deleting a `.md` is
    /// picked up on the next chat turn without a reload command.
    /// Builtins (`researcher` + `general-purpose`) are always
    /// present regardless of `.md` failures.
    pub subagent_cache: Arc<SubagentCache>,
    /// A2 + B7 (Permission system + per-session Mode,
    /// 2026-06-13): in-flight `permission:ask` oneshot senders,
    /// keyed by request id (UUID generated by the agent loop on
    /// Tier 3 emit). The IPC `permission_response` handler in
    /// `commands/permissions.rs` looks up the rid and sends the
    /// user's decision, which wakes the agent loop's
    /// `tokio::select!` arm inside `permissions::check`.
    pub permission_asks: PermissionStore,
    /// 2026-06-30 (`ask_user_question` task): in-flight
    /// `ask_user_question` oneshot senders, keyed by session id.
    /// Mirrors `permission_asks` (parallel shape, separate store
    /// — see `agent::question_store` doc for the
    /// PermissionStore-vs-QuestionStore rationale).
    pub question_store: QuestionStore,
    /// L1a (2026-06-19): cross-request background-shell registry.
    /// Lives in `AppState` so the agent loop, the
    /// `run_background_shell` / `shell_status` / `shell_kill`
    /// tools, `delete_session`, and the `RunEvent::Exit` hook all
    /// share the same handle. The agent loop clones the `Arc`
    /// into `ToolContext` every turn; the tools call into it from
    /// there.
    ///
    /// The concrete `Arc<InMemoryBackgroundShellRegistry>` type
    /// (vs. `Arc<dyn BackgroundShellRegistry>`) matches the
    /// pattern used by the other cross-request handles in
    /// `AppState` (`MemoryCache`, `SkillCache`, `ReadGuard`) —
    /// the daemon-ization swap is a future PR that touches only
    /// `AppState::load` (per L1 PRD Q1 decision C).
    pub background_shells: crate::background_shell::DefaultRegistry,
}

impl AppState {
    /// Bootstrap app state. Called from `lib.rs::run`'s setup
    /// closure via `tauri::async_runtime::block_on`.
    ///
    /// Responsibilities:
    /// 1. Load the env-derived LLM config (cold-start fallback).
    /// 2. Open the SQLite pool + run migrations.
    /// 3. Spawn the git-metadata backfill task for pre-PR2 projects.
    /// 4. **NEW (grill decision #3)**: build the `ProviderCatalog`
    ///    from `db::list_providers`. Failures are logged + the
    ///    provider is skipped; the catalog is still returned
    ///    (possibly empty) so `AppState::load` doesn't unwind on
    ///    a single bad row.
    pub async fn load(app: &AppHandle) -> Self {
        // Cold-start env-derived config is only consulted by the
        // `get_llm_config` IPC fallback and the (currently unused)
        // "no-catalog" code path. The chat command reads from the
        // DB-backed `ProviderCatalog` built below — a missing env
        // var here only matters if the DB also has no configured
        // provider, in which case the first chat surfaces an
        // `LlmError::Auth`. Log at `info` so cold start isn't noisy.
        let config = LlmConfig::from_env().unwrap_or_else(|e| {
            tracing::info!(
                error = %e,
                "cold-start LlmConfig::from_env failed; using unconfigured (DB provider catalog takes precedence)"
            );
            LlmConfig::unconfigured()
        });
        let tools = crate::tools::builtin_tools();
        tracing::info!(
            base_url = %config.base_url,
            model = %config.model,
            tools_count = tools.len(),
            thinking_effort = %config.thinking_effort,
            "LLM config loaded"
        );

        // Resolve app_data_dir, then open SQLite there.
        let app_data_dir = app
            .path()
            .app_data_dir()
            .expect("failed to resolve app_data_dir");
        let db_path = app_data_dir.join("everlasting.db");
        let db = crate::db::init_pool(&db_path)
            .await
            .expect("failed to open sqlite pool");
        crate::db::run_migrations(&db)
            .await
            .expect("failed to run migrations");
        tracing::info!(db_path = %db_path.display(), "sqlite ready");

        // 2026-06-26: reap subagent_runs left `running` by a crashed
        // previous process. Without this, a worker whose terminal
        // `update_run_finished` never ran (process killed mid-dispatch)
        // renders as a phantom "still in progress" run in the UI
        // forever. Best-effort — a failure is logged, not fatal.
        match crate::db::subagent_runs::reap_orphaned_runs(&db).await {
            Ok(n) if n > 0 => tracing::info!(
                reaped = n,
                "startup: reaped orphaned subagent_runs (marked error)"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(
                error = %e,
                "startup: failed to reap orphaned subagent_runs (non-fatal)"
            ),
        }

        // Grill decision #3: build the provider catalog. We do this
        // BEFORE the backfill spawn so a backfill panic doesn't
        // leave the catalog half-built.
        let catalog = build_provider_catalog(&db).await;

        // B5 Memory cache. Read-through with an mtime fence
        // (RULE-C-001, 2026-06-15): every `load_for_session`
        // stats each file and reloads on change, so no
        // background watcher is needed — the cache is always
        // fresh on the next read. New projects are picked up
        // automatically (first load stats the file).
        let memory_cache = MemoryCache::arc();

        // B3 /command cache. Same read-through mtime-fence pattern as
        // memory: every `list_commands` stats the dir's *.md files
        // and re-scans only on a change.
        let command_cache = CommandCache::arc();

        // B4 skill cache. Same read-through mtime-fence pattern; skill
        // dirs hold `<name>/SKILL.md` (a directory, not a single file).
        let skill_cache = SkillCache::arc();

        // L3d (2026-06-25): subagent cache. Same read-through
        // mtime-fence pattern; `chat_loop.rs:957` builds the dynamic
        // `dispatch_subagent` enum from `cache.list(project_path)` on
        // every turn so a freshly-written `.md` is picked up without
        // a reload command. Builtins are merged in at list time.
        let subagent_cache = SubagentCache::arc();

        // Startup batch backfill of pre-PR2 project rows. The fix:
        // spawn a fire-and-forget task that re-probes the git
        // status of every stale project, writes the result, and
        // emits a Tauri event so the frontend can refresh its
        // in-memory list. The spawn happens AFTER migrations run
        // and is `tauri::async_runtime::spawn`-based.
        let backfill_pool = db.clone();
        let backfill_app = app.clone();
        tauri::async_runtime::spawn(async move {
            match crate::projects::store::batch_reprobe_git_metadata(&backfill_pool).await {
                Ok(updated) => {
                    if updated > 0 {
                        if let Err(e) = backfill_app.emit("projects:refreshed", updated) {
                            tracing::warn!(
                                error = %e,
                                updated,
                                "emit projects:refreshed failed"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "git metadata backfill failed");
                }
            }
        });

        Self {
            config,
            tools,
            db,
            catalog: Arc::new(RwLock::new(catalog)),
            app_data_dir,
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            session_active_request: Arc::new(Mutex::new(HashMap::new())),
            inflight_exits: Arc::new(Mutex::new(HashMap::new())),
            read_guard: ReadGuard::new(),
            memory_cache,
            command_cache,
            skill_cache,
            subagent_cache,
            permission_asks: crate::agent::permissions::new_permission_store(),
            // 2026-06-30 (`ask_user_question` task): fresh
            // in-memory oneshot registry. Parallel to
            // `permission_asks` (see `agent::question_store` for
            // the rationale). Lifted from `load_for_session` /
            // `register` / `resolve` (no mtime fence — the store
            // is purely in-process).
            question_store: QuestionStore::new(),
            // L1a (2026-06-19): fresh in-memory background-shell
            // registry. The single GUI-process impl
            // (`InMemoryBackgroundShellRegistry`) holds a
            // `HashMap<(session_id, shell_id), ShellEntry>` plus
            // a per-session completion-notification queue. Lives
            // for the process lifetime; `kill_all` is invoked
            // from the `RunEvent::Exit` hook so app shutdown
            // doesn't leak process groups.
            background_shells: crate::background_shell::default_registry(),
        }
    }

    /// Rebuild the in-memory provider catalog from the DB.
    /// Called after any provider/model CRUD operation so the next
    /// chat request picks up the new config without restart.
    pub async fn rebuild_catalog(&self) {
        let new_catalog = build_provider_catalog(&self.db).await;
        let mut guard = self.catalog.write().await;
        *guard = new_catalog;
    }
}

/// Build the `ProviderCatalog` (grill decision #3) from the DB.
///
/// For every row in `db::list_providers`, look up the list of
/// models it owns, and for each model construct a `Box<dyn Provider>`
/// via `build_provider`. The map is keyed by `models.id`.
///
/// Per-provider failure modes:
/// - `build_provider` returns `Err`: log a `warn!` and skip. A
///   single bad protocol string doesn't poison the whole catalog.
/// - The provider row has no models: skip (nothing to dispatch).
///
/// `ModelWithProvider` already denormalizes the provider info;
/// we use `list_providers` to get the `api_key` (the secret) and
/// iterate `list_models` per provider to keep the lookup O(P*M)
/// instead of re-querying per model.
async fn build_provider_catalog(db: &SqlitePool) -> ProviderCatalog {
    let mut catalog: ProviderCatalog = HashMap::new();

    let providers = match crate::db::list_providers(db).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "build_provider_catalog: list_providers failed; starting with empty catalog");
            return catalog;
        }
    };

    let models = match crate::db::list_models(db).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "build_provider_catalog: list_models failed; starting with empty catalog");
            return catalog;
        }
    };

    for provider_row in &providers {
        // Find every model owned by this provider.
        let owned: Vec<&crate::db::ModelRow> = models
            .iter()
            .filter(|mwp| mwp.model.provider_id == provider_row.id)
            .map(|mwp| &mwp.model)
            .collect();
        if owned.is_empty() {
            // A provider with no models is fine (just unused); we
            // skip without logging because the user might be in
            // the middle of adding their first model.
            continue;
        }
        for model_row in owned {
            // `build_provider` is sync; it just constructs the
            // adapter object — no I/O. Cheap to call per row.
            match crate::llm::build_provider(provider_row, model_row) {
                Ok(provider) => {
                    catalog.insert(model_row.id.clone(), Arc::from(provider));
                }
                Err(e) => {
                    tracing::warn!(
                        provider = %provider_row.display_name,
                        model = %model_row.display_name,
                        error = %e,
                        "build_provider_catalog: skipping model (build_provider failed)"
                    );
                }
            }
        }
    }

    tracing::info!(
        catalog_size = catalog.len(),
        "provider catalog built"
    );
    catalog
}

// ---------------------------------------------------------------------------
// CancellationGuard
// ---------------------------------------------------------------------------

/// RAII guard that removes a request_id from the cancellations map
/// on Drop. We use a guard (not a bare `remove` call at every `return`
/// point) so a future refactor that adds a new early-return path
/// can't accidentally leak the entry. The guard is `Send` because
/// it only holds `Arc<Mutex<HashMap<...>>>` clones, which are
/// themselves `Send + Sync`.
///
/// Grill decision #1 (locked): lives in `state.rs` next to
/// `AppState` (the two cancellation maps it touches are also
/// there).
pub struct CancellationGuard {
    pub cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    /// Also clears the `session_active_request` entry for
    /// `session_id`, so a destructive command that looks up
    /// "is this session in-flight?" after the guard drops sees an
    /// empty answer (matches the agent loop's own exit semantics).
    pub session_active_request: Arc<Mutex<HashMap<String, String>>>,
    pub request_id: String,
    pub session_id: String,
    /// B6 Subagent (2026-06-19, review #2 / RULE-E-005): when
    /// `true`, Drop skips the `session_active_request.remove(...)`
    /// step (the `cancellations.remove(...)` still runs). Worker
    /// agents reuse the parent's `session_id` for audit/DB linkage
    /// but their rid must NOT be the session's "active request" —
    /// that slot belongs to the parent chat. Removing it on worker
    /// exit would corrupt the parent's cancel-inflight semantics.
    /// Production chat passes `false` (unchanged behavior); the
    /// B6 worker path (PR1b) passes `true`.
    pub skip_session_active: bool,
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        let cancellations = self.cancellations.clone();
        let session_active_request = self.session_active_request.clone();
        let request_id = self.request_id.clone();
        let session_id = self.session_id.clone();
        let skip_session_active = self.skip_session_active;
        tauri::async_runtime::spawn(async move {
            let mut map = cancellations.lock().await;
            map.remove(&request_id);
            drop(map);
            if !skip_session_active {
                let mut s2p = session_active_request.lock().await;
                s2p.remove(&session_id);
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Event payloads
// ---------------------------------------------------------------------------

/// Event payload for the high-frequency `chat-event` channel
/// (start / delta / thinking_delta / signature_delta /
/// redacted_thinking_delta / done / error).
#[derive(Serialize, Clone)]
pub struct ChatEventPayload {
    pub request_id: String,
    #[serde(flatten)]
    pub event: ChatEvent,
}

/// Event payload for the low-frequency `tool:call` channel.
#[derive(Serialize, Clone)]
pub struct ToolCallPayload {
    pub request_id: String,
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// Event payload for the low-frequency `tool:result` channel.
#[derive(Serialize, Clone)]
pub struct ToolResultPayload {
    pub request_id: String,
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

// ---------------------------------------------------------------------------
// ChatEventSink — abstracted emit surface (P1 RULE-A-006)
//
// The agent loop's only Tauri-side dependency is `AppHandle::emit`
// for three event channels: `chat-event` / `tool:call` / `tool:result`.
// The trait abstracts that surface so the agent loop can run
// against a `MockEmitter` in integration tests, and the production
// `AppHandle` simply implements the same trait (via a one-line
// wrapper struct, see `AppHandleSink`).
//
// The trait is split into 3 non-generic methods (one per channel)
// so it remains dyn-compatible — Rust requires that trait objects
// (`dyn Trait`) do not have generic methods. Each method takes
// the typed payload directly; serde dispatch happens at the
// `AppHandle::emit` boundary in production, or is recorded
// verbatim in `MockEmitter` for tests.
// ---------------------------------------------------------------------------

/// The three Tauri channels the agent loop emits on. A test
/// implementation (e.g. `MockEmitter`) records events into a Vec
/// for assertion; the production `AppHandleSink` forwards to
/// `app.emit(name, payload)` for live IPC dispatch.
///
/// All four methods are exercised in production (P1 RULE-A-006
/// closure, 2026-06-15): `chat_loop::run_chat_loop` dispatches
/// every agent-loop emit through the trait, with the production
/// `AppHandleSink` forwarding to `tauri::AppHandle::emit` for
/// live IPC dispatch. The test variant (`MockEmitter` in
/// `agent/tests.rs`) records events into a Vec for assertion.
pub trait ChatEventSink: Send + Sync + 'static {
    /// Emit a `ChatEvent` on the `chat-event` channel.
    fn emit_chat_event(&self, payload: &ChatEventPayload);
    /// Emit a `ToolCallPayload` on the `tool:call` channel.
    fn emit_tool_call(&self, payload: &ToolCallPayload);
    /// Emit a `ToolResultPayload` on the `tool:result` channel.
    fn emit_tool_result(&self, payload: &ToolResultPayload);
    /// Emit a `PermissionAskPayload` on the `permission:ask`
    /// channel. The ⑨ 关 Tier 3 path uses this to surface the
    /// permission modal prompt to the frontend; the test
    /// `MockEmitter` records it so the test can assert
    /// "Tier 4 was triggered" without a live UI.
    fn emit_permission_ask(&self, payload: PermissionAskPayload);
    /// Emit a `PermissionAskResolved` transcript entry recording
    /// the outcome of a worker's `PermissionAsk`
    /// (RULE-WorkerAsk-001, 2026-06-22). Called by `ask_path`'s
    /// worker branch AFTER its `tokio::select!` returns its
    /// outcome. **Transcript-only** — the default no-op impl is
    /// correct for every sink EXCEPT `SubagentBufferSink`, which
    /// overrides it to record a `TranscriptKind
    /// ::PermissionAskResolved` entry into the worker's
    /// transcript (for historical-replay rendering in the drawer).
    /// `AppHandleSink` and the test `MockEmitter` use the default
    /// no-op (no parent-side audit / IPC for worker resolve).
    fn emit_permission_ask_resolved(&self, _rid: &str, _outcome: &str) {}
    /// Emit a `ToolQuestionPayload` on the `tool:question`
    /// channel (2026-06-30, `ask_user_question` task). Used by
    /// `tools/ask_user_question::execute_blocking` to push the
    /// pending question to the frontend so the user can render
    /// an inline `<AskUserQuestionCard>` and answer or skip.
    /// Default impl is a **silent** no-op (not `warn!` — matches
    /// `emit_permission_ask_resolved`'s default style; a noisy warn
    /// on every call would spam logs if a future sink forgot to
    /// override and the tool somehow fired). Only `AppHandleSink`
    /// (production) and the test `MockEmitter` implement it for real;
    /// `SubagentBufferSink` inherits the no-op — but
    /// `ask_user_question` is structurally disabled for workers (see
    /// `agent::subagent::STRUCTURALLY_DISABLED`), so the worker never
    /// reaches this method; the no-op is "defense in depth" against a
    /// future config that re-enables it.
    fn emit_tool_question(&self, _payload: &crate::agent::question_store::ToolQuestionPayload) {
        // Silent no-op — see the doc comment above.
    }
}

/// Production `AppHandle` adapter. The Tauri trait `Emitter` is in
/// scope; we forward each method to `app.emit(name, payload)`.
pub struct AppHandleSink {
    pub app: AppHandle,
}

impl ChatEventSink for AppHandleSink {
    fn emit_chat_event(&self, payload: &ChatEventPayload) {
        if let Err(e) = self.app.emit("chat-event", payload.clone()) {
            tracing::warn!(error = %e, "AppHandleSink: chat-event emit failed");
        }
    }
    fn emit_tool_call(&self, payload: &ToolCallPayload) {
        if let Err(e) = self.app.emit("tool:call", payload.clone()) {
            tracing::warn!(error = %e, "AppHandleSink: tool:call emit failed");
        }
    }
    fn emit_tool_result(&self, payload: &ToolResultPayload) {
        if let Err(e) = self.app.emit("tool:result", payload.clone()) {
            tracing::warn!(error = %e, "AppHandleSink: tool:result emit failed");
        }
    }
    fn emit_permission_ask(&self, payload: PermissionAskPayload) {
        if let Err(e) = self.app.emit("permission:ask", payload) {
            tracing::warn!(error = %e, "AppHandleSink: permission:ask emit failed");
        }
    }
    fn emit_tool_question(
        &self,
        payload: &crate::agent::question_store::ToolQuestionPayload,
    ) {
        // 2026-06-30 (`ask_user_question` task): production
        // `AppHandleSink` forwards the question payload to the
        // Tauri `tool:question` channel. The frontend
        // `streamController` listens on this channel (Phase C,
        // task `06-30-ask-user-question-tool`) and inserts the
        // payload into `questionCardsStore.pendingBySession`.
        if let Err(e) = self.app.emit("tool:question", payload.clone()) {
            tracing::warn!(error = %e, "AppHandleSink: tool:question emit failed");
        }
    }
}