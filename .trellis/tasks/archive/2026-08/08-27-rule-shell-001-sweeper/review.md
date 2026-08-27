# Review — RULE-SHELL-001 background shell 条目清扫 sweeper

> 评审对象:PRD / design / research / implement(任务处于 **planning** 阶段,仓库中**尚无任何本任务代码**,`git status` 仅有该任务目录;`sweep_completed_shells` / `spawn_shell_sweeper` / `SHELL_RETENTION_MS` 全库无实现痕迹)。
> 评审方式:逐条核对 PRD/design/research 引用的代码事实(行号/结构/先例/语义),验证设计取舍在现有代码上的可落地性。
> 结论:**文档取证质量高,主体设计成立,可直接进入实现**。发现 4 个实现期留意点(P2×1、P3×3),均不阻塞开工。

---

## 1. 代码取证核实结果(现场核对,与文档一致性)

| 文档主张 | 实际代码 | 判定 |
|---|---|---|
| `Inner.shells: HashMap<(String,String), ShellEntry>` 任何路径不删除,`:76` 有 sweeper TODO 注释(`in_memory.rs:80`) | `in_memory.rs:80` `shells: HashMap<(String, String), ShellEntry>`;`:74-79` 字段 doc 明文 "Entries are NOT removed on completion... A separate sweeper (TODO: PR3 or follow-up) can prune entries older than N minutes" | ✅ 一致 |
| `:353` `kill_all_for_session` 尾注 TODO(`in_memory.rs:353`) | `kill_all_for_session` 定义于 `:329`,`:353` 注释 "sits in the map until pruned (TODO: PR3 lifecycle)" | ✅ 一致 |
| Done 条目携带完整 `stdout/stderr` 缓冲 + 超 30KiB 落盘但内存副本保留(`DISK_SPILL_THRESHOLD`) | `in_memory.rs:53-54` `DISK_SPILL_THRESHOLD = 30 * 1024`;`:587` `if stdout.len()+stderr.len() > DISK_SPILL_THRESHOLD` 落盘;`ShellState::Done { stdout: Vec<u8>, stderr: Vec<u8>, full_output_path }`(`:124-129`) | ✅ 一致(内存大头论断成立) |
| 通知队列 cap 100、`drain_notifications` 用 `remove` 整队列移除 | `MAX_NOTIFICATIONS_PER_SESSION = 100`(`:39`);`push_notification_bounded` 溢出丢最旧(`:509-513`) | ✅ 一致 |
| `status()` 文档已预留清扫语义("or was already cleaned up") | `mod.rs:265-267` "Returns NotFound if the shell doesn't exist or was already cleaned up" | ✅ 一致(清扫后 NotFound 是既有契约,非行为破坏) |
| `run_background_task` 写回用 `if let Some(entry)`,条目消失不 panic;`started_at_lookup` 有 vanish 兜底 | `:607` `if let Some(entry) = g.shells.get_mut(...)`;`started_at_lookup`(`:632-642`)`map(...).unwrap_or_else(now_ms)` | ✅ 一致(sweep 竞态天然容忍) |
| `kill()` 对 Done 幂等返回 Ok,条目被 sweep 后返回 NotFound | `:313-316` Done 臂 `Ok(())`,注释 "Idempotent: killing a Done entry is a no-op" | ✅ 一致 |
| `AppState::load → default_registry()` 是生产构造点(`state.rs:467`),registry 随进程活全程 | `state.rs:467` `background_shells: crate::background_shell::default_registry()`;`:205` 字段类型 `DefaultRegistry` | ✅ 一致 |
| `delete_session` 调 `kill_all_for_session` → 全转 Done 照样永驻("删了 session 债还在"主路径) | `commands/sessions.rs:334-356`:`cleanup_outputs_dir` → `kill_all_for_session`(fire-and-forget) | ✅ 一致 |
| `spawn_backup_task` 先例:detached `tokio::spawn` + interval,bin main 在 load_daemon_state 后调用(`daemon/server.rs:71` / `everlasting-daemon.rs:158`) | `daemon/server.rs:71` `pub fn spawn_backup_task(state: &AppState, data_dir: &Path)`,注释含 "**no timer tasks in the GUI main process**"(`:69`);`bin/everlasting-daemon.rs:158` 在 `load_daemon_state` 后调用 | ✅ 一致(装配模式与架构约束双确认) |
| "不进 `default_registry()`(几十处测试构造点会被动 spawn 常驻任务)" | 全库 `default_registry()` 调用点 ~30 处(不含定义),遍布 tools 测试构造 | ✅ 一致("几十处"准确) |
| 常量 `DEFAULT_MAX_RUNTIME_MS = 86_400_000` / `MAX_NOTIFICATIONS_PER_SESSION = 100` 与锚定测试先例 `notification_queue_cap_is_100` | `:43` / `:39`;锚定测试 `:763` `assert_eq!(DEFAULT_MAX_RUNTIME_MS, 86_400_000)`、`:769` `notification_queue_cap_is_100` | ✅ 一致(新常量可仿同风格锚定) |
| `BackgroundShellNotification` 自包含 outcome + exit_code(1h 后迟到查询仍可得结果) | `mod.rs:85-100`: `shell_session_id / session_id / outcome / exit_code / started_at / completed_at(MonotonicMs)` | ✅ 一致(丢 preview 不丢结果) |
| interval 首 tick 立即触发(启动即清一轮) | `spawn_backup_task` 注释 "A fresh interval's FIRST tick completes immediately → the loop body runs once right away"(`daemon/server.rs:81-84`) | ✅ 一致 |
| 新常量 `pub(crate)` 落 `in_memory.rs` 后 daemon wrapper 可读 | `mod.rs:47` `pub mod in_memory;`(非 private)→ 同 crate 的 `daemon/server.rs` 可直接引用 `in_memory::SHELL_RETENTION_MS` | ✅ 一致(无需 re-export,见 §2) |

