# STRUCTURE — 项目代码结构全景图

> **基线**:2026-09-01(在 2026-08-31 基础上 + 09-01 批:Sandbox P3c 三态 per-project 配置 + Plan 只读面 + 前台升级闭环 / Sandbox P3d 后台升级闭环)
> **来源**:融合本地 audit `.trellis/workspace/Carlos/audit-2026-06-09/04-codebase-map.md` + Opus评审 `docs/_history/reviews/REVIEW-claude-opus-2026-06-09.md` + 8-PR 系列实际落地状态 + 06-23/24 10 个 split + 07-08~10 workflow 集成 + 07-20~23 daemon 化 + 07-23~08-04 交错思考/review-viz/群聊 + 08-11~13 remote-control epic(remote 服务端 + tunnel client + 移动 PWA)+ workspace 翻转 + 08-14~28(C7/C7D/memory-gov/B1/D2/C3+/budget/softcap/worker-trace/F1/F4/F5/F6/F3/F2·F2b)
> **状态**: 由 CLAUDE.md §Architecture 段引用
>
> **2026-09-01 同步**:Sandbox P3c/P3d 功能批。§3 后端树补 `agent/permissions/escalation.rs`(前台升级闭环)+ `agent/chat_loop/background_escalation.rs`(P3d 后台 EscalationOffer 下轮注入时 resolve)+ `sandbox/` 注释扩三态(P3c `resolve_policy` 决策链 + Plan 只读面 + 前后台升级闭环);§2 前端树补 `settings/ProjectSandboxTab.vue`(项目沙盒三态设置面);§5 IPC 总命令数 107→108(新增 `update_project_sandbox_policy`,2026-09-01 实测 `generate_handler!` 注册数),Projects 域 8→9;§6 `projects` 表补 `sandbox_policy` 列(off/readwrite/readonly 默认 readwrite,AuditKind 29 不变——升级闭环零新 kind);§8.8 执行期沙盒扩 P3c/P3d。
>
> **2026-08-31 同步**:08-29~31 功能批。§3 后端树补 `sandbox/`(P3b Landlock+seccomp 执行器)+ `tools/tool_output.rs`(C6 截断统一)+ scheduler per_run 三档与 set_app_config_list 写通道;§2 前端树补 `chat/ShellCard.vue` + `chat/PermissionActions.vue` + `settings/ScheduledTasksTab.vue`,§1 顶层树补 `app/e2e/`;§5 IPC 表修正为 107(2026-08-31 实测 invoke_handler 注册数;旧 118 为含注释的 grep 口径)+ 明细补 Scheduled tasks/message_queue/usage/attachments 域与审计分页命令;§6 表数修正为 13 实体 + 2 FTS5 虚拟 + `scheduled_tasks` 补三新列 + AuditKind 28→29;§11 端到端改为 Playwright 自动化(修正旧"无自动化"事实错误)。
>
> **2026-08-28 同步**:08-14~28 功能批。§3 后端树补 `scheduler/`(F2 定时调度器)+ `agent/message_queue.rs`(F1)+ `agent/doc_extract.rs`(F5)+ `tools/{search_history.rs,web_search/,stub.rs}`(D2②/F4/C7D);§5 IPC 表 97→118;§6 schema 表 12→14(补 `scheduled_tasks`);AuditKind 17→28。
>
> **2026-08-13 同步**:remote-control-epic-s1 合入(merge `94828cb`)+ workspace 翻转(08-11)。§1 顶层结构改 3-crate workspace(根 `Cargo.toml` / `Cargo.lock` / `crates/`);§2 前端补 router/views/stores/transport/auth + PWA;§3 后端补 `daemon/tunnel/` + `commands/pairing.rs` + config.rs tunnel 命令;§5 IPC 表 91→97;§10.2/§12 补根 workspace 构建与 Cargo.lock 位置。
>
> **2026-08-05 同步**:补群聊(`agent/group_chat.rs` + `group_chat_loop.rs` + `nominate_speaker`/`end_discussion` 工具 + `GroupChatConfigModal.vue` + sessions/messages 群聊列)+ C2 review-viz(`commands/review.rs` + `ReviewMatrix*` 4 组件 + `reviewState.ts` store)+ 交错思考(ARCHITECTURE ⑦⑭)。§2/§3 目录树补新模块;§5 IPC 表 79→91 + Review 域;§6 表 8→12(补 `subagent_model_overrides`/`autonomous_memories`/`turn_trace` + 群聊列);stores/commands 清单补漏。下次重大重构后再次校准。
>
> **2026-07-23 同步**:daemon 化(remote-access Phase 2)收官。§1/§2/§3 目录树补 `daemon/` + `bin/` + `sidecar.rs` + `transport/` + `BrowserHeader.vue` + `scripts/`;§4 依赖图/数据流改为 transport + 双进程;§5 IPC 表更新为 79 + 双暴露;§8 加 transport/sidecar 设计模式;§10.2/§12 补 daemon 命令与依赖;§13.3 全景图重画。
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

## 1. 顶层结构

```
everlasting/
├── AGENTS.md # 多 agent 配置 (2026-06引入)
├── CLAUDE.md # Claude Code 会话入口(架构段引用本文)
├── README.md # 项目一句话 +链接
├── STRUCTURE.md # ← 本文件(8-PR5 创建,根目录显眼位置)
├── THIRD_PARTY_LICENSES.md #第三方许可清单
├── Cargo.toml # ★ workspace(08-11 翻转) — members = app/src-tauri + crates/everlasting-remote + crates/everlasting-remote-protocol;default-members 只含两 remote crate
├── Cargo.lock # ★ 根锁文件(08-11 自 app/src-tauri/ 上移,锁定全 workspace)
├── crates/ # ★ NEW (08-11 remote epic) — 独立 Rust crate(workspace 成员)
│ ├── everlasting-remote/ # 云服务端(auth/配对码/WSS 隧道/反向代理/SSE 桥/限速/DB;零系统库依赖)
│ └── everlasting-remote-protocol/ # 隧道帧协议定义
├── docs/ # 设计文档(全中文)
├── scripts/ # ★ (07-23 daemon.sh;08-11 补 remote.sh + deploy-remote.sh + remote-e2e-smoke.mjs)
├── app/ #唯一前端应用包(单仓模式;Rust 侧为 workspace 成员)
│ ├── e2e/ # ★ NEW (08-30 Playwright) — 浏览器回归(fixtures.ts + 3 spec + README)
│ ├── src/ # Vue3 前端(含 ★ transport/ 抽象层,07-20;router/views/stores + PWA,08-11)
│ └── src-tauri/ # Rust 后端(Tauri2;daemon 化后含 ★ daemon/ + bin/ + sidecar.rs + tunnel/)
└── .trellis/ # Trellis 工作流 + spec + tasks + workspace
```

**3-crate workspace + 单前端包**(08-11 workspace 翻转):根 `Cargo.toml` 是 workspace(非 package),members = `app/src-tauri`(everlasting)+ `crates/everlasting-remote` + `crates/everlasting-remote-protocol`;`default-members` 只含两个 remote crate,故根目录裸 `cargo build/test` 只编译 remote 两 crate,app 需 `-p everlasting`(或 cd app/src-tauri 后裸命令)。根无 `package.json`,前端只有 1 个包 `app/`;spec `backend` / `frontend` 是逻辑分层,不是物理包。

