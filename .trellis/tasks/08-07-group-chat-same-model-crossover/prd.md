# 群聊同模型串台治理

## Goal

治理群聊**同模型组合**下 moderator 与 participant 角色之间的**记忆串台**问题。
当 moderator 和某个 participant 是同一个模型实例（08-06 PRD D2 定为硬约束必须支持的
组合），该模型在自己的两个角色 turn 之间会混淆"现在我该以谁的身份发言"——
严重时导致生成中断（`[生成出错中断]`），讨论无法推进。

这是 session `8be4687f`（08-07 02:50）DB 取证发现的三层缺陷中的**第三层**，与
第一/二层（`use_skill` 幻觉 + M3 夺权，已在 `08-07-group-chat-toolset-and-identity`
落地修复）**症状相似但根因不同**：第一/二层是"另一个模型主动接管"，靠工具收敛 +
prompt 加固能治；第三层是"同一个模型记忆串台"，prompt 层扛不住，需更深的设计。

> **本文档是 brainstorm 起点**：根因已坐实（DB thinking 字段），不重新调研；
> 直接进方案空间。完整前因后果见 `08-07-group-chat-toolset-and-identity/research/diagnosis.md`
> §3 第三层 + §6 交接要点，原始 DB 证据见同任务 `research/db-evidence-8be4687f.md`。

## Background / 事实（DB 取证，已完成）

取证 DB：`/root/.local/share/dev.everlasting.app/everlasting.db`
取证 session：`8be4687f-3e2d-4023-9d58-91af70abae43`（2026-08-07 02:50）

### session 配置（决定性问题）

- moderator = D4F = **同一个 deepseek 模型**（`model_id b8d0abc2`）。
  - moderator 模型 = session 自身的 `model_id`（`group_chat.rs:128`，`build_group_chat_ctx`）。
  - D4F 的 model 也配置成 `b8d0abc2` → moderator 和 D4F 是**同一个模型实例**。
  - 这是 PRD D2（08-06 任务）定为"硬约束必须支持"的同模型组合场景。
- M3 = 另一个模型（`model_id 42274366`）。

### 关键时序（seq 12 / 14，第三层独有）

| seq | DB speaker | 实际生成模型 | thinking 开头 | 内容/动作 |
|----|------------|-------------|--------------|----------|
| 12 | moderator | deepseek（b8d0abc2） | **"我是 M3,参与者之一"** | `nominate_speaker(D4F)`（thinking 全是角色纠结） |
| 14 | D4F | deepseek（b8d0abc2） | **"等等,我又犯了同样的错误——我是 M3...不应该提名 D4F"** | `[生成出错中断]`（无 tool_use，纯生成崩溃） |

（seq 9 是 M3 夺权，属第二层，已在 toolset 任务治理——本任务不覆盖。）

### 根因（已坐实，DB thinking 字段为证）

1. **同模型实例**：seq 12（moderator turn）和 seq 14（D4F turn）都是 `b8d0abc2` 生成。
   对该模型来说，这两个 turn 的 history 是连续的——**它分不清"现在我该是 moderator 还是 D4F"**。
2. **角色认知崩在 turn 间串台**：seq 12 它以 moderator 身份被调用，thinking 自称 M3，
   纠结一番后仍 `nominate_speaker(D4F)`；seq 14 它以 D4F 身份被调用，thinking 又是
   "我又犯了同样的错误"，反复自我怀疑 → **生成中断**。
3. **不是 API 错误、不是孤儿 tool_use、不是 400**：seq 14 content 无 tool_use 块。
   真相是模型在 thinking 里反复纠结角色，无法收敛到稳定输出生成，被截断/中断。
4. **prompt 层扛不住**：seq 12/14 的 thinking 明确引用了 system prompt 的角色定义
   （说明模型读了），但输入太矛盾，它不认。这是**同模型组合的结构性后果**，不是
   prompt 措辞能完全解决的。

### 关键代码位置（备查）

| 关注点 | 位置 |
|---|---|
| moderator = session model_id（同模型来源） | `group_chat.rs:128`（`build_group_chat_ctx`） |
| moderator/participant system_prompt 全替换 | `group_chat_loop.rs`（override 路径，消费于 `chat_loop.rs:923-924`） |
| 串行时序（`for round { moderator → participant }`） | `group_chat_loop.rs` 主循环 |
| identity-guard prompt（5 层防御之一） | `group_chat_loop.rs` `moderator_system_prompt` / `participant_system_prompt` |
| wire speaker 标注（OpenAI name / Anthropic @name） | `wire.rs` |

