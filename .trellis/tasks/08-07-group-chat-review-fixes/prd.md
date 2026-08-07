# 群聊评审问题修复

## Goal

修复群聊（`group_chat`）功能三维度评审（编排 / 自我认知 / 可用性）发现的 5 个
问题，让身份正确性有自动化回归底线、编排器边界对用户可见、参与者能查询代码库
取材实证。核心代码：`app/src-tauri/src/agent/group_chat_loop.rs`（编排器）、
`group_chat.rs`（ctx）、`chat_loop.rs:950-1035`（D-D 入口护栏）、
`wire.rs:240-305`（孤儿自愈）、`tests_group_chat.rs`（集成测试）、
`app/src/stores/streamController.ts`（前端逐轮流式）、
`app/src/components/chat/GroupChatConfigModal.vue`（配置 UI）。既有不变量沉淀在
`.trellis/spec/backend/agent-loop-architecture.md` §"Group-chat transcript view"。

## Background / 事实（评审取证）

- 串行时序 + 落库后 reload 实现互见性（PRD R4，不改并发）。
- 身份认知靠 5 层防御：wire speaker 标注、`participant_view` 转录隔离、
  `participant_tool_defs` 工具隔离、identity-guard prompt、moderator 单轮。
- 08-06 已废弃 round-robin 兜底，改为"不派人 + 重试 moderator + nudge"
  （`group_chat_loop.rs:553-586`，`MAX_NO_NOMINATE_STREAK=3`）。
- 所有群聊测试的 LLM 均为 mock（`tests_group_chat.rs`，664 行）。
- 参与者工具集 = `builtin_tools()` 全集减两个仲裁工具
  （`group_chat_loop.rs:278-284`），含 `read_file`/`grep`/`glob`/`list_dir`/
  `web_fetch`/`write_file`/`edit_file`/`shell`/`run_background_shell`/
  `shell_kill`/`update_checklist`/`use_skill`。`max_turns=1` 让工具调用无法
  形成闭环（调了 read_file 没有第二轮据实发言）。
- `stop_reason` 是成熟的自由字符串约定（`end_turn`/`cancelled`/`group_chat_end`/
  `loop_terminated`/`max_turns`/`tool_use` 均在用）；前端 `done` handler 已基于
  `stop_reason` 判 finalize（`streamController.ts:1397`）。
- `ParticipantConfig.order`（`group_chat.rs:42`）编排器完全不用（round-robin
  废弃后发言顺序由 moderator `nominate_speaker` 决定），仅前端 UI 残留重排按钮。

## Requirements

### R1【高】身份正确性自动化回归底线
为身份正确性加一道不依赖真模型的自动化契约测试，作为 prompt / view 过滤逻辑
回归的底线。验证目标：在最坏输入（弱模型输出他名前缀、同模型组合）下，
`participant_view` + 身份护栏不产生自相矛盾的 `speaker ↔ content` 归属。
具体不变量与测试落点见 design.md §R1。

### R2【中】编排器静默 break/continue 变面向用户的事件（D2：复用 Done.stop_reason）
当前以下路径只 `tracing::warn`，前端无感知：
- 连续 3 次 moderator 不提名 → 静默 break（`group_chat_loop.rs:571-578`）
- nominee 不在花名册 → 静默 continue（`:590-596`）
- participant provider 解析失败 → 静默 continue（`:601-604`）
- `MAX_ORCHESTRATION_ROUNDS=30` 耗尽 → 仅 warn（`:714`）

落地形态：编排器在终态 break 时 emit `Done { stop_reason }`，stop_reason 取
`moderator_stuck` / `nominee_unknown` / `participant_unresolved` /
`max_rounds` 等。`nominee_unknown` / `participant_unresolved` 是 `continue`
（单轮可恢复），按当前架构这两条 continue 无法 emit 终态 Done（讨论还在跑）——
改为 emit 一个**非终态的可见事件**（复用 `Done.stop_reason` 但 stop_reason 非
`group_chat_end`，前端不 finalize，仅记一条 system/notice 行）。两条终态
break（`moderator_stuck` / `max_rounds`）emit 终态 Done 并 finalize。前端
finalize 白名单 + 用户提示文案落地。详见 design.md §R2。

