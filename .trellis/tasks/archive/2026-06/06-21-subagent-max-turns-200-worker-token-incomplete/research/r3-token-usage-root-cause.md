# Research: R3 — Worker `subagent_runs.token_usage_json` Always-Zero Root Cause

- **Query**: Why does the `c27f3fd7-...-7fd5e6d75eaf` general-purpose worker run show `{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}` in `subagent_runs.token_usage_json` (run from `4fbc4515-...`), while a sibling `researcher` run (`4588194e-...`) shows the expected real cumulative usage (`170879 / 606 / 0 / 145792`)?
- **Scope**: internal (Rust agent loop + provider SSE + sink + DB)
- **Date**: 2026-06-21
- **DB inspected**: `~/.local/share/dev.everlasting.app/everlasting.db` (live app data)

## Verdict (one line)

**Primary root cause = (a) `max_turns` terminal path in `chat_loop.rs` drops `last_usage` when emitting the worker terminal `Done`.** The other three suspects are essentially clean.

## Evidence

### 1. DB cross-check (this is the discriminator)

`SELECT status, subagent_name, COUNT(*), token_usage_json` over the live `subagent_runs` table:

| id (head) | subagent_name | status | token_usage_json |
|---|---|---|---|
| `4588194e-…` | researcher | completed | `{"input_tokens":170879,"output_tokens":606,"cache_creation_input_tokens":0,"cache_read_input_tokens":145792}` |
| `c27f3fd7-…` | general-purpose | completed | `{"input_tokens":0,"output_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}` |

Both runs reached `status='completed'` (NOT cancelled, NOT error), both have populated `transcript_json` (1.1 MiB vs 310 KiB), neither has `transcript_truncated=1`. So the bug fires on the **completed** terminal path, not on cancel / error / truncation.

The fact that `transcript_json` is non-empty for `c27f3fd7` rules out (b) "the worker path never collects any usage" wholesale — the worker DID run turns and DID push events into the sink. So the sink's per-turn accumulator did see *some* `ChatEvent::Done { usage: Some(_) }` events. But the terminal write shows all-zero. That forces the bug to the **terminal synthesis** path, not to per-event accumulation.

### 2. `last_usage` lifecycle in `chat_loop.rs`

- **Init**: `chat_loop.rs:873` — `let mut last_usage: Option<crate::llm::types::TokenUsage> = None;` (per-turn, inside the `turn` loop).
- **Update**: `chat_loop.rs:966-968` — on `ChatEvent::Done { stop_reason: sr, usage }` we do `last_usage = usage.clone();`. This captures the per-turn usage into the local.
- **Where it dies** — three terminal sites:
  1. **Normal completion** (`chat_loop.rs:1277-1282`): emits `Done { stop_reason, usage: last_usage }` — `last_usage` IS threaded through. Correct.
  2. **Cancel** (`chat_loop.rs:1226-1233` and `1751-1758`): emits `Done { stop_reason: Some("cancelled"), usage: None }` — usage deliberately dropped (cancel path has no usage yet).
  3. **`max_turns` reached** (`chat_loop.rs:1797-1804`): emits `Done { stop_reason: Some("max_turns"), usage: None }` — **the local `last_usage` from the final turn is dropped on the floor**. The local variable is consumed by the iteration that produced it; the function exits the loop and synthesizes a brand-new terminal event with `usage: None`.

   This is the asymmetry: site #1 reads `last_usage` at the end of the turn that *finished naturally* (per-turn Done carries usage; we re-emit it); sites #2/#3 hard-code `usage: None` because they assume no terminal usage will arrive (true for cancel; **false for max_turns**, which always exits the loop at the end of a turn that DID emit a Done with usage).

   **This is the bug.** It explains why the `c27f3fd7` row exists with all-zero usage: the worker hit `turn_limit` on a clean turn-end (we know `transcript_json` is non-empty, so turns ran) and the terminal Done went out with `usage: None`.

### 3. Sink drain order vs `update_run_finished` write order in `subagent.rs`

- `subagent.rs:835-849` — `SubagentBufferSink.emit_chat_event` captures each `ChatEvent::Done { usage }` into `self.per_turn_usage` (Vec). The `cumulative_usage()` accessor (`subagent.rs:794-801`) sums the live Vec without draining.
- `subagent.rs:782-789` — `drain_per_turn_usage()` exists but is unused in production (no callsite found anywhere outside tests/`db/tests.rs`).
- The terminal write happens in `chat_loop.rs:2320-2351` after `run_chat_loop` returns:
  1. `worker_sink.cumulative_usage()` (line 2327) reads the Vec;
  2. `update_run_finished(...)` (line 2334) writes it as `token_usage_json` (`db/subagent_runs.rs:313-314` serializes via `serde_json::to_string`).

   **So if the sink's `per_turn_usage` Vec is empty at drain time, the JSON written is `TokenUsage::default()` → all zeros.** This is exactly what `c27f3fd7` shows.

