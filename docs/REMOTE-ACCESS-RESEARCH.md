# 远程访问 / 多通道改造 — 调研评估

> **状态**:调研评估稿(2026-07-20)。本文档是 [ROADMAP.md §2 第四档 B10 飞书 IM](./ROADMAP.md) 触发条件的延伸——把"daemon 化"从抽象决策展开为可执行的改造路径。
> **关联**:[ARCHITECTURE.md §4 Agent Daemon 化](./ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路) / [§5 Channel Adapter 抽象](./ARCHITECTURE.md#5-决策channel-adapter-抽象为多入口铺路) / [BACKLOG.md §6/§7](./BACKLOG.md) / [TECH.md](./TECH.md)
> **结论速览**:见 [§6 决策建议](#6-决策建议)。一句话——**近期先抽 transport / Channel trait 这层"包装通道",daemon 化作为第二阶段解锁本机浏览器访问;认证配对 + 跨设备远程作为远期规划。不要先 daemon 再做包装**。

---

## 0. 背景与目标

> 📌 **给 reviewer 的项目锚点**(2026-07-20):
> - **Everlasting 是什么**:个人单人维护的"AI coding agent 工作台",作者自研 agent harness(非 Anthropic/Codex SDK 包装),目标对标 Claude Code 能力(聊天 + 编辑代码 + 运行命令)。**单人/单用户**用途,不考虑多租户。
> - **技术栈锁定**(见 [TECH.md](./TECH.md)):Tauri 2 + Vue 3 + Rust(tokio)+ SQLite + git2-rs + 手写 SSE。**这些不可换**——技术选型已"锁定",讨论请基于现栈。
> - **现状成熟度**:MVP + V2 多档路线图**21+ 项已实施**(多 Provider / 权限系统 / memory / subagent / workflow / 后台 shell / trace viewer 等)。**79 个 Tauri command、10 个 emit 事件、21 个前端文件直 import Tauri API**(其中真实 `listen()` 调用集中在 4 个文件)。**这不是 greenfield 项目**,任何改造方案需考虑迁移成本与回归风险。
> - **emit 抽象现状**(2026-07-20 review 后修正):并非"全部收敛"。10 个事件中 **7 个经 `AppHandleSink`**(`state.rs:647-711`),另 **6 处直调 `app.emit`**(agent error 路径 / helpers / subagent sink / projects backfill),涉及 4 个文件。详见 §1.2b。
> - **早期文档已留 daemon 化设计**:[ARCHITECTURE §4/§5](./ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路) 早就把"拆 daemon + Channel Adapter trait"写进目标态,但触发条件只挂在"飞书 IM 决定实施时",**是预估而非完整方案**。本文档把这条路展开为可执行的渐进迁移评估。
> - **本文定位**:调研评估稿,**不是 PRD**。覆盖现状盘点、外部对标(Claude Code / opencode 等)、技术选型权衡、改造路径设计。近期交付只到 Phase 1+2(本机浏览器访问),跨设备远程(Phase 3)是远期规划。reviewer 可质疑方案、补充对标、指出风险,但不必当成已批准的实施计划。

### 0.1 用户诉求

1. **真浏览器远程访问**(不是 Electron 换壳)——浏览器关掉,agent 继续跑
2. **本地直连**(LAN / 同机)
3. **远程配对**(跨网络、跨设备)
4. **可能上 Electron**(作为另一类 client)

> 📌 **近期 / 远期划分**(2026-07-20 决定):诉求 1+2(本机浏览器访问)是近期 Phase 1+2 的目标;诉求 3(跨设备远程配对)定为**远期规划**(Phase 3),等本机访问跑通、协议稳定后再启动;诉求 4(Electron)是可选项。本文全文覆盖四个诉求的技术方案,但 §6 路线图明确标注了哪些是近期交付、哪些留作远期。

早期文档([ARCHITECTURE §4](./ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路)、[ROADMAP B10](./ROADMAP.md))只把 daemon 化挂在"飞书 IM 触发"这一个条件上,是**预估**而非完整方案。本次调研要做的事:

- **盘点现状**:现在的 IPC 表面到底有多大?前后端耦合点在哪?
- **外部对标**:同类项目(Claude Code / opencode / Cursor / Cline)怎么做远程化?
- **技术选型**:传输协议 / 认证配对 / 网络拓扑 / 进程管理 的可选方案与权衡
- **改造路径**:从现状到"网页 + Electron + 本地直连 + 远程配对"的渐进迁移路线
- **落盘**:让后续任何接手的人(包括未来的自己)不必重新调研

**非目标**:本文不做飞书 channel 的设计(那是 [BACKLOG §6](./BACKLOG.md#6-im-通道飞书) 的事),也不做云端同步([BACKLOG §7](./BACKLOG.md#7-云端状态同步))。

---

## 1. 现状盘点

### 1.1 进程拓扑(当前 MVP)

```
┌─────────────────────────────────────────┐
│  Tauri GUI Process(单进程)              │
│  ┌──────────────────────────────────┐  │
│  │  Vue 3 前端(webview 内)          │  │
│  │  invoke() / listen() ──┐         │  │
│  └────────────────────────┤─────────┘  │
│                           │ Tauri IPC  │
│  ┌────────────────────────▼─────────┐  │
│  │  Rust 后端(同进程 tokio runtime)│  │
│  │  - 79 个 #[tauri::command]       │  │
│  │  - 10 个 app.emit 事件           │  │
│  │  - agent loop(spawn per chat)    │  │
│  │  - SQLite / git2 / bg shell      │  │
│  └──────────────────────────────────┘  │
└─────────────────────────────────────────┘
```

**关键事实**:`invoke()` / `listen()` 是 Tauri webview **进程内**的 IPC,基于 webview 的 postMessage 机制,**无法从外部浏览器访问**(已由 [Tauri issue #3655](https://github.com/tauri-apps/tauri/issues/3655) 确认:Tauri 不是 web server,不支持浏览器远程访问后端)。这是"网页访问"诉求必须跨过的第一道硬墙。

### 1.2 IPC 表面盘点(Rust 端)

#### 1.2a invoke 字符串全集(79 个)

按模块分组,完整清单。权威源:`app/src-tauri/src/lib.rs:155-333` 的 `generate_handler!` 宏(2026-07-20 实测,剔除注释行后计数)。

| 模块 | 数量 | invoke 字符串 | 备注 |
|---|---|---|---|
| `agent::chat` | 1 | `chat` | 主入口,spawn agent loop;通信走 emit |
| `commands::cancel` | 1 | `cancel_chat` | cancel token |
| `commands::sessions` | 14 | `list_sessions` / `create_session` / `load_session` / `delete_session` / `clear_session_messages` / `rename_session` / `set_session_color` / `set_session_workflow_enabled` / `set_session_plugin_name` / `list_workflow_plugins` / `diff_worktree` / `update_message_latency` / `record_tool_duration` / `edit_user_message` | session CRUD + worktree + F5 延迟 |
| `commands::providers` | 13 | `list_providers` / `add_provider` / `update_provider` / `delete_provider` / `list_models` / `add_model` / `update_model` / `delete_model` / `get_default_model` / `set_default_model` / `update_session_model_id` / `test_provider` ⚠️ / `test_model` | `test_provider` 已 `#[allow(dead_code)]` |
| `commands::projects` | 8 | `list_projects` / `list_hidden_projects` / `create_project` / `update_project_path` / `update_project_name` / `hide_project` / `unhide_project` / `pick_project_dir` | `pick_project_dir` 依赖 Tauri dialog API |
| `commands::permissions` | 8 | `set_session_mode` / `permission_response` / `grant_tool_permission` ⚠️ / `list_session_tool_permissions` / `revoke_tool_permission` / `list_session_audit_events` / `list_turn_traces` / `clear_session_trace` | `grant_tool_permission` 已 dead_code |
| `commands::memory` | 7 | `read_memory_layers` / `read_memory_content` / `open_memory_in_editor` / `list_autonomous_memories` / `delete_autonomous_memory` / `update_autonomous_memory_status` / `update_autonomous_memory` | `open_memory_in_editor` spawn 外部进程 |
| `commands::question` | 5 | `resolve_tool_question` / `resolve_mode_change` / `resolve_task_state_transition` / `get_pending_interaction` / `get_pending_question` ⚠️ | `get_pending_question` 已 `#[deprecated]` |
| `commands::worktree` | 4 | `publish_session_to_main` / `attach_worktree` / `detach_worktree` / `delete_worktree` | git worktree 生命周期 |
| `commands::subagent_runs` | 4 | `list_subagent_runs_by_session` / `get_subagent_run` / `merge_worker_run` / `discard_worker_run` | worker 收口 |
| `commands::panel` | 3 | `list_panel_items` / `get_skill_body` / `list_subagents` | `/` + `@@` trigger 面板 |
| `commands::task` | 2 | `create_task` / `archive_task` | workflow 任务 CRUD |
| `commands::subagents` | 2 | `list_subagents_with_model` / `set_subagent_model` | per-subagent model override |
| `commands::files` | 2 | `list_files` / `list_files_at` | `@` 补全,后者仅接受 `root == "/"` |
| `commands::config` | 2 | `get_llm_config` / `get_home_dir` | `get_home_dir` 依赖 AppHandle.path() |
| `commands::command_palette` | 2 | `list_commands` / `get_command_body` | `/` trigger 面板(与 panel 是**不同函数**,功能上服务于重叠的 trigger) |
| `commands::ui` | 1 | `apply_ui_diff` | `ApplyUiDiffResult` 见 §1.2c |
| **总计** | **79** | | |

> ⚠️ **数字核验说明**:2026-07-20 review(MiniMax-M3 / DeepSeek-v4-pro)指出早期草稿的"57"严重低估。实测 `generate_handler!` 宏内 `(agent|commands)::xxx` 路径出现 80 次,但其中 1 次是注释 `// lives in agent::chat because...`,剔除后**真实唯一命令数 = 79**。

#### 1.2b emit 事件全集(10 个)与抽象现状

全部 `app.emit(NAME, payload)`(`tauri::Emitter::emit`),**无** `emit_to` / `emit_filter`。

| 事件名 | payload 类型 | 主要触发点 | 是否经 `AppHandleSink` |
|---|---|---|---|
| `chat-event` | `ChatEventPayload { request_id, event: ChatEvent }` | `state.rs:653`(sink);**另:`agent/chat.rs:187` error 路径直调**、`agent/helpers.rs:160` 直调 | ⚠️ 部分(主路径经 sink,error/helper 直调) |
| `tool:call` | `ToolCallPayload { request_id, id, name, input }` | `state.rs:658` | ✅ 经 sink |
| `tool:result` | `ToolResultPayload { request_id, tool_use_id, content, is_error }` | `state.rs:663`(sink);**另:`agent/helpers.rs:170` 直调** | ⚠️ 部分 |
| `permission:ask` | `PermissionAskPayload` | `state.rs:668`(sink);**另:`agent/subagent/sink.rs:698` 直调**(worker 路径) | ⚠️ 部分 |
| `tool:question` | `ToolQuestionPayload` | `state.rs:679` | ✅ 经 sink |
| `mode:change:request` | `ModeChangePayload` | `state.rs:692` | ✅ 经 sink |
| `task:state:transition:request` | `TaskStateTransitionPayload` | `state.rs:706` | ✅ 经 sink |
| `subagent:event` | `{ runId, sessionId, kind, payload, timestamp }` | **`agent/subagent/sink.rs:279` 直调**(有意绕过 sink,走 collector 路径) | ❌ 直调 |
| `subagent:finished` | 动态 payload | **`agent/subagent/dispatch.rs:1192` 直调** | ❌ 直调 |
| `projects:refreshed` | `usize` | **`state.rs:317` 直调**(启动 backfill 完成通知,从 `AppState::load` 内) | ❌ 直调 |

**⚠️ 抽象现状修正**(2026-07-20 review 后):

> 早期草稿断言"所有 emit 收敛到一个 sink 实现,agent loop 不直接调 `app.emit`"——**这是错的**。
>
> 实测 10 个事件中,**7 个经 `AppHandleSink`**(`state.rs:647-711`,实现 `ChatEventSink` trait),**3 个完全直调**(`subagent:event` / `subagent:finished` / `projects:refreshed`),另外 `chat-event` / `tool:result` / `permission:ask` 还有**直调旁路**。
>
> **共 6 处直调散点**,涉及 4 个文件:
> - `agent/chat.rs:187` — pre-flight error 路径直调 `chat-event`
> - `agent/helpers.rs:160,170` — `emit_chat_event` / `emit_tool_result` 直调(疑似早期 helper,后续未迁移到 sink)
> - `agent/subagent/sink.rs:279,698` — `subagent:event` / `permission:ask` 直调(**有意为之**:subagent 事件注入走 collector 路径,与父 agent loop 的 sink 是两套语义,注释明确 "runs in place of app_handle.emit")
> - `agent/subagent/dispatch.rs:1192` — `subagent:finished` 直调
> - `state.rs:317` — `projects:refreshed` 直调(从 `AppState::load` 的后台 backfill 任务)
>
> **对 daemon 化 / transport 抽象的影响**:Phase 1 后端**不是零改动**。换 transport 时这 6 处散点都要收(subagent 路径需要保留 collector 双通道语义,不能简单合并到 sink)。详见 §4.2 修正后的工作量。

#### 1.2c 错误协议(已 ready)

`AppCommandError`(`error.rs:74`)已经是 `Serialize` 的 flat wire shape:

```rust
struct AppCommandError {
    category: ErrorCategory,  // Auth | RateLimit | InvalidRequest | Server | Network
    kind: String,             // "LlmError" / "GitError" 等
    message: String,          // 中文友好消息
    retryable: bool,
    request_id: Option<String>,
}
```

camelCase + PascalCase category,**跨进程 JSON 协议化零成本**。11 个领域错误类型已经 `impl From<E> for AppCommandError`。

**`apply_ui_diff` 的特殊情况**(2026-07-20 review 后修正):

> 早期草稿说"`apply_ui_diff` 是唯一返回 `Result<_, String>` 裸错误的命令,协议化时要纠正"——**这是伪命题**。
>
> 实测 `commands/ui.rs:74-260` 全文,签名确实是 `Result<ApplyUiDiffResult, String>`,但**所有 8 个错误路径都返回 `Ok({ok: false, kind, error})`**,没有任何路径返回 `Err(String)`——签名里的 `String` 是死代码。
>
> 错误语义已经结构化进 `kind` 字段,取值 `empty` / `parse` / `boundary` / `conflict` / `io`(5 种)。真正可做的改进是**把 `kind: String` 升级为 enum**(serde `rename_all = "lowercase"` 兼容现有字符串值),与 `AppCommandError` 体系对齐——但函数签名 `Result<_, String>` 在协议化时**无需修正**。

### 1.3 共享状态(`AppState`)

唯一状态容器:`Arc<AppState>`(`state.rs:74`),通过 `State<'_, Arc<AppState>>` 在 79 个 command 间共享。

| 字段 | 类型 | daemon 化影响 |
|---|---|---|
| `db` | `SqlitePool`(内部 Arc) | 跨进程**不能共享 pool**,要么 daemon 独占 DB、client 只走 RPC,要么每 client 各自开 pool(冲突) |
| `catalog` | `Arc<RwLock<ProviderCatalog>>` | daemon 独占 |
| `cancellations` | `Arc<Mutex<HashMap<String, CancellationToken>>>` | daemon 独占(cancel 必须命中 daemon 进程的 token) |
| `session_active_request` | `Arc<Mutex<HashMap<String, String>>>` | 同上 |
| `inflight_exits` | `Arc<Mutex<HashMap<String, oneshot::Receiver<()>>>>` | 同上 |
| `permission_asks` / `question_store` | oneshot 注册表(`Arc<Mutex<HashMap>>`) | **关键**:跨进程时 oneshot 必须在 daemon 内 resolve,client 只能发"用户答了"消息 |
| `background_shells` | `Arc<InMemoryBackgroundShellRegistry>` | 持有 `tokio::process::Child`,**必须**在 daemon 进程 |
| `read_guard` / 4 个 cache | 内部 Arc | daemon 独占 |

**结论**:几乎所有有状态字段都必须随 daemon 迁移。`AppState::load`(`state.rs:215-364`)是 daemon main 的蓝本。

### 1.4 后台任务三类

| 类别 | 位置 | daemon 化难度 |
|---|---|---|
| **agent loop**(per-chat spawn) | `agent/chat.rs:261` | 中:spawn 逻辑不变,只是 emit sink 换 transport |
| **启动一次性任务**(3 个) | `lib.rs:133/149` + `state.rs:313` | 低:照搬到 daemon main |
| **常驻有状态单例** | `BackgroundShellRegistry` | 高:持有 Child 进程,跨进程时 client 无法直接 spawn,必须全部 RPC 化 |

### 1.5 前端 transport 耦合点

#### 1.5a 直接 import `@tauri-apps/*` 的文件(共 21 个非测试文件)

这是 transport 抽象要替换的最小集合(2026-07-20 实测核验):

**统计**:
- **21 个非测试文件** import `@tauri-apps/api/(core|event|window)` 或 `@tauri-apps/plugin-os`
- 其中 **17 个文件**调用 `invoke`(见下表;另有 `main.ts` 仅 import 类型不算调用点)
- **真实 `listen()` 调用集中在 4 个文件**(streamController 6 个监听点 / permissions 1 个 / projects 1 个 / subagentRuns 2 个,共 10 个监听点)
- **1 个文件**用窗口/OS API(TitleBar)
- 无任何 `@tauri-apps/api/{dialog,fs,path,menu,shell,http}` 导入
- 另有 `*.test.ts` 文件 22 个也 import 了 Tauri API(测试 mock),Phase 1 需同步改造

> ⚠️ **listen 文件数说明**:MiniMax review 报"8 个文件调 listen",实测其中 4 个(`toolModeChange.ts` / `toolQuestion.ts` / `toolTaskStateTransition.ts` / `questionCards.types.ts`)是**注释里提到 `listen<>`**,非真实调用点。真实 `listen()` 只在 4 个 store 文件里。

| 类别 | 文件 | 用途 |
|---|---|---|
| **stores(13)** | `streamController.ts` | invoke + listen(**核心改造点**,单文件消费 6 invoke + 6 listen) |
| | `chat.ts` | invoke |
| | `permissions.ts` | invoke + listen(`permission:ask`) |
| | `projects.ts` | invoke + listen(`projects:refreshed`) |
| | `subagentRuns.ts` | invoke + listen(`subagent:event` / `subagent:finished`) |
| | `audit.ts` / `config.ts` / `memory.ts` / `models.ts` / `permissionGrants.ts` / `providers.ts` / `subagents.ts` / `traceStore.ts` | invoke |
| **utils(4)** | `toolModeChange.ts` / `toolQuestion.ts` / `toolTaskStateTransition.ts` / `uiDiffApply.ts` | invoke(薄包装,单命令封装) |
| | `useErrorBus.ts` | invoke 错误解析 |
| **components(4)** | `chat/ChatInput.vue` / `chat/ModelSelect.vue` / `chat/AskUserQuestionCard.vue` / `settings/ModelsTab.vue` | invoke |
| | `layout/TitleBar.vue` | `getCurrentWindow` + `plugin-os`(窗口控制 + 平台判断) |
| **entry(1)** | `main.ts` | import 类型(无真实调用,可不动) |

#### 1.5b listen 事件全集(10 个监听点,与 Rust emit 一一对应)

| 事件名 | 监听点 | 说明 |
|---|---|---|
| `chat-event` | `streamController.ts:1826` | LLM 流主通道,按 request_id 路由 |
| `tool:call` | `streamController.ts:1829` | |
| `tool:result` | `streamController.ts:1832` | |
| `tool:question` | `streamController.ts:1848` | `ask_user_question` 卡片 |
| `mode:change:request` | `streamController.ts:1861` | `request_mode_change` 卡片 |
| `task:state:transition:request` | `streamController.ts:1871` | W1 task 状态转换卡片 |
| `permission:ask` | `permissions.ts:252` | Tier 3/4 权限 modal |
| `subagent:event` | `subagentRuns.ts:442` | worker transcript 流 |
| `subagent:finished` | `subagentRuns.ts:473` | worker 完成 |
| `projects:refreshed` | `projects.ts:131` | 启动 backfill 完成通知 |

**streamController 是单点 funnel**——6 个流式事件都从这里进 Pinia store,按 `activeRequests: Map<requestId, RequestState>`(`streamController.ts:736`)路由。这是 transport 抽象的天然切点。该文件单点消费了 6 个 invoke(`chat` / `cancel_chat` / `load_session` / `record_tool_duration` / `update_message_latency` / `get_pending_interaction`)+ 6 个 listen,是 transport 抽象后最大的受益者。

#### 1.5c 其他 Tauri API 使用(非 IPC)

| API | 用途 | 浏览器/Electron 等价物 |
|---|---|---|
| `@tauri-apps/api/window` (`getCurrentWindow`) | TitleBar 最小化/最大化/关闭 | 浏览器:N/A(无标题栏);Electron:`BrowserWindow` API |
| `@tauri-apps/plugin-os` (`platform`) | 平台判断(mac/win/linux) | 浏览器:`navigator.userAgent`;Electron:`process.platform` |
| `app.dialog().pick_project_dir` | 原生目录选择器(`pick_project_dir` command) | 浏览器:**拿不到绝对路径**(安全限制),需降级见下 |

**`pick_project_dir` 浏览器降级**(2026-07-20 review 补充):

> 浏览器 `<input type="file" webkitdirectory>` 只返回 `File` 对象 + 相对路径名,**拿不到绝对路径**(浏览器安全限制,无法绕过)。`pick_project_dir` 在 Tauri 版返回的是文件系统绝对路径。
>
> 降级方案:浏览器版改为**文本输入框让用户手动粘贴/输入项目绝对路径**,daemon 端校验路径存在性。这是 UX 降级,不是 transport 适配能解决的——Phase 2 需要专门做 UX spike。

**结论**:除 IPC 外,TitleBar 可降级(浏览器无标题栏正常),目录选择器需 UX 重设计(浏览器拿不到绝对路径)。

### 1.6 现状盘点结论

| 维度 | 现状 | daemon 化友好度 |
|---|---|---|
| IPC 表面规模 | 79 命令 + 10 事件 | **大**,但已收敛 |
| emit 抽象 | `AppHandleSink` 覆盖 7/10 事件,**6 处直调散点** | ⚠️ 部分抽象,Phase 1 需收散点 |
| 错误协议 | `AppCommandError` 已 JSON-ready | ✅ 跨进程零改造 |
| 前端耦合 | 21 非测试文件直 import Tauri API(另 22 测试文件) | ❌ 需要抽 transport 层 |
| 共享状态 | `Arc<AppState>` 集中 | ✅ daemon 独占即可 |
| HTTP/WS 代码 | **完全没有** | 从零起步 |
| 平台 API 依赖 | TitleBar(可降级)+ 目录选择器(需 UX 重设计) | 中,`pick_project_dir` 是硬点 |

---

## 2. 外部对标:同类项目怎么做

### 2.1 Claude Code Remote Control(Anthropic 官方)

**拓扑选择**:不暴露本地端口,**所有流量经 Anthropic API over TLS 中转**([官方文档](https://code.claude.com/docs/remote-control))。

**四种拓扑**([claude-code-from-source.com ch16](https://claude-code-from-source.com/ch16-remote/)):
1. Local CLI ↔ claude.ai(同机浏览器)
2. Local CLI ↔ Mobile app(跨设备)
3. Local CLI ↔ 另一台机器的 CLI
4. Cloud Execution(完全云端)

**关键设计模式**(对我们最有启发):

- **读写不对称**:远程客户端默认**只读**(能看流、看 diff、看工具结果),写操作(批准 permission、回答 ask_user_question、取消、改 mode)需要**显式 grant**。这正好对应我们的 `permission:ask` / `tool:question` / `mode:change:request` / `task:state:transition:request` 四类 emit 事件——它们是天然需要远程 round-trip 的点。
- **FlushGate**:敏感操作累积到阈值才批量同步,避免高频 round-trip。对应我们的 `chat-event` delta 流(高频,不该走远程 grant 路径)。
- **BoundedUUIDSet**:用 LRU + UUID 集合防重放/去重。对应我们的 `streamController.ts` LRU 20。
- **JWT epoch**:session token 带 epoch,server 端可批量作废一批 token(用于"我换设备了,踢掉旧的")。
- **传输协议 transport-agnostic**([claude-world.com s13](https://claude-world.com/tutorials/s13-control-protocol/)):本地走 stdin/stdout,远程走 HTTPS。同一份控制协议,不同传输。

**启发**:
- 不要让远程客户端承担高频流式 delta —— 用"摘要 + 拉取"模式,或者只对低频事件做远程 round-trip。
- 协议设计要 transport-agnostic,这与我们 [ARCHITECTURE §5](./ARCHITECTURE.md#5-决策channel-adapter-抽象为多入口铺路) 的 Channel trait 思路一致。

### 2.2 opencode(`sst/opencode`)

**拓扑**:`opencode serve` 启动 headless HTTP server,暴露 OpenAPI 端点 + WebSocket for 事件流。客户端(TUI / SDK / IDE 插件)通过 HTTP 连接([opencode.ai/docs/server](https://opencode.ai/docs/server/))。

**关键设计**:
- **HTTP + SSE/WS 混合**:CRUD 走 REST,事件流走 SSE 或 WebSocket。
- **多客户端并发**:server 是单点权威,多个 client 连同一个 server(与 Claude Code 不同,opencode 不强制走云)。
- **JSON-RPC over HTTP**([changelog](https://opencode.ai/changelog)):plugin client 复用 active server 而非默认端口。
- **SDK 抽象**([opencode.ai/docs/sdk](https://opencode.ai/docs/sdk/)):type-safe client,前后端类型共享。

**启发**:
- HTTP server + WebSocket 事件流是**自托管**最直接的方案,不需要云中转。
- opencode 的 server/client 分离 + 类型共享 SDK,正是我们 transport 抽象的目标形态。

### 2.3 Cursor / Cline / Windsurf

这些是 **IDE 内插件**(VS Code extension),agent core 跑在 extension host 进程,与编辑器同进程,不涉及远程化。对我们的"网页访问"诉求参考价值有限。它们的 API 接入方式(Override Base URL / 环境变量 / OpenAI-compatible)是 **LLM provider 层**的事,与 transport 层无关。

### 2.4 对标小结

| 项目 | 拓扑 | 云中转? | 传输 | 对我们的意义 |
|---|---|---|---|---|
| Claude Code | local CLI + 云中转 | ✅ 强制 | HTTPS + TLS | 读写不对称 / FlushGate / transport-agnostic 设计模式 |
| opencode | self-hosted HTTP server | ❌ | HTTP + SSE/WS | 最接近我们目标,可借鉴 server/client 分离 |
| Cursor/Cline | IDE 插件同进程 | N/A | VS Code extension API | 不参考 |

**结论**:**opencode 模式是我们的主参照**(自托管 HTTP server + 多 client),Claude Code 的设计模式(读写不对称、FlushGate)作为协议层借鉴。

---

## 3. 技术选型

### 3.1 传输协议

#### 3.1a 候选对比

| 协议 | 方向 | 复杂度 | 流式 | 浏览器支持 | 适合场景 |
|---|---|---|---|---|---|
| **SSE** (Server-Sent Events) | 单向(server→client) | 低 | ✅ 原生 | ✅ `EventSource` | token 流、事件推送 |
| **WebSocket** | 双向 | 中 | ✅ 原生 | ✅ `WebSocket` | 高频双向、ask round-trip |
| **HTTP/2 streaming** | 双向(stream) | 高 | ✅ | ✅(fetch + ReadableStream) | 多路复用、头部压缩 |
| **gRPC** | 双向 | 高 | ✅ | ❌(需 gRPC-Web proxy) | 强类型、内部服务 |
| **long polling** | 模拟双向 | 低 | ❌ | ✅ | 兼容性兜底 |

#### 3.1b 行业共识(2025-2026)

[BuildMVPFast](https://www.buildmvpfast.com/blog/streaming-llm-responses-sse-vs-websockets-2026):**所有主流 LLM provider 都选 SSE**(OpenAI / Anthropic / Google),因为:
- 流式响应本质是单向(server→client),WebSocket 的双向能力被浪费
- SSE 走 HTTP,自带重连、缓存、代理友好
- WebSocket 在企业代理/NAT 下更难穿

[karls.io 案例](https://www.karls.io/ai-agent-progress-chat-websocket-server-sent-events/):NestJS agent 进度推送最终选 SSE,理由"低延迟够用、资源占用低、实现简单"。

#### 3.1c 我们的诉求分析

我们有**两类流量**:

1. **高频单向流**:`chat-event` delta(每个 token 一条)、`tool:call`、`tool:result`、`subagent:event`
   → **SSE 完美适配**
2. **低频双向 round-trip**:`permission:ask` → 用户答 → `permission_response`;`tool:question` → `resolve_tool_question`;`mode:change:request` → `resolve_mode_change`;`task:state:transition:request` → `resolve_task_state_transition`
   → 这几条是"server 主动问 client,client 回答"模式

对第 2 类,有三个方案:
- **(a) SSE + HTTP POST 回答**:server 经 SSE 推问,client 经普通 POST 回答。**最简单,无需 WebSocket**。
- **(b) WebSocket 双通道**:一条 WS 既推又收。多一个连接管理复杂度。
- **(c) 第二条 SSE**(client→server 反向):SSE 不支持反向,pass。

**推荐 (a) SSE + HTTP POST**——契合 Claude Code 的"读写不对称",实现最简,浏览器原生支持。

#### 3.1d 推荐

| 流量类型 | 协议 | 端点形态 |
|---|---|---|
| **CRUD**(79 个 command 的大部分) | HTTP POST/GET,JSON body/response | `POST /api/sessions/create` 等 |
| **高频事件流** | SSE | `GET /api/stream/{session_id}`(订阅) |
| **低频 round-trip 回答** | HTTP POST | `POST /api/permission/respond` 等 |

**为什么不上 WebSocket**:对我们这个规模(单用户、单 daemon),WebSocket 的双向能力用不满,徒增连接管理、重连、心跳复杂度。SSE + POST 已覆盖全部需求,且浏览器原生 `EventSource` 自动重连。**未来如果加飞书 channel / 多客户端并发编辑,再评估 WebSocket**。

> 注:opencode 用了 WebSocket,因为它从一开始就考虑多 client 并发 + plugin 系统。我们当前规模不到那个量级。

#### 3.1e SSE 的 backpressure / 缓冲风险(2026-07-20 review 补充)

SSE 走 HTTP/1.1 chunked transfer,**server→client 单向,server 无法直接知道 client 处理速度**。LLM 流极快时(Claude Sonnet 4.5 + cache hit 可达 60-80 tok/s),daemon 端如果不做显式流控,缓冲区会膨胀。

**具体风险**:
1. `HttpSseSink` 收到 `chat_event` → 写入 `tokio::sync::mpsc::Sender`
2. 若 mpsc buffer 满且用 `await send`(阻塞),会反压 agent loop——**这可能拖慢整个 agent 循环**
3. 若用 `try_send`(丢),前端会看到 token 流断裂
4. 前端 `EventSource` 处理不过来时,浏览器自己有缓冲(Chrome 实测 ~6MB / connection,Firefox ~1MB)
5. **客户端掉线检测**:SSE 无 ack 机制,daemon 不知道 client 掉线,要靠 TCP keepalive 或定期 SSE comment frame(`: ping\n\n`)心跳

**Phase 2 实施时的设计要点**(具体数字留待实施时定):
- mpsc buffer 用 **bounded**(如 64-256),`await send` 反压 agent loop——这是**可接受的**,因为前端真慢的时候本来就该慢,不能丢 token
- daemon 端定期发 SSE comment frame 心跳(`: ping\n\n` 每 15-30s),前端 `EventSource` 自动忽略但 TCP 层确认连接存活
- 客户端掉线时,SSE write 会报错,daemon 清理该 session 的 active request
- 不要照搬"Vercel AI SDK 20ms tick"这类未验证的数字,实施时实测

**与 WebSocket 的对比补一句**:opencode 选 WS 部分原因是它有 native flow control(可 pause/resume);我们单用户场景下,SSE + bounded buffer + 心跳已经够用,不值得为此上 WS。

### 3.2 认证与配对

#### 3.2a 威胁模型(先明确要防什么)

| 威胁 | 场景 | 严重度 |
|---|---|---|
| 未授权读 | 看到代码、session 历史、API key 配置 | 🔴 高(代码泄露) |
| 未授权写 | 注入命令、批准 permission、改文件 | 🔴 极高(RCE 等价) |
| 中间人 | 窃听/篡改流量 | 🔴 极高 |
| 重放 | 截获 token 后重发 | 🟡 中 |
| 蠕虫(配对码被暴破) | 短码在线暴破 | 🟡 中 |

**底线**:agent 能执行任意 shell + 改任意文件,**未授权访问 = RCE**。认证必须强。

#### 3.2b 方案对比

| 方案 | 启动成本 | 客户端支持 | 撤销能力 | 适用拓扑 |
|---|---|---|---|---|
| **mTLS**(双向证书) | 高(自建 CA + 证书分发) | ❌ 浏览器不支持用户证书 UI | 强(吊销证书) | LAN / VPN 内 |
| **Bearer token**(长随机 token) | 低(配对时生成) | ✅ 全部 | 中(改 token) | 全部 |
| **配对码 + 派生 token** | 中(QR / 6 位码) | ✅ 全部 | 中 | 跨网络首次配对 |
| **HTTP Basic** | 低 | ✅ | 弱 | ❌ 不推荐(明文) |
| **JWT** | 中 | ✅ | 强(epoch 作废) | 多设备 |

[apalrd.net mTLS 指南](https://www.apalrd.net/posts/2024/network_mtls/) / [OneUptime mTLS](https://oneuptime.com/blog/post/2026-02-20-mtls-service-authentication/view/) 的共识:**mTLS 是 service-to-service 的金标准,但客户端支持差**(浏览器对用户证书的 UI 几乎不存在,[r/selfhosted 讨论](https://www.reddit.com/r/selfhosted/comments/1hzdk33/why_is_mtlsclient_cert_authentication_not_more/))。

**关键约束**:我们要支持**真浏览器**访问 → mTLS 出局(浏览器不支持)。Bearer token + 配对码是唯一可行的强认证方案。

#### 3.2c 推荐方案:配对码 → 派生 Bearer token

**配对流程**(借鉴 Claude Code 的 device pairing + Tailscale 的配对码):

```
┌─────────────┐                    ┌─────────────┐
│  Browser    │                    │  Daemon     │
│ (新设备)    │                    │ (已运行)    │
└──────┬──────┘                    └──────┬──────┘
       │                                  │
       │  1. 用户在已配对设备点"添加新设备"│
       │                                  │
       │  2. Daemon 生成 6 位配对码 +     │
       │     pending_request_id,显示 QR  │
       │     (有效期 5 分钟,单次)        │
       │                                  │
       │  3. 用户在浏览器打开网页,       │
       │     手输配对码或扫 QR            │
       │                                  │
       │  4. POST /api/pair { code }      │
       │ ────────────────────────────────>│
       │                                  │
       │  5. Daemon 校验 code,生成       │
       │     device_token(32 字节随机)   │
       │     + device_id + device_name,   │
       │     写入 SQLite devices 表       │
       │                                  │
       │  6. 200 { device_token, expires }│
       │ <────────────────────────────────│
       │                                  │
       │  7. Browser 存 token 到          │
       │     localStorage / IndexedDB     │
       │                                  │
       │  8. 后续请求带                    │
       │     Authorization: Bearer <token>│
       │ ────────────────────────────────>│
       │                                  │
```

**设计要点**:
- **配对码 6 位 + 5 分钟过期 + 单次使用**——在线暴破窗口小(100 万种组合 / 5 分钟,即便无 rate limit 也难暴破;加 rate limit 3 次/IP 后基本无风险)
- **token 32 字节随机**(`OsRng`),不可猜测
- **token 存 SQLite `devices` 表**(新表),可单条撤销
- **token 带 epoch**(借鉴 Claude Code):改 epoch 即可批量作废所有旧 token("我换设备了")
- **HTTPS 强制**:token 不走明文 HTTP(见 §3.3)

**降级方案(LAN 直连)**:
- 同机/同 LAN 可关 HTTPS,用配对码 + token over HTTP(配对码本身是一次性密钥,劫持窗口小)
- 但**生产环境强制 HTTPS**

### 3.3 网络拓扑(三种部署形态)

#### 3.3a 形态 A:本地直连(LAN / 同机 / **WSL**)

```
Browser ──(HTTP, localhost/LAN)──> Daemon
```

- daemon 监听 `127.0.0.1:PORT`(同机)或 `0.0.0.0:PORT`(LAN)
- HTTPS 可选(LAN 内可信时)
- 配对:同屏 QR 或手动输码
- **最简单,首发版本目标**

**WSL 2 浏览器访问链路**(本项目 WSL-first 的核心部署路径,2026-07-20 review 补充):

> 项目是 WSL-first 设计,典型场景是 **daemon 跑在 WSL 2 内**(操作 WSL 内的代码),**用户在 Windows 宿主打开浏览器**(Chrome/Edge)访问。
>
> - **WSL 2 默认启用 localhost forwarding**:Windows 宿主访问 `http://localhost:PORT` 会自动转发到 WSL 2 内监听该端口的进程([微软官方文档](https://learn.microsoft.com/en-us/windows/wsl/networking))。这是默认行为,通常无需额外配置。
> - **daemon 监听地址**:为支持 WSL→Windows 宿主访问,daemon 应监听 `0.0.0.0:PORT` 或 `127.0.0.1:PORT`(WSL 2 的 localhost forwarding 两种都支持,但 `0.0.0.0` 更保险,兼容 LAN 场景)。
> - **Phase 2 第一验证关**:实施时第一步就是 `curl http://localhost:PORT/api/health` 从 **Windows 宿主**确认连通性。如果 localhost forwarding 不可用(罕见,通常是 Windows 防火墙或 WSL 版本问题),降级方案:
>   - 用 WSL 2 虚拟 IP(`ip addr show eth0` 拿到 `172.x.x.x`),Windows 浏览器访问 `http://172.x.x.x:PORT`
>   - 或用 `netsh interface portproxy` 手动配端口转发
> - **WSLg vs 网络**:`pnpm tauri dev` 现在能工作是因为 Tauri 走 WSLg 窗口转发(与网络无关),daemon 化后浏览器访问走的是**网络层**,是不同的转发机制,必须单独验证。

#### 3.3b 形态 B:远程配对(跨网络,经中转)

两个子方案:

**B1. 用户已有 VPS**(BACKLOG §7 提到的方向):
```
Browser ──HTTPS──> VPS daemon ──(本地 agent)
                     (用户 VPS)
```
- 用户把 daemon 跑在自己的 VPS上,agent 在 VPS 上操作代码
- 浏览器直连 VPS IP / 域名
- **优点**:简单直接,无中转
- **缺点**:VPS 要装 Rust + 所有依赖;代码得在 VPS 上(或 daemon 远程操作本机,复杂)

**B2. Tailscale / Cloudflare Tunnel**(本机 daemon + 穿透):
```
Browser ──HTTPS──> Cloudflare/Tailscale ──> 本机 Daemon
                                                 │
                                                 └> 操作本地代码
```

[Tailscale vs Cloudflare Tunnel 对比](https://tailscale.com/compare/cloudflare-access):
| 维度 | Tailscale Funnel | Cloudflare Tunnel |
|---|---|---|
| 架构 | P2P mesh VPN,设备直连 | 经 Cloudflare 边缘代理 |
| 公网暴露 | Funnel = 开放到公网(类 port forwarding) | 藏在 CF 后面 + WAF |
| 客户端要求 | 需装 Tailscale 客户端 | 浏览器即可 |
| 免费额度 | 设备/用户数限制 | 隧道 generous |
| 适合 | 自己设备间互访 | 公开访问自托管服务 |

**推荐 B2 + Cloudflare Tunnel**:
- 浏览器零配置(不需装客户端)
- 自带 HTTPS + WAF + DDoS 防护
- 用户本机跑 daemon,CF Tunnel 把 `agent.mydomain.com` 反向代理到 `localhost:PORT`
- 代码留在本机,daemon 操作本地代码(符合 WSL-first 设计)
- **配对码仍需**:CF Tunnel 不做应用层认证,只做传输层

#### 3.3c 形态 C:Electron 桌面 app

- Electron 主进程 = thin client,内置 Vue 前端
- 通过同一套 HTTP/SSE 协议连 daemon(可以是本机或远程)
- **本质等同浏览器 client**,只是多了个壳(可装、可托盘、可原生通知)
- 与 Tauri 版并存:Tauri 版仍走 in-process IPC(快),Electron 版走 HTTP transport(远程)

#### 3.3d 推荐演进顺序

> 📌 这里的"阶段"指**网络拓扑形态**的演进,与 §4 的改造阶段(Phase 1/2/3)是不同维度。形态 A 对应 §4 Phase 2,形态 B2 对应 §4 Phase 3(已定为远期)。

```
近期:形态 A(本地直连 / 同 LAN,HTTP 或自签 HTTPS)
       ↓ 验证 transport 抽象 + HTTP/SSE 协议正确性
远期:形态 B2(Cloudflare Tunnel,HTTPS + 配对码)
       ↓ 解锁跨设备远程访问(Phase 3 远期规划)
可选:形态 C(Electron thin client)
       ↓ 复用 transport,加壳
```

形态 B1(VPS daemon)作为**可选**,看用户是否需要"代码在云端"的形态。本项目 WSL-first 设计倾向于代码留本机,所以 B2 优先。

### 3.4 进程管理

#### 3.4a daemon 生命周期需求

- **自动启动**:开机/登录时拉起
- **崩溃恢复**:挂了自动重启
- **优雅退出**:收到信号后 drain in-flight 请求,再退出(尤其 agent loop 不能硬杀)
- **日志收集**:stdout/stderr 落盘

#### 3.4b 方案对比

| 方案 | 平台 | 复杂度 | 自动重启 | 推荐度 |
|---|---|---|---|---|
| **systemd** | Linux | 低(unit 文件几十行) | ✅ `Restart=on-failure` | ✅ WSL/Linux 首选 |
| **launchd** | macOS | 中(plist) | ✅ `KeepAlive` | ✅ macOS 首选 |
| **Windows Service / sc.exe** | Windows | 中 | ✅ | ✅ Windows 首选 |
| **supervisord** | 跨平台(Python) | 中 | ✅ | ⚠️ 多一层 Python 依赖 |
| **pm2** | 跨平台(Node) | 中 | ✅ | ❌ 引入 Node runtime,不推荐 |
| **自研 supervisor** | 全平台 | 高 | 需自己写 | ❌ over-engineering |

[ARCHITECTURE §4](./ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路) 已经定了:**自研 daemon,不用 pm2/supervisord**——"进程就一个,行为可预测,systemd unit 几十行就够"。

#### 3.4c 推荐实现

**Rust 端关键模式**(来自 [Tokio graceful shutdown](https://tokio.rs/tokio/topics/shutdown) + [OneUptime 指南](https://oneuptime.com/blog/post/2026-01-25-graceful-shutdown-rust-services/view)):

1. **不 daemonize**,前台跑,交给 systemd 管理(`Type=simple` 或 `Type=notify`)
2. **必装 SIGTERM handler**——Tokio 默认不装,systemd 的 `KillSignal=SIGTERM` 会硬杀
3. **`CancellationToken`**(tokio-util)协调退出:`signal::unix::signal(SignalKind::terminate())` 触发 cancel,所有 task select! 监听
4. **`TimeoutStopSec=30s`** 给足 drain 时间(agent loop 一轮可能跑很久,但 cancel 后应快速退出)
5. **`KillMode=mixed`**——主进程 SIGTERM,子进程(shell tool spawn 的)不被立即杀
6. **`sd_notify(READY=1)`**(可选,用 [sd-notify crate](https://blog.dend.ro/introducing-sd-notify/))——告诉 systemd 何时 ready

**WSL 特殊性**:WSL 2 默认不带 systemd(可手动开启 `systemd=true`),需要给 WSL 用户提供**手动启动脚本**作为降级方案(类似现在的 `pnpm tauri dev`)。

#### 3.4d daemon 启动入口设计

```
src-tauri/src/
├── lib.rs                  # Tauri 入口(保留,GUI 进程)
├── main.rs                 # Windows 子系统入口(GUI)
├── daemon/                 # ★ NEW
│   ├── mod.rs              # daemon main 入口
│   ├── server.rs           # HTTP server(axum)
│   ├── routes/             # 79 个 command → HTTP handler
│   ├── sse.rs              # SSE 事件流 endpoint
│   ├── auth.rs             # 配对码 + token 校验
│   └── transport.rs        # 实现 ChatEventSink trait(emit → SSE push)
└── bin/
    └── everlasting-daemon.rs  # ★ NEW cargo bin(daemon 可执行文件)
```

daemon 是独立的 `cargo bin`,与 Tauri app 共享 `src-tauri` 的 agent core / db / tools 等模块,**只换最外层入口**。

---

## 4. 改造路径设计

### 4.1 总体策略:近期两阶段 + 远期规划

```
                    Phase 0          Phase 1           Phase 2           Phase 3 (远期)
                    (现状)           (transport 抽象)  (daemon 拆分)     (认证 + 远程)
                    ────────         ──────────────    ──────────────    ──────────────
进程拓扑:           单进程           单进程             双进程(GUI+daemon) 双进程 + 远程
前端 transport:     Tauri 直连       Transport iface   Transport iface   Transport iface
                                     ├ TauriTransport  ├ TauriTransport  ├ TauriTransport
                                     └ (HttpTransport  └ HttpTransport  └ HttpTransport
                                        stub)            (loopback)        (远程 + auth)
后端 IPC:           #[command]       #[command]        #[command]        HTTP handlers
                                                    + daemon routes
emit 通道:          app.emit          app.emit          SSE (loopback)    SSE (远程)
认证:               无                无                无                配对码 + token
Tauri 版可用:       ✅                ✅                ✅(连本机 daemon)  ✅
浏览器可用:          ❌                ❌                ✅(同机)          ✅(远程)
回归风险:           —                 低(零行为变化)    中(双进程协调)    中(加认证)
```

**核心原则**:**每个阶段都能独立交付价值,且 Tauri 版始终保持可用**。

> 📌 **Phase 3(认证 + 跨设备远程)定为远期规划**。近期交付聚焦 Phase 1 + Phase 2:先让 Tauri 版无缝切到 transport 接口、再让本机浏览器能访问 daemon。跨网络远程访问涉及配对码、HTTPS、Cloudflare Tunnel、读写不对称等一整套认证/部署工作,放到本机访问跑通、协议稳定后再启动——届时 Phase 1/2 沉淀的 transport 抽象和 HTTP/SSE 协议会自然成为它的底座,无需返工。

### 4.2 Phase 1:transport 抽象(前端)+ emit 散点收敛(后端)

**目标**:把 Tauri IPC 收敛到接口背后,**行为零变化**,为后续换 transport 铺路。

**前端**——新增 `app/src/transport/`:

```typescript
// app/src/transport/types.ts
export interface Transport {
  // 对应 invoke
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  // 对应 listen,返回 unlisten
  listen<T>(event: string, handler: (payload: T) => void): Promise<() => void>;
}

// app/src/transport/tauri.ts
export const tauriTransport: Transport = {
  invoke: (cmd, args) => invoke(cmd, args),
  listen: (event, handler) => listen(event, (e) => handler(e.payload as T)),
};

// app/src/transport/http.ts (Phase 2 填充)
// 关键:httpTransport 内部维护单个 EventSource + 事件分发表,
// 对外保持 listen(event, handler) 签名不变——错位在 httpTransport 内部消化,
// Tauri 端的 streamController "按 requestId 分发"逻辑零改动。
export const httpTransport: Transport = { /* stub */ };

// app/src/transport/index.ts
export const transport = isTauri() ? tauriTransport : httpTransport;
```

> ⚠️ **listen 接口设计**(2026-07-20 review 补充):Tauri 是全局广播(`listen("chat-event")` 收所有 session),SSE 按 session 订阅(`EventSource("/api/stream/{session_id}")`)。**两种语义不对称**。MiniMax review 建议改 `subscribe(sessionId, handler)`,但那会破坏 streamController 现有的 requestId 分发逻辑。**采纳 DeepSeek 的方案 B**:httpTransport 内部维护单个全局 SSE 连接 + 事件名→handler 分发表,对外保持 `listen(event, handler)` 签名不变。这样 Tauri 端零改动,错位在 httpTransport 内部消化。

**前端改造工作量**:21 个非测试文件的 `import { invoke/listen } from '@tauri-apps/...'` 改成 `import { transport } from '@/transport'`(13 stores + 5 utils + 4 components - 1 TitleBar 只改 window/os)。**机械替换,无逻辑变化**。另有 22 个 `*.test.ts` 文件需同步改 mock(vitest `vi.mock('@/transport', ...)`)。

**后端**——⚠️ **不是零改动**(2026-07-20 review 修正):

> 早期草稿说"Phase 1 后端零改动"——错。实测有 **6 处 emit 散点**绕过 `AppHandleSink`(见 §1.2b)。Phase 1 要把这 6 处收口,否则 Phase 2 换 transport 时会漏。
>
> 工作内容:
> 1. `AppHandleSink` trait 保留(已实现 7 事件)
> 2. 收 `agent/chat.rs:187` pre-flight error 直调 → 走 sink
> 3. 收 `agent/helpers.rs:160,170` 的 `emit_chat_event` / `emit_tool_result` → 走 sink(疑似早期遗留,该删函数直接用 sink)
> 4. `agent/subagent/sink.rs:279,698` + `dispatch.rs:1192` 的 subagent 事件 → **保留 collector 双通道语义**,但把 `tauri::AppHandle` 类型抽象成 `dyn SubagentEventSink` trait,让 daemon 化时能换实现
> 5. `state.rs:317` `projects:refreshed` 直调 → 走一个轻量 emit 抽象(它在 `AppState::load` 后台任务里,不在 agent loop)
>
> **结论**:Phase 1 是"前端机械替换 + 后端收 6 处散点",后端不是零改动,但工作量可控(主要是类型抽象,不改业务逻辑)。


**验证**:全套 vitest + `pnpm tauri dev` 跑通,所有现有测试不改动。

**这一步的价值**:解锁后续所有阶段,且本身不引入任何风险。

### 4.3 Phase 2:daemon 拆分 + 本地 HTTP server

**目标**:把 agent core 拆到独立 daemon 进程,本机浏览器可访问。

**步骤**:

1. **抽 daemon main**(`src-tauri/src/bin/everlasting-daemon.rs`):
   - 复用 `AppState::load` 逻辑(**去掉 `AppHandle` 依赖**,改用 `dirs` crate 取 `data_dir()` / `home_dir()`)——⚠️ 必须验证两种路径产出一致,否则 daemon 读写的是另一个 SQLite 文件
   - 启动 axum HTTP server,监听 `0.0.0.0:PORT`(支持 WSL→Windows 宿主转发,见 §3.3a)
   - 注册 79 个 command → HTTP handler(机械映射)
   - 实现 `HttpSseSink`:emit 事件 → SSE 推送;维护 `session_id → Vec<SseSender>` 路由表,按 request_id→session_id 分发

2. **HTTP handler 映射规则**(示例):
   ```
   invoke('chat', {requestId, sessionId, messages}) 
     → POST /api/chat {requestId, sessionId, messages}   (body 字段保持现有 snake_case,与 AppCommandError 一致)
   
   listen('chat-event') 
     → GET /api/stream/{sessionId}  (SSE;httpTransport 内部开一个全局连接,按 event 字段分发)
   
   invoke('permission_response', {rid, decision})
     → POST /api/permission/respond {rid, decision}
   ```

3. **协议 schema**:用 serde + TypeScript codegen(`ts-rs`,与现有 snake_case 类型一致)保证前后端类型一致,避免手写 drift。需验证 `ts-rs` 能覆盖 `ChatEvent` 这种 `#[serde(tag = "type")]` 的内部 tagged enum。

4. **前端 `httpTransport` 填充**(方案 B,见 §4.2):
   - `invoke` → `fetch('/api/...', {method, body, headers})`
   - `listen` → httpTransport 内部维护**单个全局 `EventSource('/api/stream/all')`**,收到事件后按 event 名分发给已注册的 handler;对外保持 `listen(event, handler)` 签名不变

5. **⚠️ GUI 进程改为 thin client,且不开本地 db pool**(2026-07-20 review 修正):
   - **决策:GUI 切 httpTransport 后,所有数据操作走 HTTP RPC,不再开本地 SqlitePool**
   - 理由:SQLite 不适合多进程写,GUI 和 daemon 同时开 pool 会触发 `SQLITE_BUSY`(即便都只读也有 lock 竞争)。**彻底消除 dual-pool 风险**的做法是 GUI 完全不开 db,所有读写经 daemon。
   - GUI 进程不再调 `AppState::load`(或调一个"瘦版"只拿 transport 句柄,不拿 db/catalog/cancellations)
   - `pnpm tauri dev` 仍可用:Tauri app 启动时自动 spawn daemon 子进程,关闭时清理;连不上 daemon 则提示用户

6. **⚠️ daemon 内嵌前端静态文件 server**(2026-07-20 review 补充):
   - daemon 用 `tower-http::services::ServeDir` 指向 `app/dist/`,实现**单二进制部署**
   - 浏览器访问 `http://localhost:PORT` 同时拿到前端 HTML/JS + API(同源,免 CORS)
   - 开发模式:`pnpm tauri dev` 时前端走 Vite dev server,daemon 不 serve 静态文件;生产模式 daemon 内嵌

7. **⚠️ oneshot → HTTP handler 的跨进程转换**(2026-07-20 review 补充,Phase 2 核心难点):

   以 `permission:ask` 为例,当前流程(单进程):
   ```
   agent loop → permission_store.request_permission() → 创建 oneshot channel + 注册 rid→sender
              → AppHandleSink.emit("permission:ask") → 前端 modal
   用户点"允许" → invoke("permission_response", {rid, decision})
              → permission_store.resolve(rid, decision) → oneshot sender 发送 → agent loop 收到
   ```

   daemon 化后(跨进程):
   ```
   agent loop(daemon 内) → permission_store.request_permission() → oneshot + 注册 rid→sender
                        → HttpSseSink.emit("permission:ask") → SSE 推到浏览器
   用户点"允许" → fetch('POST /api/permission/respond', {rid, decision})
              → axum handler(Extension<Arc<AppState>>) → state.permission_store.resolve(rid, decision)
              → oneshot sender 发送 → agent loop(daemon 内)收到
   ```

   **关键转换点**:axum handler 通过 `Extension<Arc<AppState>>` 拿到 `permission_store`(与 Tauri command 的 `State<Arc<AppState>>` 是同一份 state),调 `resolve(rid, decision)` 命中 daemon 进程内的 oneshot sender。**oneshot 本身不跨进程**——它始终在 daemon 内,只是触发它的"用户回答"从 Tauri invoke 变成了 HTTP POST。

   **适用范围**:这个模式覆盖所有 4 类 round-trip(`permission:ask` / `tool:question` / `mode:change:request` / `task:state:transition:request`),共用 `permission_store` / `question_store` 的 oneshot 注册表。Phase 2 详细设计时把这 4 类的序列图画全,作为参考实现。

8. **边界情况:客户端断开**(2026-07-20 review 补充):
   - 用户关浏览器标签页时,`cancel_chat` 请求发不出去,daemon 的 agent loop 会继续跑
   - daemon 端 `HttpSseSink` 检测 SSE 连接断开(write 报错)→ 触发该 session active request 的 cancel + 标记 idle
   - 非 MVP 必须,但标为已知边界情况

**关键决策点**:
- **GUI 进程是否保留内嵌 agent core(双模式)?** Phase 2 **先做单模式**(GUI 全走 httpTransport,agent 只在 daemon)。双模式(GUI 内嵌 agent core 作为离线 fallback)作为 Phase 2 后期的增量,前置条件是先把"dual-pool 写竞争"解决(GUI db readonly 或全走 RPC)——本步骤 5 已选"全走 RPC",自然消除该竞争。

**这一步的价值**:本机浏览器可访问(含 WSL→Windows 宿主),Tauri 版仍可用,agent core 在 daemon 里长跑不被 GUI 重启打断。


### 4.4 Phase 3(远期规划):认证 + 跨设备远程访问

> 📌 **本阶段定为远期规划**,不在近期交付范围内。前置条件:Phase 2 本机浏览器访问跑通、HTTP/SSE 协议经实际使用稳定。下面是设计草稿,留待启动时细化。

**目标**:跨网络安全访问。

**步骤**:

1. **配对码流程实现**(§3.2c):
   - SQLite 新表 `devices(device_id, device_token, device_name, created_at, last_seen, epoch)`
   - `POST /api/pair {code}` → 返回 token
   - 中间件:`Authorization: Bearer <token>` 校验

2. **HTTPS**:
   - 形态 A(LAN):自签证书或关 HTTPS(可信网络)
   - 形态 B2(远程):Cloudflare Tunnel 自动给 HTTPS,或 Caddy 自动 Let's Encrypt

3. **Cloudflare Tunnel 配置文档**(用户手册):
   ```
   cloudflared tunnel create everlasting
   cloudflared tunnel route dns everlasting agent.yourdomain.com
   # config.yml 指向 localhost:PORT
   ```

4. **读写不对称**(借鉴 Claude Code):
   - 远程 client 默认只读(看流、看 diff)
   - 写操作(批准 permission、回答 question、改 mode、cancel、改文件)需要**额外确认**(可以是配置开关,默认本地无需确认、远程需要)

**这一步的价值**(启动后):跨设备远程访问,真正达成"远程配对"诉求。但近期 ROI 不高——本机/同 LAN 访问已经覆盖大部分日常使用场景。

### 4.5 Phase 4(可选):Electron thin client

**目标**:复用 Phase 2/3 的 transport,加个 Electron 壳。

- Electron 主进程 = 浏览器 + 原生能力(托盘、通知、自动启动)
- 渲染进程 = 同一份 Vue 前端(已有 `app/src/`)
- transport 默认用 `httpTransport`(连本机或远程 daemon)

**与 Tauri 的取舍**:
| 维度 | Tauri | Electron |
|---|---|---|
| 包大小 | ~10 MB | ~100 MB |
| 内存 | 低 | 高(Chromium) |
| 原生能力 | 强(Rust) | 中(Node) |
| 浏览器复用 | ❌(webview 私有) | ✅(Chromium 同核) |
| 维护成本 | 已有 | 新增 |

**建议**:除非有强烈的"原生通知/托盘/自动更新"需求,**Tauri + Web 浏览器访问** 已覆盖全部场景,Electron 是 nice-to-have。

---

## 5. 风险与未决问题

### 5.1 已识别风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| **协议 drift**(79 命令手写易错) | 高 | `ts-rs` / `typeshare` 自动生成 TS 类型 |
| **DB 并发**(多 client 同时写) | 高 | daemon 独占 DB,client 只走 RPC(SQLite 不适合多进程写) |
| **agent loop 跨进程 cancel** | 中 | cancel 必须命中 daemon 的 CancellationToken map —— HTTP `cancel_chat` 直达 daemon |
| **后台 shell 跨进程查询** | 中 | `BackgroundShellRegistry` daemon 独占,client `shell_status` 走 RPC |
| **transport 抽象过早 overdesign** | 低 | ARCHITECTURE §5 自己标注的风险;但 Phase 1 只抽最小接口,不上 Channel trait 满集 |
| **远程延迟** | 中 | 高频 delta 流可考虑批量(FlushGate);低频 round-trip 可接受 |
| **认证 bypass** | 极高 | 配对码强 rate limit + token 32 字节随机 + HTTPS 强制 |
| **worktree / git 操作跨进程** | 中 | libgit2 不是线程安全的多进程友好,daemon 独占 |

### 5.2 未决问题(需进一步决策)

1. **GUI 进程是否保留内嵌 agent core?**
   - 选项 A:保留(双模式,daemon 没起时仍能用)——复杂度高,但离线友好
   - 选项 B:不保留(GUI 永远是 thin client)——简单,但 daemon 挂了 GUI 就废
   - **倾向 A**,但 Phase 2 先做 B(简单),Phase 3 再补 A

2. **协议格式:REST 还是 JSON-RPC?**
   - REST:URL 语义清晰,OpenAPI 文档免费
   - JSON-RPC:与 opencode 一致,单 endpoint,方法名直接对 invoke 字符串
   - **倾向 REST**(79 个命令本来就有清晰的资源划分:session / project / provider / ...)

3. **SSE 还是 WebSocket for 事件流?**
   - 本文推荐 SSE + HTTP POST(§3.1d)
   - 若未来上飞书 / 多 client 并发编辑,可能需要重评估 WebSocket
   - **倾向 SSE**,留好抽象点(`Transport.listen` 接口与具体协议无关)

4. **配对码的 UX:QR 还是手输?**
   - QR:已配对设备扫码(需摄像头)
   - 手输:6 位码,跨设备无摄像头场景
   - **倾向两者都支持**(配对码生成时同时出 QR 和明文码)

5. **多 client 并发:session 锁?**
   - 两个 client 同时往一个 session 发消息怎么办?
   - 现有 `session_active_request` map 已是单 session 单 inflight 设计
   - 跨 client 时需要明确:后发的拒绝?排队?抢占?
   - **倾向"后发拒绝,提示已在跑"**(最简,符合现状)

6. **daemon 与 GUI 的版本兼容?**
   - daemon 升级了 GUI 没升级,协议不匹配
   - **需在协议里加版本号**(`/api/v1/...`),daemon 支持多版本

### 5.3 不做(明确排除)

- **多用户**:本项目是个人工具,不考虑多租户([DESIGN.md] 明确)
- **agent core 上云**:代码留本机([BACKLOG §7] 云端只同步状态摘要,不跑 agent)
- **WebSocket**(近期 Phase 1-2 范围内):SSE 够用
- **mTLS**:浏览器不支持,出局
- **完整 Channel trait 满集**(TauriGuiChannel / FeishuChannel / CliChannel 全实现):近期只做最小 transport 抽象,Channel trait 等 Phase 3 远期或飞书真要上时再补

---

## 6. 决策建议

### 6.1 一句话结论

> **近期先抽 transport(前端)+ 收 emit 散点(后端 6 处)这层"包装通道",让 Tauri 变成 transport 的一个实现;然后 daemon 化作为第二阶段,本地 HTTP server + SSE,解锁本机浏览器访问(含 WSL→Windows 宿主)。认证配对 + 跨设备远程访问作为远期规划,等本机访问跑通、协议稳定后再启动。不要先 daemon 再做包装。**

### 6.2 推荐路线图(建议归入 ROADMAP)

| 阶段 | 定位 | 工作量估 | 交付物 | 依赖 |
|---|---|---|---|---|
| **Phase 1**:transport 抽象 + emit 散点收敛 | 🟢 近期 | 2-3 天 | 前端 `Transport` interface + `TauriTransport`(21 文件 + 22 测试文件);后端收 6 处 emit 散点 | 无 |
| **Phase 2**:daemon 拆分 + 本地 HTTP | 🟢 近期 | **2-3 周** + 0.5 周 E2E | `everlasting-daemon` bin + axum + SSE + 静态文件 server + `HttpTransport`;本机浏览器可访问(含 WSL) | Phase 1 |
| **Phase 3**:认证 + 跨设备远程 | 🔴 远期 | 1 周+ | 配对码 + token + Cloudflare Tunnel 文档 + 读写不对称 | Phase 2 协议稳定 |
| **Phase 4**:Electron(可选) | 🟡 可选 | 3-5 天 | Electron thin client | Phase 2 |

> ⚠️ **工作量修正说明**(2026-07-20 review):早期草稿估 Phase 1 = 1-2 天 / Phase 2 = 1-2 周。修正原因:
> - Phase 1 加了"后端收 6 处 emit 散点"(原以为零改动),上调为 2-3 天
> - Phase 2 加了 WSL 部署验证 / 静态文件 server / GUI 全走 RPC / oneshot 跨进程转换 / E2E harness,从 1-2 周上调为 2-3 周 + 0.5 周 E2E

### 6.3 与现有文档的衔接

- **[ARCHITECTURE §4 触发条件]**:原文"BACKLOG §6 飞书 channel 决定实施时"——本次调研后,**触发条件应改为"真浏览器远程访问诉求确定实施时(Phase 1+2)"**。跨设备远程(Phase 3)与飞书仍同属远期触发条件。
- **[ARCHITECTURE §5 Channel trait]**:近期 Phase 1-2 只做 transport 最小抽象,不上 Channel trait 满集(避免 overdesign,与 §5 风险条款一致)。
- **[ROADMAP 第四档 B10]**:建议拆为 B10a(transport 抽象,近期)/ B10b(daemon 拆分,近期)/ B10c(认证 + 跨设备远程,**仍留远期**)。B10a/B10b 因用户已明确诉求可提到第二/三档;B10c 维持第四档。
- **[BACKLOG §7 云端同步]**:本文 Phase 3 的 Cloudflare Tunnel 方案与 §7 的"VPS 自托管 daemon"是互补关系——§7 是 agent 上云,本文是 agent 留本机 + 穿透访问,后者更符合 WSL-first。两者都属于远期。

### 6.4 立即可做的第一步

**Phase 1 是低风险、高收益**(2026-07-20 review 修正:不是"零风险"):
- 不改任何运行时**行为**,但后端要收 6 处 emit 散点(类型抽象,不改业务逻辑)
- 解锁 Phase 2(daemon 拆分),并为远期 Phase 3(认证 + 远程)预留好抽象点
- 即便最终不继续推进,transport 抽象本身也让代码更清晰(21 + 22 个文件不再散落 Tauri import)

**验证标准**:
- **Phase 1**:全套 vitest 跑通(22 个测试文件 mock 改造)+ `pnpm tauri dev` 零行为变化;后端 6 处散点收敛后,`cargo test` 全绿
- **Phase 2**:上线前必须有 1 套端到端测试覆盖 10 类 SSE 事件 + 4 类 round-trip(permission/question/mode_change/task_state_transition),与 Tauri 端事件序列对拍一致;WSL→Windows 宿主浏览器访问 `curl http://localhost:PORT/api/health` 连通性验证

**建议**:先按 [REMOTE-ACCESS-ROADMAP.md](./REMOTE-ACCESS-ROADMAP.md) 的子阶段拆分启动 Phase 1。Phase 3 远期规划的设计草稿(§4.4)留作未来启动时的参考,近期不投入。

**测试策略**(2026-07-20 review 补充):
- **Phase 1**:vitest 用 `vi.mock('@/transport', () => ({ transport: mockTransport }))` 替换现有 Tauri mock;新增 `transport/tauri.test.ts` 验证 `tauriTransport` 正确转发 invoke/listen
- **Phase 2**:axum handler 单元测试(`axum::test` + `TestRequest`,覆盖 79 个 handler 的 happy path + 错误码);SSE 集成测试(mock provider 跑 1 轮 agent loop,验证事件序列);端到端测试(daemon + 浏览器 harness,见上)
- **回归**:daemon 化后,用同一套 vitest(走 httpTransport)确保 79 个 command 行为与 Tauri 版一致

---

## 附录 A:参考链接

### 同类项目
- [Claude Code Remote Control(官方)](https://code.claude.com/docs/remote-control)
- [Claude Code from Source ch16: Remote Control and Cloud Execution](https://claude-code-from-source.com/ch16-remote/)
- [Claude Code Control Protocol(transport-agnostic)](https://claude-world.com/tutorials/s13-control-protocol/)
- [Claude Code Remote Control Security Risks(Penligent)](https://www.penligent.ai/hackinglabs/fr/claude-code-remote-control-security-risks-when-a-local-session-becomes-a-remote-execution-interface/)
- [opencode Server 文档](https://opencode.ai/docs/server/)
- [opencode SDK 文档](https://opencode.ai/docs/sdk/)
- [opencode hitchhiker's guide(headless/daemon)](https://man.ilayk.com/gists/opencode/)

### 传输协议
- [SSE vs WebSockets for Streaming LLM Responses(BuildMVPFast)](https://www.buildmvpfast.com/blog/streaming-llm-responses-sse-vs-websockets-2026)
- [Choosing between WebSockets and SSE for AI Agents(LinkedIn)](https://www.linkedin.com/posts/sitalakshmi04_ai-agent-agentdevelopmentkit-activity-7352087913809006593-WAy_)
- [AI Agent Chat: WebSockets vs SSE(karls.io)](https://www.karls.io/ai-agent-progress-chat-websocket-server-sent-events/)
- [Streaming AI Responses: WebSocket / SSE / gRPC(Medium)](https://medium.com/@pranavprakash4777/streaming-ai-responses-with-websockets-sse-and-grpc-which-one-wins-a481cab403d3)
- [Server-Sent Events vs WebSockets: When to Use SSE(DevOps.dev)](https://blog.devops.dev/server-sent-events-are-the-real-time-feature-most-teams-overcomplicate-fbcafcbc35cf)

### 认证与配对
- [Securely Expose Homelab Services with mTLS(apalrd.net)](https://www.apalrd.net/posts/2024/network_mtls/)
- [How to Implement mTLS for Service-to-Service Authentication(OneUptime)](https://oneuptime.com/blog/post/2026-02-20-mtls-service-authentication/view/)
- [Why is mTLS not more common?(r/selfhosted)](https://www.reddit.com/r/selfhosted/comments/1hzdk33/why_is_mtlsclient_cert_authentication_not_more/)
- [Best-Practice Security with mTLS(CECG)](https://www.cecg.io/blog/best-practice-security-automation-operability-with-mtls/)

### 网络拓扑
- [Cloudflare vs Tailscale(官方对比)](https://tailscale.com/compare/cloudflare-access)
- [Pangolin vs Cloudflare Tunnels vs Tailscale(Contabo)](https://contabo.com/blog/pangolin-vs-cloudflare-tunnels-vs-tailscale/)
- [Tailscale vs Cloudflare Tunnel for Home Remote Access(HomeTechOps)](https://hometechops.com/guides/home-remote-access-tailscale-vs-cloudflare-tunnel)

### 进程管理 / Rust daemon
- [Tokio: Graceful Shutdown(官方)](https://tokio.rs/tokio/topics/shutdown)
- [How to Implement Graceful Shutdown in Rust Services(OneUptime)](https://oneuptime.com/blog/post/2026-01-25-graceful-shutdown-rust-services/view)
- [How to Use systemd Type=notify(OneUptime)](https://oneuptime.com/blog/post/2026-03-02-how-to-use-systemd-type-notify-for-ready-signaling-on-ubuntu/view)
- [tokio::signal::unix(Signal)API 文档](https://docs.rs/tokio/latest/tokio/signal/unix/struct.Signal.html)
- [systemd.service man page](https://www.freedesktop.org/software/systemd/man/systemd.service.html)
- [systemd.kill man page](https://www.freedesktop.org/software/systemd/man/systemd.kill.html)
- [Introducing the sd-notify crate(blog.dend.ro)](https://blog.dend.ro/introducing-sd-notify/)

### Tauri 限制
- [Tauri issue #3655: Can a Tauri App Run Inside a Browser?](https://github.com/tauri-apps/tauri/issues/3655)
- [Invoke Desktop App Through the Browser(dev.to)](https://dev.to/rain9/tauri-6-invoke-desktop-application-functionality-through-the-browser-811)
