# CONTEXT.md

> Everlasting 项目术语表(glossary)。
> 本文件是 **glossary,只定义术语**;实现决策(schema / 写入时机 / 颜色阈值等)走 `docs/IMPLEMENTATION/decisions-2026-{06,07,08}.md` 决策日志(按月分卷,无 `decisions.md`),本文件不重复。
> 词条内的实现状态为**历史快照**(落地时记录),新进展/新特性只更新 [ROADMAP.md](./ROADMAP.md) 与决策日志,不在此追加;术语新增时才加词条。

---

## 术语表

### Turn (LLM turn)
一次 LLM HTTP 请求(Anthropic Messages API / OpenAI Chat Completions 一次 stream)。
一个用户消息可能引发 N 次 turn(主调 + tool_use 回填),受 agent loop `MAX_TURNS`(200)限制。

### TokenUsage
LLM 一次响应的 token 使用四元组(Anthropic schema 视角):

- **`input_tokens`** — 当次请求中送入的 token 数,**已包含** `cache_creation_input_tokens` + `cache_read_input_tokens`(Anthropic 语义)
- **`output_tokens`** — 当次响应生成的 token 数
- **`cache_creation_input_tokens`** — 当次请求中**新创建**的 cache token(下次可命中)
- **`cache_read_input_tokens`** — 当次请求中**命中**的 cache token

OpenAI Chat Completions 的归一化映射(在 Provider 层完成,ChatEvent 出来时已统一):

- `prompt_tokens` → `input_tokens`
- `completion_tokens` → `output_tokens`
- `prompt_tokens_details.cached_tokens` → `cache_read_input_tokens`
- `cache_creation_input_tokens` → `0`(OpenAI 暂无对应字段)

### Context Pressure (上下文压力)
**当前 context 窗口的占用比例**。定义为:

- 分子 = session 累计 `input_tokens`(sum over turns)
- 分母 = `ModelRow.context_window`(默认 200K)

`input_tokens` 已包含 cache_creation + cache_read,所以 cache 命中**不重复计**——使用 cache 会让压力增长更慢。`output_tokens` **不计入** context 压力(那是响应,不是 context)。

### Cache Hit (cache 命中)
LLM 一次请求中,从 prompt cache 读回的 token(`cache_read_input_tokens`)。计费按 Anthropic / OpenAI 各自规定(Anthropic `cache_read_input_tokens` 按 0.1x input 价;OpenAI `cached_tokens` 按 0.5x input 价)。

### Context Window (上下文窗口)
LLM 模型能处理的最大 input token 数(Anthropic Sonnet / Opus 默认 200K)。数据来源:`ModelRow.context_window` 列,seed 时硬编码。

### Per-session 累积 (Token 统计颗粒度)
Token 统计在 DB 层的存储颗粒度为 session 维度:`sessions` 表的 4 列(input_tokens_total / output_tokens_total / cache_creation_total / cache_read_total)。每次 LLM turn Done 时单条 SQL UPDATE 累加。

### Anthropic SSE Usage
Anthropic Messages API 的 token 用量在 SSE 流的 `message_delta` 事件中携带(`usage: { input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens }`),累计语义,本 turn 累计。

### OpenAI Stream Usage
OpenAI Chat Completions 的 token 用量在流末尾携带(`usage: { prompt_tokens, completion_tokens, total_tokens, prompt_tokens_details: { cached_tokens } }`),**仅在请求体发送 `stream_options: { include_usage: true }` 时**返回。

### Checklist (agent 自跟踪清单)
> **实现状态**:**B12 已落地(2026-06-19)**。实现细节(注入机制 / 前端组件 / DB 表)见 [ROADMAP §1.2 B12 行](./ROADMAP.md) + [决策日志 2026-06-18](./IMPLEMENTATION/decisions-2026-06.md),此处只留定义。

LLM 在跑复杂多步任务时维护的**结构化进度清单**——agent 自己写、改、标记完成,用于不丢失自己的计划与进度。对齐 Claude Code 的 `TaskCreate/TaskList`、opencode 的 `todowrite`、Cline 的 plan-act。

