# 设计草案 — 群聊 per-role history 隔离(形态 B)

> **这是讨论稿**(`design-draft.md`),不是正式 design。形态(B 逻辑 history 隔离)
> 为主方案,形态 A(物理 session 分表)列为备选。定稿后迁移到 `design.md`。
>
> 调研依据:编排器现状(三份 agent 报告)+ signature 400 约束查证
> (`anthropic.rs:593-626`)+ participant_view 现状(`group_chat_loop.rs:231-313`)。

## 0. 设计目标(精准定义"治什么")

**根因**(查证坐实,见对话记录):群聊每个角色 reload 出的 history 里,共存多条
不同 `name` 的 `role: assistant` 消息(含各自 thinking + signature)。模型把所有
`role: assistant` 历史当作"我自己过去的输出";多身份 assistant 尤其是带 reasoning
的,造成高认知负荷,同模型时推理风格一致把负荷推过临界点 → 串台/生成中断。

**signature 硬约束**(查证坐实):Anthropic 的 `ContentBlock::Thinking { signature }`
**必须原样回传**(`types.rs:92` 注释 "MUST be echoed back verbatim — otherwise the
API returns 400";`anthropic.rs:593-606` 记录了因丢空 signature thinking 触发 turn-2
400 的血泪史)。这意味着**不能在一个共享 history 里选择性净化他人 thinking 块**。

**设计目标**:让每个角色的 LLM 上下文里,**只存在它自己生成的 `role: assistant`
消息**(含完整 thinking + signature,回传链不断);**他人发言作为 `role: user` 投递**
(不携带 thinking signature,不触发回传约束)。这样:
- 认知根因消除(模型看到的 assistant 历史只有自己)。
- signature 约束绕开(他人不是 assistant,无 signature 义务)。
- 同模型 + 不同模型串台都被治(M3 夺权也消失——M3 看不到 moderator 的仲裁流)。

**不在目标内**:DB 按 session 物理分库审计、角色独立重放——当前无证据需要。

---

## 1. 为什么选形态 B(逻辑 history)而非形态 A(物理 session)

**两者认知隔离效果等价**,差异在落地层:

| 维度 | 形态 A(物理分表) | 形态 B(逻辑 history) |
|---|---|---|
| 认知隔离 | 完全 | **完全(等价)** |
| signature 绕开 | 是(物理隔离) | **是(逻辑隔离)** |
| DB schema | 加 parent_session_id/新表 | **不改** |
| 前端 transcript/IPC/流式 | 大改(聚合多 session) | **不改** |
| 编排器 | 改(reload+跨 session 投递) | 改(per-role history 组装) |
| participant_view | 废弃 | **升级为组装器** |
| 冲击面 | 5 领域 | **1 领域** |
| 可回滚 | 难(DB 迁移) | 易(纯逻辑) |

**关键洞察**(为什么逻辑隔离也能绕 signature):signature 约束约束的是"**同一对话
上下文里 assistant 消息的完整性**",而"谁和谁是同一对话上下文"由**传给 provider 的
messages 数组**决定。形态 B 从一开始就给每个角色构造独立 messages 数组(不是从共享
DB reload 再过滤),他人发言的 thinking 根本不进它的数组——它就是 user 输入,无
signature 义务。和形态 A 物理隔离在 provider 视角下**观察不可区分**。

**结论**:形态 A 的 DB 真分表是 over-engineering,代价买的是当前无证据的诉求。形态 B
以 1/5 代价达到等价隔离效果,且可回滚。**主方案走形态 B。**

---

## 2. 形态 B 核心:per-role history 组装器

### 2.1 现状:`participant_view`(剥仲裁,但留所有 assistant)

```
全量共享 DB messages
  → participant_view (group_chat_loop.rs:231)
    → 剥 moderator 的 nominate/end 仲裁 tool_use 对
    → 保留所有角色的 text + thinking 作为 role: assistant  ← 串台源
  → 传给 participant 的 run_chat_loop
```

### 2.2 目标:`role_history`(每人一份独立上下文)

```
全量共享 DB messages(仍是单 session_id,前端/DB 不动)
  → role_history(full, current_role)  ← 新组装器,取代 participant_view
    → 对每条消息判定"属于当前角色 / 属于他人 / 属于人类":
        - 人类 user 消息(speaker == None 且 role==user 的原始 prompt):保留 role:user
        - 当前角色自己的 assistant 消息(speaker == current_role):原样保留
          role:assistant + 全部 blocks(含 thinking+signature)  ← 回传链不断
        - 他人 assistant 消息(speaker != current_role):
            改写为 role:user + 单个 Text block "@<speaker>: <text>"
            【thinking 块丢弃】  ← 不进当前上下文,无 signature 义务
        - 他人/自己的 tool_use → tool_result 配对:见 §2.3
    → 返回该角色的独立 history
  → 传给该角色的 run_chat_loop
```

