# Review — 群聊 per-role history 隔离(形态 B)

> **评审对象**:任务 `08-07-group-chat-same-model-crossover`(planning 阶段,未 `task.py start`)。
> **评审范围**:`prd.md` / `design.md` / `design-draft.md` / `implement.md` / jsonl 清单,逐条对照代码查证。
> **评审结论**:方案根因判断与形态取舍**成立**;但 R4"不动的"边界漏了 **2 条会被改写的链路**(D-D 入口守卫 persist、provider wire 归属 pass)——**必须补进 design 才能开工(阻塞)**。另有文档路径断裂 2 处、测试迁移语义低估 1 处。
> **代码基线**:`main@7cf1271`(planning 提交)。工作树干净,`participant_view` 仍在线。

---

## 0. 结论摘要

| 级别 | 编号 | 问题 | 性质 |
|---|---|---|---|
| 🔴 阻塞 | P0-1 | 改写出的 user 行会触发 D-D 入口守卫**重复落库**(DB 污染 + 前端重复行) | design 缺口 |
| 🔴 阻塞 | P0-2 | wire 层 `apply_speaker_prefix` 造成**双重 `@` 前缀**(Anthropic 路径) | design 缺口 |
| 🟠 需改 | P1-1 | `identity_contract_view` 迁移不是"断言对象替换",是**语义重构** | implement 低估 |
| 🟠 需改 | P1-2 | `research/design-draft.md` 路径断裂(jsonl 加载失败) | 文档 |
| 🟡 顺手 | P2-1 | design 状态机代码重复分支 + 死注释 | cosmetic |
| 🟡 顺手 | P2-2 | `unwrap_or("?")` 会把 speaker None 的 assistant 静默写成 `@?: text` | 健壮性 |
| 🟡 顺手 | P2-3 | moderator 方向失去 participant tool 细节未写进 PRD R3 影响清单 | 文档 |

---

## 1. 查证成立的部分(无需改动)

以下论断逐条对代码核实,**成立**:

1. **根因坐实**:群聊单 `session_id` 共享 history,`run_group_chat_loop` 对 moderator 传 `full` 全量(`group_chat_loop.rs:536-540`)、对 participant 传 `participant_view(&full)`(`:715-716`);`reload_messages`(`:421-451`)从 `m.content` 反序列化,thinking 块原样保留。
2. **串台源**:`participant_view`(`:231-264`)只剥 moderator 的 nominate/end 对,**保留所有角色 text + thinking 为 `role: assistant`**;moderator 侧连 participant 的 thinking 都全量可见(现状更糟)。
3. **signature 硬约束**:`types.rs` `ContentBlock::Thinking` 注释 "MUST be echoed back verbatim — otherwise the API returns 400";`anthropic.rs` 的 `apply_deepseek_reasoning_fix` 注释记录了 thinking 块丢失触发 turn-2 400 的血泪史。→ "不能在共享 history 里选择性净化他人 thinking" 成立。
4. **形态 A vs B 取舍**:DB schema / 前端 transcript / IPC / 流式的冲击面对比属实;形态 B 只碰编排器一层、可回滚。
5. **绕 signature 论证**:signature 约束绑定"同一对话上下文里 assistant 消息的完整性",上下文边界由传给 provider 的 messages 数组决定——形态 B 一开始不把他人 thinking 放进数组,逻辑成立。
6. **D-D 入口守卫作用域**:`group_chat_state.is_some()` + `user_message_matches`(`chat_loop.rs:170-210, 987-1034`)确实只匹配 role==user 的 DB 行、按字节/`tool_use_id` 判等——**这正是 P0-1 的判定基础**。
7. **前端 chip 只认 speaker**:`MessageItem.vue:196-224` chip 渲染仅读 `message.speaker`,不依赖 role;`load_session` IPC 从 DB 行取 speaker。
8. **工具白名单 / identity-guard prompt / moderator 单轮**:08-07 既有修复与形态 B 兼容(R4 判断成立)。

---

## 2. 🔴 P0-1(阻塞):改写出的 user 消息触发 D-D 入口守卫重复落库

### 2.1 机制

R4 声称 "`run_chat_loop` — 收到的就是组装好的 messages,不关心来源",**不成立**。`run_chat_loop` 的用户消息 persist 点(chat_loop.rs:987-1034)会拿 **tail user 消息**走 D-D 入口守卫:

```
tail user 消息 → user_message_matches(db_row, msg)
   → tool_result 块: 按 tool_use_id 判等
   → 纯文本:        按 to_text() 字节判等 (仅比对 DB role==user 的行)
   → 匹配 → 视同已落库,跳过 persist (现状: tail 恒为人类 prompt / tool_result,均匹配)
```

形态 B 下,**他人最后一条 text 发言被改写为 `@<speaker>: <text>` 的 user 行,成为新 tail**。该行在 DB 里不存在——原始行是 **role=assistant**,而守卫只比 DB 的 `role == "user"` 行 → 不匹配 → **判定为"新的人类消息"→ `persist_turn` 写入新行**。

### 2.2 后果(每次发言逐轮叠加)

1. DB 每个 participant turn 追加一条 `@<speaker>: <text>` 的重复 user 行(带 `speaker` 列);
2. 前端 `load_session` 渲染时出现**重复的 speaker chip 行**(`@moderator: 我先请 M3…` 会以"moderator 的 user 行"再显示一遍);
3. 重复行带 `speaker: Some(...)`,下一轮 reload 后落入 design 状态机 `(Role::User, Some(_))` 分支"保守保留"→ **永久留在上下文**;
4. 内容带 `@` 前缀,再叠 P0-2 的 wire 前缀 → 上下文持续膨胀且失真。

### 2.3 建议(冻结签名内的最小修法)

extend D-D 守卫:在 `group_chat_state.is_some()` 作用域内,**tail user 消息 `speaker.is_some()` 时视同已落库、跳过 persist**。

- **安全依据**:群聊现代码库中,人类 prompt(`speaker: None`)、tool_result(`speaker: None`)、synthetic tool_result(`speaker: None`)恒为 None;`speaker Some` 的 user 行**只能是 role_history 的改写产物**。判定零误伤。
- **向后兼容**:现有全部调用路径(speaker None)行为不变。
- **与 P0-2 方案 (a) 配套**(见 §3.2)。

> 需写进 `design.md` §R1 + R4,不能只靠 implement 阶段"遇到再说"。这条不补,群聊每轮都会 DB 写重 + 前端重行,AC4"冲击面只在编排器"也不成立(守卫在 chat_loop.rs,属于第二处改动点)。

---

## 3. 🔴 P0-2(阻塞):wire 层双重 `@` 前缀

### 3.1 机制

design §R1 决定:他人 assistant → 改写行**内容**带 `@<speaker>: <text>` **且保留 `speaker` 字段**。

但 `AnthropicProvider::send` 末尾对**所有带 `speaker` 的消息**(包括 user)跑 `apply_speaker_prefix`(`anthropic.rs:710-740`):字符串 content 前置 `@name: `、数组 content 前置一个 text 块——**speaker 字段存在即再插一层前缀**:

```
改写行: content = "@moderator: <text>",  speaker = Some("moderator")
  → Anthropic wire:  content = "@moderator: @moderator: <text>"
```

OpenAI 路径同理(`openai.rs:164-175`):`name` 字段 + 内容自带前缀并存。前缀虽不触发 400,但**语义失真**(模型看到 `@moderator: @moderator: …`),且设计意图("让模型知道来源")被双重实现。

### 3.2 设计依据的错位

design §R1 留 `speaker` 的理由是"前端 chip 渲染不变"——**该推理前提错误**:前端渲染读的是 `load_session` IPC 的 DB 行,从不渲染 in-loop 的 view。`speaker` 字段的实际消费者是 **wire 层归属 pass**(Anthropic `@` 前缀 / OpenAI `name` 字段),不是前端。

### 3.3 建议(二选一,写进 design)

- **(a) 推荐**:改写行**保留 `speaker`、内容不带 `@` 前缀**——归属交给 wire 层统一负责(Anthropic 自动 `@`、OpenAI 自动 `name`)。与 P0-1 修复(按 `speaker.is_some()` 跳 persist)正好配套。
- (b) 内容带前缀、`speaker` 置 `None`——但会失去 P0-1 的区分信号,且 OpenAI 侧失去 `name` 归属字段。

**注意**:方案 (a) 下 OpenAI/Anthropic 双 provider 都能归因,且 Anthropic 不再双重前缀;但需在 `role_history_other_speaker_rewritten_as_user` 测试里同时断言"内容无 `@` 前缀 + speaker 保留"(或按选定方案调整断言)。

---

## 4. 🟠 P1-1:identity_contract_view 迁移是语义重构,不是断言对象替换

