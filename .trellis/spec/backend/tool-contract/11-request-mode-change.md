## Scenario: request_mode_change tool (B6+ A, 2026-07-07)

**Context**: 现有 `ask_user_question` 是"信息询问"类工具(用户回答选项,
数据回填 tool_result)。`request_mode_change` 走**写操作**通道:用户
二选一 allow/deny,允许路径修改 `sessions.mode` + 走 `set_session_mode`
的 Yolo 二次 modal / root guard / audit 全套副作用。**不**复用
`ask_user_question` 的 2-4 选项 schema(语义不同,audit kind 不同,
持久化层不同)。完整 PRD 在
`.trellis/tasks/07-07-07-07-request-mode-change-tool/`,design 5 时序
见同目录 `design.md` §2。

### 1. Scope / Trigger

- Trigger: 主 agent 在 turn N 通过 LLM `tool_use` 申请把当前 session
  的 `mode` 切到 `edit` / `plan` / `yolo` 之一(LLM 不能申请 `background`,
  schema enum 硬编码 3 档),用户通过 inline message card 看到目标 mode +
  原因,选择"允许"或"拒绝"。**允许** → 复用 `set_session_mode` IPC 的
  副作用链(DB 持久化 + 审计 + Yolo 安全守门 + 模式 prefix 切换,沿
  现有路径走避免双路径漂移);**拒绝** → `is_error: true` + 取消标记
  回灌,LLM 自决。下一 turn 起 mode 切换在 system prompt 体现。
- 只服务**主 session**。Worker subagent 永久禁用(`STRUCTURALLY_DISABLED`
  加 `request_mode_change`),worker 想切 mode 必须回 parent。
- 对标 Claude Code `request_mode_change` tool 形态(同工具名,语义对齐:
  LLM 申请 / 用户授权 / 副作用由 IPC 链统一落库)。

### 2. Signatures

#### Tool declaration(`app/src-tauri/src/tools/request_mode_change.rs`)

```rust
ToolDef {
    name: "request_mode_change",
    description: "Ask the user to switch this session's permission mode \
                  (edit / plan / yolo). The user sees an inline card with the \
                  target mode and the reason you provide; they choose Allow \
                  or Deny. On Allow, the mode is updated and the system prompt \
                  reflects the new mode on the next turn. On Deny, you get \
                  {\"cancelled_by_user\": true} and should adapt. If the target \
                  mode is the current mode, the call returns {\"noop\": true} \
                  and no card is shown.\n\n\
                  Use this when you've completed a read-only plan and need write \
                  access (request edit), or when you want to propose a change \
                  before acting on it (request plan), or when the user has \
                  explicitly approved running unattended (request yolo). Do not \
                  request yolo unless the task is fully understood and \
                  irreversible actions are acceptable.",
    input_schema: json!({
      "type": "object",
      "properties": {
        "target_mode": {
          "type": "string",
          "enum": ["edit", "plan", "yolo"],
          "description": "The mode to request. Must be one of: edit, plan, yolo."
        },
        "reason": {
          "type": "string",
          "maxLength": 500,
          "description": "Optional ≤500-char explanation shown on the card. \
                          Be specific about why this mode is needed for the next step."
        }
      },
      "required": ["target_mode"]
    })
}
```

#### 后端 tool entry 签名

```rust
// app/src-tauri/src/tools/request_mode_change.rs
pub type BlockingToolResult = (
    String,                            // content
    bool,                              // is_error
    crate::tools::ToolContextUpdate,
    Option<i32>,                       // exit_code
);

pub async fn execute_blocking(
    input: &serde_json::Value,
    session_id: &str,
    tool_use_id: &str,
    current_mode: db::Mode,            // ← execute_blocking 需要当前 mode 做 noop 判断
    store: &QuestionStore,             // ← 共用 ask_user_question 的 store(扩展为 PendingInteraction)
    sink: &Arc<dyn ChatEventSink>,
    cancel: &CancellationToken,
) -> BlockingToolResult;
```

