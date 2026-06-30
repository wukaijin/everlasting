# Implement: AskUserQuestion-style agent 阻塞反问 tool

## 1. Ordered Implementation Checklist

按依赖顺序排列,每步独立可验证。

### Phase A — Backend foundation(无前端依赖)

- [ ] **A1**. 新建 `app/src-tauri/src/agent/question_store.rs`
  - `QuestionStore: Arc<tokio::sync::Mutex<HashMap<String, PendingQuestion>>>`
  - `register / resolve / remove / get / list` 方法 + `QuestionStoreError` enum
  - 内联 `#[cfg(test)] mod tests` 覆盖 register/resolve/AlreadyPending/NotFound/AlreadyResolved 五条路径
  - **验证**:`cd app/src-tauri && cargo test --lib question_store`

- [ ] **A2**. 新建 `app/src-tauri/src/tools/ask_user_question.rs`
  - `AskUserQuestionInput` struct + serde derive(字段 snake_case)
  - `definition() -> ToolDef` 给 `builtin_tools()`(仅暴露 schema)
  - `execute_blocking(input, ctx, question_store, sink, cancel) -> (String, bool, ..., Option<i32>)` **唯一执行入口**:内部 1) schema 校验,失败 short-circuit 返回 `is_error: true`(不挂起);2) 通过则 register + emit + select。不再单独的 `execute` 短路径(避免双路径冗余)
  - 内联 `#[cfg(test)] mod tests` 覆盖 schema 边界(空 questions / >4 / options 越界 / header >12)——直接调 `execute_blocking`,断言 schema 失败时 mock store/sink **未被调用**
  - **验证**:`cd app/src-tauri && cargo test --lib ask_user_question`

- [ ] **A3**. 修改 `app/src-tauri/src/tools/mod.rs`
  - `builtin_tools()` 加入 `ask_user_question::definition()`(在 `update_checklist` 后面)——**仅注册 schema**
  - `execute_tool` **不加** `ask_user_question` 分支(chat_loop 特判走 `execute_blocking`,见 A6;在 dispatch 处加注释"此 tool 由 chat_loop 直接处理,不经 execute_tool")
  - **验证**:`cargo check`

- [ ] **A4**. 修改 `app/src-tauri/src/agent/subagent/mod.rs`
  - ⚠ **实施记录(2026-06-30)**:`STRUCTURALLY_DISABLED` 常量实际定义在 `subagent/mod.rs`(被同文件的 `filter_tools_for_subagent` 消费),**非**原写的 `permissions/mode.rs`。`permissions/mode.rs` 只有 `filter_tools_for_mode`(Plan-mode write dropping),加在那里是错的。已加在 `subagent/mod.rs:580` 的 `STRUCTURALLY_DISABLED` 数组,常量列表是唯一 source of truth。
  - `STRUCTURALLY_DISABLED` 列表加 `"ask_user_question"`
  - **验证**:`cargo test --lib permissions::tests_mode`(注:实际验证是 `cargo test --lib filter_strips_structurally_disabled`,该测试已扩展开 case 显式含 `ask_user_question`)

- [ ] **A5**. 修改 `app/src-tauri/src/state.rs`
  - `AppState` 加 `pub question_store: QuestionStore`
  - 初始化:`QuestionStore::new()` in `AppState::load`
  - `ChatEventSink` trait 加 `fn emit_tool_question(&self, payload: &ToolQuestionPayload)`(**sync,对齐 trait 现有 sync 设计**,见 design §5.2 实施记录)
  - `AppHandleSink` 实现 `emit_tool_question`(调用 `app.emit("tool:question", payload)`)
  - `MockEmitter` 实现 `emit_tool_question`(push 进 `Vec<ToolQuestionPayload>`)
  - **验证**:`cargo check` + 现有 `tests_agent_loop` / `tests_subagent` 测试不破

