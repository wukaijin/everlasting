# Cross-Layer Thinking Guide

> **Purpose**: Think through data flow across layers before implementing.

---

## The Problem

**Most bugs happen at layer boundaries**, not within layers.

Common cross-layer bugs:
- API returns format A, frontend expects format B
- Database stores X, service transforms to Y, but loses data
- Multiple layers implement the same logic differently

---

## Before Implementing Cross-Layer Features

### Step 1: Map the Data Flow

Draw out how data moves:

```
Source → Transform → Store → Retrieve → Transform → Display
```

For each arrow, ask:
- What format is the data in?
- What could go wrong?
- Who is responsible for validation?

### Step 2: Identify Boundaries

| Boundary | Common Issues |
|----------|---------------|
| API ↔ Service | Type mismatches, missing fields |
| Service ↔ Database | Format conversions, null handling |
| Backend ↔ Frontend | Serialization, date formats |
| Component ↔ Component | Props shape changes |

### Step 3: Define Contracts

For each boundary:
- What is the exact input format?
- What is the exact output format?
- What errors can occur?

---

## Common Cross-Layer Mistakes

### Mistake 1: Implicit Format Assumptions

**Bad**: Assuming date format without checking

**Good**: Explicit format conversion at boundaries

### Mistake 2: Scattered Validation

**Bad**: Validating the same thing in multiple layers

**Good**: Validate once at the entry point

### Mistake 3: Leaky Abstractions

**Bad**: Component knows about database schema

**Good**: Each layer only knows its neighbors

---

## Checklist for Cross-Layer Features

Before implementation:
- [ ] Mapped the complete data flow
- [ ] Identified all layer boundaries
- [ ] Defined format at each boundary
- [ ] Decided where validation happens

After implementation:
- [ ] Tested with edge cases (null, empty, invalid)
- [ ] Verified error handling at each boundary
- [ ] Checked data survives round-trip

---

## Cross-Platform Template Consistency

In Trellis, command templates (e.g., `record-session.md`) exist in **multiple platforms** with identical or near-identical content. This is a cross-layer boundary.

### Checklist: After Modifying Any Command Template

