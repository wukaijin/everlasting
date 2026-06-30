# AskUserQuestion 风格的 agent 反问 tool

## Goal

让 agent 在主循环里能向用户提出 1–4 个结构化选择题(每题 2–4 选项 + 可选 multi_select + 可选 preview),实现"挂起-等待-恢复"的阻塞语义,选项落进 LLM 上下文继续推理。和 Claude Code `AskUserQuestion` 等价能力,但 UI 形态是**inline message card**(非 modal),且**支持 session 挂起保留**(切走 session 不取消 question,切回可继续回答)。

只服务**主 session**,worker subagent 不适用(禁问)。

## Background(已查代码,落地时的关键事实)

### Tool dispatch 单点

`app/src-tauri/src/tools/mod.rs::execute_tool(name, input, ctx, guard, session_id, skill_cache, cancel)` 是唯一 dispatch。新 tool 加一个 `match` 分支。

### Agent loop 长阻塞改造点

`app/src-tauri/src/agent/chat_loop.rs::run_chat_loop` 在每个 turn 的 LLM 响应里累积 `tool_calls`,然后**串行/并行调 `execute_tool()`**。当前所有 tool 都是"短阻塞,立刻返回"。要实现"长阻塞 tool",loop 必须识别 tool 名属于"blocking set",**走新加的"blocking tool 挂起点"**(发 IPC 事件 → `tokio::select!{cancel, oneshot}` 等待 → 拿到 user 回答后当成 tool_result 回灌 messages)。

实现位置:`run_chat_loop` 内 tool 处理阶段。

**Turn 计数器代价**:`for turn in 1..=turn_limit` 是 Rust range iterator,blocking tool 恢复后 turn 必然递增 1 次。**v1 接受这个代价**(MAX_TURNS=50,blocking tool 是稀有事件);v2 可重构 while 循环实现零消耗——本设计不阻塞。

**Seq 持久化**:assistant tool_use 块跟现有路径 persist(seq=N),tool_result 块紧跟 persist(seq=N+1),顺序天然正确,无需特殊处理。

### QuestionStore 访问边界

QuestionStore 是 backend `Arc<tokio::sync::Mutex<HashMap<String, PendingQuestion>>>`,前端不能直接读。Session 切换时前端通过新 Tauri command `get_pending_question(session_id)` 查询当前 pending state(若有则 `Some(ToolQuestionPayload)`,否则 `None`)。Command 路由到 `state.question_store.get(&session_id)`。

### 已有的"长阻塞"先例(不直接复用,仅参考)

`app/src-tauri/src/agent/permissions/ask.rs` 实现了 "register_ask → IPC event `permission:ask` → tokio::select!{cancel, timeout, oneshot} → resolve_ask"。**结构上**可借鉴;**数据上** PermissionStore / PermissionAskPayload 跟新 tool 完全独立(权限是 security gate,question 是 UX)。

### IPC 通道

已有:`chat-event` / `tool:call` / `tool:result` / `permission:ask`。
新增:`tool:question`(推 question 到前端)+ `tool:question_resolved`(反向 resolve IPC)。

### Frontend 并行 session 架构

`app/src/stores/streamController.ts`:**SSE 单源 + LRU 20 + activeRequests**。最多 20 个 session 的状态在内存里并存。切换 session 不销毁其他 session 的状态(走 LRU 淘汰)。

`Pinia chat.ts` 是 facade,管 `sessions` 列表 + `currentSessionId` + CRUD 委托;`streamController` 是真正的数据源。

**关键设计含义**:切换 session 时,其他 session 的 in-memory state(包括 pending question)不会被销毁,只会按 LRU 自然淘汰。新 question card 必须在 LRU state 里跟 DB-loaded messages 合并渲染。

### Persistence

`persist_turn` 写 assistant + tool_result 进 `messages` 表,reload 完整还原。`tool_result` 块按现有路径自然进 messages。

