# RULE-SHELL-001 研究笔记:background shell 泄漏面与清扫方案调研

> 2026-08-27,主会话直接调研(代码事实均经现场核对)。

## 1. 泄漏面精确画像

### 1.1 `Inner.shells: HashMap<(String, String), ShellEntry>`(`in_memory.rs:80`)

- 条目在 `start()` 插入,**任何路径都不删除**。两处 TODO 注释自认:
  - `in_memory.rs:76`("A separate sweeper (TODO: PR3 or follow-up) can prune entries older than N minutes")
  - `in_memory.rs:353`(`kill_all_for_session` 尾注 "the entry just sits in the map until pruned (TODO: PR3 lifecycle)")
- **内存大头是 `ShellState::Done` 携带的 `stdout: Vec<u8>` / `stderr: Vec<u8>`**(`in_memory.rs:124-129`):
  - `run_background_task` 用 `read_to_end` 把完整输出读进内存;超过 30 KiB(`DISK_SPILL_THRESHOLD`)会落盘到 `<cwd>/.everlasting/outputs/<uuid>.txt`,**但内存副本保留在条目里**(仅供 status preview 构建)。
  - 即一次大输出构建(比如 50 MiB 日志)→ 50 MiB 常驻到进程退出。
  - `command: String` + `cwd: PathBuf` 每条目另有小额占用。
- `Running` 条目本身不持缓冲(spawn 后 stdout/stderr 由 task 持有),泄漏主体是 Done 条目。

### 1.2 `Inner.notifications: HashMap<String, VecDeque<BackgroundShellNotification>>`

- 每 session cap 100(`MAX_NOTIFICATIONS_PER_SESSION`,`push_notification_bounded` 溢出丢最旧)。
- `drain_notifications` 用 `remove(session_id)` 整队列移除 → 常规路径不泄漏。
- 残留:session 再无下一轮 turn → 队列条目(≤100 条小结构,约几十 KB/session)留在 map。**体积可忽略,本任务不处理。**

### 1.3 daemon 化为什么把它变成真实问题

- 生产构造点 `state.rs:467`(`AppState::load` → `default_registry()`),registry 随 AppState 活整个进程生命周期。
- GUI 进程(Tauri Thin/Full)退出即释放;**daemon(`everlasting-daemon`)长驻数周**,Done 条目只增不减。
- `delete_session`(`sessions.rs:334-356`)调 `kill_all_for_session` → 全部转 Done → 照样永驻。这是"删了 session 债还在"的主路径。

## 2. 关键既有语义(方案可依赖)

- **`status()` 文档已预留清扫语义**(`mod.rs:265-267`):"Returns `NotFound` if the shell doesn't exist **or was already cleaned up**"。移除条目后 LLM 得 NotFound 是设计内行为,不是新破坏。
- `run_background_task` 写回 Done 用 `if let Some(entry) = g.shells.get_mut(...)`(`in_memory.rs:607`),`started_at_lookup` 也有 vanish 兜底(`:632-642`)→ **条目被并发移除不 panic,天然容忍 sweep 竞态**。
- `kill()` 对 Done 条目幂等返回 Ok(`in_memory.rs:313-316`);条目被 sweep 后 `kill` 返回 NotFound(等价"unknown shell",现状已有该路径)。
- `kill_all`(`RunEvent::Exit`/lib.rs:497)只 `take()` kill_tx,不依赖条目存在性。

## 3. 触发机制两案对比

### 案 A:interval 定时清扫(daemon bin spawn)✅ 选定

- 先例:`server::spawn_backup_task`(`daemon/server.rs:71`,2026-08-24 RULE-DB-001)——detached `tokio::spawn` + `tokio::time::interval`,bin main 在 `load_daemon_state` 后调用(`everlasting-daemon.rs:158`)。
- spec 已收编该模式:`.trellis/spec/backend/daemon-server.md` §"Pattern: daemon 运维伴生物"("不要把 spawn 挪进 bin 内联——lib 的模块私有,沿用 wrapper 先例")。
- **架构约束(backup task 注释明文)**:"GUI Full mode deliberately stays backup-free — **no timer tasks in the GUI main process**"。sweeper 同样只在 daemon bin spawn;GUI 进程生命周期短 + 退出 kill_all,泄漏可忽略。
- 覆盖最完整:即使 daemon 全程无新 shell start(零 event 触发),超龄条目也会被清。

