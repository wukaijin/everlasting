# P0 — C3 tail pair orphan 修复 + 超窗降级

## Goal

关闭 RULE-A-001 + RULE-A-002 两个 P0 数据完整性 bug:
1. **`group_droppable_turns` tail pair 边界可能产生 orphan tool_use/tool_result**(`context.rs:334-381`)— 当 assistant(tool_use) 找不到完整配对时被当 singleton drop,留下孤立 tool_use 或 tool_result,Anthropic 400
2. **`compact_messages` 全 droppable 丢完仍超窗时静默继续**(`context.rs:160-260`)— 单条超大 tool_result 单独构成 tail 时,撞 Anthropic `prompt is too long` 400,DB/内存分叉

## Decisions (ADR-lite)

**Context**:
- RULE-A-001:`group_droppable_turns` 的 `else { /* assistant(tool_use) without a following tool_result */ singleton drop }` 分支(`context.rs:367-374`)。当中间段的 assistant(tool_use) 因 history 被截断 / 上一轮 bug / 边界 race 等原因**没有对应的 user(tool_result)**时,被当 singleton drop,留下**孤立 tool_use**(LLM 上下文不完整,可能撞 400)。代码自身注释 `:341` "Under heavy pressure this leaves an orphan tool_result" 承认边界风险。
- RULE-A-002:`compact_messages` 的 for loop 走完所有 group,如果仍超 target,`tokens_after > target` 直接返回(`:252`),无错误信号。Agent loop 拿到 messages 直接发请求,撞 prompt too long 400。

**Decisions**:

### RULE-A-001 修复

1. **隐式保护 assistant(tool_use)**: 任何含 ToolUse 的 assistant message,如果**找不到完整配对**(下一条不是 user(tool_result) 或者已经到 tail_index 之外),**不 emit group** — 即该 assistant message 转为隐式 protected。修复 line 367-374 的 `else { singleton }` 分支。
2. **规则总结**:
   - `i + 1 < tail_index && pair_with_next` → pair group(已正确)
   - `i + 1 == tail_index && pair_with_next` → skip(已正确,line 360-366)
   - `i + 1 < tail_index && !pair_with_next` → **singleton protected**(line 367-374 改为 skip)
   - `i + 1 == tail_index && !pair_with_next` → skip(已隐式正确,因为 while 条件 `i < tail_index`,不会进入)
3. **修复后 invariant**: assistant(tool_use) **永远**不会独自 drop(要么配对丢,要么不丢),从源头消除 orphan tool_use 风险

### RULE-A-002 修复

1. **`CompactResult` 加 `degradation: DegradationKind` 字段**:
   - `DegradationKind::None` — 正常路径
   - `DegradationKind::StillOver { tokens_after, target }` — 全 droppable 丢完仍超窗
   - `DegradationKind::NoCandidates` — 没东西可丢(保持现有 dropped_count=0 行为)
2. **`compact_messages` 在 line 252 之后加检查**:
   ```rust
   let tokens_after = estimate_messages_tokens(&out).await;
   let degradation = if tokens_after > target {
       tracing::error!(
           tokens_after,
           target,
           "C3 compaction: all droppable exhausted but still over target"
       );
       DegradationKind::StillOver { tokens_after, target }
   } else {
       DegradationKind::None
   };
   CompactResult { messages: out, dropped_count, tokens_before, tokens_after, degradation }
   ```
3. **Agent loop 检查 degradation**:`chat.rs` 在调 `compact_messages` 后,若 `degradation == StillOver`,emit Error 事件("context window exceeded after compression"),**不**发 LLM 请求
4. **不尝试再压**: 不调 LLM summarization / 不进一步 drop head pair(MVP 简化)

**Consequences**:
- 撞 prompt too long 不再静默发请求,而是显式 Error 给 LLM(用户能看到)
- 极压场景(单条 tool_result > target)的真实反应是"无法继续",符合 fail-safe
- `CompactResult` 加字段是 ABI 变化,需同步改所有调用点(`chat.rs` 1 处)
- 历史 review §3.4 "persist 失败 emit Error" 是同模式,本修复是该 pattern 的复述

---

## Requirements

### R1 — group_droppable_turns tail pair 修复

* `app/src-tauri/src/agent/context.rs:367-374` 的 `else { /* without a following tool_result */ singleton drop }` 分支改为 skip
* 注释更新说明新的 invariant: "ToolUse-bearing assistant messages are implicitly protected unless they have a complete (tool_use, tool_result) pair within the droppable middle"
* 新单测覆盖:
  - `group_protects_orphan_tool_use`: middle 段含 assistant(tool_use) + user(text) + user(text) → 第二个 user(text) 是 tail,第一个 assistant(tool_use) 找不到配对 → 不 emit group,只 emit 后面的 singleton
  - `group_drops_complete_pair`: 完整 pair(assistant(tool_use) + user(tool_result))在 middle 中间 → emit pair group
  - `group_protects_tool_use_at_tail`: assistant(tool_use) 紧邻 protected tail(tail 是 user text 非 tool_result) → 不 emit group
  - 现有 11+ 个 context 单测全部仍 pass

