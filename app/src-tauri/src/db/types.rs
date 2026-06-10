//! Row types and enums shared across the [`db`](super) module.
//!
//! Everything here is a pure data container / enum with no DB I/O. CRUD
//! functions live in the per-domain submodules (`projects` /
//! `sessions` / `providers` / `models` / `config`) and re-import
//! these types as needed. `db/mod.rs` re-exports them so callers can
//! keep using the `db::FooRow` / `db::WorktreeState` paths from
//! before the split.

use serde::{Deserialize, Serialize, Serializer};

impl Serialize for WorktreeState {
 fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
 s.serialize_str(self.as_str())
 }
}

// ---------------------------------------------------------------------------
// Provider / Model row types (PR1 of multi-model task)
// ---------------------------------------------------------------------------

/// LLM provider protocol. Maps to the wire format the LLM client
/// speaks. PR1 ships `Anthropic` (Messages API) and `Openai` (Chat
/// Completions); future protocols (Ollama, Gemini, …) extend this
/// enum in step with `Provider` impls added under
/// `app/src-tauri/src/llm/provider/`.
///
/// The enum + methods are intentionally unused in PR1 (PR1 only
/// persists `protocol` as a TEXT column); the dispatch in PR2's
/// `Provider` impls will pick them up. `#[allow(dead_code)]` keeps
/// the lib build clean in the meantime.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderProtocol {
 Anthropic,
 Openai,
}

#[allow(dead_code)]
impl ProviderProtocol {
 pub fn as_str(&self) -> &'static str {
 match self {
 Self::Anthropic => "anthropic",
 Self::Openai => "openai",
 }
 }

 /// Lenient parse for DB values. Unknown values fall back to
 /// `Anthropic` so a future schema migration that adds a new
 /// protocol doesn't crash an older binary reading a newer DB.
 pub fn from_str_opt(s: &str) -> Self {
 match s {
 "openai" => Self::Openai,
 _ => Self::Anthropic,
 }
 }
}

/// A user-managed LLM provider entry. Multiple rows may share the
/// same `protocol` (e.g. "Anthropic官方" + "wukaijin转发" both
/// `protocol=anthropic`); the `display_name` is the user-facing
/// label that disambiguates them in the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRow {
 pub id: String,
 pub protocol: String,
 pub display_name: String,
 pub base_url: String,
 pub api_key: String,
 pub created_at: String,
 pub updated_at: String,
}

/// A user-managed LLM model entry. Always bound to one
/// `ProviderRow` via `provider_id` (FK with `ON DELETE CASCADE`).
/// Optional fields (`max_tokens`, `thinking_effort`) override the
/// global env defaults; `None` means "fall back to global".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRow {
 pub id: String,
 pub provider_id: String,
 pub model_name: String,
 pub display_name: String,
 pub max_tokens: Option<u32>,
 pub thinking_effort: Option<String>,
 pub supports_thinking: bool,
 pub context_window: u32,
 pub created_at: String,
 pub updated_at: String,
}

/// `ModelRow` denormalized with the parent provider's `display_name`
/// + `protocol`. The UI renders this view directly (model picker
/// groups models under their provider's display name) so the
/// frontend does not need a second IPC roundtrip to render the
/// dropdown.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelWithProvider {
 #[serde(flatten)]
 pub model: ModelRow,
 pub provider_display_name: String,
 pub provider_protocol: String,
}

// ---------------------------------------------------------------------------
// Row types (Serialize for Tauri IPC payload)
// ---------------------------------------------------------------------------

/// Possible worktree states for a session. The state machine is
/// tri-valued:
///
/// - `None` (DB value `"none"`): the session was never attached
/// to a worktree. `worktree_path` is NULL.
/// - `Active` (DB value `"active"`): a worktree is currently
/// bound to this session. `worktree_path` is non-NULL.
/// - `Detached` (DB value `"detached"`): a worktree WAS attached
/// at some point, but the user has since unbound it. The
/// directory + branch are preserved on disk and
/// `last_worktree_path` records the path that was unbound (for
/// the "上次 worktree" UI affordance — a detached session still
/// has a branch on disk, and the user may want to re-attach to
/// it).
///
/// Migration: a session that was created under step4 (auto-create
/// flow, before the opt-in refactor) has `worktree_path IS NOT
/// NULL` but `worktree_state IS NULL`. `run_migrations` backfills
/// these to `"active"` so the UI behaves the same as a freshly
/// attached session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeState {
 None,
 Active,
 Detached,
}

