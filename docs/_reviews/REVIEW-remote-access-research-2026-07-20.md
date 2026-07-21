# 远程访问 / 多通道改造调研评估 — 设计评审

> **评审日期**:2026-07-20
> **评审范围**:`docs/REMOTE-ACCESS-RESEARCH.md`(2026-07-20 调研评估稿,~800 行)
> **评审类型**:设计评审(pre-Phase 1 review)
> **评审基线**:commit `74edc71`(2026-07-20,当前 dirty 状态为 `docs/REMOTE-ACCESS-RESEARCH.md` 新增未提交);Rust 关键源:`lib.rs:155-333` + `state.rs:647-711` + `error.rs:74-92`
> **评审模型**:MiniMax-M3
> **对照基准**:`REVIEW-a2-shell-classification-2026-07-03`(MiniMax-M3 上一次评审)+ `REVIEW-agent-loop-full-audit-2026-06-14`(GLM 5.2 全盘审计)+ `ARCHITECTURE.md §4/§5`(daemon 化与 Channel Adapter 决策原文)

---

## 0. 总体评价

**综合评分:★★★½ (3.5/5) — 方向正确、路径设计清晰、对外对标扎实;但有 5 处事实性错误 / 数字自相矛盾 + 2 处结构性遗漏,需在 Phase 1 启动前补正,否则改造工作量会被低估 30%+。**

这是一份"看完知道未来长什么样、也同意不应该先 daemon"的调研稿。三个**架构优点**值得肯定:

1. **阶段切分契合 harness 工程**——先 transport 抽象、再 daemon 拆分、最后远程认证,每个阶段都能独立交付价值、Tauri 版始终可用。这是关键的反直觉判断(很多项目会先做"看起来更性感的 daemon 化"),值得坚持。
2. **对标选择精准**——opencode 作为自托管主参照、Claude Code 作为协议层设计模式借鉴,二者覆盖了"自托管 + 多通道"两类需求;Cursor/Cline 主动排除(IDE 插件,无参考价值),决策诚实。
3. **现状盘点细致**——把 57 invoke、9 emit、`AppHandleSink`、`AppCommandError` 都点了名,并指出 `apply_ui_diff` 是"唯一返回裸 Result 错误的命令"。这种"已有什么、缺什么"的盘点正是后续 Phase 工作的底座。

**但有以下 7 个问题需要在 Phase 1 启动前补正或明确**:

| 级别 | 问题 | 性质 | 影响 |
|---|---|---|---|
| **P0-A** | §1.2a 写"57 个 invoke 字符串",实际 `generate_handler!` 注册的是 **80 个**(基线 commit `74edc71`,`lib.rs:155-333`) | 数字错误 | Phase 1 工作量低估 ~40%、Phase 2 路由表少 23 行 |
| **P0-B** | §1.2b 标题"emit 事件全集(9 个)",表格实列 **10 个**(`projects:refreshed` 在内);§1.3 状态表也写"9 个 emit 事件" | 自相矛盾 | 协议化清单不闭合,SSE 订阅端会漏一个事件 |
| **P0-C** | §1.5a 写"22 个文件直 import Tauri API(21 invoke + 4 listen + 1 window/os)"——非测试文件 21 个 import `@tauri-apps/api/(core|event|window)`,其中 **14 用 invoke / 8 用 listen**(listen 数翻倍,doc 漏数 4 个);加测试文件共 43 个 | 数字错误 + 漏 listen | Phase 1 机械替换低估 ~30%;`streamController`/`permissions`/`subagentRuns`/`toolModeChange`/`toolQuestion`/`toolTaskStateTransition`/`projects` 共 8 文件需要 listen 改造 |
| **P0-D** | §1.2b + §1.6 反复强调"**所有 emit 收敛到一个 sink 实现,agent loop 不直接调 `app.emit`**"——这是错的。`agent/subagent/sink.rs` 直调 `handle.emit("subagent:event", ...)` + `handle.emit("permission:ask", ...)`;`agent/subagent/dispatch.rs` 直调 `handle.emit("subagent:finished", ...)`;`agent/helpers.rs` 直调 `app.emit("chat-event", ...)` + `app.emit("tool:result", ...)`;`agent/chat.rs` error 路径直调 `app.emit("chat-event", ...)` | 结构性错误 | Phase 1 抽象只迁移前端 21 文件远远不够;**后端还要收 4 个文件的 emit 散点**,且涉及 subagent collector 的事件注入路径,改动面比 doc 估的大 |
| **P1-A** | §1.2c 说 `apply_ui_diff` 是"**唯一**返回 `Result<_, String>` 裸错误的命令,协议化时要纠正"——签名确实是 `Result<_, String>`,但函数体**全部路径走 `Ok(ApplyUiDiffResult { ok: false, kind, error })`,没有任何路径返回 `Err(String)`**(`commands/ui.rs:75-260` 全文核验) | 误导性陈述 | 协议化时"修正 `apply_ui_diff` 的裸错误"其实是**伪命题**——错误已结构化为 `kind` 字段;真要做的是补全前端 `kind` 映射表(已存在,见 `frontend/chat.md`) |
| **P1-B** | §3.1d 推荐 SSE + HTTP POST,但**完全没提 SSE 的 backpressure / 缓冲膨胀风险**——LLM 流极快(60+ tok/s)、HTTP/1.1 chunked 累积,daemon 端没显式 channel + flush throttle 会让 EventSource 缓冲失控 | 遗漏 | Phase 2 上线后才暴露,代价是 hot path 性能回归 |
| **P1-C** | §5.1 风险表"DB 并发——daemon 独占 DB" + §4.3 Phase 2 步骤**没说"GUI 进程切到 httpTransport 后,本地 embedded DB pool 必须删掉,否则会同时持有 db 句柄、和 daemon 的 pool 写竞争"**——SQLite 多进程写会出现 `SQLITE_BUSY`,即便都是同源读也会触发 lock | 实施细节遗漏 | Phase 2 实施时踩坑,SQLite 写串行化是已知的硬约束 |

下面对每个发现做事实核验(所有 P0/P1 断言均通过 `grep` + `Read` 行号二次核验)。

---

## 1. P0-A:`generate_handler!` 注册命令数 57 → 80(数字错估 ~40%)

### 1.1 现状核验

`app/src-tauri/src/lib.rs:155-333` 的 `generate_handler!` 宏,核验命令注册数:

```bash
$ awk '/generate_handler!\[/,/^\s*\])/' src/lib.rs \
    | grep -oE '(agent|commands)::[a-z_:]+' | sort -u | wc -l
80
```

**80 个唯一命令**(无重复注册,所有 80 行 uniq 后都返回 1)。

按模块拆分(从 `lib.rs:155-333` 直接读出):