### Audit

`record_tool_executed_audit` 已记录每次 tool 调用的 name/input/duration/exit_code,新 tool 自动纳入。

## Technical Notes

- **Worker filter**:`filter_tools_for_subagent` 的 `STRUCTURALLY_DISABLED` 列表加 `ask_user_question`,worker tool list 结构性屏蔽。
- **ChatEventSink trait 扩展**:`state.rs::ChatEventSink` 加 `fn emit_tool_question(&self, payload: &ToolQuestionPayload)`(**sync,非 async**——对齐 trait 现有 sync 设计,见 design §5.2 实施记录),`AppHandleSink` 走 `app.emit("tool:question", payload)`,`MockEmitter` push 进 `Vec` 给测试断言;`SubagentBufferSink` 用 trait 默认 no-op(worker 禁用,见 design §7)。
- **run_chat_loop 参数扩展**:新增第 N+1 参数 `question_store: QuestionStore`(跟 `permission_asks` 同位置),production + tests 都传。`chat` Tauri command 从 `AppState.question_store.clone()` 拿到。
- **挂起消耗 1 turn**:见 Background § Agent loop 长阻塞改造点,v1 不重构 while 循环。
- **不修改 `messages[0]` 注入**:tool_result 跟在 user prompt 之后自然走 messages 历史,不影响 `cache_control: Ephemeral` 缓存窗口。
- **单 pending 互斥**:复用 `PermissionStore` 的单 key `HashMap<session_id, oneshot>` 形态(独立 `QuestionStore`)。第二个并发调用 → `{"error": "已有 pending question,等当前回答完成"}` 结构化错误。
- **字段命名**:snake_case 跟其他 tool 一致,LLM 在 Claude Code 训练过这套 schema,零学习成本。
- **Question card UI**:不走 modal 形态,inline 在消息流(跟 `ToolCallCard` 同层)。Pinia store 加一个 `pendingBySession: Map<session_id, ToolQuestionPayload>` 字段作为**缓存**;**backend `get_pending_question` 是唯一 source of truth**——`streamController` 在 session 切换 / `ensureLoaded` 时通过该 command 查询,并以结果**覆盖**前端缓存。原因:`messagesBySession` 会按 LRU 淘汰久未访问的 session,前端缓存可能脏;而 pending 本身活在 backend `QuestionStore`,不受 LRU 影响,切回总能复原。
- **Session 切换保留 pending**:`QuestionStore` 保留 oneshot 不释放;切 session 时前端 invoke `get_pending_question(session_id)`,把对应 session 的 pending card 注入到该 session 的 message stream。**不触发 cancel**。
- **App crash 仍丢失**:oneshot 在内存里,进程死 = pending 全失。可接受(v1),因为崩了 agent loop 也死了。

## Requirements

### Tool 定义(R1–R5)

- **R1**. 新 builtin tool `ask_user_question`,`builtin_tools()` 列表里加入(在 `update_checklist` 旁边,低风险档)以暴露 schema 给 LLM。**执行不经 `execute_tool`**:由 `chat_loop` 在 tool 处理阶段特判 name == `ask_user_question` → 直接调 `ask_user_question::execute_blocking(...)`(`execute_tool` 不加 match 分支,避免双路径冗余)。
- **R2**. Tool 输入 schema(完全照搬 Claude Code `AskUserQuestion`,字段名 snake_case):
  ```rust
  struct AskUserQuestionInput {
      questions: Vec<Question>,  // 1..=4
  }
  struct Question {
      question: String,                   // 必填,题干
      header: Option<String>,             // ≤12 字符,card chip
      options: Vec<Option>,               // 2..=4
      multi_select: Option<bool>,         // 默认 false
  }
  struct Option {
      label: String,                      // 必填
      description: Option<String>,
      preview: Option<String>,            // markdown 渲染预览面板
  }
  ```
  schema 校验在 tool execute 入口(超长 / 越界直接 `is_error: true` + 结构化错误)。
