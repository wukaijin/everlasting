# request_mode_change tool: LLM 申请 mode 切换

## Goal

让 LLM 在主循环里向用户**申请切换当前 session 的 mode**(edit / plan / yolo),用户通过 inline message card 看到目标 mode + 理由,选择"允许"或"拒绝"。允许 → 复用现有 `set_session_mode` IPC 的副作用(DB 持久化 + 审计 + Yolo 安全守门 + 模式 prefix 切换);拒绝 → `is_error: true` + `{"cancelled_by_user": true}` 回灌,LLM 自行决定下一步。

只服务**主 session**,worker subagent 不适用(禁问)。

## 背景与动机

### 现状(mode 系统已就绪)

Mode 是 per-session 状态(`sessions.mode` TEXT 列,枚举 `edit` / `plan` / `yolo` / `background`),三种 user-facing 档:
- `Edit`: 默认,full access
- `Plan`: ⑧a 三重防御(per-turn system prompt + tool list 过滤 + runtime intercept),read-only
- `Yolo`: 跳过 Tier 3 ask modal,只保留 Tier 2 hard kill list
- `Background`: enum 保留,UI 不暴露

**模式切换路径(user 主动)**:前端 `ModeSelect.vue` / `ChatInput` 的 `Shift+Tab` 循环 → `useChatStore.requestSetMode` → `set_session_mode` IPC → `commands/permissions.rs::set_session_mode`(DB UPDATE + 审计 + Yolo root guard)。该路径已稳定(PR2,2026-06-13)。

### 痛点(LLM 想切 mode 必须拒绝)

- LLM 在 plan mode 探索到方案后,**当前无工具自请切到 edit mode 落代码**。只能返回 "please switch me to Edit mode",由用户手动按 `Shift+Tab` 切。
- 反向也有:LLM 在 edit 模式做大批量改动,想切到 plan 提议架构 → 同样需用户手动。
- Yolo 模式(高风险)用户一般不会默认开启,但 LLM 在跑 destructive task 时可申请(Yolo 守门在 IPC 层,tool 路径沿用即可)。

### 与 ask_user_question 的对比

| 维度 | ask_user_question | request_mode_change |
|---|---|---|
| 性质 | 信息询问(gather input) | **写操作** (改 session.mode) |
| Tier 4 ask | 不需要(Low risk) | **不需要**(沿用 set_session_mode IPC 的 Yolo guard)|
| 决策 1 / 决策 2 | 2-4 选项 | **二选一**: allow / deny |
| Schema 复杂度 | 1-4 questions × 2-4 options | 单字段: target_mode + reason |
| UI 形态 | inline card(同档) | inline card(同档) |
| Worker 禁用 | 是 | 是 |
| audit | `tool_executed`(现有) | **新增** `mode_change_requested` / `mode_change_allowed` / `mode_change_denied` |
| 持久化 | tool_result 进 messages | tool_result 进 messages **+ DB session.mode UPDATE + mode_changed audit** |

参考 [archive/2026-06/06-30-ask-user-question-tool/prd.md](../archive/2026-06/06-30-ask-user-question-tool/prd.md) 全套架构(QuestionStore / chat_loop 拦截 / inline card / session 保留),本工具采用**完全相同的 dispatch 范式** + **唯一的 apply 副作用**。

## 已确认事实(代码证据)