---

## 2. 前端 `app/src/` 树

```
app/src/
├── main.ts #入口
├── router/ # ★ NEW (08-11 remote) — vue-router 4(index.ts:/chat + /pairing + /nodes + isRemoteContext() 守卫)
├── views/ # ★ NEW (08-11 remote) — ChatView / PairingView(配对码兑换) / NodeListView(节点列表)
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
│ │ ├── ShellCard.vue # ★ NEW (08-30) — shell 命令专属卡(命令块常驻 + 一体化审批)
│ │ ├── PermissionActions.vue # ★ NEW (08-30) — 审批按钮组抽取(ShellCard/ToolCallCard 共用)
│ │ ├── DiffView.vue / DeleteWorktreeConfirm.vue / EmptyProjectState.vue
│ │ ├── SubagentDrawer.vue # (06-23 拆)1257→~900 行
│ │ ├── SubagentDrawerHeader.vue # ★ NEW (06-23 拆,~250 行)
│ │ ├── SubagentDrawerErrorCard.vue # ★ NEW (06-23 拆,~100 行,R25 错误卡)
│ │ ├── WorktreeChip.vue # ★ NEW (8-PR3拆出)
│ │ ├── DiffModal.vue # ★ NEW (8-PR3拆出)
│ │ ├── GroupChatConfigModal.vue # ★ NEW (07-29 群聊) — 创建群聊 session + 参与者配置 UI
│ │ ├── ReviewMatrix.vue / ReviewMatrixGrid.vue # ★ NEW (07-26 C2) — review-state 矩阵视图(维度×发现)
│ │ ├── ReviewFindingDetail.vue / ReviewDimensionCompare.vue # ★ NEW (07-26 C2) — 发现详情 + 维度对比
│ ├── trace/ # ★ NEW (07-14 E2) — harness trace viewer (TracePanel drawer / TurnTimeline / TurnCard / TraceEventItem)
│ ├── settings/ # (8-PR3拆分后)
│ │ ├── SettingsModal.vue / DefaultTab.vue / ProvidersTab.vue
│ │ ├── ModelsTab.vue # ★容器(364 行,954→364)
│ │ ├── ModelRow.vue # ★ NEW (8-PR3拆出)
│ │ ├── ModelForm.vue # ★ NEW (8-PR3拆出)
│ │ ├── ProjectSandboxTab.vue # ★ NEW (09-01 P3c) — 项目沙盒三态设置面(sandbox_policy raw/effective 分离,RULE-SBX-002)
│ │ ├── RemoteTab.vue # ★ NEW (08-11 remote) — 远程隧道配置(tunnel 状态 + remote 配置 + 配对入口)
│ │ ├── ScheduledTasksTab.vue # ★ NEW (08-28 F2) — 定时任务管理 tab(CRUD + 状态 + per_run 三档表单,08-31)
│ │ └── DeleteModelConfirm.vue # ★ NEW (8-PR3拆出)
│ └── layout/ # (Opus §4.1漏看,8-PR4阶段补)
│ ├── AppShell.vue / AppHeader.vue / AppLogo.vue
│ ├── Sidebar.vue / TitleBar.vue
│ ├── BrowserHeader.vue # ★ NEW (07-23) — 浏览器模式顶栏(isTauriWebview()=false 时替代 TitleBar)
├── stores/ # Pinia状态
│ ├── chat.ts # (06-23 拆)facade: sessions + currentSessionId + currentCwd + CRUD委托
│ ├── chat.types.ts # ★ NEW (06-23 拆,~310 行纯类型 + 强绑定 const)
│ ├── streamController.ts # ★ SSE 单源 + LRU20 + activeRequests (8-PR3拆)
│ ├── streamController.test.ts
│ ├── subagentRuns.ts # (06-23 拆)store 主体,留 coerceStatus
│ ├── subagentRuns.types.ts # ★ NEW (06-23 拆,~354 行)
│ ├── runAccumulator.ts # ★ NEW (06-23 拆,~537 行 RunAccumulator + parseTranscriptJson)
│ ├── config.ts / projects.ts / models.ts / providers.ts
│ ├── permissions.ts / permissionGrants.ts / audit.ts / memory.ts / checklist.ts
│ ├── questionCards.ts + questionCards.types.ts # ★ NEW (07-07) — ask_user_question / request_mode_change inline card
│ ├── subagents.ts # ★ NEW (07-03 B6+ C) — subagent 模型 override UI store
│ ├── traceStore.ts # ★ NEW (07-14 E2) — harness trace store (live+回看同构 TurnTrace)
│ ├── reviewState.ts # ★ NEW (07-26 C2) — review-state 矩阵视图 store(三态载荷)
│ ├── nodes.ts # ★ NEW (08-11 remote) — 远程节点列表 store
│ ├── pairing.ts # ★ NEW (08-11 remote) — 配对码兑换 device_token 流
│ ├── remoteConfig.ts # ★ NEW (08-11 remote) — remote 隧道配置 store(RemoteTab 数据源)
│ ├── chatModeActions.ts / chatSendActions.ts / chatSessionActions.ts / chatMessageActions.ts # ★ (08-14 拆 chat.ts facade 的动作组合)
│ ├── messageQueueStore.ts # ★ NEW (08-25 F1) — 消息队列 store(排队提示 + 召回/移除)
│ ├── scheduledTasks.ts # ★ NEW (08-28 F2) — 定时任务 store(CRUD + 状态)
│ ├── quota.ts # ★ NEW (08-27 F6) — 用量窗口 store
│ ├── webSearch.ts # ★ NEW (08-25 F4) — web_search 配置 store
│ ├── streamEvents.ts / streamRehydrate.ts # ★ (08-14 拆分) — streamController 的 SSE 事件分发 + rehydrate
│ └── traceStore.test.ts
└── utils/ # (Opus §4.2漏看,8-PR4阶段补;06-23 加 chatInputCodeMirror)
 ├── lru.ts + .test.ts / markdown.ts + .test.ts
 ├── messageFormat.ts + .test.ts / path.ts + .test.ts
 ├── chatInputCodeMirror.ts # ★ NEW (06-23 拆,~564 行 CM 6 composable,0 store import)
 └── duration.ts / tokenUsage.ts / audit.ts / colorTag.ts / useKeyboard.ts
transport/ # ★ NEW (07-20 remote-access) — 前端 transport 抽象层(invoke/listen 与载体解耦)
├── index.ts # resolveTransport():默认 httpTransport;?transport=tauri 逃生 → tauriTransport
├── http.ts # httpTransport(fetch POST + SSE EventSource → everlasting-daemon 同源;pwa-remote 模式走 /api/v1/proxy 前缀 + Bearer)
├── auth.ts # ★ NEW (08-11 remote) — pwa-remote 模式 device_token 注入(isRemoteContext 检测 + access_token 通道)
├── tauri.ts # tauriTransport(@tauri-apps/api 透传,Full 模式逃生舱)
├── health.ts # daemon health 轮询 + 自动降级
├── env.ts # isTauriWebview():Tauri webview vs 纯浏览器检测
├── types.ts # Transport trait 签名 + UnlistenFn
└── *.test.ts # http / health / transport / transport-parity 4 个测试
```

