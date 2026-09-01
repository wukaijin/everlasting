## Scenario: L1a Background Shell Tools (`run_background_shell` / `shell_status` / `shell_kill`, 2026-06-19)

> **Source of truth**: implementation lives in
> `app/src-tauri/src/tools/{run_background_shell,shell_status,shell_kill}.rs`,
> backed by `app/src-tauri/src/background_shell/{mod.rs,in_memory.rs}`.
> Decisions Q1-Q7 live in `.trellis/tasks/06-19-l1-shell-pty/prd.md`.
> Follow-up: L1b (real PTY via `portable-pty`) and L3 (parallel
> subagent + worktree isolation) are separate tasks.

### 1. Scope / Trigger

The synchronous `shell` tool's 600-second cap is insufficient for
"fire-and-forget" workloads: full builds, package installs, large
test suites, dev servers. L1a adds a sibling toolset so the LLM
can start a long-running command, return a handle immediately,
and react to its completion when the next agent-loop turn begins.
Same `sh -c <command>` execution model, same env-allowlist
(RULE-E-001), same process-group SIGKILL (RULE-E-002), same
30 KB disk-spill threshold — but the lifetime crosses both the
`execute_tool` call AND the `invoke("chat")` call.

### 2. Signatures

#### Tool declarations (appended to `builtin_tools()`)

```rust
// app/src-tauri/src/tools/run_background_shell.rs
ToolDef {
  name: "run_background_shell",
  description: "Start a shell command in the background and return immediately \
                with a `shell_session_id` handle. Use this for long-running commands \
                (full builds, package installs, large test suites, dev servers) that \
                exceed the synchronous `shell` tool's 600-second timeout cap...",
  input_schema: json!({
    "type": "object",
    "properties": {
      "command": { "type": "string" },
      "working_directory": { "type": "string" },
      "max_runtime_ms": { "type": "integer" }
    },
    "required": ["command"]
  })
}

// app/src-tauri/src/tools/shell_status.rs
ToolDef { name: "shell_status", input_schema: { session_id: string } }

// app/src-tauri/src/tools/shell_kill.rs
ToolDef { name: "shell_kill", input_schema: { session_id: string } }
```

#### `BackgroundShellRegistry` trait + impl

```rust
// app/src-tauri/src/background_shell/mod.rs
#[async_trait-free — native async fn in trait]
pub trait BackgroundShellRegistry: Send + Sync {
    async fn start(&self, session_id, command, cwd, max_runtime_ms) -> Result<String, BackgroundShellError>;
    async fn status(&self, session_id, shell_session_id) -> Result<BackgroundShellStatus, BackgroundShellError>;
    async fn kill(&self, session_id, shell_session_id) -> Result<(), BackgroundShellError>;
    async fn kill_all_for_session(&self, session_id) -> Result<(), BackgroundShellError>;
    async fn drain_notifications(&self, session_id) -> Vec<BackgroundShellNotification>;
    async fn kill_all(&self) -> Result<(), BackgroundShellError>;
}

pub type DefaultRegistry = Arc<InMemoryBackgroundShellRegistry>;
pub fn default_registry() -> DefaultRegistry { ... }
```

#### `ToolContext` extension (PR2 additive change)

```rust
// app/src-tauri/src/tools/mod.rs
#[derive(Clone)]
pub struct ToolContext {
    pub worktree_path: PathBuf,
    pub cwd: PathBuf,
    pub checklist: ChecklistHandle,
    // NEW (L1a 2026-06-19):
    pub background_shells: DefaultRegistry,
}
```

The registry is a concrete `Arc<InMemoryBackgroundShellRegistry>`
(not `dyn BackgroundShellRegistry`) — matches the codebase's
pattern for the other cross-request handles (`MemoryCache`,
`SkillCache`, `ReadGuard`). Future daemon-ization swaps the
concrete type at the `AppState::load` site only.

### 3. Contracts

#### Background shell session_id format

- `shell_session_id` is `bsh_<uuid-simple>` (32 hex chars, no
  dashes). Stable shape — the LLM and the frontend regex-match on
  the `bsh_` prefix.
