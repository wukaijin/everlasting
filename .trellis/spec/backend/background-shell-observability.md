# Background Shell Observability Spec

> 后台 shell 的 UI 可观测性契约:registry `list_for_session` + `background_shell:update` 事件 + emitter 双模式接线。
> 2026-09-02,task `09-02-chat-task-panel`。UI 消费端见 [frontend/chat/activity-panel.md](../frontend/chat/activity-panel.md)。

## Scenario: chat 运行状态面板(后台 shell 可观测)

### 1. Scope / Trigger

- 触发:新增跨层契约(2 条 IPC 命令 + 1 条 SSE/Tauri 双路径事件 + registry→emitter 注入)。
- 模式定位:**chat loop 之外**的异步状态(registry watcher task)要推 UI 时的唯一通路 —— 无 `ChatEventSink` 上下文,用 **AppState 级 emitter 注入**,不新造 sink trait。

### 2. Signatures

```rust
// background_shell/mod.rs
pub const EVENT_NAME: &str = "background_shell:update";
pub type ShellEventEmitter = Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>;
// trait BackgroundShellRegistry 新增:
async fn list_for_session(&self, session_id: &str) -> Vec<BackgroundShellSummary>;
// in_memory impl 新增:
pub fn set_event_emitter(&self, emitter: ShellEventEmitter); // None→Some 才生效,重复忽略+warn

// commands/background_shells.rs
list_background_shells_inner(session_id, state) -> Result<Vec<BackgroundShellSummary>>
kill_background_shell_inner(session_id, shell_session_id, state) -> Result<()>
```

### 3. Contracts

- `BackgroundShellSummary`(camelCase,TS 镜像 `app/src/stores/backgroundShells.ts`):`shellSessionId / sessionId / command / status("running"|"completed"|"failed"|"killed"|"timed_out"|"spawn_failed") / startedAtMs / elapsedMs / exitCode / stdoutPreview / stderrPreview / fullOutputPath / originToolUseId`。
- `ShellEventPayload`(camelCase,双路径同形):`{ kind: "started"|"exited"|"pruned", sessionId, shellSessionId, shell: Summary|null }`;pruned 时 `shell: None`,前端按 id 删。
- 发射点(仅三处,锁 drop 后调用):`start()` 成功→started;`run_background_task` 汇合点→exited(kill/timeout/normal 全经 kill_tx/select 汇入此点;SpawnFailed 分支在 start 内直发 exited,不 spawn watcher → 无双发);`sweep_completed_shells` 逐条→pruned。`kill_all_for_session`/`kill_all` 不另发。
- emitter 接线(装配期一次性):daemon bin → `daemon::server::wire_background_shell_events`(SSE broadcast);Tauri Full 模式 `lib.rs` setup AppState::load 后(`app_handle.emit`,失败仅 warn);Thin 模式 GUI 无 AppState,不接线。
- 注册清单(新命令五处):`lib.rs` invoke_handler、`commands/mod.rs` 名单(Thin 代理白名单)、`daemon/routes/mod.rs` nest、route 文件 `daemon/routes/background_shells.rs`、`app/src/transport/http.ts` `CMD_TO_DOMAIN`(routes-sync 守卫会抓)。

### 4. Validation & Error Matrix

| 条件 | 行为 |
|---|---|
| `list_background_shells` 对无 shell 的 session | 返回空数组(非错误) |
| `kill_background_shell` id 不存在/已 prune | `BackgroundShellError::NotFound` → AppCommandError(前端 toast) |
| `kill_background_shell` 已终态 | registry.kill 幂等 Ok,**不发事件** |
| 重复 `set_event_emitter` | 忽略第二次 + `tracing::warn!`(防测试串扰) |

### 5. Good/Base/Bad Cases

- Good:running shell 完成后,面板行在 exited 事件到达时翻终态并带 preview(实现超集:killed/timed_out 也带 preview,对面板展开更有用;LLM 面 `BackgroundShellStatus` 不变)。
- Base:空 session list → 面板不渲染 shell section。
- Bad:在持有 registry 锁时调 emitter(死锁/长尾风险)—— 必须短锁内克隆 summary、drop 后发射。

### 6. Tests Required

- `background_shell::in_memory::tests`:list shape+排序(running 优先/started_at 倒序)、started→exited 序列与 payload 字段、pruned 逐条 id、二次 set 被忽略。
- `tests/e2e.rs` 路由清单 + `http.routes-sync.test.ts` 守卫锁注册。

### 7. Wrong vs Correct

#### Wrong

```rust
// MonotonicMs 直接进前端当天数用 —— 它是进程启动起的单调 ms,不是墙钟
let ago = Date.now() - shell.startedAtMs; // ❌ 两套时钟
```

#### Correct

```ts
// store 收到 running 摘要时记 receivedAt(墙钟),显示值 = elapsedMs + (now - receivedAt)
// startedAtMs 永不参与与 Date.now() 的运算;fetch 整表替换/事件 upsert 均无条件重置 receivedAt
```

> **Warning**(2026-09-02 check 实证):receivedAt 若在 fetch 替换时保留旧值,session 切走再切回会把离开期间的时长双倍计入(elapsed 多算 60s)。规则:**任何 running 摘要入店即无条件重置 receivedAt**(SSE payload 的 elapsedMs 是发射时刻新鲜值,无重放,重置无副作用)。