- **R3**. Agent loop 阻塞分支:`chat_loop` 在 tool 处理阶段特判 `ask_user_question` → 调 `execute_blocking`(**内部先做 schema 校验,失败 short-circuit 返回 `is_error: true`,不挂起**)→ 校验通过则发 `tool:question` IPC 事件(携带 session_id + tool_use_id + questions)→ `tokio::select!{cancel, oneshot.recv()}` 等待 → 拿到 user 回答后当成 tool_result 回灌 messages。**session cancel 仍能中断**。**Turn 计数器递增 1 次**(v1 接受,见 Technical Notes)。
- **R4**. User 回答格式作为 `tool_result` 回灌:
  ```json
  [
    {"question": "...", "header": "...", "options": ["label1"], "multi_select": false},
    {"question": "...", "options": ["label2", "label3"], "multi_select": true}
  ]
  ```
- **R5**. 取消语义(用户在 card 上点"**跳过**"按钮):tool_result = `{"cancelled": true}`(LLM 看到错误自己决定下一步)。**UI 命名"跳过"避免和全局 Stop 按钮混淆**——wire payload 仍是 `{"cancelled": true}`(语义统一)。session cancel 中断走原 cancel 路径。

### UI 形态(R6–R8)

- **R6**. **Inline message card,非 modal**。question 在消息流里渲染成 `AskUserQuestionCard.vue` 组件,跟 `ToolCallCard` 同层,不弹浮层。
- **R7**. **整体提交语义(对齐 wire §4.2 一次性 answer 数组)**:Card 底部一个"提交"按钮,用户填完**所有** question(单选 = radio 选 1,多选 = checkbox 选 N)后一次性 resolve 整张 card。**禁止"单选即时 resolve"**——会与 wire 的一次性 answer 数组冲突(若 card 含多题,第一题点完就 resolve 会让其余题悬空)。**取消必须显式点 card 上的"跳过"按钮**(无 modal 关闭语义),跳过同样一次性作用于整张 card。
- **R7a**. **答完保留展开全程**:用户提交 / 跳过后,card 仍展示完整信息(问题 / 选项 / 描述 / preview / 已选项高亮 / 跳过状态),不折叠。便于跨 session 切换回顾完整 Q&A 历史(配合 R9–R11)。
- **R8**. 多 question(1–4 个)展示:**单 card 内多 section**(跟 Claude Code 一致),每个 section 是一个 question,带 header chip + 选项 + 描述 + preview 折叠面板。

### Session 挂起保留(R9–R11)

- **R9**. **QuestionStore 保留 oneshot 跨 session 切换不释放**。`QuestionStore: Arc<Mutex<HashMap<session_id, PendingQuestion>>>` key 是 session_id,value 是 oneshot + 完整 question payload(供前端 reload 时渲染)。
- **R10**. **前端 session 切换时,通过新 Tauri command `get_pending_question(session_id)` 查询当前 pending state**,把对应 session 的 pending card 注入到该 session 的 message stream。Command 路由 `state.question_store.get(&session_id)`,返回 `Option<ToolQuestionPayload>`。`streamController` 在 session 切换 / 加载时调此命令合并 pending card。切走 session 时不卸载 card,切回时通过命令复原。
- **R11**. **Pending question 不依赖 session_active_request 路径**。即使 session 没在 active request,oneshot 仍存活;agent loop 在 `run_chat_loop` 内 `tokio::select!` 持续等待(只要进程不死)。

### 并发(R12)

- **R12**. 同 session 单 pending question 互斥。第二个并发调用 → `{"error": "已有 pending question,等当前回答完成"}`(`is_error: true`),LLM 自然串行。

### Worker(R13)

- **R13**. **Worker subagent 不适用**(`STRUCTURALLY_DISABLED` 加入 `filter_tools_for_subagent` 列表)。