**不变量(必须守住)**:
- **当前角色的 assistant 消息原样保留**(含 thinking signature)→ Anthropic 回传满足。
- **他人发言不再以 assistant 身份出现**→ 认知根因消除。
- **人类原始 prompt 保留为 user**→ 讨论主题不丢。

### 2.3 tool_use / tool_result 配对的处理(最复杂点)

现状有两类 tool 对:
1. **moderator 仲裁对**(nominate/end):已被 participant_view 剥离,形态 B 沿用剥离。
   这些是"他人的调度行为",当前角色本就不该看见。
2. **participant 自己的调研对**(read_file/grep 等,R3 max_turns=20):**原样保留**。
   这是当前角色自己的 assistant tool_use → user tool_result,role 配对合法。
3. **他人的调研对**(moderator 或其他 participant 的 read_file):**问题点**。
   - 他人 assistant 行带 ToolUse → 它若改写成 user,后面紧跟的 tool_result 行怎么办?
   - OpenAI 严格要求 assistant(tool_calls) 后跟 role:tool,不能插 user。

**处理方案**:他人的调研 tool 对**整对剥离**(assistant tool_use 行 + 其 tool_result 行
都丢掉),不投递给当前角色。理由:
- 当前角色看不到他人调了什么工具(本就不该看到他人的 assistant 细节)。
- tool_result 内容如果重要(moderator 查到的代码),由 moderator 在它的**文本发言**里
  转述(它会作为 user 消息投递:"@moderator: 根据代码,我观察到...")。
- 这正是你说的"不共享工具调用结果"——工具调用本身隔离,结果靠发言转述。

**这带来一个语义变化**(需用户确认,见 §5):当前角色看不到他人调研的原始 tool_result,
只能看到他人在发言里转述的结论。如果 moderator 只调工具不发言(现状 seq 1/3/5 那样),
那些调研结果对 participant 完全不可见。

### 2.4 moderator 的 history

moderator 也走 `role_history(full, "moderator")`:
- moderator 自己的 assistant 消息(含它的仲裁 tool_use)原样保留——它需要知道自己提名过谁。
- 他人(participant)的发言改写为 user 投递——moderator 看到参与者说了什么。
- 人类原始 prompt 保留。

**这样 moderator 和 participant 用同一个组装器**,只是 current_role 不同。比现状
(moderator 用全量、participant 用 participant_view 两套逻辑)更对称。

---

## 3. 落地改动(仅编排器一层)

### 3.1 新增 `role_history` 函数(`group_chat_loop.rs`)

取代 `participant_view` + `participant_view_row`。签名:

```rust
/// Build a role's isolated LLM history from the shared DB transcript.
/// The current role sees: human prompts (role:user), its OWN assistant
/// messages verbatim (incl. thinking+signature), and OTHER speakers'
/// utterances rewritten as role:user text ("@<speaker>: <text>"). Other
/// speakers' tool_use/tool_result pairs are dropped entirely (tool
/// results are not shared — relayed only via their text remarks). The
/// moderator's arbitration pairs (nominate/end) are also dropped for
/// non-moderator roles.
fn role_history(full: &[ChatMessage], current_role: &str) -> Vec<ChatMessage>
```

实现是一遍扫描 + 状态机(类似 participant_view 的 pending 跳过逻辑,但更统一):
- 遇 assistant 行:判定 speaker。== current_role → 原样保留;!= current_role 且含
  ToolUse → 标记 pending(跳过其 tool_result);!= current_role 且纯 text/thinking →
  改写成 user 文本(只取 Text 块,丢 Thinking 块)。
- 遇 user 行:若是某 pending 的 tool_result → skip;若是人类原始 prompt(speaker==None)
  → 保留;若是某角色的 user 行(罕见,工具结果以外的)→ 判定归属。

### 3.2 调用点改动(`group_chat_loop.rs`)

- moderator turn(`:536-540` reload 后):`full` → `role_history(&full, "moderator")`。
- participant turn(`:715-716`):`participant_view(&full)` →
  `role_history(&full, &participant.name)`。
- **删除** `participant_view`(`:231`)+ `participant_view_row`(`:270`)+ 它们的测试。

### 3.3 不动的(关键)

- **DB schema / persist / load_session** — 单 session_id 不变,speaker 列不变。
- **前端 messagesBySession / RequestState / load_session IPC / 流式聚合** — 完全不动。
  每个角色的 turn 仍是单 request 上的一个 turn,speaker 事件/chip 渲染不变。
- **run_chat_loop** — 它收到的就是组装好的 messages,不关心怎么组装的。
- **turn_state / nominate_speaker / end_discussion 拦截** — 不动。
- **group_chat_tool_defs**(08-07 R1)— 不动。

