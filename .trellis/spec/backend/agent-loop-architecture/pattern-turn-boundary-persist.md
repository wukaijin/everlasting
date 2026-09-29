## Pattern: Turn-boundary persist symmetry — error arm matches cancel arm (RULE-A-007, 2026-06-17)

**Problem**: When the LLM stream emits `ChatEvent::Error` mid-turn,
the agent loop's per-event arm emits the Error to the frontend
(already rendered as a terminal signal) and sets `had_error = true`.
Before RULE-A-007, the post-stream-loop code did `if had_error { return; }`
— bailing out **without** persisting any of the turn's accumulated
`text_parts` / `finalized_thinking` / `tool_calls`. The cancel path,
in contrast, flushed pending thinking, appended `CANCELLED_MARKER` to
the text, and called `persist_turn` so the partial turn survived in
the DB. This asymmetry meant: a user who watched a partial response
render live, then reloaded the session, would find the assistant turn
missing entirely (cancel preserved, error discarded).

**Solution**: The error arm now mirrors the cancel arm. Both paths:

1. Flush pending thinking into `finalized_thinking`.
2. Log an info-level `tracing` line (`cancelled — persisting partial turn`
   vs `errored — persisting partial turn`) so the cause is distinguishable.
3. Build the assistant blocks (`thinking` + `text` + `tool_use` +
   `redacted_thinking`) and append a sentinel marker to the text:
   - Cancel → `CANCELLED_MARKER` (`"[已停止]"`)
   - Error → `ERROR_MARKER` (`"[生成出错中断]"`, RULE-A-007 new constant)
   - Empty-text edge case: marker alone (symmetric branch in each arm).
4. `persist_turn` the partial row.
5. Emit `ChatEvent::TurnComplete { seq, ...latency }` so the frontend
   has the seq + latency breakdown for the partial row.

The two arms differ in **two** places only:

| Concern | Cancel path | Error path |
|---|---|---|
| Persist failure handling | log-only (no emit; the loop is about to emit terminal `Done{cancelled}`) | **log-only** (no emit; the per-event arm already emitted terminal `Error`. A second Error would be a conflicting double-terminal — RULE-A-007 decision B) |
| Terminal signal after persist | `Done { stop_reason: "cancelled", usage: None }` | (none — the pre-emit `Error` is the terminal; no follow-up `Done`) |

### Why error persist failure is log-only (RULE-A-007 decision B)

RULE-A-003 (2026-06-15) made **normal-path** persist failures emit a
typed `ChatEvent::Error{Server}` + abort (otherwise disk-full / DB-lock
contention would silently lose the user message). The error path is
**different**: the per-event arm at `ChatEvent::Error { .. }` already
emits the Error to the frontend before the persist attempt. Emitting
`emit_persist_failure` on top would produce two terminal events
(Error + Error), and the frontend's terminal handling would fire twice.

The cancel path's synthetic tool_result persist already uses log-only
for the same reason (its terminal `Done{cancelled}` is about to fire).
RULE-A-007 makes the error path's assistant-turn persist follow the
same log-only pattern, keeping the "exactly one terminal event per
request" invariant intact.

### Why error path still emits TurnComplete (RULE-A-007 decision C)