### R2 — CompactResult degradation 字段

* `app/src-tauri/src/agent/context.rs:60-73` 加 `pub degradation: DegradationKind` 字段
* 定义 `pub enum DegradationKind { None, StillOver { tokens_after: u32, target: u32 }, NoCandidates }`(放 module 顶部或与 CompactResult 同区域)
* `compact_messages` 三条返回路径填充:
  - line 168-175: trigger 未达 → `DegradationKind::None`
  - line 182-189: 消息太少 → `DegradationKind::NoCandidates`
  - line 199-206: groups 为空 → `DegradationKind::NoCandidates`
  - line 235-242: dropped_count == 0(else branch) → `DegradationKind::NoCandidates`
  - line 252+: 正常 → 上面 decision 3 的逻辑
* 现有所有 `CompactResult { ... }` 构造点加 `degradation: DegradationKind::None` 或正确值

### R3 — agent loop 处理 StillOver

* `app/src-tauri/src/agent/chat.rs` 找调 `compact_messages` 的地方(应该是每 turn 调用一次)
* 在调用后加 match:
  ```rust
  match compact.degradation {
      DegradationKind::None | DegradationKind::NoCandidates => { /* proceed */ },
      DegradationKind::StillOver { tokens_after, target } => {
          // emit Error event to frontend, abort turn
          let msg = format!(
              "Context window exceeded after compression: {} tokens > {} target",
              tokens_after, target
          );
          yield ChatEvent::Error(msg);
          return Ok(());
      }
  }
  ```
* 加单测覆盖此路径(MockProvider 返回 text-only 验证不调 send,DB persist 不写新 turn)

### R4 — 测试覆盖

* 上面 R1 + R3 各列单测
* 加单测 `compact_emits_still_over_degradation`:
  - 构造 messages: head[2] + middle[1 个超大 tool_result 50K token] + tail[1 user text]
  - 设 context_window=1000(强制超窗)
  - 调 `compact_messages`,断言 `degradation == StillOver { ... }` 且 `messages.len() == 3`(没东西可丢,因为唯一的 droppable 是 protected tail)

---

## Acceptance Criteria

* [ ] `group_droppable_turns` 的 `else` 分支不再 emit singleton for orphan tool_use
* [ ] `CompactResult` 加 `degradation: DegradationKind` 字段
* [ ] `compact_messages` 在全 droppable 丢完仍超窗时设 `StillOver`
* [ ] `chat.rs` 处理 `StillOver` 走 Error 事件 + 早返回
* [ ] 至少 5 个新单测(R1 三个 + R2 一个 + R3 一个)
* [ ] 现有 context/chat 单测全部仍 pass
* [ ] `cargo test --lib agent::context agent::chat` green
* [ ] `cargo check` 无新增 warning

---

## Definition of Done

* 上述 Acceptance Criteria 全 ✅
* PR merge 后更新 `docs/_reviews/DEBT.md`:
  - `RULE-A-001`: `Status: closed` + commit + PR
  - `RULE-A-002`: `Status: closed` + commit + PR
* ARCHITECTURE.md §2.5.5 加注 "RULE-A-001 + RULE-A-002 已实施"

---

## Out of Scope

* LLM summarization(C3-v2)
* 进一步压缩 head pair(MVP 拒绝 drop memory synthetic pair,遵守 invariant)
* Streaming token 实时预算扣减
* OpenAI o1+ thinking token 特殊处理
* compression 触发时 UI 标记("context compressed at turn N")— C3 PR2,见 `06-12-c3-context-token` prd

---

## Technical Approach

### 实施步骤

**Step 1: 改 group_droppable_turns (R1)**