**PWA(08-11 remote)**:`vite-plugin-pwa`(manifest + app-shell precache SW,配在 `app/vite.config.ts`)+ 图标 `app/public/icons/`(192/512/maskable),移动端可安装,经 remote 服务端 + WSS 隧道反向访问 PC daemon。

**关键组件依赖**:
```
App.vue
├── router/index.ts (★ 08-11 vue-router 4) → views/{ChatView, NodeListView, PairingView}(PWA 远程入口)
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
 ├── ModelsTab → ModelRow + ModelForm + DeleteModelConfirm (NEW)
 └── RemoteTab (★ 08-11,远程隧道配置)
```

**Store依赖**(单源流): `streamController` (唯一 SSE listener) → `chat` → `config` / `projects` / `models` / `providers`。remote 侧(08-11)独立成链:`nodes` / `pairing` / `remoteConfig` 走 `transport/auth.ts` 的 device_token 认证(pwa-remote 模式),不挂 chat 主链。

---

## 3. 后端 `app/src-tauri/src/` 树

```
app/src-tauri/src/
├── main.rs # Windows子系统入口 + init_tracing (8-PR1提取)
├── lib.rs # ★入口(94 行,3195→94,纯 mod声明 + invoke_handler)
├── state.rs # ★ NEW (8-PR1) — AppState + CancellationGuard + ProviderCatalog(load_inner / load_from_dir;daemon 侧也用)
├── sidecar.rs # ★ NEW (07-22 P2.4) — GuiMode{Thin,Full} + spawn_and_manage(everlasting-daemon sidecar via tauri-plugin-shell)
├── db/ # ★ NEW (8-PR2;06-23 拆 tests.rs → 6 文件)
│ ├── mod.rs / types.rs / migrations.rs
│ ├── projects.rs / sessions.rs / providers.rs / models.rs / config.rs
│ ├── subagent_runs.rs / permissions.rs
│ ├── trace.rs # ★ NEW (07-14 E2) — turn_trace 表 CRUD + 4 UPSERT + list/clear
│ ├── projects_tests.rs / sessions_tests.rs / providers_tests.rs # ★ (06-23 拆,无 common,test_pool 6 份复制)
│ ├── permissions_tests.rs / messages_tests.rs / subagent_runs_tests.rs
├── llm/
│ ├── mod.rs / client.rs (BlockState状态机) / sse.rs / error.rs / types.rs
│ ├── retry.rs # ★ NEW (07-05 A5+) — retry_open wrapper(Full Jitter + 首字节前重试)
│ └── provider/ # 多 provider (06-08/09引入)
│ ├── mod.rs (Provider trait + build_provider工厂)
│ ├── anthropic.rs / openai.rs / mock.rs
│ └── wire.rs # WireMessage中间层(1109 行,高内聚不拆)
├── agent/ # ★ NEW (8-PR1;06-23/24 拆 subagent/ + chat_loop + tests;07-08~10 加 workflow/)
│ ├── mod.rs
│ ├── chat.rs # Agent Loop entry(IPC 入口)
│ ├── trace.rs # ★ NEW (07-14 E2) — trace pipeline(3 record_* 双写 emit+upsert)
│ ├── chat_loop.rs # (06-23 抽 run_subagent 后)2586→~2064 行,主循环 + 主循环辅助
│ ├── chat_loop/ # (主循环分治子目录:drive/init/tools/suite;09-01 P3d 加 background_escalation)
│ │ └── background_escalation.rs # ★ NEW (09-01 P3d) — 后台 EscalationOffer 下轮注入时 resolve_all(695 行)
│ ├── message_queue.rs # ★ NEW (08-25 F1) — per-session 内存消息队列(FIFO/uuid 寻址/上限 20)+ run_queue_driver 驱动器(turn 边界 drain 批量注入)
│ ├── doc_extract.rs # ★ NEW (08-26 F5) — @文件 PDF/docx/xlsx 原生文本提取纯函数(pdf-extract / quick-xml / calamine,bytes 进文本出)
│ ├── group_chat.rs / group_chat_loop.rs # ★ NEW (07-29) — 群聊 turn-taking 编排引擎(moderator 轮次控制 + 参与者身份护栏 + 终止/发言人事件 + 逐轮流式;session_type=group_chat 时走此循环)
│ ├── subagent/ # ★ NEW (06-23 拆 4 文件,06-23/24 续拆 dispatch)
│ │ ├── mod.rs # 类型 + helpers(lookup / assemble / filter / build_messages)
│ │ ├── sink.rs # SubagentBufferSink (1450 行)
│ │ ├── transcript.rs # transcript 类型 (219 行)
│ │ ├── truncate_summary.rs # 4 MiB cap + format_dispatch_result (910 行)
│ │ └── dispatch.rs # ★ (06-23 抽自 chat_loop.rs)~520 行 run_subagent + resolve_project_id + SUBAGENT_MAX_TURNS
│ ├── workflow/ # ★ NEW (07-08~10) — workflow 系统核心(workflow.json 外置 + task state machine)
│ │ ├── mod.rs (re-exports + 公共类型)
│ │ ├── def.rs (WorkflowDef struct + 4 访问函数 + default_workflow)
│ │ ├── builtin.rs (builtin dev workflow plugin loader)
│ │ ├── inject.rs (per-turn breadcrumb + bootstrap hint + resolve_current_task 即时读盘)
│ │ ├── state.rs (TaskStatus state machine helpers)
│ │ └── task.rs (TaskJson schema + read_task lenient + create_task_init)
│ ├── provider.rs (resolve_chat_provider + PreFlightError)
│ ├── system_prompt.rs / thinking.rs / helpers.rs / behavior_prompt.rs
│ ├── at_file.rs # B2 @文件补全
│ ├── context.rs # C3 context 压缩
│ ├── loop_detection.rs # ★ (06-24 C2 + 07-06 C2+) — 分级触发 + per-run-local count + worker break
│ ├── question_store.rs # ★ (07-07) — ask_user_question 跨 turn 状态
│ ├── auto_reflect.rs / memory_recall.rs / memory_hygiene.rs # ★ V2 2 期 自主记忆(06-29)
│ ├── permissions/ # ★ (06-23 拆 mod.rs → 8 模块 + 6 测试文件)
│ │ ├── mod.rs # 纯 re-exports(原 2814 → ~50 行)
│ │ ├── types.rs # Risk / Decision / AuditKind / WorkerAskTerminal + PermissionContext/Response/PendingAsk/ToolKind
│ │ ├── store.rs # PermissionStore + register/resolve/cancel
│ │ ├── payload.rs # PermissionAskPayload + ASK_TIMEOUT
│ │ ├── mode.rs # mode_system_prefix + filter_tools_for_mode
│ │ ├── audit.rs # AuditKind 29 variant + 3 record 函数
│ │ ├── check.rs # check 主函数 + classify + grant checks(+ check/ 子目录;P3c 09-01 Tier 4 shell 分支面短路:Policy≠Off 跳过审批直接 Allow)
│ │ ├── ask.rs # ask_path + build_ask_reason
│ │ ├── escalation.rs # ★ NEW (09-01 P3c;P3d 参数化) — 前台升级闭环原语:prefix_grant_hit / EscalationHandle::ask / audit_grant_rerun(tool_name 参数化,后台共用)
│ │ ├── dangerous.rs / shell_trust.rs # sibling(原拆分前已存在)
│ │ ├── tests_common.rs # (06-23 拆)CaptureAskSink/worker_ctx_with_db/LocalSink
│ │ ├── tests_check.rs / tests_ask.rs / tests_audit.rs
│ │ └── tests_store.rs / tests_payload.rs / tests_types.rs / tests_mode.rs
│ ├── tests_common.rs / tests_cancellation.rs / tests_envelope.rs # ★ (06-23 拆 tests.rs → 6 文件)
│ ├── tests_prompts.rs / tests_agent_loop.rs / tests_subagent.rs
├── commands/ # ★ NEW (8-PR1) — Tauri commands按域拆(07-03/07/08 加 task/subagents/question)
│ ├── mod.rs / cancel.rs / config.rs(★ 08-11 加 get_remote_config/set_remote_config/get_tunnel_status)
│ ├── providers.rs (Provider/Model CRUD + test_provider + test_model)
│ ├── sessions.rs (Session CRUD + diff_worktree)
│ ├── worktree.rs (attach/detach/delete + cancel_inflight)
│ ├── projects.rs (Project CRUD + browse_dir 目录浏览)
│ ├── permissions.rs / memory.rs / command_palette.rs / panel.rs / files.rs / subagent_runs.rs
│ ├── task.rs # ★ (07-08~10) — task.json CRUD IPC(create_task / read_task / list_tasks / set_task_state / archive_task)
│ ├── subagents.rs # ★ (07-03) — subagent model override IPC(list_subagents_with_model + set_subagent_model)
│ ├── question.rs # ★ (07-07) — get_pending_interaction + resolve_mode_change(QuestionStore 升级)
│ ├── review.rs # ★ (07-26 C2) — get_review_state + get_current_task_slug(review-state.json 三态载荷)
│ ├── pairing.rs # ★ NEW (08-11 remote) — generate_pairing_code(配对码,远端 PWA 兑换 device_token)
│ ├── scheduled_tasks.rs # ★ NEW (08-28 F2) — 定时任务 CRUD(create/update/delete/list;08-31 加 per_run 目标 session 三档)
│ ├── message_queue.rs # ★ NEW (08-25 F1) — 消息队列 IPC(list/remove/recall queued messages)
│ ├── usage.rs # ★ NEW (08-27 F6) — usage_window(用量窗口)
│ ├── attachments.rs # ★ NEW (08-27 F5) — save_attachment(@附件落盘)
│ └── ui.rs # ★ (07-13 B9+) — apply_ui_diff(生成式 UI diff 应用)
├── tools/ # 内置工具 (28 个 builtin;07-08~10 加 workflow 等;07-29 加群聊 nominate_speaker/end_discussion;08-17 加 search_history;08-25 加 web_search;08-29 加 schedule_task 家族 3 个;08-30 C6 截断统一;08-31 sandbox 执行期接入;09-01 P3c 三态/Plan 面 + P3d ToolContext.tool_use_id 盖章)
│ ├── mod.rs (builtin_tools + execute_tool 分发 + filter_tools_for_mode/subagent/workflow)
│ ├── read_file.rs / write_file.rs / edit_file.rs (644L)
│ ├── shell.rs (5min超时;08-30 C6 起输出截断走统一 tool_output 契约 三恢复模式 + spill)
│ ├── tool_output.rs # ★ NEW (08-30 C6) — tool 输出截断统一契约(三恢复模式:inline 截断 / spill 落盘 / 行级恢复指引;统一 `<truncated>` 标记;spill 落 `app_data_dir/outputs/<session>/`;shell/background/read_file/web_fetch/grep 共用)
│ ├── shell_kill.rs / shell_status.rs / run_background_shell.rs # L1a 后台 shell(06-19)
│ ├── grep.rs / glob.rs / list_dir.rs # L2 并发只读集
│ ├── web_fetch.rs # P1 web 抓取(SSRF 拦截 + 5 MiB body cap)
│ ├── web_search/ # ★ (08-25 F4) 搜索工具(Tavily/DDG 双后端 + 30s 预算重试环;mod.rs)
│ ├── search_history.rs # ★ (08-17 D2②) 跨 session 全文搜索 tool(薄封装 db::search)
│ ├── use_skill.rs # B4 Skill 调用(workflow-aware 三层渐进披露)
│ ├── use_ui.rs # ★ (07-02 B9) 生成式 UI(non-blocking execute + UiCard registry)
│ ├── update_checklist.rs # B12 agent 自跟踪 checklist(workflow 分支同步 task.json.items)
│ ├── remember.rs # ★ (06-29 V2 2 期) 自主记忆写入 tool
│ ├── ask_user_question.rs # 跨 turn 问题(selector 复用 ask_user_question 路径)
│ ├── dispatch_subagent.rs # B6 派 worker agent
│ ├── merge_worker.rs / discard_worker.rs # ★ L3b PR3 worker worktree 收口(ToolKind::GitMutation)
│ ├── request_mode_change.rs # ★ (07-07 B6+ A) LLM 申请切 mode(用户 inline card 授权)
│ ├── create_task.rs / request_task_state_transition.rs # ★ (07-08/10) workflow tools(workflow_enabled session 可见,filter_tools_for_workflow 白名单)
│ ├── nominate_speaker.rs / end_discussion.rs # ★ (07-29 群聊) — moderator 发言控制 SIGNAL 工具(chat_loop 拦截记录提名/终止信号)
│ ├── stub.rs # ★ (08-14 C7D) STUB_CANDIDATES + load_tool_schemas 元工具(stub 原地替换大 schema 工具)
│ └── read_guard.rs (session隔离读权限,edit_file前置)
├── skill/ # B4 Skill 系统
│ ├── mod.rs / loader.rs (SkillCache + 17 单测)
├── memory/ # B5 Memory 指令文件系统 + V2 2 期 自主记忆存储
│ ├── mod.rs / loader.rs / file.rs / watcher.rs / tokens.rs / types.rs
├── background_shell/ # ★ NEW (06-19 L1a) — BackgroundShellRegistry trait + InMemory impl
│ ├── mod.rs (trait 定义 + 公共类型)
│ └── in_memory.rs (tokio 后台 task + 进程内 registry + drain_notifications)
├── daemon/ # ★ NEW (07-20~23 remote-access) — axum HTTP daemon(everlasting-daemon bin 的核心)
│ ├── mod.rs (re-exports + daemon_version)
│ ├── server.rs (build_router + serve_daemon:TcpListener 0.0.0.0:7456 + ServeDir 同源 SPA fallback + graceful shutdown)
│ ├── sse.rs (HttpSseSink — agent loop 的 SSE 事件流广播;08-27 起 chat-event payload 回填 session_id)
│ ├── error.rs (DaemonError → axum IntoResponse)
│ ├── tunnel/ # ★ NEW (08-11 remote) — PC 侧 WSS tunnel client(连 crates/everlasting-remote,loopback 转发到本机 daemon)
│ │ ├── mod.rs / client.rs (WSS 长连接:shared_secret + node_id + 心跳)
│ │ ├── config.rs (tunnel 配置:remote_url/shared_secret 等,存 app_config)
│ │ ├── manager.rs / dispatcher.rs (TunnelManager + 请求分发 loopback 转发)
│ │ ├── node_id.rs / sse_bridge.rs (节点 id + SSE 桥接,取消只停转发)
│ │ └── tests.rs / e2e_tests.rs (单元 + 端到端隧道测试)
│ └── routes/ # 108 个 #[tauri::command] 镜像为 REST 路由(同 handler 双暴露 IPC+HTTP)
│ ├── mod.rs / health.rs / stream.rs(SSE)
│ ├── sessions.rs / projects.rs / config.rs / providers.rs / subagents.rs / subagent_runs.rs
│ ├── memory.rs / permissions.rs / files.rs / worktree.rs / task.rs / question.rs / review.rs
│ ├── command_palette.rs / panel.rs / agent.rs / cancel.rs / ui.rs / scheduled_tasks.rs / message_queue.rs / usage.rs / attachments.rs / pairing.rs
├── scheduler/ # ★ NEW (08-28 F2/F2b) — daemon 常驻定时调度器(30s tick + CancellationToken 停机;单一扫描算法 + due 落账 + catch-up;同 session 每 tick 至多一 fire;F2b 四道 gate + completed 审计;08-31 per_run 三档)
│ ├── mod.rs (spawn_task_scheduler + tick 主循环 + kill switch)
│ ├── compute.rs (most_recent_due / next_fire_display 等纯函数)
│ ├── tests_tick.rs / tests_lost.rs
│ └── (fire 走 chat_inner + origin 载体链落 messages.metadata.scheduled)
├── sandbox/ # ★ NEW (08-31 P3b;09-01 P3c 三态) — 执行期沙盒(Landlock ruleset + seccomp BPF;P3b ReadOnly 档 → P3c 三态 per-project `sandbox_policy`(off/readwrite/readonly)全命令进沙盒 + Plan 只读面 + resolve_policy 决策链;前后台升级闭环在 agent/permissions/escalation.rs + chat_loop/background_escalation.rs)
│ ├── mod.rs (入口 + 策略组装 + spawn 沙盒命令)
│ ├── landlock.rs (Landlock ruleset——文件系统路径限制)
│ ├── seccomp.rs (seccomp BPF——系统调用过滤)
│ ├── policy.rs (ruleset 摘要 + 配置解析)
│ └── tests_sandbox.rs
├── bin/ # ★ NEW (07-20) — cargo bin targets
│ └── everlasting-daemon.rs (daemon 进程入口:resolve_data_dir + clap --port/--data-dir + serve_daemon)
├── git/
│ ├── mod.rs / worktree.rs (745L) / diff.rs (git --numstat) / error.rs
└── projects/
 ├── mod.rs / types.rs / store.rs / detector.rs / boundary.rs
```

