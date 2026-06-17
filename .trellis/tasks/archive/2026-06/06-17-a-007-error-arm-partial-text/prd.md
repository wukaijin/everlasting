# fix: RULE-A-007 — error arm persist partial text

## Goal

修 DEBT RULE-A-007(P2,Agent Loop):SSE 流中途 error 时,agent loop 的 error arm 直接 `return`,**丢弃已累积的 `text_parts` / `finalized_thinking` / `tool_calls`** —— 这些 delta 已通过 `ChatEvent::Delta` 渲染给前端,但 reload 后从 DB 读不到(与 cancel 路径不对称,cancel 路径会 flush + 构造 assistant_blocks + persist partial turn)。

Fix = error arm 也 persist 已累积的 partial turn,行为对称 cancel 路径。

## 根因(代码定位)

`app/src-tauri/src/agent/chat_loop.rs` SSE 事件循环:

```rust
// L594-600: error 事件处理 —— emit Error 给前端 + 置 had_error
ChatEvent::Error { .. } => {
    ... turn_thinking_done ...
    emit_chat_event_via_sink(&sink, &rid, &event);  // L598 前端已收到 Error
    had_error = true;
}

// L621-623: Done / Error 都 break 出 stream loop
if matches!(event, ChatEvent::Done { .. } | ChatEvent::Error { .. }) { break; }

// L628-630: ❌ error arm 直接 return,丢弃累积内容
if had_error {
    return;   // ← RULE-A-007:不 persist text_parts / thinking / tool_calls
}

// L632-702: cancel 路径(对比)—— flush + 构造 assistant_blocks +
//           CANCELLED_MARKER 追加 + persist_turn 落库 ✓
if cancelled {
    flush_pending_thinking(...);
    tracing::info!("chat: cancelled — persisting partial turn");
}
...
let mut full_text = text_parts.join("");
if cancelled {
    if full_text.is_empty() { full_text = CANCELLED_MARKER.to_string(); }
    else { full_text.push_str("\n\n"); full_text.push_str(CANCELLED_MARKER); }
}
... assistant_blocks 构造(thinking + text + tool_calls)...
if let Err(e) = persist_turn(...).await { emit_persist_failure(...); return; }
emit TurnComplete; messages.push; seq += 1;
```

**不对称**:cancel persist partial ✓,error 丢弃 partial ✗。

## Fix 设计(对称 cancel 路径,最小改动)

### 决策 A:ERROR_MARKER 对称 CANCELLED_MARKER(text 追加,非 metadata)

- 在 CANCELLED_MARKER 定义同处(D3 PRD 提到 import 自 chat_loop.rs:51 对应的模块)新增 `ERROR_MARKER` 常量,文案对齐现有中文风格(如 `"[生成出错中断]"`,implement 时看 CANCELLED_MARKER 实际文案对齐)。
- **否决方案 B(metadata `interrupted: "error"` 字段)**:虽然 D3 加了 metadata 通道,但 cancel 用的是 CANCELLED_MARKER text 追加(既定模式),error 对称用 ERROR_MARKER text 追加才一致;引入 metadata 会让"中断标记"有两种表达(cancel=text marker / error=metadata),增加前端渲染分支。对称性优先。

### 决策 B:error arm persist 失败 = log-only(对称 cancel tool_result persist 失败)

- error 已 emit Error 事件(L598)。若 partial turn persist 再 `emit_persist_failure`,会发出**第二个 terminal Error 事件**,跟已发的 Error 冲突。
- **对称 cancel tool_result persist 失败处理**(chat_loop.rs L719-723 注释明确"loop is about to emit terminal cancelled Done, an Error here would be second terminal event"→ log-only):error arm partial persist 失败也 `tracing::error!`-only,不 emit。
- **否决**"error persist 失败也 emit_persist_failure"(RULE-A-003 正常路径模式):正常路径没 pre-emit Error,所以 emit_persist_failure 是首个 terminal;error 路径已 pre-emit,场景不同。

### 决策 C:error arm 也 emit TurnComplete

- cancel 路径 persist 后 emit TurnComplete(L703-712,给前端 seq + latency 定位 partial message)。error 对称也 emit TurnComplete(seq 指向 partial turn),否则前端收不到 partial message 的 seq 定位。
- Error 事件(L598)+ TurnComplete 并存:Error 通知"出错了",TurnComplete 通知"这个 seq 的 partial turn 已落库 + latency"。前端 listener 各自处理,不冲突。

