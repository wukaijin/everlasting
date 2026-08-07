# Design — 群聊 per-role history 隔离(形态 B)

> PRD: `prd.md`。设计草案与取舍论证: `research/design-draft.md`。
> 调研依据:三份架构现状报告(编排器/前端/DB)+ signature 约束查证。

## 全局约束(不改)

- 串行时序(`for round { moderator → participant }`,`group_chat_loop.rs:522`)不变。
- DB 单 session_id 不变;messages 表 schema 不变;persist_turn/load_session 不变。
- 前端 `messagesBySession` / RequestState / load_session IPC / 流式聚合不变。
- `run_chat_loop` 签名不变(收到的 messages 就是组装好的,不关心来源)。
- `turn_state`(Arc<Mutex>)、`nominate_speaker`/`end_discussion` 拦截机制不变。
- `group_chat_tool_defs`(08-07 R1)不变。
- `MAX_ORCHESTRATION_ROUNDS=30` 不变。max_turns:moderator=Some(1),participant=Some(20)。
- 5 层身份防御其余 4 层(wire speaker 标注 / 工具隔离 / identity-guard prompt /
  moderator 单轮)不动,只升级 view 层(participant_view → role_history)。

---

## R1 — `role_history` 组装器

### 现状(`participant_view`,串台源)

```
reload_messages(full 共享 DB)
  → participant_view (group_chat_loop.rs:231-313)
    → 剥 moderator 的 nominate/end 仲裁 tool_use 对
    → 保留所有角色的 text + Thinking 块作为 role:assistant  ← 多身份共存 = 串台源
    → (signature 原样保留在他人 Thinking 块里 → 不能在共享 history 里净化)
  → run_chat_loop(participant)
```

moderator 用全量 `full`(无 view 层),同样存在多身份 assistant 共存。

### 设计:`role_history(full, current_role)` 一遍扫描状态机

```rust
/// Build a role's isolated LLM history from the shared DB transcript.
///
/// Each role sees ONLY its own assistant messages (verbatim, incl. thinking
/// + signature — Anthropic round-trip safe) plus other speakers' utterances
/// rewritten as role:user ("@<name>: <text>"). Other speakers' Thinking
/// blocks are dropped (they carry signatures bound to *their* generation
/// context; re-injecting them into *this* role's context would either break
/// the signature round-trip or, worse, be echoed as if this role produced
/// them). Other speakers' tool_use/tool_result pairs are dropped entirely
/// (tool results are NOT shared — relayed only via their text remarks). The
/// moderator's arbitration pairs (nominate/end) are also dropped for
/// non-moderator roles.
fn role_history(full: &[ChatMessage], current_role: &str) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::with_capacity(full.len());
    let mut pending_other_tool_use_ids: Vec<String> = Vec::new(); // 其人 tool_use 的 id,用于跳过其 tool_result
    for m in full {
        // (1) 先处理 pending:若这行 user 是某个他人 tool_use 的 tool_result,跳过
        if !pending_other_tool_use_ids.is_empty() {
            if row_carries_any_tool_result(m, &pending_other_tool_use_ids) {
                pending_other_tool_use_ids.retain(|id| /* 清掉匹配的 */);
                continue; // 整行跳过(他人 tool_result 不进当前上下文)
            }
            pending_other_tool_use_ids.clear();
        }
        // (2) 按角色归属重写
        match (m.role, &m.speaker) {
            (Role::User, None) => out.push(m.clone()),             // 人类原始 prompt
            (Role::Assistant, sp) if sp.as_deref() == Some(current_role) => {
                out.push(m.clone());                                // 自己的 assistant 原样
            }
            (Role::Assistant, sp) => {
                // 他人 assistant。抽 text 改写为 user;ToolUse 收集 id 待跳过;Thinking 丢
                let text = extract_text_blocks(&m.content);
                if !text.is_empty() {
                    out.push(ChatMessage {
                        role: Role::User,
                        content: MessageContent::Text(format!("@{}: {}", sp.as_deref().unwrap_or("?"), text)),
                        speaker: sp.clone(),                        // 保留原 speaker 供前端 chip(虽然 role 变了)
                    });
                }
                pending_other_tool_use_ids.extend(extract_tool_use_ids(&m.content));
            }
            (Role::User, Some(_)) => {
                // 他人 user 行(罕见,非 tool_result 的 user)。保守保留为 user。
                out.push(m.clone());
            }
            (Role::User, None) => out.push(m.clone()),             // 人类 user(重复分支,合并)
        }
    }
    out
}
```

**关键不变量**:
- **当前角色 assistant 原样保留**(role:assistant + 全部 blocks,含 Thinking+signature)→
  Anthropic 回传满足(AC1/AC5 契约守)。
- **他人 assistant 不以 assistant 身份出现** → 认知根因消除。
- **他人 Thinking 块丢弃** → 不进当前上下文,无 signature 回传义务,也不被误当"自己生成"。
- **他人 tool 对整对剥离**(assistant tool_use + 其 tool_result)→ 不污染 role 配对,
  符合"不共享工具结果"语义。
- **人类原始 prompt 保留为 user** → 讨论主题不丢。
- **speaker 字段保留在改写后的 user 消息上** → 前端 chip 渲染不变(虽然 role 变 user,
  但 speaker 标识仍在;MessageItem 的 chip 仅看 speaker 字段,不依赖 role——见调研 §4.2)。