| 事实 | 位置 |
|---|---|
| `builtin_tools()` 在 `tools/mod.rs` 注册,19 个 tool 包含 `ask_user_question` / `update_checklist` 等 | `app/src-tauri/src/tools/mod.rs` |
| `ask_user_question` **故意**不在 `execute_tool` dispatch 表,`chat_loop.rs` 拦截 name → `execute_blocking` | `tools/ask_user_question.rs:31-35` 文档注释 + `chat_loop.rs:3110-3113` |
| `QuestionStore` 是 `Arc<Mutex<HashMap<session_id, PendingQuestion>>>`,与 PermissionStore 同形态;前端靠 `get_pending_question` IPC 跨 session 恢复 | `agent/question_store.rs:179-205` |
| `set_session_mode` IPC 已实现 DB UPDATE + mode_changed 审计 + Yolo root guard | `commands/permissions.rs:80-180` |
| `chat_loop.rs::run_chat_loop` 在 turn 0 读 `loaded_session.session.mode`(line 600),后续 LLM 调用**沿用该值** —— mode 切换是**下一 turn 起效** | `agent/chat_loop.rs:600` |
| `requestSetMode` 已有 Yolo 二次确认 modal(`pendingYoloConfirm` flag + `confirmYolo` action) | `stores/chat.ts` (见 `chatMode.test.ts` AC §1-§6) |
| 前端 `ModeSelect.vue` 暴露 3 档(edit/plan/yolo),通过 `useChatStore.requestSetMode` 路由 | `components/chat/ModeSelect.vue:176` |
| `ToolDef` 在 `builtin_tools()` 中是单 `ToolDef { name, description, input_schema }`,可枚举 + LLM 自描述 | `llm/types.rs` + `tools/ask_user_question.rs:108-165` |
| `record_audit_event(pool, session_id, kind, payload)` 是 17 类 audit 统一入口 | `db/audit.rs` |
| `is_parallel_eligible` 是**白名单**(`read_file / grep / glob / list_dir / use_skill`),`ask_user_question` / `dispatch_subagent` 都不在 → 整批走串行,`request_mode_change` 同档(默认不在白名单) | `agent/chat_loop.rs::is_parallel_eligible` |
| `STRUCTURALLY_DISABLED` 是 worker tool list 的黑名单(`update_checklist` 在内),新 tool 加进黑名单 = worker 不可见 | `agent/subagent/filter.rs` |
| `QuestionStore` 与 PermissionStore **互不感知** —— 同 session 已有 pending question 时,**第二个 ask_user_question** 报 `AlreadyPending` 错误;request_mode_change 应**共用同一 store 互斥语义**(只允许 1 个待决交互,避免 UI 堆叠) | `agent/question_store.rs:208-214` |
| `chat_loop` 的 turn 0 loaded_session 在每次 LLM 调用前 `filter_tools_for_mode(tools, mode)` 重新应用 —— mode 切换**下一 turn 自动同步 tool list** | `agent/chat_loop.rs` 接近 LLM 调用的位置 + `permissions/mode.rs:52-73` |

## 已敲定决策(brainstorm 结论)