## Requirements

1. 删 `chat_loop.rs:628-630` 的 `if had_error { return; }`
2. error 时 flush pending thinking + `tracing::info!("chat: errored — persisting partial turn")`(对称 cancel L632-638)
3. 新增 `ERROR_MARKER` const(定义位置对齐 CANCELLED_MARKER),text 追加逻辑加 `else if had_error { ... }` 分支(对称 cancel 的 L650-657 CANCELLED_MARKER 追加)
4. error 走共享的 assistant_blocks 构造 + persist_turn(L672-702),但 **persist 失败 = log-only**(决策 B,不 emit_persist_failure)
5. error persist 成功后 emit TurnComplete(决策 C,对称 cancel L703-712)
6. 不破坏正常(Done)路径 / cancel 路径 / RULE-A-003 emit_persist_failure(正常路径)

## Acceptance Criteria

- [ ] error 中途(error 事件触发后)partial text + thinking + tool_calls 全部落库
- [ ] reload session 后,error 中断的 partial turn 能从 DB 读回(text 含 ERROR_MARKER)
- [ ] empty text error:text_parts 为空时,full_text = ERROR_MARKER(对称 cancel empty → CANCELLED_MARKER)
- [ ] error persist 失败:log-only,**不**发第二个 Error terminal 事件(测试断言 sink call_count 不增加 Error)
- [ ] error 后 emit TurnComplete(seq 指向 partial turn)
- [ ] 正常 Done 路径 + cancel 路径行为不变(回归测试)
- [ ] `cargo check` 0 warning / `cargo test --lib` 全 pass

## Definition of Done

- Tests:集成测试覆盖 error partial persist + empty error + persist 失败 log-only + TurnComplete emit + 正常/cancel 回归
- `cargo check` 0 warning / `cargo test --lib` 全 pass
- spec `.trellis/spec/backend/agent-loop-architecture.md` 同步 error arm persist 语义(§turn 边界 / error-cancel 对称)
- spec `.trellis/spec/backend/error-handling.md` 加 error partial persist + log-only 失败处理(对齐 RULE-A-003)
- DEBT.md RULE-A-007 Status open → closed,Closed At 回填 commit hash,Resolution Notes 引用本 PRD + 设计决策
- `docs/IMPLEMENTATION.md §4` 加 ADR(error arm persist 设计 + 决策 A/B/C)

## Out of Scope

- RULE-A-008(estimate_messages_tokens 重复,P2)—— 独立 task
- RULE-A-009(死代码抑制,P2)—— 独立 task
- metadata `interrupted` 字段方案(决策 A 否决)
- error 中断的 partial turn 前端渲染样式调整(本任务只保证数据落库,前端现有 markdown 渲染 ERROR_MARKER 文本即可;若要特殊样式单独 task)
- 二次取消语义(RULE-A-010,D3 已 spec 偏离声明关闭)
- C3 压缩对 error partial turn 的处理(独立)

## Technical Notes

### 关键复用
- cancel 路径 persist partial(chat_loop.rs L632-714)—— error arm 完全对称
- `CANCELLED_MARKER` const(chat_loop.rs:51 import)—— ERROR_MARKER 定义同处
- cancel tool_result persist 失败 log-only(L719-723)—— error persist 失败处理模板
- `persist_turn` + `emit_persist_failure`(RULE-A-003,L957 helper)—— 正常路径用,error 路径不用 emit

### 关键约束(不变量)
- 改 chat_loop 改 1 处全生效(06-15 RULE-A-006 闭环,chat.rs 是薄 pre-flight)
- error 已 emit Error(L598),任何后续 persist 失败不得再 emit terminal Error(双 terminal 冲突)
- error persist 的 partial turn seq 续号(跟 cancel 一致)
- 不破坏 RULE-A-003(正常路径 persist 失败仍 emit_persist_failure + return)
- 不破坏 RULE-A-004(audit 在 cancel check 之后——本任务不动 audit)

### 关联
- DEBT RULE-A-007(本条,P2 open)
- DEBT RULE-A-003(cancel/正常 persist 失败处理参考,P1 closed `d8ee7d9`)
- DEBT §收尾路径建议第 3 条(D3 收尾时提过 A-007 留独立 task——本 task 即是)
- D3 PR3 ADR(docs/IMPLEMENTATION.md §4 2026-06-17)提到 A-007 仍 open 留独立 task