### 工具函数(私有,`group_chat_loop.rs` 内)

```rust
fn extract_text_blocks(c: &MessageContent) -> String
fn extract_tool_use_ids(c: &MessageContent) -> Vec<String>
fn row_carries_any_tool_result(m: &ChatMessage, ids: &[String]) -> bool
```

---

## R2 — 调用点切换 + 旧逻辑删除

### moderator turn(`group_chat_loop.rs:536-540`)

```rust
let full = if round == 0 { messages.clone() }
           else { reload_messages(&db, &session_id).await };
let history = role_history(&full, "moderator");    // ← 新
// ...run_chat_loop(... messages: history ...)
```

现状 moderator 用全量 `full`(无 view)→ 改为 `role_history(&full, "moderator")`。
这样 moderator 也只看到自己的 assistant 历史 + participant 发言(作为 user),和
participant 用同一组装器,对称且隔离一致。

### participant turn(`group_chat_loop.rs:715-716`)

```rust
let full = reload_messages(&db, &session_id).await;
let view = role_history(&full, &participant.name);    // ← 取代 participant_view(&full)
// ...run_chat_loop(... messages: view ...)
```

### 删除

- `participant_view`(`group_chat_loop.rs:231-264`)
- `participant_view_row`(`:270-313`)
- 它们的测试(`group_chat_loop.rs:1373+` 的 `participant_view_*` 一族 +
  `tests_group_chat.rs` 相关)

---

## R3 — 工具结果语义:不共享

已并入 R1 的状态机:他人 assistant 含 ToolUse → 收集 id,其后续 tool_result 行整对跳过。
当前角色看不到他人调研的原始数据,只通过他人文本发言得知结论。

**影响记录在 PRD R3**:moderator 默默调研时 participant 不知情;两人查同文件可能重复。
这是隔离优先的代价,用户初始表述接受。

---

## 测试设计

### 新增(`group_chat_loop.rs` 测试模块 + `tests_group_chat.rs`)

| 测试 | 断言 |
|---|---|
| `role_history_current_role_assistant_verbatim` | 当前角色 assistant 消息原样保留(role:assistant + 全部 blocks,含 Thinking{thinking,signature}) |
| `role_history_other_speaker_rewritten_as_user` | 他人 assistant → role:user + `"@<name>: <text>"`;**不含 Thinking 块** |
| `role_history_other_thinking_dropped` | 他人 Thinking/RedactedThinking 块不出现(关键:绕 signature) |
| `role_history_other_tool_pair_dropped` | 他人 read_file 的 ToolUse + 其 tool_result 整对不出现在结果里 |
| `role_history_own_tool_pair_preserved` | 当前角色自己的 read_file 对原样保留(role 配对合法,OpenAI 不报 400) |
| `role_history_human_prompt_preserved` | 人类原始 prompt(speaker==None)保留为 role:user |
| `role_history_moderator_arbitration_dropped_for_participant` | moderator 的 nominate/end 对不出现在 participant history(沿用 participant_view 不变量) |
| `role_history_moderator_keeps_own_arbitration` | moderator 自己的 nominate/end 对保留(moderator 需知道自己提名过谁) |
| `role_history_multiturn_same_role_preserved` | 某 participant 多轮发言,所有自己的 assistant 轮次保留(speaker==current_role 覆盖所有轮) |
| `role_history_signature_roundtrip_contract` | 构造含 signature 的 transcript,断言当前角色 Thinking.signature 完整(契约级守 Anthropic 回传) |

### 迁移既有测试

`participant_view_*` 一族(`:1373+` + `tests_group_chat.rs`)改写为 `role_history_*`
对应场景。不变量(仲裁剥离、相邻性、混合 tool 对)在 role_history 下仍成立,断言对象
从 participant_view 输出改为 role_history 输出。

### 不破坏的既有测试

- `identity_contract_*`(08-07 R1)— 守 prompt 角色边界 + view 结构,role_history 是
  view 层升级,契约仍成立(需在 implement 阶段确认断言不依赖 participant_view 具体输出)。
- `group_chat_tool_defs_*`(08-07 R1)— 工具层不动。
- prompt 回归测试(08-07 R3)— prompt 不动。

---

## 风险与回滚

- **R1 重写规则边界 bug**(如误把自己当他人剥离):缓解 — 10 个测试覆盖每条重写规则;
  signature 契约测试 + identity_contract 兜底。回滚 = 恢复 participant_view。
- **他人发言改写成 user 后语义失真**:`"@moderator: <text>"` 前缀让模型知道来源;
  thinking 丢失意味着模型看不到他人推理过程(本就是设计目标,非 bug)。
- **moderator 看不到 participant 的 tool 细节**:moderator 只看到 participant 的文本
  发言,不影响它提名/收尾(它本就只读文本判断讨论走向)。
- **跨 provider 一致**:OpenAI 无 signature,他人 thinking 丢弃无额外影响;Anthropic
  他人 thinking 带签名,丢弃正是为不触发回传约束。两 provider 行为一致(role_history
  无 provider 分叉)。
- **回滚成本**:纯逻辑层改动,无 DB 迁移,git revert 即可。

---

## 备选(形态 A,默认不走)

若未来出现按角色独立审计/重放/迁移的 DB 层诉求,才考虑形态 A(物理 session 分表)。
要点见 `research/design-draft.md` §5。当前无证据,不展开设计。