**不是什么**(本项目内这几个词都已占用,需消歧义):
- **不是** Trellis task(`.trellis/tasks/`,dev-workflow 的 PRD / 排期任务)
- **不是** plan mode(`Mode::Plan`,权限模式,拒 tool_use)
- **不是** subagent(B6,main agent 派 worker agent,独立 context + summary 回填)

典型形态:agent 在一个任务的多 turn run 中反复更新它,每轮把当前清单重新注入 context,从而"看到自己还剩什么没做"。

---

### Subagent / dispatch_subagent
父 session 通过 `dispatch_subagent` tool 派 worker agent 跑独立任务。worker 拥有**独立 context + token 预算**,完成 / 取消 / 失败后回填 summary。实现细节见 [ROADMAP §1.2 B6/L3a-d 行](./ROADMAP.md#12-路线图外完成) + [决策日志 2026-06-18/20/21](./IMPLEMENTATION/decisions-2026-06.md);`app/src-tauri/src/agent/subagent/dispatch.rs` 实现 `run_subagent`。

### SubagentRun
`subagent_runs` 表一行(`db/migrations/schema.rs` 的 subagent_runs 段),完整 schema: `id` / `parent_session_id` / `parent_request_id` / `subagent_name` / `status` / `started_at` / `finished_at` / `task` / `final_text` / `summary` / `turn_count` / `token_usage_json` / `transcript_json` / `transcript_truncated` / `worktree_path` / `isolation`(L3b PR1 起)。

- **status** ∈ `{running, completed, cancelled, error, incomplete}`(终态 4 个,无 `failed`)
- `transcript_json` 持久化整段 transcript + `transcript_truncated` 哨兵(超限截断)
- app 启动时 `reap_orphaned_runs` 把上一进程崩溃留下的 `running` 标记为 `error`(防止假 running)
- **B1 关系(2026-08-16)**:SubagentRun 本身无新列,但 worker subagent 处理的图片 attachment 落 `messages.metadata.attachments[]` JSON 引用(父 session `parent_session_id` 共享,worker 与 parent 共用同一 `messages` 主表),通过 `parent_session_id` 反查可拿到 worker 期间产生的所有 messages / metadata / attachments

### Worker Worktree
L3b PR1-PR4(L3b = subagent isolation 维)落地的 worker 隔离机制(branch 前缀 `worker/<run_id>` / `git worktree lock` 跑期间 / merge·discard·sweep 生命周期)。机制细节见 [ROADMAP §1.2 L3b 行](./ROADMAP.md#12-路线图外完成) + [决策日志 2026-06-27/28](./IMPLEMENTATION/decisions-2026-06.md)。

### BackgroundShell (L1a 后台 shell)
`run_background_shell` 启动后台 shell(tokio Child,**不带 PTY**,L1b follow-up 接 `portable-pty`),`shell_status` 拉 exit_code,`shell_kill` 终止。实现细节(Registry trait / 三触发 `select!` / Q1-Q7 决策 / 生命周期钩子)见 [ROADMAP §1.2 L1 行](./ROADMAP.md#12-路线图外完成) + [决策日志 2026-06-19](./IMPLEMENTATION/decisions-2026-06.md)。

### MAX_TURNS
当前常量 `200`(`app/src-tauri/src/agent/mod.rs:76`)。Agent Loop 单 request 内最大 turn 数。实现状态见 [ROADMAP §1.2 softcap 行](./ROADMAP.md#12-路线图外完成) + [pattern-turn-limit-softcap](../.trellis/spec/backend/agent-loop-architecture/pattern-turn-limit-softcap.md);变更轨迹 `20 → 50 → 200`。

### Context Compression Thresholds (C3+ 替代 C3,2026-08-18)
原 C3 阈值(`context_window * 0.80` 触发,降到 `0.50`,B5 memory 永远保护,2026-06-12 落地)已被 **C3+ LLM 摘要式压缩(2026-08-18)** 替换。当前实现:0.85 触发 → LLM 9 段模板摘要 + 保留区存活 + `cutoff_seq` 水位折叠,摘要行落 `messages` 表 `metadata.kind = "compaction_summary"`;连续 3 次失败熔断回退 C3 机械丢组;实现细节见 [ROADMAP §1.2 C3+ 行](./ROADMAP.md#12-路线图外完成) + [ARCHITECTURE §2.5.5/§2.5.13](./ARCHITECTURE.md)。

### Loop Detection (C2 循环检测)
**分级触发**取代原文单一 0.9 阈值,因单一阈值无法适配短 / 长 input:

- **L1 精确签名硬触发**(N=3):同一 tool_use 签名(含 `edit_file` 的 `old_string` 避免正当多块编辑误判)连续 3 次,直接打断 loop
- **L2 Jaccard 软提示**(N=5,0.85):最近 5 次 tool_use 的 input 集合 Jaccard ≥ 0.85 时,**hint 作为 `ContentBlock::Text` 注入 result message**,LLM 下一轮看到提示,**不跳过执行、不终止 loop**

撞线兜底见上节 MAX_TURNS(2026-08-19 起软卡询问,非硬停)。实现见 `app/src-tauri/src/agent/loop_detection.rs`;C2+ 主动干预(每 run `loop_hit_count` N=3 + 三分支询问)见 [ROADMAP §1.2 C2+ 行](./ROADMAP.md#12-路线图外完成)。

### AuditKind
`session_audit_events.kind` 字符串枚举,**2026-08-20 实测 27 类**,按域分组(完整 27 variant 列表见 `app/src-tauri/src/agent/permissions/audit.rs` + [ARCHITECTURE §2.5.8](./ARCHITECTURE.md)):

- **Tool 域(5)**:ToolDenied / ToolAllowed / ToolPermissionAsk / ToolExecuted / ToolDeniedYolo
- **Permission 域(3)**:PermissionGranted / PermissionTimeout / RequestCancelled
- **Mode 域(6)**:ModeChanged / YoloEntered / YoloExited / ModeChangeRequested(07-07 request_mode_change 工具)/ ModeChangeAllowed / ModeChangeDenied
- **Message 域(2)**:EditMessage(D3 PR1 06-17)/ ResendMessage(D3 PR3 06-17)
- **Loop 域(2)**:LoopIntervention(C2+ 07-05 主动干预)/ TurnLimitSoftcap(08-19 MAX_TURNS 软卡询问)
- **Worker 域(4)**:WorkerAskAllowed / WorkerAskDenied / WorkerAskTimedOut / WorkerAskCancelled(L3b 06-22 RULE-FrontSubagent-003 fix)
- **TaskStateTransition 域(3)**:TaskStateTransitionRequested / Allowed / Denied(07-08 workflow Phase 3 Step 3.1)
- **Budget 域(1)**:ContextBudgetTrim(08-19 关卡⑤硬卡裁剪,unified-context-budget)
- **UI 域(1)**:UiDiffApplied(B9+ D4 07-13 apply_ui_diff IPC 成功)

每类 payload_json 结构不同;`record_tool_executed_audit` 落 `tool_executed` 的 `{tool_name, tool_input, duration_ms, exit_code}`。查询走 `list_session_audit_events` Tauri command + 前端 `useAuditStore` + `<AuditLogModal>`(reka-ui Dialog);E2 07-14 起增 `turn_seq` 列(审计按 turn 落表)。

### L1 / L2 / L3 命名约定
路线图子档命名,2026-08-20 同步(新特性加入新字母)。完整逐项状态见 [ROADMAP §1.2](./ROADMAP.md#12-路线图外完成) 对应行 + [CONTEXT 词条](./CONTEXT.md) 对应术语:

- **L1**:后台 shell + 完成通知(L1a 不带 PTY / L1b 后续接 portable-pty)
- **L2**:单 turn 多 tool 并发(只读 batch,`is_parallel_eligible` + `FuturesUnordered`)
- **L3**:Subagent 三层:
  - **L3a**:并发只读 dispatch(`force_readonly` 剥写 + `DELEGATION_MAX_CONCURRENT_CHILDREN` 默认 3)
  - **L3b**:worker worktree 隔离(PR1 serial 核心 / PR2 concurrent 解锁 / PR3 merge_worker + discard_worker + sweep / PR3+ permission+concurrency hardening / PR4 前端合并/丢弃 UI)
  - **L3c**:worker 联网(`SubagentDef.tools` + `READONLY_TOOL_ALLOWLIST` 加 `web_fetch`)
  - **L3d**:frontmatter loader(`~/.config/everlasting/agents/*.md` + `<project>/.everlasting/agents/*.md`,数组 frontmatter 解析已落地)
- **C7** (2026-08-14):tools token 治理 — `STUB_CANDIDATES` 静态裁剪 + `turn_trace.tools_token` 度量
- **C7D** (2026-08-14):tools stub 注册 + 元工具按需取回 — `StubRegistry` + `load_tool_schemas` 元工具 + `tools_stub_enabled` gate
- **C3** (2026-06-12,已废):机械丢组 0.80→0.50,被 **C3+** 替换
- **C3+** (2026-08-18):LLM 摘要式压缩(见上 Context Compression Thresholds 节)
- **memory-gov** (2026-08-15):指令块窗口治理 — `memory/digest.rs` 切节注入 + `load_memory_sections` 元工具 + `turn_trace.memory_token` 度量
- **B1** (2026-08-16/17):image multimodal — `ContentBlock::Image` / `ImageRef` 双形态 + `models.supports_images` + `messages.metadata.attachments[]` + 二进制 GET 路由
- **D2①** (2026-08-17):跨 session 全文搜索 — `messages_fts` FTS5 + `db/search.rs` 双路分派 + `SearchModal`
- **D2②** (2026-08-17):agent `search_history` tool(`READONLY_TOOL_ALLOWLIST` 第 6 员,薄封装 `db::search`)
- **E2** (2026-07-14):turn-level harness trace — `turn_trace` 表 + 4 个 ChatEvent + `session_audit_events.turn_seq` 列
- **B11** (2026-08-11~13):远程遥控通道 — `crates/everlasting-remote` 云中继 + PC tunnel client + 手机 PWA + Cargo workspace 翻转
- **unified-context-budget** (2026-08-19):统一 token 预算 — `turn_trace` 三新列 + 关卡⑤硬卡裁剪引擎
- **softcap** (2026-08-19):MAX_TURNS 软卡(见上 MAX_TURNS 节)
- **compact 命令** (2026-08-19):手动 `/compact` — 空闲期 LLM 摘要压缩入口
- **handoff** (2026-08-19):跨 session 接力 — HUD 按 session 隔离 + 接力摘要进下一 session
- **worker per-turn trace** (2026-08-20):`turn_trace` 并入 run 维度(`UNIQUE(session_id, run_id, seq)`,`''` 哨兵主行)

### daemon 化进程模型(07-20~23 remote-access epic 落地)

agent core 从 Tauri GUI 进程拆出为独立 daemon 进程后引入的术语。详见 [ARCHITECTURE §1/§4](./ARCHITECTURE.md)。实现细节(phase 拆分 / transport 抽象 / SSE / sidecar 管理)见 [ROADMAP §1.2 "daemon 化" 行](./ROADMAP.md#12-路线图外完成) + [决策日志 2026-07-20~23](./IMPLEMENTATION/decisions-2026-07.md)。

- **everlasting-daemon** — cargo bin target(`app/src-tauri/src/bin/everlasting-daemon.rs`),跑 agent core 的独立进程。axum HTTP server,监听 `0.0.0.0:7456`,持有 SQLite pool(WAL writer)。
- **sidecar** — GUI 进程(Tauri)把 daemon 作为子进程 spawn 出来的模式(`sidecar.rs::spawn_and_manage`)。`RunEvent::Exit` 钩子 kill sidecar,无孤儿进程。spawn args:`--port` + `--data-dir`(对齐 GUI 的 `app_data_dir`,保证开同一个 SQLite)。
- **GuiMode**(`sidecar.rs` 枚举)—— GUI 运行模式:
  - **Thin**(默认):GUI 不加载 `AppState`、不开 DB pool、不跑 sweep/hygiene;只 spawn daemon sidecar 并经 `httpTransport` 通信。
  - **Full**(`?transport=tauri` 或 `EVERLASTING_GUI_FULL_STATE=1`):legacy in-process —— GUI 加载 `AppState` + 走 Tauri IPC,不 spawn sidecar。daemon 故障时的逃生舱。
- **transport 抽象层**(`app/src/transport/`)—— 前端把 `invoke`/`listen` 与载体解耦:
  - **httpTransport**(默认):fetch POST 到 daemon `/api/v1/*` + EventSource 订阅 `/api/v1/stream` SSE。Tauri webview 和纯浏览器都用它。
  - **tauriTransport**(逃生):`@tauri-apps/api` 的 `invoke`/`listen` 透传,仅 Full 模式。`?transport=tauri` URL query 触发。
  - `resolveTransport()`(`index.ts`)按 URL query 选;`health.ts` 轮询 daemon health 必要时降级。
- **HttpSseSink**(`daemon/sse.rs`)—— agent loop 的事件广播出口:把 `ChatEvent`(`chat-event`/`tool:call`/`tool:result` 等)经同源 SSE 推给前端。Full 模式下对应 Tauri `app.emit`。
- **ServeDir**(`tower-http`)—— daemon 同源服务前端 `dist/` SPA 的 fallback,使纯浏览器访问 `http://localhost:7456/` 直接拿到前端(浏览器模式)。
- **浏览器模式** — 无 Tauri 运行时的纯浏览器访问形态。前端 `isTauriWebview()`(`transport/env.ts`)=false 时用 `BrowserHeader.vue` 替代 `TitleBar.vue`。管理脚本 `scripts/daemon.sh`。
- **handler 双暴露(Q0 决策)** —— **2026-08-18 实测 95** 个原 `#[tauri::command]` handler 同时被 `daemon/routes/` 镜像为 REST 路由;同一份 handler 代码既服务 Tauri IPC 又服务 HTTP,代码复用不分裂。
- **everlasting-remote** — 独立二进制(`crates/everlasting-remote/` + `crates/everlasting-remote-protocol/`,2026-08-11 workspace 翻转后为 workspace members),云端 axum 服务端(国内 2C2G 服务器,nginx 反代 HTTPS)。shared_secret auth(防伪 daemon)+ device_token 认证;配对码 60s 一次性 + per-IP 限速;WSS 隧道服务端 + 反向代理 + SSE 桥;DB `nodes` / `devices` / `pairing_codes` 三表。只存 token/devices/配对码,**不存 agent 数据**;PC daemon 本地功能零依赖 remote。
- **tunnel client / TunnelManager**(`app/src-tauri/src/daemon/tunnel/`,子模块 client / config / dispatcher / manager / node_id / sse_bridge)—— PC daemon 侧出站 WSS 长连接 + loopback 转发,把云上 remote 的请求转发到本地 agent core。取消只停转发(`sse_bridge` 的 `select!`),不终止本地会话。
- **node_id** — PC daemon 在 remote 上的节点身份(`devices` 表),WSS 长连接与 `/api/v1/proxy` 按 node_id 路由。
- **配对码 / device_token** — bootstrap 凭据:PC Remote tab 生成 6 位配对码(60s 一次性),手机 PWA redeem 后换 64-hex `device_token`;此后经 `Authorization: Bearer` + SSE `?access_token=` 访问。
- **pwa-remote 模式** — `httpTransport` 内部第三态:前端持有 `device_token` 时请求加 `/api/v1/proxy` 前缀 + Bearer,SSE 带 `access_token`;vue-router 守卫仅 remote-served 语境 gate 配对页。PWA 壳:vite-plugin-pwa + `public/icons/`。
- **Settings RemoteTab / remoteConfig store** — 前端远程设置入口(`app/src/components/settings/RemoteTab.vue` + `stores/remoteConfig.ts`),GUI 侧配置 remote 隧道相关状态。

---

## 相关决策

- 设计决策走 [`docs/IMPLEMENTATION/decisions-2026-{06,07,08}.md` 决策日志](./IMPLEMENTATION/)(按月分卷,无 `decisions.md`;本月新建条目落 `decisions-YYYY-MM.md` + 更新 `[ARCHITECTURE.md](./ARCHITECTURE.md)` 对应章节)
- A4 Token 相关术语、Checklist(agent 自跟踪清单)均已落地(详见上文 Checklist 条目,B12 2026-06-19),作为术语定义保留
- 跨层契约走 `.trellis/spec/backend/llm-contract.md` "Scenario: Token Usage Tracking" 段
