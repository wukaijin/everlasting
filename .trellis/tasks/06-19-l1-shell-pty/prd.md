# L1 — 后台 shell + 完成通知(最小可用版 / L1a)

> brainstorm 进行中。范围定为 L1a(后台 `tokio::process::Command` + 退出注入通知,不带 PTY)。
> 本 PRD 不重复调研内容,只沉淀本项目落地决策;详细背景见 `research/l1-background-shell-survey.md`(待写)+ `docs/spikes/2026-06-19-async-parallel-tool-research.md` §5.1。

## Goal

把 `shell` tool 从"同步阻塞 + 120s/600s timeout"升级为"可选后台执行 + 完成时向 agent loop 注入系统通知",解决长任务(build / 大 test suite / 长 install)盖不住的问题。MVP = L1a(后台 `tokio::process::Command`,无 PTY、无 stdout 流式);PTY 与交互式 REPL 留 L1b。

## What I already know(代码现状 + 调研)

### `shell` tool 现状(`app/src-tauri/src/tools/shell.rs`)

- `timeout` 参数:默认 120s,max 600s;超时就 `kill_and_collect` + 部分输出 + timeout 标记。
- `process_group(0)` + `killpg` 已就位(RULE-E-002),子孙进程一并 kill。
- `apply_safe_env` 已就位(RULE-E-001),API key 不泄露。
- 全部同步:第 401 行 `tokio::select!` 等 child.wait 或 timeout,LLM 在结果返回前拿不到响应。
- 30 KB 输出 spill 到 `<cwd>/.everlasting/outputs/<uuid>.txt`,head+tail 1 KB preview。
- 已知痛点:600s cap 仍不够(pnpm 完整 install / 大项目 build / `--release` 慢档),超时就杀,LLM 拿不到结果。

### Agent loop 现状(`app/src-tauri/src/agent/chat_loop.rs`)

- 主循环在第 631 行 `provider.send` + 第 648 行 `loop {}`,每轮:LLM → tool 执行 → 收集 tool_result → 下一轮 LLM。
- 注入点候选(按优先级):
  - **行 919 之前**:`messages.push(msg)` 收集 assistant 响应
  - **行 631 之前**:`provider.send` 之前可以 prepend `pending_notifications`
  - **行 1360-1364**:`build_synthetic_tool_result_message` 后 push,这是 tool_result 路径
  - **推荐**:prepend 在 `provider.send` 前消费 `pending_notifications` 作为 user message(对齐 opencode-pty `<pty_exited>` 语义)
- `MAX_TURNS = 50` 已硬卡,L1 不能绕过。
- `CancellationToken` 走整个 agent loop,但 L1 后台 shell 跑的时候 agent loop 已经结束 → cancel token 的语义需要重新界定。

### 常驻状态现状(`app/src-tauri/src/state.rs` + `AppState`)

- `AppState` 是 `tauri::State<AppHandle>` 包装,跨请求共享。
- 已有的"跨请求常驻"先例:`PermissionStore`、`ReadGuard`、`SkillCache`、`MemoryWatcher` 都是 `Arc<Mutex<..>>` 形态挂在 state 上。
- session 列表从 SQLite 读,**不存内存**,所以 L1 的后台 session table 必须是进程内常驻 + session 关闭时清理。

### L2 调研已点透的两个隐藏成本点(`docs/spikes/2026-06-19-async-parallel-tool-research.md` §5.1)

**① daemon 化耦合**
- 后台 shell 跨多轮/跨多次 `invoke`(`agent` 在 `npm run build` 跑着时继续对话;build 在两次 send 之间完成)
- 必需形态:`AppState` 持有 `background_shells: HashMap<ShellSessionId, BackgroundShellMeta>` + `pending_notifications: VecDeque<SystemNotice>`,agent loop 每轮开头消费
- 与 [CLAUDE.md](../../CLAUDE.md) "daemon 化路线"(GUI 进程 vs Agent Daemon 分离)强耦合
- §5.1 建议:L1 与 daemon 化一并规划,**不要 ship 会被推翻的中间态** — **需在 PRD 中显式决策**

**② PTY vs 后台 Command(L1a vs L1b)**
- L1a = 后台 `tokio::process::Command`(管道模式,非 PTY),低
- L1b = `portable-pty` 真 PTY(支持 `pty_write` / 交互式 dev server),高(翻倍)
- 用户已选 L1a;L1b 留作 follow-up,§5.1 写"分阶段 L1a → L1b"