- [ ] **A6**. 修改 `app/src-tauri/src/agent/chat_loop.rs`
  - 在 tool 处理阶段增加 blocking tool 分支识别(`tool_name == "ask_user_question"`)
  - 调 `ask_user_question::execute_blocking(...)` 替代 `execute_tool(...)`
  - 把返回结果当作 tool_result 块追加到 messages(走现有路径)
  - **接受 turn 计数 +1**(v1 不重构 while 循环)
  - **无需改 `is_parallel_eligible`**(它是**白名单** `NAME_ELIGIBLE = [read_file, grep, glob, list_dir, use_skill]`,`ask_user_question` 不在白名单 → 天然返回 false → 整批自动走 L1a Serial;`dispatch_subagent` 同理,并非靠显式 `=> false` 分支)。**不要**加 `ask_user_question => false` 分支(那是黑名单心智模型,与现有机制不符)
  - **验证**:`cargo check` + `tests_ask_user_question`(F1 新建)跑通

- [ ] **A7**. 新建 `app/src-tauri/src/commands/question.rs`
  - `#[tauri::command] pub async fn resolve_tool_question(...)` 路由到 `state.question_store.resolve(...)`
  - `#[tauri::command] pub async fn get_pending_question(...)` 路由到 `state.question_store.get(...)` 返回 `Option<ToolQuestionPayload>`
  - resolve payload 解析 `Answer` / `Cancelled` 两种 union
  - 注册到 `lib.rs::run` 的 `tauri::generate_handler!`
  - **验证**:`cargo check`

### Phase B — Backend integration test(端到端验证)