`identity_contract_view_holds_under_same_model_and_mislabel`(`group_chat_loop.rs:1537`)直接调 `participant_view`,其核心前提(见测试注释)是:

> "**view 不能/无法改写 content**;content 消毒是 prompt 的职责,view 只保证结构" → mislabeled 行(`speaker="M3"` 但内容 `@D4F: …`)必须**原样透传**。

而 `role_history` **恰恰会改写内容**——mislabeled 行会被改写为 `role:user` + 前缀归因。这条测试的语义从"view 结构不变量"变成"view 按 speaker 重写归属",**不是 implement.md Step 3 写的"断言对象换成 role_history 输出、不变量不变"**。

建议在 `design.md` §测试迁移里显式写清:
- 旧断言(透传 + 无仲裁残留 + 无孤儿对)哪些在 `role_history` 下仍成立、哪些要重写;
- mislabeled 行在 role_history 下的新预期(改写为 user + 前缀,Thinking 丢弃);
- 避免 implement 子代理照抄"迁移"字面后测试翻车。

`identity_contract_prompts_separate_roles_under_same_model`(`:1614`)只测 prompt,不依赖 view,不受影响 ✓。

---

## 5. 🟠 P1-2:文档路径断裂

- `design.md:3` 引用 `research/design-draft.md`,`implement.jsonl` 第 2 条同引——**该文件不存在**;草案实际在任务根目录 `design-draft.md`,`research/` 目录未创建。子代理消费 jsonl 时会加载失败。
- 建议:把 `design-draft.md` 移入 `research/`(顺手满足 complex task research/ 产物的推荐项),或修正三处引用。
- 另:jsonl 清单里可补 `research/diagnosis.md` 的引用(三层缺陷因果链,本任务"第三层"的直接依据),它现在只在 implement 侧被引用,check 侧没有。

---

## 6. 🟡 P2(顺手项)

- **P2-1**:design 状态机代码中 `(Role::User, None)` 分支重复两遍 + 注释"重复分支,合并"。建议实现为 `match (m.role, m.speaker.is_some())`,少一层分支噪音。
- **P2-2**:`sp.as_deref().unwrap_or("?")` 会把 speaker None 的 assistant 行静默改写为 `@?: <text>`。群聊中不该出现 speaker None 的 assistant 行,建议 `debug_assert` 或显式 `unreachable!` 分支,避免 `@?` 悄悄进上下文。
- **P2-3**:行为变化补记——现代码 moderator 拿 `full` verbatim,**今天能看到 participant 的 tool 对与 thinking**;形态 B 后 moderator 也只看到 participant 文本发言。design §风险提过一句,建议在 PRD R3 影响清单里同时写"moderator 方向失去 participant tool 细节"(现只写了 participant 方向)。

---

## 7. 建议的修订清单(待用户确认后执行)

1. `design.md`:
   - §R1:改写行归属策略定为 P0-2 方案 (a)(保留 speaker、内容不带 `@` 前缀);修正"前端 chip"理由为"wire 层归属 pass";
   - §R1 状态机代码:合并重复分支;`unwrap_or("?")` 改显式分支;
   - §R4 "不动的":移除 `run_chat_loop` 条目,新增 **D-D 入口守卫扩展**(P0-1:群聊作用域内 speaker Some 的 tail user 视同已落库);
   - §测试设计:改写 `role_history_other_speaker_rewritten_as_user` 断言(无 `@` 前缀 + speaker 保留);补 `role_history_*` 的 D-D 守卫测试(tail 改写行不触发 persist);显式迁移 `identity_contract_view` 语义(P1-1);
   - 修正 `research/design-draft.md` 引用。
2. `prd.md`:
   - R3 影响清单补 moderator 方向(P2-3);
   - R4 措辞:DB 不变 + 守卫扩展(一处非编排器改动点,如实陈述);
   - AC 补一条:D-D 守卫扩展后,改写出的 user 行不落库、前端无重复行。
3. `implement.md`:
   - Step 1/2 补 D-D 守卫修改点;Step 3 迁移测试按 P1-1 语义重写;Step 4 补"群聊运行后 DB 无 `@` 前缀重复行"的人工核验。
4. jsonl:`implement.jsonl` / `check.jsonl` 修正 `research/design-draft.md` 路径;check 侧补 `research/diagnosis.md`。

> 修订后请复核 `task.py validate` 通过,再决定是否 `task.py start`(workflow 1.4 review gate)。
