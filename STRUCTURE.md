# STRUCTURE — 项目代码结构全景图

> **基线**:2026-06-24 commit `7f2553b`(包含 8-PR1/2/3/4 + 10 个文件 split:chat-types / subagent / chat-loop / agent-tests / permissions / subagent-runs / message-item / subagent-drawer / db-tests / chat-input)
> **来源**:融合本地 audit `.trellis/workspace/Carlos/audit-2026-06-09/04-codebase-map.md` + Opus评审 `docs/_reviews/REVIEW-claude-opus-2026-06-09.md` + 8-PR 系列实际落地状态 + 06-23/24 10 个 split
> **状态**: 由 CLAUDE.md §Architecture段引用
>
> **2026-06-24 同步**:10 个 split 已全部合 main,本文件目录树 / §2/§3 / §5/§6 / §9 已重校反映 split 后真实文件结构。下次重大重构后再次校准。
>
> **历史快照标注约定**:本任务对 split 前路径保留 + 加 (拆分自 X, 2026-06-23/24) 标注,git blame 可追溯。

---

##目录

1. [顶层结构](#1-顶层结构)
2. [前端 `app/src/`树](#2-前端-appsrc-树)
3. [后端 `app/src-tauri/src/`树](#3-后端-appsrc-taurisrc-树)
4. [关键模块依赖图](#4-关键模块依赖图)
5. [Tauri IPC表面](#5-tauri-ipc-表面)
6. [数据库 schema](#6-数据库-schema)
7. [Tauri IPC事件表面](#7-tauri-ipc-事件表面)
8. [关键设计模式](#8-关键设计模式)
9. [前端 ↔ 后端数据流](#9-前端--后端数据流)
10. [环境与构建](#10-环境与构建)
11. [测试与质量门](#11-测试与质量门)
12. [依赖与第三方集成](#12-依赖与第三方集成)
13. [文档地图 + 一页式 ASCII 全景](#13-文档地图--一页式-ascii-全景)

---

##1.顶层结构

```
everlasting/
├── AGENTS.md # 多 agent 配置 (2026-06引入)
├── CLAUDE.md # Claude Code 会话入口(架构段引用本文)
├── README.md # 项目一句话 +链接
├── STRUCTURE.md # ← 本文件(8-PR5 创建,根目录显眼位置)
├── THIRD_PARTY_LICENSES.md #第三方许可清单
├── docs/ # 设计文档(全中文)
├── app/ #唯一应用包(单仓模式)
│ ├── src/ # Vue3 前端
│ └── src-tauri/ # Rust 后端(Tauri2)
└── .trellis/ # Trellis 工作流 + spec + tasks + workspace
```

**单包结构**:根无 `package.json`,实际只有1 个包 `app/`。spec `backend` / `frontend` 是逻辑分层,不是物理包。

---

##2. 前端 `app/src/`树

```
app/src/
├── main.ts #入口
├── App.vue #根组件(KeepAlive +全局 dialog)
├── style.css # Tailwind基础 +全局 CSS变量(设计令牌)
├── components/
│ ├── ChatWindow.vue #顶层 chat容器(纯组合)
│ ├── ProjectTabs.vue #顶部项目 tab栏
│ ├── SessionList.vue #侧边栏 session列表
│ ├── Icon.vue # 图标 wrapper
│ ├── chat/ # (8-PR3拆分后;06-23 续拆 3 组件 + 1 composable)
│ │ ├── ChatPanel.vue # ★容器(523 行,957→523)
│ │ ├── ChatInput.vue # (06-23 拆)1834→~712 行,留 props/emits + 提交编排
│ │ ├── ChatInputLatencyPopover.vue # ★ NEW (06-23 拆,自包含 chip+popover)
│ │ ├── ChatInputHintRow.vue # ★ NEW (06-23 拆,token tooltip + ModelSelect)
│ │ ├── chatInputTokens.ts # CodeMirror token highlight plugin(不动)
│ │ ├── MessageList.vue
│ │ ├── MessageItem.vue # (06-23 拆)1099→~770 行
│ │ ├── MessageItemEdit.vue # ★ NEW (06-23 拆,~180 行)
│ │ ├── MessageItemFooter.vue # ★ NEW (06-23 拆,~120 行,error+latency)
│ │ ├── ThinkingBlock.vue / ToolCallCard.vue / ModelSelect.vue
│ │ ├── DiffView.vue / DeleteWorktreeConfirm.vue / EmptyProjectState.vue
│ │ ├── SubagentDrawer.vue # (06-23 拆)1257→~900 行
│ │ ├── SubagentDrawerHeader.vue # ★ NEW (06-23 拆,~250 行)
│ │ ├── SubagentDrawerErrorCard.vue # ★ NEW (06-23 拆,~100 行,R25 错误卡)
│ │ ├── WorktreeChip.vue # ★ NEW (8-PR3拆出)
│ │ └── DiffModal.vue # ★ NEW (8-PR3拆出)
│ ├── settings/ # (8-PR3拆分后)
│ │ ├── SettingsModal.vue / DefaultTab.vue / ProvidersTab.vue
│ │ ├── ModelsTab.vue # ★容器(364 行,954→364)
│ │ ├── ModelRow.vue # ★ NEW (8-PR3拆出)
│ │ ├── ModelForm.vue # ★ NEW (8-PR3拆出)
│ │ └── DeleteModelConfirm.vue # ★ NEW (8-PR3拆出)
│ └── layout/ # (Opus §4.1漏看,8-PR4阶段补)
│ ├── AppShell.vue / AppHeader.vue / AppLogo.vue
│ ├── Sidebar.vue / TitleBar.vue
├── stores/ # Pinia状态
│ ├── chat.ts # (06-23 拆)facade: sessions + currentSessionId + currentCwd + CRUD委托
│ ├── chat.types.ts # ★ NEW (06-23 拆,~310 行纯类型 + 强绑定 const)
│ ├── streamController.ts # ★ SSE 单源 + LRU20 + activeRequests (8-PR3拆)
│ ├── streamController.test.ts
│ ├── subagentRuns.ts # (06-23 拆)store 主体,留 coerceStatus
│ ├── subagentRuns.types.ts # ★ NEW (06-23 拆,~354 行)
│ ├── runAccumulator.ts # ★ NEW (06-23 拆,~537 行 RunAccumulator + parseTranscriptJson)
│ ├── config.ts / projects.ts / models.ts / providers.ts
│ └── permissions.ts / audit.ts / memory.ts / checklist.ts
└── utils/ # (Opus §4.2漏看,8-PR4阶段补;06-23 加 chatInputCodeMirror)
 ├── lru.ts + .test.ts / markdown.ts + .test.ts
 ├── messageFormat.ts + .test.ts / path.ts + .test.ts
 ├── chatInputCodeMirror.ts # ★ NEW (06-23 拆,~564 行 CM 6 composable,0 store import)
 └── duration.ts / tokenUsage.ts / audit.ts / colorTag.ts / useKeyboard.ts
```

**关键组件依赖**:
```
App.vue
└── ProjectTabs.vue
 └── ChatWindow.vue
 ├── SessionList.vue
 └── ChatPanel.vue (8-PR3拆后)
 ├── MessageList → MessageItem (含 ThinkingBlock + ToolCallCard)
 ├── ChatInput → ModelSelect
 ├── WorktreeChip (NEW) / DiffModal → DiffView
 ├── DeleteWorktreeConfirm / EmptyProjectState (条件)
 └── SettingsModal (按需)
 ├── ProvidersTab
 └── ModelsTab → ModelRow + ModelForm + DeleteModelConfirm (NEW)
```

**Store依赖**(单源流): `streamController` (唯一 SSE listener) → `chat` → `config` / `projects` / `models` / `providers`。

---

##3. 后端 `app/src-tauri/src/`树

```
app/src-tauri/src/
├── main.rs # Windows子系统入口 + init_tracing (8-PR1提取)
├── lib.rs # ★入口(94 行,3195→94,纯 mod声明 + invoke_handler)
├── state.rs # ★ NEW (8-PR1) — AppState + CancellationGuard + ProviderCatalog
├── db/ # ★ NEW (8-PR2;06-23 拆 tests.rs → 6 文件)
│ ├── mod.rs / types.rs / migrations.rs
│ ├── projects.rs / sessions.rs / providers.rs / models.rs / config.rs
│ ├── subagent_runs.rs / permissions.rs
│ ├── projects_tests.rs / sessions_tests.rs / providers_tests.rs # ★ (06-23 拆,无 common,test_pool 6 份复制)
│ ├── permissions_tests.rs / messages_tests.rs / subagent_runs_tests.rs
├── llm/
│ ├── mod.rs / client.rs (BlockState状态机) / sse.rs / error.rs / types.rs
│ └── provider/ # 多 provider (06-08/09引入)
│ ├── mod.rs (Provider trait + build_provider工厂)
│ ├── anthropic.rs / openai.rs / mock.rs
│ └── wire.rs # WireMessage中间层(1109 行,高内聚不拆)
├── agent/ # ★ NEW (8-PR1;06-23/24 拆 subagent/ + chat_loop + tests)
│ ├── mod.rs
│ ├── chat.rs # Agent Loop entry(IPC 入口)
│ ├── chat_loop.rs # (06-23 抽 run_subagent 后)2586→~2064 行,主循环 + 主循环辅助
│ ├── subagent/ # ★ NEW (06-23 拆 4 文件,06-23/24 续拆 dispatch)
│ │ ├── mod.rs # 类型 + helpers(lookup / assemble / filter / build_messages)
│ │ ├── sink.rs # SubagentBufferSink (1450 行)
│ │ ├── transcript.rs # transcript 类型 (219 行)
│ │ ├── truncate_summary.rs # 4 MiB cap + format_dispatch_result (910 行)
│ │ └── dispatch.rs # ★ (06-23 抽自 chat_loop.rs)~520 行 run_subagent + resolve_project_id + SUBAGENT_MAX_TURNS
│ ├── provider.rs (resolve_chat_provider + PreFlightError)
│ ├── system_prompt.rs / thinking.rs / helpers.rs / behavior_prompt.rs
│ ├── at_file.rs # B2 @文件补全
│ ├── context.rs # C3 context 压缩
│ ├── permissions/ # ★ (06-23 拆 mod.rs → 8 模块 + 6 测试文件)
│ │ ├── mod.rs # 纯 re-exports(原 2814 → ~50 行)
│ │ ├── types.rs # Risk / Decision / AuditKind / WorkerAskTerminal + PermissionContext/Response/PendingAsk/ToolKind
│ │ ├── store.rs # PermissionStore + register/resolve/cancel
│ │ ├── payload.rs # PermissionAskPayload + ASK_TIMEOUT
│ │ ├── mode.rs # mode_system_prefix + filter_tools_for_mode
│ │ ├── audit.rs # AuditKind 17 variant + 3 record 函数
│ │ ├── check.rs # check 主函数 + classify + grant checks
│ │ ├── ask.rs # ask_path + build_ask_reason
│ │ ├── dangerous.rs / shell_trust.rs # sibling(原拆分前已存在)
│ │ ├── tests_common.rs # (06-23 拆)CaptureAskSink/worker_ctx_with_db/LocalSink
│ │ ├── tests_check.rs / tests_ask.rs / tests_audit.rs
│ │ └── tests_store.rs / tests_payload.rs / tests_types.rs / tests_mode.rs
│ ├── tests_common.rs / tests_cancellation.rs / tests_envelope.rs # ★ (06-23 拆 tests.rs → 6 文件)
│ ├── tests_prompts.rs / tests_agent_loop.rs / tests_subagent.rs
├── commands/ # ★ NEW (8-PR1) — Tauri commands按域拆
│ ├── mod.rs / cancel.rs / config.rs
│ ├── providers.rs (Provider/Model CRUD + test_provider + test_model)
│ ├── sessions.rs (Session CRUD + diff_worktree)
│ ├── worktree.rs (attach/detach/delete + cancel_inflight)
│ ├── projects.rs (Project CRUD + pick_project_dir)
│ ├── permissions.rs / memory.rs / command_palette.rs / panel.rs / files.rs / subagent_runs.rs
├── tools/ # 内置工具 (10 个:read_file / write_file / edit_file / shell / grep / glob / list_dir / web_fetch / use_skill / update_checklist)
│ ├── mod.rs (builtin_tools + execute_tool分发)
│ ├── read_file.rs / write_file.rs / edit_file.rs (644L)
│ ├── shell.rs (5min超时 +30K spill) / grep.rs / glob.rs / list_dir.rs
│ ├── web_fetch.rs # P1 web 抓取(SSRF 拦截 + 5 MiB body cap)
│ ├── use_skill.rs # Skill 调用 tool(B4)
│ └── read_guard.rs (session隔离读权限,edit_file前置)
├── skill/ # B4 Skill 系统
│ ├── mod.rs / loader.rs (SkillCache + 17 单测)
├── memory/ # B5 Memory 指令文件系统
│ ├── mod.rs / loader.rs / file.rs / watcher.rs / tokens.rs / types.rs
├── git/
│ ├── mod.rs / worktree.rs (745L) / diff.rs (git --numstat) / error.rs
└── projects/
 ├── mod.rs / types.rs / store.rs / detector.rs / boundary.rs
```

**模块依赖图**(单向,06-24 反映 10 个 split 后):
```
lib.rs (mod声明 + invoke_handler)
 ├── main.rs (entry + init_tracing)
 ├── state (共享状态 + Cancellation)
 ├── db/* (CRUD by域 + 6 tests_*.rs)
 ├── llm/provider::* → llm::client (BlockState) → types/sse/error
 ├── agent::* (chat + chat_loop + subagent/* + provider + system_prompt + thinking + helpers + permissions/* + 6 tests_*.rs)
 │ →引用 llm::provider + tools + db
 ├── commands::* (IPC分发,10 个域) → agent + db + git + projects
 ├── tools/* → read_guard
 ├── skill/* → loader
 ├── memory/* → loader
 ├── git/* (worktree + diff)
 └── projects/* (types + store + detector + boundary)
```

---

##4.关键模块依赖图

###4.1前后端模块依赖

```
┌─────────────────────────── 前端 ────────────────────────────┐
│ App.vue │
│ ├─ ChatWindow → SessionList + ChatPanel │
│ │ → MessageList/ChatInput/WorktreeChip/DiffModal │
│ └─ SettingsModal → ProvidersTab + ModelsTab(拆 ModelRow/Form)│
│ │
│ Pinia: streamController (单源) → chat → config/projects/... │
└────────────────────────────────────────────────────────────┘
 │ Tauri IPC (invoke + listen)
 ▼
┌─────────────────────────── 后端 ────────────────────────────┐
│ lib.rs (33 个 command) │
│ ├─ commands/* (IPC分发) │
│ ├─ agent/* → llm::provider::* → wire.rs + client.rs │
│ ├─ tools/* (8个 + read_guard) │
│ ├─ db/* (CRUD by域 + migrations) │
│ ├─ git/* (worktree + diff) │
│ └─ projects/* (boundary + detector + store) │
└────────────────────────────────────────────────────────────┘
```

###4.2跨层数据流

```
用户输入 → ChatInput → chat.send() → invoke('chat')
 → agent::chat → resolve_chat_provider → Provider::chat_stream
 → BlockState(SSE) → emit('chat-event')
 → streamController (单源 listener) → chat mutation
 → ChatPanel.vue渲染
```

---

##5. Tauri IPC表面

**总命令数**:~60 个(2026-06-24 实测 `#[tauri::command]`;06-10 快照 33 → 06-18 快照 54 → 06-24 续增 subagent_runs + checklist + memory 等)

|域 | IPC 数 |文件位置 |
|----|-------|---------|
| Agent Loop |1 | `agent/chat.rs` (chat) |
| Cancel |1 | `commands/cancel.rs` |
| LLM config |2 | `commands/config.rs` |
| Provider CRUD |4 | `commands/providers.rs` |
| Model CRUD |5 | `commands/providers.rs` |
| Session model |1 | `commands/providers.rs` (update_session_model_id) |
| Test connection |2 | `commands/providers.rs` (test_provider + test_model) |
| Session CRUD |4 | `commands/sessions.rs` |
| Session worktree |1 | `commands/sessions.rs` (diff_worktree) |
| Worktree |4 | `commands/worktree.rs` (attach/detach/delete + cancel_inflight) |
| Project CRUD |7 | `commands/projects.rs` |
| Project pick |1 | `commands/projects.rs` (pick_project_dir) |
| Permission |2 | `commands/permissions.rs` (permission_response + 读路径) |
| Memory |1 | `commands/memory.rs` (load_for_session 缓存) |
| Command palette |1 | `commands/command_palette.rs` (list_panel_items) |
| Panel / File |2 | `commands/{panel,files}.rs` |
| Subagent runs |5 | `commands/subagent_runs.rs` (get_subagent_run + list_runs_by_session + list_runs_summary_by_session + …) |
| Audit (C4) |1 | `commands/audit.rs` (list_audit_events) |
| Checklist (B12) |1 | `commands/checklist.rs`(随 B12) |
| Files |1 | `commands/files.rs` (list_files 供 @ 触发) |

**IPC命名**: Rust snake_case → Tauri2自动 camelCase转换给前端。

---

##6.数据库 schema

**位置**: `app/src-tauri/src/db/mod.rs::run_migrations`

**8 张核心表**(2026-06-24 现状;原 06-10 快照 7 张 → 06-13 加 `session_audit_events` + `session_tool_permissions` + 06-20 B6 PR2 加 `subagent_runs`):

| 表 | 主键 |关键字段 |
|----|------|---------|
| `projects` | `id` (UUID) | `path` / `name` / `is_git_repo` / `git_remote` / `git_branch` / `is_hidden` |
| `sessions` | `id` (UUID) | `project_id` (FK) / `title` / `model_id` (FK, nullable) / `worktree_path` / `worktree_state` / `current_cwd` / `created_at` / `updated_at` |
| `messages` | `id` | `session_id` (FK) / `role` / `content` (JSON) / `tool_use` (JSON) / `tool_result` (JSON) / `thinking_blocks` (JSON) / `metadata` (JSON, B2 PR3 加:含 `edited_at` / `original_content` 等 D3 字段) / `ttfb_ms` / `gen_ms` / `total_ms` (F5 latency 三列) / `thinking_ms` (F5) / `created_at` |
| `providers` | `id` | `name` / `protocol` / `base_url` / `api_key` / `enabled` |
| `models` | `id` | `provider_id` (FK) / `name` / `model_id` / `max_tokens` / `enabled` |
| `app_config` | `key` | `value` (JSON) |
| `session_audit_events` | `id` | `session_id` (FK, ON DELETE CASCADE) / `kind` (AuditKind 17 variant wire string) / `tool_use_id?` / `path?` / `match_kind?` / `match_value?` / `mode?` / `risk?` / `ts` (RFC3339) |
| `session_tool_permissions` | `id` | `session_id` (FK) / `tool_name` / `match_kind` (`tool` / `prefix` / `path`) / `match_value` / `granted_at` |
| `subagent_runs` | `id` (UUID) | `parent_session_id` (FK, ON DELETE CASCADE) / `parent_request_id` (TEXT, soft FK) / `subagent_name` / `status` (CHECK 5 值) / `started_at` / `finished_at?` / `token_usage_json?` / `summary?` / `transcript_json?` / `transcript_truncated` / `turn_count?`(RULE-FrontSubagent-004 加) / `created_at` |

**索引**:
```sql
CREATE INDEX idx_sessions_project_id ON sessions(project_id);
CREATE INDEX idx_messages_session_id ON messages(session_id);
CREATE INDEX idx_models_provider_id ON models(provider_id);
CREATE INDEX idx_subagent_runs_session_started ON subagent_runs(parent_session_id, started_at DESC);
CREATE INDEX idx_subagent_runs_request ON subagent_runs(parent_request_id);
CREATE INDEX idx_session_audit_events_session_ts ON session_audit_events(session_id, ts DESC);
```

**外键**: `PRAGMA foreign_keys = ON`。`sessions.model_id` 是软 FK (无 `REFERENCES`),允许删除 model 不级联 (Opus D决策)。`subagent_runs.parent_session_id` 是硬 FK + CASCADE,B6 PR2 决定(详见 `.trellis/spec/backend/subagent-runs-schema.md`)。

---

##7. Tauri IPC事件表面

###7.1 高频 payload事件(单事件名 + payload判别)

```typescript
listen<ChatEventPayload>('chat-event', (e) => {
 switch (e.payload.event.type) {
 case 'message_start': /* ... */
 case 'content_block_start': /* ... */
 case 'content_block_delta': /* ... */
 case 'content_block_stop': /* ... */
 case 'message_delta': /* ... */
 case 'message_stop': /* ... */
 case 'ping': /* ... */
 case 'error': /* ... */
 }
});
```

**路由**: 单源 `streamController.ts`监听,按 `request_id`路由到对应 session。

###7.2 低频独立事件

```typescript
listen('tool:call', (e) => { /* ToolCallPayload */ });
listen('tool:result', (e) => { /* ToolResultPayload */ });
```

**设计决策**: 高频 token走 `chat-event`(避免 IPC调度开销);低频 tool call/result走独立事件名(前端可选择性 filter)。详见 `docs/IMPLEMENTATION.md §4决策日志`。

---

##8.关键设计模式

###8.1 流式处理单源(前端)

`streamController.ts` 是 IPC ↔ Pinia 的唯一入口。`chat.ts` 不直接监听 Tauri事件。多 session 并发按 `request_id`路由,LRU20限制活跃请求。详见 `.trellis/spec/frontend/state-management.md`。

###8.2 Provider抽象(后端)

```rust
#[async_trait]
pub trait Provider: Send + Sync {
 async fn chat_stream(&self, request: ChatRequest)
 -> Result<Pin<Box<dyn Stream<Item = Result<ChatEvent, LlmError>> + Send>>, LlmError>;
 fn capabilities(&self) -> WireCapabilities;
}
```

实现: `AnthropicProvider` / `OpenAIProvider`。`wire.rs` 中间层 `WireMessage` / `WireBlock` 是协议无关的"agent内部表示",provider 实现负责 `<-> Wire`转换。

###8.3 ProviderCatalog (8-PR1 新增)

`agent::provider::resolve_chat_provider()` 在 chat启动时一次性构造 catalog,pre-flight 检查 model_id存在 / provider enabled / default_model 配置。避免 per-turn重复构造。

###8.4 Project边界校验

`projects/boundary.rs` 的 `assert_within_project()`拦截所有 tool 调用 (`read_file` / `write_file` / `edit_file` / `shell` / `grep` / `glob` / `list_dir`) 和 LLM指定的 `working_directory`。

###8.5 ReadGuard

`tools/read_guard.rs` 实现 session隔离的"已读文件"集合。`edit_file`写入前必须先 read (3 道 check:已读 / 文件未变 / 未过期),防 LLM写"未见过"的文件。

###8.6 CancellationGuard (RAII)

`state::CancellationGuard` 在 drop 时清理。取消路径: `cancel_chat` command → `CancellationGuard::cancel()` → 中断 SSE stream。

###8.7错误处理

后端 `anyhow` (边界) + `thiserror` (领域)。`LlmError`5 类分类: Auth / RateLimit / Network / InvalidRequest / Server,中文用户消息见 `app/src-tauri/src/llm/error.rs`。

---

##9. 前端 ↔ 后端数据流

###9.1 用户发一条消息(完整)

```
[1] ChatInput.vue 输入 → emit
[2] Pinia chat.send() → invoke('chat', { requestId, sessionId, messages, projectId, cwd })
[3] Tauri IPC
[4] Rust agent::chat::chat
 ├─构造 ToolContext(project_root, session_id, request_id)
 ├─ resolve_chat_provider(model_id → ProviderCatalog)
 └─ agent_loop::run_one_turn() (max20 turns)
 ├─ Provider::chat_stream → SSE → emit('chat-event')
 ├─ if tool_use: emit('tool:call') → tools::execute_tool() → emit('tool:result')
 └─ tool_result 回填 →下一轮
 ↓ turn结束
[5] db::persist_turn()
[6] 前端 streamController(单源)监听 chat-event → chat mutation → ChatPanel渲染
```

###9.2 多 session 并发

- 前端 streamController 按 `request_id`路由
- 后端每个 `chat` command spawn独立 tokio task
- `CancellationGuard` (RAII) 在 drop 时清理
-取消:`cancel_chat` command → `CancellationGuard::cancel()` → 中断 SSE stream

###9.3 Tool 执行流

```
LLM 返回 tool_use → emit('tool:call') → 前端 ToolCallCard显示
 → tools::execute_tool()(边界检查 + ReadGuard)
 → emit('tool:result') → 前端 ToolCallCard显示结果
 →构造 tool_result 回填 LLM →下一轮 Agent Loop
```

**Tool 路径检查**:`agent/permissions/check.rs::is_within_root`(`is_parallel_eligible` 谓词的 path-in-root 分支,`chat_loop.rs` 内,2026-06-19 RULE-A-013 收口后从纯 name 白名单升级为 name + path-in-root 双校验;路径解析约定 `absolute → as-is / relative → root.join(p) / None → eligible` 与 `agent/permissions/check.rs:560-571` 完全一致)

---

##10. 环境与构建

###10.1 环境变量

|变量 | 默认 |用途 |
|------|------|------|
| `ANTHROPIC_API_KEY` | (必需) | Anthropic API key |
| `ANTHROPIC_AUTH_TOKEN` | (可选) | Anthropic auth token替代 |
| `ANTHROPIC_BASE_URL` | `https://api.anthropic.com` | Anthropic base URL |
| `OPENAI_API_KEY` | (可选) | OpenAI API key |
| `OPENAI_BASE_URL` | `https://api.openai.com/v1` | OpenAI base URL |
| `LLM_MODEL` | `GLM-4.7` | 默认模型 |
| `LLM_MAX_TOKENS` | `1024` | 默认 max tokens |
| `RUST_LOG` | (无) | tracing级别(如 `debug`) |

###10.2 构建命令

| 命令 |用途 |
|------|------|
| `cd app && pnpm tauri dev` |启动 dev server(Tauri窗口) |
| `cd app && pnpm tauri build` |前端 type-check + build + Rust编译 +打包 |
| `cd app && pnpm dev` | 仅 Vite dev server |
| `cd app && pnpm build` | 仅前端 build |
| `cd app/src-tauri && cargo check` |快速 Rust编译检查 |
| `cd app/src-tauri && cargo test --lib` | Rust单元测试 |

###10.3 WSL特殊性

linuxbrew pkg-config覆盖系统路径、webkit2gtk-4.1 / gdk-pixbuf-2.0 系统库、CJK字体 HarmonyOS Sans SC 子集打包。详见 `docs/HACKING-wsl.md`。

---

##11. 测试与质量门

|层级 |框架 |覆盖范围 | 文件位置 |
|------|------|---------|---------|
| Rust单元测试 | `#[cfg(test)]` cargo test | sse / error / 部分 db / 部分 llm / wire.rs47% tests | `app/src-tauri/src/{llm,db,agent}/**` |
| 前端单元测试 | vitest | markdown + streamController11 it + lru + messageFormat + path | `app/src/utils/*.test.ts` + `app/src/stores/streamController.test.ts` |
| 前端类型检查 | `vue-tsc --noEmit` | 全 | `pnpm build` |
|端到端 |手动 | Tauri窗口实测 | (无自动化) |

**质量门**: `vue-tsc --noEmit`(pre-build) / `cargo check`(dev) / `cargo test --lib`(可选,CI 未配) /手动端到端(必经)。

**缺口**:端到端无自动化;前端组件测试覆盖率低;Rust集成测试少。

---

##12.依赖与第三方集成

| 层 | 技术 | 版本 |锁定位置 |
|----|------|------|---------|
|桌面框架 | Tauri2 |2.x | `app/src-tauri/Cargo.toml` |
| 前端 | Vue3.4+ |3.4+ | `app/package.json` |
| 前端构建 | Vite |5.x | `app/package.json` |
|状态 | Pinia |2.x | `app/package.json` |
| UI组件 | reka-ui |2.9.9(锁精确) | `app/package.json` |
| 后端 | Rust1.75+ |1.96.0 | `app/src-tauri/Cargo.toml` |
| HTTP | reqwest |0.12 | `app/src-tauri/Cargo.toml` |
|异步 | tokio |1.x | Tauri自带 |
| 数据库 | sqlx + SQLite | sqlx0.7 | `app/src-tauri/Cargo.toml` |
| Git | git2-rs |0.19 | `app/src-tauri/Cargo.toml` |
|错误 | anyhow + thiserror | 最新 | `app/src-tauri/Cargo.toml` |
|日志 | tracing |0.1 | `app/src-tauri/Cargo.toml` |
| Markdown | marked |18.0.5(锁精确) | `app/package.json` |
| Markdown 安全 | DOMPurify |3.4.8(锁精确) | `app/package.json` |

**已评估不引入**:
- ❌ `eventsource-stream`(手写 SSE,spike-002验证)
- ❌ `claude-agent-sdk` / `codex-sdk`(自研 agent core)
- ❌ `sea-orm` / `diesel`(手写 sqlx)
- ❌ `langchain` / `dspy-rs`
- ❌ `rig-core`(2026-06-09决策弃用,自研 Provider trait 已足够)
- ❌ `PyO3` / `Electron`

---

##13.文档地图 + 一页式 ASCII 全景

###13.1文档地图

```
项目根
├── CLAUDE.md # AI 会话入口(架构段引用本文)
├── README.md # 项目一句话 +状态
├── AGENTS.md # 多 agent 配置
├── STRUCTURE.md # ← 本文件
├── docs/ # 设计文档(全中文)
│ ├── README.md # docs索引
│ ├── ARCHITECTURE.md #架构 +16阶段生命周期
│ ├── IMPLEMENTATION.md #8步路线图 +决策日志
│ ├── DESIGN.md / TECH.md / BACKLOG.md
│ ├── HACKING-wsl.md / HACKING-llm.md / HACKING-markdown.md
│ ├── _archive/ / _reviews/ / spikes/
└── .trellis/
 ├── workflow.md
 ├── spec/ # AI协作者规约(8-PR4 已清理空文件)
 │ ├── backend/ # (8-PR5拆 llm-contract 为5 子文件)
 │ ├── frontend/
 │ └── guides/
 ├── tasks/ # (含 archive/2026-06/)
 └── workspace/Carlos/ # journal / audit
```

###13.2文档读取顺序(新 session)

1. **CLAUDE.md**(必读)
2. **IMPLEMENTATION.md**(必读)
4. **DESIGN.md**(必读)
5. **ARCHITECTURE.md**(写代码时反复查)
6. **STRUCTURE.md**(本文,代码结构)
7. **HACKING-***(撞坑时查)
8. **.trellis/spec/***(改代码前必读)
9. **.trellis/tasks/archive/2026-06/***(历史决策)

###13.3 一页式 ASCII 全景

```
┌──────────────────────────────────────────────────────────────┐
│ Everlasting — Vibe Coding Workbench │
│ Tauri2 + Vue3 + Rust + 自研 agent core + WSL-first │
│ │
│ ┌────────────────────┐ ┌──────────────────────────┐ │
│ │ Vue3 Frontend │ IPC │ Rust Agent Core │ │
│ │ (app/src/) │◄────► │ (app/src-tauri/src/) │ │
│ │ · Pinia(7 stores) │ │ ·33 tauri commands │ │
│ │ · stream1 source │ │ · Provider trait │ │
│ │ · reka-ui2.9.9 │ │ (Anthropic/OpenAI) │ │
│ │ · marked+DOMPurify │ │ · Tool registry (8) │ │
│ │ · Vue3.4+ │ │ · git2-rs worktree │ │
│ │ │ │ · sqlx + SQLite (7表) │ │
│ │ │ │ · Hand-written SSE │ │
│ └────────────────────┘ └──────────────────────────┘ │
│ │ │ │
│ ▼ ▼ │
│ ┌──────────────────┐ ┌──────────────────────────┐ │
│ │ 设计令牌/字体 │ │ LLM APIs │ │
│ │主题 │ │ (Anthropic/OpenAI/GLM) │ │
│ └──────────────────┘ └──────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘

代码: app/ 单包(src前端 + src-tauri后端)
文档: docs/ 设计文档 + .trellis/spec/ AI规约
任务: .trellis/tasks/任务 + archive
```

---

## 与 CLAUDE.md / README.md 的关系

### 当前分工

|文档 | 内容 |
|------|------|
| **CLAUDE.md** | 项目概览 +常用命令 + Architecture段(引用本文件) + Env + Tech Stack |
| **README.md** | 项目一句话 +状态 +链接 |
| **STRUCTURE.md** (本文) | 代码结构全景(13 节) |
| **docs/ARCHITECTURE.md** | 系统架构 +16阶段生命周期 |
| **docs/IMPLEMENTATION.md** |8步路线图 +决策日志 |
| **docs/HACKING-*** |踩坑记录(WSL / LLM / markdown) |

###维护边界

- **CLAUDE.md** 不重复本文件;Architecture段只列目录骨架 +引用链接
- **README.md**简短;新读者顺序: README → CLAUDE.md → STRUCTURE.md
- **STRUCTURE.md** 是**单一真相源**;所有"项目代码结构"问题都查本文
- **docs/ARCHITECTURE.md**关注**架构概念**,不重复代码树
- **docs/HACKING-***关注**踩坑记录**,与本文正交

###何时更新哪个文档

|变更类型 | 更新位置 |
|---------|---------|
|顶层文件增删 | 本文件 §1 + CLAUDE.md |
| Vue组件增删 | 本文件 §2 |
| 后端模块增删 | 本文件 §3 |
| tauri command增删 | 本文件 §5 + CLAUDE.md Architecture |
| 数据库表增删 | 本文件 §6 |
| 环境变量增删 | 本文件 §10 + CLAUDE.md |
|依赖增删 | 本文件 §12 + CLAUDE.md + docs/TECH.md |
|架构概念变化 | docs/ARCHITECTURE.md |
|路线图变更 | docs/IMPLEMENTATION.md |
|撞新坑 | docs/HACKING-*.md |
|实施后决策变更 | docs/IMPLEMENTATION.md §4决策日志 |

---

*本文件由 Step8-PR5 创建,基线 commit `0f9a167`。下次重大重构后再次校准。*