### 模式交互(R14–R15)

- **R14**. Plan / Edit / Yolo 三档默认都 Allow,无需 Tier 4 ask,跟 `update_checklist` 同档(`Risk::Low`)。
- **R15**. Plan mode 不禁用。**v1 三档(Plan/Edit/Yolo)行为一致,均挂起等用户回答**——Yolo 跳过的是*权限*询问,不是*信息*询问,agent 真正需要信息时仍该问。Yolo auto-decide(自动选默认项)策略留 v2。

### 持久化(R16–R18)

- **R16**. assistant turn(含 `ToolUse(ask_user_question)` 块)→ `persist_turn` 写 `messages` 表。
- **R17**. 下一轮 assistant turn 的 tool_result 块(用户答案)→ 同样进 `messages` → reload 还原完整 Q&A 流。
- **R18**. Audit:依赖现有 `record_tool_executed_audit`,每次调用产生一行 `tool_executed` audit。**不新建专用表** `tool_questions`。

### 取消 / 终止(R19–R20)

- **R19**. session cancel 路径(`token.cancelled()`)中断挂起。
- **R20**. 用户在 card 上点"跳过" → `{"cancelled": true}`。**session 切换不触发 cancel**(见 R9–R11)。**无 timeout 机制**——超时策略放 v2 跟 auto-decide 一起做。

### 并发执行与并行性(R21)

- **R21**. **`ask_user_question` 天然排除在 parallel eligibility 之外(无需改 `is_parallel_eligible`)**。`chat_loop.rs::is_parallel_eligible` 是**白名单**机制(`NAME_ELIGIBLE = [read_file, grep, glob, list_dir, use_skill]`,只有这 5 个纯读 tool 且路径在项目内才并行),`ask_user_question` 不在白名单 → 任一含它的批次 `is_parallel_eligible` 返回 false → **整批自动走 L1a 串行**(跟 `dispatch_subagent` 同机制——后者也是靠不在白名单走串行,而非显式 `=> false` 分支)。批次内执行顺序:**按 LLM 声明顺序串行,ask_user_question 在其位置阻塞,之前 tool 执行后等待,之后 tool 等 question 解决后才执行**——question 阻塞期间同 batch 其他 tool 全部暂停,跟 Claude Code 一致。

### 前端组件分发(R22)

- **R22**. **`MessageItem.vue` / `MessageList.vue` 增加 tool name → component 分发**:`ask_user_question` 路由到 `<AskUserQuestionCard>`,其他 tool 仍走 `<ToolCallCard>`(单 component,不分发)。`AskUserQuestionCard` 在 `<ToolCallCard>` 紧下方插入,共享 assistant turn 上下文。

## Acceptance Criteria

