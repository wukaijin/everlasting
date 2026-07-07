# Implement: `request_mode_change` tool

> 配套 PRD: [prd.md](./prd.md) · Design: [design.md](./design.md)
>
> 实施时严格按本文件 §1 ordered checklist 推进;每步独立可验证。

## 1. Ordered Implementation Checklist

### Phase A — Backend foundation(无前端依赖)

#### A1. 扩展 `app/src-tauri/src/agent/question_store.rs`(store 升级为 PendingInteraction 互斥)
- [ ] 加 `pub struct ModeChangePayload { session_id, tool_use_id, target_mode, current_mode, reason, ts }`(serde snake_case)
- [ ] 加 `pub enum PendingInteraction { Question(ToolQuestionPayload), ModeChange(ModeChangePayload) }`(`#[serde(tag = "kind", rename_all = "snake_case")]`)
- [ ] 加 `pub enum InteractionKind { Question, ModeChange }` + serde
- [ ] 加 `pub struct PendingInteractionEntry { kind: InteractionKind, payload: PendingInteraction }`
- [ ] 加 `pub enum InteractionResponse { Answered(serde_json::Value), Cancelled }` —— 替代原 `QuestionResponse`,oneshot 通道承载新类型
- [ ] `PendingQuestion` 加 `kind: InteractionKind` 字段(默认 Question,back-compat)
- [ ] `register` 签名扩展:`(session_id, tool_use_id, payload: PendingInteraction) -> Result<oneshot::Receiver<InteractionResponse>, QuestionStoreError>`,**互斥语义不变**(`contains_key` → `AlreadyPending`)
- [ ] `resolve` 签名扩展:`(session_id, response: InteractionResponse) -> Result<PendingInteractionEntry, QuestionStoreError>`(返回 entry 让 caller 知道 resolve 了哪种 kind)
- [ ] `get_payload` 返回 `Option<PendingInteractionEntry>`(替代原 `Option<ToolQuestionPayload>`)
- [ ] `remove` 保留(行为不变)
- [ ] 内联 `#[cfg(test)] mod tests` 新增: `register_question_then_mode_change_pending_returns_already_pending`、`resolve_mode_change_returns_entry_kind_mode_change`
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib question_store`

#### A2. 新建 `app/src-tauri/src/tools/request_mode_change.rs`
- [ ] `RequestModeChangeInput { target_mode: String, reason: Option<String> }`(snake_case serde)
- [ ] 常量:`MAX_REASON_LEN = 500`、`VALID_MODES = ["edit", "plan", "yolo"]`
- [ ] `definition() -> ToolDef`:`enum: ["edit", "plan", "yolo"]`,description 完整版
- [ ] `validate(input) -> Result<(), ValidationError>`:`target_mode` 在 enum / `reason` ≤500 / `target_mode` 非空
- [ ] `pub async fn execute_blocking(input, session_id, tool_use_id, current_mode, store, sink, cancel) -> BlockingToolResult`
  - 流程:1) parse + validate 失败 short-circuit `is_error: true`;2) **noop 判定**:`target_mode == current_mode → 立即 return `(json noop, false, _, None)`,写 `mode_change_requested{noop:true}` audit;3) record_audit(`mode_change_requested`);4) `store.register(sid, tuid, PendingInteraction::ModeChange(payload))` 失败 → AlreadyPending 错误;5) `sink.emit_mode_change_request(&payload)`;6) `tokio::select!{ cancel | oneshot.recv() }`:cancel arm → store.remove + `{"cancelled_by_session": true}`;oneshot arm:
    - `InteractionResponse::Answered(json)` → 写 `mode_change_allowed` audit → 返回 `(json_allowed, false, _, None)`
    - `InteractionResponse::Cancelled` → 写 `mode_change_denied` audit → 返回 `({"cancelled_by_user": true}, true, _, None)`
