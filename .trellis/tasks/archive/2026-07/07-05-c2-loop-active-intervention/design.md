# Design — C2+ 循环检测主动干预

> PRD: `.trellis/tasks/07-05-c2-loop-active-intervention/prd.md`
> 父 spec: `backend/agent-loop-architecture.md`（C2 章节扩展 C2+）+ `backend/tool-contract.md`（AuditKind）

## 1. 架构边界

### 不动
- `loop_detection.rs`（纯函数层，31 单测已稳定）
- `question_store.rs`（QuestionStore / ToolQuestionPayload / QuestionResponse 已稳定，C2+ 纯 caller）
- `ask_user_question.rs` tool（LLM-driven 路径不变）
- `MAX_TURNS=200`

### 改动文件清单

| 文件 | 改动 | 说明 |
|---|---|---|
| `agent/chat_loop.rs` | **核心** | 加 `loop_hit_count`；detect 后 count++ / 触发询问 / select! 等待 / 三分支处理；emit Done 加 `loop_terminated` stop_reason |
| `agent/permissions/audit.rs` | enum + as_str + record fn | 加 `AuditKind::LoopIntervention` 变体 + `as_str()` 返 `"loop_intervention"` + `record_loop_intervention_audit` helper（仿 `record_message_resend_audit`） |
| `agent/subagent/dispatch.rs` | worker break 告知父 | 检测 worker `Done.stop_reason="loop_terminated"` → `format_dispatch_result_with_model` 追加「worker 因循环被终止」行 |
| 前端 `AuditLogModal` / audit store | kind 分发 | 加 `loop_intervention` case 渲染 payload |
| `db/permissions.rs` | 无 | `record_audit_event` 已通用，新 AuditKind 走现成路径（kind 列 TEXT，无 migration） |

## 2. 数据流（C2+ 状态机）

```
每轮 turn（chat_loop.rs:1968-1984 接入点）:
  loop_window.push(this turn's tool_calls)
  verdict = detect(loop_window)
  ┌─ verdict == None ──→ loop_hit_count = 0; 走原 hint 注入路径（None 无 hint）
  │
  └─ verdict != None ──→ loop_hit_count += 1
       ┌─ loop_hit_count < 3 ──→ 走原 hint 注入路径（loop_hint = verdict.hint_text()）
       │
       └─ loop_hit_count >= 3 ──→ 【C2+ 触发】
            payload = 固定文案（PRD R2）
            audit(action="asked")
            match QuestionStore::register(session_id, "loop_intervention_<turn>", payload) {
              Ok(rx) => {
                emit_tool_question(payload)
                select! {
                  _ = token.cancelled() => {
                    QuestionStore::remove(session_id)
                    emit Done(stop_reason="cancelled")   // 用户主动 Stop
                    return
                  }
                  resp = rx => match resp {
                    Answered(["终止 loop"]) | Cancelled => {
                      audit(action="terminated")
                      emit Done(stop_reason="loop_terminated")  // C2+ 干预终止
                      return
                    }
                    Answered(["继续"]) => {
                      audit(action="continued")
                      loop_hit_count = 0
                      loop_hint = enhanced_hint_text()  // 增强 hint，注入 result message
                      // 继续走 result_blocks 构造 + 回填 LLM
                    }
                  }
                }
              }
              Err(AlreadyPending) => {
                // LLM 并发 ask_user_question 占了 slot → 本轮跳过，不阻塞
                tracing::warn!("C2+ skipped: question already pending");
                // 走原 hint 注入路径
              }
            }
```

**关键时序**：count + register + select 在 **detect 之后、`result_blocks` 构造之前**。即询问期间这一轮的 tool 已执行完（副作用已发生），但 tool_result 尚未回填 LLM。
- 「终止」→ 不回填，直接 break（语义同 cancel：tool 副作用已发生，但不继续喂 LLM）。
- 「继续」→ 回填 tool_result + 增强 hint，LLM 下一轮看到 hint + 用户介入信号。

## 3. 关键契约

### 3.1 QuestionStore 复用
- `register(session_id, "loop_intervention_<turn>", payload)` —— `tool_use_id` 用 `loop_intervention_<turn>` 前缀（前端 `<AskUserQuestionCard>` 不依赖 tool_use_id 匹配，只渲染 payload）
- 复用 `emit_tool_question`（`ChatEventSink` trait，sync）+ `resolve_tool_question` command + 前端 `<AskUserQuestionCard>` + `get_pending_question` session-switch 恢复
- **caller 是 chat_loop 顶层**（harness-driven），非 tool execute_blocking 路径

### 3.2 AuditKind::LoopIntervention payload
```json
{
  "hit_count": 3,
  "verdict_kind": "hard",   // "hard" | "soft"
  "action": "asked",        // "asked" | "terminated" | "continued"
  "run_id": null            // PR1 实现: Option<&str> 占位，主 loop 传 None，worker 未来 audit surface 传 Some
}
```
落表点：
- `asked`：register 成功后立即落
- `terminated`：用户「终止」分支
- `continued`：用户「继续」分支
- **worker break 不落**（worker 无独立审计 surface，worker run 自有 transcript）

> PR1 偏离记录：design 原 3 字段，实现加了 `run_id: Option<&str>`（additive，不违反 R4 三字段契约）。理由：future-proofing，避免未来 worker audit surface 扩展时改签名。

### 3.3 stop_reason
| 场景 | stop_reason |
|---|---|
| 用户「终止 loop」 | `"loop_terminated"` |
| 用户点 Stop（session cancel） | `"cancelled"`（现有） |
| 用户「继续」后烧满 MAX_TURNS | `"end_turn"`（现有） |