## Constraints / 约束

- **D2（08-06，硬约束）**：同模型组合（moderator = 某 participant）必须支持。
  若某方案要改它（如"禁止 moderator 与任何 participant 同模型"），**必须先和用户重新拍板**。
- **不推翻 5 层身份防御**：wire speaker 标注 / `participant_view` 转录隔离 / 工具隔离 /
  identity-guard prompt / moderator 单轮——本任务在这些**之上**补强，不替换。
- **不破坏第一/二层已落地修复**：`group_chat_tool_defs` 白名单、无 streak、prompt 加固
  均保留。本任务的修复要和它们叠加兼容。
- **`MAX_ORCHESTRATION_ROUNDS=30` 不变**（唯一外层硬上限）。
- **D-D 入口护栏 / wire 孤儿自愈不动**（已稳定）。
- **前置依赖**：`08-07-group-chat-toolset-and-identity` 已落地（工具收敛让 history 更干净，
  便于单独观察同模型串台的效果，排除第一/二层干扰）。

## 方案空间（待 brainstorm，未收敛）

diagnosis §6 列出的候选方向（不穷尽，brainstorm 时可增删）：

1. **wire 层角色锚定**：同模型 turn 之间在 wire 请求里强化"当前你是 X"的提示。
   现状已有 OpenAI `name` 字段 / Anthropic `@name` 前缀——是否需要在它们之外再加
   更强的锚定（如每条 message 前缀角色标记 / system 段重复当前角色声明）？
2. **同模型组合约束（违反 D2，需重新拍板）**：在 UI/配置层限制 moderator 不得与任何
   participant 同模型。**注意**：这违反 D2 硬约束，brainstorm 时需和用户确认是否松动。
3. **turn 间隔离**：同模型的不同角色 turn 是否需要某种状态重置，而非共享连续 history？
   （如：moderator turn 和 participant turn 各自维护独立 context window？）但这会
   大改 transcript 架构，权衡复杂。
4. **prompt 层的最终尝试**：在 identity-guard 之外，针对"同模型"这一具体情形，
   给模型更明确的"你现在被调用的角色 = X"的当前-turn 锚定（区别于历史角色）。
   但 diagnosis §3 已判定 prompt 层效果有限——需评估是否值得再做一次。
5. **其他**：brainstorm 时开放。

brainstorm 时需明确区分：哪些方案是**真模型才能验证**的（如 1/3/4 的行为效果）、
哪些是**架构决策可静态论证**的（如 2 的取舍）。

## Out of Scope（本任务不做）

- 第一层（`use_skill` 幻觉）/ 第二层（M3 夺权）——已在 toolset 任务修复。
- `MAX_ORCHESTRATION_ROUNDS=30` 调值。
- 人类抢占插话改 live 介入。
- 真模型端到端验证的**自动化**（仍人工；本任务的修复效果验证依赖真模型重跑
  同模型组合 session，mock 无法复现记忆串台——这是第三层区别于第一/二层的特性）。

## Requirements

- 待 brainstorm 收敛方案后填写。核心方向：让同模型实例在多角色 turn 之间能稳定
  区分"当前发言身份"，避免 seq 14 式的角色纠结→生成中断。

## Acceptance Criteria

- [ ] AC1：根因方案落地（具体条款待 brainstorm）。
- [ ] AC2：既有的 5 层身份防御 + toolset 任务修复（工具白名单/无 streak/prompt 加固）
      不被破坏——既有群聊测试全绿。
- [ ] AC3：`cargo test --lib` + `pnpm test` + clippy 零警告 + vue-tsc 零错误。
- [ ] AC4（人工）：真模型重跑同模型组合 session，确认不再出现 seq 12/14 式的
      thinking 自称他角色 + 生成中断。（非自动化可守，记录为人工验证项。）

## Notes

- **复杂任务**：`design.md` + `implement.md` 在 `task.py start` 前补齐。
- **跨任务关联**：本任务是 session `8be4687f` 三层缺陷的第三层；第一/二层已归档于
  `08-07-group-chat-toolset-and-identity`。取证材料在同任务的 `research/` 下，本任务
  可直接引用，不必复制。
- **验证特殊性**：第三层修复效果**必须靠真模型重跑同模型组合 session 才能验证**
  （mock 无法复现模型的记忆串台）。brainstorm 方案时要考虑"如何在缺乏自动化验证的
  前提下保证修复有效性"——可能需要更强的不变量论证 + 契约测试守结构。
