# Design — 群聊 per-role history 隔离(形态 B)

> PRD: `prd.md`。设计草案与取舍论证: `research/design-draft.md`(形态 A vs B 取舍 +
> signature 约束分析)。
> 调研依据:三份架构现状报告(编排器/前端/DB)+ signature 约束查证。
> **评审修订**(review.md,P0/P1/P2):归属策略改方案 (a)、补 D-D 守卫扩展、
> 状态机代码清理、测试语义重写。本版反映修订后状态。

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
/// rewritten as role:user. Other speakers' Thinking blocks are dropped
/// (they carry signatures bound to *their* generation context; re-injecting
/// them into *this* role's context would either break the signature
/// round-trip or be echoed as if this role produced them). Other speakers'
/// tool_use/tool_result pairs are dropped entirely (tool results are NOT
/// shared — relayed only via their text remarks). The moderator's
/// arbitration pairs (nominate/end) are also dropped for non-moderator roles.
///
/// 归属策略(P0-2 评审修订):改写行 **保留 speaker 字段、content 不带 `@` 前缀**。
/// 归属交给 wire 层统一负责——Anthropic `apply_speaker_prefix` 自动插 `@name: `、
/// OpenAI 自动填 `name` 字段。若 content 自带 `@` 前缀会造成双重前缀(Anthropic
/// `@moderator: @moderator: …`)。speaker 字段同时是 P0-1 D-D 守卫的区分信号。
fn role_history(full: &[ChatMessage], current_role: &str) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::with_capacity(full.len());
    let mut pending_other_tool_use_ids: Vec<String> = Vec::new();
    for m in full {
        // (1) 若这行是某个他人 tool_use 的 tool_result,跳过(他人 tool 对整对剥离)
        if !pending_other_tool_use_ids.is_empty() {
            if row_carries_any_tool_result(m, &pending_other_tool_use_ids) {
                pending_other_tool_use_ids.clear();
                continue;
            }
            pending_other_tool_use_ids.clear();
        }
        // (2) 按角色归属重写。先判 role 再判 speaker,避免重复分支(P2-1)。
        match m.role {
            Role::User => out.push(m.clone()),   // 人类 prompt / tool_result(非他人 pending)
            Role::Assistant => {
                match &m.speaker {
                    Some(sp) if sp == current_role => out.push(m.clone()),  // 自己 assistant 原样
                    Some(sp) => {
                        // 他人 assistant:抽 text 改写为 user(不带 @ 前缀,归属靠 speaker 字段);
                        // Thinking 丢;ToolUse 收集 id 待跳过。
                        let text = extract_text_blocks(&m.content);
                        if !text.is_empty() {
                            out.push(ChatMessage {
                                role: Role::User,
                                content: MessageContent::Text(text),
                                speaker: Some(sp.clone()),
                            });
                        }
                        pending_other_tool_use_ids.extend(extract_tool_use_ids(&m.content));
                    }
                    None => {
                        // 群聊 assistant 行必有 speaker(moderator 或 participant.name)。
                        // 出现 None 说明数据异常(P2-2):不静默改写成 @?,直接原样保留
                        // 交后续诊断,或加 debug_assert 报警。实现时选 debug_assert。
                        debug_assert!(m.speaker.is_some(), "group-chat assistant row missing speaker: {:?}", m);
                        out.push(m.clone());
                    }
                }
            }
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
- **改写行保留 speaker、content 不带 `@` 前缀**(P0-2 方案 a)→ wire 层统一归属,
  无双重前缀;speaker 字段供 D-D 守卫区分(P0-1)。

### 工具函数(私有,`group_chat_loop.rs` 内)

```rust
fn extract_text_blocks(c: &MessageContent) -> String
fn extract_tool_use_ids(c: &MessageContent) -> Vec<String>
fn row_carries_any_tool_result(m: &ChatMessage, ids: &[String]) -> bool
```

---

## R1.5 — D-D 入口守卫扩展(P0-1,第二改动点,在 chat_loop.rs)

### 问题(评审 P0-1 坐实)

`run_chat_loop` 的 tail user 消息 persist 点(`chat_loop.rs:987-1034`)走 D-D 入口守卫:
tail user 消息在 DB 里按 `role == "user"` + `user_message_matches`(文本字节/tool_use_id 判等)
查找,**且仅在 `group_chat_state.is_some()` 时生效**;匹配则跳过 persist,否则 `persist_turn`。

形态 B 下,**他人最后一条 text 发言被改写为 `role:user` + `speaker:Some(...)` 的新 tail**
(原始 DB 行是 assistant),守卫按 role==user 查 DB 找不到匹配 → **判定为"新人类消息"→ 重复落库**。

后果(每轮叠加):DB 追加 `@<speaker>: <text>` 重复 user 行 → 前端 `load_session` 显示
重复 speaker chip 行 → 下一轮 reload 落入"保守保留"分支永久留在上下文 → 持续膨胀。

### 修复:守卫扩展(chat_loop.rs:990-1000 附近)

在 `already_in_db` 判定里,**对 tail user 消息 `speaker.is_some()` 视同已落库、跳过 persist**
(仅在 `group_chat_state.is_some()` 作用域内,与现有守卫同域):

```rust
let already_in_db = (!skip_persist)
    .then(|| {
        if msg.speaker.is_some() {
            // P0-1 + P0-3 (08-07-group-chat-same-model-crossover): a tail
            // user row carrying `speaker` can ONLY be a role_history
            // rewrite product (他人发言改写) — human prompts, tool_results,
            // synthetic tool_results all have speaker == None. Treat it as
            // already persisted to avoid duplicate writes + frontend ghost
            // rows. seq 取尾部最后一个 user 行(语义上最接近改写行的位置);
            // 实际 seq 值不再被消费(注入被下方 last_user_snapshot=None 跳过)。
            return loaded_session.messages.iter()
                .rev()
                .find(|m| m.role == "user")
                .map(|db_row| db_row.seq);
        }
        loaded_session.messages.iter()
            .filter(|m| m.role == "user")
            .find(|db_row| user_message_matches(db_row, &msg))
    })
    .flatten()
    .filter(|_| group_chat_state.is_some())
    .map(|db_row| db_row.seq);
```

**且**(P0-3,注入跳过的另一半):`already_in_db` 命中(speaker Some)分支的返回,
`last_user_snapshot` 改为 `None`(现状是 `Some(msg.content)`):

```rust
if let Some(existing_seq) = already_in_db {
    let user_seq = existing_seq;
    // P0-3: 改写行 last_user_snapshot 返回 None —— chat_loop.rs:1116 的
    // at_file 注入条件 (injections 非空 && last_user_snapshot.is_some()) 为
    // false,注入整体跳过。改写行是他人发言转述,不是人类输入,本就不该
    // 触发 @file 注入(否则注入 manifest 会写到错误的 seq 行 + 前端
    // FileInjections 事件打到错误消息)。
    (None, user_seq)
} else { ... }
```

**安全依据**:群聊现代码库中,人类 prompt(`speaker: None`)、tool_result(`speaker: None`)、
synthetic tool_result(`speaker: None`)恒为 None;`speaker: Some` 的 user 行**只能是
role_history 的改写产物**。判定零误伤,现有调用路径(speaker None)行为不变。

> **P0-3 背景(自查发现,评审 P0-1 只抓了一半)**:原方案守卫闭包写
> `find(|db_row| msg_speaker_some || user_message_matches(...))` —— `msg_speaker_some`
> 是外层常数,`find` 对 DB **第一条** user 行就返回 true,拿到错误 seq;且跳过 persist
> 分支 `last_user_snapshot` 仍返回 `Some(改写内容)`,触发 `:1116` 的 at_file 注入 → 注入
> manifest 写到错误 seq 行 + 前端事件错位。修正:speaker Some 时 seq 取尾部最后一个
> user 行 + `last_user_snapshot` 返回 None。两条配套,重复落库与注入错位一起解决。

### 这意味着 R4 的修订

`run_chat_loop` 不再"完全不动"——它的 D-D 守卫有**一处扩展**(在群聊作用域内的判定:
跳过 persist + 跳过 at_file 注入)。这是本任务除编排器外**唯一的第二改动点**,如实记入
PRD R4 / AC4。改动面仍极小(守卫判定加 `speaker.is_some()` 分支 + `last_user_snapshot`
返回 None),且不改变守卫对非群聊路径的行为。

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
| `role_history_other_speaker_rewritten_as_user` | 他人 assistant → role:user + **content 不带 `@` 前缀** + `speaker` 字段保留为该发言者;**不含 Thinking 块**(P0-2 修订) |
| `role_history_other_thinking_dropped` | 他人 Thinking/RedactedThinking 块不出现(关键:绕 signature) |
| `role_history_other_tool_pair_dropped` | 他人 read_file 的 ToolUse + 其 tool_result 整对不出现在结果里 |
| `role_history_own_tool_pair_preserved` | 当前角色自己的 read_file 对原样保留(role 配对合法,OpenAI 不报 400) |
| `role_history_human_prompt_preserved` | 人类原始 prompt(speaker==None)保留为 role:user |
| `role_history_moderator_arbitration_dropped_for_participant` | moderator 的 nominate/end 对不出现在 participant history(沿用 participant_view 不变量) |
| `role_history_moderator_keeps_own_arbitration` | moderator 自己的 nominate/end 对保留(moderator 需知道自己提名过谁) |
| `role_history_multiturn_same_role_preserved` | 某 participant 多轮发言,所有自己的 assistant 轮次保留(speaker==current_role 覆盖所有轮) |
| `role_history_signature_roundtrip_contract` | 构造含 signature 的 transcript,断言当前角色 Thinking.signature 完整(契约级守 Anthropic 回传) |
| `role_history_wire_no_double_prefix`(P0-2) | 改写行经 wire 序列化后,Anthropic `@name:` 前缀只出现一次(无双重);OpenAI `name` 字段正确填入 |

### D-D 守卫扩展测试(P0-1 + P0-3,在 chat_loop 守卫的测试处)

| 测试 | 断言 |
|---|---|
| `dd_guard_skips_persist_for_speaker_user_in_group_chat` | 群聊作用域内,tail user 行 `speaker.is_some()` → 不触发 persist_turn(无 DB 重复行) |
| `dd_guard_unchanged_for_classic_chat_speaker_none` | 经典聊天(speaker None)/ 群聊 human prompt(speaker None)→ 守卫行为不变 |
| `dd_guard_rewrite_row_skips_at_file_injection`(P0-3) | 改写行(speaker Some)命中守卫后 `last_user_snapshot` 返回 None → at_file 注入条件 false,无注入 manifest 写入、无 FileInjections 事件 |
| `dd_guard_rewrite_row_seq_not_first_user_row`(P0-3) | 改写行的 `user_seq` 不等于 DB 第一条 user 行的 seq(find 不再因常数短路匹配错行);`last_user_snapshot` None 时 seq 不被消费,但值合理 |

### 迁移既有测试(P1-1 语义重写,关键)

`participant_view_*` 一族(`group_chat_loop.rs:1373+` + `tests_group_chat.rs`)迁移到
`role_history_*`,但**不是简单断言对象替换**——其中一条测试的**前提发生逆转**,必须重写:

**`identity_contract_view_holds_under_same_model_and_mislabel`(`group_chat_loop.rs:1537`)**
- **旧前提**(注释 `:1541-1544, :1560-1563`):"view 不读 speaker、不消毒 content,
  mislabeled 行原样透传"——测的是 view 结构不变量。
- **新前提**(role_history):view **按 speaker 强制归因**——mislabeled 行
  (`speaker="M3"` 但内容 `@D4F: …`)会被改写为 `role:user + speaker=M3`,content 仍是
  原文 `@D4F: …`(role_history 不消毒文本,只改 role/speaker 归属 + 丢 Thinking)。
- **迁移做法**:断言该 mislabeled 行在 role_history 输出里变成 `role:user + speaker=M3`,
  content 文本不变;**不再断言"原样透传 assistant"**。仲裁剥离 + 无孤儿不变量仍成立。
- **另**:08-07 加的 `identity_contract_prompts_separate_roles_under_same_model`(`:1614`)
  只测 prompt,不依赖 view,**不受影响** ✓。

其余 `participant_view_*` 测试(仲裁剥离、相邻性、混合 tool 对)在 role_history 下
不变量仍成立,断言对象换成 role_history 输出即可。

### 不破坏的既有测试

- `identity_contract_prompts_*`(08-07 R1)— 只测 prompt,不依赖 view,不受影响。
- `group_chat_tool_defs_*`(08-07 R1)— 工具层不动。
- prompt 回归测试(08-07 R3)— prompt 不动。

---

## 风险与回滚

- **R1 重写规则边界 bug**(如误把自己当他人剥离):缓解 — 测试覆盖每条重写规则;
  signature 契约测试 + identity_contract 兜底。回滚 = 恢复 participant_view。
- **P0-1 守卫扩展误伤**(把真人类消息也跳过):缓解 — `speaker.is_some()` 信号零误伤
  (群聊人类消息恒 speaker None);两测试守边界。回滚 = 去掉短路。
- **P0-2 归属策略**(保留 speaker、不带 @ 前缀):OpenAI/Anthropic 双 provider 都能归因;
  `role_history_wire_no_double_prefix` 守无双重前缀。
- **他人发言改写成 user 后语义**:模型通过 wire 层 `@name:`/`name` 字段知道来源;
  thinking 丢失意味着模型看不到他人推理过程(本就是设计目标,非 bug)。
- **moderator 看不到 participant 的 tool 细节**(P2-3):moderator 只看到 participant 文本
  发言,不影响它提名/收尾(它本就只读文本判断讨论走向)。双向影响记入 PRD R3。
- **跨 provider 一致**:OpenAI 无 signature,他人 thinking 丢弃无额外影响;Anthropic
  他人 thinking 带签名,丢弃正是为不触发回传约束。两 provider 行为一致(role_history
  无 provider 分叉)。
- **回滚成本**:纯逻辑层改动(R1 组装器 + R1.5 守卫一处短路),无 DB 迁移,git revert 即可。

---

## 备选(形态 A,默认不走)

若未来出现按角色独立审计/重放/迁移的 DB 层诉求,才考虑形态 A(物理 session 分表)。
要点见 `research/design-draft.md` §5。当前无证据,不展开设计。