- The format is distinct from the chat session id (a plain UUID
  with dashes), so the two namespaces never collide.

#### Wire name collision: `session_id` in the LLM-facing schema

Per the L1 PRD, the `shell_status` / `shell_kill` input schemas
expose a field named `session_id`. Internally this is the
*background shell's* id, NOT the chat session id. The tool layer
translates using `ctx.background_shells` (chat-session-scoped via
the registry's `(session_id, shell_session_id)` key) + the chat
`session_id` argument always supplied by `execute_tool_inner`'s
dispatch. The rename is intentional: the LLM doesn't need to
track two distinct "session" concepts — the background shell is
just "the background shell you started".

#### `run_background_shell` execution flow

1. Parse `command` (required) + optional `working_directory` +
   optional `max_runtime_ms`.
2. Resolve the effective cwd via
   `projects::boundary::assert_within_root(&ctx.worktree_path, requested)`
   (mirrors `shell::execute`). Failure → `is_error: true` +
   plain-English rejection message so the LLM self-corrects.
3. Pull `DefaultRegistry` from `ctx.background_shells`.
4. `registry.start(chat_session_id, command, validated_cwd,
   max_runtime_ms)` returns the new `shell_session_id`.
5. Format confirmation string: `Started background shell
   bsh_xxx (cwd: <path>). Use shell_status to query progress, or
   shell_kill to terminate. When it finishes, you will see a
   \`[system] 后台 shell ... 已完成...\` message at the start of
   your next turn.`
6. `ToolContextUpdate::new_cwd = Some(validated_cwd)` so the
   agent loop persists the cwd at turn end (same UX as
   `shell::execute`).

#### Completion notification injection (the load-bearing part)

In `agent::chat_loop::run_chat_loop`, each turn AFTER C3
compaction and BEFORE `provider.send`:

```rust
let background_notifications =
    background_shells.drain_notifications(&session_id).await;
let turn_messages = {
    let checklist_snapshot = ...;
    let mut req = messages.clone();
    // ... checklist injection (unchanged) ...
    // APPEND each notification as ONE user message:
    for note in &background_notifications {
        let text = format!(
            "[system] 后台 shell {} 已完成,exit code {}。调 shell_status(session_id=\"{}\") 看输出。",
            note.shell_session_id,
            note.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "N/A".into()),
            note.shell_session_id,
        );
        req.push(ChatMessage {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::Text { text, cache_control: None }]),
        });
    }
    req
};
```

- **APPEND, never prepend** — the memory breakpoint at
  `messages[0]` carries a `cache_control: Ephemeral` block; a
  per-turn-mutating notification at position 0 would bust the
  memory cache every turn (50 turns × ~100 KiB of instruction
  files = the exact cost explosion the B5 memory-caching work
  was built to eliminate). Same rule as the B12 checklist
  injection (see §"update_checklist" Scenario §7).
- **No `cache_control`** on the notification block — the block
  changes every turn, so a cache breakpoint would never hit.
- **One message per notification** — the LLM tracks multiple
  completions more reliably when they're separated.
- **Drain once per turn, not per tool_use** — the queue is
  consumed at the start of every turn; drained notifications are
  GONE from the registry.
- **Empty queue → skip injection** — fast path, no extra
  `.clone()`, no extra `push()`. The `notification count == 0`
  branch is the common case (no background shells active).
- **P3d (2026-09-01): the drain site resolves escalation offers
  BEFORE any text is assembled** — a sandbox-originated shell that
  failed on an out-of-face denial carries
  `notification.escalation = Some(EscalationOffer)`; the chat loop
  (`agent/chat_loop/background_escalation.rs::resolve_all`) runs the
  foreground §5.2 machinery (prefix-grant → Ask card attached to the
  ORIGINAL run_background_shell call via `offer.tool_use_id` →
  one-shot unsandboxed rerun via `registry.start(sandbox=None,
  origin=None)`) and renders exactly ONE terminal text per
  notification. Notifications without an offer render the legacy
  format byte-identically. Plan-mode turns never escalate. Full
  contract: `.trellis/spec/backend/sandbox-executor.md` §11.

#### Permission routing (⑨ 关 Tier 4)

- `run_background_shell` → `ToolKind::Shell` (same routing as
  `shell`). The Tier 4 shell branch (kill-list + 3-tier
  `classify_prefix` + prefix grants) applies uniformly. A user's
  "始终允许" grant on `cargo` works for BOTH sync `shell` and
  async `run_background_shell`.
- `shell_status` / `shell_kill` → `ToolKind::Other` (Tier 5
  default allow). No `permission:ask` modal. Cross-session
  access still rejected by the registry's session-scoping
  (`(chat_session_id, shell_session_id)` key).
- `run_background_shell` is `Risk::High` (matches `shell`);
  `shell_status` / `shell_kill` are `Risk::Low` (read-only /
  kill-only, no new code executed).
- Plan mode filter drops `run_background_shell` along with
  `shell` / `write_file` / `edit_file` (read-only session).

#### Compound command classification (A2+ P1+P2, 2026-07-04)

The Tier 4 shell branch classifies the WHOLE command, not just the
first token — a user's grant on `ls` must NOT auto-allow
`ls; rm -rf ~/notes`. Four gates (all in `shell_trust.rs` /
`check.rs`, applied uniformly to sync `shell` and async
`run_background_shell`):

1. **Grant short-circuit gate (P1, R1)** — the (a) prefix-grant
   short-circuit AND the worker per-run prefix-grant short-circuit
   are wrapped together in `if !has_structural_metachar(cmd)`. A
   command containing `|` / `&&` / `;` does NOT enjoy either
   short-circuit; it falls through to classification. The gate is
   deliberately NOT quote-aware (false positive is safe: the grant
   is skipped, then `classify_prefix` re-splits accurately and
   produces the right tier); a single-segment `ls` still matches
   the grant and Allow.
2. **Compound split + take-max (P2, R2/R4)** — `classify_prefix`
   splits on top-level `;` / `&&` / `||` / `|` (quoting/escaping-
   aware 4-state machine) and classifies each segment via
   `classify_single`; the result is the most-dangerous tier
   (`Ask > SideEffect > ReadOnly`). `git diff | head` → ReadOnly
   (R2 win); `ls; rm x` → max(ReadOnly, Ask) = Ask.
3. **Write-redirect bump (P1, R3)** — a segment containing a write
   redirection (`>` / `>>` / `&>` / `[N]>file`) bumps to at least
   SideEffect. fd duplication (`2>&1` / `>&N`) and input redirection
   (`<` / `<<` / `<<<`) do NOT bump. `git diff > patch.txt` →
   SideEffect (was ReadOnly — the silent Plan-mode write hole).
4. **Command substitution (R4 fail-safe)** — `$()` or backtick
   anywhere → the WHOLE command is `Ask` (NOT quote-aware; `echo
   $(rm x)` deletes a file at shell-expand time, the outer `echo`
   cannot widen it).

Invariants unchanged: Tier 2 kill-list, Yolo bypass, Mode three-
tier semantics, grant schema (`tool`/`prefix`/`path`), 17
AuditKind, `shell.rs` execution hardening, "when in doubt, Ask".
See `agent/permissions/shell_trust.rs` module docs for the
classification algorithm; ADR in `docs/IMPLEMENTATION.md §4`
(2026-07-04).

#### Lifecycle hooks (3 trigger points)

| Hook | Where | Trigger |
|---|---|---|
| `kill_all_for_session(session_id)` | `commands/sessions.rs::delete_session` | User deletes the chat session; every bg shell under that session is SIGKILLed |
| `kill_all()` | `lib.rs::run`'s `RunEvent::Exit` closure | App shutdown; every bg shell across every session is SIGKILLed |
| `kill(session_id, shell_session_id)` | `shell_kill::execute` | LLM-driven kill; SIGKILLs one shell |

All three use the registry's process-group SIGKILL (RULE-E-002)
so descendants of `&` / `nohup` / pipelines are reaped along
with the direct child.

### 4. Validation & Error Matrix

| Condition | Result |
|---|---|
| `command` missing | `is_error: true`, "Missing required parameter: command" |
| `working_directory` outside project root | `is_error: true`, "working_directory '<path>' rejected: <reason>" |
| `max_runtime_ms <= 0` | Default 86_400_000 (24h) |
| `max_runtime_ms > 600_000` | No upper clamp (PRD Q6) |
| `registry.start` spawn failure (ENOENT / EACCES) | `is_error: true`, "Failed to spawn background shell: <io error>"; `SpawnFailed` notification still queued |
| `shell_status` / `shell_kill` on unknown shell | `is_error: true`, "Background shell <id> not found" |
| `shell_status` / `shell_kill` cross-session access | `is_error: true`, "Background shell <id> is not owned by this chat session" |
| `shell_kill` on already-completed shell | `Ok(())` — idempotent no-op |
| `notification_queue` overflows past 100 entries | Oldest entry dropped + `tracing::warn!` (no panic) |
| `delete_session` with running bg shells | `kill_all_for_session` called; SIGKILLs sent synchronously, teardown async (does not block IPC) |
| App `RunEvent::Exit` with running bg shells | `kill_all` called; SIGKILLs sent before OS process reaping |
| `agent_loop` empty notification queue | Skip injection (no extra `.clone()`, no extra `push()`) |

### 5. Good / Base / Bad Cases

#### Good: build → status → fix

1. LLM calls `run_background_shell({"command": "pnpm install"})`.
2. Tool returns `Started background shell bsh_a (cwd: /repo)`.
3. LLM's turn ends (`Done{end_turn}`); agent loop persists the
   `tool_result`.
4. ~30s later, the bg shell exits. The spawned task writes
   `BackgroundShellNotification { outcome: Completed, exit_code: 0 }`
   and pushes to the per-session queue.
5. LLM sends a new message. Agent loop starts; C3 compaction
   (passthrough); `drain_notifications` returns the 1 entry.
6. Turn 1 request body: user message appended with
   `[system] 后台 shell bsh_a 已完成,exit code 0。调 shell_status(session_id="bsh_a") 看输出。`
7. LLM sees the notification, calls `shell_status`, reads the
   stdout/stderr preview + optional disk-spill path, and decides
   the next action.

#### Bad: completion arrives during turn 1

1. LLM calls `run_background_shell({"command": "echo fast"})`.
2. Tool returns immediately with `bsh_a` (echo completes in
   <50ms, but the spawned task hasn't pushed the notification
   yet).
3. Agent loop builds the `tool_result` user message and calls
   `provider.send` for turn 1.
4. The notification is queued AFTER turn 1's drain, so turn 2's
   drain picks it up. Correct behavior — turn 1's `provider.send`
   has already gone out by the time the notification lands.

#### Bad: prepend instead of append (cache invariant)

```rust
// BAD — would bust the memory cache breakpoint at messages[0]
let mut req = messages.clone();
req.insert(0, ChatMessage {
    role: Role::User,
    content: MessageContent::Blocks(vec![ContentBlock::Text { text: notification }]),
});
```

The memory breakpoint at `messages[0]` is part of Anthropic's
cache key (everything before it must be byte-identical across
turns). A per-turn-changing notification at position 0 sits
IN FRONT of the breakpoint and busts the memory cache every
turn. The fix lives in `chat_loop.rs` — the injection is
`req.push(notification_msg)` AFTER the checklist block, not
`req.insert(0, ...)`.

### 6. Tests Required

#### Backend (`cargo test --lib`)

| Test | Asserts |
|---|---|
| `tools::run_background_shell::tests::definition_has_correct_name` | Tool name |
| `tools::run_background_shell::tests::definition_documents_max_runtime` | `max_runtime_ms` field present + schema `required` is `["command"]` |
| `tools::run_background_shell::tests::execute_starts_a_background_shell` | Happy path; returns shell_id; carries cwd in `ToolContextUpdate` |
| `tools::run_background_shell::tests::execute_missing_command_returns_error` | `is_error: true` + "Missing required parameter" |
| `tools::run_background_shell::tests::execute_rejects_outside_root_cwd` | Boundary rejection |
| `tools::run_background_shell::tests::execute_without_session_id_returns_bug_marker` | Defensive no-session-id path |
| `tools::run_background_shell::tests::returned_shell_id_is_queryable_via_registry` | Round-trip: started shell is `status`-able via the same registry |
| `tools::shell_status::tests::definition_has_correct_name` / `definition_schema_requires_session_id` | Name + required |
| `tools::shell_status::tests::execute_missing_session_id_returns_error` / `execute_unknown_shell_returns_not_found_error` / `execute_without_chat_session_id_returns_bug_marker` | Error paths |
| `tools::shell_status::tests::execute_round_trip_returns_running_or_completed` | Round-trip |
| `tools::shell_status::tests::execute_cross_session_returns_error` | Q7 session-scoping |
| `tools::shell_status::tests::format_running_status_includes_id_and_elapsed` / `format_completed_status_includes_stdout_preview_and_path` / `format_killed_status_includes_exit_code` | Status string format |
| `tools::shell_kill::tests::definition_has_correct_name` / `definition_schema_requires_session_id` | Name + required |
| `tools::shell_kill::tests::execute_missing_session_id_returns_error` / `execute_unknown_shell_returns_not_found_error` / `execute_without_chat_session_id_returns_bug_marker` | Error paths |
| `tools::shell_kill::tests::execute_round_trip_kills_running_shell` | Round-trip + state transition to Killed |
| `tools::shell_kill::tests::execute_on_completed_shell_is_idempotent` | Idempotency |
| `tools::shell_kill::tests::execute_cross_session_returns_error_and_does_not_kill` | Q7 + side-effect (cross-session call leaves the shell running) |
| `permissions::tests::risk_for_tool_includes_background_shell_high` | `Risk::High` for `run_background_shell`; `Risk::Low` for `shell_status` / `shell_kill` |
| `permissions::tests::classify_tool_routes_background_shell_to_shell_kind` | Tier 4 shell branch applies uniformly |
| `agent::tests::agent_loop_drains_background_shell_notification_into_turn_2` | End-to-end: start → complete → drain → inject on next turn → provider.send carries the text |
| `agent::tests::agent_loop_no_pending_notifications_skips_injection` | Empty-queue fast path (no extra allocations, no extra push) |

Total L1a PR2 contribution: **+22 net new tests** (the 3 tool
modules' unit tests + 2 permission tests + 2 agent-loop
integration tests). Backend suite went from 651 → 680 = **+29
net new** when combined with the PR1 contributions that landed
in this PR (the additional test helpers / dispatch fixtures).

### 7. Wrong vs Correct — registry handle vs trait object

#### Wrong: `Arc<dyn BackgroundShellRegistry>` on `ToolContext`

```rust
// BAD — would force every tool to spell out the trait on the
// dispatch site and add a layer of dynamic dispatch the
// codebase doesn't use elsewhere.
pub struct ToolContext {
    pub background_shells: Arc<dyn BackgroundShellRegistry>,
}
```

The codebase's pattern for cross-request handles is concrete
`Arc<InMemory*>` (or `Arc<MemoryCache>`, `Arc<SkillCache>`,
`ReadGuard`). The future daemon-ization swap is a one-line
change at the `AppState::load` site (per L1 PRD Q1 decision C)
— no call-site churn, no `dyn`-dispatch overhead in the hot
path (every `execute_tool` call would pay for one vtable
indirection per method invocation).

#### Correct: concrete `Arc<InMemoryBackgroundShellRegistry>`

```rust
// GOOD — matches `MemoryCache::arc()` / `SkillCache::arc()` /
// `ReadGuard::new()` pattern. Daemon-ization PR swaps the
// concrete type at the `AppState::load` site only.
pub type DefaultRegistry = Arc<InMemoryBackgroundShellRegistry>;
pub fn default_registry() -> DefaultRegistry {
    Arc::new(InMemoryBackgroundShellRegistry::new())
}
```

The trait still exists as the type-erased interface the future
daemon impl will satisfy, but the production wiring holds the
concrete impl — exactly the pattern every other cross-request
handle in `AppState` uses.