注意:`execute_blocking` **不**接收 `pool: &SqlitePool` —— 落库走
`resolve_mode_change` IPC → `set_session_mode` 内部函数(`design §5.3`
决策变更,tool 内不直接调 `db::update_session_mode`,避免双路径漂移)。
`current_mode` 由 `chat_loop` turn 0 快照提供(`chat_loop.rs:600`)。

#### IPC surface(`app/src-tauri/src/commands/permissions.rs` + `commands/question.rs`)

```rust
// 新增 — 主路径(Edit / Plan 允许路径走这里)
#[tauri::command(rename_all = "camelCase")]
pub async fn resolve_mode_change(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    tool_use_id: String,
    allow: bool,                       // true = Allow(走 set_session_mode 内部函数),false = Deny
) -> Result<SessionRow, AppCommandError>;
// 返回 SessionRow(对齐 set_session_mode 既有 IPC 形态),前端 invoke 后
// 直接 setCurrentSession(row) 刷 mode chip;Yolo 二段路径也会 setSessionMode,
// 同契约。

// 升级 — 原 get_pending_question 升级为 get_pending_interaction
#[tauri::command]
pub async fn get_pending_interaction(
    state: State<'_, Arc<AppState>>,
    session_id: String,
) -> Result<Option<PendingInteractionEntry>, AppCommandError>;
// 返回 { kind: "question" | "mode_change", payload: ... };前端 streamController
// 统一查询,旧 get_pending_question 软弃用(保留兼容,下版本删)。

// 保留 — 旧 IPC 软弃用(向后兼容)
#[tauri::command] pub async fn resolve_tool_question(...);  // 不变
```

### 3. Contracts

#### Wire shape(snake_case payload,共享 struct 豁免)

`ModeChangePayload`(`agent/question_store.rs` 新增)走 `#[serde(rename_all = "snake_case")]` 序列化
—— 跟 `ToolQuestionPayload` 同款,作为 `PendingInteraction` 的
内部 variant 与 `register` 互斥一起扩展;**不**像顶层 Tauri arg
那样 auto-camel,前端 `getPendingInteraction` IPC 解析走 snake_case:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]  // wire: target_mode/current_mode/tool_use_id/session_id/reason/ts
pub struct ModeChangePayload {
    pub session_id: String,
    pub tool_use_id: String,
    pub target_mode: String,           // "edit" | "plan" | "yolo"
    pub current_mode: String,          // session 当前 mode(LLM 自检用)
    pub reason: Option<String>,        // LLM 给的 reason
    pub ts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]  // wire: "kind": "question" | "mode_change"