- [ ] 内联 `#[cfg(test)] mod tests`:
  - `validation_empty_target_mode_short_circuits`
  - `validation_target_mode_out_of_enum_short_circuits`(`"background"`, `"yolo "`, `""`)
  - `validation_reason_too_long_short_circuits`(>500)
  - `noop_target_equals_current_returns_noop_marker`(current_mode == target)
  - `happy_path_registers_emits_and_returns_allowed`
  - `deny_path_returns_cancelled_user_marker`
  - `cancel_arm_returns_session_cancelled_marker`
  - `already_pending_returns_structured_error`(同 session 已有 pending question → AlreadyPending)
  - `validate_accepts_well_formed_input`(纯函数)
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib request_mode_change`

#### A3. 修改 `app/src-tauri/src/tools/mod.rs`
- [ ] `builtin_tools()` 列表在 `ask_user_question` 旁加 `request_mode_change::definition()`
- [ ] `execute_tool_inner` 的 `match` **不加** `request_mode_change` 分支(chat_loop 拦截走 `execute_blocking`,对齐 ask_user_question 模式;在 dispatch 处加注释"此 tool 由 chat_loop 直接处理")
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check`

#### A4. 修改 `app/src-tauri/src/agent/subagent/mod.rs`(worker 禁用)
- [ ] `STRUCTURALLY_DISABLED` 数组加 `"request_mode_change"`
- [ ] `filter_strips_structurally_disabled` 既有测试加 case:输入含 `request_mode_change` → 输出不含
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib filter_strips_structurally_disabled`

#### A5. 修改 `app/src-tauri/src/state.rs`(ChatEventSink 扩展)
- [ ] `ChatEventSink` trait 加 `fn emit_mode_change_request(&self, payload: &ModeChangePayload)`(sync,沿用 sync 设计)
- [ ] `AppHandleSink::emit_mode_change_request` → `app.emit("mode:change:request", payload)`
- [ ] `MockEmitter::emit_mode_change_request` → push 进 `Vec<ModeChangePayload>`(tests 用)
- [ ] `SubagentBufferSink::emit_mode_change_request` → no-op(worker 禁用)
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check`

#### A6. 修改 `app/src-tauri/src/agent/chat_loop.rs`(拦截分支)
- [ ] 在 tool 处理阶段、line 3110 下方新增 `else if name == "request_mode_change"` 分支(在 ask_user_question 旁,沿用完全相同的"control-flow tool interception pattern"注释)
- [ ] 调 `request_mode_change::execute_blocking(input, &session_id, id, session_mode, &store, &sink, &token).await`
- [ ] 返回的 `(content, is_error, _update, exit_code)` 走与 ask_user_question 同一处理:`record_tool_executed_audit` + `emit_tool_result` + `ContentBlock::ToolResult` push + cancel 标记
- [ ] **is_parallel_eligible 不动**(白名单机制,request_mode_change 默认不在 → 整批串行)
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check` + `cargo test --lib tests_ask_user_question` 不破

#### A7. 修改 `app/src-tauri/src/commands/permissions.rs`(新 IPC `resolve_mode_change`)
- [ ] `#[tauri::command] pub async fn resolve_mode_change(state, session_id, tool_use_id, allow: bool) -> Result<db::SessionRow, AppCommandError>`
- [ ] 内部流程:
  1. 调内部 `set_session_mode_inner(&state, &session_id, target_mode)` 纯函数(从 `set_session_mode` IPC handler 抽出,接收 mode 字符串,不含前端类型转换)→ 返回 `Result<SessionRow, AppCommandError>`(落库 + mode_changed audit + Yolo root guard 全沿用)
  2. **不论成败**都 `state.question_store.resolve(&session_id, if allow && ok { Answered(json) } else { Cancelled })`
  3. 成功时额外 `record_audit("mode_change_allowed", {prev, new, target})`
  4. 失败时 `record_audit("mode_change_denied", {target, reason: "yolo_root_guard" | "db_error"})`
  5. 返回 `Result<SessionRow>`(成功) / 透传 `AppCommandError`(失败)