| 维度 | 取值 | 理由 |
|---|---|---|
| **工具名** | `request_mode_change` | 与 `ask_user_question` 同 snake_case 风格;语义清晰(申请,非直接改)|
| **入参** | `target_mode: string` (enum: `edit` / `plan` / `yolo`) + `reason: string` (可选, ≤500 字符) | 跟 Claude Code / 现有 Mode 枚举一致;reason 是卡片副标题,LLM 解释为什么申请 |
| **目标 mode 集** | 3 档 user-facing:**edit / plan / yolo**(`background` enum 保留不暴露) | 跟 ModeSelect 一致;LLM 不能申请 `background`(v1)|
| **卡片形态** | **inline message card**(非 modal) | 与 ask_user_question 一致;切 session 不取消(ModeStore 跟 QuestionStore 同 lifecycle)|
| **决策按钮** | **允许** + **拒绝**(二选一) | 写操作必须显式确认;三选项无意义 |
| **"允许" 路径** | tool 内部直接调 `db::update_session_mode` + `db::record_audit_event("mode_changed")`,**复用 set_session_mode 的核心逻辑**;不动 IPC 回前端,避免双路径漂移 | tool 直接落库更可靠;Yolo root guard 在 tool 入口复检 |
| **"允许 Yolo" UX** | 走前端 `useChatStore.requestSetMode` 的 `pendingYoloConfirm` 二次确认 modal —— card "允许" 按钮点击时 dispatch 到 store,触发 Yolo 二次 modal;二次确认后才落库 | Yolo 高风险,user 主动 / LLM 申请共用同一道守门;不重复实现 |
| **"拒绝" 路径** | `is_error: true` + `{"cancelled_by_user": true}`(JSON) | 与 ask_user_question 的 cancelled 语义对齐,LLM 自然串行重试 |
| **当前 mode = target_mode** | tool_result 立即返回 `{"noop": true, "current_mode": "..."}`(is_error: false),不弹 card | 减少 round-trip;LLM 看到 noop 自行决定 |
| **Pending 互斥** | 与 `ask_user_question` 互斥(同 session 只允许 1 个待决交互),**共用同一 store + 同一 register key** | QuestionStore 的 `AlreadyPending` 错误自然覆盖;避免 UI 堆叠 |
| **Worker 禁用** | `STRUCTURALLY_DISABLED` 加 `request_mode_change` | worker 不可见;worker 想切 mode 必须回到 parent |
| **并行 eligibility** | 默认走串行(与 ask_user_question 同档) | 写操作 + user 阻塞语义,不适合并行 |
| **审计** | 新增 3 类:`mode_change_requested`(LLM 调 tool 触发) / `mode_change_allowed`(user 允许 + DB UPDATE 成功) / `mode_change_denied`(user 拒绝 / Yolo guard 拒) | 与现有 17 类 AuditKind 平行;`mode_changed` 由 DB UPDATE 路径自动产生,不重复 |
| **持久化** | tool_result 进 messages(同 ask_user_question);`db::update_session_mode` UPDATE sessions.mode | 复用,无新表 |
| **缓存命中** | tool_result 跟在 user prompt 之后,不影响 `cache_control: Ephemeral` 窗口 | 与 ask_user_question 一致 |
| **schema enum 形态** | `target_mode` 用 JSON schema `enum: ["edit", "plan", "yolo"]`(硬编码 3 档) | Mode 集极稳定(几乎不会变),动态 enum 收益小;LLM 直接读 schema |
| **拒绝原因的可见性** | audit payload 记 `reason`(LLM 申请原因);tool_result **不**回灌 reason 给 LLM(避免 prompt 膨胀) | audit 是审计,tool_result 是 LLM 决策输入,职责分离 |
| **IPC 统一化** | `get_pending_question` 升级为 `get_pending_interaction`(`Option<PendingInteraction>`,`kind: "question" \| "mode_change"`),新 IPC 替代 2 个并存 | 1 个 SOT 替代 2 个分立接口;前端 `streamController` 简化 |

边界推论(由决策导出,非新决策):
- LLM 申请切到当前 mode → 立即 noop,无 card 弹出(降低噪音)
- LLM 申请切到 `background` → schema 直接拒绝(enum 不含 background)
- Plan 模式 + 申请 Yolo → 仍弹 card(不特殊处理,user 拒绝即可)

## Requirements

### Tool 定义(R1–R4)

- **R1**. 新 builtin tool `request_mode_change`,`builtin_tools()` 列表里加入。**执行不经 `execute_tool`**:由 `chat_loop` 在 tool 处理阶段特判 name == `request_mode_change` → 直接调 `request_mode_change::execute_blocking(...)`(`execute_tool` 不加 match 分支,沿用 ask_user_question 模式)。
- **R2**. Tool 输入 schema(snake_case):
  ```rust
  struct RequestModeChangeInput {
      target_mode: String,        // 必填, enum: "edit" | "plan" | "yolo"
      reason: Option<String>,     // 可选, ≤500 字符, 显示在 card 副标题
  }
  ```
  schema 校验在 tool execute 入口(枚举越界 / 超长 / 空字符串 → `is_error: true` + 结构化错误)。
- **R3**. Agent loop 阻塞分支(对齐 ask_user_question):`chat_loop` 拦截 `request_mode_change` → 调 `execute_blocking`(**先 schema 校验,失败 short-circuit `is_error: true`,不挂起**)→ 校验通过则检查 `target_mode == current_mode`(命中 → 立即 noop 返回)→ 否则发 `mode:change:request` IPC 事件(携带 session_id + tool_use_id + target_mode + reason)→ `tokio::select!{cancel, oneshot.recv()}` 等待 → 拿到 user 决定(allow / deny)后回灌 tool_result。**session cancel 仍能中断**(同 ask_user_question)。**Turn 计数器递增 1 次**(v1 接受,见 ask_user_question R3 决策)。
- **R4**. Mode 应用副作用(只在 user "允许" 路径触发,tool 内部直接落库不走 IPC):
  - 调 `db::update_session_mode(pool, session_id, target_mode)`(同 `set_session_mode` 内部)
  - 调 `db::record_audit_event(pool, session_id, "mode_changed", payload)`(自动产生 `prev_mode` → `new_mode` 记录)
  - Yolo safety guard 沿用 `set_session_mode` 的 `is_running_as_root` 检查(若 root 拒绝 → 走"拒绝"路径,tool_result = `{"cancelled_by_user": true, "reason": "Cannot enable Yolo as root"}`)
  - "允许 Yolo" 走前端 `pendingYoloConfirm` 二次 modal 路径(见 R7)