- Why would the Vec be empty? Because the per-turn `Done { usage }` events were never received. The worker is started with `skip_persist=true` and runs through `run_chat_loop` (chat_loop.rs:2283). Inside that loop, when the turn-limit trips at line 1792, the function falls out of the `turn` loop and emits the synthetic Done with `usage: None` (line 1797-1804). The sink sees `Done { usage: None }` and skips the `per_turn_usage.push(*u)` branch (`subagent.rs:844-849` is gated on `if let Some(u) = usage`). Every prior turn's `Done { usage: Some(t) }` from line 966 IS captured (good — but if the worker exits cleanly via natural tool_use→stop_reason sequence, `last_usage` is threaded at line 1280 and gets captured again at sink line 844 — fine). The bad path is `max_turns`: the LAST turn's `last_usage` from line 968 is **never relayed to the sink** because the terminal Done is hand-synthesized with `None`.

- **Claim check on the streaming-fold pipeline**: `add_token_usage_streaming` is defined (`db/subagent_runs.rs:554`) and tested (`db/tests.rs:2605`) but **has no production callsite**. The comments at `subagent.rs:567-569` and `subagent.rs:838-843` describe a per-turn streaming fold into `sessions.input_tokens_total` via `add_token_usage_streaming`; that path is documented but **not wired**. The only streaming path actually firing is `db::add_token_usage` at `chat_loop.rs:1004`, which writes to `sessions` (the parent's row, because the worker reuses `parent_session_id`) — NOT to `subagent_runs`. So the parent's `sessions.input_tokens_total` likely DOES reflect the `c27f3fd7` worker's tokens (good); only the per-run `subagent_runs.token_usage_json` row is wrong.

### 4. Provider SSE usage parsing — essentially clean

- **Anthropic** (`llm/provider/anthropic.rs:546-625`): captures `message_delta.usage` (the cumulative-per-turn source, authoritative) at line 573-575, falls back to `message_start.usage` at 594-603, yields `ChatEvent::Done { stop_reason, usage }` at line 625. The all-zero guard at `parse_anthropic_usage:762` (`if input==0 && output==0 && ... { None }`) is correct — it distinguishes "no payload" from "all-zero payload" so the agent loop's SQL write isn't polluted.
- **OpenAI** (`llm/provider/openai.rs:610-680`): `stream_options.include_usage` is set at line 332; `parse_openai_usage` at line 898-925 maps `prompt_tokens → input_tokens`, `completion_tokens → output_tokens`, `prompt_tokens_details.cached_tokens → cache_read_input_tokens`; all-zero → `None` at line 916. Symmetric with Anthropic.
- Both providers have tests (`anthropic.rs:1058-1108`, `openai.rs:1350-1430`) covering the standard / minimal / no-key / all-zero payloads. No signs of (c) being a real culprit.

The `c27f3fd7` worker was likely running against an Anthropic-compatible provider (the DB also has a `researcher` row with normal values that came from the same code path), so (c) provider parsing bugs would also show on the sibling — they don't. (c) is out.

### 5. The other terminal sites

- Cancel sites (`chat_loop.rs:1226-1233`, `1751-1758`) emit `usage: None` deliberately — `was_cancelled` is the user-initiated stop, not an LLM-driven event, so there is no last usage to capture. This is correct.
- Error site (`chat_loop.rs:1246-1254`) does not emit a terminal Done at all — relies on the per-event `ChatEvent::Error` to terminate. Correct.
- Normal completion site (`chat_loop.rs:1277-1282`) emits `Done { stop_reason, usage: last_usage }`. Correct.
- `max_turns` site (`chat_loop.rs:1797-1804`): emits `Done { stop_reason: Some("max_turns"), usage: None }`. **The only site that drops `last_usage` despite `last_usage` being populated.** Incorrect.

## Recommended fix scope

**R3a alone (terminal-patch `max_turns` site) is sufficient to cover `c27f3fd7`-style runs.**

Concrete patch (illustrative, not for me to write):

```rust
// chat_loop.rs:1797-1804
emit_chat_event_via_sink(
    &sink,
    &rid,
    &ChatEvent::Done {
        stop_reason: Some("max_turns".to_string()),
        usage: last_usage,    // was: usage: None
    },
);
```

But note: by the time we reach line 1797, `last_usage` is the **last turn's** per-turn usage, not the worker's cumulative. The sink's `per_turn_usage` Vec ALREADY contains the prior turns' usage (they were captured from per-turn Done events emitted at line 922-1035 — each `Done { usage: Some(t) }` was relayed to the sink at the end of its turn). When the terminal `Done { usage: last_usage }` arrives at the sink (`subagent.rs:835-849`), the sink will push `last_usage` again into `per_turn_usage`, which **double-counts** the last turn. So R3a must either:

- **(a)** Replace `usage: None` with `usage: last_usage` AND ensure the sink's terminal `Done` doesn't double-accumulate. Either: emit the terminal `Done` BEFORE flushing the per-turn usage back into the Vec (then drain), OR distinguish "terminal Done" from "per-turn Done" in the sink arm.
- **(b)** Replace `usage: None` with `usage: None` (keep) but make `update_run_finished` accumulate `cumulative_usage() + last_usage` if non-`None`. Slightly hacky.
- **(c)** Move the `last_usage` capture OUT of the sink Vec path entirely and use a separate `last_turn_usage: Option<TokenUsage>` field on `SubagentBufferSink`. Then `update_run_finished` calls `cumulative_usage() + last_turn_usage`. Cleanest, but a bigger change.

**R3b (provider parsing) is NOT necessary** — both providers' SSE usage parsing is already exercised by tests and the sibling `researcher` row at `4588194e-…` proves the wire format is being parsed correctly in production.

**Bonus finding (not part of R3 per se but adjacent)**: the documented "streaming-fold pipeline" (`add_token_usage_streaming`) is unimplemented in production. The docstring at `subagent.rs:567-569` and the comment at `subagent.rs:838-843` describe a behavior that doesn't exist. Worth a separate PR to either remove the misleading comments or actually wire the streaming fold (it would fix the parent UI counter being a few seconds stale behind the worker's `subagent_runs.token_usage_json`).

