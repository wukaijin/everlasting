# 现状架构调研 — Chat 任务面板(2026-09-02)

主会话探索结论,含 file:line。实现前通读,可省大量重新定位时间。

## 1. Checklist(要合并进新面板的现有浮层)

- 组件 `app/src/components/chat/ChecklistCard.vue`:`position:absolute` 挂 ChatPanel 右下(bottom:156px, right:20px, z-50)。两态:展开面板(280px, 标题"进度清单"+done/total+逐行条目)/ 最小化浮球(44px, 显示 `2/5`,有 in_progress 时呼吸圈)。dumb 组件,数据靠 `items` prop。
  - 状态图标:circle(pending) / loader(in_progress, CSS `app-spin` 转 svg,注意 `transform-box: fill-box` 坑,见 :397-411) / check-mini(done 删除线)。
  - 渲染点:`app/src/components/chat/ChatPanel.vue:1082` `<ChecklistCard :items="currentChecklist" />`;computed `currentChecklist` 在 ChatPanel.vue:581(`checklistStore.getChecklist(sid)`)。
- Store `app/src/stores/checklist.ts`:`checklistBySession: Map<sessionId, ChecklistItem[]>`;缺 key=隐藏,`[]`=空态仍渲染。`ChecklistItem { content, status }`,status 三态。入口:`handleToolCall`(streamEvents.ts:974,update_checklist 的 tool:call 即时写入)+ `rehydrateFromMessages`(扫历史最后一条 committed 更新,streamController.ts:985 / streamEvents.ts:1432 调)+ `clearForNewRun`(chatSendActions.ts:310 / chatMessageActions.ts:216)+ `clearSession`(chatSessionActions.ts:228)。
- **本任务不改 checklist store 任何逻辑**,只把渲染挪进新面板。

## 2. Subagent(数据全通,面板只读 + 复用 drawer)

- DB 表 `subagent_runs`(schema_helpers.rs:153-167);summary 投影 `SubagentRunSummary`(`app/src/stores/subagentRuns.types.ts:79-115`,camelCase wire):`id/subagentName/status/startedAt/finishedAt/summary/task/finalText/turnCount/worktreePath/modelDisplay`(modelDisplay null=继承父级,chip 隐藏,AC14-15 前例)。
- 事件:`subagent:event`(流式)+ `subagent:finished`(终态),Rust 构造在 `agent/subagent/transcript.rs:91/113`;sink 双实现:
  - Tauri(Full/legacy 模式):`AppHandleSubagentSink`(`agent/subagent/event_sink.rs`)→ `app.emit`。
  - daemon(默认 Thin 模式):`HttpSseSubagentSink`(`daemon/sse.rs:368-389`)→ `SseRegistry::broadcast(event_name, payload)`。
- Store `app/src/stores/subagentRuns.ts`:
  - `runSummaryBySession: Map<sessionId, SubagentRunSummary[]>`(:129)响应式。
  - `start()`(:458)挂两个 listener;`subagent:event` 首见于某 runId 时 eager `fetchRun`+`fetchForSession`(:474-478);`subagent:finished` → flush+refetch(:494-502)。**面板可直接读 `runSummaryBySession`,终态翻转自动**。
  - `openDrawer(runId)`(:311)打开现成 `SubagentDrawer.vue`(挂 ChatWindow.vue:91 常驻)。面板行点击直接调它。
- 注意:`stores/subagents.ts` 是 Settings 的 agent 模型配置,别混。谁调 `subagentRuns.start()`:见 store 消费方(ChatView/ChatWindow 启动路径;实现时确认已有调用点,面板无需自己 start)。

## 3. Background shell(后端完整,前端零暴露 —— 本任务主缺口)