pub enum PendingInteraction {
    Question(ToolQuestionPayload),
    ModeChange(ModeChangePayload),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingInteractionEntry {
    pub kind: InteractionKind,         // "question" | "mode_change"
    pub payload: PendingInteraction,   // tagged enum
}
```

#### `QuestionStore` 升级 — `PendingInteraction` 互斥

`QuestionStore`(`agent/question_store.rs`)从
`Arc<Mutex<HashMap<session_id, PendingQuestion>>>` 升级为
`Arc<Mutex<HashMap<session_id, PendingInteraction>>>`(命名保留以降低
改动面,内部 value 类型扩展为 tagged enum):
- `register` 签名扩展:`(session_id, tool_use_id, payload: PendingInteraction) -> Result<oneshot::Receiver<InteractionResponse>, QuestionStoreError>`。
- `resolve` 保留原签名(oneshot 不区分 kind — `sender.send(InteractionResponse::Allow { new_mode } | Cancelled)`)。
- oneshot 通道类型 `QuestionResponse` → `InteractionResponse`(enum:`Answered { new_mode: SessionMode } | Cancelled`,新 generic value 类型替代原 `Vec<QuestionAnswer>`)。
- `get_payload` 改返回 `Option<PendingInteractionEntry>`。
- **互斥语义保持**:`map.contains_key(session_id) -> AlreadyPending` —— 同 session 不能并发 2 个待决交互,无论 kind(question / mode_change)。

#### Session 切换保留

- session A 挂 pending mode change → 切到 session B → B 工作一会儿 → 切回 A
  → 通过新 IPC `get_pending_interaction(session_id=A)` 查询 → 命中 pending
  entry,可选可答。Backend oneshot **全程不释放**(`QuestionStore` 跟
  `PermissionStore` 同 lifecycle,跟 ask_user_question 一致)。
- session cancel(`token.cancelled()`)中断挂起,store remove entry;
  tool_result = `{"cancelled_by_session": true}`(`is_error: true`),
  整轮终止。

#### Worker 禁用

`STRUCTURALLY_DISABLED` 黑名单加 `request_mode_change`(`agent/subagent/filter.rs`)。
worker 不可见;worker 想切 mode 必须回到 parent(由 LLM 自行决策:
"parent mode is X, work without Y tools"或"ask parent to dispatch a new
worker in a different mode")。**永久禁用**,不进白名单(同
`update_checklist` / `dispatch_subagent`)。

#### 并行 eligibility

`is_parallel_eligible` 白名单**不**加 `request_mode_change` —— 走默认
`false`,整批被强制 Serial(同 `ask_user_question` / `dispatch_subagent`
机制)。批次内按 LLM 声明顺序串行,request_mode_change 在其位置阻塞,
其他 tool 暂停等待。**Turn 计数器 +1**(同 ask_user_question 决策)。

#### 审计 contract(共 3 类,新增)

```rust
// agent/permissions/audit.rs::AuditKind — 17 → 20
pub enum AuditKind {
    // ... 既有 17 类 ...
    ModeChangeRequested,   // tool 入口,记录 LLM 申请; payload { target_mode, reason, noop: bool }
    ModeChangeAllowed,     // user 允许 + DB UPDATE 成功; payload { prev_mode, new_mode, target_mode }
    ModeChangeDenied,      // user 拒绝; payload { target_mode, reason: "user denied" | "yolo_root_guard" | "yolo_cancelled_confirm" }
}
```

`mode_changed` audit 由 `db::update_session_mode` 自动产生(沿用
`set_session_mode` 路径),**不重复**。`as_str()` 返
`"mode_change_requested"` / `"mode_change_allowed"` /
`"mode_change_denied"`,匹配穷尽,无 `_` 通配(编译器对未来 variant
强制)。

#### Yolo 二段守门(关键 UX 守门)

- 申请 `edit` / `plan` → user 点"允许"立即调 `resolve_mode_change(allow=true)` →
  IPC handler 内部调 `set_session_mode` 内部函数 → DB UPDATE + 落 audit
  + 返回 `SessionRow`。**无二次 modal**。
- 申请 `yolo` → user 点"允许"**不直接调 IPC**,而是 dispatch 到前端
  `useChatStore.requestSetMode(sid, "yolo")`,触发既有的
  `pendingYoloConfirm` modal(显示 "切换到 Yolo 将跳过所有用户确认")。
  用户在 modal 上点"确认" → `confirmYolo` action 调 `set_session_mode` IPC
  → IPC handler 检测"这是从 card dispatch 来" → 落库 + audit + **额外**调
  `store.resolve(sid, Allow)` 让 agent loop oneshot 解除。用户在 modal
  上点"取消" → 走"拒绝"路径(`mode_change_denied` audit + tool_result
  `{"cancelled_by_user": true, "reason": "user cancelled Yolo confirm"}`,
  `is_error: true`)。
- **root guard**:`is_running_as_root() == true` 时,即使 user 在 card 上
  点"允许 Yolo",二次 confirm modal 上"确认"按钮被 `disabled` +
  红字 "Cannot enable Yolo as root",点击无效 → 走"拒绝"路径,
  `tool_result = {"cancelled_by_user": true, "reason": "Cannot enable Yolo as root"}`。

### 4. Validation & Error Matrix

| 条件 | 行为 |
|---|---|
| `target_mode` 不在 enum | schema 校验拦在前面 → LLM 自决(无效 tool_use) |
| `target_mode` 是空字符串 | execute_blocking 入口 `is_error: true` + `{"error": "target_mode is empty"}` |
| `reason` > 500 字符 | execute_blocking 入口 `is_error: true` + `{"error": "reason exceeds 500 chars"}` |
| `target_mode == current_mode` (noop 路径) | 立即 `{"noop": true, "current_mode": "<x>"}`,不挂 store,不发 IPC,留痕 audit `mode_change_requested { noop: true }` |
| `map.contains_key(session_id)` 已有 pending | `QuestionStoreError::AlreadyPending` → `{"error": "已有 pending interaction, 等当前处理完成"}`,`is_error: true` |
| `is_running_as_root() && target_mode == yolo` | 走"拒绝"路径,`tool_result = {"cancelled_by_user": true, "reason": "Cannot enable Yolo as root"}`(`resolve_mode_change` IPC handler 检测) |
| `token.cancelled()` 触发 | store remove entry,`tool_result = {"cancelled_by_session": true}`,`is_error: true`,chat_loop 推 `Done{stop_reason="cancelled"}` |

### 5. Good / Base / Bad Cases

**Good (AC1 + AC4)**:
1. Plan mode LLM 申请 `edit` + `reason="需要落代码"` → card 渲染 →
   user 点"允许" → `resolve_mode_change(allow=true)` → DB
   `sessions.mode` UPDATE edit + `mode_changed` audit + `mode_change_allowed`
   audit 产生 → 前端 `ModeSelect` chip 切到 Edit → 下次 LLM 调用
   system prompt 体现 Edit 行为约定。
2. Edit mode LLM 申请 `plan` + `reason="想做架构调整"` → card 渲染 →
   user 点"允许" → DB UPDATE plan + audit + chip 切 Plan,`filter_tools_for_mode`
   下轮自动 drop 写类工具。

**Base (AC6 + AC7)**:
3. LLM 申请 `edit`,user 点"拒绝" → `mode_change_denied` audit +
   `tool_result = {"cancelled_by_user": true}`,`is_error: true`,LLM
   看到自决(可走 `ask_user_question` 询问 user 真实意图 / 改
   plan 重提)。
4. LLM 申请切到当前 mode(noop) → `mode_change_requested { noop: true }`
   audit 留痕 → 立即 `{"noop": true, "current_mode": "..."}`,`is_error: false`,
   不弹 card,LLM 下一轮看到 noop 自决。

**Bad (AC8' + AC9)**:
5. LLM 申请 `yolo` + 以 root 跑(WSL 特殊)→ user 点"允许" → 二次 modal
   "确认"按钮 disabled + 红字 → 等价拒绝路径 → `mode_change_denied
   { reason: "yolo_root_guard" }` audit + `tool_result = {"cancelled_by_user": true, "reason": "Cannot enable Yolo as root"}`。
6. 同 session 已有 pending `ask_user_question` → LLM 再调
   `request_mode_change` → `QuestionStoreError::AlreadyPending` →
   `{"error": "已有 pending interaction, 等当前处理完成"}`,
   `is_error: true`,第一个 pending 仍正常完成。

### 6. Tests Required

**Unit**(`tools/request_mode_change.rs::tests`,~11 个):

| Test | 断言 |
|---|---|
| `definition_has_correct_name` | `ToolDef.name == "request_mode_change"` |
| `definition_schema_requires_target_mode` | `input.required` 含 `target_mode` |
| `definition_schema_target_mode_enum_three` | enum == `["edit", "plan", "yolo"]` |
| `definition_schema_reason_max_length_500` | `reason.maxLength == 500` |
| `validate_accepts_well_formed_input` | 纯函数(Edit + 短 reason)→ Ok |
| `validate_rejects_empty_target_mode` | 空字符串 → Err |
| `validate_rejects_out_of_enum_target_mode` | "background" / "x" → Err |
| `validate_rejects_reason_over_500_chars` | 501 chars → Err |
| `noop_target_equals_current_returns_noop_marker` | target == current → `{"noop": true, ...}` |
| `happy_path_registers_emits_and_returns_allowed` | Allow → `{"allowed": true, ...}` |
| `deny_path_returns_cancelled_user_marker` | Deny → `{"cancelled_by_user": true, ...}` |
| `cancel_arm_returns_session_cancelled_marker` | cancel token → `{"cancelled_by_session": true, ...}` |
| `already_pending_returns_structured_error` | 已有 pending → AlreadyPending 错 |
| `audit_kind_records_requested_allowed_denied` | CapturingSink 抓 3 类 audit |

**Integration**(`agent/tests_request_mode_change.rs`,5 个 + 1 个额外):

| Test | 断言 |
|---|---|
| `chat_loop_allow_path_persists_mode_and_returns_audit` | mock provider 模拟 LLM 一次响应含 `request_mode_change` + `shell` → 整批走 Serial → allow 后 DB mode 已变 + `mode_change_requested/allowed/changed` 3 audit 齐 |
| `chat_loop_deny_path_emits_cancelled_user_tool_result` | Deny 路径 → `is_error: true` tool_result + `mode_change_denied` audit |
| `chat_loop_noop_target_equals_current_short_circuits` | target == current → no `mode:change:request` IPC emit + 立即 tool_result |
| `chat_loop_cancel_interrupts_pending` | session cancel token → store remove + cancelled tool_result |
| `chat_loop_already_pending_returns_structured_error` | LLM 第二次 request_mode_change 时 store 已被占 → 第二次 `is_error: true` + 第一次仍正常完成 |
| `chat_loop_block_serializes_with_other_tools` | 批次内含 `request_mode_change` + `shell` → 整批强制 Serial(并发组确认 false) |

**IPC handler**(`commands/tests_resolve_mode_change.rs`,5 个 + `tests_get_pending_interaction.rs`,4 个):

| Test | 断言 |
|---|---|
| `resolve_mode_change_allow_calls_set_session_mode_inner` | 内部函数被调 + audit 链齐 |
| `resolve_mode_change_allow_returns_session_row` | IPC 返 SessionRow,前端可直接 setCurrentSession |
| `resolve_mode_change_deny_writes_audit_does_not_touch_db` | `mode_change_denied` audit,`sessions.mode` 不变 |
| `resolve_mode_change_yolo_as_root_writes_denied_audit` | `is_running_as_root` 真 → `mode_change_denied { reason: "yolo_root_guard" }` |
| `resolve_mode_change_double_invoke_is_noop` | 第二次 invoke(rid 已 resolve)→ 返回 Err + warn log |
| `get_pending_interaction_returns_none_for_unknown_session` | 不存在 session → `None` |
| `get_pending_interaction_returns_question_for_ask` | 已有 pending ask_user_question → `Some({ kind: "question", ... })` |
| `get_pending_interaction_returns_mode_change` | 已有 pending mode change → `Some({ kind: "mode_change", ... })` |
| `get_pending_interaction_returns_none_after_resolve` | resolve 后 → `None` |

**Worker**(`subagent/tests_filter.rs` +1):

| Test | 断言 |
|---|---|
| `filter_strips_request_mode_change_even_if_allowlist_lists_it` | 跟 `update_checklist` 同款防御性测 |

**Parallel**(`agent/tests_parallel.rs` +1):

| Test | 断言 |
|---|---|
| `is_parallel_eligible_returns_false_for_request_mode_change` | 白名单外 → false |

### 7. Wrong vs Correct —— 拦截路径(execute_tool_inner vs chat_loop loop)

#### Wrong:`request_mode_change` 走 `execute_tool_inner` / `execute_tool` dispatch

```rust
// tools/mod.rs::execute_tool_inner
match name {
    "request_mode_change" => request_mode_change::execute(input, ctx, ...).await,
    // ...
}
```

**Why it's wrong**:① `execute_tool_inner` 签名拿不到 `current_mode`(noop 判断需要)、
拿不到 `QuestionStore`(oneshot 互斥需要),即便加到 `ToolContext` 又
模糊工具层和 agent 层边界;② `execute_tool` 是 per-tool 串行执行,
不能在 `record_audit` + `store.register` + `emit_mode_change_request`
+ `tokio::select!{cancel, oneshot}` 这套长生命周期 oneshot 等待中
保持 round-trip;③ `execute_tool` 不区分"非阻塞 tool"与"阻塞等待
用户交互 tool",混淆了 use_ui / read_file 的"立即返回"语义和
ask_user_question / request_mode_change 的"等用户决策"语义。

#### Correct:agent loop 层拦截

```rust
// chat_loop.rs tool_use 处理循环(约 :3110 — 跟 ask_user_question 同位置
// 平行加分支,共用同套 dispatch 模式)
if tool_name == "request_mode_change" {
    // 不走 execute_tool;直接调 request_mode_change::execute_blocking
    let (content, is_error, _ctx_update, _exit_code) =
        request_mode_change::execute_blocking(
            tool_input,
            session_id,
            tool_use_id,
            current_mode,        // turn 0 快照
            question_store,      // 与 ask_user_question 共用
            sink,
            cancel,
        ).await;
    // 构造 ContentBlock::ToolResult 回填(配对 RULE-A-007)
    result_blocks.push(ContentBlock::ToolResult { tool_use_id, content, is_error });
    continue;
}
// 其他 tool 走原 execute_tool 路径
let (out, is_err, update, exit_code) = execute_tool(name, input, ...).await;
```

**为什么这三个 tool 走拦截,其他 tool 走 dispatch 表**:`ask_user_question` /
`request_mode_change` / `dispatch_subagent` 都是"控制流 tool" —— 需要
agent loop 层依赖(`question_store` oneshot / `provider` + `db` worker
嵌套 / 当前 mode 快照 + 落库 IPC 链)。`execute_tool_inner` 签名
`(name, input, ctx, guard, session_id, skill_cache, cancel)` 设计上是
**纯 I/O** 接口(`read_file` / `shell` / `grep` 之类),把控制流依赖
塞进去会越界(对齐 `dispatch_subagent` 的 rationale,见
tool-contract.md §"Scenario: dispatch_subagent tool" §7)。

### 8. Design Decisions

#### Decision:不新写 `ask_user_question` 二选项形态,独立 tool

**Context**:`ask_user_question` 现有 2-4 选项 schema,可塞"target_mode
+ allow/deny"为 2 选项;复用 0 改动最简。

**Decision**:独立 `request_mode_change` tool。

**Why**:① **语义不同** —— `ask_user_question` 是"信息询问"(LLM 收集
context 决策),`request_mode_change` 是"写操作"(改 `sessions.mode`),
职责不同;② **审计 kind 不同** —— `ask_user_question` 走 `tool_executed`
既有 audit,`request_mode_change` 要新增 3 类专用 audit 表达"申请 /
允许 / 拒绝"三态;③ **IPC 链不同** —— `ask_user_question` resolve 后
只回填 tool_result,`request_mode_change` resolve 后还要走
`set_session_mode` 内部函数落库(包含 Yolo 二次 modal / root guard);
④ **Schema 复杂度膨胀** —— 把"target_mode + reason + mode 颜色映射
+ 二次 modal"塞进 ask_user_question 的 options 会变成 ad-hoc 字段,
schema 失去通用性。**对齐 Claude Code / opencode 业界模式**:写操作
类申请都是独立 tool。

#### Decision:Yolo 二次 modal 走 `chatStore.pendingResolveRequest` + `confirmYolo`

**Context**:Yolo 模式风险高,user 主动切 Yolo 已有二次 modal 守门
(`pendingYoloConfirm` + `confirmYolo` / `cancelYolo` action)。
LLM 申请 Yolo 路径需要同等守门,避免"LLM 偷偷切 Yolo"风险高于
"user 主动切 Yolo"。

**Decision**:LLM 申请 Yolo 路径**完全沿用** user 主动切 Yolo 路径。
`<RequestModeChangeCard>` 的"允许"按钮点击事件 dispatch 到
`useChatStore.requestSetMode(sid, "yolo")`,触发既有 `pendingYoloConfirm`
modal(显示 "切换到 Yolo 将跳过所有用户确认");用户在 modal 上
"确认" → `confirmYolo` action 调 `set_session_mode` IPC → IPC handler
检测"这是从 card dispatch 来" → 落库 + audit + 额外调
`store.resolve(sid, Allow)` → agent loop oneshot 解除。
**不新写 modal 组件**,不新写 store action,跟 user 主动路径共用。
**双 IPC 调用**(`set_session_mode` 落库 → `resolve_mode_change` 解 oneshot)
顺序固定:先落库,后解 oneshot(否则 agent loop 先收到 Allow 但 DB 未
落库,出现不一致)。

#### Decision:共享 `QuestionStore` + 单 pending gate(`PendingInteraction` 互斥)

**Context**:现有 `QuestionStore` 与 `PermissionStore` 互不感知,
同 session 不能并发 2 个 pending question(`AlreadyPending` 错误)。
`request_mode_change` 是新一类待决交互,理论上应独立 store 避免
类型污染。

**Decision**:扩展 `QuestionStore` 的 value 类型为 `PendingInteraction`
tagged enum(`Question(ToolQuestionPayload) | ModeChange(ModeChangePayload)`),
**共用同一 store** + **单 pending gate**。② `register` 接口扩展接受
`PendingInteraction`,`resolve` 保留原签名(oneshot 不区分 kind);③
新 IPC `get_pending_interaction` 统一查询(替代旧 `get_pending_question`,
旧 IPC 软弃用保留 1 版本)。

**Why**:① **避免 UI 堆叠** —— 同 session 已有 pending card 时弹第 2
张卡会盖住第 1 张,user 不知道先答哪个;② **避免 store 分散管理** —
N 个独立 store 各自管各自互斥,新增第 3 类(将来 `request_user_input`?)
会无限扩张;③ **互斥语义天然** —— 跟 ask_user_question 是同一类
"user 决策驱动"交互,共享单 pending gate 语义最自然;④ **前端
streamController 简化** —— 1 个 IPC 替代 2 个并存。否决「独立
`ModeChangeStore`」:双 store 互不感知,允许同 session 1 个 question
+ 1 个 mode_change 并发,UI 难管理 + LLM 行为不可预测。

#### Decision:`PendingInteraction` tagged enum,统一 `InteractionResponse`

**Context**:`QuestionStore` 的 oneshot 通道原本是 `QuestionResponse`
enum(`Answered(Vec<QuestionAnswer>) | Cancelled`)。新增 mode_change
需要 oneshot 通道能传"新的 mode 是什么"(供 LLM 看到 prev → new)。

**Decision**:oneshot 通道类型 `QuestionResponse` → `InteractionResponse`
enum:`Answered { new_mode: SessionMode } | Cancelled`。`new_mode` 字段
对 ask_user_question 语义无意义(取 None 即可),对 request_mode_change
是 `prev_mode` → `new_mode` 的迁移记录,工具结果回填时
`tool_result = {"allowed": true, "prev_mode": "...", "new_mode": "..."}`。
tagged enum 而非 trait object —— 简单、序列化、零 dyn 成本。

---
