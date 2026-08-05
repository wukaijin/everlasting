# 根治群聊 tool_result 跨 speaker 重复持久化导致 OpenAI 400

> **⚠️ 已被替代(2026-08-05)**:本任务的 `user_message_matches` 启发式守卫只覆盖
> `run_chat_loop` 的用户消息持久化点(`chat_loop.rs:1001`),DB 仍出现重复 —— 证明
> "在 reload 全部消息的策略下打补丁"无法收敛。**整体由
> `08-04-group-chat-orchestration-rewrite` 系统重写替代**(commit `ff5e9ff`):重写在
> 编排层(`group_chat_loop.rs`)做入口持久化去重,保证"进入 `run_chat_loop` 的尾部
> user 消息一定是本轮新的",从根上消除了重复。
>
> 本任务的 `user_message_matches` 守卫(及 7 个单测)**作为防御性兜底保留在
> `chat_loop.rs`**,重写方案的注释(`group_chat_loop.rs:1004`)显式引用了它保护的
> 不变量。任务目标(根治 OpenAI 400)已由重写达成,本任务归档。

---

## Goal

## Goal

修复群聊(group_chat)编排中,`run_chat_loop` 把 reload 进来的、已落库的
user-role 消息(尤其是 tool_result)当成新消息重复持久化,产生孤立
tool_result,最终被 OpenAI 以 400 拒绝("Messages with role 'tool' must be
a response to a preceding message with 'tool_calls'")的根因。

目标是从 `run_chat_loop` 的 user 消息持久化逻辑层面根治,而不是在
group_chat 层打补丁绕过(止血方案),让该持久化点不再依赖"尾部 user
消息一定未落库"这个隐式不变量。

## Background / 根因(已确认)

`run_chat_loop`(`chat_loop.rs:894-985`)在每次调用开头执行:

```rust
if let Some(last_user) = messages.iter().rev().find(|m| m.role == Role::User) {
    let msg = last_user.clone();
    if !skip_persist {
        crate::db::persist_turn(&db, &session_id, msg.role, &msg.content, seq, ...).await;
    }
    seq += 1;
}
```

它**无条件**把 `messages` 尾部最后一条 user-role 消息当新消息写一行。

- **普通单 agent 聊天不中招**:不是因为有去重机制,而是依赖隐式不变量 ——
  前端发送前把新 user 文本 append 到 history 尾部(`chat.ts:1206`),
  所以尾部 user 消息确实是未落库的新消息,persist 一次是对的。
- **群聊中招**:`run_group_chat_loop` 在 speaker 之间调
  `reload_messages(&db, &session_id)`(`group_chat_loop.rs:343/418`)
  从 DB 重载全部消息(含上一 speaker 持久化的 tool_result,role=user)。
  下一个 speaker 的 `run_chat_loop` 收到的 `messages` 尾部 user 消息是
  **已落库的 tool_result**,被当新消息再写一行 → 重复 → 孤立 tool_result
  → OpenAI 400 → 后续每轮 `[生成出错中断]` 死循环到 MAX_ORCHESTRATION_ROUNDS。

`loaded_session.messages`(`chat_loop.rs:463` 已加载,目前只用来算 `seq`)
正好可以作为"这条消息是否已在 DB"的判据,无需新参数/新查询。

## Requirements

### R1 — 修复 `run_chat_loop` user 消息持久化点
- R1.1 在 `chat_loop.rs:894` 的持久化块,增加判断:若尾部 user 消息的内容
  与 `loaded_session.messages` 中已存在的某行(尾部 user 行)匹配,
  则视为已落库,**跳过 persist**,并把 `last_user_seq` 指向该已有行的 seq
  (保证后续 FileInjections update 等仍指向正确行)。
- R1.2 判据要稳定可靠(不因 content JSON 序列化顺序、thinking signature
  等不稳定字段误判)。具体判据见 design.md。
- R1.3 **不得改变普通单 agent 聊天的行为**:普通聊天尾部是新消息,不在
  DB 中,必须照常 persist(回归保护)。

### R2 — 群聊编排验证
- R2.1 群聊多轮(moderator → participant → moderator)对话能完整跑通,
  不再出现 OpenAI 400 / `[生成出错中断]` 死循环。
- R2.2 DB 中不再产生重复的 tool_result 行(同一 tool_use_id 不出现两条
  user-role tool_result)。

## Non-Goals / 范围外

- 不改 `ChatMessage` 类型加 seq 字段(那是更大的改动,留给方案 B 若方案 A
  被否决)。本次在 `chat_loop.rs` 内用现有 `loaded_session.messages` 解决。
- 不改前端 history 构造逻辑。
- 不改 group_chat 的编排结构(reload 策略本身不变)。
- 不处理参与者工具过滤(已在上一任务 `08-04-group-chat-ui-fix` 修复并提交)。

## Acceptance Criteria

- [ ] **AC1**:新建一个 group_chat session,moderator 调用 nominate_speaker
  点名后,参与者能正常发言,moderator 能再次接话,**对话流畅跑完多轮**,
  无 OpenAI 400、无 `[生成出错中断]` 死循环。
- [ ] **AC2**:上述群聊 session 在 DB 里,每个 tool_use_id 只对应一条
  user-role tool_result(无重复)。
- [ ] **AC3**:普通单 agent 聊天的回归测试全绿(尤其涉及工具调用的多轮
  对话,user 消息仍正常落库、不丢、不重复)。
- [ ] **AC4**:`run_chat_loop` 现有单测全绿,且新增针对该持久化点的单测
  覆盖"尾部 user 消息已落库 → 跳过 persist"分支。
- [ ] **AC5**:cargo check + clippy + 现有测试套件全绿。

## Notes

- 复杂任务:需要 design.md(方案对比 + 判据设计 + 风险)和 implement.md
  (改动步骤 + 验证命令 + 回滚点)。
- 判据选择(内容比对 vs seq 标记)是核心设计决策,见 design.md。
- 提交信息沿用 `fix(group-chat): ...` 风格。