- Registry:`app/src-tauri/src/background_shell/mod.rs`(trait `BackgroundShellRegistry` :311)+ `in_memory.rs`。
  - 键 `(session_id, shell_session_id)`,`ShellEntry { command, cwd, started_at(MonotonicMs=进程启动起的 ms,不是墙钟!), max_runtime_ms, origin_tool_use_id, state, kill_tx }`(in_memory.rs:108-131)。
  - `state: Running{pid} | Done{notification, stdout, stderr, full_output_path}`。
  - 迁移点:`start()` 插 Running(:283,含 SpawnFailed 直接 Done 分支 :348-361);`run_background_task` watcher(:691)在 kill/超时/正常退出三路 select 后写 Done + push notification(:797-812);`sweep_completed_shells`(:217,daemon sweeper 1h 保留期后 prune,`daemon/server.rs:139 spawn_shell_sweeper`)。
  - `build_status_from_entry`(:587)已能把 entry 变成 `BackgroundShellStatus`(tagged serde enum:running{started_at,elapsed_ms}/completed{exit_code,completed_at,stdout_preview,stderr_preview,full_output_path}/killed{...})。
  - **trait 无 `list_for_session` 方法,要新增**(impl 遍历 shells map 过滤 session)。
- 工具层:`tools/run_background_shell.rs` / `shell_status.rs` / `shell_kill.rs`(LLM 用,非 IPC)。完成通知走 per-session 队列,下一轮 `drive.rs:817 drain_notifications` 注入 —— 与 UI 事件无关。
- **无任何 `#[tauri::command]`/daemon route 列出/订阅后台 shell**。`AppState.background_shells`(state.rs:226)+ `AppState.sse: Arc<SseRegistry>`(state.rs:248,两进程都初始化,Tauri 路径不读)。

## 4. IPC/事件接线模式(新命令照抄)

- 命令三件套:`commands/subagent_runs.rs`(`xxx_inner` 纯逻辑 + `#[tauri::command]` 薄包装)→ `lib.rs:399` invoke_handler 注册 + `commands/mod.rs:162` 命令名表(Thin 模式 GUI 代理白名单,**新命令必须加**)→ `daemon/routes/subagent_runs.rs`(axum POST `/<cmd>`,调 `_inner`)+ `daemon/routes/mod.rs` 挂 subrouter。
- 事件下发:生产点调 sink;后台 shell 的生产点在 registry(无 sink 上下文)→ **方案:给 registry 注入 emitter 回调**(`Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>`,Option + setter),两进程各自接线:
  - daemon:`daemon/server.rs:55` 构造 AppState 处(或紧后)设 `|name, payload| state.sse.broadcast(name, payload)`。
  - Tauri Full 模式:`lib.rs` setup(AppState load 于 :185)设 `|name, payload| { let _ = app_handle.emit(name, payload); }`。
  - Thin 模式 GUI 无 AppState,天然不需要接线。
- 事件名走 SSE 单流,前端 `transport.listen(name, cb)` 按 name 分发(http.ts 单全局 EventSource)。
- **wire casing**(frontend spec transport-and-pwa-modes.md):invoke 顶层 arg 前端 camelCase → transport 自动转 snake_case;返回/Rust struct 用 `#[serde(rename_all = "camelCase")]` 对齐前端(照抄 SubagentRunSummary);SSE payload 用 camelCase 字段(照抄 build_subagent_event_payload)。

## 5. 前端挂载点与样式约定

- ChatPanel.vue:imports :37-60;`currentChecklist` computed :581;模板 :1082。替换 ChecklistCard → 新面板组件(传 `:items="currentChecklist"` + `:session-id`)。
- 浮层样式:照抄 ChecklistCard 的 absolute 定位/panel/ball CSS(footer 08-24 btn-family 注释惯例:几何家族本地只留固定宽高)。
- vitest:组件测试同目录 `*.test.ts`(例 `AskUserQuestionCard.test.ts`);transport mock 有规范(见 `.trellis/spec/frontend/test-environment.md` 的 canonical transport mock)。
- Session 删除清理:`chatSessionActions.ts:228` 已调 checklist.clearSession,新 store 的 clearSession 在此并列加。

## 6. MonotonicMs 显示注意

`started_at/completed_at` 是**进程单调毫秒**,不能当墙钟用、不能与 `Date.now()` 混算。面板只显示:running 行的 elapsed(后端算好 elapsed_ms 随 list 返回)、done 行的 duration(completed_at - started_at,同源相减有效)。绝对时间一律不显示。