- [ ] 抽出 `pub(crate) async fn set_session_mode_internal(state, session_id, mode) -> Result<SessionRow, AppCommandError>`(从原 `set_session_mode` IPC handler 抽出,`set_session_mode` 改为薄壳调它)
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check`

#### A8. 修改 `app/src-tauri/src/commands/question.rs`(`get_pending_interaction` 新 IPC)
- [ ] 加 `#[tauri::command] pub async fn get_pending_interaction(state, session_id) -> Result<Option<PendingInteractionEntry>, AppCommandError>`(替代 `get_pending_question`,但保留旧 IPC 软弃用)
- [ ] `get_pending_question` 标记 `#[deprecated(note = "use get_pending_interaction")]`(不删,向后兼容)
- [ ] `resolve_tool_question` 不动(原 `QuestionResponse::Answered | Cancelled` 路径保留)
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check`

#### A9. 修改 `app/src-tauri/src/commands/mod.rs` 与 `lib.rs::run`(注册新 IPC)
- [ ] `commands/mod.rs` `tauri::generate_handler!` 加 `resolve_mode_change`、`get_pending_interaction`
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check`

#### A10. 修改 `app/src-tauri/src/db/audit.rs`(新 audit kind)
- [ ] `AuditKind` enum 加 `ModeChangeRequested`、`ModeChangeAllowed`、`ModeChangeDenied` 3 个 variant
- [ ] `as_str()` 实现 + 反向 `from_str`(影响 search filter;若 audit 不强校验,可不加 from_str)
- [ ] 验证 `record_audit_event` 已支持任意 string kind,新 variant 自动可用
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib audit`

### Phase B — Frontend foundation(IPC 接入 + store 扩展)

#### B1. 修改 `app/src/stores/questionCards.types.ts`(类型扩展)
- [ ] 新增 `ModeChangePayload` interface:`sessionId, toolUseId, targetMode, currentMode, reason?, ts`
- [ ] 新增 `ModeChangeResolvePayload` interface:`sessionId, toolUseId, allow: boolean`
- [ ] 新增 `PendingInteraction` discriminated union:`{ kind: "question", payload: ToolQuestionPayload } | { kind: "mode_change", payload: ModeChangePayload }`
- **验证**:`cd app && pnpm vue-tsc --noEmit`

#### B2. 修改 `app/src/utils/toolQuestion.ts` 与新建 `app/src/utils/toolModeChange.ts`
- [ ] `toolQuestion.ts` 不动(保留旧 `resolveToolQuestion`)
- [ ] **新建** `app/src/utils/toolModeChange.ts`:
  - `export async function resolveModeChange(payload: ModeChangeResolvePayload): Promise<SessionRow>`(返回新 SessionRow,前端 `setCurrentSession(row)` 刷新 mode)
  - `export async function getPendingInteraction(sessionId: string): Promise<PendingInteraction | null>`
- **验证**:`cd app && pnpm vue-tsc --noEmit`

#### B3. 修改 `app/src/stores/questionCards.ts`(store 扩展)
- [ ] `pendingBySession: Map<string, PendingInteraction>`(替代原 `Map<string, ToolQuestionPayload>`)
- [ ] `currentModeBySession: Map<string, SessionMode>`(新增,UI 反馈"当前 mode"用)
- [ ] actions:
  - `addPending(sessionId, payload: PendingInteraction)`(替代原 addPending)
  - `removePending(sessionId)`
  - `getPending(sessionId): PendingInteraction | undefined`
  - **`resolveModeChange(sessionId, toolUseId, allow): Promise<SessionRow>`**:调 IPC,成功 → `setCurrentSession(row)` 刷新 → `removePending(sessionId)` → return row
- [ ] `getPendingInteraction(sessionId): Promise<PendingInteraction | null>` action(调 IPC,合并到 `pendingBySession`)
- **验证**:`cd app && pnpm vue-tsc --noEmit` + `pnpm test questionCards` 不破

#### B4. 修改 `app/src/stores/streamController.ts`(listener + rehydrate)
- [ ] 注册新 listener:`listen<ModeChangePayload>("mode:change:request", ...)` → `questionCardsStore.addPending(sid, { kind: "mode_change", payload })`
- [ ] `ensureLoaded` / `rehydrateMessages` 时改调 `getPendingInteraction(sid)` 替代 `get_pending_question`(统一 source of truth)
- **验证**:`cd app && pnpm test streamController` 不破

### Phase C — Frontend Card 组件

#### C1. 新建 `app/src/components/chat/RequestModeChangeCard.vue`
- [ ] **UI 红线(实现者必读,违反即 AC10 失败)**:inline card,消息流 child,无 portal / 无遮罩 / 无 reka-ui Dialog。复用 `ToolCallCard` 现有 Card 样式系统(`--color-bg-surface` / `--color-bg-border` / `--radius-md` / spacing tokens)。**仅** Yolo 二次确认 modal 例外(沿用现有 `useChatStore.pendingYoloConfirm`)。
- [ ] props:`sessionId, toolUseId, targetMode, currentMode, reason?, state: 'pending' | 'allowed' | 'denied', allowedMode?`
- [ ] emits:`(e: "allowed", newMode: SessionMode)`, `(e: "denied")`
- [ ] template(自上而下):
  - Header chip:目标 mode 名字(`"切换到 Yolo"`)+ 状态色(plan 蓝 / edit 灰 / yolo 红)
  - Reason 文本(`<p class="reason">`,wrap,无 markdown,纯文本,可选)
  - 状态对比(`当前: ${currentMode}` → `目标: ${targetMode}`,pending 状态显示)
  - Bottom action row(pending):两按钮等宽并列,允许在左,允许按钮颜色按 target_mode
  - allowed 状态:`已切换到 [mode]` pill + 切换前 → 后对比
  - denied 状态:`已拒绝` pill
- [ ] script setup:
  - 选项点击逻辑:
    - 点"允许" → 调 `useChatStore.requestSetMode(sid, targetMode)` 走 Yolo 二次 modal 路径(若 targetMode === "yolo" → 弹 modal;否则直接落库)→ modal confirm 后调 `questionCardsStore.resolveModeChange(sid, tuid, allow=true)` → emit("allowed", newMode)
    - 点"拒绝" → `questionCardsStore.resolveModeChange(sid, tuid, allow=false)` → emit("denied")
  - 错误处理:`resolveModeChange` 失败 → toast 提示(沿用 `useErrorBus`)
- [ ] **NOOP 处理**:不弹 card(后端不 emit),UI 透明
- [ ] **Yolo 二次 modal dispatch**:点"允许" + targetMode="yolo" → 不直接调 `resolveModeChange`,而是 `useChatStore.requestSetMode(sid, "yolo")` → modal 弹 → `confirmYolo` 成功 → `resolveModeChange(sid, tuid, true)`。modal 取消 → `resolveModeChange(sid, tuid, false)` 走拒绝路径
- **验证**:`cd app && pnpm dev` 起 Tauri dev → 真实 chat 触发 → UI 正常(本步骤临时手动测,自动化在 C2)

#### C2. 新建 `app/src/components/chat/RequestModeChangeCard.test.ts`(vitest)
- [ ] 渲染 pending 状态(两按钮 + reason 显示)
- [ ] 点"允许"(非 Yolo)→ 调 `resolveModeChange(allow=true)` + 切到 allowed 状态
- [ ] 点"允许"(Yolo)→ 调 `useChatStore.requestSetMode` + 触发 `pendingYoloConfirm`
- [ ] 点"拒绝"→ 调 `resolveModeChange(allow=false)` + 切到 denied 状态
- [ ] allowed / denied 状态 pill 渲染
- [ ] targetMode 颜色映射(plan 蓝 / edit 灰 / yolo 红)
- **验证**:`cd app && pnpm test RequestModeChangeCard`

### Phase D — Integration with MessageItem

#### D1. 修改 `app/src/components/chat/MessageItem.vue`(R18)
- [ ] tool_use 块分发:`name === "request_mode_change"` → `<RequestModeChangeCard>`,其他 → `<ToolCallCard>`(不变)
- [ ] `<RequestModeChangeCard>` 在 `<ToolCallCard>` 紧下方插入,共享 assistant turn 上下文
- [ ] 从 `questionCardsStore.getPending(sid)` 拿 `pendingBySession` 数据
- [ ] passed props 完整:`sessionId, toolUseId, targetMode, currentMode, reason, state, allowedMode?`
- **验证**:`pnpm dev` → 完整流程 + AC11 通过 + 手工 DOM 检查 AC10(无 portal)

### Phase E — Backend integration test(端到端验证)

#### E1. 新建 `app/src-tauri/src/agent/tests_request_mode_change.rs`
- [ ] 注入 `mod tests_request_mode_change;` 到 `app/src-tauri/src/agent/mod.rs`
- [ ] 测试 case 列表:
  - `agent_loop_request_mode_change_serializes_with_shell_in_same_batch`(AC1)
  - `agent_loop_request_mode_change_noop_skips_card`(AC7)
  - `agent_loop_request_mode_change_session_cancel_returns_cancelled_marker`(AC16)
  - `agent_loop_request_mode_change_already_pending_returns_structured_error`(AC9)
  - `agent_loop_request_mode_change_happy_path_updates_mode_in_db`(AC4)
  - `agent_loop_request_mode_change_yolo_root_guard_denies_even_after_user_allow`(AC8')
  - `agent_loop_request_mode_change_records_mode_change_requested_allowed_denied_audits`(AC12)
- [ ] Mock provider 模拟 LLM 响应;MockEmitter 捕获 `mode:change:request` 事件;oneshot::Sender 模拟 user 决策
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib tests_request_mode_change`