**模块依赖图**(单向,07-23 反映 daemon 化后):
```
lib.rs (mod声明 + invoke_handler + sidecar spawn + RunEvent::Exit 回收)
 ├── main.rs (entry + init_tracing)
 ├── state (共享状态 + Cancellation;daemon 侧 load_from_dir 复用)
 ├── sidecar (GuiMode{Thin,Full} + spawn_and_manage daemon 子进程)
 ├── daemon/* (axum router + HttpSseSink + routes/;★ tunnel/ remote client;bin/everlasting-daemon 入口调 serve_daemon)
 │   └── tunnel/* (client/config/dispatcher/manager/node_id/sse_bridge → WSS 连 crates/everlasting-remote)
 ├── db/* (CRUD by域 + 6 tests_*.rs)
 ├── llm/provider::* → llm::client (BlockState) → types/sse/error
 ├── agent::* (chat + chat_loop + subagent/* + provider + system_prompt + thinking + helpers + permissions/* + 6 tests_*.rs)
 │ →引用 llm::provider + tools + db
 ├── commands::* (IPC分发,按域拆;同一 handler 经 daemon/routes/ 双暴露为 REST) → agent + db + git + projects
 ├── tools/* → read_guard + tool_output(截断契约) + sandbox::{landlock,seccomp,policy}(P3b 执行期沙盒 → P3c 三态/Plan 面;ToolContext.tool_use_id dispatch 统一盖章,P3d)
 ├── sandbox/* (landlock + seccomp + policy(三态 resolve_policy);shell.rs / run_background_shell.rs 调用 + permissions/escalation.rs + chat_loop/background_escalation.rs 升级闭环)
 ├── skill/* → loader
 ├── memory/* → loader
 ├── git/* (worktree + diff)
 └── projects/* (types + store + detector + boundary)
```

