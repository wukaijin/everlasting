<!-- Moved from llm-contract.md 2026-09-19 (doc-split) -->

## Scenario: E2 trace ChatEvent variants are server-emitted, not LLM-streamed (E2, 2026-07-14)

> **Source**: E2 trace viewer task `07-14-e2-harness-trace-viewer`
> (child-1 `e2-backend-trace-pipeline`).

The 3 E2 trace variants — `ContextCompacted` / `LoopHint` /
`WorkflowBreadcrumb` — are **pushed by the server (`agent::trace::
record_*` helpers), not by the LLM stream**. The provider cannot
emit them, so the SSE consumer arm must drop them defensively if
they somehow appear in the stream (same pattern as `Recall` /
`Retrying` / `FileInjections`):

```rust
// chat_loop.rs SSE consumer fallback arm
ChatEvent::ContextCompacted { .. }
| ChatEvent::LoopHint { .. }
| ChatEvent::WorkflowBreadcrumb { .. } => {
    tracing::warn!(
        request_id = %rid,
        "chat: unexpected trace event in LLM stream (ignoring)"
    );
}
```

**Wire shape** (`#[serde(tag = "kind", rename_all = "snake_case")]`
on the enum):

| Variant | Fields |
|---|---|
| `ContextCompacted` | `seq`, `tokens_before`, `tokens_after`, `dropped_count`, `degradation` (`"none"` / `"no_candidates"` / `"still_over"`) |
| `LoopHint` | `seq`, `hit_count`, `verdict_kind` (`"hard"` / `"soft"`) |
| `WorkflowBreadcrumb` | `seq`, `task_slug` (Option), `status` (Option), `breadcrumb_text` |

**Why server-emitted, not LLM-streamed.** The trace signals describe
harness decisions (C3 compaction result, C2 loop verdict, workflow
breadcrumb text) that the LLM has no knowledge of and no reason to
echo back. Server emit + persist via the `agent::trace::record_*`
helpers gives a single, typed source of truth and lets the
`turn_trace` table be populated in lockstep with the live emit
(double-write best-effort, see `agent/trace.rs`).

**Why the defensive drop arm.** If a future provider is wrapped in
a way that re-emits a `ChatEvent` it observed (e.g. during a
provider rewrite), the SSE consumer would otherwise panic on a
`match` fallthrough. The `warn!` + drop pattern keeps the agent
loop resilient without affecting the trace pipeline.