- [ ] **B1**. *(合并到 F1,实施时以 F1 为准)* — 原 B1 与 F1 重复,且 F1 更完整(含 AC1' Serial 断言 + "跳过"术语 + turn +1 断言)。原 B1 的"turn 计数不因挂起递增"是**错误**断言(应为 +1,见 PRD R3 / design §6.3),已在此剔除。F1 落地即满足本步。

### Phase C — Frontend foundation(IPC 接入)

- [ ] **C1**. 新建 `app/src/types/toolQuestion.ts`(或放到 `app/src/types/index.ts`)
  - `Question` / `Option` / `ToolQuestionPayload` / `ToolQuestionAnswer` / `ToolQuestionResolvePayload` 类型定义
  - **验证**:`pnpm vue-tsc --noEmit` 通过

- [ ] **C2**. 新建 `app/src/stores/questionCards.ts`
  - `useQuestionCardsStore` Pinia store
  - state: `pendingBySession: Map<string, PendingQuestion>`
  - actions: `addPending / removePending / getPending / list`
  - **验证**:pnpm build 通过

- [ ] **C3**. 修改 `app/src/stores/streamController.ts`
  - 注册 Tauri listener:`listen<ToolQuestionPayload>("tool:question", ...)`
  - listener handler: `questionCardsStore.addPending(event.payload)`
  - 在 `loadSessionMessages` / `rehydrateMessages` 时合并 pending:invoke `get_pending_question(sessionId)` → 注入到对应 assistant turn 的 ToolUse 块下方
  - **验证**:pnpm build 通过 + `streamController.test.ts` 不破

- [ ] **C4**. 修改 `app/src/lib/tauri.ts`(或项目里 Tauri invoke 封装位置)
  - export `resolveToolQuestion(payload: ToolQuestionResolvePayload): Promise<void>`
  - **验证**:pnpm build 通过

### Phase D — Frontend Card 组件

- [ ] **D1**. 新建 `app/src/components/chat/AskUserQuestionCard.vue`
  - ⚠ **UI 红线(实现者必读)**:必须是消息流 inline card(`ToolCallCard` 同层)。**禁止** reka-ui `Dialog`/`Popover`/`AlertDialog`、**禁止** `<Teleport to="body">`、**禁止**浮层/遮罩/mask。复用 `ToolCallCard` 现有 Card 样式。违反 → AC10 失败(modal 无法跨 session 保留 pending,见 design §5.5)
  - props: `sessionId`, `toolUseId`, `questions`, `state: 'pending' | 'answered' | 'cancelled'`, `selectedAnswer?: QuestionAnswer[]`
  - template:单 card 多 section,每 section 一个 question
    - header chip(≤12 字符)+ 题干
    - 选项:radio(单选) / checkbox(多选)
    - description 在 label 下方
    - preview 在折叠面板(`<details>` 或 reka-ui Collapsible)
  - bottom:
    - pending 状态:"提交"按钮(整体提交,见 script setup)+ "跳过"按钮(wire `{"cancelled": true}`,避免和全局 Stop 按钮混淆)
    - answered 状态:显示已选项摘要 + 仍展示全部内容(展开保留)
    - cancelled 状态:显示"已跳过"提示
  - script setup(**整体提交语义**,对齐 wire §4.2 一次性 answer 数组):
    - 选项点击 → **仅累积本地 state**(单选 radio / 多选 checkbox,不 invoke)
    - "提交"按钮 → 收集**所有** question 答案 → 一次性 `invoke resolveToolQuestion({ answer })`(禁止单选即时 resolve)
    - "跳过"按钮 → `invoke resolveToolQuestion({ cancelled: true })`(一次性作用于整张 card)
    - 收到 ack 后本地切到 answered / cancelled 状态
  - **验证**:pnpm dev 起来 → 真实 chat 触发 → UI 正常

- [ ] **D2**. 新建 `app/src/components/chat/AskUserQuestionCard.test.ts`(vitest)
  - 渲染多 question section
  - 单选 / 多选切换
  - 答完状态切换(展开保留 + 已选项高亮)
  - "跳过"按钮触发 invoke
  - **验证**:`pnpm test AskUserQuestionCard`

### Phase E — Integration with MessageList

- [ ] **E1**. 修改 `app/src/components/chat/MessageItem.vue`(R22)
  - 增加 tool name → component 分发:`ask_user_question` → `<AskUserQuestionCard>`,其他 → `<ToolCallCard>`
  - `<AskUserQuestionCard>` 在 `<ToolCallCard>` 紧下方插入,共享 assistant turn 上下文
  - 从 `questionCardsStore.pendingBySession.get(sessionId)` 拿 pending payload
  - answered / cancelled 状态自然从 store 更新
  - **验证**:pnpm dev → 完整流程 + AC11 通过

### Phase F — Backend integration test(端到端验证)

- [ ] **F1**. 在 `app/src-tauri/src/agent/tests_chat_loop.rs` 加 `agent_loop_ask_user_question_*` 测试
  - ⚠ **实施记录(2026-06-30)**:实际测试文件是新建的 `app/src-tauri/src/agent/tests_ask_user_question.rs`(在 `agent/mod.rs` 注册),而非追加到 `tests_chat_loop.rs`(后者文件不存在,agent 测试按 `tests_*` 域拆分)。
  - Mock provider 返回 `ToolUse(ask_user_question)` + `ToolUse(shell)` 序列(同 batch)
  - MockEmitter 捕获 `tool:question` 事件
  - 用 `tokio::sync::oneshot::Sender` 模拟 user 回答(`MockQuestionStore` 或直接 register 后手动 send)
  - 断言:整批走 Serial + turn 计数 +1 + tool_result 正确进 messages + LLM 看到答案继续推理
  - 覆盖路径:
    - happy path(用户答)
    - 用户跳过
    - session cancel token
    - 二次并发 register → AlreadyPending
    - **含 `ask_user_question` 的批次走 Serial + 按 LLM 声明顺序执行**(AC1')
  - ⚠ **实施记录(2026-06-30)**:`already_pending` 测试通过**预注册遗留 pending** 触发,而非 same-turn 二次调。原因:Serial 分支同 turn 内 tool_use 串行处理,第二次 register 在同 turn 不可达。预注册场景匹配生产真实触发(前次 `run_chat_loop` 跨 user-message 边界留下 pending = PRD AC9)。`question_store.rs::tests` 内的 `register_already_pending` 单元测试仍直接锁定 gate。
- **验证**:`cd app/src-tauri && cargo test --lib tests_ask_user_question`

- [ ] **F2**. 加 `get_pending_question` command 行为测试
  - register 后 get → Some(payload)
  - resolve 后 get → None
  - 不存在的 session get → None
- **验证**:`cd app/src-tauri && cargo test --lib question`

### Phase G — Cross-cutting validation

- [ ] **G1**. 检查 `app/src/components/chat/MessageItem.vue` 的 DOM 结构
  - 确认 `AskUserQuestionCard` 是消息流 child,无 portal 到 body、无遮罩
  - 满足 AC10(无 modal 验证)+ AC11(tool name 分发)

- [ ] **G2**. 跑全量测试
  - `cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test`
  - `pnpm test`
  - `pnpm vue-tsc --noEmit`
  - `cd app/src-tauri && cargo check`

## 2. Validation Commands

```bash
# Backend 单测
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib question_store
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib ask_user_question
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib permissions::tests_mode
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib tests_ask_user_question

# Backend 全测
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test

# Frontend 类型 + 单测
cd app && pnpm vue-tsc --noEmit
cd app && pnpm test

# 前端 build
cd app && pnpm build

# 端到端(WSL 环境)
cd app && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" pnpm tauri dev
# → 真实 chat 触发 ask_user_question → 验证 inline card + session switch 保留 + cancel
```

## 3. Risky Files / Rollback Points

| 文件 | 风险 | 回滚点 |
|---|---|---|
| `app/src-tauri/src/agent/chat_loop.rs` | 高 — 大文件,blocking tool 分支插入位置要准 | 修改后立刻跑 `tests_ask_user_question`(F1)+ 全套 `tests_agent_loop` 验证;git revert 单 commit |
| `app/src-tauri/src/state.rs` | 中 — ChatEventSink trait 加方法影响所有 sink 实现 | A5 阶段分别跑 `AppHandleSink` 编译 + `MockEmitter` 测试 |
| `app/src/stores/streamController.ts` | 中 — LRU 合并逻辑影响 session 加载性能 | C3 阶段做轻量级 stress test(模拟 5 个 session 切来切去) |
| `app/src/components/chat/MessageList.vue` | 中 — inline card 插入位置影响消息流渲染 | E1 阶段手工视觉测试 + DOM 结构断言(AC10) |

### 整体回滚清单

如果 v1 上线后整体不达预期,删除:
1. `app/src-tauri/src/tools/ask_user_question.rs`
2. `app/src-tauri/src/agent/question_store.rs`
3. `app/src/components/chat/AskUserQuestionCard.vue`
4. `app/src/stores/questionCards.ts`
5. `app/src-tauri/src/commands/question.rs`
6. 各文件的改动 revert(`tools/mod.rs` / `state.rs` / `chat_loop.rs` / `mode.rs` / `streamController.ts` / `MessageList.vue`)

无 schema migration,无 DB 表,无 audit 表修改,回滚干净。

## 4. Follow-up Checks Before `task.py start`

- [ ] prd.md 通过 PRD convergence pass(已做)
- [ ] design.md 覆盖架构 / 数据流 / wire 协议 / 状态管理 / 并发 / 回滚(已做)
- [ ] implement.md 覆盖步骤清单 / 验证命令 / 风险点(本文档)
- [ ] implement.jsonl + check.jsonl 各 ≥1 条真实条目(实施前 curate)
- [ ] 用户 review 通过 design.md + implement.md
- [ ] run `task.py start`

## 5. Estimated Effort

> 保留 R9–R11(session 挂起 + `get_pending_question` 作 source of truth + LRU 淘汰后校正),前端复杂度显著高于"切走即 cancel"方案——以下估算已含这部分。

- Phase A (Backend foundation): ~3-4 小时
- Phase B/F (Integration test,含 AC1' Serial + AC5' get_pending): ~2-3 小时
- Phase C (Frontend foundation + session 切换 source-of-truth 合并): ~2-3 小时
- Phase D (Card 组件 + 整体提交 + 状态保留): ~3-4 小时
- Phase E (MessageList integration): ~1 小时
- Phase G (Cross-cutting validation): ~1-2 小时

**Total: ~12-16 小时(约 2 工作日)**

## 6. Out of Implement Scope(v2 候选)

- Timeout + auto-decide
- `tool_questions` 专用表
- Worker subagent 反问(永久禁用,除非明确开启)
- Yolo mode auto-pick 策略
- 跨 session question 模板 / 历史
- 自定义自由文本回答
- 实时协作
- App crash 恢复 pending