### L2 路线图已落地的可复用机制

- `kill_and_collect`(`shell.rs:162`)+ `process_group(0)`(`shell.rs:379`)→ L1a 直接复用
- `emit_chat_event_via_sink` + `emit_tool_result` → L1a 完成通知走同一 sink
- `build_synthetic_tool_result_message`(`agent/mod.rs` 已用)→ L1a 的"完成通知"也可以是 synthetic 形态,但更干净是 user message

### 业界范本(opencode-pty / Hermes)

- **opencode-pty**:`pty_spawn` 返回 ID + `<pty_exited>` XML 块注入
- **Hermes**: `terminal(background=true)` 返回 session_id + `process(action=poll|wait|log|kill|write)`
- **共识**:tool 协议**不变**(tool 仍同步返回),后台生命周期由会话句柄管理,完成注入通知

## Assumptions(待验证)

- [ ] A1:agent loop 注入 user message 必须放在 `provider.send` 之前(每轮开头)
- [ ] A2:后台 shell 的 `CancellationToken` = 当前请求的 cancel token 不再适用(因为 loop 已经结束),L1a 必须提供独立的 `kill_background(shell_id)` 机制
- [ ] A3:`AppState` 持有 `Mutex<HashMap<..>>` 即可(无需更细粒度锁);后台 shell 完成时 lock + push notification + unlock,agent loop 每轮消费时 lock + drain + unlock
- [ ] A4:session 关闭时(`delete_session`)必须 enumerate `background_shells` 并 kill 全部,不然 leak
- [ ] A5:Tauri app 退出时,后台 shell 需要 SIGKILL 进程组(RULE-E-002 可直接复用)
- [ ] A6:无 PTY = 无 ANSI 颜色 / 无进度条刷新,LLM 只看到"完成 + exit code",符合 L1a 定位

## Decisions

### [Q1] daemon 时序 = C(trait 抽象 + GUI 内 impl,预留 daemon 迁移)

**Context**:L1 必需跨请求常驻 `background_shells` + `pending_notifications`,但 daemon 化是 ROADMAP 第二档以后的事。§5.1 建议"不要 ship 会被推翻的中间态"。

**Decision**:抽 trait `BackgroundShellRegistry`,GUI 进程内 impl = 直接调 hashmap(本 PR);daemon 化后 impl = 走 Unix socket 转发(后续 PR,只换 impl 不动调用点)。

**Consequences**:
- **+**:迁移只换 impl,L1a 调用点稳定
- **+**:trait 边界同时把 L1b(PTY)、后续 L3(并行 subagent)的 registry 共用接口预留好
- **-**:L1a 多 30~50 行 trait 定义 + mock impl,轻量 over-engineer
- **-**:trait 设计错一次就锁死,得想清楚 trait 方法集(start / status / kill / drain_notifications)

### [Q2] tool API = B(3 个新 tool:`run_background_shell` + `shell_status` + `shell_kill`)

**Context**:LLM 调后台 shell + 查询 + 杀的 tool 协议需确定。三种形态选一。

**Decision**:3 个独立 tool,职责单一,语义清晰。

- `run_background_shell({command, working_directory?, max_runtime_ms?})` → `{shell_session_id: "bsh_xxx", status: "started", started_at: ..}`
- `shell_status({session_id})` → `{status: running|completed|failed|killed, exit_code?, stdout_preview, stderr_preview, full_output_path?, started_at, completed_at?}`
- `shell_kill({session_id})` → `{status: "killed", exit_code: -1}`

**Consequences**:
- **+**:对齐 Hermes terminal/process split,业界范本
- **+**:每个 tool schema 简单,LLM 容易正确选择
- **+**:L1b(PTY)扩展 = 加 `pty_write` / `pty_resize` / `pty_log` 平级 tool,不破坏现有 3 个
- **-**:tool 表多 3 个,LLM tool 选择有小幅负担(可由 tool description 引导)

### [Q3] 通知注入 = A(prepend user message,opencode-pty 风格)

**Context**:后台 shell 完成时,LLM 怎么知道。三个候选:prepend user message / 不主动注入等下一轮查 / 专用 system-notice 通道。

**Decision**:在 `provider.send` 之前,prepend 一条 user 角色消息:`[system] 后台 shell bsh_xxx 已完成,exit code N。调 shell_status(session_id="bsh_xxx") 看输出。`