---

## 4. 关键模块依赖图

###4.1前后端模块依赖

```
┌─────────────────────────── 前端 ────────────────────────────┐
│ App.vue │
│ ├─ ChatWindow → SessionList + ChatPanel │
│ │ → MessageList/ChatInput/WorktreeChip/DiffModal │
│ └─ SettingsModal → ProvidersTab + ModelsTab(拆 ModelRow/Form)│
│ │
│ transport/ (★ 07-20): httpTransport(默认) / tauriTransport(逃生)│
│ Pinia: streamController (单源) → chat → config/projects/... │
└────────────────────────────────────────────────────────────┘
 │ transport.invoke() / transport.listen()
 │ 默认 httpTransport: 同源 HTTP POST + SSE(→ daemon)
 │ 逃生 tauriTransport: Tauri IPC(Full 模式,GUI 进程内)
 ▼
┌─────────────────────────── 后端 ────────────────────────────┐
│ everlasting-daemon(axum,独立进程) / 或 Full 模式 GUI 进程 │
│ daemon/server.rs::build_router (108 个 command 镜像 REST 路由)│
│ ├─ commands/* + daemon/routes/* (同一 handler 双暴露 IPC+HTTP)│
│ ├─ agent/* → llm::provider::* → wire.rs + client.rs │
│ ├─ tools/* (28 个 builtin + read_guard + tool_output) │
│ ├─ db/* (CRUD by域 + migrations) │
│ ├─ git/* (worktree + diff) │
│ └─ projects/* (boundary + detector + store) │
│ sidecar.rs: GUI Thin 模式 spawn 此进程;HttpSseSink 广播事件 │
└────────────────────────────────────────────────────────────┘
```

###4.2跨层数据流

```
用户输入 → ChatInput → chat.send() → transport.invoke('chat')
 默认 httpTransport: fetch POST /api/v1/chat → daemon axum 路由
 逃生 tauriTransport: tauri.invoke('chat') → GUI 进程内 handler
 → agent::chat → resolve_chat_provider → Provider::chat_stream
 → BlockState(SSE) → HttpSseSink 广播 /api/v1/stream(Full 模式则 Tauri emit)
 → streamController (单源 transport.listen) → chat mutation
 → ChatPanel.vue渲染
```

---

## 5. Tauri IPC 表面

**总命令数**:108 个(2026-09-01 实测 `invoke_handler` 注册数,与 `#[tauri::command]` 属性数一致——旧口径 118 含注释/测试文件引用,已修正;06-10 快照 33 → 06-18 快照 54 → 06-24 ~60 → 07-23 快照 79 → 08-05 快照 91 → 08-13 remote epic 续增 → 08-14~28 增 F1 队列/F4 web_search/F5 提取/F6 busy/F2 定时任务 → 08-29~31 增审计 keyset 分页 + set_app_config_list → 09-01 增 update_project_sandbox_policy)。