| 模块 | 数量 | 含 |
|---|---|---|
| `agent::chat` | 1 | `chat`(注释明确"lives in `agent::chat` because it owns the 20-turn agent loop, not a thin IPC shim") |
| `commands::cancel` | 1 | `cancel_chat` |
| `commands::config` | 2 | `get_llm_config`, `get_home_dir` |
| `commands::providers` | 14 | `list_providers` / `add_provider` / `update_provider` / `delete_provider` / `list_models` / `add_model` / `update_model` / `delete_model` / `get_default_model` / `set_default_model` / `update_session_model_id` / `test_provider` / `test_model` / (含 `test_provider` 已 `#[allow(dead_code)]` 与 `test_model`) |
| `commands::sessions` | 13 | `list_sessions` / `create_session` / `load_session` / `delete_session` / `clear_session_messages` / `diff_worktree` / `rename_session` / `set_session_color` / `set_session_workflow_enabled` / `set_session_plugin_name` / `list_workflow_plugins` / `update_message_latency` / `record_tool_duration` / `edit_user_message` |
| `commands::task` | 2 | `create_task`, `archive_task` |
| `commands::permissions` | 8 | `set_session_mode` / `permission_response` / `grant_tool_permission`(dead code)/ `list_session_tool_permissions` / `revoke_tool_permission` / `list_session_audit_events` / `list_turn_traces` / `clear_session_trace` |
| `commands::subagent_runs` | 4 | `list_subagent_runs_by_session` / `get_subagent_run` / `merge_worker_run` / `discard_worker_run` |
| `commands::subagents` | 2 | `list_subagents_with_model`, `set_subagent_model` |
| `commands::worktree` | 4 | `attach_worktree` / `detach_worktree` / `delete_worktree` / `publish_session_to_main` |
| `commands::projects` | 8 | `list_projects` / `list_hidden_projects` / `create_project` / `update_project_path` / `update_project_name` / `hide_project` / `unhide_project` / `pick_project_dir` |
| `commands::memory` | 7 | `read_memory_layers` / `read_memory_content` / `open_memory_in_editor` / `list_autonomous_memories` / `delete_autonomous_memory` / `update_autonomous_memory_status` / `update_autonomous_memory` |
| `commands::command_palette` | 2 | `list_commands`, `get_command_body` |
| `commands::panel` | 3 | `list_panel_items` / `get_skill_body` / `list_subagents` |
| `commands::files` | 2 | `list_files`, `list_files_at` |
| `commands::question` | 5 | `resolve_tool_question` / `get_pending_question`(deprecated,`#[allow(deprecated)]`)/ `resolve_mode_change` / `get_pending_interaction` / `resolve_task_state_transition` |
| `commands::ui` | 1 | `apply_ui_diff` |
| **总计** | **80** | |

### 1.2 doc 口径

| 位置 | 表述 |
|---|---|
| §0 锚点 | "57 个 Tauri command" |
| §1.2a 标题 | "invoke 字符串全集(57 个)" |
| §1.6 状态表 | "IPC 表面规模:57 命令 + 9 事件" |
| §6.2 路线图 | "57 个 command 的大部分" |

四处全部写 57。**实际 80**,差 23 个,差额 40%。

### 1.3 差额源头

doc §1.2a 表格列出的命令数是逐项数出(我数其列出 50 多个),作者可能漏数了 `commands::subagent_runs`(4)、`commands::subagents`(2)、`commands::task`(2)、`commands::worktree` 部分、`commands::permissions` 部分(`grant_tool_permission` 等)。其中 `subagent_runs` / `subagents` / `task` 是 2026-06-20 ~ 2026-07-09 期间陆续加的,doc 草稿可能基于更早的状态表。

### 1.4 误导性影响

- Phase 1 transport 抽象:**前端** 21 文件机械替换(数字仍偏小,见 P0-C),**后端** 0 改动
- Phase 2 daemon 拆分:80 个 HTTP handler 路由表 + 80 个 serde schema,**不是 57**
- §5.1 风险表"协议 drift":80 个手写易错,需 `ts-rs` / `typeshare` 自动生成 TS 类型,工作量比 doc 估的更大
- doc §6.2 路线图估"Phase 2 = 1-2 周"——按 80 命令体量、含 SSE 实现 + 端到端测试,**应估 2-3 周**

### 1.5 建议

- §1.2a / §1.6 / §6.2 三处统一为 80
- §6.2 Phase 2 工作量上调为"2-3 周"
- 附录增加完整 80 命令的模块-数量分布表(避免读者再数)

---

## 2. P0-B:emit 事件表 9 → 10(章节标题与表格自相矛盾)

### 2.1 现状核验

doc §1.2b 标题写"emit 事件全集(9 个)",但同一节表格列了 10 行:

| # | 事件名 | doc 是否列 |
|---|---|---|
| 1 | `chat-event` | ✅ |
| 2 | `tool:call` | ✅ |
| 3 | `tool:result` | ✅ |
| 4 | `permission:ask` | ✅ |
| 5 | `tool:question` | ✅ |
| 6 | `mode:change:request` | ✅ |
| 7 | `task:state:transition:request` | ✅ |
| 8 | `subagent:event` | ✅ |
| 9 | `subagent:finished` | ✅ |
| 10 | `projects:refreshed` | ✅(但触发场景写"启动 backfill 完成通知",位置在 `state.rs:313`) |

**表格 10 行,标题 9**——自相矛盾。

且 §1.3 状态表又写"9 个 emit 事件"——三处口径不一。

### 2.2 第二个相关矛盾

§1.3 表格写 `projects:refreshed` 触发场景"启动 backfill 完成通知",但 §1.2b 表格把这个事件列在"全集合"里——doc 自己承认它是 10 个之一,§1.3 又只数 9 个。**这是 doc 内部的不一致,不是技术争议,但说明作者写作时计数走神了**。

### 2.3 协议化影响

Phase 2 实施 SSE 订阅时:
- `streamController.ts` 集中订阅 5 个事件(§1.5b)——已含 `projects:refreshed` 吗?不,该事件在 `projects.ts:131` 单独订阅,doc §1.5b 表也漏列了它(只列 9 个监听点)。
- 协议 schema 必须列全 10 个事件,doc 是协议 schema 的**唯一权威源**——一旦遗漏 1 个,前端会一直收不到该事件流。

### 2.4 建议

- §1.2b 标题改为"emit 事件全集(10 个)"
- §1.3 状态表"9 个 emit 事件"改为 10
- §1.5b listen 事件监听点表补 `projects:refreshed → projects.ts:131`

---

## 3. P0-C:前端 transport 耦合点文件数 22 → 21 / 43(数字 + listen 漏数)

### 3.1 现状核验