`TurnComplete` carries the partial turn's `seq` + latency breakdown.
The frontend uses it to (a) know which row to attach the latency to
and (b) trigger any per-turn UI updates. Without it, the error path's
partial row would be in the DB but the live-streaming UI wouldn't know
its seq until a reload. The pre-emit `Error` event and the
`TurnComplete` event are **not** in conflict — they carry disjoint
information (Error = "something broke"; TurnComplete = "this seq's
partial turn landed + here's the latency"). The controller routes
each event independently.

### Constants

Both markers live in `app/src-tauri/src/agent/helpers.rs` next to
each other:

```rust
pub const CANCELLED_MARKER: &str = "[已停止]";
pub const ERROR_MARKER: &str = "[生成出错中断]";
```

The bracketed-text style survives DOMPurify unchanged, is
locale-friendly, and renders inline in the bubble's markdown. The UI
does not need a special "interrupted" render branch — existing
markdown rendering handles both markers uniformly.

### When to apply this pattern

- Any new terminal path through `run_chat_loop` that has already
  accumulated partial content (text / thinking / tool_use) MUST
  persist the partial turn. The pattern: flush → marker → persist →
  TurnComplete. Bailing out with raw `return` before persist is the
  anti-pattern that RULE-A-007 removed.
- The persist failure handling on a terminal path is log-only
  (NEVER `emit_persist_failure`) — the terminal event was already
  emitted; a second one would conflict.

### When NOT to apply

- A terminal path that has accumulated **zero** content (no
  `text_parts`, no `finalized_thinking`, no `tool_calls`,
  no `redacted_thinking_data`) skips the persist entirely — the
  `if !assistant_blocks.is_empty()` guard handles this. The error
  path's `ErrThenEnd` (no preceding delta) still hits the persist
  branch because the `ERROR_MARKER` alone populates `full_text`.
  This is intentional — the user sees a visible "[生成出错中断]"
  marker explaining what happened, rather than a blank turn.

---

## Pattern: 取消后的 tool_use/tool_result 配对完整性(N19 件⑤, 2026-09-29)

**不变量**:任意取消后,DB 尾部 `assistant(tool_use×N)` 的下一条 user
消息必含 **N 个 tool_result(真 + synthetic 差集)**。两层对齐:

1. **send/流阶段取消**(工具未开始执行):`drive.rs` 的 `if cancelled`
   臂对**全部** tool_calls 落库一条 synthetic is_error tool_result 消息
   (`helpers::build_synthetic_tool_result_message`,BUG FIX 2013)。
2. **执行阶段取消**:`finalize_turn` 的取消臂在落库前计算**差集** ——
   `result_blocks` 里已有 ToolResult 的 id 集合 ∪ `tool_calls` 中缺
   result 的 (id, name),每个追加一个 synthetic is_error block
   (`helpers::synthetic_tool_result_block`,与 drive.rs 单源),与部分
   真 result 同一条 user 消息落库。`FinalizeFrame.tool_calls` 字段
   (hub 在 move 进 `DispatchCtx` 前 clone)承载该信息。

**为什么落库层(而非只靠 wire 层)**:wire 层 `chat_request_to_wire`
的 orphan tool_use 注入(08-06 群聊 speaker-desync 事故引入)兜得住
400,但 serial 取消这个高频可枚举路径依赖它时,该 warn 的出现不再
等于"有 bug"(真异常信号被噪音淹没),且 DB 历史不自洽(回放/导出/
检索看到的历史缺一对)。本件落地后,wire 自愈回归其本职(兜不可
枚举逃逸,如群聊 max_turns 类),**该 warn 出现即回归信号**。

**顺序约束**:差集 synthetic 追加在真 result×N 之后、loop_hint Text
之前(wire 顺序:tool×N → user(text);OpenAI 要求 tool 消息紧邻
`assistant(tool_calls)`,hint Text 必须在末尾)。

**幂等性**:差集为空(L2 并行路径 slot 结构天然全配对 / 无取消 turn)
时零行为变化 —— 非取消路径逐字节保持。

**worker 模式(skip_persist)**:同样补齐 `messages.push` 的内存
transcript(SubagentBufferSink 是 worker 的执行记录,配对完整性同
样成立),但不落 DB —— 与 send 阶段取消的 worker 行为对称。

**范围边界**:dispatch 并发批(subagent)不在内 —— 其取消走
`status=cancelled` 的 dispatch tool_result 既有契约(B6)。send 阶段
(drive.rs)与执行阶段(finalize_turn)两路径**共同**保证任意取消后
DB 尾部配对完整;错误路径(had_error)本就全量 synthetic
(drive.rs),不受本件影响。

**测试**:`tests_agent_loop/cancel_pairing.rs` —
`finalize_cancel_appends_synthetic_difference_set`(5 tool_use + 2 真
结果 + cancelled → 落库消息 5 个 tool_result,hint 末尾,DB 行经
`orphan_tool_use_ids` 验证无孤儿)、`finalize_no_cancel_appends_nothing`
(门控回归:无取消零变化)、`finalize_cancel_worker_mode_pairs_
transcript_without_db_row`(worker 内存配对 + 不落库)。

---