### UI 形态(R5–R7)

- **R5**. **Inline message card,非 modal**。跟 `AskUserQuestionCard.vue` 一样:消息流 child 元素,无 portal / 无遮罩,跟 `ToolCallCard` 同层(由 `MessageItem.vue` 路由分发)。
- **R6**. Card 内容(自上而下):
  - Header chip:目标 mode 名字(如 "切换到 Yolo")+ 状态色(plan = 蓝, edit = 灰, yolo = 红)
  - Reason 文本(LLM 给的 `reason`,≤500 字符,可选)
  - Bottom action row(pending):**允许** + **拒绝** 按钮(两按钮等宽并列,允许在左;允许按钮颜色按 target_mode 映射——plan 蓝 / edit 灰 / yolo 红)
  - 答后状态(answered):"已切换到 [mode]" status pill + 切换前 → 后的 mode 对比
  - 拒绝状态(cancelled):"已拒绝" status pill
- **R7**. **"允许 Yolo" 二次确认 modal**:与 `useChatStore.requestSetMode` 走 user 主动切 Yolo 路径**完全一致** —— 点"允许" → 弹 `pendingYoloConfirm` modal(显示 "切换到 Yolo 将跳过所有用户确认,仅硬 kill list 仍生效"),用户二次确认后才落库。这避免 LLM 申请 Yolo 的风险高于 user 主动申请。

### Session 挂起保留(R8)

- **R8**. **`PendingInteraction` 是 `enum { Question(ToolQuestionPayload) | ModeChange(ModeChangePayload) }`,复用 `QuestionStore` 的 oneshot 形态**:`Arc<Mutex<HashMap<session_id, PendingInteraction>>>`,key = session_id, value = oneshot + 完整 payload。**与 ask_user_question 共用同一 store,只允许 1 个 pending**(互斥)。`register` 接口扩展为 `register(session_id, tool_use_id, kind: InteractionKind, payload)`;`resolve` 保留原签名(oneshot 不区分 kind)。**新 IPC `get_pending_interaction(session_id)` 替代原 `get_pending_question`**:返回 `Option<PendingInteraction>`,前端 streamController 统一查询。

### 并发(R9)

- **R9**. 同 session 单 pending 互斥(同 store 互斥,沿用 QuestionStore 的 `AlreadyPending` 错误)。第二个并发调用 → `{"error": "已有 pending interaction, 等当前处理完成"}`(`is_error: true`)。

### Worker(R10)

- **R10**. **Worker subagent 不适用**(`STRUCTURALLY_DISABLED` 加入 `filter_tools_for_subagent` 列表)。Worker 想切 mode 必须回到 parent。

### 模式交互(R11–R12)

- **R11**. Plan / Edit / Yolo 三档默认 tool 始终可用,无 Tier 4 ask,跟 `ask_user_question` / `update_checklist` 同档(`Risk::Low`)。tool 自身就是 mode 切换的载体,不需再经 Tier 4。
- **R12**. Plan 模式 + 申请 Yolo / 反之亦然:行为与 Plan → Edit 一致,无特殊规则。**最高决策权在 user**,LLM 不能 self-apply。

### 持久化(R13–R15)

- **R13**. assistant turn(含 `ToolUse(request_mode_change)` 块)→ `persist_turn` 写 `messages` 表。
- **R14**. tool_result 块(用户决策 + 模式应用结果)→ 同样进 `messages` → reload 还原完整 R&A 流。
- **R15**. Audit:每次 tool 调用产生 audit row(走 `record_tool_executed_audit`),**新增 3 类专用 kind**:
  - `mode_change_requested`(tool 入口,记录 LLM 申请;payload 含 target_mode + reason)
  - `mode_change_allowed`(user 允许 + DB UPDATE 成功;payload 含 prev_mode + new_mode)
  - `mode_change_denied`(user 拒绝;payload 含 target_mode)
  - 此外,**DB 自身的 `mode_changed` audit 由 `db::update_session_mode` 自动产生**(沿用 set_session_mode 路径,audit 不重复)