#### E2. 加 `get_pending_interaction` command 行为测试
- [ ] register question → get → `Some({kind: "question", payload})`
- [ ] register mode_change → get → `Some({kind: "mode_change", payload})`
- [ ] resolve → get → `None`
- [ ] 不存在 session → get → `None`
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib question`

#### E3. 加 `resolve_mode_change` IPC handler 测试
- [ ] `allow=true` + `target_mode=plan` → DB mode 更新 + `mode_change_allowed` audit + return SessionRow
- [ ] `allow=true` + `target_mode=yolo` + 非 root → 同上,Yolo 不走二次 modal(IPC 路径,二次 modal 在前端)
- [ ] `allow=true` + `target_mode=yolo` + root → 拒绝路径,tool_result = `cancelled_by_user`
- [ ] `allow=false` → 拒绝路径,`mode_change_denied` audit
- [ ] `get_session` 不存在 → InvalidRequest
- **验证**:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib permissions::tests_resolve_mode_change`

### Phase F — Cross-cutting validation

#### F1. 跑全量测试
- [ ] `cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test`(所有 cargo 测试)
- [ ] `cd app && pnpm vue-tsc --noEmit`
- [ ] `cd app && pnpm test`(vitest)
- [ ] `cd app && pnpm build`

#### F2. 端到端验证(manual,WSL 环境)
- [ ] `cd app && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" pnpm tauri dev`
- [ ] **Happy path**:plan 模式 → LLM 申请 edit → 卡片 → 允许 → ModeSelect chip 切到 edit → 后续 LLM 能写代码
- [ ] **Noop**:edit 模式 → LLM 申请 edit → 立即 tool_result(无 card)
- [ ] **Deny**:LLM 申请 plan → 卡片 → 拒绝 → LLM 看到 `cancelled_by_user`
- [ ] **Yolo 二次**:LLM 申请 yolo → 卡片 → 允许 → 二次 modal → 确认 → 落库;取消 → 拒绝路径
- [ ] **Session 切换保留**:session A 有 pending → 切 B → 切回 A → card 仍在
- [ ] **Worker 不可见**:dispatch subagent,worker tool list 不含 `request_mode_change`
- [ ] **Yolo root guard**:以 root 跑测试(WSL root 或 sudo),user 允许 Yolo → 二次 modal "确认" 按钮 disabled + 红字提示