- **AC1**. Mock provider 集成测试:LLM 一次响应批次包含 `ask_user_question` + 普通短 tool(如 `shell`)→ **整批走 Serial** → 按 LLM 声明顺序执行,ask_user_question 在其位置阻塞,其他 tool 暂停等待 → question 解决后所有 tool_result 进 messages → LLM 下一轮看到答案 → **turn 计数因 blocking tool 调用递增 1 次**。
- **AC2**. Tool schema 单元测试:输入超长 / 越界(`questions` 空 / >4 / `options` 空 / >4 / `header >12 char`)→ `is_error: true` + 结构化错误消息。
- **AC3**. `filter_tools_for_subagent` 单元测试:输出不含 `ask_user_question`。
- **AC3'**. `is_parallel_eligible` 单元测试:`ask_user_question` 返回 false;含 `ask_user_question` 的批次被强制 Serial。
- **AC4**. 真实 IPC 集成测试(手工或 `tauri::test`):消息流里出现 `AskUserQuestionCard`,4 个 question,3 单选 + 1 multi_select,选 3 选项 + 1 multi_select 两个答案,所有选项落进 tool_result 回灌 LLM,第二轮 LLM 响应可读出选择。
- **AC5**. **Session 切换保留**:session A 有 pending question → 切到 session B → session B 工作一会儿 → 切回 session A → 通过 `get_pending_question` 命令 → pending question card 仍在,可选可答。Backend oneshot 全程不释放。
- **AC5'**. **`get_pending_question` Tauri command 行为测试**:session A 有 pending → command 返回 `Some(payload)`;切到 B, command(A) 仍 `Some`;resolve 后 command(A) 返回 `None`;不存在的 session 返回 `None`。
- **AC6**. 取消路径:用户在 card 上点"跳过"按钮 → tool_result = `{"cancelled": true}` → LLM 看到能优雅应对(不强制 LLM 行为,只验证 wire shape)。
- **AC7a**. Session reload(完整 process 重启)后**已 resolve 的** Q&A 完整可见:`messages` 表里能查到 assistant `ToolUse(ask_user_question)` 块 + 下一轮 assistant turn 的 tool_result 块含 user 答案。
- **AC7b**. **pending 中的 question 在 process 重启后丢失**(oneshot 在内存,backend `QuestionStore` 不持久化)——reload 后该 turn 的 tool_result 缺失,下轮 LLM 看到悬空 tool_use。v1 接受(进程死 agent loop 亦死,见 R20 / Technical Notes)。
- **AC8**. Plan / Edit / Yolo 三档下 tool 始终可用,无 Tier 4 ask,无 Yolo auto-decide。
- **AC9**. 并发路径:同 session 同 turn 第二次调用 → 返回结构化错误,第一个 pending 仍正常完成。
- **AC10**. **无 modal 验证**:DOM 里 `AskUserQuestionCard` 是消息流的 child 元素,无浮层 / 无遮罩 / 无 portal 到 body。`body > div` 的 portal target 里没有 modal class。
- **AC11**. **MessageItem 路由验证**:tool_use 块里的 `ask_user_question` 渲染成 `<AskUserQuestionCard>`,其他 tool name 仍渲染成 `<ToolCallCard>`。

## Out of Scope(v1)

- Timeout 机制 + auto-decide
- 自定义自由文本回答
- Question 模板 / 跨 session question 历史
- 实时协作(多用户同时回答)
- 录音/上传附件作为回答
- Worker subagent 反问(永久禁用)
- Yolo mode auto-pick 策略
- 多 question 排队 / modal 堆叠(已用 inline card 替代)
- App crash 恢复 pending question(进程级 in-memory,可接受丢失)
- **`for turn in 1..=turn_limit` 重构 while 循环零消耗**(v1 接受 turn +1,见 R3 / Technical Notes)

## Notes

- 取消语义只有 1 种(`{"cancelled": true}`),由用户显式触发。
- **无 timeout 兜底**:v1 唯一清理路径是用户提交 / 跳过 / session cancel(Stop)。若都不触发,该 agent 的 tokio task 持续阻塞——可接受(单 pending 互斥 + MAX_TURNS 不受影响,阻塞期间不计 turn 推进)。auto-decide / 超时策略放 v2。
- session 切换不取消 pending,跟现有 `permission:ask` 的 cancel-on-switch 行为**故意不一致**——permission 是 security(必须立即决策),question 是 UX(可以挂起保留)。
- audit 粒度:`record_tool_executed_audit` 只记 tool name/input/duration/exit_code,**用户回答内容不进 audit**,只落在 `messages` 表的 `tool_result` 块。v1 接受此粒度(将来需要结构化决策审计时再考虑专用表,v2 候选)。
- 缓存命中:tool_result 跟在 user prompt 之后自然走 messages 历史,不影响 `cache_control: Ephemeral` 缓存窗口。
- inline card UI 跟 `ToolCallCard` 同层(消息流 child),复用现有 Card 样式系统(`reka-ui` + 项目 tokens)。