### 取消 / 终止(R16)

- **R16**. session cancel 路径(`token.cancelled()`)中断挂起,store remove。**无 timeout** —— 与 ask_user_question 决策一致(v1 简化)。App crash = pending 全失(oneshot in-memory,可接受)。

### 并行执行(R17)

- **R17**. **`request_mode_change` 不在 `is_parallel_eligible` 白名单** → 整批自动走串行(同 ask_user_question / dispatch_subagent 机制)。批次内按 LLM 声明顺序串行,request_mode_change 在其位置阻塞,其他 tool 暂停等待。

### 前端组件分发(R18)

- **R18**. `MessageItem.vue` / `MessageList.vue` 增加 tool name → component 分发:`request_mode_change` 路由到新组件 `<RequestModeChangeCard>`,其他 tool 仍走 `<ToolCallCard>`。

## Acceptance Criteria

- **AC1**. Mock provider 集成测试:LLM 一次响应包含 `request_mode_change` + 普通短 tool(如 `shell`)→ **整批走 Serial** → 按 LLM 声明顺序执行,request_mode_change 在其位置阻塞,其他 tool 暂停 → user 允许后 mode 切换落库,所有 tool_result 进 messages → LLM 下一轮看到新 mode 在 system prompt 体现。
- **AC2**. Tool schema 单元测试:输入越界(`target_mode` 不在 enum / `reason` >500 字符 / `target_mode` 空字符串)→ `is_error: true` + 结构化错误消息。
- **AC3**. `filter_tools_for_subagent` 单元测试:输出不含 `request_mode_change`(worker 不可见)。
- **AC3'**. `is_parallel_eligible` 单元测试:`request_mode_change` 返回 false;含它的批次被强制 Serial。
- **AC4**. 真实 IPC 集成测试:消息流里出现 `<RequestModeChangeCard>`,用户点"允许" → DB `sessions.mode` UPDATE + `mode_changed` audit + `mode_change_allowed` audit 产生 → 前端 `ModeSelect` chip 立即反映新 mode → 下次 LLM 调用的 system prompt 体现新 mode 行为约定。
- **AC5**. **Session 切换保留 pending**:session A 有 pending mode change → 切到 session B → session B 工作一会儿 → 切回 session A → 通过新 IPC `get_pending_interaction` 查询 → pending card 仍在,可选可答。Backend oneshot 全程不释放。
- **AC5'**. **`get_pending_interaction` Tauri command 行为测试**:session A 有 pending ask_user_question → command 返回 `Some({ kind: "question", payload })`;session A 有 pending mode change → 返回 `Some({ kind: "mode_change", payload })`;resolve 后返回 `None`;不存在 session 返回 `None`。
- **AC6**. 拒绝路径:用户在 card 上点"拒绝"按钮 → tool_result = `{"cancelled_by_user": true}`(`is_error: true`)→ LLM 看到能优雅应对(不强制 LLM 行为,只验证 wire shape)。
- **AC7**. **Noop 路径**:LLM 申请切到当前 mode → tool 立即返回 `{"noop": true, "current_mode": "<x>"}`(`is_error: false`)→ 不弹 card,不发 IPC 事件,直接完成。
- **AC7a**. Session reload(完整 process 重启)后**已 resolve 的** mode change 完整可见:`messages` 表里能查到 assistant `ToolUse(request_mode_change)` 块 + 下一轮 assistant turn 的 tool_result 块含 user 决策。
- **AC7b**. **pending 中的 mode change 在 process 重启后丢失**(oneshot in-memory)——reload 后该 turn 的 tool_result 缺失,v1 接受。
- **AC8**. **Yolo 二次确认**:
  - 申请 edit / plan → user 点"允许"立即落库,无二次 modal
  - 申请 yolo → user 点"允许"触发 `pendingYoloConfirm` modal,二次确认后落库;在 modal 上点"取消" → 走"拒绝"路径(tool_result = `{"cancelled_by_user": true}`)
