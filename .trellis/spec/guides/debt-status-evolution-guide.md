# DEBT.md Status Evolution Guide

> **Purpose**: Help future maintainers evolve a `RULE-*` entry in
> `.trellis/reviews/DEBT.md` from `partial` → `closed` (or `open` →
> `wontfix`) correctly, and avoid the stale-docstring anti-pattern that
> occurs at the partial-closure handoff.

---

## When to use this guide

You're working on a task that:

- Touches an entry in `.trellis/reviews/DEBT.md` (any RULE-*, A-*, E-*, B-* entry)
- The entry's status is being changed (partial → closed, open → closed, etc.)
- You have written/changed code that affects what the entry claims

---

## The 3-step partial → closed transition

When the implementation work for a partial entry is complete, the entry
needs three coordinated updates. Missing any one of them leaves the
entry in a state that's worse than partial — it claims closure but the
artifacts don't agree.

### Step 1: Update the `Status:` field

```diff
- Status: partial (2026-06-14)
+ Status: closed (2026-06-15)
```

Use the date the closure commit lands, not the date the original
partial entry was created. If the closure spans multiple commits, use
the date of the final commit in the chain.

### Step 2: Replace the **Partial Closure Note** with **Closure Note**

The Partial Closure Note describes what is *still broken or unverified*
at the partial-closure moment. After full closure, that section is
**misleading** — it describes problems that no longer exist.

**Wrong** (leaves the partial note in place):

```markdown
## RULE-A-006 — Production agent loop body unified with test surface

Status: closed (2026-06-15)

### Partial Closure Note
The production chat.rs spawn body is a faithful port of run_chat_loop
in chat_loop.rs. Drift hazard remains: any change to one must be
mirrored to the other.

### Closure Note
[doesn't exist]
```

**Correct** (replace, don't append):

```markdown
## RULE-A-006 — Production agent loop body unified with test surface

Status: closed (2026-06-15)

### Closure Note
[Date the closure landed + 1-2 sentences describing what was unified
+ a pointer to the implementation commit/task]
```

If the rule is closed but its history is worth preserving, move the
Partial Closure Note into a "History" subsection **after** the Closure
Note, with a "Closed YYYY-MM-DD" header. Don't delete the history
outright — it documents the path that was taken, which is useful for
similar future work.

### Step 3: Add a Re-evaluation Log entry

Every DEBT.md rule has a Re-evaluation Log section (see existing
entries). Add a new entry at the **top** of the log (most-recent-first)
describing:

1. **When**: the date of the re-evaluation
2. **What changed**: the closure commit / task ID
3. **Why now**: what unblocked the closure (e.g., "production
   migration to run_chat_loop complete, faithful-port drift
   hazard eliminated")
4. **Verifier**: who/what confirmed closure (e.g., "trellis-check
   sub-agent: 9 agent_loop_* tests pass + 0 warnings")

Example:

```markdown
### Re-evaluation Log

- **2026-06-15**: Status partial → closed. Production `chat.rs::chat`
  now routes through `run_chat_loop` (task `06-15-unify-chat-loop-dispatch`,
  commit `<hash>`). Faithful-port drift hazard eliminated: any future
  change to the agent loop body is now visible to both production and
  tests. Verifier: 9 `agent_loop_*` integration tests pass under
  `MockProvider` + `MockEmitter` covering production call path;
  `cargo check` + `cargo check --tests` 0 warnings.
```

---

## The stale-docstring anti-pattern

**Symptom**: A code change updates a *behavior* (e.g., a struct's call
sites, a function's visibility) but the *docstring* still describes
the previous state. The result is a docstring that contradicts itself
in two paragraphs:

```rust
/// ChatEventSink — emit-side abstraction for agent loop events.
///
/// Currently dispatched only through the test-gated chat_loop;
/// production chat.rs still emits via `app.emit(...)` directly.
///
/// All four methods are exercised in production: `emit_chat_event`
/// is called from 5+ sites, `emit_tool_call` from 1, ...
```

The first paragraph was accurate at the partial-closure moment. The
second paragraph was added when the closure landed. The first was
never deleted.

This is **worse than a fully stale docstring** — a fully stale
docstring is wrong but consistent. A contradictory docstring
trains the reader to ignore the docstring entirely, which is the
opposite of what code-spec is for.

### Cause: docstring update is not a single-step change

Code changes have a natural commit boundary (the implementation
commit). Docstrings are usually updated in the same commit. **But**
when the implementation is done in two stages (first partial closure,
then full closure), the partial-closure docstring update is *also* a
separate commit — and at the partial moment, it is correct.

When the full closure lands, the implementer reads the *implementation*
diff but not the *docstring* diff. The "previous state" paragraph
lingers.

### Fix: when closing a partial rule, search for stale description

When you change a `RULE-*` entry's status from partial to closed,
**search the codebase for the rule's identifier** (e.g., `RULE-A-006`)
and review every docstring / comment that references it. Delete or
rewrite the parts that describe the *partial* state.

```bash
# Find every reference to the rule identifier
grep -rn "RULE-A-006" app/ docs/ .trellis/
```

For each hit, ask: "Is this describing the **previous** state, the
**current** state, or **both**?" If both, the previous-state paragraph
needs to go (or move to a "History" subsection if the history itself
is load-bearing — see Step 2 above).

### Prevention: write docstrings about *behavior*, not *status*

Docstrings that describe a *status* (e.g., "currently dispatched only
through...") are fragile: the status changes, the docstring lies.
Docstrings that describe a *behavior* (e.g., "the sink is the
abstraction over the Tauri AppHandle — all 4 emit channels route
through it; tests inject a `MockEmitter` to assert event sequences")
survive status changes.

**Wrong** (status-oriented, brittle):

```rust
/// ChatEventSink is test-gated. Production chat.rs still emits
/// directly via app.emit(...). Once RULE-A-006 closes, this trait
/// becomes production-routed.
```

**Correct** (behavior-oriented, durable):

```rust
/// ChatEventSink abstracts the four emit channels used by the
/// agent loop body: `chat-event` (text deltas + final), `tool:call`,
/// `tool:result`, `permission:ask`. Production wires `AppHandleSink`
/// (which forwards to `app.emit`); tests wire `MockEmitter` to
/// assert event sequences.
```

The behavior-oriented docstring was correct in 2026-06-13 (when the
trait was test-only) **and** in 2026-06-15 (when production started
calling it). The status-oriented docstring was correct only at one
moment, and quickly became misleading.

---

## Checklist before changing a DEBT.md rule's status

- [ ] Status field updated (date = date of the *closing* commit, not
      the original partial-commit)
- [ ] Partial Closure Note replaced with Closure Note (or moved to
      History subsection)
- [ ] Re-evaluation Log entry added at the top with date + closure
      commit + verifier
- [ ] `grep -rn "<RULE-id>"` run, every hit reviewed for stale
      status-description paragraphs
- [ ] Docstrings rewritten in *behavior* terms, not *status* terms,
      so they survive future status changes
- [ ] Cross-references in `docs/ARCHITECTURE.md` / `docs/IMPLEMENTATION.md`
      / spec files updated to reflect closure (e.g., "(RULE-A-006
      闭环, 2026-06-15)")

If all 6 are true, the DEBT.md transition is complete and the
stale-docstring anti-pattern is avoided.