> **daemon 化后双暴露(07-20 Q0 决策)**:这 108 个 `#[tauri::command]` handler 同时被 `daemon/routes/` 镜像为 REST 路由(`/api/v1/*`),前端默认经 `httpTransport` 走 HTTP,Full 模式逃生经 Tauri IPC。下表"文件位置"指 `#[tauri::command]` 定义处,REST 路由在 `daemon/routes/<同名>.rs`。

|域 | IPC 数 |文件位置 |
|----|-------|---------|
| Agent Loop |1 | `agent/chat.rs` (chat) |
| Cancel |1 | `commands/cancel.rs` |
| Config / remote / web_search / flags |12 | `commands/config.rs` (get_llm_config / get_home_dir / remote 5 项 / web_search 2 项 / get_app_config + set_app_config_flag + set_app_config_list) |
| Provider + Model + 测试 |12 | `commands/providers.rs` |
| Session 域(CRUD + model + metadata + trace 等) |19 | `commands/sessions.rs` |
| Permission + Audit + 模式 |10 | `commands/permissions.rs` (permission_response / set_session_mode / list_session_tool_permissions / revoke_tool_permission / list_session_audit_events(+_page keyset 分页,08-30)) |
| Project CRUD |9 | `commands/projects.rs` (含 browse_dir 目录浏览 + hide/unhide + update_project_sandbox_policy(09-01 P3c 三态写通道);09-03 下线 native pick_project_dir + tauri-plugin-dialog) |
| Memory |7 | `commands/memory.rs` |
| Worktree |4 | `commands/worktree.rs` |
| Subagent runs |4 | `commands/subagent_runs.rs` |
| Scheduled tasks |4 | `commands/scheduled_tasks.rs` (list/create/update/delete,08-28;per_run 08-31) |
| Question / mode change |4 | `commands/question.rs` |
| Message queue |3 | `commands/message_queue.rs` (list/remove/recall,08-25 F1) |
| Panel |3 | `commands/panel.rs` |
| Command palette |2 | `commands/command_palette.rs` |
| Task (workflow) |2 | `commands/task.rs` |
| Subagents |2 | `commands/subagents.rs` |
| Review (C2) |2 | `commands/review.rs` |
| Files |2 | `commands/files.rs` (list_files + list_files_at) |
| Usage (F6) |2 | `commands/usage.rs` (usage_window) |
| UI diff |1 | `commands/ui.rs` |
| Remote pairing |1 | `commands/pairing.rs` |
| Attachments (F5) |1 | `commands/attachments.rs` (save_attachment,08-27) |

**IPC命名**: Rust snake_case → Tauri2自动 camelCase转换给前端。

---

## 6. 数据库 schema

**位置**: `app/src-tauri/src/db/mod.rs::run_migrations`

**13 张实体表 + 2 张 FTS5 虚拟表**(2026-08-31 现状;原 06-10 快照 7 张 → 06-13 加 `session_audit_events` + `session_tool_permissions` + 06-20 B6 PR2 加 `subagent_runs` + 06-29 V2 2 期 加 `autonomous_memories` + 07-03 B6+ C 加 `subagent_model_overrides` + 07-14 E2 加 `turn_trace` + 08-28 F2 加 `scheduled_tasks`;FTS5 虚拟表 `autonomous_memories_fts` + `messages_fts`;08-11 remote 配置走 `app_config` KV,零 migration。注:`subagent_runs` 由 `widen_subagent_runs_status_check` 表重建模式创建,不在绿色建表段——08-28 同步曾写"14 张"系把 FTS 虚拟表计入的误数,每表一个 FTS 实为 13 实体 + 2 虚拟):

| 表 | 主键 |关键字段 |
|----|------|---------|
| `projects` | `id` (UUID) | `path` / `name` / `is_git_repo` / `git_remote` / `git_branch` / `is_hidden` / `sandbox_policy` (09-01 P3c:off/readwrite/readonly,默认 readwrite) |
| `sessions` | `id` (UUID) | `project_id` (FK) / `title` / `model_id` (FK, nullable) / `session_type` (chat/group_chat,07-29 群聊) / `worktree_path` / `worktree_state` / `current_cwd` / `created_at` / `updated_at` |
| `messages` | `id` | `session_id` (FK) / `role` / `content` (JSON) / `speaker` (群聊参与者标识,07-29) / `tool_use` (JSON) / `tool_result` (JSON) / `thinking_blocks` (JSON) / `metadata` (JSON, B2 PR3 加:含 `edited_at` / `original_content` 等 D3 字段) / `ttfb_ms` / `gen_ms` / `total_ms` (F5 latency 三列) / `thinking_ms` (F5) / `created_at` |
| `providers` | `id` | `name` / `protocol` / `base_url` / `api_key` / `enabled` |
| `models` | `id` | `provider_id` (FK) / `name` / `model_id` / `max_tokens` / `enabled` |
| `app_config` | `key` | `value` (JSON) |
| `session_audit_events` | `id` | `session_id` (FK, ON DELETE CASCADE) / `kind` (AuditKind 29 variant wire string——08-31 加 `SandboxedShellExecution`) / `tool_use_id?` / `path?` / `match_kind?` / `match_value?` / `mode?` / `risk?` / `ts` (RFC3339) |
| `session_tool_permissions` | `id` | `session_id` (FK) / `tool_name` / `match_kind` (`tool` / `prefix` / `path`) / `match_value` / `granted_at` |
| `subagent_runs` | `id` (UUID) | `parent_session_id` (FK, ON DELETE CASCADE) / `parent_request_id` (TEXT, soft FK) / `subagent_name` / `status` (CHECK 5 值) / `started_at` / `finished_at?` / `token_usage_json?` / `summary?` / `transcript_json?` / `transcript_truncated` / `turn_count?`(RULE-FrontSubagent-004 加) / `created_at` |
| `autonomous_memories` | `id` | `memory_id` (UNIQUE) / `scope` / `project_id` / `kind` / `status` (candidate/active/verified) / `title` / `content` / `tags` / `tool_name` / `command_pattern` / `path_globs` / `source_session_id` / `confidence` / `hit_count` / `last_used_at` / `demoted_reason`(★ V2 2 期,06-29) |
| `subagent_model_overrides` | `agent_name` (TEXT PK) | `model_id` / `updated_at`(★ B6+ C,07-03,builtin agent 全局 DB override,优先级 `DB > frontmatter > parent`) |
| `turn_trace` | `id` (INTEGER) | `session_id` (FK CASCADE) / `seq` / `token_usage_json` / `compaction_json` / `loop_hint_json` / `breadcrumb_json` / `created_at`(★ E2,07-14,UNIQUE(session_id, seq)) |
| `scheduled_tasks` | `id` (TEXT) | `project_id` (FK CASCADE) / `target_session_id` (FK CASCADE,08-31 起可空) / `target_mode` (CHECK fixed\|per_run,08-31 DEFAULT 'fixed') / `model_id` (TEXT,08-31 per_run 档指定模型) / `last_run_session_id` (TEXT,08-31 per_run 每次执行新建 session 落此) / `name` / `prompt` / `schedule` (JSON) / `enabled` / `created_by` / `created_at` / `last_fired_at?` / `next_fire_at` / `run_count` / `max_runs?` / `ends_at?`(★ F2,08-28;08-31 per_run 三档重建表,CHECK (target_mode='per_run' OR target_session_id IS NOT NULL)) |

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

## 7. Tauri IPC 事件表面

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