#### F3. 文档同步
- [ ] `.trellis/spec/backend/tool-contract.md` 加 `request_mode_change` 工具 schema section
- [ ] `.trellis/spec/backend/permission-layer.md` 加 "Scenario: request_mode_change 写操作" section
- [ ] `.trellis/spec/backend/agent-loop-architecture.md` chat_loop 拦截点列表加 1 行
- [ ] `.trellis/spec/frontend/chat.md` inline card 红线重申;`RequestModeChangeCard` 红线
- [ ] `docs/IMPLEMENTATION.md §4` 加 ADR 条目:`request_mode_change` tool(Yolo 二段守门 + 共用 QuestionStore + 双 IPC 时序)
- [ ] `docs/ROADMAP.md` V2 路线图第二档补 1 行

## 2. Validation Commands

```bash
# Backend 单测
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib question_store
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib request_mode_change
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib filter_strips_structurally_disabled
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib permissions::tests_resolve_mode_change
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib tests_request_mode_change
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib question
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib audit
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib ask_user_question  # 回归
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib tests_ask_user_question  # 回归

# Backend 全测
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test

# Frontend 类型 + 单测
cd app && pnpm vue-tsc --noEmit
cd app && pnpm test RequestModeChangeCard
cd app && pnpm test questionCards
cd app && pnpm test streamController
cd app && pnpm test chatMode  # 回归
cd app && pnpm test AskUserQuestionCard  # 回归

# Frontend build
cd app && pnpm build

# 端到端(WSL 环境)
cd app && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" pnpm tauri dev
# → 真实 chat 触发 request_mode_change → 验证 inline card + Yolo 二次 modal + session switch 保留 + cancel
```