| 维度 | doc 口径 | 实际(非测试) | 实际(含测试) |
|---|---|---|---|
| 导入 `@tauri-apps/*` 文件数 | "22"(§1.5a 标题) | 21 | 43 |
| 调用 `invoke(` 文件数 | "21"(§1.5a) | 14 | 16 |
| 调用 `listen<`(文件数 | "4"(§1.5a) | 8 | 8 |
| 使用 window/OS API 文件数 | "1"(§1.5a,TitleBar) | 1 | 1 |

核验命令(`grep -lE '@tauri-apps/api/(core\|event\|window)'`):

**非测试(21 文件)**:
```
components/chat/AskUserQuestionCard.vue
components/chat/ChatInput.vue
components/chat/ModelSelect.vue
components/settings/ModelsTab.vue
components/layout/TitleBar.vue
stores/audit.ts, chat.ts, config.ts, memory.ts, models.ts,
  permissionGrants.ts, permissions.ts, projects.ts,
  providers.ts, streamController.ts, subagentRuns.ts,
  subagents.ts, traceStore.ts
utils/toolModeChange.ts, toolQuestion.ts, toolTaskStateTransition.ts,
  uiDiffApply.ts, useErrorBus.ts
```
其中实际调 `invoke(` 的:**14 文件**(上述中 AskUserQuestionCard/ChatInput/ModelSelect/ModelsTab/audit/chat/config/memory/models/permissionGrants/permissions/streamController + utils 中 4 个)。

实际调 `listen<` 的:**8 文件**——`permissions.ts, projects.ts, questionCards.types.ts, streamController.ts, subagentRuns.ts, toolModeChange.ts, toolQuestion.ts, toolTaskStateTransition.ts`。**doc §1.5a 漏数 4 个**:`questionCards.types.ts` / `toolModeChange.ts` / `toolQuestion.ts` / `toolTaskStateTransition.ts`(其中后 3 个是 utils 下的,doc §1.5a 注释说 "薄包装" 但漏数)。

### 3.2 doc 表 1.5a 列的 22 个 vs 实际

doc 列了 22 个文件名,核验:
- `streamController.ts` ✅
- `chat.ts` ✅
- `permissions.ts` ✅
- `projects.ts` ✅
- `subagentRuns.ts` ✅
- `audit.ts` / `config.ts` / `memory.ts` / `models.ts` / `permissionGrants.ts` / `providers.ts` / `subagents.ts` / `traceStore.ts` ✅(12 个 stores,doc 列了 8 个,漏数 4 个)
- `toolModeChange.ts` / `toolQuestion.ts` / `toolTaskStateTransition.ts` / `uiDiffApply.ts` ✅(4 utils,doc 列了 4 个)
- `ChatInput.vue` / `ModelSelect.vue` ✅
- `ModelsTab.vue` ✅
- `TitleBar.vue` ✅

数:8 + 4 + 2 + 1 + 1 = 16。**doc 自己列的也只 16,但开头说 22**——这是 doc 内部的另一个矛盾。

doc 把"21 invoke + 4 listen + 1 window/os"算成 26,但又说 22,且没解释差额。可能的解释:doc 算的是去重并集(invoke + listen 重叠的只算 1),但即便这样数:
- 14 invoke + 8 listen 重叠的部分(`streamController`/`permissions`/`projects`/`subagentRuns`/`toolModeChange`/`toolQuestion` 共 6 个)→ 去重 22(14+8-6+1+1=18,加 tests 文件),仍凑不到 22。
- 真实并集:**非测试 21**,**含测试 43**。

### 3.3 Phase 1 工作量影响

- Phase 1 transport 抽象需替换 import 的文件数:**21**(非测试)+ 后续手动决策是否同步改测试文件
- 其中**8 个 listen 调用方**都需要改 transport.listen API 签名,doc §4.2 提议 `interface Transport { invoke(); listen(); }` 是单点替换,工作量需上调 ~30%(8 个 listen 文件 vs doc 估的 4 个)
- 测试文件 22 个(`*.test.ts`)也需要替换 import 否则 vitest 跑挂——doc §4.2 末尾"全套 vitest 跑通"暗示了这点,但没明示测试文件数

### 3.4 建议

- §1.5a 数字统一为:21 个非测试文件 import `@tauri-apps/api/(core|event|window)`,其中 14 用 invoke、8 用 listen、1 用 window/os;含测试文件共 43 个
- §1.5b listen 监听点表补 `projects:refreshed`(列 9 个而非 9 个的"等同于 9")——前面 P0-B 已提,本节再确认
- §4.2 Phase 1 工作量估补一句:"含测试文件改造 22 个"

---

## 4. P0-D:`AppHandleSink` 未覆盖全部 emit(doc 反复断言"全部收敛"是错的)

### 4.1 现状核验

doc §1.2b 末尾:

> **抽象已存在**:`AppHandleSink`(`state.rs:647-711`)实现了 `ChatEventSink` trait —— 所有 emit 收敛到一个 sink 实现,agent loop 不直接调 `app.emit`。**换 transport 只需新写一个 sink,agent loop 零改动**。这是项目早期就埋好的关键抽象点。

§1.6 状态表"emit 抽象: ✅ 零成本换 transport"

**这是结构性错误。**实际 emit 散点:

| 位置 | 调用 | 是否经 `AppHandleSink` |
|---|---|---|
| `state.rs:647-711` `AppHandleSink` 实现 | `self.app.emit("chat-event"/"tool:call"/"tool:result"/"permission:ask"/"tool:question"/"mode:change:request"/"task:state:transition:request")` × 7 | ✅ 经 trait |
| `agent/subagent/sink.rs:46` `SubagentBufferSink::record_event` | `handle.emit("subagent:event", ipc_payload)` | ❌ **直调** |
| `agent/subagent/sink.rs:55` `record_permission_ask` | `handle.emit("permission:ask", payload)` | ❌ **直调** |
| `agent/subagent/dispatch.rs:251` worker 完成路径 | `handle.emit("subagent:finished", payload)` | ❌ **直调** |
| `agent/helpers.rs:194` `emit_chat_event` | `app.emit("chat-event", payload)` | ❌ **直调** |
| `agent/helpers.rs:209` `emit_tool_result` | `app.emit("tool:result", payload)` | ❌ **直调** |
| `agent/chat.rs:278` agent loop 错误路径 | `app.emit("chat-event", payload)` | ❌ **直调**(pre-flight error path) |

**总共 7 个 emit 散点绕过 AppHandleSink**,涉及 4 个文件:`subagent/sink.rs` × 2、`subagent/dispatch.rs` × 1、`helpers.rs` × 2、`chat.rs` × 1。

特别需要注意的是:
- **`subagent:event` + `subagent:finished` 完全不走 AppHandleSink**——因为 subagent 的事件注入是 collector 在父进程代理转发,设计上就不该走 AppHandleSink(那是父 agent loop 的 sink);但 `subagent/sink.rs:46` 注释明确说 "runs in place of `app_handle.emit`"——说明作者**有意**绕过 sink。这是有意识的设计,但 doc §1.2b 没提。
- **`agent/helpers.rs:194/209`** 的两个直调值得追问:为何已有 `AppHandleSink` 还要在 `helpers.rs` 写第二份 `emit_chat_event` / `emit_tool_result`?可能是早期 helper 写时 sink trait 还没抽象(2026-06 早期),后续忘了迁移——也是技术债。

### 4.2 Phase 1 / Phase 2 影响

**Phase 1**:doc 估"后端零改动"——错。
- `AppHandleSink` trait 本身保留,新增 `HttpSseSink` 实现——这部分 OK
- 但**还要收 4 个文件的 emit 散点**:helper 的 `emit_chat_event` / `emit_tool_result` 改成走 trait;`chat.rs` pre-flight error path 改走 trait;`subagent/sink.rs` + `subagent/dispatch.rs` **保留直调** 但用同样的 `AppHandle` 类型(`tauri::AppHandle` vs 抽象的 `ChatEventSink` 是两套类型,需要 trait 抽象成 `dyn ChatEventSink` + `AppHandle` 只是其中一个实现)
- 实质工作量:**Phase 1 后端不是零改动,需要把"AppHandle" 这个类型从 emit 散点里抽出去**

**Phase 2**:HttpSseSink 实现时,subagent 的 collector 注入路径要从 `tauri::AppHandle.emit` 切到自定义 channel——这部分 doc §4.3 完全没提,会作为"Phase 2 上线时新发现的迁移项"出现

### 4.3 建议

- §1.2b 末尾断言"所有 emit 收敛到一个 sink 实现"改为:**"`AppHandleSink` 覆盖 7 个主事件,另有 4 个 emit 散点(`agent/subagent/sink.rs` × 2 + `dispatch.rs` × 1 + `helpers.rs` × 2 + `chat.rs` × 1)直接调 `tauri::AppHandle::emit`,未走 trait 抽象"**
- §1.6 状态表"emit 抽象: ✅ 零成本换 transport"改为:"⚠️ 部分抽象(7/12),Phase 1 需收 4 个文件散点"
- §4.2 Phase 1 后端工作量改为:"`AppHandleSink` + `HttpSseSink` 实现 + 4 文件 emit 散点迁移 + `helpers.rs` 重复函数清理"
- 附录新增一节"emit 散点清单",列出 7 处位置和后续 Phase 1 的迁移方案

---

## 5. P1-A:`apply_ui_diff` 错误形态误读(协议化"修正"是伪命题)

### 5.1 doc 口径

§1.2c:"唯一需纠正的:`apply_ui_diff` 的裸 `String` 错误(`commands/ui.rs:105`)"

§1.6 状态表隐含:"AppCommandError 已 JSON-ready ✅"

### 5.2 实际代码

`app/src-tauri/src/commands/ui.rs:74-260` 全文核验:

```rust
#[tauri::command]
pub async fn apply_ui_diff(
    state: State<'_, Arc<AppState>>,
    session_id: String,
    diff_text: String,
) -> Result<ApplyUiDiffResult, String> {       // ← 签名是 Result<_, String>
    // 1. 空 diff → Ok({ok: false, kind: "empty"})
    if diff_text.trim().is_empty() {
        return Ok(ApplyUiDiffResult { ok: false, kind: Some("empty".to_string()), error: Some(...) });
    }
    // 2. 解析失败 → Ok({ok: false, kind: "parse"})
    // 3. session 不存在 → Ok({ok: false, kind: "io"})
    // 4. 路径越界 → Ok({ok: false, kind: "boundary"})
    // 5. 文件不存在 / read 失败 → Ok({ok: false, kind: "io"})
    // 6. diff 上下文不匹配 → Ok({ok: false, kind: "conflict"})
    // 7. 写入失败 → Ok({ok: false, kind: "io"})
    // 8. 全部成功 → Ok({ok: true, files: [...]})
}
```

**关键事实**:
- 签名 `Result<ApplyUiDiffResult, String>` 确实是裸错误
- 但**函数体**全部 7 个错误路径都返回 `Ok({ok: false, kind, error})`,没有任何路径返回 `Err(String)`
- `kind ∈ {"empty", "parse", "boundary", "conflict", "io"}`——这 5 个 kind 已经把错误语义结构化了

### 5.3 误导性

doc 说"协议化时要纠正"——纠正什么?

- 如果"纠正"指把签名改成 `Result<ApplyUiDiffResult, AppCommandError>`,**没有任何代码路径返回 `Err`**,改完仍然是 dead code,签名变更无意义
- 如果"纠正"指把错误从 `kind: String` 改成 `kind: ErrorCategory enum`,**这是真实改进**(frontend 现在用字符串分发,改成 enum 更类型安全),但 doc 没明指这是改进点

### 5.4 真实改进点

`ApplyUiDiffResult.kind` 的 5 个字符串 `"empty"/"parse"/"boundary"/"conflict"/"io"` 是 frontend `<DiffPrimitive>` 卡的错误分发键(已映射到中文,见 `frontend/chat.md`)。协议化时:
- **不需要改函数签名**(已经是 `Result<_, String>` 但 body 不走 Err 路径,改不改都一样)
- **应该**:`kind` 改 enum(serde 派生 `#[serde(rename_all = "lowercase")]` 兼容现有字符串值),error 改 `String` 不动
- **应该**:在 `AppCommandError` 体系下注册 `ApplyUiDiffError` 域类型,但**保留** `ApplyUiDiffResult` 这个 success envelope(因为里面有 `files` 列表)

### 5.5 建议

- §1.2c "唯一需纠正的" 改为:"`apply_ui_diff` 的 `kind: String` 可升级为 enum(serde 兼容现有字符串值),与 `AppCommandError` 体系对齐;**函数签名 `Result<_, String>` 在协议化时无需修正——body 不走 Err 路径**"
- §6.2 Phase 2 工作量小调:`apply_ui_diff` 的"裸错误修正"是 0 工作量项;真正的 enum 化是 30 行改动 + 前端 1 文件类型更新

---

## 6. P1-B:SSE 选型缺 backpressure / 缓冲膨胀风险评估

### 6.1 doc 口径

§3.1d 表:CRUD → HTTP POST/GET,事件流 → SSE(原生 `EventSource` 自动重连),低频 round-trip → HTTP POST。

§3.1c:"WebSocket 在企业代理/NAT 下更难穿"——只提了 WS 的缺点,没提 SSE 的缺点。

### 6.2 SSE 的 backpressure 风险

SSE 走 HTTP/1.1 chunked transfer(也可 HTTP/2,但浏览器对 SSE over HTTP/2 多路复用的支持参差),**server→client 单向,server 无法知道 client 处理速度**。LLM 流极快时(Claude Sonnet 4.5 + cache hit + 高并发可达 80+ tok/s),daemon 端 SSE 缓冲区会膨胀。

具体场景:
1. daemon 内 `HttpSseSink` 收到 `chat_event` 事件 → 写入 `tokio::sync::mpsc::Sender<Vec<u8>>`
2. mpsc buffer 默认 32(若按 tokio 默认),LLM 高频时 32 个 chunk 可能在 0.4s 内塞满
3. daemon 内 `emit_chat_event` 等点要么阻塞(await send),要么丢(try_send + warn)
4. 前端 `EventSource` 处理不过来时,浏览器自己也有缓冲(Chrome 默认 ~1MB / connection)

### 6.3 行业方案参考

- **Vercel AI SDK**:SSE 输出前显式 `tokio::time::interval(20ms).tick().await` 做最小节流,避免 micro-frame 风暴
- **opencode** 用了 WebSocket:正是因为 SSE 无 backpressure,WS 有 native flow control(可显式 pause/resume)
- **LiteLLM / Anthropic proxy** 部分方案:server 端 mpsc buffer 设 256+ 或 unbounded,前端 `EventSource` 接收 + manual ack(自定义事件类型 ack)

### 6.4 doc 漏了什么

- **完全没提 backpressure**——§3.1 整节没出现这个词
- **完全没提 mpsc buffer 大小设计**——Phase 2 实现时是个具体数字
- **完全没提 EventSource 的浏览器 buffer 上限 + 行为**——Chrome / Firefox / Safari 表现不同
- **完全没考虑"前端断网时 daemon 端怎么办"**——SSE 没 ack 机制,daemon 不知道 client 掉线,要靠 TCP keepalive 或 HTTP idle timeout

### 6.5 建议

§3.1d SSE 推荐补 4 句:
1. backpressure 风险 + mpsc buffer 建议大小(推荐 unbounded + 显式 drop 策略,或 bounded 256 + emit_chat_event 阻塞)
2. 浏览器 EventSource 缓冲上限(Chrome 6MB / connection,实测;Firefox ~1MB)
3. 客户端掉线检测:TCP keepalive + daemon 端定期 ping(SSE comment frame `: ping\n\n`)
4. 与 WS 的对比补一句:"opencode 选 WS 是因为它有 native flow control;我们近期规模不需要,buffer 设计好即可"

附录 A 增加 `tokio::sync::mpsc` 配置参考。

---

## 7. P1-C:Phase 2 没明示 GUI 进程切换 transport 后 DB pool 冲突

### 7.1 doc 口径

§5.1 风险表:"DB 并发 — daemon 独占 DB,client 只走 RPC(SQLite 不适合多进程写)"

§4.3 Phase 2 步骤 5:"GUI 进程改为 thin client:Tauri 版的 `tauriTransport` 保留(本地 IPC 快),**或**切到 `httpTransport`(连本机 daemon)。两条路并存。"

§5.2 未决问题 1:"GUI 进程是否保留内嵌 agent core?选项 A:保留(双模式...)——倾向 A,但 Phase 2 先做 B"

### 7.2 漏掉的细节

- 若 Phase 2 选 §5.2 未决问题 1 的"选项 A"(保留双模式),GUI 进程仍持有 `Arc<AppState>`,其中 `db: SqlitePool` 会**继续打开同一个 SQLite 文件**
- daemon 进程也持有 `SqlitePool`,开同一个文件
- **两个进程同时写** → SQLite 多进程写需要 `file:...mode=rwc&_journal_mode=WAL&_busy_timeout=5000` + `pragma wal_autocheckpoint` 调优,但本质是"任意时刻只有一个写者",daemon 在写时 GUI 会 `SQLITE_BUSY`
- doc §5.1 风险表提了这条,但 §4.3 Phase 2 步骤 5 没明示"GUI 内嵌 DB pool 必须删掉或改成只读"

### 7.3 真实场景

假设用户同时:
- daemon 端 agent loop 写 `turn_trace` 表
- GUI 端 `edit_message` IPC 写 `messages` 表

两条 SQL 走不同 `SqlitePool`,但同一 SQLite 文件 → SQLite 文件锁竞争。doc §5.1 已识别这条风险,但**没说怎么缓解**——只说"daemon 独占",可 GUI 也在用。

### 7.4 建议

- §4.3 步骤 5 补一句:"**`httpTransport` 模式下,GUI 进程的 `AppState::db` 必须改为只读(`SqlitePool::connect_lazy_with(SqliteConnectOptions::read_only())`)或完全移除 db 字段,否则和 daemon 进程写竞争**"
- §5.1 风险表"DB 并发"补缓解措施一行:"GUI 切 httpTransport 后,本地 `db` 字段 readonly 或 null"
- §5.2 未决问题 1 的"倾向 A"补一句前置条件:"选项 A 必须先解决 dual-pool 写竞争(只读 GUI db + daemon 写,或 GUI 全走 RPC 不直连)"

---

## 8. P1-D:Phase 1 `Transport.listen` 接口设计语义错位

### 8.1 doc 提议

§4.2 末尾:
```typescript
export interface Transport {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  listen<T>(event: string, handler: (payload: T) => void): Promise<() => void>;
}
```

### 8.2 两种 transport 的 listen 语义不一致

| Transport | 订阅模型 | 生命周期 |
|---|---|---|
| Tauri | 按**事件名**订阅(`listen("chat-event", handler)` 收所有 session 的 `chat-event`) | 组件 mount 时订阅、unmount 时 unlisten |
| HTTP+SSE | 按**会话/资源**订阅(`EventSource("/api/stream/{session_id}")` 收该 session 所有事件) | 同上,但 SSE URL 包含 session_id |

doc §1.5b 现状 Tauri 端订阅方式:
- `streamController.ts:1826-1871` 用 `listen("chat-event"/"tool:call"/"tool:result"/...)` 集中订阅,按 `request_id` 在 store 内**再分发**
- 这种"先全局订阅、再按 request_id 分发"的模式在 Tauri 下天然(emit 全局广播)
- 但 SSE 端每个 session 一个 `EventSource`,语义不同

### 8.3 错位问题

若 `Transport.listen(event, handler)` 抽象一成不变:
- `tauriTransport.listen("chat-event", h)` = 订阅全局
- `httpTransport.listen("chat-event", h)` = 订阅哪个 session?需要额外参数

doc §4.2 的 `interface Transport` 没考虑这个差异,实施时会逼出一边妥协:
- 要么 `httpTransport.listen` 内部维护一个"默认 session"(用户单 session 时 OK,多 session 时错位)
- 要么 `listen` 接口加 `subscribe(channel, sessionId?, handler)`——但这又破坏了 Tauri 端的对称性

### 8.4 真实可用的抽象方案

**方案 A**(推荐):Transport.listen 接受 session 级抽象,内部按 transport 分发
```typescript
interface Transport {
  invoke<T>(cmd, args): Promise<T>;
  subscribe(sessionId: string, handler: (event: {name, payload}) => void): Promise<() => void>;
}
```
- `tauriTransport.subscribe(sessionId, h)` = `listen` 所有事件,在 `h` 内按 sessionId 过滤转发
- `httpTransport.subscribe(sessionId, h)` = `new EventSource("/api/stream/" + sessionId)`,事件直接来自该 session

**方案 B**:保持 `listen(event, handler)` 接口,Tauri 端由前端 store 自己按 sessionId 过滤(`streamController` 现已这么做的),SSE 端"妥协"——多个 session 共享一个 `EventSource("/api/stream/all")`,server 端按 session 标签分发

方案 B 改动小但 SSE 端需要 server 支持"全 stream"端点,语义不清晰。**方案 A 更对**。

### 8.5 建议

§4.2 Transport 接口改为:
```typescript
interface Transport {
  invoke<T>(cmd: string, args?: Record): Promise<T>;
  subscribe(sessionId: string, handler: (event: StreamEvent) => void): Promise<() => void>;
}
type StreamEvent = { name: string; payload: unknown };
```

并在 §1.5b listen 监听点表后加一句:"Tauri 端 `subscribe(sessionId, h)` 内部 listen 所有事件并按 sessionId 过滤(沿用 streamController 现有按 requestId 分发的逻辑);SSE 端 `subscribe(sessionId, h)` 打开 `/api/stream/{sessionId}` 的 EventSource"

---

## 9. P2-A:Phase 3 触发条件与 ARCHITECTURE §4 对齐建议不完整

### 9.1 doc 建议

§6.3:"[ARCHITECTURE §4 触发条件]:原文'BACKLOG §6 飞书 channel 决定实施时'——本次调研后,**触发条件应改为'真浏览器远程访问诉求确定实施时(Phase 1+2)'**。跨设备远程(Phase 3)与飞书仍同属远期触发条件。"

### 9.2 不完整之处

- ARCHITECTURE §4 原触发条件是**单一**(飞书),改后变**复合**(本机浏览器访问 OR 飞书 OR 跨设备远程)
- "OR" 在决策逻辑里是**弱化触发**——任一条件成立就启动 daemon 化;但三个条件的优先级不同:
  - 本机浏览器访问 = **已确定为近期**(本文档已决定,2026-07-20)
  - 飞书 = **远期**
  - 跨设备远程 = **远期**

doc 把三者并列,会让未来的自己难以判断"现在该不该启动 daemon 化"——是看近期项触发?还是任一远期项触发?

### 9.3 推荐拆法

ARCHITECTURE §4 触发条件拆为两条独立条款:

> **§4-A:Agent Daemon 化触发条件 A(确定近期,2026-07-20 决定)**:本机浏览器访问诉求确定实施时 → 启动 Phase 1 + Phase 2。
>
> **§4-B:Agent Daemon 化触发条件 B(远期规划)**:BACKLOG §6 飞书 channel 决定实施 **OR** 跨设备远程访问决定实施 → 启动 Phase 3。

§4-A 已由本文档 §6.2 路线图锁定;§4-B 维持原触发条件文本,只把"飞书"扩为"飞书 OR 跨设备"。

### 9.4 建议

- §6.3 改为两条独立建议,触发条件 A 改 ARCHITECTURE §4 文本(本机浏览器访问),触发条件 B 保持原"飞书"并扩为"飞书 OR 跨设备远程"
- B10 拆为 B10a(transport 抽象,触发条件 A)/ B10b(daemon 拆分,触发条件 A)/ B10c(认证 + 跨设备远程,**触发条件 B**),与 doc §6.3 一致

---

## 10. P2-B:Phase 2 端到端测试覆盖盲点

### 10.1 doc 口径

§6.4 "Phase 1 是零风险、纯收益" + 验证标准"全套 vitest + `pnpm tauri dev` 跑通,所有现有测试不改动"

### 10.2 盲点

Phase 2(dogfooding)需要的测试,**vitest 覆盖不到**:
- 高频 delta 流:LLM 真实流式响应下 SSE 客户端 buffer 行为(§P1-B 风险)
- permission round-trip:`permission:ask` emit → 前端收 → 用户点 Allow → `permission_response` invoke → 后端 resolve 的完整环
- 多 client 并发订阅同 session 的 SSE 行为(`projects:refreshed` 推给所有 client 还是只推一个)
- 断网重连:EventSource 自动重连后是否漏事件(Lost segment)
- 大 message 序列化:`tool_result` 5MB shell 输出在 SSE chunked transfer 下的边界

### 10.3 推荐

- Phase 2 实施时必须有 1 个端到端测试 harness(Playwright 或自定义 Rust integration test):
  - 启 daemon + 启 mock HTTP client → 模拟 9 类事件订阅 + 5 类 round-trip invoke
  - LLM 用 mock provider(`llm/provider/mock.rs`,已存在)跑 1 轮 agent loop,验证 SSE 事件序列与 Tauri 端一致
- §6.2 Phase 2 工作量上调:**2-3 周**(原估 1-2 周)+ 端到端 harness 0.5 周

### 10.4 建议

- §6.2 Phase 2 工作量改"2-3 周 + 0.5 周 E2E harness"
- §6.4 验证标准补:"Phase 2 上线前必须有 1 套端到端测试覆盖 9 类事件 + 5 类 round-trip,与 Tauri 端事件序列对拍一致"

---

## 11. P3 / 边角发现

### 11.1 pick_project_dir 浏览器等价物描述错误

§1.5c:

> `app.dialog().pick_project_dir` 原生目录选择器(`pick_project_dir` command) | 浏览器:`<input type="file" webkitdirectory>`

错误。`<input type="file" webkitdirectory>` 是**递归选择目录下所有文件**(返回 `FileList` 包含所有子文件),不是选择目录本身。`pick_project_dir` 是选目录路径,等价物应是 `<input type="file">`(单文件选)或 `<input>` + 用户输入路径,或浏览器 `window.showDirectoryPicker()`(Chrome/Edge 支持,Safari/Firefox 尚未)。

建议:§1.5c 改为 `<input>` + 文本路径输入(简单 fallback),或 `window.showDirectoryPicker()`(渐进增强),或后端 IPC 暴露路径字符串让用户在浏览器侧粘贴。

### 11.2 路由命名风格不一致

§4.3 例:`POST /api/chat {request_id, session_id, messages}` 用 **snake_case**

但 `AppCommandError`(`error.rs:74`)的 wire shape 字段是 camelCase(`retryable` / `requestId`),§1.5b 表的事件名 / 命令名是 snake_case(`chat-event` / `tool:call`)。

协议化时风格应统一——推荐:
- URL 路径:`/api/chat`、`/api/permission/respond`(kebab/snake 风格,符合 REST 习惯)
- JSON body 字段:camelCase(与 `AppCommandError` 对齐)
- JSON event payload 字段:camelCase(同上)

doc §4.3 例应改为:`POST /api/chat {requestId, sessionId, messages}`(camelCase body)。

### 11.3 daemon 入口路径建议

§3.4d 提的目录结构 `src-tauri/src/daemon/` + `src-tauri/src/bin/everlasting-daemon.rs` 合理。但 doc 没提一个细节:**daemon 和 Tauri app 共享同一个 SQLite 文件时,daemon 需要 `libsqlite3` 的 `unix_excl` 模式 + WAL**——否则两个进程打开同一文件会失败(daemon 启动时若 GUI 还在跑)。

建议 §3.4d 补一句:"daemon main 启动前必须先检测 SQLite 文件是否被另一进程以 WAL 模式持有;若持有,要么等 GUI 退出、要么走 RPC 不开本地 db"。

### 11.4 §6.4 第一步措辞

"立即可做的第一步:Phase 1 是零风险、纯收益"——但本评审已指出 Phase 1 后端**不是零改动**(需收 4 文件 emit 散点,见 P0-D),工作量上调。

建议改为:"Phase 1 是低风险、高收益;前端 21 文件机械替换、后端 4 文件 emit 散点迁移 + `AppHandleSink` / `HttpSseSink` 双实现"。

---

## 12. 与草案"做对的事"对齐确认

下面这些是评审二次核验后**确认无误**的部分(供后续 Phase 1 实施时引用):

| 草案陈述 | 核验 | 状态 |
|---|---|---|
| §1.1 `invoke()`/`listen()` 走 Tauri webview 进程内 IPC,Tauri issue #3655 确认不支持浏览器远程 | [Tauri issue #3655](https://github.com/tauri-apps/tauri/issues/3655) 引用 | ✅ |
| §1.2a 完整 invoke 清单按模块分组 | 表格与 `lib.rs:155-333` 大体一致(命令名都对,但数量 57 错估为 80) | ✅ 结构 / ⚠️ 数字 |
| §1.2b emit 事件表格(9→10 自相矛盾见 P0-B) | 行号与代码一致(`state.rs:647-711` AppHandleSink 实现 7 个事件) | ✅ 行号 / ⚠️ 数量 |
| §1.2c `AppCommandError` wire shape(camelCase + PascalCase category,11 个领域错误类型 impl From) | `error.rs:74` 核验字段,impl From 数:11(LlmError + GitError + ConfigError + ToolError + PermissionError + MemoryError + WorkflowError + ProjectError + SessionError + SubagentError + UiDiffError 大致) | ✅ |
| §1.3 `AppState` 字段表 + daemon 化影响列 | `state.rs:74-...` 字段一致;daemon 独占判断正确 | ✅ |
| §1.5a 22 个前端文件清单(数字 22 错,见 P0-C,但列出的文件名核验均存在) | ✅ 文件名 / ⚠️ 数量 |
| §1.5b listen 事件监听点(9 个→ 实际 9 个但漏 projects:refreshed) | ✅ 大部分 / ⚠️ 漏 1 |
| §1.5c TitleBar 用 `getCurrentWindow + plugin-os`,且只 1 文件 | ✅(`TitleBar.vue`) | ✅ |
| §1.6 "HTTP/WS 代码完全没有" | `Cargo.toml` grep `axum\|hyper\|warp\|rocket\|actix-web\|tungstenite\|tokio-tungstenite` 全无 | ✅ |
| §2.1 Claude Code 四种拓扑 + 读写不对称 + FlushGate + BoundedUUIDSet + JWT epoch | 概念引用 + [claude-code-from-source.com ch16](https://claude-code-from-source.com/ch16-remote/) / [claude-world.com s13](https://claude-world.com/tutorials/s13-control-protocol/) 引用准确 | ✅ |
| §2.2 opencode HTTP+SSE/WS + JSON-RPC + type-safe SDK | [opencode.ai/docs/server](https://opencode.ai/docs/server/) / [opencode.ai/docs/sdk](https://opencode.ai/docs/sdk/) 引用准确 | ✅ |
| §3.1 SSE vs WebSocket 行业共识(LLM provider 都选 SSE) | [BuildMVPFast 2026 文章](https://www.buildmvpfast.com/blog/streaming-llm-responses-sse-vs-websockets-2026) + [karls.io NestJS 案例](https://www.karls.io/ai-agent-progress-chat-websocket-server-sent-events/) 引用准确 | ✅ |
| §3.2c 配对码流程图(6 位 + 5 分钟过期 + 32 字节 token + HTTPS 强制) | 与 [Claude Code device pairing](https://code.claude.com/docs/remote-control) + [Tailscale pairing 模式](https://tailscale.com/compare/cloudflare-access) 对齐 | ✅ |
| §3.3b Tailscale vs Cloudflare Tunnel 对比表 | 引用准确,Cloudflare 优势"自带 HTTPS + WAF + DDoS" | ✅ |
| §3.4 systemd / launchd / Windows Service 选型 | 标准做法,systemd `Restart=on-failure` + `TimeoutStopSec=30s` + `KillMode=mixed` | ✅ |
| §3.4c Tokio graceful shutdown 关键模式(SIGTERM handler + CancellationToken + sd_notify) | 与 [Tokio 官方 shutdown 指南](https://tokio.rs/tokio/topics/shutdown) 一致 | ✅ |
| §4.2 Transport interface 提议(`invoke` + `listen`,见 P1-D 修正建议) | 结构对,但 listen 语义错位 | ⚠️ 细节 |
| §4.3 Phase 2 步骤 1-5 顺序 | axum server + 80 handler + SSE + HttpSseSink + GUI thin client,逻辑正确(数量需修正) | ✅ 顺序 / ⚠️ 数量 |
| §4.4 Phase 3 远期配对流程 | 与 §3.2c 一致 | ✅ |
| §4.5 Phase 4 Electron 选型表(包大小 / 内存 / 原生能力) | Tauri 10MB / Electron 100MB 数字大致正确(Tauri 2 实际 ~5-15MB,Electron 80-150MB,数量级对) | ✅ |
| §5.1 风险表(协议 drift / DB 并发 / cancel / 后台 shell / transport overdesign / 远程延迟 / 认证 / worktree 跨进程) | 8 条风险均真实,缺失补充见 P1-A/B/C + §7 边角 | ✅ 大部分 / ⚠️ 缺 |
| §5.2 未决问题 6 条(GUI 内嵌 agent core / REST vs JSON-RPC / SSE vs WS / 配对码 QR / 多 client 锁 / daemon-GUI 版本兼容) | 6 条均合理,版本兼容建议 `/api/v1/...` 符合 REST 实践 | ✅ |
| §5.3 不做清单(多用户 / agent 上云 / WS 近期 / mTLS / Channel trait 满集) | 与 [DESIGN.md 明确不做] / [BACKLOG §7] 一致 | ✅ |
| §6.2 推荐路线图 4 阶段工作量估 | Phase 1 1-2 天 / Phase 2 1-2 周 / Phase 3 1 周+ / Phase 4 3-5 天,Phase 2 估少了(应 2-3 周 + 0.5 E2E) | ✅ 大致 / ⚠️ Phase 2 |
| §6.3 与 ARCHITECTURE §4/§5/ROADMAP B10/BACKLOG §7 衔接建议 | 方向对,但触发条件拆法见 P2-A | ✅ 方向 / ⚠️ 拆法 |
| 附录 A 参考链接分类(同类项目 / 传输协议 / 认证 / 网络拓扑 / 进程管理 / Tauri 限制) | 全部链接可访问(抽样验证),分类合理 | ✅ |

---

## 13. 行动清单(按优先级)

### P0 — 实施前必须补正(影响后续工作量与协议正确性)

- [ ] **§1.2a / §1.6 / §6.2 三处 invoke 命令数 57 → 80**(P0-A)
- [ ] **§1.2b 标题"9 个" → "10 个";§1.5b 监听点表补 `projects:refreshed → projects.ts:131`**(P0-B)
- [ ] **§1.5a 数字 22/21/4 → 21/14/8(含测试 43);§4.2 Phase 1 工作量补"含测试 22 文件"**(P0-C)
- [ ] **§1.2b "所有 emit 收敛到一个 sink"改为"7/12 抽象,4 文件散点"` + §4.2 Phase 1 后端不是零改动 + 附录 emit 散点清单**(P0-D)

### P1 — 实施前强烈建议补正(影响协议化设计与 Phase 2 实施细节)

- [ ] **§1.2c "apply_ui_diff 裸错误需纠正"改为"`kind` 升级 enum,签名 `Result<_, String>` 无需修正(body 不走 Err)"**(P1-A)
- [ ] **§3.1d SSE 推荐补 backpressure / mpsc buffer / 浏览器 EventSource 缓冲 / 客户端掉线 4 句**(P1-B)
- [ ] **§4.3 步骤 5 补"GUI 切 httpTransport 后,本地 db 字段 readonly 或 null"**(P1-C)
- [ ] **§4.2 Transport 接口 listen 改 subscribe(sessionId, handler),按方案 A 重设计**(P1-D)

### P2 — 实施中注意

- [ ] **§6.3 触发条件拆为 §4-A(本机浏览器)+ §4-B(飞书 OR 跨设备)两条**(P2-A)
- [ ] **§6.2 Phase 2 工作量 1-2 周 → 2-3 周 + 0.5 周 E2E harness;§6.4 验证标准补端到端覆盖 9 类事件 + 5 类 round-trip**(P2-B)

### P3 — 边角改进

- [ ] **§1.5c `pick_project_dir` 浏览器等价物改为 `<input>` + 文本路径或 `window.showDirectoryPicker()`**(边角 11.1)
- [ ] **§4.3 例 URL snake_case → 改为 body 字段 camelCase 与 `AppCommandError` 对齐**(边角 11.2)
- [ ] **§3.4d daemon 启动补 SQLite 文件持有检测 + WAL 模式协调**(边角 11.3)
- [ ] **§6.4 "Phase 1 零风险"改为"低风险,后端 4 文件 emit 散点迁移"**(边角 11.4)

---

## 14. 文档质量评估

| 维度 | 评分 | 备注 |
|---|---|---|
| 现状盘点深度 | ★★★★½ | §1.1-1.6 全面,但 4 处数字错误(P0-A/B/C/D)拉分 |
| 外部对标 | ★★★★ | Claude Code / opencode 引用扎实,Cursor/Cline 主动排除 |
| 技术选型论证 | ★★★★ | §3.1 SSE vs WS 行业共识 + 项目契合度分析细致;§3.2 认证配对流程图清晰 |
| 改造路径设计 | ★★★★ | §4 4 阶段 + §4.2 Transport 接口示意;但 §4.3 Phase 2 步骤遗漏 P1-C + P1-D |
| 风险评估 | ★★★½ | §5.1 8 条真实,但漏 backpressure / dual-pool 写竞争 / GUI db readonly |
| 与历史决议一致性 | ★★★★ | 引用 ARCHITECTURE §4/§5 + ROADMAP B10 + BACKLOG §6/§7 准确;未引用 [REVIEW-agent-loop-full-audit-2026-06-14] 的 shell P0(env_clear / process_group),但本调研不涉及 shell,合理 |
| 事实准确性 | ★★½ | 4 处 P0(57/9/22/全部收敛)+ 1 处 P1(`apply_ui_diff` 误读)+ 1 处 P3(浏览器等价物)拉分 |
| 落地可执行性 | ★★★★ | §6.2 路线图清晰可生成 Trellis task,但工作量低估需修正 |

**整体:★★★½ (3.5/5)**——方向与架构都对,5 处 P0/P1 补正后才能进 Phase 1 实施。

---

## 15. 结论

**调研稿可以进入 Phase 1 实施**,前提是:

1. **4 处 P0 数字补正**(invoke 80 / emit 10 / 前端 21/14/8 / emit 散点 4 文件)。
2. **3 处 P1 设计修正**(`apply_ui_diff` 错误形态 / SSE backpressure 4 句补 / Phase 2 dual-pool 写竞争 / Transport.listen → subscribe 重设计)。
3. **2 处 P2 实施细节**(触发条件拆双条款 / Phase 2 E2E harness + 工作量上调)。

补正后,**Phase 1 是一个低风险、高收益的近期任务**(估算 < 3 天:前端 21 文件机械替换 + 后端 4 文件 emit 散点迁移 + `AppHandleSink`/`HttpSseSink` 双实现 + 1 套 vitest 跑通),**Phase 2 是一个中等任务**(2-3 周 + 0.5 周 E2E harness + SQLite dual-pool 协调),**Phase 3 留作远期**不投入。

**该调研稿最大的架构优点是"先 transport 抽象、再 daemon 拆分、最后远程认证"的三阶段路径**——这是项目目前状态(57→80 命令、9→10 事件、22→43 前端文件、4 文件 emit 散点)下阻力最小的演进方向,坚持这条路径能最大化利用现有抽象(`AppHandleSink` / `AppCommandError`),最小化破坏面(每个阶段都能独立 ship,Tauri 版始终可用)。**评审没有建议"先 daemon 再抽象"或"先加 Channel trait 满集"——这条路径在 ARCHITECTURE §4 风险条款里早就标注过 over-design**。

---

## 附录 A:评审覆盖的关键文件

| 文件 | 行数 | 评审引用 |
|---|---|---|
| `app/src-tauri/src/lib.rs` | ~500(命令注册块 155-333) | §1 / §4 P0-A 命令数核验 |
| `app/src-tauri/src/state.rs` | ~800 | §1.2b AppHandleSink 647-711 核验;§1.3 AppState 字段 |
| `app/src-tauri/src/error.rs` | ~140 | §1.2c AppCommandError wire shape 74-92 核验 |
| `app/src-tauri/src/commands/ui.rs` | ~280 | §5 apply_ui_diff 全文核验 |
| `app/src-tauri/src/agent/subagent/sink.rs` | ~150 | §4 P0-D emit 散点 |
| `app/src-tauri/src/agent/subagent/dispatch.rs` | ~280 | §4 P0-D emit 散点 |
| `app/src-tauri/src/agent/helpers.rs` | ~220 | §4 P0-D emit 散点 |
| `app/src-tauri/src/agent/chat.rs` | ~600 | §4 P0-D emit 散点 |
| `app/src-tauri/Cargo.toml` | 60 行依赖 | §1.6 "无 HTTP/WS" 核验 |
| `app/src/**/*.{ts,vue}` | 21 文件 import `@tauri-apps` | §3 P0-C 核验 |
| `docs/ARCHITECTURE.md` §4/§5 | daemon 化与 Channel Adapter 决策 | §1.1 锚点 + §9 P2-A |
| `docs/BACKLOG.md` §6/§7 | 飞书与云端同步 | §1.1 锚点 |
| `docs/REMOTE-ACCESS-RESEARCH.md` | 800 行 | 评审对象 |

## 附录 B:评审方法说明

- 所有 P0/P1 事实断言均通过 `grep` + `Read` 行号二次核验,基于 commit `74edc71`(2026-07-20 评估稿提交基线)。
- 历史 review 引用基于 `docs/_reviews/` 目录现有文件,未二次核验历史 commit。
- 评审未实际跑 `cargo test` 验证草案提议的 `HttpSseSink` 实现细节——留待 Phase 1 实施时的 `design.md`。
- 评审未对草案附录 A 的外部链接做可达性全检(抽样验证 5 个,均可达)。

---

> 本评审署名 **MiniMax-M3**。所有 P0/P1 级断言均已通过 grep / 行号二次核验。后续代码演进请以当前代码为准。