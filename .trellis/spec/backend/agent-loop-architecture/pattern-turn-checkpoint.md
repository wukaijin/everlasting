# Pattern: Turn 流式检查点写点(RULE-PERSIST-001 闭合,2026-08-24)

## Problem

assistant turn 的内容只在流结束后一次性落库(drive.rs assistant persist site)。daemon 被 kill -9 / OOM 时,流式已生成内容全部丢失,且工具执行窗口崩溃会留下孤儿 `tool_use` 行(下次请求 provider 400)。

## Solution:三写点 + 一恢复,全部 `!skip_persist` 门

写点全在 `chat_loop/drive.rs`(占位/检查点/终态),恢复在 `db/sessions/messages.rs::recover_interrupted_messages`(state.rs 启动序列,reap_orphaned_runs 之后)。

| 写点 | 时机 | 函数 | 语义 |
|---|---|---|---|
| ① 占位 | LLM stream ready 后立即 | `upsert_in_progress_turn` | 空 blocks 行,status='in_progress'(崩溃先于首个 delta 也可恢复) |
| ② 检查点 | Delta / ThinkingDelta 臂,时间门 1s | 同上 | 只读克隆快照:`ordered_blocks.clone()` + pending_text/pending_thinking 追加块;**绝不调用变异的 flush_\* helper**(会破坏流式 pending 状态机) |
| ③ 终态 | 既有 assistant 落库点 | `finalize_turn_persist` | ON CONFLICT(session_id,seq) DO UPDATE 全内容列 + status=NULL;空 turn 走 `delete_in_progress_turn`(带 status='in_progress' 守卫) |

## 硬约束(违反即回归)

1. **`persist_turn` 保持裸 INSERT**:UNIQUE(session_id,seq) 冲突是 seq 漂移 bug 的告警信号(RULE-A-003 家族)。只有 assistant 落库点"知道自己前面有检查点行",由它独占 upsert。其余落库点(user 行 / tool_result 行)seq 不同,永不冲突。
2. **检查点 best-effort**:写失败 `warn!` 后继续流式,不打断、不 emit Error。时间门按 **attempt** 关门(`mark_written` 先于写)——按成功关门会在持续失败(磁盘满/BUSY)时退化为每 delta 全量克隆 + 写 + warn 的重试风暴。
3. **worker 零改动**:三个写点全在 `!skip_persist` 门内;worker 中间 turn 的记录仍是 SubagentBufferSink transcript + subagent_runs。
4. 检查点快照的块序:任一时刻 pending 至多一个(text 到达先 finalize thinking,反之亦然);防御性并存时按 thinking→text 追加。

## Wrong vs Correct

```rust
// Wrong — 变异式快照:flush 会吃掉 pending 状态,流式循环后续块序错乱
flush_pending_text(&mut pending_text, &mut ordered_blocks);
let snap = ordered_blocks.clone();

// Correct — 只读克隆,活累加器不动
let mut snap = ordered_blocks.clone();
if let Some(p) = &pending_thinking { snap.push(thinking_block_from(p)) }
if let Some(t) = &pending_text { snap.push(text_block(t.clone())) }
```

## Tests

- `db/messages_checkpoint_tests.rs`(file-backed 池):upsert 幂等/覆盖、finalize 清 status、delete 守卫、恢复 Step A/B
- `agent/tests_agent_loop/turn_checkpoint.rs`:正常多 turn 零残留、cancel 覆盖检查点、AC4 孤儿修复后第二请求 provider 实收配对 tool_result
- drive.rs 内联 `checkpoint_tests`:时间门与快照纯函数(不起真 1s 等待)