## 3. Risky Files / Rollback Points

| 文件 | 风险 | 回滚点 |
|---|---|---|
| `app/src-tauri/src/agent/question_store.rs` | **高** — store 签名升级(register 接受 `PendingInteraction` enum)影响 ask_user_question 既有 caller;blask radius 1 文件 | A1 阶段分别跑 `ask_user_question` 既有 12+ tests;不破 → 进入 A2 |
| `app/src-tauri/src/agent/chat_loop.rs` | **高** — 大文件,blocking tool 分支插入位置要准;新增分支与 ask_user_question 并列 | A6 阶段立刻跑 `tests_ask_user_question` + 全套 `tests_agent_loop`;git revert 单 commit |
| `app/src-tauri/src/commands/permissions.rs` | 中 — `set_session_mode` 拆分为 IPC handler + `set_session_mode_internal` 纯函数,影响 user 主动切 mode 路径 | A7 阶段跑既有 `chatMode.test.ts` 回归(前端 requestSetMode 不变);rust `tests_permissions` 不破 |
| `app/src-tauri/src/state.rs` | 中 — `ChatEventSink` trait 加方法影响所有 sink 实现(AppHandleSink / MockEmitter / SubagentBufferSink) | A5 阶段分别编译三处 + 跑 mock 测试 |
| `app/src/stores/questionCards.ts` | 中 — store 字段类型变化(`Map<string, ToolQuestionPayload>` → `Map<string, PendingInteraction>`)影响所有 caller | B3 阶段跑既有 `questionCards` 测试 + `streamController` 测试 + 手工 `AskUserQuestionCard.test.ts` 回归 |
| `app/src/components/chat/MessageItem.vue` | 中 — tool name 分发,影响所有 tool_use 渲染 | D1 阶段手工视觉测试 + DOM 结构断言(AC10)|
| `app/src/components/chat/RequestModeChangeCard.vue` | 中 — 新组件,Yolo 二次 modal dispatch 路径复杂 | C2 阶段 vitest 覆盖 happy path / Yolo / 拒绝 3 路径 |

### 整体回滚清单

