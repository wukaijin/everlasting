## Scenario: update_checklist tool (B12, 2026-06-19)

### 1. Scope / Trigger

- Trigger: B12 Checklist — the agent's self-tracking progress list. The model calls
  `update_checklist(items)` to atomically replace a per-request checklist held in the
  agent loop's run scope; the loop re-injects the current list into each turn so the
  model doesn't lose its plan. Aligns with Claude Code `TaskCreate/TaskList` / opencode
  `todowrite`. Paves the "inject dynamic agent-state each turn" mechanism that B6
  Subagent will reuse.
- Why code-spec depth: mandatory — this is the first tool whose state is **agent-managed
  and re-injected each turn** (not a plain request/response tool), and the injection
  placement carries a load-bearing prompt-cache invariant. The tool signature, the coerce
  contract, and the append-not-prepend injection rule are all executable contracts.

### 2. Signatures

#### Tool declaration (appended to `builtin_tools()`)

```rust
// app/src-tauri/src/tools/update_checklist.rs
ToolDef {
  name: "update_checklist",
  description: "Update your running progress checklist ... Pass the FULL list every
                call — the new list replaces the old one atomically (not append) ...
                At most one item should be in_progress at a time; if you pass multiple,
                only the last is kept as in_progress and the rest are demoted to pending ...",
  input_schema: json!({
    "type": "object",
    "properties": {
      "items": {
        "type": "array",
        "items": {
          "type": "object",
          "properties": {
            "content": { "type": "string" },
            "status":  { "type": "string", "enum": ["pending", "in_progress", "done"] }
          },
          "required": ["content", "status"]
        }
      }
    },
    "required": ["items"]
  })
}
```

#### Types + handle

```rust
#[serde(rename_all = "snake_case")]
pub enum ChecklistStatus { Pending, InProgress, Done }   // wire: "pending"/"in_progress"/"done"
pub struct ChecklistItem { pub content: String, pub status: ChecklistStatus }

/// Per-request handle. NOT a run_chat_loop parameter — it lives inside ToolContext
/// (built once per run_chat_loop call ~chat_loop.rs:216), so run_chat_loop's 23-param
/// signature is UNCHANGED by the checklist addition (no agent_loop_* test call-site
/// sync needed; the B6 cluster + the 2026-06-21 B6 review defect A fix raised it from
/// 21→23 for `app_handle` + `system_prompt_override` respectively).
pub type ChecklistHandle = Arc<Mutex<Vec<ChecklistItem>>>;
pub fn new_handle() -> ChecklistHandle;                  // fresh empty, once per run
```

#### Execute (mutates the handle, returns tool_result)

```rust
pub async fn execute(input: &serde_json::Value, handle: &ChecklistHandle)
    -> (String, /* is_error */ bool);   // always (summary + rendered list, false)
```

`execute` parses → coerces → atomically replaces the Vec (`clear()` + `extend()`, lock
held only for the swap, no I/O in the critical section) → returns
`"Checklist updated (N items, M done, K in_progress).\n\n<rendered list>"`.

### 3. Contracts

- **Full replace** (not append). Replay requires "last `update_checklist` tool_result ==
  current state", so the input array replaces the Vec in full.
- **At-most-one `in_progress` coerce** (`coerce_at_most_one_in_progress`, pure fn): keep
  the LAST `in_progress` by array order (`rposition`), demote any earlier to `pending`.
  Does NOT error, does NOT abort the loop.
- **`execute` never returns `is_error: true`** — pure in-memory state mutation, no failure mode.
- **Plan mode**: auto-allowed. `permissions::filter_tools_for_mode` drops only `write_file` /
  `edit_file` / `shell`; `update_checklist` passes. `risk_for_tool` → `Risk::Low` (`_` default).
- **Lifetime**: per-request. `new_handle()` is constructed inside each `run_chat_loop`
  invocation; new user message → new run → new empty checklist. **No DB table, no
  migration** — replay reconstructs from `update_checklist` tool_results already in history.

#### Ephemeral per-turn injection (the load-bearing part)

In `agent/chat_loop.rs`, each turn AFTER `compact_messages` and BEFORE `provider.send`,
if the Vec is non-empty:

```rust
let mut req = messages.clone();              // request COPY — persisted messages UNTOUCHED
req.push(ChatMessage {                       // APPEND, not insert(0, …)  — see §7
    role: Role::User,
    content: MessageContent::Blocks(vec![ /* checklist + in-progress focus marker */ ]),
});
provider.send(req).await
```

