# Design — 群聊评审问题修复

> PRD: `prd.md`。本设计覆盖 R1-R5 五个修复点。

## 全局约束（不改）

- 串行时序（`for round { moderator.await → participant.await }`）不变。
- 5 层身份防御（wire 标注 / `participant_view` / `participant_tool_defs` /
  identity-guard prompt / moderator 单轮）不推翻，只补强边界。
- D-D 入口护栏（`chat_loop.rs:987-1007`）不动。
- `MAX_ORCHESTRATION_ROUNDS=30` / `MAX_NO_NOMINATE_STREAK=3` 常量值不变。

---

## R2 — 编排器静默路径变面向用户的事件（D2：复用 Done.stop_reason）

### 路径分类：终态 break vs 单轮 continue

| 路径 | 当前代码 | 语义 | 终态？ |
|---|---|---|---|
| 连续 3 次不提名 | `group_chat_loop.rs:571` break | 讨论无法继续 | **是** |
| `MAX_ROUNDS` 耗尽 | `:714` 落出循环 | 讨论跑满 | **是** |
| nominee 未知 | `:590` continue | 这一轮没人发，下一轮 moderator 重试 | 否 |
| participant provider 解析失败 | `:601` continue | 同上 | 否 |

### 落地

**终态（2 条）**：编排器退出循环后，emit 终态 `Done { stop_reason }`，stop_reason
取：
- 连续不提名 → `"moderator_stuck"`
- `MAX_ROUNDS` 耗尽 → `"max_rounds"`

复用现有"非 cancel 退出 → emit 终态 Done"路径（当前只有 `group_chat_end`）。
重构：把循环退出原因提成 `enum HaltReason { ModeratorStuck, MaxRounds,
DiscussionEnded }`，循环退出后 match 它决定 stop_reason（`DiscussionEnded` →
现有 `group_chat_end`）。这样不改 finalize 语义骨架，只扩 stop_reason 取值。

**单轮 continue（2 条）**：这两条 `continue` 让讨论继续，**不能 emit 终态 Done**
（否则前端 finalize、后续轮丢失）。落地：emit 一个**非终态可见 Done**——
`Done { stop_reason: "nominee_unknown" / "participant_unresolved" }`，但
**不**是 `group_chat_end`，所以前端 `done` handler 现有条件
（`streamController.ts:1397`，`!groupChat || stop_reason==group_chat_end/cancelled`
才 finalize）**不会 finalize**，请求继续活着。前端按 stop_reason 记一条
notice（见前端改动）。这样复用 Done 承载、零新增事件类型、finalize 语义不变。

> 关键不变量：`group_chat_end` / `cancelled` 仍是**唯一 finalize 触发**；
> 新增的 4 个 stop_reason 都是非 finalize（continue 两条天然不 finalize，
> break 两条是循环真退出后那次 Done —— 这两条是终态，**会** finalize，但
> 此时讨论确已结束，finalize 正确）。需在 design 注释里明确：终态 break 的
> Done 是循环退出后那次，stop_reason 覆盖 `group_chat_end`。

### 前端改动（`streamController.ts`）

1. `done` handler 现有 finalize 条件保留（`group_chat_end`/`cancelled`）。
2. 新增：非 finalize 的 Done 若 stop_reason ∈
   `{moderator_stuck, max_rounds, nominee_unknown, participant_unresolved}`，
   在当前 placeholder 上挂一个 `notice` 字段（类比现有 `retrying` 字段，
   transient、不进 DB）。MessageItem 渲染一条灰色 notice 行（"主持人卡住，已
   停止讨论" / "轮次耗尽" / "主持人未点名任何人，重试中" / "某参与者模型不可
   用，跳过该轮"）。
3. 终态两条（moderator_stuck / max_rounds）的 Done **会** finalize（因为循环
   已退出、不再有事件）—— 所以这两条 stop_reason 要加进 finalize 白名单。
   `done` handler 条件改为：
   `!groupChat || stop_reason ∈ {group_chat_end, cancelled, moderator_stuck, max_rounds}` → finalize。
   continue 两条（nominee_unknown / participant_unresolved）**不**加白名单。

