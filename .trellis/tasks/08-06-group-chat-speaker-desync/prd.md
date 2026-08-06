# 群聊角色认知与 speaker 落库错位修复

## 背景 / 证据

诊断来源：session `a6c87247-e37f-4ec5-82bb-3a8406a765a9`（"简单聊聊这个项目的不足"，2026-08-06，群聊，fortune-telling 项目）。

参与者配置（`sessions.metadata`）：
- **M3** → `MiniMax-M3`（model_id `42274366…`）
- **D4F** → `deepseek-v4-flash`（model_id `b8d0abc2…`）
- **moderator** → session 自身 model = `deepseek-v4-flash`（**与 D4F 同模型**）

### 观测到的缺陷

1. **moderator 全程只提名了 1 次**。整个 session 仅 seq 12 有一次 `nominate_speaker(name=M3)`；seq 27 才 `end_discussion`。中间 5+ 轮 moderator 都没调工具，靠 `run_group_chat_loop` 的 round-robin 兜底（`group_chat_loop.rs:498-511`）派单。

2. **speaker 标签与内容严重错位**（核心 bug）：

| seq | DB speaker | 内容开头 | 实际应是 |
|----|-----------|---------|---------|
| 14 | M3 | `@M3: 谢谢主持人的框定...` | M3 @ 自己（自相矛盾）|
| 15 | moderator | `，接着主持人的框定来。我的盘点里...` | 参与者在接话 |
| 16 | **M3** | `@D4F: 接过 M3 留的合规钩子...` | **D4F** 在说话 |
| 20 | M3 | `@moderator: 两位参与者跑完三轮...` | M3 自称做主持人收口 |

3. **模型角色认知崩溃**（seq 24 / seq 26 的 thinking 字段自述）：
   - seq 24 (M3): *"Looking back: 'You are D4F...' Wait, is it M3 or D4F?"*
   - seq 26 (D4F): *"the assistant has been playing M3 all along, and user role has been playing both moderator and D4F"*

4. **`###TASK_COMPLETED###` 与 `end_discussion` 双重结束语义**：seq 21/24/26 都输出了 `###TASK_COMPLETED###`（裸字符串），但它不触发 orchestrator break；真正的 `end_discussion` 直到 seq 27 才调用。导致讨论该结束时多绕了好几轮。

## 根因分析

**主因：speaker 标签是 orchestrator 按"这轮该谁"机械贴的，内容是模型自由生成的，两者没有同步约束。**

当 moderator 在 `max_turns=1` 约束下不调 `nominate_speaker`/`end_discussion` 而直接输出文本时：
- 这段文本被当 moderator 消息落库（`speaker=Some("moderator")`，`group_chat_loop.rs:480`）
- orchestrator 走 round-robin 兜底派下一个人
- 但兜底派的人和那段"被当 moderator 落库的文本"在语义上可能是同一个人连续说话
- 下一个 speaker reload 历史时看到自相矛盾的 speaker↔content 映射

串行时序（`run_chat_loop().await` 逐个阻塞）使错位标签**沿时间向后滚雪球**：前一步的错位进入下一步的 history，越叠越乱，最终模型在 thinking 里彻底迷路。

**次因：**
- `max_turns=1`（`group_chat_loop.rs:454`）对 moderator 过紧，模型来不及思考后调工具就 turn 结束。
- round-robin 兜底是静默的（只 `tracing::warn`），参与者不知道自己是兜底发言。
- moderator 与 D4F 同模型，模型在 history 里看到"自己（deepseek）"的话同时出现在两个角色名下，边界更糊。
- identity-guard block（`group_chat_loop.rs:268-298`）写得对，但扛不住 history 层的硬错位——模型在 thinking 里明确引用了 guard block，说明它读了，只是输入太矛盾。

## Goal

让群聊里每条落库消息的 `speaker` 字段与其实际发言内容语义一致，确保任何 speaker reload 出来的历史都不包含自相矛盾的角色归属，从而让参与者的 system prompt + identity-guard 能真正生效。

## Requirements

### R1 — moderator 不调工具时不能静默 round-robin（主修复）
当 moderator turn 结束且 `next_speaker=None && !discussion_ended` 时，**当前行为**（静默 round-robin 派下一个人 + 把 moderator 文本当 moderator 落库）是错位的根源。**必须改**，候选方案见"待决策 D1"。

