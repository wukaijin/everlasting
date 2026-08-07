# 群聊 per-role history 隔离(治同模型串台)

## Goal

治理群聊**角色记忆串台**——任一角色的 LLM 上下文里共存多条不同 `name` 的
`role: assistant` 消息(含各自 thinking + signature),模型把所有 assistant 历史
当作"自己过去的输出",多身份 assistant 尤其是带 reasoning 的高认知负荷,
**同模型时**(推理风格一致)把负荷推过临界点 → 串台/生成中断
(session `8be4687f` seq 12/14)。

**修复手段:per-role history 隔离(形态 B)。** 编排器为每个角色组装一份独立 LLM
上下文——只含它自己的 assistant 消息(含完整 thinking signature,回传链不断)+
他人发言改写为 `role: user` 投递。他人 thinking 不进当前上下文,既消除认知根因,
又绕开 Anthropic signature 400 硬约束。

> 原 brainstorm 期的开放方案空间(wire 锚定/禁同模型/turn 隔离/物理 session 分表)
> 已收敛——详见 `research/design-draft.md` 的取舍论证。本 PRD 反映定稿方案。

## Background / 事实(查证坐实)

### 根因(代码 + DB 双重证据)

1. **群聊单 session_id 共享 history**:`run_group_chat_loop` 对所有 speaker 传同一个
   `session_id.clone()`(`group_chat_loop.rs:569` moderator、`:763` participant);
   `reload_messages`(`:421-451`)从 `m.content` 反序列化,**thinking 块(含 signature)
   原样保留** → 所有角色的 assistant 消息(含各自 thinking)混在同一份 history。
2. **participant_view 只剥仲裁,留所有 assistant**:`group_chat_loop.rs:231-313`
   剥掉 moderator 的 nominate/end tool_use 对,但**保留所有角色的 text + thinking
   作为 `role: assistant`** → 这是串台源。
3. **signature 400 硬约束**:`ContentBlock::Thinking { signature }`(`types.rs:92`)
   注释明说 "MUST be echoed back verbatim — otherwise the API returns 400";
   `anthropic.rs:593-606` 记录丢空 signature thinking 触发 turn-2 400 的血泪史。
   → **不能在一个共享 history 里选择性净化他人 thinking 块**。

### 为什么认知根因 = "多身份 assistant 共存"

LLM 不认 `name` 字段,只认 `role`。所有 `role: assistant` 历史都被当作"我自己过去
生成的"。同模型时各角色的 reasoning 风格一致,模型更难区分"现在该走哪个身份的
推理流" → seq 14 反复纠结角色 → 生成中断。**不同模型时**风格差异提供隐含区分信号,
未过临界点(但 M3 seq 9 夺权证明不同模型也会串,只是症状不同)。

→ 本方案**治根因(多身份 assistant 共存),不治诱因(同模型)**。同模型/不同模型
串台都被消除。

### 为什么选形态 B(逻辑 history)而非形态 A(物理 session)

两者认知隔离效果等价(provider 视角下 messages 数组一样),差异在落地:
- **形态 A**(物理分表):每角色独立 session_id + messages 表,DB 加 parent_session_id,
  前端 transcript/IPC/流式全重写。冲击 5 领域,数周工程量。
- **形态 B**(逻辑 history,选定):DB 保持单 session_id(前端/IPC/流式/DB schema 全
  不动),只在编排器层组装 per-role history。冲击 1 领域(编排器),可回滚。

**关键:形态 B 也能绕 signature**——signature 约束的是"同一对话上下文里 assistant
消息的完整性",而上下文边界由传给 provider 的 messages 数组决定。形态 B 从一开始
不把他人 thinking 放进当前角色数组,他人是 user 输入,无 signature 回传义务。

## Requirements

### R1 — 新增 `role_history` 组装器(取代 `participant_view`)

新增 `fn role_history(full: &[ChatMessage], current_role: &str) -> Vec<ChatMessage>`
(`group_chat_loop.rs`),一遍扫描状态机,按角色归属重写:

| 消息类型 | 处理 |
|---|---|
| 人类原始 user 消息(speaker == None,原始 prompt) | **保留** role:user |
| 当前角色的 assistant 消息(speaker == current_role) | **原样保留**(role:assistant + 全部 blocks,含 Thinking+signature) |
| 他人 assistant 消息(speaker != current_role) | **改写为 role:user + 单 Text block** `"@<speaker>: <text>"`;**丢弃 Thinking 块** |
| 他人 assistant 含 ToolUse(调研工具) | 标记 pending,整对(其 tool_result 行)剥离;仅 text 部分(若有)改写为 user |
| 当前角色自己的调研 tool 对(read_file 等) | **原样保留**(role 配对合法) |
| moderator 仲裁对(nominate/end,对非 moderator 角色) | **剥离**(沿用 participant_view 既有不变量) |