```rust
fn group_droppable_turns(
    messages: &[ChatMessage],
    head: usize,
    tail_index: usize,
) -> Vec<(usize, usize)> {
    let mut groups = Vec::new();
    let mut i = head;
    while i < tail_index {
        let m = &messages[i];
        if m.role == Role::Assistant && has_tool_use(m) {
            // Check if the next message forms a complete pair.
            let next_idx = i + 1;
            let pair_with_next = next_idx <= messages.len() // not tail_index; bounds
                && messages.get(next_idx).map_or(false, |n| {
                    n.role == Role::User && has_tool_result(n)
                });
            if pair_with_next && next_idx < tail_index {
                // Complete pair in the droppable middle.
                groups.push((i, i + 2));
                i += 2;
            } else if pair_with_next {
                // pair_with_next && next_idx == tail_index: the
                // tool_result IS the protected tail. Neither side
                // is droppable; skip emitting any group.
                i += 1;
            } else {
                // CHANGED (RULE-A-001 fix): assistant(tool_use)
                // without a complete following tool_result. NEVER
                // drop as singleton — that would orphan the
                // tool_use block from the LLM's view, and if the
                // following message happens to be a tool_result
                // in a future turn, the orphan tool_result would
                // cause Anthropic 400. Treat as implicitly
                // protected (skip emitting group).
                //
                // Rationale: any tool_use-bearing assistant turn
                // whose tool_result pair is not also in the
                // droppable middle must stay — we cannot know
                // whether the tool_result is "next" in a future
                // turn boundary that compaction crossed.
                tracing::debug!(
                    index = i,
                    "C3: protecting tool_use assistant turn (no complete pair in droppable middle)"
                );
                i += 1;
            }
        } else {
            groups.push((i, i + 1));
            i += 1;
        }
    }
    groups
}
```

**Step 2: 加 DegradationKind**

```rust
/// What kind of state did we leave the message list in after
/// compaction? `StillOver` means we ran out of safe candidates
/// (everything left is protected head pair + protected tail +
/// implicitly-protected tool_use turns) but the budget is still
/// over target. The agent loop must NOT send this to the LLM
/// — it would 400 on `prompt is too long`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum DegradationKind {
    /// Either below trigger, or compacted cleanly to target.
    None,
    /// Compaction ran but produced no droppable candidates
    /// (the middle is empty / all implicit-protected).
    NoCandidates,
    /// Compaction dropped everything it could but the list is
    /// still over the target threshold. This is the failure
    /// mode that used to silently send an over-budget prompt.
    StillOver { tokens_after: u32, target: u32 },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CompactResult {
    pub messages: Vec<ChatMessage>,
    pub dropped_count: usize,
    pub tokens_before: u32,
    pub tokens_after: u32,
    pub degradation: DegradationKind,
}
```

**Step 3: 改 compact_messages 返回点**

按 R2 列的 5 处加 `degradation:` 字段。Step 1 注释中已经在 line 252 加判断。

**Step 4: agent loop 处理**

在 `chat.rs` 中找到 `let compact = compact_messages(messages, ctx.context_window).await;` 之类的代码,加 match:

```rust
match compact.degradation {
    DegradationKind::None | DegradationKind::NoCandidates => {
        // proceed with provider.send
    }
    DegradationKind::StillOver { tokens_after, target } => {
        tracing::error!(
            tokens_after, target, session_id,
            "C3: cannot compact below target; emitting Error"
        );
        let msg = format!(
            "Context window exceeded after compaction ({} tokens, target {}). \
             Single tool_result may be too large — try a narrower query.",
            tokens_after, target
        );
        yield ChatEvent::Error(msg);
        // Emit TurnComplete so frontend reload reads DB consistently
        yield ChatEvent::TurnComplete { ... };
        return Ok(());
    }
}
```

---

## Technical Notes

### 关键文件

* `app/src-tauri/src/agent/context.rs:60-73` — CompactResult 定义
* `app/src-tauri/src/agent/context.rs:160-260` — compact_messages
* `app/src-tauri/src/agent/context.rs:334-381` — group_droppable_turns(关键修复点)
* `app/src-tauri/src/agent/context.rs:407-` — tests 块,加新单测
* `app/src-tauri/src/agent/chat.rs` — 调用 compact_messages 处,加 degradation match

### CompactResult 字段加的兼容性

- 加 `degradation` 字段是 struct change,所有构造点必须更新
- 用 `Default` derive 让旧测试构造更简单?或者让所有测试显式 `degradation: DegradationKind::None`(更显式)
- 选显式(更显式,失败时字段不更新容易看出来)

### agent loop 早返回的影响

- 早返回前**必须** emit `TurnComplete` 事件,否则前端 reload 会卡(无 TurnComplete → DB 不一致)
- emit Error + TurnComplete 后 return Ok(()),让 CancellationGuard 正常 Drop 清理
- 不写新 turn 到 DB(context 没变) → frontend reload 读到上一 turn 状态

### 与 RULE-A-001/A-002 同 PR

两条 finding 同在 context.rs,合并 1 PR 节省 review effort。

### RULE-A-003 (persist emit Error) 是同模式

Persist 失败也走 "Error 事件 + 显式降级" 模式,本修复可作为模板。

---

## Research References

* `.trellis/reviews/DEBT.md` — RULE-A-001 + RULE-A-002
* `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` — §2.1 原始论据 + §3.2 跨子系统主题
* `docs/ARCHITECTURE.md §2.5.5` — Context-overflow degradation 设计
* `06-12-c3-context-token/prd.md` — 原始 C3 落地 PRD,本修复是后续 bug 修复
* Anthropic API error: `prompt is too long` (400) — fail-fast 比静默发请求更友好