impl WorktreeState {
 pub fn as_str(&self) -> &'static str {
 match self {
 WorktreeState::None => "none",
 WorktreeState::Active => "active",
 WorktreeState::Detached => "detached",
 }
 }

 /// Lenient parse for DB values. Unknown values are treated as
 /// `None` so a future schema migration that adds a new state
 /// doesn't crash an older binary reading a newer DB.
 pub fn from_str_opt(s: &str) -> Self {
 match s {
 "active" => WorktreeState::Active,
 "detached" => WorktreeState::Detached,
 _ => WorktreeState::None,
 }
 }
}

/// A session as stored in the DB.
#[derive(Debug, Clone, Serialize)]
pub struct SessionRow {
 pub id: String,
 pub title: String,
 pub created_at: String,
 pub updated_at: String,
 pub model: String,
 pub project_id: String,
 pub current_cwd: String,
 /// On-disk path to the session's git worktree. `None` for
 /// sessions that have never been attached (state `none`) or
 /// have been detached (state `detached` — see
 /// `last_worktree_path` for the historical path). Tools fall
 /// back to `current_cwd` when this is `None`.
 pub worktree_path: Option<String>,
 /// Current worktree state (see [`WorktreeState`]).
 pub worktree_state: WorktreeState,
 /// Path of the most recently detached worktree. `None` unless
 /// the session has been in `active` state at some point.
 /// Preserved across detach so the UI can show a "上次 worktree"
 /// chip that lets the user re-attach or inspect the branch.
 pub last_worktree_path: Option<String>,
 /// PR4 of multi-model: per-session model override. `None` when
 /// the session uses the global default model (the chat command's
 /// `resolve_chat_provider` falls back to `app_config.default_model_id`
 /// when this is NULL or the referenced model was deleted). This is a
 /// soft FK to `models.id` — no `REFERENCES` constraint so legacy rows
 /// and dangling references don't break INSERTs.
 pub model_id: Option<String>,
 /// A4 (Token Usage Tracking): per-session cumulative token
 /// usage. All four are `None` for pre-A4 sessions (the
 /// migration is non-destructive and the columns are
 /// nullable). The frontend renders `None` as "—" with the
 /// "升级前未统计" tooltip; the first LLM turn after the
 /// upgrade starts the counters from 0 and they tick up
 /// per turn. See
 /// `.trellis/spec/backend/llm-contract.md` "Scenario: Token
 /// Usage Tracking" for the schema.
 pub input_tokens_total: Option<i64>,
 pub output_tokens_total: Option<i64>,
 pub cache_creation_total: Option<i64>,
 pub cache_read_total: Option<i64>,
}

/// Summary used by `list_sessions` — includes a preview of the most recent
/// user message so the sidebar can show context without re-loading.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
 pub id: String,
 pub title: String,
 pub updated_at: String,
 pub preview: String,
 pub project_id: String,
 pub current_cwd: String,
 /// Mirror of [`SessionRow::worktree_path`]. `None` for sessions
 /// in `none` or `detached` state.
 pub worktree_path: Option<String>,
 /// Mirror of [`SessionRow::worktree_state`].
 pub worktree_state: WorktreeState,
 /// Mirror of [`SessionRow::last_worktree_path`].
 pub last_worktree_path: Option<String>,
 /// PR4 of multi-model: per-session model override. `None` when the
 /// session uses the global default model. Soft FK to `models.id`.
 pub model_id: Option<String>,
 /// A4: per-session cumulative token usage. Mirrors
 /// [`SessionRow`]. The SessionList component reads this to
 /// render the optional "X% / 200K" chip on each session
 /// card. (Today the chat-input hint is the canonical place
 /// to see the number, but the SessionList summary carries
 /// the data so a future PR can wire up a session-card
 /// indicator without re-loading.)
 pub input_tokens_total: Option<i64>,
 pub output_tokens_total: Option<i64>,
 pub cache_creation_total: Option<i64>,
 pub cache_read_total: Option<i64>,
}

/// A message as stored in the DB. `content` is JSON (`Vec<ContentBlock>`).
#[derive(Debug, Clone, Serialize)]
pub struct MessageRow {
 pub id: i64,
 pub session_id: String,
 pub role: String,
 pub content: serde_json::Value,
 pub text: String,
 pub has_tool_calls: bool,
 pub has_tool_results: bool,
 pub created_at: String,
 pub seq: i64,
 /// Optional structured metadata. `None` for chat history rows;
 /// `Some(json)` for system events injected by the worktree
 /// commands. Used by rehydrate to filter or specially render.
 pub metadata: Option<serde_json::Value>,
}

/// Result of `load_session` — session meta + all messages ordered by `seq`.
#[derive(Debug, Clone, Serialize)]
pub struct LoadedSession {
 pub session: SessionRow,
 pub messages: Vec<MessageRow>,
}