**设计决策**: 高频 token走 `chat-event`(避免 IPC调度开销);低频 tool call/result走独立事件名(前端可选择性 filter)。详见 `docs/IMPLEMENTATION/decisions.md`。

---

## 8. 关键设计模式

###8.1 流式处理单源(前端)

`streamController.ts` 是 **transport ↔ Pinia 的唯一入口**(daemon 化后默认 httpTransport,Full 模式 tauriTransport)。`chat.ts` 不直接监听 Tauri/SSE 事件。多 session 并发按 `request_id`路由,LRU20限制活跃请求。详见 `.trellis/spec/frontend/state-management.md`。

###8.1a Transport 抽象(★ 07-20 前端)

`app/src/transport/` 把 `invoke`/`listen` 与载体解耦:`httpTransport`(fetch POST + SSE EventSource,默认,连 daemon)/ `tauriTransport`(`@tauri-apps/api` 透传,Full 模式逃生)。`resolveTransport()` 读 URL `?transport=` 决定;`health.ts` 轮询 daemon health 必要时降级。所有 store 经 `transport.invoke` 而非裸 `@tauri-apps/api`,故同一前端既能跑 Tauri webview 也能跑纯浏览器。

###8.1b Sidecar + GuiMode(★ 07-22 后端)

`sidecar.rs` 定义 `GuiMode::{Thin,Full}`:`Thin`(默认)GUI 不加载 AppState/不开 DB pool,只 spawn `everlasting-daemon` 子进程(tauri-plugin-shell)并经 httpTransport 通信,`RunEvent::Exit` 钩子 kill sidecar;`Full`(`?transport=tauri` 或 `EVERLASTING_GUI_FULL_STATE=1`)是 legacy in-process 逃生。daemon 侧 `server.rs::serve_daemon` 绑 `0.0.0.0:7456`,`HttpSseSink`(sse.rs)广播事件,ServeDir 同源服务 SPA。

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

###8.8 执行期沙盒 (★ 08-31 P3b → 09-01 P3c/P3d)

沙盒档(sandbox_policy ≠ off,含 Plan 只读面)shell 命令在 `sandbox/`(Landlock ruleset + seccomp BPF)下 spawn;P3c 起 `resolve_policy` 三态决策链(capability → Yolo → 项目 off → kill-switch → Plan → 项目面)替代 P3b 的 ReadOnly 档 gate——默认 readwrite 档**全命令**进沙盒,`background_shell` 共用路径;面外失败走升级闭环(前台 `permissions/escalation.rs` Ask 卡 / 后台 `chat_loop/background_escalation.rs` EscalationOffer 下轮注入时 resolve,一次性不沙盒重跑)。详见 `.trellis/spec/backend/sandbox-executor.md`。

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

项目**不读任何 LLM 相关 env 变量**。provider / model / api_key / base_url 全部通过 UI Settings 配置,落盘到 DB catalog(`providers` / `models` / `app_config` 表)。历史上曾有 `ANTHROPIC_API_KEY` / `LLM_MODEL` 等 env 兜底路径,已在多 Provider catalog 架构落地后移除。

|变量 | 默认 |用途 |
|------|------|------|
| `RUST_LOG` | (无) | tracing级别(如 `debug`) |

###10.2 构建命令

| 命令 |用途 |
|------|------|
| `cd app && pnpm tauri dev` |启动 dev server(Tauri窗口;Thin 模式自动 spawn daemon sidecar) |
| `cd app && pnpm tauri build` |前端 type-check + build + Rust编译 +打包 |
| `cd app && pnpm dev` | 仅 Vite dev server |
| `cd app && pnpm build` | 仅前端 build |
| `cd app && pnpm test` | ★ 前端 vitest 单元测试(覆盖 `app/src/**/*.test.ts`) |
| `cd app && pnpm test:e2e` | ★ NEW (08-30 Playwright) — 浏览器交互回归(app/e2e/,CI blocking 门禁) |
| `cd app/src-tauri && cargo check` |快速 Rust编译检查 |
| `cd app/src-tauri && cargo test --lib` | Rust单元测试 |
| `cd app/src-tauri && cargo build --bin everlasting-daemon` | ★ 只编译 daemon bin(GUI sidecar 模式由 build.rs 自动 staging) |
| `cargo build -p everlasting --bin everlasting-daemon`(根) | ★ workspace(08-11)根目录构建 daemon(等价 cd app/src-tauri 后裸 `cargo build --bin everlasting-daemon`) |
| `cargo check` / `cargo test`(根,裸) | ★ 只作用于 default-members(remote 两 crate,不会碰 app) |
| `cargo check -p everlasting` / `cargo test -p everlasting --lib`(根) | 显式指定 app crate(等价 cd app/src-tauri 后裸命令;WSL 下 PKG_CONFIG_PATH 仍需) |
| `./scripts/daemon.sh start\|bg\|stop\|restart\|status\|logs` | ★ daemon 浏览器模式管理(详见 HACKING-wsl.md) |
| `./scripts/remote.sh start\|status` / `./scripts/deploy-remote.sh` / `./scripts/remote-e2e-smoke.mjs` | ★ NEW (08-11/13) — remote 服务端本地管理 / 云端部署 / E2E 冒烟(详见 REMOTE-DEPLOY.md + REMOTE-ACCESS-E2E.md) |

**Cargo.lock / target 位置(08-11 翻转)**:Cargo.lock 在仓库根(自 `app/src-tauri/Cargo.lock` 上移,锁定全 workspace);cargo 产物 `target/` 在仓库根。

###10.3 WSL特殊性

linuxbrew pkg-config覆盖系统路径、webkit2gtk-4.1 / gdk-pixbuf-2.0 系统库、CJK字体 HarmonyOS Sans SC 子集打包。详见 `docs/HACKING-wsl.md`。remote 两 crate(`crates/everlasting-remote*`)零系统库依赖(纯 Rust,无 gtk/webkit),无需 PKG_CONFIG_PATH,`cargo test -p everlasting-remote` 在 WSL 直接可跑。

---

##11. 测试与质量门

|层级 |框架 |覆盖范围 | 文件位置 |
|------|------|---------|---------|
| Rust单元测试 | `#[cfg(test)]` cargo test | sse / error / 部分 db / 部分 llm / wire.rs47% tests | `app/src-tauri/src/{llm,db,agent}/**` |
| 前端单元测试 | vitest | markdown + streamController11 it + lru + messageFormat + path | `app/src/utils/*.test.ts` + `app/src/stores/streamController.test.ts` |
| 前端类型检查 | `vue-tsc --noEmit` | 全 | `pnpm build` |
| 浏览器交互回归 (e2e) | Playwright (@playwright/test,08-30 RULE-TEST-001) | 三试点:chat-input-keys / permission-revoke-confirm / question-card-scroll,^1.62.1(devDep) | `app/e2e/`(fixtures.ts + 3 spec + README) |
|端到端(手动) |手动 | Tauri窗口实测 + remote 链路 `scripts/remote-e2e-smoke.mjs` | (手动必经,无自动化脚本) |

**质量门**: `vue-tsc --noEmit`(pre-build) / `cargo check`(dev) / `cargo test --lib`(可选,CI 未配) / `pnpm test:e2e`(Playwright,CI blocking 门禁) /手动端到端(必经)。

**缺口**:Rust集成测试少(浏览器层已由 Playwright 覆盖,见 `.trellis/spec/frontend/browser-regression.md`)。