### 案 B:event-triggered(每次 `start()` 时顺带 sweep)

- 先例:`agent/memory_hygiene.rs` "Why event-triggered (not interval)"(每 10 次 insert 触发 + 启动一次)。
- 优点:全模式生效(含 GUI Full / 测试),零生命周期管道。
- 缺点:无法覆盖"曾启动 N 个长时 shell、此后 daemon 全程零新 start"的尾部场景(条目固定滞留到进程退出);且该注释成文于 2026-06-29,彼时"项目无长驻 interval task"的前提已被 backup task / tunnel ticker 打破。
- **结论**:A 为正解,daemon 是问题主体;B 的全模式覆盖优势对 GUI 短生命周期进程无实际价值。

## 4. 设计要点(写入 design.md 的素材)

1. **sweep 范围**:仅 `ShellState::Done` 且 `now_ms() - notification.completed_at > retention` 的条目。**绝不动 Running 条目**——移除会孤儿化 kill_tx(LLM 失去 kill 能力)且丢进程组追踪;Running 有 max-runtime 定时器(默认 24h)兜底最终转 Done。
2. **retention 默认 1h**(`SHELL_RETENTION_MS = 3_600_000`):
   - 通知在下一轮 turn 开始即 drain,LLM 典型在完成数秒~数分钟内查 status;1h 覆盖 99% 查询窗口。
   - 超过 1h 的迟到查询:通知本体自带 outcome + exit_code(`BackgroundShellNotification` 自包含),只丢 stdout preview;与 `status()` 的 "already cleaned up" 文档语义一致。
   - 与 `DEFAULT_MAX_RUNTIME_MS = 24h` 语义分层:运行上限 24h,结果保留 1h。
3. **sweep 间隔 5min**(`SWEEP_INTERVAL_MS = 300_000`):遍历是小 map 上的时间戳比较,成本可忽略;5min 粒度让"1h retention"实际偏差 ±5min,无感。
4. **trait 不加方法**:`BackgroundShellRegistry` 是 LLM 工具面(start/status/kill/drain);清扫是 impl 私有生命周期管理,放 `InMemoryBackgroundShellRegistry` 固有方法 + daemon 侧 spawn wrapper。
5. **通知队列、spill 文件均不动**(非目标):
   - 队列:cap 100 × 小结构,session 回来还能拿到结果通知。
   - spill 文件:`cleanup_outputs_dir` 在 delete_session 时清理(`sessions.rs:334`),生命周期挂在 session 上,与内存条目无关。
6. **测试策略**:sweep 方法接受注入 retention(`Duration`),单测用 retention=0 / 大值直接验证"清 Done / 保 Running / 保新 Done";spawn wrapper 保持 10 行薄壳不单测(同 backup task 先例);存量测试全部不回归。
7. 顺带销账:实现后删除 `in_memory.rs:76` 与 `:353` 两处 TODO 注释 + 更新 `mod.rs` 模块文档相关表述。

## 5. 相关文件清单

| 文件 | 角色 |
|---|---|
| `app/src-tauri/src/background_shell/in_memory.rs` | sweep 方法 + 常量 + 单测 + 两处 TODO 销账 |
| `app/src-tauri/src/background_shell/mod.rs` | 模块文档更新(可选 re-export) |
| `app/src-tauri/src/daemon/server.rs` | `spawn_shell_sweeper` wrapper(仿 `spawn_backup_task`) |
| `app/src-tauri/src/bin/everlasting-daemon.rs` | bin main 加一行 spawn 调用 |
| `.trellis/reviews/DEBT.md` | 闭合时删除 RULE-SHELL-001 条目 |
| `.trellis/spec/backend/daemon-server.md` | Phase 3.3 spec update 候选(sweeper 并入"运维伴生物"Pattern) |