### 3.4 测试

- `role_history_current_role_sees_own_assistant_verbatim`:当前角色的 assistant 消息
  (含 Thinking block + signature)原样保留。
- `role_history_other_speakers_rewritten_as_user`:他人 assistant → role:user +
  "@<name>: <text>",Thinking 块不出现。
- `role_history_other_tool_pairs_dropped`:他人 read_file 的 tool_use + tool_result
  整对不出现在结果里。
- `role_history_own_tool_pairs_preserved`:当前角色自己的 read_file 对原样保留
  (role 配对合法)。
- `role_history_human_prompt_preserved`:人类原始 prompt 保留为 user。
- `role_history_moderator_arbitration_dropped_for_participant`:moderator 的
  nominate/end 对不出现在 participant 的 history(沿用 participant_view 既有不变量)。
- **signature 回传契约测试**:构造一段 transcript,跑 role_history,断言当前角色的
  Thinking 块 signature 完整(模拟 Anthropic 回传需求)。

### 3.5 迁移既有测试

`participant_view_*` 一族测试(`group_chat_loop.rs:1373+` + `tests_group_chat.rs`)
改写为 `role_history_*` 对应场景。不变量(仲裁剥离、相邻性)在 role_history 下仍成立,
只是断言对象变了。

---

## 4. 风险与开放问题

### 4.1 语义变化:工具结果不共享(需用户确认)

他人(participant 或 moderator)的调研 tool_result 对当前角色**完全不可见**,
只能通过他们发言里的转述得知。影响:
- moderator 连续调研(seq 1/3/5)不发言时,participant 不知道 moderator 查了什么。
- 两个 participant 调研同一文件会重复调用(token 浪费)。
- **好处**:context 完全干净,无串台;符合你"不共享工具调用结果"的初始表述。

这是 §2.3 的直接后果。**需用户拍板:接受这个语义?还是希望 tool_result 内容(纯文本,
非 tool_use 块)投递给其他角色?** 后者要加一层"tool_result 文本提取 + user 投递"逻辑。

### 4.2 moderator 的仲裁行为对 participant 完全不可见

现状 participant_view 已经剥离 nominate/end,形态 B 沿用。但形态 B 更彻底——
moderator 的仲裁 tool_use 连其 thinking 一起从 participant 视野消失(participant 只
看到被提名后 moderator 的文本发言,如有的话)。这是好事(隔离更干净),但需确认
moderator 的提名 reasoning 是否该对 participant 可见。倾向:不可见(隔离优先)。

### 4.3 多轮发言的相邻性

某个 participant 多轮发言(被多次提名),它的历史里会保留自己所有轮次的 assistant
消息——这是对的(它需要记得自己说过什么)。但要确认 role_history 不误把它的旧轮次
当"他人"剥离。实现上 speaker == current_role 判定覆盖所有轮次,安全。

### 4.4 thinking signature 跨 provider

OpenAI 不需要 signature(`wire.rs:95-101` supports_thinking_signatures 只 Anthropic)。
形态 B 在 OpenAI provider 下,他人 thinking 本就不带 signature,丢弃无额外影响。
Anthropic provider 下,他人 thinking 带签名,丢弃正是为了不触发回传约束。**两 provider
行为一致(他人 thinking 都不进当前上下文),形态 B 无 provider 分叉。**

### 4.5 边界:moderator 自己的 history 是否要剥 participant 的仲裁

participant 不该有仲裁工具(group_chat_tool_defs 已保证),所以 participant 的
assistant 消息不会有 nominate/end。moderator 的 history 里不会出现"participant 的
仲裁行为"。无需额外处理。

---

## 5. 备选:形态 A(物理 session 分表)简述

若用户有 §0 列外的 DB 层诉求(按角色独立审计/重放/迁移),才走形态 A。要点:
- DB:sessions 加 `parent_session_id TEXT`(软 FK,无约束,惯例如 model_id);或新建
  `session_members(session_id, role, child_session_id)` 关系表。
- 编排器:每角色独立 session_id,跨角色消息投递(persist 他人发言为子 session 的 user 行)。
- 前端:`messagesBySession` 改聚合、load_session IPC 扩展按 parent 聚合、RequestState
  重写(多 request_id 聚合到父视图)、流式重写。
- 冲击面 5 领域,工程量数周。**默认不走。**

---

## 6. 下一步(待用户确认)

1. **主方案定形态 B?**(我强烈推荐;形态 A 仅在有额外 DB 诉求时考虑)
2. **§4.1 工具结果语义**:接受"工具结果不共享,靠发言转述"?还是要 tool_result 文本投递?
3. 确认后:本草案迁移到 `design.md`,补 `implement.md`(执行清单 + 验证命令),
   PRD 收敛(改 Goal/Requirements/AC 反映形态 B),最终评审。