### 3.4 worker dispatch_result（R5）
worker 复用 `run_chat_loop` → 触发时走 `Done(stop_reason="loop_terminated")`。父侧检测 → 在 result content 追加：
```
[loop terminated: worker 因循环重复操作被自动终止，未完成全部步骤]
```
父 agent 看到此提示后自行决策重试 / 换路径 / 接受。

> **PR3 实现偏差**（trellis-check 评估「可接受」+ 有代码注释）：
> 1. 检测点不在 `format_dispatch_result_with_model` 内（不加第 5 参），而在 `run_subagent` 通过 sink 的 `was_loop_terminated: AtomicBool`（同 `was_cancelled` / `had_error` 模式）caller-append，跟 `worker_changes_summary` 同款 tail-append。
> 2. worker 路由复用 `SubagentStatus::Incomplete`（不加 `LoopTerminated` 变体），避免 DB CHECK 约束 + migration + 前端 drawer 状态胶囊波及。
>
> 两偏差都符合 R5「worker 直接 break + 告知父」语义。

## 4. 关键 Trade-off

### 4.1 为什么 chat_loop 顶层 select! 而非新 tool？
**选 A（chat_loop 顶层）**：harness 在 detect 命中后直接 register + select，等用户响应。
**否决 B（合成 ask_user_question tool_use）**：要伪造 LLM 没发起的 tool_use + tool_use_id，语义混乱；且 tool 路径假定「LLM 决定何时问」，C2+ 是「harness 决定何时问」，caller 不同。
**否决 C（新独立 oneshot + 新 event）**：复制一套 question 机制，违反 DRY；前端要新组件。

A 复用整条 QuestionStore + IPC 链 + 现成 `<AskUserQuestionCard>`，零新前端组件。

### 4.2 为什么 worker 直接 break 而非 round-trip 问父？
**选 A（直接 break + 告知父）**：worker 在 dispatch_result 告知父，父决策。
**否决 B（WorkerAskBanner round-trip）**：
- worker 用父 session_id 作 QuestionStore key（L3c 笔记），与父自己 ask_user_question 竞 `AlreadyPending` slot
- 打扰用户（worker 死循环烧的是 worker 自己的预算）
- worker `max_turns` 更短，烧钱风险本就小

A 让 worker 自有限损，父 agent 拿到「worker 被终止」信号后自主决策，符合「worker 是父的 delegated tool」语义。

### 4.3 为什么 detect 接入点不动？
C2+ 复用 C2 的 `chat_loop.rs:1968-1984` 接入点（每轮 tool 执行后一次）。不在 `loop_detection.rs` 加 count —— 纯函数层保持无状态，count 是 caller 侧 loop-local 状态。

## 5. 兼容性

- **B6 worker 继承**：worker 复用 `run_chat_loop` → 自动继承 `loop_hit_count`（同 `loop_window` 跨 turn 累积）。worker 触发走 R5 直接 break。
- **AlreadyPending 降级**：LLM 并发 `ask_user_question` 占 slot 时，C2+ `register` 返 `AlreadyPending` → 本轮跳过（不阻塞 loop），下轮再试。降级路径仍走原 hint 注入。
- **session cancel race**：select! cancel arm 优先（biased），保证用户 Stop 期间不悬挂 pending。`QuestionStore::remove` 清 slot。
- **session-switch**：QuestionStore 设计上**不**在 session-switch 时取消（R9-R11，pending 跨 switch 存活）。C2+ 询问期间用户切走再切回，`get_pending_question` 恢复 card。
- **DB 无 migration**：AuditKind 走现成 `kind` TEXT 列 + `record_audit_event` 通用路径。

## 6. 测试策略

### 单测（chat_loop 集成测试，tests_agent_loop.rs）
- `c2plus_terminates_after_3_consecutive_hard_loops`：构造 5 连相同 `read_file` → 第 3 轮 detect 命中触发询问 → mock QuestionStore resolve「终止」→ assert break + stop_reason="loop_terminated"
- `c2plus_continue_resets_count`：3 轮命中 → resolve「继续」→ assert count=0 + 增强 hint 在 result + loop 继续
- `c2plus_none_resets_count`：3 命中后 1 轮 None → count=0（需再次 3 连命中才触发）
- `c2plus_session_cancel_during_ask`：询问期间 cancel token → assert stop_reason="cancelled" + QuestionStore 清 slot
- `c2plus_already_pending_skips`：预占 QuestionStore slot → C2+ 本轮跳过 + 走原 hint
- `c2plus_worker_breaks_and_notifies_parent`：worker run 5 连相同 → assert worker stop_reason="loop_terminated" + dispatch_result 含「循环被终止」

### audit.rs 单测
- `record_loop_intervention_audit` payload 正确（hit_count/verdict_kind/action）

### 前端 vitest
- AuditLogModal 渲染 `loop_intervention` kind（payload 分发）

## 7. 风险点

- **select! 在 chat_loop 顶层阻塞**：询问期间 agent loop 暂停（等用户）。这是预期行为（同 ask_user_question tool），但要注意 cancel token 必须 select 进去，否则用户 Stop 不响应。
- **loop_hit_count 跨 worker 共享还是独立**：worker 在自己的 `run_chat_loop` 调用里，`loop_hit_count` 是该调用的 local，与父的 count 独立（正确：worker 是独立 loop）。
- **增强 hint 文案**：「继续」后注入的 hint 要比原 hint 更强（明确告诉 LLM 用户已介入）。文案在 design 阶段不定死，实现时定（建议含「用户已确认你在循环，请立即改变策略或停止」）。
