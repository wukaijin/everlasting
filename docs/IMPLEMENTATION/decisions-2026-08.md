### 2026-08-06 — 群聊 speaker 落库错位修复(废弃 round-robin fallback + wire 孤儿自愈)

- **Context**:moderator 常用自然语言引导("接下来请 D4F…")但不调 `nominate_speaker`,旧 round-robin fallback 按 `participants[round % len]` 机械派人 → 派错的人为回应语境模仿他人口吻发言 → 角色认知崩溃、speaker 标签错位(DB 取证 session a6c87247)。外围修复均失败:v1 加重试循环放大 orphan tool_use(D4F 28 个 400),v2 把 max_turns 1→6 破坏"nominate 后 turn 结束"语义(participant 完全不发言)。
- **决策 — 废弃 round-robin fallback,改为重试 moderator turn**:`next_speaker == None` 时不再机械轮转,而是 `no_nominate_streak += 1` 回到 for round 顶部**重跑 moderator turn**(不派任何人),超过 `MAX_NO_NOMINATE_STREAK` 才放弃。`nominate_speaker` 成为唯一调度机制;moderator `max_turns=1` 保持(nominate 后天然结束)。merge `fix/group-chat-speaker-desync`(`48960d7`)。

### 2026-08-07 — 群聊评审修复 + 工具集收敛 + per-role history 隔离 + 总结卡片

- **Context**:DB 取证(session 8be4687f)治理群聊三层缺陷:① `use_skill` 幻觉(moderator/participant 用 `update_checklist`/`use_skill` 自建调度,黑名单没拦住);② 参与者夺权(moderator-only 工具被调用);③ 同模型串台(多身份 assistant 共存同一上下文)。连续两个任务合并提交(评审修复 + 工具集收敛)。
- **决策 R1 — 工具白名单取代黑名单**:新增 `group_chat_tool_defs(tool_defs, is_moderator)`(取代 `participant_tool_defs`):moderator = 调研白名单(`read_file`/`grep`/`glob`/`list_dir`/`web_fetch`)+ 仲裁工具(`nominate_speaker`/`end_discussion`);participant = 仅调研白名单。**白名单穷尽** — 新增 builtin 工具默认不进群聊,防"群聊误用新工具"复发。
- **决策 R2 — 编排器静默路径变可见事件**:4 条静默 break/continue 改为 emit `Done{stop_reason}`。终态 max_rounds → finalize;非终态 nominee_unknown / participant_unresolved → 挂 notice 不 finalize(前端 `groupChatNotice` + `ChatMessage.notice` + MessageItem 提示行)。
- **决策 R3 — 参与者 max_turns 1→20**:允许参与者查询代码库取材实证(read_file 等工具后第二轮发言);moderator 保持 1。
- **决策 R4 — 删 `ParticipantConfig.order` 死字段**:round-robin 废弃后 order 不再用,UI ↑/↓ 重排按钮移除(防误导);serde 忽略旧 metadata 的 order(向后兼容 + 回归测试锁死)。
- **决策 R5 — `participant_view` 相邻性不变量**:显式不变量注释 + 参与者多轮非仲裁对混合场景测试(锁死 R3 改 max_turns 后不破坏)。
- **决策 — per-role history 隔离(`role_history`)**:`role_history(full, current_role)` 一遍扫描状态机取代 `participant_view`:每个角色只见自己的 assistant 行(verbatim,含 thinking + signature — Anthropic round-trip 安全),他人发言改写 `role:user`(保留 speaker 字段、content 不带 `@` 前缀 — 归属由 wire 层负责,防双重前缀),他人 thinking / 工具对剥离(工具结果不共享),moderator 仲裁对从参与者视角同样剥离。治第三层"同模型串台"。配套 D-D 守卫扩展(speaker 短路,防 rewrite 行重复落库 → DB 污染 + 前端 ghost rows)。
- **决策 — 参与者 research 指引(08-07-group-chat-participation-summary 前半)**:工具白名单已下发但 identity-guard 禁令过重,弱模型把"禁止夺权"过度泛化为"讨论轮不调任何工具"(参与者零工具调用,DB 取证 session 6c00f286);prompt 补"调研工具是你的,只用来佐证自己的观点"指引,恢复实证取材能力。
- **决策 — end_discussion 总结卡片(08-07-group-chat-participation-summary 后半)**:moderator 调 `end_discussion` 把完整总结放进 `input.summary`,前端新增 `DiscussionSummaryCard.vue` 提取渲染"讨论总结"卡片(200px 截断可滚动,markdown 渲染,与 ToolCallCard 共用 result 数据源),总结不再只藏在工具卡片里。
- 任务:`08-06-group-chat-speaker-desync` / `08-07-group-chat-review-fixes` / `08-07-group-chat-toolset-and-identity` / `08-07-group-chat-role-history-isolation` / `08-07-group-chat-participation-summary`(均 archive)。spec:`.trellis/spec/backend/agent-loop-architecture/pattern-worker-worktree-override.md` §"Group-chat transcript view" Pattern(role_history + 工具白名单 + D-D 守卫)。

### 2026-08-07 — agent-loop:tool_use 执行以 tool_calls 为准,不依赖 stop_reason

- **Context**:DB 取证(session d7fe451c)moderator 某轮发出 3 个 `read_file` tool_use,但 provider(Console Go,OpenAI 兼容)在 tool_calls-only 流上的 finish_reason 不是 `"tool_calls"`(`"stop"`→`"end_turn"` 或缺失)。旧判定 `should_continue = stop_reason == Some("tool_use") && !tool_calls.is_empty()` 不成立 → 直接 return:`assistant(tool_use)` 行已落库但工具未执行、`tool_result` 未落库 → 孤儿 tool_calls 违反 llm-contract §Pair Atomicity → 每次请求 400 → 群聊烧掉 `MAX_ORCHESTRATION_ROUNDS`(26 个连续错误 turn)。
- **决策**:工具执行 gate 只看 `tool_calls` 本身 — 模型发出任何 tool_use 就必须执行并回灌结果;`stop_reason` 只决定终态 `Done` 值。spec 补 `llm-contract.md` §Pair Atomicity 执行侧契约(When this bites),回归测试 `agent_loop_tool_use_with_non_tool_use_stop_reason_still_executes`。