**未核实到/需实现期确认的引用**:

- research §5 把 `.trellis/spec/backend/daemon-server.md` 列为 "Phase 3.3 spec update 候选"(sweeper 并入"运维伴生物"Pattern),但 PRD R1-R6 / AC 均未提 spec 更新——要么实现后走一次 spec 收编(与 backup task 同页),要么把该行从 research 移除,避免"文档承诺了 PRD 没排期的事"。
- PRD AC3 说 wrapper "落在 `daemon/server.rs`",research §5 同列该文件——一致,无出入。

---

## 2. 设计评估

### 方案正确性 — 成立

- **只扫 Done 是必要约束**:Running 条目携带 `kill_tx`,移除 = LLM 失去 kill 能力 + 丢进程组追踪;max-runtime 定时器(24h)兜底终态化。研究对"移除 Running 的后果"描述准确(`:313-316` Done 臂幂等 + Running 臂 `take()` 佐证)。
- **竞态安全论证完整**:sweep 持 `inner.lock()` 遍历与写回互斥,无并发窗口;写回侧 `if let Some` 容忍条目消失——研究引用的三处兜底(`:607` / `:632-642` / `kill_all` 只 take kill_tx)全部现场属实。
- **interval vs event-triggered 选型正确**:daemon 是问题主体、GUI 短生命周期进程泄漏可忽略;interval 覆盖"零新 start 的尾部滞留"场景,案 A 是唯一正解。引用 `memory_hygiene.rs` 的 2026-06-29 前提已被 backup task / tunnel ticker 打破——判断合理。
- **D2 装配点排除正确**:不进 `AppState::load`(GUI 共用构造点)、不进 `default_registry()`(~30 处测试构造点)——两处排除都符合 "no timer tasks in the GUI main process" 架构约束,且避免测试运行时被常驻任务污染。
- **常量可见性无障碍**:`pub mod in_memory` + `pub(crate)` 常量,daemon wrapper 直接引用,无需 re-export(design D3 隐含该假设,已核实成立)。

### 可留意的实现期定夺点(见 §3)

D1 签名写 `Duration`、D3 又说"实现时取 ms u64 更贴现状"——二选一需在实现期定死;测试 3 的 `sleep 30` 会拖慢测试集,值得优化。

---

## 3. 实现期必须处理的问题

### P2 — 测试 3 `sweep_keeps_running_entries` 用 `sleep 30` 会拖慢测试集(design D5)

sweep 谓词只匹配 `ShellState::Done`,Running 条目根本不进入判定分支——**不需要等 30 秒来证明"Running 保留"**。项目测试规范明确反对单测磨时间(`cargo test` 多线程,一个 30s 测试成最慢项,边际 ~3× 慢测试集)。建议:start 后短 sleep(1-2s,越过 spawn 窗口)或干脆立即 sweep(retention=0)断言返回 0 + status 仍 Running;任务末尾照常 kill 收尾。若担心 spawn 窗口期状态未定,可用既有 `status()` 轮询到 Running 再 sweep,全程 <2s。

### P3 — swept shell 的通知仍可能在下轮 turn drain 到(半截窗口)

sweep 只清 `shells` 条目,不清 `Inner.notifications`(Non-Goals 明示不动)。因此存在一个窄窗口:shell 已被 sweep(LLM 此时 `shell_status` 得 NotFound),但其完成通知仍坐在 session 队列里,下一轮 turn 会被 `drain_notifications` 交给 LLM。语义上可解释(通知自包含 outcome/exit_code,与 R3 一致),但实现时建议确认 loop drain 通知的消费方式,避免 LLM 先收到"shell 已完成"的通知、随后 status 查不到而困惑。若担心,可在 sweep 时顺带把该 shell 的通知从队列移除(一行,仍在锁内)——但注意这超出 Non-Goals 声明,需在 PRD 里写明才做,否则维持现状即可。

### P3 — D1/D3 对 sweep 方法签名的说法不一致(实现期定夺)

D1 契约写 `sweep_completed_shells(retention: Duration) -> usize`,D3 又写"方法签名收 Duration(或 ms u64...实现时取后者更贴现状)"。建议实现期取 **ms u64**,与模块 `MonotonicMs` 风格一致(`now_ms()` 返回 u64,`completed_at` 是 u64),避免 Duration↔ms 转换噪声;wrapper 从 `SHELL_RETENTION_MS` 常量读值传入。测试锚定断言也顺带统一为 u64。

### P3 — DEBT.md 销账需同步更新 section 头计数(AC6)

当前 `RULE-SHELL-001` 条目位于 DEBT.md "## P2 — 健壮性 + 债务" section(`[2 items]`)。删除条目后 section 头计数需同步改为 `[1 item]`,并仿 RULE-ARGS-001 先例(8-27 header 注记)在 header 下加一行闭合说明(指向本任务 + git log)。纯文案,但漏改会让 DEBT.md 内部计数失真。

---

## 4. 总体结论

PRD/design/research 对现有代码的取证**质量高、全部属实**:泄漏面画像精确(内存大头 = Done 条目的 stdout/stderr 缓冲,spill 不释放)、既有语义依赖(`status()` 文档预留 NotFound、写回竞态兜底)扎实、装配点排除(load / default_registry)与架构约束("no timer tasks in the GUI")完全一致、interval 选型论证充分。**设计可直接进入实现**,唯一 P2 是测试 3 的 30s sleep(改成 <2s 即可),其余为实现期小定夺。AC1-AC6 验收项与 PRD 需求一一对应,无缺口。

> 评审人:Carlos · 2026-08-27