### R3【中，已定】参与者 max_turns 1 → 20（D3：沿用全工具集）
`group_chat_loop.rs:665` 参与者调用传 `Some(1)` 改为 `Some(20)`，让参与者
能查询代码库取材实证（`read_file`/`grep` 等）后再发言。**仅参与者放宽**，
moderator 保持 `Some(1)` 不变（nominate 后天然结束 + 堵身份混淆填充文本，
08-04 follow-up 既定决策，不可回退）。参与者工具集 = 全工具集减两个仲裁工具
（含 `write_file`/`edit_file`/`shell` 等写与执行类），不收敛。

### R4【低，已定】删 `ParticipantConfig.order` + UI 重排按钮（D4）
删 `group_chat.rs:42` 的 `order` 字段、`GroupChatConfigModal.vue` 的 ↑/↓
按钮（`:162-172`）、submit 里写的 `order: i`。参与者发言顺序完全由 moderator
`nominate_speaker` 决定。DB 既有 `order` 列存值向后兼容（反序列化忽略）。

### R5【低】`participant_view` 相邻性不变量锁死
调研结论：R3 改参与者 `max_turns` **不破坏**仲裁对相邻性——仲裁对（moderator
的 nominate/end）只在 moderator turn（`max_turns=1`）落库，工具↔结果在同轮
persist，seq 相邻；参与者多轮产生的是**非仲裁** tool 对，`participant_view`
明确放行（passthrough），`participant_view_row` 只按 name==NOMINATE/END 过滤，
不会误剥。风险已排除，R5 收敛为"加断言/注释锁死不变量"，不改逻辑。详见
design.md §R5。

## 已定决策

- **D-1**：参与者 `max_turns` 1 → 20，允许查询代码库取材实证。moderator 保持 1。
  串行时序不变。
- **D2**：R2 用复用 `Done.stop_reason`（不新增 ChatEvent 变体）。
- **D3**：参与者沿用全工具集（含写/shell），不收敛。
- **D4**：删 `order` 字段 + UI 重排按钮。

## Out of Scope

- 串行→并发改造（PRD R4 不改）。
- 真模型端到端验证（仍人工，R1 提供自动化底线）。
- 主持人 persona 可配置化（当前固定 `moderator_system_prompt` 模板）。
- 人类抢占插话改成 live 介入（当前 cancel + 重启讨论，不变）。
- 参与者工具集收敛 / permission 分层（D3 拍板不收敛）。

## Acceptance Criteria

- [ ] AC1（R1）：新增身份正确性契约测试，覆盖最坏输入下 speaker↔content 不自
      相矛盾；`cd app/src-tauri && PKG_CONFIG_PATH="..." cargo test --lib` 绿。
- [ ] AC2（R2）：4 条静默路径各自有面向用户的事件（2 终态 break emit 终态
      Done + finalize；2 单轮 continue emit 非终态可见事件不 finalize）；前端
      在 moderator 卡住 / 参与者不可用 / 轮次耗尽时给出可见提示；测试覆盖。
- [ ] AC3（R3）：参与者 `max_turns = 20`；moderator 保持 1；既有群聊测试绿；
      新增"参与者调用 read_file 后有第二轮发言"的集成测试绿。
- [ ] AC4（R4）：`order` 字段 + UI 重排按钮删除；既有 session（metadata 含
      order）反序列化不报错；既有群聊测试绿。
- [ ] AC5（R5）：`participant_view` 相邻性不变量以断言/注释锁死；R3 后既有
      `participant_view` 单测全绿。
- [ ] AC6：`cd app && pnpm test` 群聊相关前端测试绿（streamController /
      GroupChatConfigModal）。

## Notes

- 复杂任务，`design.md` + `implement.md` 已补齐。
- 既有不变量见 `agent-loop-architecture.md` §"Group-chat transcript view"
  7 条 key contracts，R2/R3 改动需对照不破坏。
- 验证命令：后端 `cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`
  （多线程默认，勿加 `--test-threads=1`）；前端 `cd app && pnpm test`。