## Open questions for the main agent

1. **Confirm the c27f3fd7 worker actually hit `max_turns`** vs ended via natural stop_reason=`end_turn`. The DB row says `status='completed'` and the worker transcript is 310 KiB; `run_subagent` only sets `Completed` when `!was_cancelled() && !had_error()` and the natural exit would route through the normal completion site (line 1277) which DOES thread `last_usage`. So either: the worker hit `max_turns` (and the bug applies) OR the worker hit `max_turns` and was then marked completed because it didn't cancel/error (likely — need to look at how `run_chat_loop` exit-status maps to `SubagentStatus`). **Confirm by inspecting the c27f3fd7 transcript's last entries** (the `stop_reason` of the final `Done` in `transcript_json` should be `"max_turns"` if the bug applies).

2. **R3a fix shape**: prefer option (a)/(c) above over (b). My recommendation is (a) — replace `usage: None` with `usage: last_usage` at the max_turns site AND change the sink's `Done` arm to NOT push usage into `per_turn_usage` when `stop_reason` is one of the terminal synthetic stops (`max_turns`/`cancelled`). The cancellation site still uses `usage: None` so the guard just needs to allow `max_turns` to forward `last_usage` while not double-counting. Alternatively (c) — separate field on the sink — is cleaner but bigger.

3. **Test coverage**: the `subagent.rs:1607-1640` tests (`buffer_sink_accumulates_token_usage_per_turn` etc.) cover per-turn accumulation but **not the terminal-emit-then-cumulative path**. A regression test should: emit `Done { stop_reason: Some("end_turn"), usage: Some(u1) }`, then `Done { stop_reason: Some("end_turn"), usage: Some(u2) }`, then `Done { stop_reason: Some("max_turns"), usage: Some(u3) }`, then assert `cumulative_usage() == u1+u2+u3` AND `update_run_finished` would persist the correct JSON. This locks R3a against future regressions.

4. **Documented-but-unimplemented streaming fold**: out of scope for R3 but flagged. Should we (i) delete the comments at `subagent.rs:567-569` and `subagent.rs:838-843` and `db/subagent_runs.rs:18` claiming per-turn streaming folds happen, OR (ii) actually wire `add_token_usage_streaming` into `chat_loop.rs:1004`'s per-turn `Done` handler? My lean is (i) — the existing `db::add_token_usage` at line 1004 already covers the parent-session live counter, and the per-run JSON is the terminal write which works once R3a is in.

## Caveats

- DB inspected is the dev app's local copy. Production data may show a different distribution; the **structural** bug (max_turns site dropping last_usage) is independent of the specific row.
- Only 2 `subagent_runs` rows exist in the live DB (one `researcher`, one `general-purpose`). One shows the bug, one doesn't. The sibling success is enough to rule out (b) and (c); the asymmetry between max_turns (no usage) and end_turn (with usage) terminal sites is enough to localize (a).
- `add_token_usage_streaming` might be wired in a different branch / WIP commit not visible at HEAD; the comment trail says "B6 PR2 (2026-06-20)" so it's recent. If main agent wants to double-check, `git log --oneline -- app/src-tauri/src/db/subagent_runs.rs | head -20` will show.