**Consequences**:
- **+**:对齐 opencode-pty `<pty_exited>`,LLM 立即知道,自然决策
- **+**:走标准 messages 路径,与现有 synthetic tool_result_message 同形
- **+**:user message `[system]` 前缀明示"非用户发送",降低 LLM 困惑
- **-**:每轮 LLM context 多一条消息;但消息极短(~150 bytes),token 影响小
- **-**:MAX_TURNS = 50 是按 LLM turn 计,该消息算一次"通知 turn"吗?**需澄清**(见 Q6 边界)

### [Q4] 通知深度 = A(仅 exit code + session_id,LLM 主动 query)

**Context**:完成通知里塞多少信息。三个候选:仅 exit code / exit code + 完整回填 / 仅路径。

**Decision**:通知仅说"session_id X 已完成,exit code N",LLM 要看 stdout/stderr 主动调 `shell_status`。

**Consequences**:
- **+**:context 永远不被动塞大输出,token 可控
- **+**:LLM 主动 query 时可结合当前决策选择性看,符合 agent 主动 reasoning
- **-**:每个后台任务 LLM 会多 1 次 tool call(`shell_status`),但 status 工具轻量,影响小

### [Q5] 主动 query 机制 = 已在 Q2 决定(3 tool:`run_background_shell` + `shell_status` + `shell_kill`)

不再单列。

### [Q6] 超时 / 资源 = A(LLM 传 `max_runtime_ms`,默认 24h)

**Context**:后台 shell 不阻塞 agent loop 后,需要独立的超时机制。

**Decision**:`run_background_shell` 加 `max_runtime_ms?` 参数(int,ms,默认 86400000 = 24h,无上限)。后台 tokio task 中 `tokio::time::sleep(max_runtime_ms)` 触发时:`kill_and_collect` 进程组(E-002 复用)+ 注入 `[system] 后台 shell bsh_xxx 超时完成,exit code -1` 通知。

**Consequences**:
- **+**:与现有 `shell` `timeout` 参数语义对齐,LLM 认知一致
- **+**:默认 24h 是合理"过夜 build"上限,不会误杀
- **+**:无上限 = LLM 不传也接受,风险可控(LLM 调后台后忘记 kill = 用户可重启 app)
- **-**:进程组 kill 后台超时 = 与 `shell` 同步超时一样逻辑,代码可复用

### [Q7] 跨 session 可见性 + 权限 = A(session-scoped + Tier 4 SideEffect)

**Context**:session A 是否能 kill session B 的后台 shell;新 tool 的权限 tier。

**Decision**:
- **可见性**:`BackgroundShellRegistry::start()` / `status()` / `kill()` 都接受 `session_id` 参数,key = `(session_id, shell_session_id)`。`shell_status` / `shell_kill` 拒绝访问其他 session 的 shell(返回 `is_error: true`)。
- **权限 tier**:`run_background_shell` 与 `shell` 同 Tier 4 SideEffect,在 `Edit`/`Plan` mode 下默认 emit `permission:ask`(除非 session_tool_permissions 有 grant),`Yolo` mode 静默 Allow。

**Consequences**:
- **+**:与现有 `shell` 的 Tier 4 行为一致,UX 一致(用户在 modal 看到的就是同一种 prompt)
- **+**:跨 session 不可见 = 安全边界清晰,无需额外鉴权逻辑
- **-**:`run_background_shell` 在 Plan mode 下也会触发 modal(plan mode 默认不能跑 tool,但与 shell 对齐不引入新 mode 复杂度)
- **-**:session delete 时必须 enumerate 该 session 所有后台 shell + kill,trait 必须提供 `kill_all_for_session(session_id)` 方法

### 错误处理(默认设计,待 review 时再细化)

- **spawn 失败**(command not found / EACCES):后台 task 不创建,`run_background_shell` 直接返回 `is_error: true` + 错误信息,与现有 `shell` 行为一致
- **进程组 kill 失败**(ESRCH 以外):tracing::warn! 不 cascade(复用 `kill_and_collect` 已有逻辑)
- **notification 队列溢出**:无界 `VecDeque` 在 app 长时间运行后无界增长 → **设计决策**:`pending_notifications` 上限 100 条,溢出时丢最早的通知 + tracing::warn!(本任务 PR 实现)
- **session delete 中有 running shell**:trait `kill_all_for_session` 强制 SIGKILL 进程组(E-002),同步等所有 task 退出
- **app 退出时仍有 running shell**:Tauri `RunEvent::Exit` 钩子调 `kill_all` 一次,E-002 进程组 SIGKILL