### 测试（`tests_group_chat.rs`）

- 终态：构造 moderator 连续 `MAX_NO_NOMINATE_STREAK` 次不调工具的 mock，断言
  emit 恰一个 `Done { stop_reason: "moderator_stuck" }` 且为最后一个 Done。
- 单轮：构造 moderator nominate 一个不在花名册的名字，断言 emit
  `Done { stop_reason: "nominee_unknown" }` 且**非** finalize（后续仍有事件）。

---

## R3 — 参与者 max_turns 1 → 20（D3：全工具集）

### 改动
`group_chat_loop.rs:665`（participant 分支）`Some(1)` → `Some(20)`。
moderator 分支（`:506`）`Some(1)` 不变。注释更新：从"single turn"改为
"up to 20 turns — participant may query the codebase (read_file/grep) before
responding; moderator stays single-turn to preserve the nominate-then-end
contract and to suppress identity-confusing filler text"。

### 关键不变量复核（对照 `agent-loop-architecture.md` 7 条 contracts）
- Contract 3"moderator max_turns=1"——**不动**，只改参与者。
- Contract 1（pair-atomic stripping）/ Contract 2（shared turn-state
  scope）/ Contract 4（reload retained）/ Contract 6（terminal signal）——
  参与者多轮不碰这些。参与者多轮的 tool 对是**非仲裁**，`participant_view`
  passthrough，下一个 speaker reload 看到完整非仲裁对（正确，互见性要求）。
- Contract 5（identity-guard prompt）—— 参与者多轮不削弱身份护栏。
- **风险点**：参与者 max_turns=20 后，参与者自己可能在多轮里调
  `run_background_shell` 起后台进程。这是 D3 拍板"全工具集"接受的副作用，
  不在本任务收敛。注释提示即可。

### 测试
新增集成测试：mock 参与者第一轮返回 tool_use(read_file)（stop_reason=tool_use），
第二轮返回 text 发言（end_turn）。断言：
- 参与者 `sent_messages` 恰 2 次（多轮闭环）。
- DB 里参与者那条 assistant 行 + user(tool_result) 行相邻落库。
- 无 `ChatEvent::Error`。
- `participant_view` 放行该非仲裁对（已有单测覆盖，这里跑一遍集成）。

---

## R1 — 身份正确性自动化回归底线

### 验证的不变量（最坏输入下仍成立）
1. **转录隔离契约**：给定一个含 moderator 仲裁对 + 参与者发言 + 同模型混淆
   的 `full`，`participant_view(&full)` 的输出里：
   - 0 个 `nominate_speaker`/`end_discussion` ToolUse/ToolResult 块（既有测
     试 `participant_view_strips_arbitration_pair_keeps_text` 等已覆盖，但
     这里加一个**"同模型+他名前缀"的最坏构造**）。
   - 所有保留行 speaker↔role 不自相矛盾（assistant 行 speaker 仍是该发言者，
     不出现"assistant 行内容是别人在说话"——这层靠 prompt，测试只能验 view 层）。
2. **prompt 契约**：`participant_system_prompt(name, persona)` 输出含明确角色
   边界块、禁自名开头、允许 @别人；`moderator_system_prompt` 含 roster + 不
     含参与者。这部分既有单测（`participant_prompt_forbids_self_label...` /
   `moderator_prompt_forbids_self_label`）已覆盖，R1 补一个**"同模型组合"
   场景**：参与者 name 与 moderator 同模型（构造 GroupChatCtx 使
   moderator_model_id == participant.model），断言 prompt 仍区分角色。

