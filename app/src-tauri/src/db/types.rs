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

// ---------------------------------------------------------------------------
// A2 + B7: per-session Mode (2026-06-13, 3-档化 2026-06-13)
//
// Each session carries a `mode` (one of `edit` / `plan` / `yolo`)
// that drives both the agent loop's ⑨ 关 permission policy
// and the ⑧a Mode check (tool list filtering + system prompt prefix
// + runtime intercept). Background is reserved in the enum so a
// future PR can add it without a schema change; the MVP UI does
// not expose it.
//
// 3 档化决策 (2026-06-13 ADR-lite in IMPLEMENTATION.md §4):
// - Chat 改名 Edit (语义更清晰 — "I want edits to happen")
// - Review 删除 (行为与 Plan 重复, system prompt 强调的"只读分析"价值不大)
// - Yolo 保留
// - Breaking rename: 'chat'/'review' wire 字符串直接移除, 不保留 alias
//   (单机 desktop app, 无跨版本兼容需求)
//
// `from_str_opt` follows the same lenient-parse pattern as
// `WorktreeState`: unknown / empty DB strings fall back to `Edit`
// (the most permissive policy). The `Serialize` form is the
// lowercase string (matches the DB column).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
 Edit,
 Plan,
 Yolo,
 /// Reserved for a future PR (per BACKLOG §4.2). MVP UI does
 /// not expose this; the enum position keeps the schema stable
 /// if Background is ever promoted.
 #[allow(dead_code)]
 Background,
}

impl Mode {
 pub fn as_str(&self) -> &'static str {
 match self {
 Mode::Edit => "edit",
 Mode::Plan => "plan",
 Mode::Yolo => "yolo",
 Mode::Background => "background",
 }
 }

 /// Lenient parse: unknown / empty strings fall back to `Edit`
 /// (the most permissive policy, matches the migration backfill).
 /// Old 'chat' / 'review' strings are NOT aliased — they fall
 /// through to `Edit` (and a startup backfill rewrites them in
 /// the DB on first run).
 pub fn from_str_opt(s: &str) -> Self {
 match s {
 "plan" => Mode::Plan,
 "yolo" => Mode::Yolo,
 "background" => Mode::Background,
 _ => Mode::Edit,
 }
 }
}

// ---------------------------------------------------------------------------
// Row types (Serialize for Tauri IPC payload)
// ---------------------------------------------------------------------------

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
 pub worktree_path: Option<String>,
 pub worktree_state: WorktreeState,
 pub last_worktree_path: Option<String>,
 pub model_id: Option<String>,
 pub input_tokens_total: Option<i64>,
 pub output_tokens_total: Option<i64>,
 pub cache_creation_total: Option<i64>,
 pub cache_read_total: Option<i64>,
 /// D1 (Color Tag): palette index 0-7, NULL = no mark.
 pub color_tag: Option<i32>,
 /// A2 + B7 (Permission system + per-session Mode, 2026-06-13):
 /// the session's current mode. Defaults to `Chat` for legacy
 /// sessions via the migration backfill (see
 /// `db::migrations::run_migrations`). Persisted on every
 /// `set_session_mode` IPC call.
 pub mode: Mode,
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
 pub worktree_path: Option<String>,
 pub worktree_state: WorktreeState,
 pub last_worktree_path: Option<String>,
 pub model_id: Option<String>,
 pub input_tokens_total: Option<i64>,
 pub output_tokens_total: Option<i64>,
 pub cache_creation_total: Option<i64>,
 pub cache_read_total: Option<i64>,
 /// D1 (Color Tag): palette index 0-7, NULL = no mark.
 pub color_tag: Option<i32>,
 /// A2 + B7: per-session Mode (see `SessionRow::mode`). The
 /// sidebar uses this for the mode badge / chip without a
 /// per-session IPC round-trip.
 pub mode: Mode,
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
 /// F5 (LLM Latency Tracking): per-message latency breakdown.
 /// All three fields are `null` for pre-F5 rows (the columns
 /// are nullable; a legacy message's first post-upgrade turn
 /// will have them set by the new `update_message_latency` IPC
 /// fired at `done`). The UI reads these to render the
 /// assistant bubble's bottom-right total + hover tooltip
 /// (TTFB / 生成 / 端到端 breakdown). See
 /// `.trellis/spec/backend/llm-contract.md` "Scenario: Latency
 /// Tracking" for the field semantics.
 pub ttfb_ms: Option<i64>,
 pub gen_ms: Option<i64>,
 pub total_ms: Option<i64>,
 /// F5 follow-up: thinking-phase wall-clock duration (ms).
 /// First `thinking_delta` → first non-thinking boundary
 /// (text `delta`, `tool:call`, `done`, or `error`).
 /// `None` for messages that never entered the thinking
 /// phase. Drives the "Thought for X.Xs" header in
 /// `ThinkingBlock.vue` (replaces the previous "X tokens"
 /// estimate). Persisted by the `update_message_thinking`
 /// IPC fired at stream end, same shape as
 /// `update_message_latency`.
 pub thinking_ms: Option<i64>,
}

/// Result of `load_session` — session meta + all messages ordered by `seq`.
#[derive(Debug, Clone, Serialize)]
pub struct LoadedSession {
 pub session: SessionRow,
 pub messages: Vec<MessageRow>,
}