如果 v1 上线后整体不达预期,删除:
1. `app/src-tauri/src/tools/request_mode_change.rs`
2. `app/src/components/chat/RequestModeChangeCard.vue` + `.test.ts`
3. `app/src/utils/toolModeChange.ts`
4. `app/src-tauri/src/agent/tests_request_mode_change.rs`
5. 各文件改动 revert:
   - `tools/mod.rs`(`builtin_tools` 注册)
   - `agent/subagent/mod.rs`(`STRUCTURALLY_DISABLED`)
   - `agent/chat_loop.rs`(拦截分支)
   - `agent/question_store.rs`(回到原 register / QuestionResponse 签名)
   - `state.rs`(ChatEventSink 新方法)
   - `commands/permissions.rs`(回到原 `set_session_mode`,无内部函数)
   - `commands/question.rs`(无 `get_pending_interaction`)
   - `commands/mod.rs` + `lib.rs`(IPC 注册)
   - `db/audit.rs`(3 个新 variant)
   - `stores/questionCards.ts` + `questionCards.types.ts`
   - `stores/streamController.ts`(回到原 `get_pending_question`)
   - `components/chat/MessageItem.vue`(去掉 `request_mode_change` 分发)
6. `docs/IMPLEMENTATION.md §4` ADR 条目 revert

无 schema migration,无 DB 表,无 audit 表修改,**回滚干净**。

## 4. Follow-up Checks Before `task.py start`

- [ ] prd.md 通过 PRD convergence pass(已做 — 9 个 R*,14 个 AC,Out of Scope + Notes 完整)
- [ ] design.md 覆盖架构 / 数据流(5 时序) / wire 协议 / 状态管理 / 并发 / 回滚(已做 — 11 节)
- [ ] implement.md 覆盖步骤清单(Phase A–F, 31 步)/ 验证命令 / 风险点(本文档)
- [ ] implement.jsonl + check.jsonl 各 ≥1 条真实条目(实施前 curate,见 §5)
- [ ] 用户 review 通过 design.md + implement.md
- [ ] `task.py start`

## 5. Estimated Effort

> 保留 Yolo 二段守门 + 共用 QuestionStore + 双 IPC 时序(对比直接落库的简单方案 +30% 时间,见 design §6 决策变更),估算已含这部分。

- Phase A (Backend foundation, 10 步): ~4-5 小时
  - A1 store 升级: 1 小时(签名 + 既有 12+ tests 修复)
  - A2 新工具 + 9 tests: 1.5 小时
  - A3-A6: 1 小时
  - A7 IPC handler 重构(`set_session_mode` 拆分 + `resolve_mode_change`): 1 小时
  - A8-A10: 0.5 小时
- Phase B (Frontend foundation, 4 步): ~2 小时
- Phase C (Card 组件, 2 步): ~2-3 小时
- Phase D (MessageItem integration): ~0.5 小时
- Phase E (Integration tests, 3 步): ~3-4 小时
- Phase F (Validation + docs): ~1.5-2 小时

**Total: ~13-17 小时(约 2 工作日)**

## 6. Out of Implement Scope(v2 候选)

- Timeout + auto-decide
- Tool 内直接落库方案(本期走 IPC 复用,见 design §6 决策变更理由)
- LLM 申请 `background` 模式(enum 永远不暴露)
- 跨 session mode 同步 / 切回 user 上次 mode
- App crash 恢复 pending
- **multi-select target_mode**(单选;LLM 不能一次申请多个 mode,语义模糊)
- **reason 多语言**(v1 接受 reason 是单语言文本;UI 不翻译)
- **`for turn in 1..=turn_limit` 重构 while 循环零消耗**(v1 接受 turn +1,见 PRD R3)

## 7. Cross-Phase Coordination Notes

### A1 → A2 时序关键

A1 改 store 签名后,**A2 直接用新签名**,避免反复改写。A1 必须先 commit,A2 才能基于新签名写 `execute_blocking`。

### A7 拆分 `set_session_mode_internal` 的 blast radius

- 现状:`set_session_mode` IPC 是单函数,逻辑全部在 handler 内
- 拆分:抽 `set_session_mode_internal(state, sid, mode) -> Result<SessionRow, AppCommandError>` 纯函数,handler 变薄壳
- 影响:`set_session_mode` IPC 行为不变(返回 `SessionRow`,写 `mode_changed` + Yolo transition audit)
- 风险:低;逻辑 1:1 搬迁,既有 `chatMode.test.ts` 覆盖 IPC 行为
- 验证:A7 完成后跑 `chatMode.test.ts` 6 case + `permissions::tests_set_session_mode` 既有 tests