### 落点
新文件 `app/src-tauri/src/agent/tests_group_chat_identity.rs`（与
`tests_group_chat.rs` 并列，专注身份契约；避免污染既有 664 行集成测试文件）。
或在 `group_chat_loop.rs` 的 `#[cfg(test)] mod tests` 加一个
`identity_contract_same_model_combination` 子模块——后者改动小，倾向后者。
具体在 implement.md 定。

### 诚实边界（写进测试注释）
这是**契约级**底线，不是行为级：它验证"输入再坏，view/prompt 的结构不变量
不破"。它**不能**保证真模型不串角色（那需要真模型）。但它能在有人动了
`participant_view` 过滤逻辑 / prompt 模板时立刻报警——这正是"自动化回归底线"
的目的。

---

## R4 — 删 order + UI 按钮（D4）

### 后端
`group_chat.rs:42` 删 `pub order: Option<i32>` 字段 + 其 `#[serde(default)]`
`#[allow(dead_code)]` 注解。`GroupChatConfig` 反序列化用 `serde(default)` 未知
字段默认忽略（确认 `serde_json::from_value` 对多余 key 不报错——serde 默认
ignore unknown fields，需验证；若 ParticipantConfig 未 deny_unknown_fields 则
安全）。既有 metadata 含 `order` 的 session 仍能反序列化（忽略该 key）。

### 前端
`GroupChatConfigModal.vue`：
- 删 `moveUp` / `moveDown` 函数（`:162-172`）。
- 删 template 里 ↑/↓ 两个 `<button>`（`:271-288`）。
- `submit()` 里 `order: i` 删除（`:188`、`:203`）。
- `ParticipantConfig` TS 类型（`chat.types.ts`）删 `order?`。
- re-seed draft 时 `order: p.order` 删除（`:132`）。
- 既有测试 `GroupChatConfigModal.test.ts` 更新（若有断言 order/重排）。

---

## R5 — participant_view 相邻性不变量锁死

### 调研结论
R3 改参与者 max_turns **不破坏**仲裁对相邻性：
- 仲裁对（nominate_speaker / end_discussion）只在 **moderator turn** 出现，
  moderator `max_turns=1`（不变）→ tool_use 与 tool_result 在同一轮 persist，
  seq 相邻。
- 参与者 max_turns=20 产生的是**非仲裁** tool 对（read_file 等），这些对
  `participant_view` 明确 passthrough（`participant_view_row` 只按
  name==NOMINATE/END 过滤），**不进 strip 状态机**，相邻性假设无关。

### 落地
不改逻辑。在 `participant_view` 函数 doc（`group_chat_loop.rs:181-185`）补
一条显式不变量断言 + 注释：
- 不变量："仲裁对的相邻性只依赖 moderator turn 的单轮落库；参与者多轮的非
  仲裁 tool 对不进 strip 状态机（passthrough），不依赖相邻性。"
- 加一个 debug_assert（或测试）：对 `participant_view` 的输出断言
  `no_orphan_pairs`（既有 `no_orphan_pairs` helper 已存在）在所有既有 view
  测试上成立——其实已覆盖，R5 补注释 + 一条"参与者多轮非仲裁对"的 view 测试
  显式锁死（既有 `participant_view_non_arbitration_tool_pair_passes_through`
  已有，R5 加一条"参与者多轮 + moderator 仲裁对混合"的场景）。

---

## 风险与回滚

- **R2 终态 Done finalize 白名单**：若漏加 `moderator_stuck`/`max_rounds` 到
  finalize 条件，讨论结束后前端不 finalize、请求挂死。测试必须覆盖"终态 Done
  后无残留 active request"。
- **R3 max_turns=20 成本**：3 参与者 × 多轮 × 工具调用，单讨论成本上升。MVP
  可接受；若成本爆，后续可调低常量。回滚 = 改回 `Some(1)`。
- **R4 删字段**：唯一风险是反序列化既有 metadata 报错。serde 默认 ignore
  unknown fields，安全；implement 阶段加一条"含 order 的 metadata 仍能反序列
  化"测试锁死。