### R2 — 调用点切换

- moderator turn(`group_chat_loop.rs:536-540` reload 后):`full` →
  `role_history(&full, "moderator")`。
- participant turn(`:715-716`):`participant_view(&full)` →
  `role_history(&full, &participant.name)`。
- **删除** `participant_view`(`:231`)+ `participant_view_row`(`:270`)+ 对应测试。

### R3 — 工具结果语义:不共享

他人的 tool_use/tool_result 整对剥离,当前角色看不到他人调了什么工具/原始数据,
**只通过他人文本发言得知结论**。这是用户初始表述("不共享工具调用结果")的落地,
也是隔离优先原则的体现。影响:
- moderator 连续调研不发言时,participant 不知调研内容。
- 两人查同文件可能重复调用(token 浪费,可接受)。
- 好处:context 完全干净,无串台。

### R4 — 不动的(关键不变量)

- **DB schema / persist_turn / load_session** — 单 session_id,speaker 列不变。
- **前端** `messagesBySession` / `RequestState` / `load_session` IPC / 流式聚合 — 不动。
  每角色 turn 仍是单 request 上的一个 turn,speaker 事件/chip 渲染不变。
- **run_chat_loop** — 收到的就是组装好的 messages,不关心来源。
- **turn_state / nominate_speaker / end_discussion 拦截** — 不动。
- **group_chat_tool_defs**(08-07 R1)— 不动。
- 08-07 落地的所有修复(工具白名单/无 streak/prompt 加固)— 不破坏。

## Out of Scope

- 形态 A(物理 session 分表) — 仅在有按角色独立审计/重放 DB 诉求时考虑,当前无证据。
- `MAX_ORCHESTRATION_ROUNDS=30` 调值。
- 真模型端到端验证自动化 — 仍人工(mock 无法复现串台,但本方案的不变量可契约级守)。
- turn 间并发/隔离状态重置 — 串行时序不变。

## Acceptance Criteria

- [ ] AC1(R1):`role_history` 实现且按上表重写;当前角色 assistant 消息(含
      Thinking+signature)原样保留,他人 assistant 改写为 user+text 且不含 Thinking。
- [ ] AC2(R2):moderator/participant 两处调用切换;`participant_view` 一族删除。
- [ ] AC3(R3):他人 tool_use/tool_result 整对剥离;当前角色自己的 tool 对保留。
- [ ] AC4(R4):DB schema / 前端 / run_chat_loop / turn_state / tool_defs 均无改动
      (diff 审查确认冲击面只在编排器)。
- [ ] AC5:`role_history_*` 测试覆盖上表所有重写规则 + signature 回传契约
      (当前角色 Thinking 块 signature 完整)。
- [ ] AC6:既有群聊测试(`participant_view_*` 迁移为 `role_history_*` + identity_contract
      + view 不变量)全绿。
- [ ] AC7:`cargo test --lib` + `pnpm test` + clippy 零警告 + vue-tsc 零错误。
- [ ] AC8(人工):真模型重跑同模型组合 session,确认不再出现 seq 12/14 式角色
      纠结/生成中断。

## Constraints / 约束

- **D2(08-06,硬约束)**:同模型组合必须支持。**本方案不碰 D2**——它治根因
  (多身份 assistant 共存),不治诱因(同模型),D2 完整保留。
- **5 层身份防御**:本方案在 view 层(升级 participant_view → role_history)补强,
  其余 4 层(wire speaker 标注 / 工具隔离 / identity-guard prompt / moderator 单轮)不动。
- **signature 回传**:当前角色自己的 Thinking 块必须原样保留(AC5 契约守)。

## 已定决策

- **形态选择 = B(逻辑 history 隔离)** — 主方案。代价 1/5,效果与形态 A 等价,可回滚。
- **工具结果语义 = 不共享**(R3)— 他人 tool 对整对剥离,结果靠发言转述。
- 主方案采用工程判断推进(brainstorm 阶段方案空间开放,基于调研收敛)。

## Notes

- **复杂任务**:`design.md`(技术设计,见 `research/design-draft.md` 待迁移)+
  `implement.md`(执行清单)在 `task.py start` 前补齐。
- **跨任务关联**:本任务是 session `8be4687f` 三层缺陷的第三层;第一/二层已归档于
  `08-07-group-chat-toolset-and-identity`。本方案和 toolset 任务的修复叠加兼容(R4)。
- **验证策略**:根因(多身份 assistant 共存)可契约级守(AC1/AC5),不依赖真模型;
  最终行为效果(AC8)仍需真模型重跑。