### R2 — speaker 标签必须反映真实发言者
无论走哪条路，落库的 `speaker` 字段必须和该 turn 实际生成内容的模型身份一致。不允许"标成 moderator 但内容是参与者在接话"。

### R3 — 单一结束语义（修订：`###TASK_COMPLETED###` 是幻觉）
调研确认：`###TASK_COMPLETED###` 在代码库中**根本不存在**，既不在任何 prompt 模板里，也不被任何代码检测。它是 LLM 在角色认知崩溃时凭空生成的字符串（见 research）。真正的结束信号只有 `end_discussion` 工具调用（`end_discussion.rs:16` + `chat_loop.rs:3746-3765` 拦截）。

因此 R3 的本质是：**让模型可靠地用 `end_discussion` 结束**，而不是靠幻觉编结束标记。修复靠 R1（moderator 重试 + prompt 强制）+ 可选的 tool_choice 强制；不需要"移除"任何东西。

### R4 — 串行时序不变
群聊严格串行（逐个发言，`run_chat_loop().await` 阻塞）是互见性的基础，**不改成并发**。本任务只修每一步的标签一致性，不动时序模型。

### R5 — 不破坏现有 group_chat 测试
`tests_group_chat.rs` + `group_chat_loop.rs` 内的 `participant_view` / `participant_tool_defs` / prompt 回归测试必须继续通过。

## Acceptance Criteria

- [ ] AC1: moderator turn 结束时若未提名且未结束讨论，不再静默 round-robin（按 D1 决策落地方案执行）。
- [ ] AC2: 任意 speaker reload 出来的历史里，不存在"`speaker=X` 但内容明显是 Y 在说话"的自相矛盾条目（构造回归测试覆盖）。
- [ ] AC3: moderator 能可靠地用 `end_discussion` 结束讨论，不再依赖（也不产生）`###TASK_COMPLETED###` 之类的幻觉结束标记。重跑群聊时模型不再输出该字符串（人工验收）。
- [ ] AC4: 用 fortune-telling session 的配置（M3+D4F+moderator=deepseek）重跑一次群聊，M3/D4F 的 thinking 不再出现"is it M3 or D4F"式的角色困惑（人工验收）。
- [ ] AC5: `cargo test --lib` 群聊相关测试全绿；`pnpm test` 前端群聊相关测试全绿。

## 已定决策

### D1 — moderator 不调工具时的处理策略 ✅
**用户拍板：(a) 重试 moderator turn + 强制 tool call。**

调研补充：
- 当前 `max_turns=1`（`group_chat_loop.rs:454`）下 moderator **无法**"先思考再调工具"——一次 LLM 调用里二选一。
- **方案 A（外层重试，保持 max_turns=1）比方案 B（max_turns=2）更符合现有架构**：外层 `for round` + `turn_state` + `reload_messages` 本就是为"每轮独立 moderator turn"设计的；方案 B 与 `group_chat_loop.rs:444-453` 明确记录的设计决策（max_turns=1 是为堵 turn-2 填充文本身份混淆）直接冲突，等于回退。
- 重试 turn 可在 system prompt 里追加"上次没提名，必须调 nominate_speaker / end_discussion"。
- tool_choice（OpenAI `tool_choice:"required"` / Anthropic `tool_choice:{type:"any"}`）**当前完全没实现**（全仓 grep 零命中）。引入需动 `ChatRequest` + `Provider::send` 签名 + wire 层 + 两个 provider。**作为可选增强，不阻塞 R1 主修复**——先用"prompt 强制 + 重试"落地，tool_choice 列为 P2 增强。

### D2 — 同模型组合 ✅
**用户拍板：必须支持任意同模型组合**（moderator 与参与者同模型、参与者之间同模型）。"同模型"不是要避免的问题，是硬约束。意味着角色区分只能靠 prompt + speaker 标签，不能靠模型差异——这反而强化了 R1/R2 的必要性。

### D3 — round-robin fallback
重试达上限后仍需最终兜底，round-robin 保留为"最后手段"，但须显式标注（见 design）。

## Notes

- 串行时序是对的，不要改并发模型。
- identity-guard block（`group_chat_loop.rs:268-298`）不用大改，它的问题是"输入太矛盾"，修了 R2 它就能生效。
- 前端 `streamController.ts` 的 groupChat 逐轮 placeholder 逻辑（92-113 行）是配套时序，本次不动。
- 诊断 SQL / DB 证据见 `research/`（待补）或直接查 session `a6c87247…`。