- **AC8'**. **Yolo root guard**:`is_running_as_root() == true` 时,即使 user 在 card 上点"允许 Yolo",二次 confirm modal 上"确认"也走拒绝路径 —— 在二次 modal 渲染时检测 root 并直接 disable "确认" 按钮 + 显式红字 "Cannot enable Yolo as root";tool_result = `{"cancelled_by_user": true, "reason": "Cannot enable Yolo as root"}`。
- **AC9**. 并发路径:同 session 同 turn 第二次调用 `request_mode_change`(且 session 已有 pending ask_user_question)→ 返回 `QuestionStoreError::AlreadyPending` 等价错误,第一个 pending 仍正常完成。
- **AC10**. **无 modal 验证**(除 Yolo 二次确认):DOM 里 `<RequestModeChangeCard>` 是消息流的 child 元素,无浮层 / 无遮罩 / 无 portal 到 body(除 Yolo 二次确认 modal,沿用现有 `useChatStore.pendingYoloConfirm` 实现)。
- **AC11**. **MessageItem 路由验证**:tool_use 块里的 `request_mode_change` 渲染成 `<RequestModeChangeCard>`,其他 tool name 仍渲染成 `<ToolCallCard>`。
- **AC12**. **审计 kind 完整**:调用一次 `request_mode_change` 至少产生 1 条 `mode_change_requested` audit;user 允许产生 1 条 `mode_change_allowed` + 1 条 `mode_changed`(DB 自动);user 拒绝产生 1 条 `mode_change_denied`。
- **AC13**. **回归**:`ask_user_question` 既有测试全绿;`set_session_mode` 既有测试全绿;`tests_subagent.rs` / `tests_agent_loop.rs` / `tests_chat.rs` / `tests_ask_user_question.rs` 零回归。
- **AC14**. **全绿**:`cargo test --lib`(带 `PKG_CONFIG_PATH`)+ `vue-tsc --noEmit` + `vitest run` 全绿。

## Out of Scope(v1)

- Timeout 机制 + auto-decide(同 ask_user_question)
- 多 mode 排队 / 多次申请合并
- 自由文本 reason 编辑
- 跨 session mode 同步
- Worker subagent mode 申请(永久禁用)
- LLM 申请切到 `background` 模式(enum 不暴露)
- **切回 user 上一次手动切的 mode**(LLM 申请 edit 时,如果 user 切到过 plan 再切到 yolo,LLM 申请 edit 是切到 edit,不是切回 yolo —— 简单优先)
- App crash 恢复 pending mode change(进程级 in-memory)
- **`for turn in 1..=turn_limit` 重构 while 循环零消耗**(v1 接受 turn +1)

## Notes

- 取消语义只有 1 种(`{"cancelled_by_user": true}`),由用户显式触发;noop 是另一类(`{"noop": true, "current_mode": "..."}`,is_error: false)。
- **无 timeout 兜底**:v1 唯一清理路径是用户允许 / 拒绝 / session cancel(Stop)。
- session 切换不取消 pending,跟 ask_user_question 一致(ModeStore 与 QuestionStore 复用同一挂起语义)。
- 复用 `set_session_mode` 的 Yolo root guard + 持久化逻辑,**避免 tool 内重复实现** —— tool 入口只做"申请 → 等决策 → 复用核心副作用"三步,user 主动 / LLM 申请两条路径行为完全一致(都用 `db::update_session_mode` 落库)。
- Yolo 二次确认 modal **完全沿用** `useChatStore.pendingYoloConfirm`,不新写 modal 组件;只在 card "允许" 按钮点击事件里 dispatch 到 store action(store action 内部判断 `pendingYoloConfirm` flag 走 Yolo 路径 or 直接 IPC)。
- "拒绝" 不调 `db::update_session_mode`(不写 `mode_changed` audit),只走 `mode_change_denied` 专用 audit,职责清晰。
- 核心 spec: `.trellis/spec/backend/tool-contract.md`(tool schema 规范)+ `permission-layer.md`(mode 系统 + Tier 3 ask 形态)+ `frontend/chat.md`(inline card 红线)。
- 技术设计见 `design.md`,执行清单见 `implement.md`。