### 全部决策汇总

| # | 决策项 | 选择 |
|---|---|---|
| Q1 | daemon 时序 | **C** trait 抽象 + GUI 内 impl |
| Q2 | tool API | **B** 3 个新 tool |
| Q3 | 通知注入 | **A** prepend user message |
| Q4 | 通知深度 | **A** 仅 exit code |
| Q5 | 主动 query | 由 Q2 决定 |
| Q6 | 超时 / 资源 | **A** `max_runtime_ms` 默认 24h |
| Q7 | 可见性 + 权限 | **A** session-scoped + Tier 4 |

## Requirements

### 后台执行能力(Q2 决策)

- LLM 可调 `run_background_shell({"command": str, "working_directory"?: str, "max_runtime_ms"?: int})`,立即返回 `{"shell_session_id": "bsh_xxx", "status": "started", "started_at": ..}`。
- LLM 可调 `shell_status({"session_id": str})` 查状态:`{status: running|completed|failed|killed, exit_code?, stdout_preview, stderr_preview, full_output_path?, started_at, completed_at?}`。
- LLM 可调 `shell_kill({"session_id": str})` 主动 kill:`{status: "killed", exit_code: -1}`。
- 3 个 tool 都是 session-scoped(Q7),`shell_status`/`shell_kill` 拒绝访问其他 session 的 shell(返回 `is_error: true`)。

### 完成通知(Q3 + Q4 决策)

- 后台 shell 退出时(exit / kill / 超时),agent loop 下一次 turn 开头 prepend 一条 user 消息:`[system] 后台 shell <shell_session_id> 已完成,exit code <N>。调 shell_status(session_id="<id>") 看输出。`
- 通知**仅**包含 `shell_session_id` + `exit_code`,**不**塞 stdout/stderr。LLM 主动调 `shell_status` 拉。
- 多个后台 shell 同轮完成 → 多条通知 prepend,顺序按完成时间排序。
- 通知注入位置:`agent/chat_loop.rs:631` `provider.send` 之前,从 `BackgroundShellRegistry::drain_notifications(session_id)` 取。

### 进程生命周期与资源(Q6 + Q7 决策)

- 后台 shell 跑在独立 tokio task,所有权归 `BackgroundShellRegistry`(trait 抽象 + GUI 内 impl)。
- `max_runtime_ms`:默认 86400000(24h),无上限(LLM 不传 = 跑到底)。后台 task `tokio::time::sleep` + `kill_and_collect` 进程组(E-002 复用)。
- session delete 时调 `BackgroundShellRegistry::kill_all_for_session(session_id)`,同步等所有 task 退出。
- Tauri app `RunEvent::Exit` 钩子调 `kill_all`,防止 app 退出后后台进程 leak。
- `pending_notifications` 上限 100 条 / session,溢出时丢最早 + tracing::warn!。

### 权限(Q7 决策)

- `run_background_shell` 与 `shell` 同 Tier 4 SideEffect,Edit/Plan mode 默认 emit `permission:ask`,Yolo mode 静默 Allow。
- `shell_status` / `shell_kill` = Tier 4.1 ReadOnly 类(无 cwd 写、无 env 改、无 spawn),**静默 Allow**(与 read_file/grep 同档)。但是跨 session 访问仍走 Q7 的 session_id 校验 → 即便 Tier 5 仍拒绝。

### Trait 抽象(Q1 决策)

```rust
#[async_trait]
pub trait BackgroundShellRegistry: Send + Sync {
    /// Start a background shell. Returns the new shell_session_id.
    async fn start(
        &self,
        session_id: &str,
        command: String,
        cwd: PathBuf,
        max_runtime_ms: Option<u64>,
    ) -> Result<String, BackgroundShellError>;

    /// Query status. session_id is the chat session, shell_session_id is the bg shell.
    async fn status(
        &self,
        session_id: &str,
        shell_session_id: &str,
    ) -> Result<BackgroundShellStatus, BackgroundShellError>;

    /// Force kill the process group. Idempotent.
    async fn kill(
        &self,
        session_id: &str,
        shell_session_id: &str,
    ) -> Result<(), BackgroundShellError>;

    /// Kill all shells belonging to a chat session. Called from delete_session.
    async fn kill_all_for_session(&self, session_id: &str) -> Result<(), BackgroundShellError>;

    /// Drain pending completion notifications for a chat session.
    /// Called at the start of each agent loop turn.
    async fn drain_notifications(
        &self,
        session_id: &str,
    ) -> Vec<BackgroundShellNotification>;

    /// Kill everything (Tauri RunEvent::Exit hook).
    async fn kill_all(&self) -> Result<(), BackgroundShellError>;
}
```