---

## 12. 依赖与第三方集成

| 层 | 技术 | 版本 |锁定位置 |
|----|------|------|---------|
|桌面框架 | Tauri2 |2.x | `app/src-tauri/Cargo.toml` |
| 前端 | Vue3.4+ |3.4+ | `app/package.json` |
| 前端构建 | Vite |5.x | `app/package.json` |
|状态 | Pinia |2.x | `app/package.json` |
| UI组件 | reka-ui |2.9.9(锁精确) | `app/package.json` |
| 后端 | Rust1.75+ |1.96.0 | `app/src-tauri/Cargo.toml` |
| HTTP | reqwest |0.12 | `app/src-tauri/Cargo.toml` |
| Agent daemon | axum |0.7 | `app/src-tauri/Cargo.toml`(★ 07-20,HTTP framework + macros) |
| daemon 中间件 | tower + tower-http |0.5 / 0.6 | `app/src-tauri/Cargo.toml`(★ 07-20,ServeDir 同源 SPA + CORS + trace) |
| daemon 流 | tokio-stream |0.1(sync) | `app/src-tauri/Cargo.toml`(★ 07-20,BroadcastStream for SSE) |
| daemon CLI | clap |(derive) | `app/src-tauri/Cargo.toml`(★ 07-20,--port/--data-dir) |
| daemon spawn | tauri-plugin-shell |2 | `app/src-tauri/Cargo.toml`(★ 07-22,GUI spawn sidecar) |
|异步 | tokio |1.x | Tauri自带 |
| 数据库 | sqlx + SQLite | sqlx0.7 | `app/src-tauri/Cargo.toml` |
| Git | git2-rs |0.19 | `app/src-tauri/Cargo.toml` |
|错误 | anyhow + thiserror | 最新 | `app/src-tauri/Cargo.toml` |
|日志 | tracing |0.1 | `app/src-tauri/Cargo.toml` |
| Markdown | marked |18.0.5(锁精确) | `app/package.json` |
| Markdown 安全 | DOMPurify |3.4.8(锁精确) | `app/package.json` |
| 前端路由 | vue-router |4 | `app/package.json`(★ 08-11 remote) |
| PWA | vite-plugin-pwa |1.3.0 | `app/package.json`(★ 08-11 remote,manifest + SW) |
| 浏览器回归 | @playwright/test | ^1.62.1(devDep) | `app/package.json`(★ 08-30,Playwright 流水线,见 `.trellis/spec/frontend/browser-regression.md`) |
| 远程服务端 | everlasting-remote |(workspace 内) | `crates/everlasting-remote/Cargo.toml`(★ 08-11,零系统库依赖) |
| 远程协议 | everlasting-remote-protocol |(workspace 内) | `crates/everlasting-remote-protocol/Cargo.toml`(★ 08-11) |

**锁定位置说明(08-11 workspace 翻转)**:Rust 依赖锁定在根 `Cargo.lock`(自 `app/src-tauri/Cargo.lock` 上移);`app/src-tauri/Cargo.toml` 仍是 app crate manifest,新 crate 依赖在 `crates/*/Cargo.toml`;前端依赖仍在 `app/package.json`。

**已评估不引入**:
- ❌ `eventsource-stream`(手写 SSE,spike-002验证)
- ❌ `claude-agent-sdk` / `codex-sdk`(自研 agent core)
- ❌ `sea-orm` / `diesel`(手写 sqlx)
- ❌ `langchain` / `dspy-rs`
- ❌ `rig-core`(2026-06-09决策弃用,自研 Provider trait 已足够)
- ❌ `PyO3` / `Electron`

---

## 13. 文档地图 + 一页式 ASCII 全景

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
│ ├── DESIGN.md / TECH.md / BACKLOG.md / ROADMAP.md # ★ 技术路线图(单一 source of truth)
│ ├── CONTEXT.md # ★ 术语表 / BUGLIST.md # ★ 缺陷跟踪 / DEBUG_DB.md # ★ SQLite 直连调试
│ ├── HACKING-wsl.md / HACKING-llm.md / HACKING-markdown.md
│ ├── REMOTE-DEPLOY.md / REMOTE-ACCESS-E2E.md # ★ (08-11/13) — remote 服务端部署 + E2E 验收
│ ├── _history/ (统一历史归档) / spikes/
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
3. **ROADMAP.md**(看当前进度)
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
│ Tauri2(瘦客户端) + Vue3 + Rust + 自研 agent core + WSL-first │
│ daemon 化(07-20):agent core 在独立 everlasting-daemon 进程 │
│ │
│ ┌────────────────────┐ transport ┌──────────────────────────┐ │
│ │ Vue3 Frontend │ http(默认) │ everlasting-daemon │ │
│ │ (app/src/) │ /tauri(逃生)│ (axum,app/src-tauri/ │ │
│ │ · Pinia(30 stores) │◄─────────►│ src/daemon/ + bin/) │ │
│ │ · transport/ 抽象 │ │ ·108 commands→REST 路由 │ │
│ │ · stream1 source │ │ · Provider trait │ │
│ │ · reka-ui2.9.9 │ │ (Anthropic/OpenAI) │ │
│ │ · marked+DOMPurify │ │ · Tool registry (28) │ │
│ │ · Vue3.4+ │ │ · git2-rs worktree │ │
│ │ · BrowserHeader │ │ · sqlx + SQLite │ │
│ │ (浏览器模式) │ │ · HttpSseSink + ServeDir │ │
│ └────────────────────┘ └──────────────────────────┘ │
│ ▼ WSS 隧道(tunnel/,08-11) → crates/everlasting-remote 云服务端 │
│ (配对码 + auth + 反向代理 + SSE 桥 + 限速 + DB;移动 PWA 经 │
│ /api/v1/proxy + device_token 反向访问 PC daemon) │
│ sidecar.rs: GUI Thin 模式 spawn daemon;RunEvent::Exit 回收 │
│ │ │
│ ▼ │
│ ┌──────────────────────────┐ │
│ │ LLM APIs │ │
│ │ (Anthropic/OpenAI/...) │ │
│ └──────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘

代码: 3-crate workspace(根 Cargo.toml + Cargo.lock;app/ 前端 + src-tauri 后端含 daemon/ + tunnel/ + crates/everlasting-remote*) + scripts/(daemon.sh + remote.sh + deploy-remote.sh + remote-e2e-smoke.mjs)
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
|实施后决策变更 | docs/IMPLEMENTATION/decisions.md |

---

*本文件由 Step8-PR5 创建,基线 commit `0f9a167`;2026-07-23 同步至 `3307d93`(daemon 化);2026-08-05 同步至 `6449f16`(群聊 + review-viz + 交错思考);2026-08-13 同步至 `94828cb`(remote-control-epic-s1 + workspace 翻转);2026-08-31 同步(08-29~31 批:schedule_task 家族 / C6 截断统一 / ShellCard / keyset 分页 / Playwright e2e / Sandbox P3b / per_run。task `08-31-docs-sync-batch`);2026-09-01 同步(Sandbox P3c/P3d:三态配置 / Plan 只读面 / 前后台升级闭环,IPC 107→108,`projects.sandbox_policy` 列。task `09-01-a2-sandbox-docs-sync`)。下次重大重构后再次校准。*
