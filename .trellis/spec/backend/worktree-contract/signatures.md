<!-- Moved from worktree-contract.md 2026-09-19 (doc-split) -->

###2. Signatures

#### New Tauri commands (`app/src-tauri/src/commands/worktree.rs`)

```rust
#[tauri::command]
async fn attach_worktree(
 state: State<'_, Arc<AppState>>,
 session_id: String,
) -> Result<db::SessionRow, String>;

#[tauri::command]
async fn detach_worktree(
 state: State<'_, Arc<AppState>>,
 session_id: String,
) -> Result<db::SessionRow, String>;

#[tauri::command]
async fn delete_worktree(
 state: State<'_, Arc<AppState>>,
 session_id: String,
) -> Result<db::SessionRow, String>;
```

Each is registered in `invoke_handler!` and exposed to the frontend.

#### New DB schema (`app/src-tauri/src/db/sessions.rs`)

```sql
ALTER TABLE sessions ADD COLUMN worktree_state TEXT NOT NULL DEFAULT 'none';
ALTER TABLE sessions ADD COLUMN last_worktree_path TEXT;
-- One-shot backfill on startup (idempotent, see migration helper):
UPDATE sessions
SET worktree_state = 'active'
WHERE worktree_path IS NOT NULL
 AND worktree_state = 'none';
```

Valid `worktree_state` values (snake_case strings; serialized as
`#[serde(rename_all = "snake_case")]`):

| Value | Meaning | `worktree_path` | `last_worktree_path` |
|-------|---------|-----------------|----------------------|
| `"none"` | Never had a worktree, or worktree was deleted. | `NULL` | `NULL` (never used) or last value preserved |
| `"active"` | A worktree is currently bound to this session. | `Some(<path>)` | `NULL` (or previous value preserved) |
| `"detached"` | Had a worktree, now unbound. Directory may still exist on disk. | `NULL` | `Some(<previous path>)` |

#### New helpers

```rust
// app/src-tauri/src/git/worktree.rs
pub fn check_clean(repo_path: &Path) -> Result<(), GitError>;
// Uses libgit2 `Repository::open(repo_path)?.status()?` to detect modified
// tracked files + untracked files. Ignores .gitignore'd files.
// Rejects when status is non-empty.

// app/src-tauri/src/db/sessions.rs
pub async fn set_worktree_state(
 pool: &SqlitePool, session_id: &str, state: WorktreeState,
 last_worktree_path: Option<&str>,
) -> Result<(), sqlx::Error>;

pub async fn insert_system_event(
 pool: &SqlitePool, session_id: &str, text: &str,
) -> Result<(), sqlx::Error>;
// Inserts a row into `messages` with role='user' and content=text.
// seq = max(seq)+1 for the session.

// app/src-tauri/src/agent/helpers.rs
pub fn tool_result_envelope(content: String, worktree_path: &Path) -> String;
// Returns: {"result": "<content>", "cwd": "<worktree_path>"}
// Lives in agent::helpers (NOT in the tool modules) so the existing 60+ tool
// unit tests are unchanged.

pub async fn cancel_inflight_for_session(
 cancellations: &Arc<Mutex<HashMap<String, CancellationToken>>>,
 session_active_request: &Arc<Mutex<HashMap<String, String>>>,
 inflight_exits: &Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>>,
 session_id: &str,
) -> Option<oneshot::Receiver<()>>;
// Cancels the in-flight token for `session_id` (if any) AND returns
// the matching "agent loop exited" signal (RULE-E-005, 2026-06-15).
// Returns None when no in-flight request exists, or when a concurrent
// destructive op already drained the single-consumer receiver. The
// caller passes the result to `await_inflight_exit(rx, label)` before
// doing destructive work.
```

#### New AppState field

```rust
struct AppState {
 // ... existing fields ...
 session_active_request: Arc<Mutex<HashMap<String, String>>>,
 // Maps session_id -> currently active request_id.
 // Inserted by `chat` on spawn; cleared by CancellationGuard on Drop.
 // Read by the 3 destructive paths to find the request_id to cancel.
 inflight_exits: Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>>,
 // RULE-E-005 (2026-06-15): request_id -> "agent loop exited" signal.
 // `chat` inserts the Receiver on spawn; the spawn closure `.send(())`s
 // the paired Sender once `run_chat_loop` returns, then removes the
 // entry. `cancel_inflight_for_session` drains the Receiver (single-
 // consumer) so the destructive caller can `await_inflight_exit` it.
}
```