- GUI 进程内 impl:`InMemoryBackgroundShellRegistry { shells: Arc<Mutex<HashMap<(String, String), BackgroundShellMeta>>>, notifications: Arc<Mutex<HashMap<String, VecDeque<BackgroundShellNotification>>>> }`。
- daemon 化后 impl:替换为 Unix socket 转发,所有调用点不变。

## Acceptance Criteria

- [ ] LLM 调 `run_background_shell({"command": "sleep 5"})` 立即返回 `shell_session_id`(毫秒级),后台进程真实在跑
- [ ] 后台 shell 完成(exit / kill / 超时)后,**下一次** agent loop turn 开头 LLM 收到 `[system] 后台 shell bsh_xxx 已完成,exit code 0...` 通知
- [ ] LLM 调 `shell_status` 拿到 `{status: completed, exit_code: 0, stdout_preview: "..."}`;>30KB 时返回 `full_output_path`
- [ ] LLM 调 `shell_kill` 后,后台进程在 1s 内退出,通知含 `exit_code: -1` + 标记 `killed`
- [ ] session delete 时,该 session 的所有 running shell 在 1s 内被 kill 进程组(`kill_all_for_session`)
- [ ] Tauri app 退出时,所有 running shell 被 kill(`kill_all`)
- [ ] `run_background_shell` 在 Edit/Plan mode 下触发 `permission:ask` modal;Yolo 模式静默 Allow
- [ ] session A 调 `shell_status` 查 session B 的 shell 返回 `is_error: true`(session-scoped)
- [ ] 后台 shell 在 `max_runtime_ms` 后被自动 kill + 注入"超时完成"通知
- [ ] `pending_notifications` 溢出 100 条时丢最早 + warn(不 panic)
- [ ] 集成测试:MockProvider 模拟"启动后台 → 等待完成 → LLM 看到通知 → LLM 主动调 shell_status"完整链路
- [ ] `cargo test --lib` 全绿,`cargo check` 0 warning
- [ ] 现有 `shell` tool 行为不变(回归)

## Definition of Done

- 单元 + 集成测试(MockProvider 验证多轮通知消费)
- `cargo check` 0 warning,`cargo test --lib` 全绿
- `docs/ARCHITECTURE.md` §X 补"后台 shell + 完成通知"小节
- `docs/HACKING-wsl.md` 补一条相关坑(如有)
- `ROADMAP.md` L1 移到 §1.2 已实施(若完成)
- DEBT.md 新债登记(若有遗留)

## Out of Scope(explicit,L1b / 后续)

- PTY 模式 / `pty_write` 交互式 dev server / ANSI 颜色 / 进度条刷新 — **L1b**,L1a ship 后再立项
- 多 tool 并发后台(L1a 一次只能 background 一个 shell,语义清晰)
- 并行 subagent + worktree — L3(独立任务)
- daemon 化本身(若 Q1 选 A,L1a 走 GUI 内常驻中间态,daemon 化是后续独立 PR)
- 后台 shell 的实时 stdout 流式推送(L1a 只在完成时一次性回填,流式留 L1b)
- 用户手动"后台 shell 列表" UI(L1a 只通过 LLM 交互,UI 后续)

## Technical Notes(待补)

- 调研出处:`docs/spikes/2026-06-19-async-parallel-tool-research.md` §2.1 + §5.1(已读,无需重写)
- 进程组 kill 复用:`tools/shell.rs:162` `kill_and_collect` + `tools/shell.rs:379` `process_group(0)`
- 注入点候选:`agent/chat_loop.rs:631` `provider.send` 之前 / `:1360` `build_synthetic_tool_result_message`
- AppState 常驻态:`app/src-tauri/src/state.rs`
- 相关 DEBT:E-001(env 窃密 closed)/ E-002(进程组 kill closed)/ A-004(cancel audit skip closed)
- `ChatWindow.vue` / `MessageList.vue` 已支持流式 tool result,L1a 完成通知大概率无需改前端(取决于 Q3 选择)