- [ ] Find all platforms with the same command: `find src/templates/*/commands/trellis/ -name "<command>.*"`
- [ ] Update all platform copies (Markdown `.md` and TOML `.toml`)
- [ ] For Gemini TOML: adapt line continuations (`\\` vs `\`) and triple-quoted strings
- [ ] Run `/trellis:check-cross-layer` to verify nothing was missed

**Real-world example**: Updated `record-session.md` in Claude to use `--mode record`, but forgot iFlow, Kilo, OpenCode, and Gemini — caught by cross-layer check.

---

## Generated Runtime Template Upgrade Consistency

Some generated files are both documentation and runtime input. In Trellis,
`.trellis/workflow.md` is parsed by `get_context.py`, `workflow_phase.py`,
SessionStart filters, and per-turn hooks. Template changes must be validated
against both fresh init and upgrade paths.

### Checklist: After Modifying A Runtime-Parsed Template

- [ ] Identify every runtime parser that reads the template, not just the file
  writer that installs it
- [ ] Check whether relevant syntax lives outside obvious managed regions
  such as tag blocks
- [ ] Verify fresh `init` output and a versioned `update` scenario that writes
  the older `.trellis/.version`
- [ ] Add an upgrade regression using an older pristine template fixture, then
  assert the installed file reaches the current packaged shape
- [ ] Update the backend spec that owns the runtime contract

**Real-world example**: Codex inline mode changed workflow platform markers from
`[Codex]` / `[Kilo, Antigravity, Windsurf]` to `[codex-sub-agent]` /
`[codex-inline, Kilo, Antigravity, Windsurf]`. Fresh init was correct, but
`trellis update` only merged `[workflow-state:*]` blocks and preserved stale
markers outside those blocks. Result: upgraded projects got new hook scripts
but old workflow routing, so `get_context.py --mode phase --platform codex`
could return empty Phase 2.1 detail.

---

## Mode-Detection Probe Checklist

When a CLI auto-detects a mode by probing a remote resource (e.g., checking if `index.json` exists to decide marketplace vs direct download):

### Before implementing:
- [ ] Probe runs in **ALL** code paths that use the result (interactive, `-y`, `--flag` combos)
- [ ] 404 vs transient error are distinguished — don't treat both as "not found"
- [ ] Transient errors **abort or retry**, never silently switch modes
- [ ] Shared state (caches, prefetched data) is **reset** when context changes (e.g., user switches source)
- [ ] **Shortcut paths** (e.g., `--template` skipping picker) must have the same error-handling quality as the probed path — check that downstream functions don't call catch-all wrappers

### After implementing:
- [ ] Trace every path from probe result to the mode-decision branch — no fallthrough
- [ ] External format contracts (giget URI, raw URLs) are tested or at least documented as comments
- [ ] Metadata reads consume a complete response or use a streaming parser — never parse a fixed-size prefix as full JSON
- [ ] When reconstructing a composite identifier from parsed parts, verify **all** fields are included and in the **correct position** (e.g., `provider:repo/path#ref` not `provider:repo#ref/path`)
- [ ] Verify that **action functions** called after a shortcut don't internally use the old catch-all fetch — they must use the probe-quality variant when error distinction matters

**Real-world example**: Custom registry flow had 8 bugs across 3 review rounds: (1) probe only ran in interactive mode, (2) transient errors fell through to wrong mode, (3) giget URI had `#ref` in wrong position, (4) prefetched templates leaked across source switches, (5) `--template` shortcut bypassed probe but `downloadTemplateById` internally used catch-all `fetchTemplateIndex`, turning timeouts into "Template not found".

**Real-world example**: Agent-session update hints fetched npm `latest` metadata with `response.read(4096)` and then parsed it as complete JSON. The `@mindfoldhq/trellis` package metadata exceeded 4 KB, so the JSON was truncated, parse failed silently, and the first session injection showed no update hint. Fix: read the complete response before parsing, and add a regression where `version` is followed by an 8 KB metadata tail.

---

## When to Create Flow Documentation

Create detailed flow docs when:
- Feature spans 3+ layers
- Multiple teams are involved
- Data format is complex
- Feature has caused bugs before

---

## Adding a New Anthropic Content Block Type

The LLM client supports four content block types: `text`, `tool_use`,
`thinking`, `redacted_thinking`. Adding a fifth (e.g. `image`, `document`,
or a new Anthropic-side block) requires coordinated changes in **six**
places. Missing any one of them produces a silent data-loss bug or a 400
from the upstream API.

### Checklist

- [ ] **`app/src-tauri/src/llm/types.rs` — wire type.** Add the variant to
      `ContentBlock` with the correct `#[serde(tag = "type", rename_all = "snake_case")]`
      attribute. Add a round-trip test that asserts the exact JSON shape Anthropic
      emits and accepts.
- [ ] **`app/src-tauri/src/llm/client.rs` — SSE parser.** Add the
      `content_block_start` arm for the new type. If it streams via deltas,
      add a `BlockState` variant and the `content_block_delta` arms; if it
      delivers the full payload in `content_block_start` (like
      `redacted_thinking`), buffer it and emit on `content_block_stop`.
- [ ] **`app/src-tauri/src/llm/client.rs` — `ChatEvent` variant.** If the
      new block produces a stream the frontend should react to incrementally,
      add a `ChatEvent` variant. **Buffer-then-emit on stop** for any
      complete-payload field (signature, data, etc.) — never emit per-delta.
- [ ] **`app/src-tauri/src/llm/types.rs` — outbound payload.** If the block
      is part of an assistant message that gets re-sent to Anthropic on the
      next turn, add it to `ContentBlockPayload` (TS) and to
      `MessageContent`'s `Blocks` variant. **Order matters** — Anthropic
      enforces a specific order (thinking first).
- [ ] **`app/src-tauri/src/lib.rs` — agent loop finalize.** In the
      `flush_pending_*` pattern (one flusher per stream type), accumulate
      and finalize the block at the turn boundary. The block must end up
      in the persisted `MessageContent::Blocks` so the next turn's history
      carries it.
- [ ] **`app/src/stores/chat.ts` + `app/src/components/ChatWindow.vue` — UI.**
      - `ChatMessage` field for the in-memory representation.
      - Event handler in `handleChatEvent` for the new `kind`.
      - Rehydrate path (`rehydrateMessages`) restores from DB.
      - `toPayloadContent` includes the new block in the outbound order.
      - Render in `ChatWindow.vue` at the correct position relative to
        other blocks (e.g. thinking above the bubble, redacted as a
        placeholder).

### Pre-flight checks (before opening the PR)

- [ ] All four round-trip unit tests in the LLM contract pass.
- [ ] `cargo test` is green (currently 57 tests).
- [ ] `pnpm build` (vue-tsc strict) is green.
- [ ] Manual smoke test in `pnpm tauri dev`:
      - The new block streams in correctly.
      - Session switch + return restores the block.
      - A follow-up turn after a block-containing turn does NOT 400.

### Reference

The full step-6 implementation in this repo followed this checklist for
`thinking` and `redacted_thinking`. See `backend/llm-contract.md` for the
wire-level details and `backend/error-handling.md` for the failure modes.

---

## Worktree State Transitions Affect LLM Context (2026-06-08)

A worktree state change is not a pure UI event — it changes the LLM's
view of the world. Before implementing anything that mutates the
`worktree_state` column or moves a session between worktree and project
root:

- [ ] **Cancel in-flight chat BEFORE the destructive work.** The agent
      loop is writing messages / tool results on every turn. If the
      destructive path races an in-flight `INSERT` against a deleted
      `sessions` row, the message is lost (FK violation). Use a
      `session_active_request: HashMap<session_id, request_id>` map to
      find the active token; call `token.cancel()` first, then proceed.
      The agent loop's `tokio::select!` exits, and the
      `CancellationGuard` Drop clears the map entry.
- [ ] **Inject the system event AFTER the destructive succeeds, BEFORE
      the next LLM turn.** A `[worktree event] <description>` row in the
      `messages` table is the only way the LLM learns the worktree
      changed. Insert with `role='user'`, content prefixed with
      `[worktree event]`, metadata `{kind: "worktree_event", event: ...}`.
- [ ] **Refresh the frontend message cache.** The store's
      `messagesBySession` is a read-through cache. After the backend
      inserts the event, the chat store must call
      `controller.refresh(sessionId)` to evict + reload. Otherwise the
      next `send()` builds the LLM payload from the stale cache and
      omits the event.
- [ ] **Order matters across the three layers.** The full sequence for
      `attach_worktree` is:
      1. `cancel_inflight_for_session(sid)` (backend)
      2. `git::worktree::create(...)` (backend)
      3. `db::set_worktree_state(sid, Active, None)` (backend)
      4. `db::insert_system_event(sid, "[worktree event] attached: ...")` (backend)
      5. Return the new `SessionRow` to the frontend
      6. `controller.refresh(sid)` (frontend store)
      7. Re-render the chat panel header chip
- [ ] **Test the race.** Manual smoke test: trigger a long-running
      stream, then click the destructive button. The frontend
      `:disabled="isStreaming"` is a UX guard, not a safety net; the
      backend cancel hook must close the race window.

See `backend/llm-contract.md` "Scenario: Worktree State Transparency +
LLM Cancel" for the full signatures, contracts, and validation matrix
of the 3 new Tauri commands (`attach_worktree` / `detach_worktree` /
`delete_worktree`).
