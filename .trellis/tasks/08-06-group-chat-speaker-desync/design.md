# Design — 群聊角色认知与 speaker 落库错位修复

> PRD: `prd.md`。本设计经过三次迭代修正（见 §0 失败教训），最终锁定根因。

## 0. 失败教训（为什么之前的改动全错了）

三个 session 的迭代暴露了一个被反复忽视的根因，前两轮修复都在治外围：

| session | 代码版本 | mod | M3 | D4F | errors | 表象 |
|---------|---------|-----|----|----|--------|------|
| a6c87247 | 原始 HEAD | 11 | 6 | 4 | 0 | speaker 标签错位 |
| 4a9d3566 | v1（重试+max_turns=1）| 31 | 9 | 4 | 28 | D4F 死循环报错 |
| 093823f3 | v2（max_turns=6+grounding）| 13 | 0 | 1 | 0 | participant 完全不发言 |

- **v1 错误**：加重试循环放大了 orphan tool_use，引入 D4F 28 个 400。
- **v2 错误**：max_turns=1→6 破坏了"nominate 后 turn 结束"语义——moderator 调 nominate 后 run_chat_loop 继续 turn，participant turn 永远不执行（M3=0, D4F=1）。
- **共同错误**：两轮修复都没碰 round-robin fallback——真正的根因。

## 1. 根因：round-robin fallback 机械派错人

DB 证据（session a6c87247，原始 HEAD 代码）：

```
seq 15 moderator 文本："接着主持人的框定来"（没调 nominate）
  → round-robin 派 participants[1%2] = M3
  → 但上下文暗示该 D4F 发言
seq 16 speaker=M3 但内容 "@D4F: 接过 M3 留的合规钩子"
  → M3 被迫模仿 D4F 的口吻 → 角色认知崩溃
```

**moderator 经常用自然语言引导（"接下来请 D4F…"）但不调 `nominate_speaker`。** 原始 fallback 是 `participants[round % len]`——按序号轮转，完全不看 moderator 的意图。派错的人为了回应 moderator 的语境，就模仿别人的口吻发言。

这是从 a6c87247 到 093823f3 三个 session 一以贯之的唯一根因。串行时序（每个 `run_chat_loop().await` 阻塞 + reload）让错配标签沿时间向后滚雪球，最终所有参与者角色崩溃。

## 2. 修复：废弃 round-robin，改为重试 moderator turn（不派人）

### 核心改动（`group_chat_loop.rs`，~40 行净增，基于原始 HEAD）

**保持 max_turns=1 不变**（nominate 后天然结束，不回归）。

只改一个地方：`next_speaker == None` 时的处理。

**旧行为**（原始 HEAD）：
```rust
None => {
    let idx = round % gc_ctx.participants.len();  // 机械轮转
    gc_ctx.participants[idx].name.clone()          // 派错的人
}
```

**新行为**：
```rust
None => {
    no_nominate_streak += 1;
    if no_nominate_streak > MAX_NO_NOMINATE_STREAK { break; }  // 放弃
    continue;  // 回到 for round 顶部重跑 moderator turn，不派任何人
}
Some(n) => { no_nominate_streak = 0; n }  // 提名了，归零
```

**重试时的 prompt nudge**：moderator 的 system prompt 在 `no_nominate_streak > 0` 时追加 `moderator_nudge(streak)`，明确要求"必须调 nominate_speaker 或 end_discussion，不许只输出文本"。

### 为什么这样修能解决所有问题

| 问题 | 怎么解决 |
|------|---------|
| round-robin 派错人 | 不派人，重试 moderator |
| M3 模仿 D4F 口吻 | 不再错配，M3 只在被正确提名时发言 |
| moderator 不调工具 | 重试 + nudge 强制，最多 MAX_NO_NOMINATE_STREAK(3) 次 |
| moderator 探索不够（max_turns=1）| 多个 round 各 max_turns=1，累积探索（round 0 探索文件 A，round 1 探索文件 B…）|
| nominate 后 turn 不结束 | max_turns=1 保持，nominate 后天然结束 → participant turn 正常执行 |

### 关键不变量

- **max_turns=1 不变**——这是 08-04 follow-up 的正确决策，nominate 后 turn 必须立即结束。
- **不再有 round-robin fallback**——moderator 没提名时唯一出路是重试或停止。
- **MAX_ORCHESTRATION_ROUNDS=30 不变**——仍是外层硬上限；`MAX_NO_NOMINATE_STREAK=3` 是"连续不提名"的子上限。

## 3. wire.rs 孤儿自愈（保留，独立修复）

session 4a9d3566 暴露的 D4F `[生成出错中断]` 根因是 orphan tool_use（intercept 的 error tool_result 在 max_turns 退出时没 persist）。wire 层原检测到孤儿只打日志照发请求 → deepseek 400。

**修复**（`wire.rs:chat_request_to_wire`）：检测到孤儿时自动注入合成 tool_result（`is_error=true`），满足 Pair Atomicity，请求不再 400。这是防御性修复——即使 DB 有孤儿（任何来源），对话都能继续。

测试：`chat_request_to_wire_heals_orphan_tool_use_with_synthetic_result` + `chat_request_to_wire_no_heal_when_history_is_clean`。

## 4. 时序模型（不变）

群聊严格串行：`for round { moderator.await → participant.await }`。每个 `run_chat_loop().await` 阻塞到落库才返回。互见性靠"落库 + reload"。本设计不动时序。

## 5. 验证

- `moderator_nudge_forces_tool_call`：nudge 含两工具名 + "Do NOT output only text" + streak 计数 ✅
- `max_no_nominate_streak_is_sane`：1 ≤ streak ≤ 5 ✅
- 原始 11 个 group_chat 测试全绿（回到 HEAD 测试集）✅
- wire.rs 33 个测试全绿（含 2 个自愈测试）✅
- clippy 零警告 ✅

## 6. 未做（P1）

- **max_turns 退出 persist 残留 result_blocks**（chat_loop.rs:4445）：孤儿的根本来源。wire 自愈已止血，此项降为 P1 单列。
- **grounding 共享机制**（之前的 1+3 prompt）：等 round-robin 修复验证后，视需要再加。当前 moderator 在多 round 探索后自然产出背景文本，参与者 reload 能看到。