### Yolo 二次 modal 的前后端时序

**关键不变量**:`set_session_mode` IPC 是**唯一**落库入口,user 主动 / LLM 申请两条路径共用。

```
[LLM 申请 yolo 路径]
前端 card "允许" → useChatStore.requestSetMode(sid, "yolo")
  ↓ pendingYoloConfirm = true, 弹 modal
[二次 modal "确认"]
useChatStore.confirmYolo()
  ↓ invoke("set_session_mode", { sid, mode: "yolo" })
后端 set_session_mode IPC handler
  ↓ 调 set_session_mode_internal → DB UPDATE + mode_changed audit + Yolo root guard
  ↓ 返回 SessionRow
[前端 confirmYolo 拿 row]
  ↓ invoke("resolve_mode_change", { sid, tuid, allow: true })
后端 resolve_mode_change IPC handler
  ↓ 调 set_session_mode_internal(sid, target_mode) [实际无副作用,已落库]
  ↓ store.resolve(sid, Answered(json)) → oneshot 解除
  ↓ record_audit("mode_change_allowed")
  ↓ 返回 SessionRow
[前端 resolveModeChange 拿 row]
  ↓ setCurrentSession(row) 刷新 mode
  ↓ questionCardsStore.removePending(sid)
  ↓ agent loop 拿到 tool_result = {"allowed": true, "prev_mode": "plan", "new_mode": "yolo"}
```

**注**:`resolve_mode_change` handler **仍**调 `set_session_mode_internal` 一次(本期 IPC 设计,即使已知 DB 已是新 mode)。理由:统一行为路径,IPC handler 不知道前端是 user 主动 / LLM 申请 / 别处,保持幂等性(`mode_changed` audit 在 DB 层是 UPDATE 的副作用,二次调用是 no-op)。若发现 audit 噪音,follow-up 让 IPC handler 检测"DB 已是 target_mode" 跳过 audit 写。

### Yolo root guard 失败路径(双重保险)

```
[LLM 申请 yolo,user 允许]
card "允许" → requestSetMode(sid, "yolo")
  ↓ Yolo 二次 modal 弹(modal 内 "确认" 按钮根据 is_running_as_root 检测 enabled/disabled)
  ↓ 若 root:disabled + 红字 "Cannot enable Yolo as root"
  ↓ 若非 root:enabled
[若非 root + 确认]
  → set_session_mode(yolo) → 落库 → resolve_mode_change(true) → 完
[若 root + 点击 disabled 按钮] → 按钮无响应,用户必须取消 modal
  → 取消 modal → useChatStore.cancelYolo() → modal 关
  → requestModeChange action 走"拒绝"路径:resolve_mode_change(false) → store resolve(Cancelled) → audit denied → tool_result cancelled_user
```

**前端 root 检测**:`is_running_as_root` 需要后端 IPC 暴露(新增 `is_running_as_root() -> bool` 简单 IPC,或复用现有 `set_session_mode` 的 `is_running_as_root` 在二次 modal 显示时检测)。**简化路径**:二次 modal 始终显示,但后端 `set_session_mode` IPC 本身有 root guard,失败时返回 `Cannot enable Yolo as root` 错误 → `confirmYolo` catch 错误 → `resolveModeChange(sid, tuid, false)` 走拒绝路径 + toast "Cannot enable Yolo as root"。**决策 = 简化路径**,不新增 IPC,后端一处守门,前端透传错误。

## 8. Trellis Workflow Notes

- 本任务使用 `07-07-07-07-request-mode-change-tool` 任务目录
- Phase 2 实施时 dispatch `trellis-implement` subagent,每步独立 commit
- Phase 2 中段(Phase A 完成后)dispatch `trellis-check` 一次,确认 store 升级 + 既有 tests 零回归
- Phase 2 末段(Phase E 完成后)dispatch `trellis-check` 一次,确认全量测试绿
- Phase 3 (3.3 spec update) 同步 `.trellis/spec/*` + `docs/*` 文档
- 完工后 `chore(task): archive 07-07-request-mode-change-tool`