- The block is appended to a **clone**; the persisted `messages` Vec never carries it.
- **No `cache_control`** on the block (it changes every turn → cache would never hit).
- Empty Vec (turn 1, before any update) → skip injection, symmetric to memory/skill empty-skip.

### 4. Validation & Error Matrix

| Condition | Result |
|---|---|
| Missing `items` key | Empty list (`is_error: false`) — same as `{"items": []}` |
| Empty `items: []` | Clears the list (`is_error: false`) |
| 2+ items `status: "in_progress"` | Coerced to exactly 1 (last kept, rest → `pending`); `is_error: false` |
| Unrecognized `status` string | Coerced to `pending`; `is_error: false` |
| Item missing `content` | That item skipped (rest pass); `is_error: false` |
| Tool cancelled mid-execution (RULE-A-004) | No `tool_executed` audit row; cancel path emits a synthetic `is_error: true` tool_result to preserve the tool_use/tool_result pairing invariant |

### 5. Good / Base / Bad Cases

- **Good**: model calls with `[{a, done}, {b, in_progress}, {c, pending}]` → Vec replaced;
  next turn the ephemeral block is appended to the request; card renders.
- **Base**: model passes two `in_progress` → coerced to one; summary reads `1 in_progress`;
  ephemeral block marks only the last as `<- in progress`.
- **Bad (anti-pattern the tool corrects)**: model calls intending to "add one item" but
  passes only the new item → list full-replaced to just that one (prior items lost). The
  tool is full-replace by design (replay contract); the model must always pass the FULL list.

### 6. Tests Required

Backend (`cargo test --lib`):

| Test | Asserts |
|---|---|
| `update_checklist::execute_full_replace_not_append` | 2nd call's list fully replaces (2 items), not append (5) |
| `update_checklist::execute_two_in_progress_coerces_to_one` | 2 in_progress → 1 (last kept); summary `1 in_progress` |
| `update_checklist::execute_empty_items_clears_list` / `_missing_items_key_treated_as_empty` | empty/missing → cleared, is_error false |
| `update_checklist::execute_unknown_status_coerced_to_pending` | `"blocked"` → pending |
| `update_checklist::coerce_keeps_last_in_progress_demotes_earlier` | rposition last, demote earlier |
| `agent_loop_update_checklist_replaces_vec_and_injects_next_turn` | tool_use → Vec replaced → tool_result full list → turn-2 request carries the ephemeral block; turn-1 does NOT; persisted messages never carry it |
| `agent_loop_update_checklist_coerces_two_in_progress_to_one` | ephemeral block marks only the last in_progress |
| `agent_loop_cancelled_update_checklist_skips_audit_row` | RULE-A-004: cancelled update → 0 `tool_executed` rows |

Frontend: `pnpm build` (vue-tsc strict) + `pnpm vitest run` (31 checklist store tests). The
TS `coerceAtMostOneInProgress` must stay a line-by-line mirror of the Rust
`coerce_at_most_one_in_progress` — a drift is a cross-layer bug (see
`frontend/state-management.md §"B12 Checklist store"`).

### 7. Wrong vs Correct — injection placement (prompt-cache invariant)

#### Wrong: prepend the checklist block to position 0

```rust
// BAD — insert(0, …) puts the per-turn-mutating block IN FRONT of the memory
// breakpoint at messages[0].
let mut req = messages.clone();
req.insert(0, checklist_msg);   // ← busts the memory cache key
provider.send(req).await
```

The memory instructions block at `messages[0]` carries a `cache_control: Ephemeral`
breakpoint (`memory/loader.rs::build_instructions_blocks`). Anthropic's cache key is the
byte-prefix up to the breakpoint; everything before it must be byte-identical across turns.
A per-turn-changing checklist block at position 0 sits in front of the breakpoint and busts
the memory cache every turn (50 turns × ~100 KiB instruction files = the exact cost
explosion B5 was built to eliminate — 1.0× input billing instead of 0.1× cache-read).

#### Correct: append the checklist block (after the breakpoint)

```rust
// GOOD — append keeps the checklist AFTER the memory breakpoint, so the memory
// cache window stays intact (same reason the B4 skill block sits at position 2,
// after the memory pair).
let mut req = messages.clone();
req.push(checklist_msg);        // ← after the breakpoint, cache preserved
provider.send(req).await
```

Anthropic accepts consecutive user-role messages, so appending a user block after the
user's latest prompt is wire-legal; recency (checklist is the last thing in context) is a
bonus. (This was caught by `trellis-check` on PR1 — the original plan said "prepend".)

---
