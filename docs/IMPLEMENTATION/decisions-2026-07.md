### 2026-07-28 — subagent resume (C1) + TaskStatus 自定义 plugin state (C0)

- **Context**:review epic 前置基建两缺口:① review 用自定义 workflow plugin,但 `TaskStatus` 硬编码 builtin dev workflow 的四态(Planning/Implement/Check/Done),不支持 plugin 自定义状态;② subagent run 中断后无法续跑,长任务断点重跑成本高。
- **决策 C0 — TaskStatus accommodate custom plugin state**:`TaskStatus` 扩展支持非 builtin workflow plugin 的自定义状态字符串(plugin 自带状态机定义)。
- **决策 C1 — resume mechanism for worker runs**:中断的 subagent run 可续跑(保留 run_id + 已完成 turn 的 transcript,从中断点继续)。Merge `feat/subagent-resume-c1`(C0 `e1afa67` + C1 `703ab7d`)。


### 2026-07-26 — C2 review-state 可视化 + lefthook pre-commit

- **Context(Review 可视化)**:review plugin 产出 `.everlasting/outputs/review-state.json`(维度×发现矩阵 + 三态:pending/approved/rejected),但前端无可视化,用户只能读 JSON。
- **决策 — 前端矩阵视图 + tool:call 路由刷新**:新增 4 组件(`<ReviewMatrix>` / `<ReviewMatrixGrid>` 维度×发现网格 / `<ReviewFindingDetail>` / `<ReviewDimensionCompare>`)+ `reviewState.ts` store(三态载荷);刷新机制复用 `streamController` 的 `tool:call` 路由(review plugin 写完 review-state.json 后调一个 tool 触发前端重读,无新 backend event)。后端 `commands/review.rs`(get_review_state + get_current_task_slug,3 IPC)+ `daemon/routes/review.rs` 双暴露。
- **Context(lefthook)**:cargo fmt 未跑 / pnpm-lock.yaml 漂移反复进仓,CI 才发现。
- **决策 — lefthook pre-commit 拦截**:引入 `lefthook.yml`,pre-commit 阶段跑 `cargo fmt --check` + lockfile 同步检查,堵问题进仓。spec 沉淀为强制约定(`.trellis/spec/`)。


### 2026-07-26 — ask_user_question allow_custom + skip-semantics

- **Context**:`ask_user_question` 原只能让用户在 LLM 给定的 options 里选,无法接受自由输入;且回灌语义不清(用户跳过 vs deny 都是 `is_error: true`)。
- **决策 — allow_custom 选项 + skip-semantics**:加 `allow_custom` 字段(为 true 时用户除预设 options 外可自由输入文本);区分 skip(自由输入 / 跳过,`is_error: false`)vs deny(拒绝,`is_error: true`),让 LLM 能据语义决定下一步。任务 `07-29-ask-user-question-custom-input/`(archive)。


### 2026-07-25 — daemon graceful shutdown 加固

- **Context**:SIGTERM 硬终止 daemon 时,agent loop 可能还在跑(in-flight LLM 请求 + tool 执行),直接退出导致:SSE 长连接客户端卡住等 SIGKILL、agent loop 资源泄漏、sidecar 孤儿进程。
- **决策 — 三层 graceful shutdown**:① `serve_daemon` 收到 shutdown signal 后主动 cancel + drain agent loop(等当前 turn 收口,而非立即死);② 主动关 SSE 长连接(`HttpSseSink` graceful,客户端立即感知断开而非超时);③ sidecar 孤儿进程清理(`RunEvent::Exit` 钩子兜底 kill)。修复 `serve_daemon` 3s 自杀回归(过早返回导致 sidecar 误判 daemon 已死)。另:`f95d5ff` 持 SIGNAL_TEST_MUTEX 防进程级信号竞争假性测试失败。


### 2026-07-23/24 — 交错思考渲染(contentBlocks 真实流序)

- **Context**:Anthropic Messages API 的 SSE 流里 thinking / text / tool_use block 按**真实到达序**交错出现,但旧实现落库时按"先 text 后 tool_use"分组,导致 thinking 块在中途消失 + 工具无法在思考之间穿插执行。
- **决策 — 后端保留真实流序 + 前端 run 分组时间轴**:`chat_loop.rs` 落库时保留 BlockState 时间戳序(thinking/text/tool_use 按 SSE 到达序),前端按 run 分组 + contentBlocks 时间轴交错渲染。修复 Anthropic thinking 消失 + 实现真·工具穿插(工具在思考之间执行)。
- **影响**:设计文档沉淀为 [docs/INTERLEAVED-THINKING-DESIGN.md](../_history/2026-08-28-interleaved-thinking-design.md)(含评审 triage 修订)。3 commit:后端落库(`ba1eeca`)+ 前端 run 分组(`5b1fc81`)+ 实时流序交错渲染(`78d7ec7`)+ 修复 thinking 消失(`8daaf23`)。


### 2026-07-20 — Agent daemon 化 + HTTP/SSE transport(项目迄今最大架构变更)

**Context**: 截至 2026-07-17,agent core(Tauri command handler + `AppState` + agent loop + SQLite pool)全跑在 Tauri GUI 进程内,前端唯一经 `invoke`/`listen` 直连。两件事把它顶到必须拆:(1) **远程访问 / 浏览器模式需求** —— WSL 跑 agent core、Windows 宿主浏览器访问,是本项目的核心使用场景,而 Tauri webview 绑死单进程,浏览器无法直连;(2) **agent core 与 GUI 解耦** —— GUI 崩溃/重启会带走在途 agent loop,且 GUI 二进制随 Tauri/WebKitGTK 体积庞大、启动慢。daemon 化把 agent core 拆成独立 `everlasting-daemon` 进程(axum HTTP + 同源 SSE + `ServeDir` 服务 SPA),前端引入 transport 抽象层(`httpTransport` 默认 / `tauriTransport` 逃生舱),新增 sidecar spawn + 浏览器模式。**这不是 B10 飞书触发的**(本节下方 2026-06-10 路线图快照把 B10 标「触发 daemon 化」是当时预期,实际触发源是远程访问需求,且已于 2026-07 落地)。commits `0dbc747`(transport 抽象,Phase 1)→ `5a212f0`(P2.1+P2.2 axum server)→ `f2a675b`(P2.3 SSE)→ `84d4689`(P2.4 sidecar+ServeDir+默认 httpTransport)→ `e6b7a2f`(P2.5 E2E)→ 手动测试修复 `ba41c1d`/`6581257`/`16548fd`/`df991a5`/`a2bd611`,~15K 行。任务档案 `.trellis/tasks/archive/2026-07/07-20-remote-access-daemon-split/`(design.md/implement.md/prd.md)+ `.trellis/tasks/archive/2026-07/07-20-remote-access-transport-abstraction/`。

**关键决策(7 个,按"为什么")**:

1. **为什么拆 daemon · agent core 与 GUI 解耦(Q0)**:`AppState`(SQLite pool + agent loop + LLM provider)从 Tauri GUI 进程抽出为独立 `everlasting-daemon` bin。GUI 进程在 Thin 模式(默认)**不开 `SqlitePool`**(`sidecar.rs`:Thin 模式 GUI does NOT load `AppState`/does NOT open pool),agent core 全在 daemon。**否决**「继续 in-process」—— 远程访问 / 浏览器模式无解;**否决**「只做 transport 抽象不拆进程」(Phase 1 `0dbc747` 只抽象前端 emit/invoke 散点,未动进程模型)—— 不拆进程浏览器仍无法连。
2. **为什么 axum(而非 actix / 裸 hyper / tide)**:选 `axum 0.7`(Tower-on-Hyper,`macros` feature)。理由:① Tower 生态(`tower 0.5` + `tower-http 0.6` 的 ServeDir / CorsLayer / TraceLayer)即插即用,P2.4 ServeDir 服务 SPA、P2.3 SSE 复用 `tower-http::services` 几乎零成本;② `#[axum::macros]` 的 handler 宏让 79 个 handler 镜像 `#[tauri::command]` 写法,迁移机械;③ 与 tokio runtime 天然集成(daemon `#[tokio::main]`)。**否决** actix(actor 模型心智负担与本项目无业务 actor 不匹配)+ 裸 hyper(要手写路由/提取器/中间件,79 handler 成本过高)。
3. **为什么 sidecar spawn(GUI spawn daemon,Q0 决议)而非 systemd / 用户手动起**:GUI 用 `tauri-plugin-shell` spawn `everlasting-daemon` sidecar(prod),`concurrently` 同起两者(dev)。理由:① 单二进制部署 —— 用户装一个 Tauri app 即得 GUI+daemon,无需配 systemd unit;② 生命周期绑定 —— 关 Tauri 窗口 `RunEvent::Exit` 钩子自动 SIGTERM sidecar,不留孤儿进程;③ `--data-dir` 让 GUI 把 Tauri-resolved `app_data_dir` 显式传给 daemon,DB 路径精确对齐。**否决** systemd(WSL 主环境无 systemd 开箱即用,跨平台部署摩擦大)+ 用户手动起 daemon(认知负担 + PID 管理无人做)。`scripts/daemon.sh`(commit `a2bd611`)给裸跑场景补了 PID 文件管理 + 多实例保护。
4. **为什么默认 httpTransport(而非保留 Tauri IPC 默认)**:前端 transport 抽象层 `isTauriWebview() ? tauriTransport : httpTransport`,GUI 默认 Thin 模式走 httpTransport(同源 sidecar)。逃生舱 `?transport=tauri` 强制 Full 模式 —— GUI 自己 `AppState::load` 开 pool,daemon 不 spawn,走原 in-process。理由:浏览器模式是核心场景,默认必须通;但 daemon 化初期不稳时需一键回退到验证过的 in-process 路径(dogfooding 2 周期,见 design §6.2)。**否决**「保留 Tauri IPC 默认」—— 那浏览器模式永远默认不通,违背拆 daemon 初衷。
5. **为什么 ServeDir 同源(而非独立 nginx / 双端口)**:daemon `tower-http::services::ServeDir` 直接 serve `dist/`,API + SPA 同源(`http://localhost:7456`),浏览器零 CORS 配置。dev 模式才前后端分离(vite 1420 + `?daemonUrl=` 跨域,daemon `CorsLayer::very_permissive` 放行)。**否决**独立 nginx 反代(多一个进程 + 配置)+ 双端口(浏览器跨域 + cookie/SSE 复杂度)。`resolve_dist_dir()` 从 `current_exe()` 向上搜 `src-tauri/` 取兄弟 `dist/`(适配 sidecar binaries/、target/release、target/debug 三种二进制位置,commit `6581257`)。
6. **为什么 handler 双暴露(IPC + HTTP,Q0 决议)**:79 个 `#[tauri::command]` 的业务逻辑抽成 `xxx_inner(state: &Arc<AppState>, ...)` 单份实现,Tauri 入口和 axum handler 都调 `_inner`,无 duplication。**否决**「Tauri 入口立即废弃」—— 双入口并行期(P2.1→P2.4)让每阶段可独立回滚,Full 模式逃生舱也依赖 Tauri 入口存活;待 dogfooding 稳定后再 archive。**否决**「强制抽 `crate::service::*` 层」—— 拿到复用又不付跨模块搬迁 + 多一层抽象成本,`_inner` 函数留在原 command 文件足够。
7. **DB 路径对齐(Q0 + commit `16548fd`)**:daemon `resolve_data_dir()` = `dirs::data_dir().join(EVERLASTING_APP_IDENTIFIER)`,`EVERLASTING_APP_IDENTIFIER` 由 `build.rs` 从 `tauri.conf.json` 读出编译期 `env!()` 注入,与 Tauri `app.path().app_data_dir()`(= `dirs::data_dir().join(config.identifier)`)对齐到同一个 `dev.everlasting.app/everlasting.db`。**坑**:修前 daemon 用 `dirs::data_dir().join("everlasting")`(无 `dev.` 前缀),裸跑打开空 DB 丢失 GUI 的历史消息(孤儿 DB,详见 [DEBUG_DB §1.0](../DEBUG_DB.md#10-daemon-化后的三条解析路径2026-07-同步))。2 个单测锁住 identifier 拼接(`resolve_data_dir_ends_with_app_identifier` / `resolve_data_dir_not_legacy_hardcoded`)。(`resolve_data_dir_ends_with_app_identifier` / `resolve_data_dir_not_legacy_hardcoded`)。

**Consequences**:
- 远程访问 / 浏览器模式打通:WSL 跑 daemon + Windows 宿主浏览器 `http://localhost:7456` 经 WSL 2 localhost forwarding 直达,完整功能(发消息 / 流式 / permission / subagent)。
- GUI/daemon 解耦:GUI 崩溃不再带走 agent loop(daemon 独占 SQLite WAL writer);Thin 模式 GUI 是瘦客户端。
- 双入口并行期:Tauri 入口保留,`?transport=tauri` Full 模式作逃生舱,dogfooding 2 周期内 daemon 不稳可一键回退。
- 新增依赖:axum 0.7 + tower 0.5 + tower-http 0.6 + tokio-stream 0.1 + clap 4(`Cargo.toml`,daemon bin 是唯一 clap 消费者,lib 不依赖)。
- 已知后续项:daemon graceful shutdown 在有浏览器 SSE 长连接时收到 SIGTERM 后挂起(等连接完成),靠 `scripts/daemon.sh` SIGTERM→8s→SIGKILL 兜底,不影响使用。
- E4 手动 smoke + E5 dogfooding(≥2 周计时)留运行期验证。

**关联**: PRD + design + implement `.trellis/tasks/archive/2026-07/07-20-remote-access-daemon-split/`;transport 抽象前置任务 `.trellis/tasks/archive/2026-07/07-20-remote-access-transport-abstraction/`;Phase 编排 `docs/REMOTE-ACCESS-ROADMAP.md`;调研 `docs/_archive/2026-07-20-remote-access-research.md`;手动测试 `docs/_archive/2026-07-23-manual-test-p2.md`;WSL 部署 `docs/HACKING-wsl.md` §远程访问 daemon 部署;DB 路径 `docs/DEBUG_DB.md` §1.0。commits 见上 Context。


### 2026-07-14 — E2 turn-level harness trace viewer(后端 trace 管道 + 前端独立面板)

- **Context**:ROADMAP 第三档 E2("per-turn 决策时间线 / harness 学习教具")。调研确认 4 维度(C3 压缩 / per-turn token / C2 soft hint / workflow breadcrumb)里只有"工具执行+延迟"有完整 per-turn 持久化,其余三维几乎零持久化 — C3 `CompactResult{tokens_before/after,dropped_count,degradation}` 只在 `tracing`;per-turn token `update_last_turn_usage` OVERWRITE session snapshot 列丢历史;C2 soft hint 1-2 连击无事件(仅 ≥3 intervention 落 `loop_intervention` 审计);breadcrumb `append_workflow_breadcrumb` 注入 `messages[0]` 后丢弃。加审计表无 `turn_seq` 列。核心矛盾 = scope(动不动后端 trace 管道)。

- **决策 D1 — scope = 完整版**(否决"纯前端聚合 MVP" / "折中"):纯前端只覆盖 ~1.5 维,教具价值低对不起 E2 定位;完整版补全 4 维 + 审计 turn_seq。parent `07-14-e2-harness-trace-viewer` + 2 child(后端管道 child-1 先 / 前端面板 child-2 依赖 child-1 的 event+IPC+数据结构)。

- **决策 D2 — viewer = 独立面板 live+回看**(否决"升级 AuditLogModal 事后刷新" / "侧栏 drawer"):教具定位要能看 harness 实时压缩/循环/工具调用;独立面板可常驻不挡 chat。约束后端:必须实时 emit ChatEvent(live)+ 必须落盘(回看)。

- **决策 D3 — 落盘 = always-on + 清理入口**(否决 opt-in debug / emit-only):个人 SQLite 工作台写入开销可忽略;opt-in 痛点 = 出问题后才想回看但没开,违背调试器定位。emit always-on(live 面板随时可用)+ 落盘 always-on(回看任何历史 session)+ 清理入口兜底 DB 增长。

- **决策 D4 — 新表 `turn_trace` 而非 messages 加列**:一个 turn 多维 trace(token/compaction/loop/breadcrumb)在不同写点到达,`UNIQUE(session_id,seq)` + UPSERT 累积各列;messages 是对话内容,trace 是观测,语义隔离不污染 messages schema 稳定性。

- **决策 D5 — `record_audit_event` 加 `turn_seq` 参数(不取 thread-local 上下文)**:精确对齐是 viewer 核心价值;21 类调用点机械扩散,chat_loop 传 `Some(seq)` / IPC handler 传 None;编译器强制 + grep 防漏。漏传 → 审计行 `turn_seq` NULL 回看时游离(可接受,前端 UNGROUPED 虚拟 seq footer 兜底)。

- **决策 D6 — trace 旁路观测,不动 agent 决策逻辑**:C3 仍按 `CompactResult` 降级/终止;C2 仍按 `loop_hit_count` 干预;breadcrumb 仍注入 prompt。trace 只在已有写点旁 emit + 落盘,不改决策分支。best-effort 落盘失败 `warn!` 吞不传播(同 `record_*_audit`)。worker gate 复用 `!skip_persist`(RULE-A-015),worker turn_trace 不冲 parent。

- **决策 D7 — `request_mode_change`/`request_task_state_transition` 的 `execute_blocking` 补 seq 参数**:初版传 None(execute_blocking 签名无 seq);trellis-check 判断补 seq 简单(签名加 `turn_seq` + chat_loop 拦截调用点传 `Some(seq)`),让这两个 turn 内 LLM 触发的审计也归 turn。`record_message_resend_audit` 保持 None(pre-turn-loop 事件,seq 是下一个未开始 turn)。

- **关键教训 —「文档状态表滞后是常态」**:代码先行(另一 session 完成 child-1/child-2 + commit + archive),文档(ROADMAP §2 状态行 / CLAUDE / STRUCTURE / frontend spec / IMPLEMENTATION §4)滞后,需独立 doc-sync follow-up。符合 memory `handoff-lags-behind-commits`。

- **测试**:1539 后端(cargo test --lib,新增 7:`turn_trace` upsert 累积/顺序/清除/覆盖/空值/级联 + `turn_seq` 审计)+ 863 前端(vitest,含 `trace.test.ts` 8 + `traceStore.test.ts`)+ vue-tsc 0 err + cargo fmt clean。零回归(现有审计/C3/C2/breadcrumb 行为不变)。

- **推后期**:筛选(按维度/turn/工具过滤)/ 导出(JSON)/ worker trace 隔离(`is_worker` 列,MVP 接受混入 parent session_id)/ C2 `loop_window` 滑动窗口中间态可视化(只 emit hit_count+verdict 摘要)/ workflow from→to hook 独立审计行(维持 `task.json.summary` marker)。


### 2026-07-13 — B9+ 生成式 UI 收尾(D3 通用 button + D4 diff 应用)

- **Context**:B9(07-02)落地只读(silent-allow `use_ui` + `<DiffPrimitive>` 复用 `DiffView` + `<CodeBlockPrimitive>` hljs + selector 复用 `ask_user_question`),parent PRD 把 D3 通用 button + D4 diff 应用 + D5 session 开关推迟。`use_ui` 当前是纯展示(零副作用);引入"用户动作 → 后端写"后,核心命门 = "应用动作"的权限归属 — 既不能破坏 plan 模式语义,也不能与 `edit_file` 形成"两种修改模型"冲突。

- **决策 D-Q1 — D4 定位 = 用户确认 UI(LLM 提议 + 用户应用,不走 LLM tool 权限链)**:LLM 用 `diff`/`button` primitive 提议修改,用户点"应用"才写文件。应用动作是**用户触发的前端 IPC**(`apply_ui_diff`),**不是** LLM tool — 故不注册进 `builtin_tools()`、`filter_tools_for_mode` 看不见它 → plan 模式天然可用(plan 约束 LLM 不约束 user)。**vs `edit_file`**:`edit_file` = LLM 自主改(走 `builtin_tools()` + Tier/PermissionStore),`apply_ui_diff` = 用户拍板(走 IPC + `assert_within_root` + audit,无 Tier / 无 PermissionStore)。两类工具适用场景用 `use_ui::definition().description` 写清(`edit_file` 默认改 / `use_ui` 该让用户看的场景),LLM 自分流。

- **决策 D-Q2a — action 模型 = 预定义枚举,不复用既有 tool 引用 / 不自定义 payload**:否决"action = 引用既有 LLM tool 名"(与 D-Q1 矛盾,会让用户点击触发 LLM tool 链)、否决"自定义 payload"(安全面过大,个人工具无必要)。**枚举首批边界 = `apply_diff`(后端 IPC 写文件) + `copy`(剪贴板) + `dismiss`(本地隐藏)**;`run_command` 等命令类动作**不在本批**(与 shell tool 重复触发路径,安全面陡增)。

- **决策 D-Q2b — `apply_diff` 是 Tauri IPC,不弹 modal,做 boundary + 审计,不走 LLM tool 权限链**:用户点"应用"= 显式意图(同 `merge_worker_run` L3b PR3 前例)。`assert_within_root(worktree_path, path)`(同 `edit_file:116`)做项目边界校验;`AuditKind::UiDiffApplied`(payload `{files: [{path, added, removed}], total_files}`)落表;不弹权限 modal(DiffView 已展示变更预览)。

- **决策 D-Q3 — scope = D4 + D3,不做 D5**:`D5` session `allow_generative_ui` 开关继续推迟(本批 D3+D4 已让 `use_ui` 从"只读展示"升"可交互提议",用户点击授权的语义边界够清晰;D5 是 product 决策不是架构缺口,留 follow-up)。`form` / `chart` / `table` primitive 同步推迟(无关本批安全面)。

- **决策 D-Q4 — `apply_ui_diff` 权限形态**:不弹 modal + `assert_within_root` + 审计落表(详见 D-Q2b)。**写目标 = `session.worktree_path` ?? `session.current_cwd` fallback**(同 `edit_file` / `chat_loop` 既有约定;无 worktree 不拒绝,落到 project 原目录)。

- **决策 D-Q5 — 手写 unified diff parser/applier,零新增依赖**(否决引入 `diffy` crate):TECH §1.4 "零新增依赖"硬约束;算法小(~250 LOC + 24 单测覆盖:parse / 多文件多 hunk / context 匹配 / 冲突 fail-fast / 行号偏移 / 空 diff / 无路径头);项目自研调性一致(对照 `read_file::cat_n_format`、`llm::sse` 手写状态机)。**行号语义**:`@@ -oldStart,oldLines +newStart,newLines @@` 中 `oldStart` 是**原始文件** 1-indexed 行号,apply 时跟踪 `cumulative_offset = Σ (newLines - oldLines)` over applied hunks,在 modified buffer 定位 `oldStart - 1 + cumulative_offset`。

- **决策 D-Q6 — 失败语义 = 整失败不部分写**(design §2.3):parse 整 diff → 逐 FilePatch read → apply_to_file(纯函数 text→text)→ 全成功才 `tokio::fs::write` 任一文件。任一 hunk context 不匹配 → 全失败,前端 inline error,不写任何文件。**Audit 仅在写全部成功后落**(中间失败不污染审计表;失败次数可由前端日志追踪)。

- **决策 D-Q7 — `use_ui` 始终 Silent Allow(Tier 5)+ `Risk::Low`**:展示本身无副作用未变;button 的"动作"由前端按 `action` 分发,`use_ui::execute` 不执行 action,仍返"已渲染 N 个 primitive"。**plan/edit/yolo 三档 mode 都不影响 use_ui 可见性**,只影响用户应用按钮 click 后的写路径(走 `apply_ui_diff` IPC,IPC 不进 LLM tool 链)。

- **决策 D-Q8 — Raw fallback 形式(无 `---`/`+++` 头)禁用 Apply 按钮**:`DiffPrimitive` 加 `hasUnifiedHeaders` 谓词(/^--- /m + /^\+\+\+ /m),无头时 Apply 按钮 `disabled` + tooltip「该 diff 格式不可应用(需带 ---/+++ 路径头的标准 unified diff)」。后端 parser 二次兜底(无路径头 → `kind="parse"` 拒绝),前端禁用避免无意义 round-trip。

- **关键教训 —「手写文本协议先期只测单一 case 易漏 corner」**:`diff_apply.rs` 写完第一版,parse 测试 `parse_missing_headers_returns_missing_header` 期望 `-old\n+new` 返 `MissingHeader`,实际返 `IncompleteHeader`(parser 优先看 `-old` 当 removed hunk line,无 hunk → IncompleteHeader)。两 variant 在 IPC 端都映射到 `kind="parse"`,UX 等价,但测试断言要 align 实际行为。**后续手写协议类(任何 text-based state machine)首版单测应枚举所有 4 corner + 接受真实分支**,而非"按 PRD 期望断言"。

- **测试**:1531 后端(cargo test --lib)+ 842 前端(vitest)+ vue-tsc 0 err + cargo fmt clean。本批新增:24 `diff_apply::tests`(parse / apply / 多 hunk offset / context conflict / raw fallback)+ 4 `commands::ui::tests`(ParseError → IPC `kind` 映射 + 成功/失败结果序列化 + Mode enum import 守门)+ 7 `use_ui::tests::execute_button_*`(3 action 验证 + 缺 action / 未知 action / apply_diff 缺 diff_text / 空 diff_text 拒绝)+ 17 `DiffPrimitive.test.ts` B9+ D4 段(IPC invoke spy + 成功 toast + kind 文案 + raw fallback disabled + 无 session disabled + 异常错误归 `io` + reject 隐藏)+ 11 `ButtonPrimitive.test.ts`(3 action 分发 + 默认 label + 自定义 label override + apply_diff 走 IPC + copy 走 clipboard + dismiss 本地 + 无 session disabled + 未知 action defensive)。零回归。

- **推后期**:D5 session `allow_generative_ui` 开关(产品决策非架构缺口)/ `run_command` 等命令类 button action(安全面陡增,follow-up 安全评估后单独立项)/ `form` / `chart` / `table` primitive(独立需求)/ patch 三方合并或 rename / 二进制 diff 探测 / 行级权限(n 行确认 vs 整文件)/ 多文件 diff 跨 project 边界(目前每个文件 boundary 单独校验)/ 文件创建(`oldLines=0` 暗示)/ App crash 恢复 pending apply / `apply_ui_diff` 进度流式报告(目前整批写完才返;长 diff 可走 L1a 同样的 handle + 通知模式)。


### 2026-07-10 — workflow task.json hardening(R1-R5):read_task lenient + `create_task` tool + 即时 resolve + 软推荐 hint

**Context**: 跟随 07-08~07-10 workflow 集成(见 `2026-07-08 — workflow 系统总览`),`task.json` 作为 LLM 可写的普通文件但无 schema 保护 —— `create_task` 是 Tauri IPC 非 LLM tool,LLM 在 loop 内只能 `write_file`/`edit_file` 手写,实测两次把 task.json 写崩(planning 漏 `created_at` / implement 手改 `status:"in_progress"` 绕过 `update_checklist` 安全映射)。共同根因:读侧严格(derive Deserialize + 必填 `created_at`/`updated_at`),写侧无 guard,LLM 一次手写就致命,`resolve_current_task` 静默跳过 → `current_task=None` → `request_task_state_transition` 报 `no active workflow task`。任务 `.trellis/tasks/archive/2026-07/07-10-workflow-task-json-hardening/`。

**关键决策(5 个 R,按"为什么"而非时间序)**:

1. **R1 read_task lenient 在类型层而非 read_task 函数内(止血 + 防御)**:给 `TaskStatus` 加自定义 `Deserialize`(delegate `from_str_opt`,非法值 fallback `Planning`),给 `TaskJson.created_at`/`updated_at` 加 `#[serde(default)]`(默认空串),给 `TaskItem.content` 加 `#[serde(default)]`。**否决**"read_task 内 best-effort 二次解析" —— 要维护两套解析,`resolve_current_task` / `set_task_state` / `update_checklist` 多处调 `read_task` 都应受益,类型层加 default + 自定义 Deserialize 是单点修复。5 个 case 测试覆盖(缺 created_at / 缺 updated_at / item status=in_progress / item status=pending / item 缺 content),`write_task`/`create_task_init` 内部写路径仍 100% 合法(lenient 只兜底外部手写,不影响写路径)。

2. **R2 `create_task` 升级为 LLM tool + 新增 `filter_tools_for_workflow`**(补合规入口 + 顺带收编 `request_task_state_transition`):新 `tools/create_task.rs` 复用 `create_task_init` 保证 schema 正确建档,注册进 `builtin_tools()`,**新增** `filter_tools_for_workflow` 白名单 `{create_task, request_task_state_transition}`,仅 `workflow_enabled=true` 的 session 保留这俩。**否决**「全局可见 + 执行 gate」(那是 `request_task_state_transition` 现状,有 schema 污染缺陷 —— 非 workflow session 的 LLM 看到 schema、误调才报错)。worker 侧 `filter_tools_for_subagent` 的 `STRUCTURALLY_DISABLED` 集补 `create_task`(worker 不该跨 session 建档)。

3. **R3 transition 拦截即时 resolve**(修冻结 ctx):`chat_loop.rs:3426` 不再读冻结的 `workflow_ctx.current_task`(IPC 入口建一次,200-turn loop 内不变),改为 `resolve_current_task(&current_ctx.worktree_path).await` 即时读盘取 `(current_state, current_slug)`。状态机门控必须基于盘上真实状态 —— agent 可能在本 loop 内 `create_task`/`update_checklist` 改了盘,ctx 快照已过时,与 apply 侧 `resolve_task_state_transition` 的 "read fresh off disk" 一致。

4. **R4 breadcrumb 注入统一即时 resolve**(不 mut ctx):`append_workflow_breadcrumb` 签名改为接收「即时 resolve 的 current_task」而非整个 ctx。**否决**改 `&mut Option<WorkflowCtx>` —— `workflow_ctx` 在 loop 内多处被 `&` 借(`append_workflow_breadcrumb`、subagent dispatch 的 `workflow_ctx.as_ref()`、loop_detection 等),改 mut 要审计全部借用点,易出错。性能:`resolve_current_task` 是 `read_dir` + 每文件小 parse(task 目录通常 0-1 个非终态 task),每 turn 一次成本可接受。**保留** ctx.current_task 给 delegation template / subagent dispatch(不频繁路径,只状态机门控 + breadcrumb 两点改即时)。

5. **R5 bootstrap hint 文案软推荐**(不禁止 write_file):`inject.rs:620-625` 引导 LLM **优先**用 `create_task` tool 建档(省 token + 模板全),但**不禁止 write_file** —— 韧性由 R1 lenient 在读取侧兜底,不在写入侧设 path guard 卡(过度严格 + 失去扩展性:未来 schema 加字段 LLM 仍可直接写)。`create_task` tool description 定位「便捷建档助手」(**非唯一合规路径**),说明 write_file 也可,read_task lenient 兜底。

**Consequences**:
- LLM 手写 task.json 不再致命(缺字段 / 错枚举 / 缺 content 都被 lenient 兜底,`resolve_current_task` 不再静默跳过有效 task)。
- workflow_enabled=false 的 session 不再看到 workflow tool schema(`create_task` + `request_task_state_transition` 从 turn_tool_defs 剥掉),LLM 误调率归零。
- transition 拦截 + breadcrumb 都基于盘上最新状态,同 loop 后续 turn 立即反映新状态(原 ctx 冻结导致"以为没切成功,反复试"循环消失)。
- 写入侧零约束,扩展性保留:未来 task.json 加字段 LLM 仍可手写,read_task lenient 自动受益。

**关联**: PRD + design + implement `.trellis/tasks/archive/2026-07/07-10-workflow-task-json-hardening/`;commit `c6a983e fix(workflow): task.json 对 LLM 手写的健壮性加固 (R1-R5)`;Step 1(R1)/ Step 2(R3+R4)/ Step 3(R2)/ Step 4(R5) 各 commit 内部分拆;新 tool `create_task` 与 `filter_tools_for_workflow` 测试覆盖三场景(workflow / 非 workflow / worker);1348+ 后端 + 794+ 前端 tests passed(0 failed)。


### 2026-07-09 — workflow chip merge:WorkflowToggle + PluginSelect 合并为单 chip + popover

**Context**: Step 2.2(07-08)UI 落了两个并列 toggle:`WorkflowToggle`(启停 workflow plugin)和 `PluginSelect`(切具体 plugin 名)。两组件在顶栏占两个 chip 位 + 各自 popover,用户认知负担高 + 视觉碎片化。测试中用户多次把这两个控件当作"重复的开关"。任务 `.trellis/tasks/archive/2026-07/07-09-07-09-workflow-chip-merge/`。

**关键决策(3 个,按"为什么"而非时间序)**:

1. **合并为单 chip,plugin 选择作 popover 内容**(而非"二级菜单"或"折叠"):**否决**二级菜单 —— 多一层点击,workflow toggle 是高频操作。**否决**折叠(隐藏在 ... 内)—— toggle 状态不够显眼。popover 模式:`chip` 始终可见显示当前 plugin 名或"off",点开 popover 内 `[toggle][plugin 下拉][reset to default]`,符合 macOS / IDE 习惯。

2. **chip 状态文案统一三态**(off / on + plugin 名 / 切换中):off → 灰底"workflow";on → 高亮 + plugin 名"dev-workflow";切换中 → spinner + 半透。三态用同一 chip 控件,避免双控件状态错位。

3. **保留独立 components 作为底层,新 chip 组合二者**:不写单文件大组件,`WorkflowToggle` + `PluginSelect` 作为内部子组件,新 `<WorkflowChip>` 持有 popover open 状态 + 联动二者 store。回退路径清晰(解 chip 可回到 toggle + select)。

**Consequences**:
- 顶栏控件从 2 个 chip 减为 1 个,视觉简洁度提升。
- toggle 与 select 联动更直观(同 popover 内,不再误判为重复控件)。
- 底层组件可复用,未来若需"高级模式"(独立 toggle / select),回退成本低。

**关联**: PRD + design + implement `.trellis/tasks/archive/2026-07/07-09-07-09-workflow-chip-merge/`;commit `5db27be refactor(workflow): 合并 WorkflowToggle + PluginSelect 为单 chip + popover`;前端 vitest 覆盖三态渲染 + popover 联动;vue-tsc 0 err;pnpm build 成功。


### 2026-07-09 — 前端 `task_state_transition` 交互卡片

**Context**: `request_task_state_transition` tool 在 IPC handler 改了 `task.json.status`(走 07-08 Step 3.1 `set_task_state` IPC),但前端无对应 UI 反馈 —— 用户看到 tool_use 出现,但不知道 transition 是否成功 / 当前 task 状态是什么,只能切到 settings 或文件树手动看。任务 `.trellis/tasks/archive/2026-07/07-09-workflow-transition-card/`。

**关键决策(3 个,按"为什么"而非时间序)**:

1. **前端卡片复用既有 ToolCallCard 框架**(而非新建 Modal 组件):`request_task_state_transition` 与 `ask_user_question` 同类"user 决策驱动"交互,沿用 `toolExecutedCard` 模式 + 转场动画。**否决**新建 Modal —— 与 ask_user_question 视觉不一致,UI 风格分裂。卡片显示「任务从 `Planning` → `Implement`(确认?)」+ 状态箭头 + 时间戳。

2. **状态变化驱动渲染,非 polling**:卡片订阅 `task.json.status` 的变更(经 `workflow_ctx.current_state` event),状态变了才重渲染;不在卡片内做 setInterval 轮询。**否决**轮询 —— 增加 IPC 频次,实测同 session 内 status 变化通常 < 1Hz,event-driven 已足够。

3. **transition 成功 / 失败 / 取消三态卡片**(统一 lifecycle):成功 → 绿色 ✓ + "已切到 Implement";失败 → 红 ✗ + reason;取消 → 灰 ⊘ + "user cancelled"。三态用同一 `<TransitionCard>` 组件,props 控制,不写三个独立组件。

**Consequences**:
- 用户能看到 transition 是否落地,不再需要切到文件树 / settings 手动确认。
- 卡片与 ask_user_question 视觉一致,UI 风格统一。
- event-driven 零额外 IPC 负担。

**关联**: PRD + design + implement `.trellis/tasks/archive/2026-07/07-09-workflow-transition-card/`;commit `969466b feat(workflow): 前端 task_state_transition 交互卡片`;前端 vitest 覆盖三态渲染 + event-driven 重渲染;vue-tsc 0 err;pnpm build 成功。


### 2026-07-08 — pending-indicator 跨 session 待处理交互 UI 三档提醒(角标 + 徽章 + toast)

**Context**: 07-08 之前,`ask_user_question` / `request_task_state_transition` 跨 session 消息携带 pending 标记时,前端只在当前 session 显示 inline card,**用户切到别的 session 后无任何提示** —— 等切回来才发现有未答问题,体验割裂。

**关键决策(3 个,按"为什么"而非时间序)**:

1. **三档分层提醒**(角标 + 徽章 + toast),不只 toast 一种:角标(Sidebar session 项 红点)= "有 pending";徽章(顶栏 workflow chip 旁 小数字)= "N 个 pending";toast(切到其他 session 时)= "session X 有 N 个未答问题,点击切回"。**否决**只用 toast —— toast 易错过(用户可能不在电脑前),角标持久化更可靠。**否决**只角标 —— 切 session 时无即时触达。三档互补,覆盖「持久 / 即时 / 唤回」三个维度。

2. **pending 标记随消息持久化,不入 ChatEvent**:消息带 `pending: true` 字段(经 store 层),切 session 时 store 重新计算本 session 的 pending 数 → 更新角标 / 徽标。**否决**入 ChatEvent —— ChatEvent 是 transient 事件流,不应持久化。

3. **去重:N 个相同 session 的 pending 只算 1 次**(以 session 维度而非消息维度):同一 session 的多个 pending 消息合并为一个角标 / 徽章计数单元。**否决**消息级计数 —— 1 session 5 个 pending 问题在 sidebar 显示 "5",体验过噪,session 级 "1" 更合理。

**Consequences**:
- 切 session 不再"丢"pending 问题,角标持续提醒。
- 顶栏徽章一眼能看出全局 pending 总数,适合用户优先答哪条。
- 角标 / 徽标 / toast 三档互补,持久 + 即时 + 唤回三个维度全覆盖。
- session 级去重避免计数爆炸。

**关联**: 07-08 同期落地;commit `b95e8c5 feat(pending-indicator): 跨 session 待处理交互 UI 提醒(角标+徽章+toast 三档)`;前端 vitest 覆盖三档渲染 + session 切换触发 + 去重逻辑;vue-tsc 0 err;pnpm build 成功。


### 2026-07-08 — workflow 系统总览:workflow.json 外置 + builtin dev plugin + Step 0.1~3.3(task 状态机 / breadcrumb 注入 / delegation 模板 / archive IPC)

**Context**: 项目从 2026-06-21 V2 第二档收尾后,B8(DAG workflow 编排层)一直列在 ROADMAP §2 第四档。本期(07-08 ~ 07-10 跨度 3 天,~25 commit)把 workflow 从"概念"推到"完整落地":workflow.json 外置(不再 hardcode 模板)、内置 dev workflow plugin(开箱即用,新项目零配置启动)、Step 0.1~3.3 完整 9 阶段管线(task 状态机 / breadcrumb 注入 / delegation 模板注入 / set_task_state IPC / archive_task IPC / spec 沉淀触发)、plugin skill loader、agent 自跟踪 checklist 与 task.json 同步。任务 `.trellis/tasks/archive/2026-07/07-08-workflow-integration/`(主)+ 07-09/07-10 多个延伸任务。

**关键决策(9 个,按"为什么"而非时间序)**:

1. **workflow.json 外置 + load_workflow + validate + fallback(Step 0.1~2.1)**:不再 hardcode 4 个 workflow state 模板在 Rust 代码里,改为 `.everlasting/workflow.json` 外置文件 + `load_workflow` 解析 + `validate`(schema + 引用完整性)+ `fallback`(文件缺失 / 解析失败 → 内存 default)。**否决** Rust 静态定义 —— 用户改 workflow 配置得重新编译;**否决**完全 freeform JSON 无 validate —— 改坏一个字段整 plugin 崩。`WorkflowDef` struct 强类型 + JSON 双向,改一个 state 名 typo 在 validate 阶段就报,而不是 chat_loop 跑起来才报。

2. **builtin dev workflow plugin 开箱即用(07-09 builtin plugin task)**:默认 plugin 写在 `resources/builtin-workflow/dev/workflow.json`,跟应用一起 ship。新项目无 `.everlasting/` 目录也能跑,自动 fallback 到 builtin。**否决**"必须先建 workflow.json 才生效" —— onboarding 摩擦大,违背"开箱即用"目标。builtin plugin 不入 Git(用户改了就覆盖),只读默认。

3. **Step 0.1~3.3 完整 9 阶段管线**:Step 0.1 sessions 表加 `workflow_enabled` 列;Step 0.2 顶栏 toggle UI;Step 0.3 `WorkflowDef` struct + 4 访问函数;Step 0.4 `task.json` 读写 + `create_task` IPC;Step 0.5 `chat_loop` 30 参 + per-turn breadcrumb 注入;Step 1.x plugin skill loader 4 步;Step 2.x delegation 模板 + role-gate;Step 3.x `set_task_state` / `archive_task` IPC + `TaskStatus::Completed`。每步独立 commit 独立验证,有问题单步 revert。

4. **plugin agents/ 落点 + `SubagentSource::Plugin`(Step 2.3)**:`dispatch_subagent` 加 plugin 来源,plugin 自带的 agents/ 目录与 builtin agents/ 并列,优先级 plugin > builtin。**否决**"plugin agents 复用 builtin 路径" —— plugin 升级不能污染 builtin;**否决**"plugin agents 必须 import builtin 才能用" —— 增加 onboarding 成本。

5. **task 状态机(`TaskStatus`):Planning → Implement → Check → Done,四态单向**:`TaskStatus` enum derive `Deserialize` + `from_str_opt`(非法→Planning);`update_checklist` 有安全映射 `InProgress→Implement`(防 LLM 乱写状态);`request_task_state_transition` 走 IPC + UI 卡片(07-09 transition-card)。**否决**任意跳转 —— workflow 语义模糊;**否决**加 Cancel 中间态 —— v1 简化,Cancel 等 v2 再加。

6. **breadcrumb per-turn 注入(Step 0.5)**:每 turn 开头由 `build_workflow_breadcrumb` 拼合成带 `cache_control: ephemeral` 的 synthetic user message 注入,跟 06-11 B5 Memory 注入位置重构同款机制(对齐 design §3.1)。**否决**"system prompt 拼装" —— 跟 B5 不一致,且会破坏 cache hit。**否决**"工具描述追加" —— token 浪费。

7. **delegation 模板注入(dispatch turn, Step 2.5)**:`run_subagent` 派 worker 时,worker 收到的 system prompt 含 delegation 模板(parent task context + 当前 step + 期望产出)。**否决**"worker 自己去查 task.json" —— 增加 worker 复杂度,违反"worker 是简化版 agent"定位。

8. **`create_task` IPC + `archive_task` IPC(Step 0.4 + 3.3)**:前端通过 IPC 驱动 task 生命周期。`create_task` 复用同一 `create_task_init` 内部函数(后续 07-10 把它升为 LLM tool,R2 决策),写路径单一真源。`archive_task` IPC 改 `TaskStatus::Completed` + `completed_at` 时间戳 + 移到 `_archive/`。

9. **`.everlasting/spec/` + trigger_spec_distillation(Step 3.2)**:`set_task_state → Done` 触发 `trigger_spec_distillation`,把 task 期间产生的 spec-worthy 内容(commit msg / design 决策 / 关键代码引用)蒸馏到 `.everlasting/spec/`。**否决**"task 完成只归档 task.json" —— 浪费知识,违背 "memory 不只是 chat history" 的 V2 2 期哲学。

**Consequences**:
- 新项目零配置开箱即用(builtin dev plugin),用户改 workflow 配置不改代码。
- 任务全流程可追溯(task.json + breadcrumb + transition 卡片),用户能清楚看到当前状态。
- worker 复用 plugin 的 agents/,plugin 升级不污染 builtin。
- task 完成触发 spec 蒸馏,知识沉淀而非归档即忘。
- 9 个阶段 25 commit 独立可 revert,任何阶段出问题可回滚。
- 07-10 task.json hardening(R1-R5)是本系统的稳态加固,非新功能。

**关联**: 主任务 `.trellis/tasks/archive/2026-07/07-08-workflow-integration/`(9 阶段)+ 延伸 `.trellis/tasks/archive/2026-07/07-09-workflow-builtin-plugin/`(commit `7c22a6d`)+ `.trellis/tasks/archive/2026-07/07-09-workflow-transition-card/`(commit `969466b`)+ `.trellis/tasks/archive/2026-07/07-09-07-09-workflow-chip-merge/`(commit `5db27be`)+ `.trellis/tasks/archive/2026-07/07-10-workflow-task-json-hardening/`(commit `c6a983e`)+ pending-indicator(commit `b95e8c5`);spec 增量待建 `.trellis/spec/backend/workflow.md` + `.trellis/spec/backend/task-state-machine.md`(07-08 后续沉淀);1348+ 后端 + 794+ 前端 tests passed(0 failed,跨 25 commit 累积)。


### 2026-07-07 — `request_mode_change` tool:复用 `set_session_mode` IPC + 共用 `QuestionStore` + Yolo 二次 modal 双 IPC

**Context**: LLM 在主 loop 里无工具申请 mode 切换,只能在 plan 模式返回 "please switch me to Edit mode",由用户手动按 `Shift+Tab` 切。新 tool `request_mode_change` 让 LLM 在 turn N 通过 `tool_use` 申请,用户 inline card 二选一,允许路径修改 `sessions.mode`。任务 `.trellis/tasks/archive/2026-07/07-07-07-07-request-mode-change-tool/`,完整 PRD + design 5 时序 + 11 单测 + 5 集成测试 + 5 IPC 测试 + 23 前端测试。

**关键决策(7 个,按"为什么"而非时间序)**:

1. **新 tool 独立而非复用 `ask_user_question` 的 2 选项形态**:`ask_user_question` 是"信息询问"(收集 context),`request_mode_change` 是"写操作"(改 `sessions.mode`),职责根本不同。复用 ask_user_question 需要塞 "target_mode + reason + mode 颜色映射 + 二次 modal" 进 options 数组变成 ad-hoc 字段,schema 失去通用性。审计 kind 不同(`tool_executed` vs `mode_change_requested/allowed/denied` 3 类新增),IPC 链不同(resolve_tool_question 单纯回填 vs `resolve_mode_change` 走 `set_session_mode` 内部函数落库)。对齐 Claude Code 业界模式,写操作类申请都是独立 tool。

2. **Yolo 二次 modal 守门完全沿用 `chatStore.pendingResolveRequest` + `confirmYolo` 路径**:`useChatStore.requestSetMode(sid, "yolo")` → `pendingYoloConfirm = true` → modal(显示 "切换到 Yolo 将跳过所有用户确认") → `confirmYolo` / `cancelYolo` action。**不**新写 modal 组件,**不**新写 store action。LLM 申请 Yolo 风险 = user 主动切 Yolo 风险,共用同一道守门避免"LLM 偷偷切 Yolo"风险高于"user 主动切 Yolo"的设计失衡。`<RequestModeChangeCard>` 的"允许"按钮 emit handler 分支:`if (targetMode === 'yolo') requestSetMode(); else resolveModeChange(allow=true)`。

3. **共用 `QuestionStore` + 单 pending gate(`PendingInteraction` 互斥)**:扩展 `QuestionStore` 的 value 类型为 `PendingInteraction` tagged enum(`Question(ToolQuestionPayload) | ModeChange(ModeChangePayload)`),**不**新建独立 `ModeChangeStore`。`register` 接口扩展接受 `PendingInteraction`,`resolve` 保留原签名(oneshot 不区分 kind)。**否决**独立 store —— 双 store 互不感知,允许同 session 1 个 question + 1 个 mode_change 并发,UI 难管理 + LLM 行为不可预测。**否决**"只校验不互斥" —— 同 session 同时挂 2 张 card UI 堆叠,user 不知道先答哪个。`map.contains_key(session_id) -> AlreadyPending` 互斥语义天然,跟 ask_user_question 是同一类"user 决策驱动"交互,共享单 pending gate 语义最自然。新 IPC `get_pending_interaction(session_id) -> Option<PendingInteractionEntry>` 统一查询,旧 `get_pending_question` 软弃用保留 1 版本(向后兼容)。

4. **IPC 链:`resolve_mode_change` → `set_session_mode` 内部函数落库,tool 内部不直接 `db::update_session_mode`**:第一稿 design §5.3 写"tool 内部直接调 `db::update_session_mode` 落库",实装时发现双路径漂移风险 —— Yolo root guard / Yolo 二次 modal / `mode_changed` audit 完整性都集中在 `set_session_mode` 内部函数,tool 单独落库会导致这 3 处副作用重复实现 + 行为漂移。改为:`resolve_mode_change` IPC handler 内部调 `set_session_mode` 内部函数(不开 IPC,直接调内部 helper)→ 落库 + audit + 返回 `SessionRow` → 调 `store.resolve(sid, Allow)` 让 agent loop oneshot 解除。**关键不变量**:`store.resolve` 必须在 `db::update_session_mode` **之后**调用(否则 agent loop 先收到 Allow 但 DB 未落库,出现不一致,design §7.2 风险段)。

5. **Yolo 二段路径的"双 IPC 顺序"**:`set_session_mode` 落库 + `mode_changed` audit → `resolve_mode_change` 解 oneshot + `mode_change_allowed` audit。`set_session_mode` handler 检测"这是从 card dispatch 来的"(`pendingResolveRequest` 标记),额外调 `store.resolve(sid, Allow)`;前端 `confirmYolo` 拿到 `SessionRow` 后调 IPC B 解 oneshot(IPC B 第二次 resolve 是 no-op,`AlreadyResolved` 错误码 + warn log,return Ok 防双 audit)。Yolo 二次 modal "取消"路径 → 只调 IPC B(allow=false)→ `mode_change_denied { reason: "yolo_cancelled_confirm" }` audit + tool_result `{"cancelled_by_user": true, "reason": "user cancelled Yolo confirm"}`。

6. **`PendingInteraction` tagged enum + `InteractionResponse` 统一 oneshot 通道类型**:oneshot 通道类型 `QuestionResponse` → `InteractionResponse` enum:`Answered { new_mode: SessionMode } | Cancelled`。`new_mode` 字段对 ask_user_question 语义无意义(取 None 即可),对 request_mode_change 是 `prev_mode` → `new_mode` 迁移记录,工具结果回填 `{"allowed": true, "prev_mode": "...", "new_mode": "..."}`。tagged enum 而非 trait object —— 简单、序列化、零 dyn 成本。

7. **wire shape 共享 struct 豁免**:`ModeChangePayload` 走 `#[serde(rename_all = "snake_case")]` 序列化(共享 struct,**不**像顶层 Tauri arg 那样 auto-camel,跟 `ToolQuestionPayload` 同款)。前端 `getPendingInteraction` IPC 解析时按 snake_case 拿 `target_mode` / `current_mode` / `tool_use_id` / `session_id` / `reason` / `ts`。

**审计种类(20 = 17 旧 + 3 新)**:

- `mode_change_requested`(tool 入口;payload `{target_mode, reason, noop: bool}`,`noop=true` 是 LLM 申请切到当前 mode 的留痕)
- `mode_change_allowed`(resolve 走允许路径;payload `{prev_mode, new_mode, target_mode}`,**不**含 `mode_changed` —— 由 `db::update_session_mode` 自动产生,职责分离)
- `mode_change_denied`(resolve 走拒绝路径;payload `{target_mode, reason: "user denied"|"yolo_root_guard"|"yolo_cancelled_confirm"}`)

**6 处副作用 + 3 偏差(trellis-check 评估「可接受」+ 代码注释)**:

- `Pool<SqlitePool>` 参从 PRD R4 初稿进入 tool 签名,后从 design §6 决策变更里移除 —— 落库走 IPC,tool 不需要 pool。
- `current_mode` 参保留:noop 判断需要;`chat_loop` turn 0 快照提供(`chat_loop.rs:600`)。
- `noop_target_equals_current_returns_noop_marker` 单测验证 noop 路径不挂 store / 不发 IPC / 留痕 audit。
- 第二次 `resolve_mode_change` invoke(rid 已 resolve)→ `AlreadyResolved` 错误码 + warn log + `Ok(())` no-op(防双 audit 写入 + 防 IPC handler panic)。
- 旧 `get_pending_question` IPC 软弃用,保留兼容 1 版本,前端新代码用 `get_pending_interaction`;旧 IPC 标记 `@deprecated` 不删,下版本再删(对齐 BACKLOG §3.2 "废弃保留归档" 原则)。
- `get_pending_interaction` 返回 `Option<PendingInteractionEntry>`,前端 `useQuestionCardsStore` 走 `Map<sid, PendingInteraction>` 路由 kind 派发。

**Consequences**:

- LLM 在 plan 模式可申请切到 edit 落代码(无需 user 手动 Shift+Tab),反向 edit 模式可申请切到 plan 提议架构,Yolo 高风险任务可申请(经二次 modal 守门)。
- `QuestionStore` 升级为 `PendingInteraction` tagged enum 是破坏性签名变更,但仅 1 个 caller(`ask_user_question::execute_blocking`),改写为 `PendingInteraction::Question(parsed)` 即可,blast radius 极小(详见 design §7.1)。
- Yolo root guard 行为集中(`is_running_as_root` 检测只在 `set_session_mode` 内部函数 + `resolve_mode_change` handler 各 1 次),复用 IPC 路径避免双路径漂移,审计完整性天然。
- `set_session_mode` 仍为唯一 mode 落库入口(2026-07-07 决定,**不**回归到 PRD R4 初稿"tool 内部落库"路径),前端 IPC 双向兼容。

**非阻塞观察**:

- `current_mode` 陈旧风险:LLM 在 turn N 申请切到 X,noop 判断时 `current_mode` 是 turn 0 快照;若 user 在同 LLM 响应前手动改了 mode(罕见),`current_mode` 已陈旧 → noop 误判。可接受(误判最多 1 次"误以为切了但没切",下 turn user 调 mode 后 LLM 看到正确)。改进可选 v2:每次 `execute_blocking` 实时 `load_session(...).mode` 做 noop 判断(避免陈旧)。
- IPC-level 双重 resolve 兜底存在,但前端 `useQuestionCardsStore.resolveModeChange` 内部已 dedupe(返回前先 `if (pendingBySession.get(sid)?.kind === 'mode_change')` 守卫),IPC 端 `AlreadyResolved` 是防御性兜底,正常运行不会触发。

**关联**: PRD + design + implement `.trellis/tasks/archive/2026-07/07-07-07-07-request-mode-change-tool/`;spec 增量 [tool-contract/11-request-mode-change.md §request_mode_change](../../.trellis/spec/backend/tool-contract/11-request-mode-change.md) + [permission-layer.md §5c](../../.trellis/spec/backend/permission-layer.md) + [agent-loop-architecture/pattern-worker-subagent.md §"Tool interception"](../../.trellis/spec/backend/agent-loop-architecture/pattern-worker-subagent.md) + [frontend/chat/request-mode-change.md §request_mode_change](../../.trellis/spec/frontend/chat/request-mode-change.md);1348 后端(`cargo test --lib`)+ 794 前端(`pnpm test`)+ vue-tsc 0 err + pnpm build 成功,4 commit(后端 / IPC + audit / 前端 / 集成测试)。


### 2026-07-07 — `request_mode_change` tool(LLM 申请切 mode)

- **Context**:LLM 在主 loop 无工具申请 mode 切换,Plan 模式只能返回 "please switch me to Edit mode" 由 user 手动 `Shift+Tab` 切;反向同理(Edit 模式想切 Plan 提议架构)。本档补一个 LLM-driven mode 申请工具,user 通过 inline card 二选一授权,沿用现有 `set_session_mode` IPC 副作用链(DB + 审计 + Yolo 二次 modal + root guard)避免双路径漂移。

- **决策 D1 — 工具名 `request_mode_change`(snake_case 风格跟 `ask_user_question` 对称)**:语义清晰(申请,非直接改)。`target_mode` ∈ `{edit, plan, yolo}` 3 档 user-facing(`background` enum 永远不暴露)。schema 硬编码 enum,不动 LLM 动态 build。

- **决策 D2 — 卡片形态 = inline message card,非 modal**(沿用 `ask_user_question` 范式):AC10 红线明确:无 portal / 无遮罩 / 无 reka-ui Dialog。**唯一例外**:Yolo 二次确认 modal 沿用既有 `useChatStore.pendingYoloConfirm`,不新写 modal 组件。

- **决策 D3 — "允许" 路径 = 前端触发 IPC + `resolve_mode_change` handler 调 `set_session_mode_internal` 落库**(关键变更,对比 PRD R4 初稿"tool 内部直接调 `db::update_session_mode`"):否决直接落库方案,改走 IPC 复用,理由:① **单一落库入口** —— user 主动切 mode / LLM 申请切 mode 两条路径行为必须完全一致,共用 `set_session_mode_internal` 纯函数(`set_session_mode` IPC handler 抽出的,逻辑 1:1 搬迁);② **Yolo 二次 modal 在前端触发**(`chatStore.requestSetMode` → `pendingYoloConfirm` modal → `confirmYolo` 调双 IPC);③ **审计一致** —— `mode_changed` audit 由 `db::update_session_mode` 自动产生,`resolve_mode_change` 调一遍 `set_session_mode_internal` 是幂等的(二次 UPDATE 无副作用)。

- **决策 D4 — Yolo 二次 modal 守门通过 `chatStore.pendingResolveRequest` + `confirmYolo(pendingResolve)` hook**:card "允许" + `targetMode === "yolo"` → 不直接调 `resolveModeChange`,而是 `useChatStore.requestSetMode(sid, "yolo")` → modal 弹 → `confirmYolo` 成功 → `resolveModeChange(allow=true)` 解 oneshot;modal 取消 → `resolveModeChange(allow=false)` 走拒绝路径。`pendingResolve?: { sessionId, toolUseId, targetMode }` 是 `confirmYolo` / `cancelYolo` 的可选参数,**user 主动切 Yolo 路径(Shift+Tab / popover)ref 为 null,完全不动既有代码**。`is_running_as_root` 时 modal "确认"按钮 disabled + 红字 "Cannot enable Yolo as root",后端 `set_session_mode` IPC 兜底再守一道。

- **决策 D5 — `QuestionStore` 升级为 `PendingInteraction` tagged enum(`Question \| ModeChange`)**:单 pending gate 跨 kind 互斥(同 session 不能并发 2 个待决交互)。`register` 接受 `PendingInteraction`,`resolve` 返 `PendingInteractionEntry { kind, payload }` 让 caller 知道 resolve 了哪种 kind(决定写哪个 audit)。新 IPC `get_pending_interaction` 统一查询,旧 `get_pending_question` 软弃用保留兼容(`#[deprecated]`,`#[allow(deprecated)]` 在 `lib.rs` 注册处抑制警告)。

- **决策 D6 — 3 类新 audit kind 跟 `mode_changed` 不重复**:`mode_change_requested`(LLM 调 tool 触发,记录 target_mode + reason)/ `mode_change_allowed`(user 允许 + DB UPDATE 成功,记录 prev → new)/ `mode_change_denied`(user 拒绝 / Yolo guard 触发,记录 target + reason)。`mode_changed` 由 `db::update_session_mode` 自动产生,**不重复写**。AuditKind 17 → 20。

- **决策 D7 — `noop` 短路(target_mode == current_mode)**:tool 立即返 `{"noop": true, "current_mode": "..."}`(`is_error: false`),不弹 card、不发 IPC 事件。减少 round-trip,LLM 看到 noop 自决。同时写 1 条 `mode_change_requested{noop: true}` audit 保留 LLM 申请痕迹。

- **决策 D8 — Worker subagent 禁用**:`STRUCTURALLY_DISABLED` 加 `request_mode_change`(跟 `update_checklist` 同档)。Worker 想切 mode 必须回 parent。**并行 eligibility 走默认 false**(整批 Serial,跟 `ask_user_question` / `dispatch_subagent` 同档)—— 白名单机制自动生效,不加显式 branch。

- **关键教训 —「测试装配 vs 设计意图」**:Phase E1 集成测试写时漏了 session-cancel 路径的 `token.cancel()` 触发器,`HangingThenCancel` MockProvider 是永远挂起的 stream,`run_chat_loop` 永久挂起死等 oneshot。修正 = 加 watcher `poll mock.call_count >= 1 → token.cancel()`,**对齐 `agent_loop_ask_user_question_session_cancel` 既有范式**(PR2 已有同款测试,新文件漏抄)。**抄模板时核对每行**,特别 cancel / session-switch / race-prone fixture。

- **关键教训 —「`stop_reason` 决定 chat_loop 行为」**:Tests 2/4/5 初版用 `stop_reason=end_turn`,LLM 发 `tool_use` 但 chat_loop 直接 exit(`chat_loop.rs:2017` `should_continue = stop_reason == "tool_use" && !tool_calls.is_empty()`),工具完全没触发。修正 = Turn 1 `stop_reason=tool_use` + 加 Turn 2(text + end_turn)收口。**测试 fixture 必须反映真实 LLM 协议**,end_turn 不带 tool_use,tool_use 必须带 tool_use(否则 loop 提前退出)。

- **测试**:1348 后端(cargo test --lib)+ 794 前端(vitest)+ vue-tsc 0 err + pnpm build 成功。Phase E1 集成测试 5/5(AC1/AC7/AC9/AC12/AC16)+ Phase E2 IPC 测试 4/4 + Phase E3 IPC 测试 5/5(含 Yolo root guard,gated on `is_running_as_root()`)。前端组件测试 23/23(RequestModeChangeCard)。零回归。

- **推后期**:Timeout + auto-decide / 多 mode 排队合并 / 自由文本 reason 编辑 / 跨 session mode 同步 / LLM 申请切到 `background` / 切回 user 上次手动 mode / App crash 恢复 pending / `for turn in 1..=turn_limit` 重构 while 循环零消耗 / 多语言 reason / `Resolve<Option<PendingInteraction>>` 在 `message_history_replay` 接入以展示历史 mode change 卡片。spec 增量(tool-contract / permission-layer / agent-loop-architecture / frontend/chat)已在 ROADMAP §1.2 行引用,内容 follow-up 补。


### 2026-07-06/07 — B6+ B subagent dispatch 动态选模型(优先级链收口 `dispatch > DB > frontmatter > parent`)

**Context**: B6+ A(07-03 frontmatter `model:` 声明)+ C(07-03 DB override + Settings UI)都是**声明性** —— 写一次,所有 dispatch 该 agent 都用那个 model。缺**临时性**单次覆盖:parent 用 Claude 写代码,dispatch reviewer 时临时指定 GPT-4o 做跨模型对抗性 review,不想改 reviewer 的全局默认。两条入口:LLM path(`dispatch_subagent` 加 `model` 参)+ user path(`@@agent --model=<X> <task>` 前缀)。任务 `.trellis/tasks/archive/2026-07/07-06-b6plus-b-dispatch-model-arg/`,PRD R1-R6 + AC1-12。

**关键决策(5 个)**:

1. **优先级链单点接入(`Option::or` 一行)**:`dispatch_model.clone().or(resolved_lower)` —— dispatch override 在最高位,`None` 时 `Option::or` 短路到 A/C 的 `resolve_final_model`(DB > frontmatter)。`resolve_final_model` / `resolve_worker_provider` **签名零改动**(两者既有单测零回归,A/C 零回归)。否决"改 `resolve_final_model` 签名加 dispatch 参" —— 会污染 A/C 纯函数边界,且 dispatch override 生命周期与 DB/frontmatter 不同(临时 vs 持久),不应塞进同一解析器。

2. **display_name 反查而非强求 id**:LLM path 的 schema `model` enum 值 = display_name(system prompt 不列模型,LLM 无从猜 UUID);user path 的 `--model=<X>` 接受 id 或 display_name。后端 `resolve_model_by_name_or_id` 先 id passthrough(`get_model` O(1)),miss 再 `list_models` 反查 display_name(多同名取首 + `warn!`,deterministic;DB 不强约束 display_name 唯一)。比强求 UUID 友好,成本 ~20 行 + 测试。

3. **`--model=` flag 位置约束(紧跟 agent 名、task 之前)**:git/cargo flag 语义。task 中间的 `--model=` 不误解析(task 文本常含 flag);`chat.ts` 扩展原 `@@<name> <task>` 正则,在 name 与 task 之间可选插入 `--model=<X>`。`@@agent --model=` 后无值 → 解析失败报清晰错误(不静默丢弃,也不误解析 task 文本)。

4. **失效兜底而非报错**:dispatch 指定的 model 不在 catalog(被删 / display_name 拼错 / id 不存在 / 反查无果)→ 不失败,`warn!` + 降级到 `resolve_final_model` 结果(再无则 parent)。临时 override 不持久化(不写 DB/frontmatter,永久改走 Settings UI / C)。

5. **可观测零特例**:`subagent_runs.model_display` + tool_result `[model:]` 行复用 A/C 既有的 `worker_display`(`resolve_worker_provider` 第三项),dispatch 临时命中也走同一路径,无特例分支;`format_dispatch_result_with_model` 不改。

**Consequences**: B6+ A/B/C 三档完整,优先级链 `dispatch > DB > frontmatter > parent` 收口。会话内 CRUD model 的 schema enum 滞后(下会话刷新,失效兜底兜住)。worker 自身不能选 model(`dispatch_subagent` 被 `STRUCTURALLY_DISABLED`,no nesting),model 选择权只在 parent(LLM 或 user)。

**关联**: PRD + design + implement `.trellis/tasks/archive/2026-07/07-06-b6plus-b-dispatch-model-arg/`;代码 commit `996aa2e`(`dispatch.rs` +374 核心优先级接入 + `resolve_model_by_name_or_id` / `mod.rs` +142 schema `model` enum + `ForcedDispatch.model_id` + `ModelOption` / `chat_loop.rs` +49 / `chat.ts` +124 `@@ --model=` 解析 / `chat.test.ts` +182,7 文件 +852/-31)+ spec commit `dc3e422`(agent-loop 优先级表 row26 + tool-contract §B6+B + frontend/chat `@@ --model=` 解析);复用 A 的 `resolve_worker_provider` + C 的 `resolve_final_model`(两者签名零改动)。


### 2026-07-06 — V2-2+ 自主记忆可观测性 + 管理面板(A1-A11 后端 + B1-B5 前端)

**Context**: V2 2 期自主记忆(06-29)全是 agent 自主闭环 —— 写(`remember`)/ 召回(FTS + pitfall)/ 反思(P4 auto-reflect)/ 提升(P5 状态机)/ 卫生(dedup + age-out)。用户面对的是黑盒:看不到命中、改不了内容、转不了状态、删不了过时项。V2-2+ 补**人**的入口。任务 `.trellis/tasks/archive/2026-07/07-06-am-observability-panel/`,PRD R1-R5 + design D1-D7。

**决策(D1-D7 + 实施)**:

1. **`edited_by_user` provenance 列(D1)**:autonomous_memories 加 `BOOLEAN NOT NULL DEFAULT 0`。0=agent 写(`remember`/P4),1=人工编辑(`update_memory`)。migration 复用 `add_autonomous_memories_column_if_missing` helper(PRAGMA table_info 检查,非破坏,旧行回填 0)。`MemoryRow` 加 `#[serde(default)]` 向后兼容。UI 据此渲染「人工编辑」徽标。
2. **`validate_memory_text` 安全网提取(D2)**:从 `insert_memory` 抽出 empty/超长/sensitive-regex/sensitive-path/temp-path + home 泛化为单源 helper,`insert_memory` + `update_memory` 共用。**否决**两条独立安全网 —— 人工编辑必须复用 agent 写入的护栏,否则用户手改能绕过 sensitive 检查。回归测试 `insert_memory_still_safe_after_helper_extract` 锁基线(P4/P5 依赖)。
3. **`ChatEvent::Recall` 只读 / 非持久化(D4/A7)**:新 `Recall { hits: Vec<RecallHit> }` 变体,跟 `Retrying` 同类(transient,不写 messages DB shape)。`RecallHit { memory_id, title, kind, source }` **无 `rename_all`**(snake_case),对齐既有 ChatEvent nested-payload 约定(`Retrying`/`FileInjections` 均 snake)。与 `MemoryRow`(独立 `#[serde(rename_all="camelCase")]`)是两个类型,勿混。
4. **sibling 重构,原 fn 退化为 thin wrapper(A8/A9)**:`build_recall_text_with_rows` 返 `(text, rows)`;`recall_pitfall_with_hits` 返 `(PitfallRecall, Vec<MemoryRow>)`。**关键约束**:原 `build_recall_text` / `recall_pitfall` 保留为 wrapper(`.map(|(t,_)| t)` / `.0`),只为 P2/P3 既有测试零回归 — production 走 with_rows。`PitfallRecall` enum 字节不变(Footnote/SoftBlock/None)→ P3/P4/P5 闭环零回归。4 个 dead-code 警告加 `#[allow(dead_code)]`(镜像 `recall_pitfall_footnote` 先例)。
5. **worker sink 结构隔离(AC7)**:`SubagentBufferSink.emit_chat_event` 只调 `self.record()`(→ `subagent:event` channel),**无 `chat-event` emit 路径**。结构性锁定:测试 `worker_sink_does_not_forward_recall_to_main_chat`。worker 子记忆命中永不冒泡到用户聊天。
6. **状态机矩阵 — 前端只读副本 + 后端硬墙(D6)**:前端 `LEGAL_STATUS_TRANSITIONS`(memory.ts 导出)= backend `update_status` 矩阵的只读副本。RuntimeMemoryModal dropdown 只 OFFER 合法目标;**backend 永远 re-validate**(transactional 二次校验),race / stale dropdown 也兜得住。前端**不**做合法性检查(会跟 backend 漂移)。转 demoted 弹内联 reason input(矩阵仅 →demoted 接受 reason)。
7. **recall 状态归 feature store(D7/B1)**:`recallHitsBySession: reactive(new Map)` 落 `useMemoryStore`,**不**归 streamController —— `state-management.md:131-139` 跨切面领域状态归 feature store,controller 只路由(checklist-routing 先例:`handleToolCall` 内 `CHECKLIST_TOOL_NAME` 分支,`streamEvents.ts`)。`handleChatEvent` 的 `case "recall"` 调 `pushRecallHits`;`startRequest` 清空(per-turn 累积,新 user message 清空 — chip 显示「本次召回」非累计)。`recallHitsForSession` 是纯读 computed(getter 不 mutate 自己 track 的 deps — `state-management.md:166-212` 硬规则)。

**Recall event emit 点(3 处)+ defensive arm**:
- FTS 召回(turn start,`chat_loop.rs:1391`)
- pitfall 召回(tool dispatch,`:2496` / `:3267`)
- LLM stream 路径 defensive match arm(`:1710`)丢弃漏过来的 Recall(只读事件不该来自 LLM)
- `emit_recall_event` helper 集中 emit 逻辑

**前端 UI(B2-B4)**:
- `MemoryPreview.vue` runtime row:hitCount/lastUsedAt chip(AC1,仅 hitCount>0)+ 「人工编辑」徽标 + 行点击 emit `manage`(宿主决定开哪个 modal,避免 MemoryPreview 硬绑)。delete 按钮 `@click.stop` 不冒泡。
- `RuntimeMemoryModal.vue`(新建):reka-ui Dialog 6 件套(复刻 MemoryModal sizing 80vw/min640/max900/80vh + zoom 动画)。嵌套在 MemoryModal Dialog 内(reka-ui 2.9.9 支持嵌套,焦点陷阱移到最内层)。统计区 + 状态 Select(复刻 ModelForm 模式,portal 子元素 `:deep()`)+ 编辑(native input/textarea,reka-ui 2.9.9 无 TextField)+ 删除(复用 ConfirmDialog,portal 到 body,z-index 1100 < Dialog 2000 层叠正确)。
- `ChatPanel.vue` recall chip(header 下、main 上方;MessageList 无 banner slot,ChatPanel 是跨切面 overlay 宿主)。折叠「本次召回 N 条」→ 展开 按源分组(语义/fts + 陷阱/pitfall)。

**Consequences**:
- 用户能看 agent 召回了什么(recall chip)、改写错记忆(RuntimeMemoryModal 编辑,带安全网)、手动转状态(矩阵驱动 dropdown,backend 硬墙)、删过时记忆。
- `insert_memory` 安全网单源化(P4/P5 未来扩展只改一处);P3/P4/P5 自动闭环零回归(31 + 7 + 7 既有测试全绿)。
- worker 隔离结构性锁定,未来加新 ChatEvent 变体也继承(chat-event channel 永不来自 worker sink)。
- recall hits per-session 累积,session 关闭不清(同 messagesBySession LRU 语义);每条 ~80 字节,内存可接受。

**非阻塞观察(trellis-check 评估「可接受」)**:
- `RecallHit.memory_id` 是 SQLite auto-id(对齐 `MemoryRow.id`),非 UUID — RuntimeMemoryModal 用它开 row,删除仍走 UUID。
- reka-ui Select popper 在 jsdom 不渲染 items 直到 open,矩阵逻辑改测导出常量(`LEGAL_STATUS_TRANSITIONS`)而非 DOM — 矩阵是 unit under test,Select 交互是 reka-ui 的责任。
- IPC-level 合法/非法转换 roundtrip 测试缺失 — 项目惯例 IPC 层薄包裹、逻辑在 db 层测(矩阵已被现有 `update_status_*` 测试覆盖)。

**关联**: PRD `.trellis/tasks/archive/2026-07/07-06-am-observability-panel/`(prd + design + implement)+ spec [backend/memory.md §V2-2+](../../.trellis/spec/backend/memory.md) + [frontend/memory-ui.md §V2-2+](../../.trellis/spec/frontend/memory-ui.md);1300 后端(`cargo test --lib`)+ 757 前端(`pnpm test`)+ vue-tsc 0 err + fmt 干净。2 commits(Phase A `08fb30a` / Phase B `a0c2c4f`),分支 `feat/am-observability-panel-phase-a`。


### 2026-07-06 — CI 测试自动化管线(双 job 并行 + cargo fmt gate + 预存 flaky 修复)

**Context**: V2 第三档 E1。项目积累 1274 cargo + 718 vitest + vue-tsc 全靠手动跑(`PKG_CONFIG_PATH=... cargo test` + `pnpm test` + `vue-tsc`),无防回归;最近 A5+ 一轮(1274/718)已现手动验证摩擦,journal 记"1266 全绿不准,Step 5 后即 fail"——正是 CI 该堵的回归类型。DEBT.md 0 open items,纯前瞻性基建。

**决策**:

1. **scope A(test+type,不出包)**:CI 核心使命是防回归;`cargo test --lib` 编译期已编全 crate + 1274 单测 + 718 vitest + vue-tsc 覆盖 95%+。出包留独立 release.yml(tag-triggered),不进 PR。**否决 A+B**:每个 PR 多 10-15min 产没人用的 .deb。
2. **双 job 并行(rust + frontend)**:frontend 不需 webkit,独立快跑;总时长 ≈ rust job。
3. **cargo fmt gate + 全量 fmt 前置**:Q3 决议 C。fmt 当前不干净(at_file.rs 等),加 gate 必先全量 fmt(118 文件机械改动,单独 commit)。clippy 留 follow-up(首次状态未知 + 修复含判断,先本地清再加 gate)。
4. **apt 系统依赖清单**:即使只跑 `cargo test --lib`,build.rs(`tauri_build::build()`)编译期就要 pkg-config 找 webkit → apt install 必选,与 scope 无关。`libwebkit2gtk-4.1-dev`(Tauri 2 用 4.1 非 4.0)+ build-essential + libxdo-dev + librsvg2-dev + libayatana-appindicator3-dev;reqwest 用 rustls 故省 libssl-dev。
5. **paths-ignore + concurrency**:忽略 `**/*.md`/`docs/**`/`.trellis/**` 纯文档改动(每次 task 收尾改 .trellis,触发 CI 是浪费 GH 免费分钟);`concurrency cancel-in-progress` 同 ref 重复 push 取消旧 run。
6. **CI 首跑暴露 2 个预存 flaky**(项目此前无 CI,本地 N 次侥幸过):
   - **background shell drain race**(生产代码修复 `in_memory.rs::drain_notifications`)— destructive pop 与 shell 完成 push 竞速;echo fork+exec+exit+push(几 ms)可能晚于 turn 切换(μs),drain pop 空队列。**真实生产 race**(快 shell + loop 早结束 → notification 永不 drain,LLM 不知道 shell 完成),不只测试 flaky。修复 = 队列空 + 近期(<200ms)running shell 时 yield+poll(5ms,cap 100ms),dev server(>200ms)不受影响,队列非空/无 running 立即返回(原行为保留)。
   - **loader mtime fence 精度**(测试侧修复 `memory/tests.rs`)— 两次连续 fs::write 间隔过短 FS mtime 精度不足区分(ext4 ns 但 overlay/tmpfs + 并行负载弱化 delta);原 sleep 15ms 在写 v2 之后对 v2 自身 mtime 无效(只保证落盘,不保证推进);改 spin until mtime 真推进(cap 2s,确定性,失败 panic = FS 精度不足本身是 fence-invalidating bug)。

**结果**:CI 双 job 全绿(rust ~5min / frontend ~2min);drain race 修复 10/10 单跑 + 3x 全量 1274/1274 稳定;mtime fence 3x 全量稳定。clippy gate 记 follow-up(ROADMAP §2)。


### 2026-07-06 — C2+ 循环检测主动干预(per-run-local count + QuestionStore 复用 + 三分支 + worker 直接 break)

**Context**: C2(06-24)循环检测命中只注入软提示 hint 到 tool_result,不终止 loop,MAX_TURNS=200 是唯一硬兜底。死循环(反复 `read_file` 同路径 / `edit_file` 同一失败 `old_string` / 近似 `shell`)烧满 200 turn 的敞口未堵。C2+ 补中间主动层:软提示连续失效 N 次后,harness 主动询问用户是否终止。

**决策**:

1. **N=3 + Hard/Soft 共用单一计数器**:不区别对待。Hard 零假阳(~5 连 byte-identical 才问),Soft 同 3 轮。两套计数器增状态机复杂度无收益。
2. **per-`run_chat_loop`-local `loop_hit_count`**:跟 `loop_window` 同生命周期,跨 turn 累积,worker 复用 `run_chat_loop` 自动继承。不落 DB(loop 是当次行为)。`verdict==None` 一轮即归零(连续性重置)。
3. **复用 QuestionStore 全链而非新 tool**:chat_loop 顶层 caller(harness-driven,非 LLM tool 路径)— `register(session_id, "loop_intervention_<turn>", payload)` + `emit_tool_question` + `resolve_tool_question` IPC + 前端 `<AskUserQuestionCard>`。否决「合成 ask_user_question tool_use」(伪造 LLM 没发起的 tool_use 语义混乱)+ 否决「新独立 oneshot+event」(违反 DRY,前端要新组件)。chat_loop 已有 `question_store` + `sink` 参数(**28 参签名零改动** — 关键可行性确认,PR2 前摸清 `chat_loop.rs:392`)。
4. **三分支 select!**:`biased` 第一位 `token.cancelled()`(用户 Stop 期间不悬挂)。终止 → `Done{stop_reason="loop_terminated"}`(区分 cancel)/ 继续 → count 清零 + 增强 hint 注入 result message / cancel → `Done{cancelled}`。`QuestionResponse::Cancelled`(用户跳过 card)归「终止」分支(安全默认停止)。
5. **worker `effective_is_worker` gate 直接 break**:不弹 banner(避免占父 QuestionStore slot + 避免打扰用户),`dispatch_result` caller-append `[loop terminated: ...]` 告知父 agent。worker 烧钱风险本就小(独立 token 预算 + 更短 max_turns)。否决「WorkerAskBanner round-trip」。
6. **`AuditKind::LoopIntervention` 无 migration**:enum 变体 + `as_str()` 走现成 `kind` TEXT 列 + `record_loop_intervention_audit`(仿 `record_message_resend_audit`,best-effort 非事务内)。payload `{hit_count, verdict_kind, action, run_id}`。C2+ 是用户可见行为(比 C2 被动 hint 高一级),应落审计 + 为 E2 trace viewer 铺路。worker 路径不写 audit(R5,worker 无独立审计 surface)。
7. **`AlreadyPending` 降级**:LLM 并发 `ask_user_question` 占 slot 时,`register` 返 `AlreadyPending` → C2+ 本轮跳过(走原 hint,不阻塞),下轮再试。QuestionStore 单 pending gate 天然消解。

**3 个实现偏差(trellis-check 评估「可接受」+ 有代码注释)**:
- `run_id: Option<&str>` 第 4 字段(design 原 3 字段):future-proofing,主 loop 传 None,worker 未来 audit surface 传 Some。additive,不违反 R4 三字段契约。
- dispatch caller-append(不加 `format_dispatch_result_with_model` 第 5 参):通过 sink `was_loop_terminated: AtomicBool`(同 `was_cancelled` 模式)在 `run_subagent` 追加,跟 `worker_changes_summary` 同款 tail-append。format 函数签名稳定。
- worker 路由复用 `SubagentStatus::Incomplete`(不加 `LoopTerminated` 变体):避免 DB CHECK 约束 + migration + 前端 drawer 状态胶囊波及。`[loop terminated: ...]` tool_result 行已让父 LLM 看到终止,专门 DB 列是 over-engineering。符合 R5。

**Consequences**:
- agent 死循环时(~5 连相同 call),用户在第 3 次 detect 命中拿到显式终止入口,不再被动烧满 200 turn 或手动 Stop。
- C2 原 hint 路径不变(loop_hit_count < 3 时走原 `loop_hint`);`ask_user_question` LLM tool 路径不变;`loop_detection.rs` 零改动(31 单测)。
- worker 死循环自动终止 + 告知父,父 agent 决策重试/换路径/接受。
- `Done.stop_reason` 三态:`loop_terminated` / `cancelled` / `end_turn`,前端 `WorkerTextTimeline.vue` 当 opaque 字符串渲染,无新 case。
- `tests_agent_loop::agent_loop_max_turns_emits_done_marker` 加「继续」resolver 循环(200 连相同 list_dir 现在第 3 轮触发 C2+ 阻塞,这正是 C2+ 设计目的)。

**关联**: PRD `.trellis/tasks/archive/2026-07/07-05-c2-loop-active-intervention/`(prd + design + implement)+ spec [tool-contract "C2+ loop intervention"](../../.trellis/spec/backend/tool-contract.md);1282 后端(`cargo test --lib`)+ 728 前端(`pnpm test`)+ vue-tsc 0 err;clippy gate 受预存 rustc 1.82 vs deps 1.85+ 环境阻塞(E1 follow-up,非 C2+ 引入)。


### 2026-07-05 — A5+ LLM 网络健壮性(retry_open wrapper + Full Jitter + 首字节前重试 + headers 字段扩展)

**Context**: A5(07-02)错误契约落地 5 类 LlmError 分类,但 provider 层无重试 — 单次 503 / 429 / 网络抖动就让整轮 turn 失败,长会话(多 Provider 中转 + 国内网络)脆。DESIGN §5.1 风险表原列"LLM 流式 token 断连 → 实现重连,断点续传用 message ID"作退路,但调研(`docs/research/llm-network-resilience-survey.md` §5.4)证实 SSE 协议无 resumption,message ID 续传不可行,只能整请求重发。

**决策**:

1. **外层 wrapper 落点**(`llm/retry.rs::retry_open`)而非 Provider trait 内:Provider 应专注协议转换不感知 retry;单一 source of retry 逻辑可单测;wrapper 可见 chat_loop 的 `token`(R7 取消)与 `sink`(R8 前端事件)。Provider trait 签名零改动(constraint 1)。
2. **Full Jitter**(`uniform(0, min(cap, base·2^attempt))`)而非纯指数:AWS Architecture Blog 共识 — 纯指数让并发客户端聚集(同步退避 thundering herd)。`RetryPolicy::default()`: max=3 / base=0.5s / cap=30s / budget=60s / retry_after_cap=60s。
3. **首字节前重试边界**(对齐 Claude Code "before visible output"):`retry_open` 一旦收到任何 `Ok(ChatEvent)` 即返回 `OpenOutcome::Stream`,之后所有 stream Err 在 chat_loop per-event loop 处理(had_error + ERROR_MARKER + partial tool),**不回 retry**。因 everlasting tool 执行在 stream 完成后,首字节前重发 = 零 tool 副作用,不需幂等 key / 去重表。比 Claude Code(流中可能有 visible output)更彻底。
4. **`LlmError` 加 `headers: HeaderMap` 字段**(`RateLimit`/`Server`):为 `parse_retry_after` 解析 advisory 提供 source。**5 类名称与分类逻辑不变**(prd constraint 3 的边界细化,非破坏);headers 不入序列化(无 DB migration)。`Auth`/`InvalidRequest`/`Network` 不带 header(网络错误无 response,4xx 非 429 不重试)。
5. **retry-after 优先 + 60s 二次封顶**:advisory 命中覆盖 jitter(尊重服务端意图),但封顶 `retry_after_cap=60s`(SDK parity — Anthropic/OpenAI 都 60s),更长 advisory fallthrough 到 jitter。解析 5 格式:`retry-after-ms` / `retry-after`(秒/HTTP-date)/ OpenAI `x-ratelimit-reset-requests`/`-tokens`(Go duration,自写 parser 不引 humantime)。
6. **双向独立熔断**(`max_retries` 次数 + `budget` 总 sleep):任一触达即停。budget(60s)防 OpenCode 式"session 死几小时"失败模式。Step 8 测试覆盖 budget-先 / max-先 两路径。
7. **取消语义**:`retry_open` 两个 select(首字节 await / backoff sleep)都 `biased` 第一位 `token.cancelled()` — sleep 中取消立即响应(返回 `Cancelled`,chat_loop 走 C1 路径不 had_error)。**这改变了 Step 5 前的 cancel 时序**(原 provider.send 同步立即记 call_count → cancel 几乎不可能在 send 前;retry_open 多一层 async + 入口 is_cancelled 检查 → cancel 可在 send 前 short-circuit,这是预期语义 — cancel 早响应不浪费请求),`agent_loop_ask_user_question_session_cancel` 测试相应从固定 80ms 改为"等 call_count>=1 再 cancel"精确同步到"stream pending, send done"窗口。
8. **Retry 不入审计**:transient UX,不入 AuditKind(prd grill §4 锁定,避免 17 类 AuditKind 膨胀)。
9. **命名演进**:design 原写 `retry_send`/`RetryOutcome`/`ChatEventSink`,实现中演化为 `retry_open`(语义更准 — 它 open stream 不消费到终态)/ `OpenOutcome::Stream|Cancelled`(chat_loop 拿到 Stream 后用既有 select loop 消费,零改动)/ `RetrySink`(故意做窄 — 只 emit retrying,不透传 ChatEvent,透传靠 OpenOutcome::Stream)。commit dd00104 msg 记录。

**Consequences**:
- 长会话遇 503/429/网络抖动自动重试(Full Jitter + retry-after),用户看到"↩ 重试中 N/M,Ts 后重发…(reason)"chip,不再整轮失败重来。
- 首字节后断连(含 stream 中途 Network err)维持现状(chat_loop had_error + partial tool 执行)— 不引入新风险。
- token 统计不重复(R9):retry_open 首字节前失败不消费 stream → 不 emit Done → `update_last_turn_usage`(UPDATE OVERWRITE,只在 Done arm)只记最终成功 turn 一次。集成测试直查 SQL `sessions.last_input_tokens == <success_usage>` 验证。
- `LlmError::RateLimit`/`Server` 变体加 headers 字段 — 所有构造点同步改,`From<LlmError> for AppError` 边界更新,既有测试断言同步。
- Step 5 commit msg "1266 测试全绿"不准 — `agent_loop_ask_user_question_session_cancel` 实际在 Step 5 后即 fail(cancel 时序漂移),Step 6 收尾时修复(测试侧适配 retry_open 新 cancel 语义,非 retry_open bug)。

**关联**: PRD `.trellis/tasks/archive/2026-07/07-04-a5plus-llm-network-resilience/`(prd + design + implement)+ 调研 `docs/research/llm-network-resilience-survey.md` + spec `.trellis/spec/backend/llm-contract.md` "Scenario: LLM Retry / Backoff (A5+)";commit `a26e7e0`(Step 1-4 基础设施)+ `dd00104`(Step 5 接入)+ 本日 Step 6-9 收尾;1274 后端 + 718 前端 tests passed(0 failed)。


### 2026-07-04 — A2+ shell 精细判定(复合命令拆分取 max + grant 短路对结构元字符失效 + `>` 写重定向升 SideEffect)

**Context**: A2+B7(06-12/13)shell 三档分类用 first-token + 一刀切结构降级,有三个缺口:① **R1 安全** — `ls; rm -rf ~/notes` 首 token=`ls`,用户对 `ls` 点过"始终允许"后 Tier 4 prefix-grant 短路在 classify 之前直接放行,跳过结构降级;Tier 2 kill-list 故意不挡 `rm -rf <非根>`(`dangerous.rs` deliberately narrow),`~/notes` 在项目外 git 救不回。② **R2 体验** — 纯读管道 `git diff | head` / `ls | grep foo` 被一刀切降级 Ask,Plan 模式(本就是只读分析)每次都要放行。③ **R3 独立漏判** — 结构检测只查 `|`/`&&`/`;`,**不查 `>`** → `git diff > patch.txt` 走 first-token=git diff → ReadOnly → Plan 模式静默写文件。源方案过双 review(commit `fbb7ced` / `247ed68`)落 `docs/A2-SHELL-CLASSIFICATION.md`。

**决策**:

1. **P1+P2 同 PR**(方案 §4 锁定):先上 P2 拆分器而不收紧 P1 grant,复合命令仍会被 (a) 短路放行,拆分成果到不了 → 必须同 PR。implement.md 原计划 P1/P2 分两 commit,实际**单 commit**:代码高度耦合(`classify_prefix` 入口同时依赖 P1 `detect_write_redirect` + P2 `split_top_level`,sub-agent 一次实现),且 design §6 回滚 shape 明确"P1+P2 同 PR,实际回滚单元是整个 PR(`git revert <merge>`)"。P1 内部先于 P2(Step 1-2 → 3-6)仅作实现顺序与理解路径,非提交边界。
2. **自研零依赖拆分器** vs tree-sitter-bash vs 沙盒优先:选自研。tree-sitter-bash 重依赖 overkill(引号/转义 corner case 靠 4 态状态机 Normal/Single/Double/Escaped + 测试矩阵锁足够);P3 bubblewrap 沙盒远期独立 task(判定错了也限损,本任务不碰)。契合项目"自研 SSE / 自研 Provider trait"风格。
3. **`has_structural_metachar` v1 不引号感知**(`contains('|') || contains("&&") || contains(';')`):grant 短路前置用宽检测。false-positive 安全 — `echo "a;b"` 报 true → 跳过 grant 短路 → 回落 `classify_prefix` → 拆分器引号感知不拆 → 单段 echo → ReadOnly(结果正确,仅损失短路速度);false-negative 不可接受,故用宽检测而非引号感知。
4. **命令替换 `$()` / 反引号一律 Ask**(`has_command_substitution`,不看引号):fail-safe。`echo $(rm x)` 在 shell 展开阶段执行会真删文件,按外层 `echo` 放宽是危险误判。`'$()'` 单引号内字面也判 Ask(用户可放行,acceptable)。拆分器因此无需处理 `$()` 嵌套 → 状态机简化为 4 态。
5. **`detect_write_redirect` per-segment**:`>`/`>>`/`&>`(整体重定向)/`[N]>file` 升 SideEffect;`>&N`/`2>&1`(fd 复制,无文件副作用)/`<`/`<<`/`<<<`(输入,纯读)不升。被 `classify_single` 每段调用一次(P1 检测设计成可被 P2 拆分器复用的 per-segment 函数,prd Notes 锁定)。
6. **`ShellTrust::severity()` + 自由函数 `max_of()`** 不 derive Ord:内敛偏序(`ReadOnly=0 < SideEffect=1 < Ask=2`),避免改 enum trait 表面(序列化 / cross-type PartialOrd 风险)。
7. **单 `&`(bash 后台标记)不拆**:design §3.3 状态机表未明确,实现选择留在段内。bash 单 `&` 是后台执行,严格说应分离,但 v1 接受为盲区(design §7 风险表)—— `first_token` 把它当 token 一部分 → 非 whitelist → Ask 兜底。
8. **check.rs 两处 grant 短路合一**包进单个 `if !has_structural_metachar(cmd)`:(a) prefix-grant + worker run-grant 共享同一 gate(R1:worker 路径不能成 bypass),而非两个独立 if —— 语义等价,且改动更集中。

**Consequences**:
- 纯读管道/命令链(`git diff | head` / `ls && echo done` / `echo a; echo b`)所有模式静默放行(ReadOnly)— R2 恢复,Plan 模式体验修复。
- `ls; rm -rf ~/notes` + grant 不再被短路放行 → 回落 classify → 拆分 → rm 段 Ask → 整条 Ask — R1 收口(worker run-grant 同款,worker 路径不成 bypass)。
- `git diff > patch.txt` → SideEffect(Plan 弹窗 / Edit 静默 Allow)— R3 收口。
- 命令替换 / 空命令 / 全空段 一律 Ask(fail-safe 不变)。
- **v1 接受的盲区**(design §7):`VAR=val cmd` env-prefix(段首 token=`VAR=val` 非 whitelist → 整条 Ask,远期 first-token 可剥前缀)/ 单 `&` 后台不拆 / 拆分器引号误判(靠 grant 前置 `contains` 兜底 + Tier 2 kill-list 兜底灾难性模式)/ `$var` 变量展开(静态不可知,P3 沙盒兜底)。
- 七不变量全保持(grant schema 三 match_kind / Mode 三档 / 17 AuditKind / Tier 2 kill-list / Yolo bypass / shell.rs 执行层 / "不确定就 Ask");grant 表存量数据无需迁移(前置是代码层,grant 行不受影响,存量授权对单条命令仍生效)。

**关联**: PRD `.trellis/tasks/archive/2026-07/07-04-a2-shell-p1p2-classify/prd.md` + design.md + implement.md;方案 `docs/A2-SHELL-CLASSIFICATION.md`(经双 review);spec `.trellis/spec/backend/tool-contract.md` "Compound command classification (A2+)" 段;commit 见 `git log --grep a2-shell-p1p2-classify`;1237 tests passed(0 failed,55 shell_trust 新增/重判 + 3 check grant 短路集成)。


### 2026-07-03 — B6+ C subagent per-agent 模型 UI(builtin DB override + 写回 frontmatter + card/drawer 可观测性)

- **Context**:A 任务(`07-03-subagent-frontmatter-model`)已落地 frontmatter `model:` 声明 + 透传 + `[model: X]` wire 行,但留两块缺口:① 内置 agent(`researcher` / `general-purpose`)无 frontmatter 文件可配 model;② frontmatter 写 `models.id`(UUID)对人类极不友好。parent `07-03-subagent-per-agent-model-ui` + 4 child(0 DB / 1 优先级 / 1b 可观测性 / 2 写回 / 3 IPC / 4 前端 / 5 收尾)。
- **决策 D1 — 优先级 `DB override > frontmatter > parent`,DB 作用域 global**:brainstorm 一轮收敛。UI 全局偏好直觉胜出(一处配即生效,无需关心文件)。DB global 简化 schema(无 project 归属),builtin agent 唯一可配置入口。失效 override(catalog miss)`resolve_worker_provider` 内 `warn!` + 降级 parent,UI 标红"model 已删除",不级联清表(留 follow-up)。
- **决策 D2 — `resolve_final_model` 在 `run_subagent` 前置解析,`resolve_worker_provider` 本身不动**:`run_chat_loop` 24 参签名不变(25 行表新增 1 行为决策 D2 注释,无新 param)。A 任务的 6 个 `resolve_worker_provider` 单测零回归是本决策的关键 —— 优先级链与 catalog 解析在两个独立函数,后者只看到"已收敛的 Option<id>"。两层纯函数各自可单测。
- **决策 D3 — frontmatter 写回走行级编辑,不引 YAML crate**:读原文件 → 定位 fence → 仅改 `model:` 行 → 原子写(`.tmp`+rename)。body / 注释 / 字段顺序 / 引号风格原样保留;round-trip 整个 frontmatter 会丢所有这些,得不偿失。无 fence 文件**返错**(loader 会因缺 `name` 拒收,正常路径必有 fence;不隐式补 fence 避免魔法改写用户文件结构)。`apply_model_line` 纯函数 + `write_frontmatter_model` IO 包装,前者单测覆盖 8 case,后者覆盖 5 case。
- **决策 D4 — `subagent_runs.model_display` 直接取 `resolve_worker_provider` 第三项 `Option<String>`,catalog 命中写 `Some(name)`,parent 继承 / catalog miss 写 NULL**:与 `format_dispatch_result_with_model` 的 `[model:]` 行**完全一致**(`None` 同步省略行 + 写 NULL)。**不改 `run_subagent` 签名**(parent display 不在其入参,留 follow-up),thread 风险 + 测试同步成本不值;若要 parent 继承时也显示具体 model,需把 chat 入口 `ResolvedChatProvider.model_display_name` thread 到 `run_subagent`(A 任务 design §113/118 设想但**未落地**,已记入 subagents.md AC13 注释)。前端 card + drawer 据此显示 chip + inline model,`v-if` 隐藏 NULL(pre-C 旧 row / parent 继承 / catalog miss 降级 一律不报错)。
- **决策 D5 — `workerSummaryPreview` 同时 strip `[status:]` + `[model:]` 行**:chip 已独立显示 model + status,文本里再出现是视觉噪声。regex 过滤而非 substring replace,应对 `[status: error]` / `[status: completed]` / `[model: Claude Test]` / `[model: <id>]` 各种值。`extractToolResultDisplay` 不动(它处理 envelope,不是本任务的负担)。
- **决策 D6 — Settings UI 走 reactive Map per-row spinner 隔离**(仿 `subagentRuns.mergeStateByRunId`):多 agent 同时 set 互不阻塞;`finally` 清 spinner 防 stuck;`disabled` attribute 防双击 + store `if (spinnerByName.has(name))` 兜底(防 in-flight 期间 click 落 store)。
- **关键教训 —「跨层类型加字段,fixture 同步很贵」**:`SubagentRunSummary` + `SubagentRunRow` 各加 `modelDisplay: string | null`,3 个测试文件(subagentRuns.test / SubagentDrawer.test / WorkerMergeControls.test / ToolCallCard.test)共 4 处 fixture literal 编译失败。**加新 IPC 字段前先 grep 全部 `SubagentRunSummary | SubagentRunRow` 字面量 + 同步补 fixture**,否则 vue-tsc 拉一个 fail 1 测试 + 改 4 文件,比写 1 个新 component 还累。已沉淀进 `frontend/state-management.md §Cross-layer drift traps`。
- **关键教训 —「不引新 IPC 时 cargo test 时间 ~2min 拉到 1 个 6 字段 fixture 失败循环」**:`run_chat_loop` 不增参的决策 + `resolve_final_model` 在 `run_subagent` body 内现取 db 让 25 行表仅多 1 行注释(无新 param 同步 36+ agent_loop_* 测试),节省 20+ 行 fixture 同步成本 —— 是 D2 决策的"不增 param"设计正确的第二份证据。
- **测试**:1205 Rust(`cargo test --lib`,含 5 个 `resolve_final_model` + 3 个 `subagent_runs` model_display + 13 个 `loader` apply_model_line + write_frontmatter_model + 6 个 `subagent_overrides` CRUD)+ 712 TS vitest(`vue-tsc 0 err`)+ vue-tsc strict 全绿,既有 1191 Rust + 711 vitest 零回归。AC1-15 测覆盖(priority 4 + DB CRUD 6 + write-back 5+8 + IPC 2 + model_display persistence 3 + UI strip 1 + per-row spinner 1)。
- **推后期**:per-project 作用域 / model 删除级联清 override / parent display thread 到 run_subagent(完整 A 任务 design §113/118 落地) / dispatch 时动态选 model(`@@agent --model=`,B 任务) / `set_subagent_model` IPC 加 subagent cache list 缓存(`mtime-fenced` 已即时刷新,但 list 大时每次 set 重查 models N+1,小优化) / SubagentsTab 加"按 source 过滤"。


### 2026-07-02 — B9 生成式 UI(部分落地:selector/diff/code_block,button 推后期)

- **Context**:输出层生成式 UI。parent `07-02-b9-generative-ui` + 3 child(A use_ui 基础设施 / B code_block hljs / C diff 复用 DiffView)。brainstorm 收敛 6 决策(D1-D6),MVP 范围从"4 primitives 全做"缩到"selector 复用 + diff 只读 + code_block 高亮",button/diff应用/开关全推后期。
- **决策 D1 — 承载机制 = `use_ui` 单 tool + primitives 数组**(否决 `ui_render` content block):现有所有结构化输出都走 tool_use 且管线成熟(权限⑨/审计/persist_turn 持久化/前端 tool_name dispatch);新增 ContentBlock 变体要改 Anthropic+OpenAI 两 Provider wire,且 Anthropic 不原生认 ui_render block。B9 primitive 本质是 agent 主动决定展示 UI,跟"决定调 tool"无别 —— content block 唯一独占的"自然输出"优势在此场景是空的。架构文档(§⑭)预留的 ui_render/use_ui/ui:render 全仓零实现,B9 从零设计承载机制。
- **决策 D2 — 执行模型 = non-blocking 展示;selector 复用 ask_user_question**:use_ui 立即返 tool_result("已渲染 N 个"),不等用户交互。**selector 不重做,直接 = `ask_user_question`**(blocking oneshot 已 06-30 验证,语义 100% 重合)。前端 registry 统一 dispatch:`tool_name===ask_user_question`→AskUserQuestionCard;`tool_name===use_ui`→按 primitive.type 路由。
- **决策 D3 — 独立 button primitive 推后期**:B9 最大安全面(action 白名单 + 高危 action 过权限⑨,DESIGN.md:70)。首批 80% 用例(询问=selector/展示=diff+code_block)已覆盖。
- **决策 D4 — diff primitive MVP 只读 + 复制**(应用推后期):edit_file+权限⑨+DiffView 已覆盖"修改确认"全流程;diff primitive MVP 聚焦展示型,避免与 edit_file 并存两种修改模型造成 LLM 困惑。零新增安全面。
- **决策 D5 — session 开关 MVP 不做**:use_ui non-blocking 展示型无副作用,Mode(edit/plan/yolo)已是更通用控制层。
- **决策 D6 — code_block 高亮 = hljs `lib/common`**:最轻、marked-highlight 集成成熟。两入口共用 `renderCodeHtml`(markdown 代码块 + CodeBlockPrimitive),语言集永不分裂。
- **关键教训 —「先查代码再问用户」避免重复造轮子**:brainstorm 前探索发现 selector 已实质落地(ask_user_question)、diff 渲染层已落地(DiffView 且在 ToolCallCard:635 内嵌)、code_block 半落地(markdown 无高亮)。**4 个 primitive 里 2.5 个已以特化形式存在**,真正从零做的只有 button + 统一 UiCard 模型。Evidence rule 在 parent brainstorm 阶段就砍掉了"selector 怎么做""diff 渲染怎么实现"两个伪问题。
- **关键教训 —「registry 可扩展性」兑现于 Child B/C 零改动 dispatch**:Child A 设计 registry 为 `Record<type, Component>` Map + UiCard 遍历 dispatch,Child B/C 各自只改 registry 一行条目,UiCard/MessageItem dispatch 零改动。
- **关键教训 — hljs 接入改变 markdown HTML 输出 → 测试断言要跟**:marked-highlight 让代码块从 `<code>print(1)</code>` 变成 `<span class="hljs-built_in">print</span>...`。markdown.test.ts / MarkdownDetailModal.test.ts 的 `toContain("print(1)")` 失效。**接入改变输出的库时,先 grep 现有断言看哪些 substring 匹配会破**。
- **测试**:cargo test 1146+/use_ui 12 + vue-tsc 0 err + vitest 694/694(UiCard 8 + CodeBlockPrimitive 7 + DiffPrimitive 9 + 4 处断言适配 hljs)。端到端(tauri dev)待手动验收。
- **推后期**:独立 button primitive + action 白名单(D3)/ diff 应用(D4)/ session allow_generative_ui 开关(D5)/ 自由式 UI / form/chart/table primitive。


### 2026-07-02 — A5 错误处理完善(全栈错误契约:`AppCommandError` wire shape + 10 类型 `impl AppError` + 前端 errorBus)

- **Context**:`error-handling.md` 是半成品模板(五章里三章 `To be filled`)+ spec drift(称 `LlmErrorKind`/`Protocol`,代码实际 `LlmError`/`InvalidRequest`);代码侧 10 个对外错误类型仅 `LlmError` 有 `category()/user_message()`,Tauri command 全为 `Result<T,String>`(~65 处)无结构化错误,前端错误散点 `.catch(console.error)` 静默吞。本期目标:spec 补活文档 + Rust `AppError` trait 统一 + `AppCommandError` wire shape 一次性全切 IPC + 前端 `useErrorBus` 按 category 路由。
- **决策 D1 — IPC 错误 = `AppCommandError { category, kind, message, retryable, request_id }` 结构化 wire**(否决继续用 String):category 驱动前端路由,kind/message 供诊断与展示,request_id 关联 tracing。不带 stacktrace(IPC 体积 + 用户消息无 stack,stack 留 tracing log)。
- **决策 D2 — `retryable` 默认按 category 派生,本期零 override**(否决每 variant 手填):Server/Network/RateLimit=true,Auth/InvalidRequest=false。初版设计曾用 `BackgroundShellError::Timeout` 当 override 唯一样板,review 发现该 variant 不存在,整个 override 机制本期无真实案例,删除。
- **决策 D3 — 10 个领域 `From<E>` 手写 + `From<anyhow::Error>` 边界兜底**(否决 blanket `From<E: AppError>`):AppError impl 分散各类型文件,blanket 触发 coherence 冲突。anyhow 兜底必须,因 commands 大量 `?anyhow`,无此转换 PR-A5-3 编译失败(先 downcast 已知类型,未命中归 Server/`kind="Anyhow"`)。
- **决策 D4 — 一次性全切 IPC,无 String 兼容层,A5-3/A5-5 同次发布**(否决双协议期):errorBus `parseAppCommandError` 容错 String rejection 降级 Server/Unknown,降低迁移期回归风险;但前后端签名必须同次发布,消除"后端返对象/前端按 String 解析"中间窗口。不留 `From<AppCommandError> for String` 临时兼容层。
- **决策 D5 — 5 类 category 复用 LlmError**(否决 PermissionDenied/Cancelled/NotFound 独立类):LlmError 5 类已是成熟的 category 原型;其余 9 类型 variant 归并(典型 NotFound→InvalidRequest、Db→Server)。独立类后续按需扩。
- **决策 D6 — `PreFlightError::EmptyApiKey/DecryptFailed` → Auth**(非统一 InvalidRequest):前端 Auth 路由正是"引导去 Settings 检查 API key",语义对齐;NoModel/ProviderMissing/BuildFailed 仍归 InvalidRequest。
- **决策 D7 — 前端 `errors` 数组 `MAX_ERRORS=50` FIFO**(否决无限增长 / TTL):长会话 Server/Network 风暴防护;单条 dismiss / TTL 推 toast UI follow-up。
- **关键教训 —「planning drift 比 spec drift 更贵」**:review 发现 design.md §5 映射表 6 个类型的 variant 名凭印象虚构(`GitError::NotFound/Conflict`、`BackgroundShellError::Timeout`、`ReflectError::Parse`、`WebFetchError::Http4xx/Http5xx`、`QuestionStoreError::Duplicate`、`StatusTransitionError` 漏 Db),且"11 个 thiserror"计数错(实际 10 对外,含 2 手写)+ 漏 `ValidationError`(`pub(crate)`)。**写 planning 前必须 `rg "pub enum .*Error" -g '*.rs'` + 逐 `#[error]` 行核对真实 variant 名**,否则实施连环返工。已沉淀进 `error-handling.md §Common Mistakes`。
- **关键教训 —「trait 超类型约束的隐藏工作量要先核实」**:`trait AppError: std::error::Error` 对 `PreFlightError` 不成立(它无 Display 也无 Error impl,只有 `auth_message()/invalid_request_message()` 分方法)。PR-A5-2 必须先补两个 impl。三个类型(LlmError/PreFlightError/QuestionStoreError)现有对外接口形态各异,非"一刀切迁移"。
- **测试**:进行中(PR-A5-2 ~ A5-5 完成后补:10 类型 ~41 variant `category()/user_message()` 快照 + `HttpStatus` 4xx/5xx 分流 + `From<anyhow::Error>` 兜底 + grep `Result<String>` 残留 + `parseAppCommandError` 容错 + cargo/pnpm 全绿)。
- **推后期**:多语言 i18n `user_message` / 自动重试策略 / Telemetry Metrics(request_id→Sentry/OTel)/ PermissionDenied·Cancelled·NotFound 独立 category / legacy `.catch(console.error)` 全量替换 / toast UI 接 reka-ui + 单条 dismiss·TTL / `ValidationError`(`pub(crate)`)纳入 impl AppError。


### 2026-07-01 — read 族 tool 层硬卡解耦 + 敏感路径 deny/allow-list(+ `~` 展开)

**Context**: read 族(read_file/grep/glob/list_dir)的 tool 层 `assert_within_root` 与权限层 `ask_path` 口径冲突 —— 权限层 Tier 4 对项目外路径弹窗 ask,用户 Allow 后 `execute_tool` 内的 `assert_within_root` 又以 "outside project root" 硬拒。即"假 ask":弹窗点了允许也没用。用户原始动机:读 `~/.config/everlasting/commands/test-b3.md` 报错。目标:拉齐 Claude Code Read 能力(它默认能读项目外)+ 受控(不放弃审计/用户感知)。

**决策**:

1. **删 read 族 tool 层 `assert_within_root`** —— 边界判定收归权限层单一 source of truth,消除口径冲突。write/edit 的 `assert_within_root` 保留(defense in depth,写不可逆)。
2. **权限层 Tier 2.5 敏感路径 deny-list**(`permissions/sensitive.rs`,对标 `dangerous.rs`):中等档 pattern(私钥/`.env`/credentials),命中即硬 `Deny`、含 yolo、不可绕过。**仅项目外 lexical**生效(Q1.2:项目内 `.env` 信任);**项目内 symlink 逃逸**额外挡(canonicalize 后到项目外且敏感 —— 恢复原 `assert_within_root` 的 symlink 防御,lexical deny-list 单独挡不住)。
3. **Tier 4 受信 allow-list**:`~/.config/everlasting/**` 免 ask 直接 Allow(app 自己的运行时数据,agent 读它本不该弹窗)。优先级 deny > allow > ask。
4. **新 helper `projects::boundary::resolve_path`** 展开 `~`/`~/...` → home —— read 族 4 tool + check.rs 2 处 abs_path 共用。这是 allow-list 实用的硬前提(LLM 自然传 `~/...`,否则 `~` 被当字面目录名 → 路径错)。
5. **双 anchor**:`cwd` 决定 ask vs silent-Allow(权限层历史不变);`worktree_path`(项目根)决定 deny/allow 的"项目外"触发 —— 避免 session cwd 是子目录时项目根文件被误判 outside。`PermissionContext` 加 `worktree_path` 字段(5 处构造点)。

**Consequences**:

- read 项目外受控:edit/plan 弹 ask、yolo 放行+审计、敏感硬 deny、everlasting data 免 ask。
- 私钥/凭证/项目外 `.env` 不进 LLM context(不可逆泄露面堵住)。
- **已知 gap**(OOS):grep/glob 的 deny-list 只匹配 `path` 参数(搜索根),搜索结果里偶遇敏感文件内容不额外过滤(等同 redaction,留 follow-up)。项目内真 `.env`/`*.pem` 信任不挡(Q1.2)。
- write 族"假 ask"同构问题(write 项目外 ask 通过后仍被 tool 层拒)留后续 follow-up —— 本 task 仅保证 write 零回归。

**两轮 review 各补一个盲区**:trellis-check 抓到 symlink escape(安全回归,已修);用户 review 抓到 `~` 不解析(allow-list 形同虚设,已修)。两者自验时都用"绝对路径测试"绕过。

**关联**: PRD `.trellis/tasks/archive/2026-07/07-01-read-side-boundary-decouple/prd.md`;commit `87c91f0`;spec `.trellis/spec/backend/project-cwd-boundary.md` §5 + §7;1127 tests passed。

