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
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::llm::{ChatEvent, LlmConfig, Provider, ToolDef};
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
    /// Per-session read fingerprints. The `edit_file` tool consults
    /// this guard to ensure the LLM (a) read the file in the current
    /// session and (b) the file hasn't been modified on disk since.
    /// Lives in process state, not persisted.
    pub read_guard: ReadGuard,
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
        let config = LlmConfig::from_env().unwrap_or_else(|e| {
            tracing::warn!(
                error = %e,
                "ANTHROPIC_API_KEY not set; chat requests will return an auth error"
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

        // Grill decision #3: build the provider catalog. We do this
        // BEFORE the backfill spawn so a backfill panic doesn't
        // leave the catalog half-built.
        let catalog = build_provider_catalog(&db).await;

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
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            session_active_request: Arc::new(Mutex::new(HashMap::new())),
            read_guard: ReadGuard::new(),
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
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        let cancellations = self.cancellations.clone();
        let session_active_request = self.session_active_request.clone();
        let request_id = self.request_id.clone();
        let session_id = self.session_id.clone();
        tauri::async_runtime::spawn(async move {
            let mut map = cancellations.lock().await;
            map.remove(&request_id);
            drop(map);
            let mut s2p = session_active_request.lock().await;
            s2p.remove(&session_id);
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