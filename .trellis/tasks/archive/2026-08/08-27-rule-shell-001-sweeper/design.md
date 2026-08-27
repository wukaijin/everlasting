# design.md — RULE-SHELL-001 sweeper

> 依据:`research/codebase-findings.md`(现场核对过的代码事实)。本文件只记
> 决策与契约,不复述研究过程。

## D1 清扫谓词与内存回收点

```
sweep_completed_shells(retention_ms: u64) -> usize
```

> 签名定死 `u64`(ms),与模块 `MonotonicMs` / `now_ms()` / `completed_at` 全
> u64 风格一致,免 `Duration` ↔ ms 转换噪声(外部 review 指出 D1/D3 原文
> "Duration(或 u64)"两说并存,此处收敛)。wrapper 从 `SHELL_RETENTION_MS`
> 读值传入。

- 遍历 `Inner.shells`,`retain` 掉满足以下谓词的条目:
  `matches!(state, ShellState::Done{..}) && now_ms() - notification.completed_at > retention_ms`
- 返回移除数(供 wrapper `info!` 日志;返回 0 不打日志,与 backup task 的
  `Ok(_) => {}` 风格一致)。
- **只动 Done**:Running 条目携带 `kill_tx`,移除 = 孤儿化进程组(LLM 失去
  kill 能力);max-runtime 定时器(默认 24h)保证 Running 终态化,随后可扫。
- **竞态安全(既有代码已保证)**:`run_background_task` 写回用
  `if let Some(entry)`,条目消失不 panic;sweep 持锁遍历与写回互斥,无窗口。

## D2 触发:daemon interval spawn(仿 `spawn_backup_task`)

- `daemon/server.rs` 新增 `spawn_shell_sweeper(state: &AppState)`:
  detached `tokio::spawn` + `tokio::time::interval(5min)`,循环体调
  `state.background_shells.sweep_completed_shells(retention)`,移除数 > 0 时
  `tracing::info!`(session 维度不敏感,不打 per-shell 明细)。
- `bin/everlasting-daemon.rs` 在 `spawn_backup_task` 调用点后加一行。
- **不进 `AppState::load`**(GUI 也走该构造点,违反 "no timer tasks in the
  GUI main process");**不进 `default_registry()`**(几十处测试构造点会被动
  spawn 常驻任务,污染测试运行时)。
- 失败面:清扫是纯内存操作无 Result;interval 首 tick 立即触发一次,启动即
  清一轮(对 backup task 注释的既有语义复用,顺带覆盖"重启前遗留"——虽然
  实际上重启即空 map,此特性只是无害的统一)。

### D2.1 通知"半截窗口"——已评估,维持不动(外部 review P3)

窗口:shell 完成 → 通知在队列等下轮 turn drain → 条目被 sweep(>1h)→
session 迟归后通知照常投递,但 `shell_status` 已 NotFound。

现场核实(`agent/chat_loop/drive.rs:815`):通知文案为
`[system] 后台 shell <id> 已完成,exit code <N>。调 shell_status(...) 看输出。`
——outcome + exit_code 在通知本体里,LLM 先拿到终态结果;随后 status 得
NotFound 是"already cleaned up"既有语义,LLM 只丢 stdout preview,可自然降级。

**拒绝** review 提出的备选"sweep 时顺带删队列里该 shell 的通知":删掉通知反而
剥夺 LLM 对终态的唯一知情渠道(shell 完成都不知道),比只丢 preview 更糟;
且通知自包含正是 1h retention 正当性的来源。Non-Goals 维持原判。

## D3 参数

| 常量 | 值 | 理由 |
|---|---|---|
| `SHELL_RETENTION_MS` | 3_600_000(1h) | 通知下一轮 turn 即 drain,LLM 查询集中在完成后的秒~分钟级;超时迟到者仍可从自包含通知拿到 outcome/exit_code,仅丢 preview。与 24h 运行上限分层互不纠缠。 |
| `SWEEP_INTERVAL_MS` | 300_000(5min) | 小 map 时间戳比较,成本≈0;±5min 偏差对 1h retention 无感。 |

- 两常量 `pub(crate)` 落 `in_memory.rs`,与 `DEFAULT_MAX_RUNTIME_MS` 相邻,
  并仿 `notification_queue_cap_is_100` 加锚定单测(防无意改动)。
- retention 经 wrapper 从常量读入;方法签名收 `Duration`(或 ms u64,与模块
  `MonotonicMs` 风格一致——实现时取后者更贴现状)。

## D4 文档与销账

- `in_memory.rs:76` `shells` 字段注释:删 TODO,改述现行行为("Done entries
  are pruned by the daemon sweeper after SHELL_RETENTION_MS")。
- `in_memory.rs:353` `kill_all_for_session` 尾注:删 "(TODO: PR3 lifecycle)"
  括号,指向 sweeper。
- `mod.rs` 模块 doc "Resource bounds" 段补一句结果保留语义。
- DEBT.md 删条目(随闭合 commit)。

## D5 测试计划

1. `sweep_removes_done_beyond_retention`:`true` 完成后 sweep(retention=0)
   → `status` NotFound;返回值 = 1。
2. `sweep_keeps_recent_done`:完成后 sweep(极大 retention)→ status 仍
   Completed。
3. `sweep_keeps_running_entries`:`sleep 30` 只是"让条目停在 Running"的
   命令,**不等待 30s**——沿用既有测试模式(如
   `kill_all_for_session_only_affects_target_session` 同用 `sleep 30`,全程
   ~1-2s):start 返回即条目已在 map(Running 在 `start()` 第 4 步同步插入),
   立即 sweep(0) 断言返回 0 + status 仍 Running;末尾 kill 收尾。全程无需
   sleep 等待(外部 review 曾误读为需等 30s,此处明示防 implement 期同款
   误读)。
4. `sweep_bounds_anchored`:两个常量值锚定断言。
5. 存量全量回归:`cargo test -p everlasting --lib` + clippy(AC5)。

wrapper(D2)为 10 行薄壳,不加直接单测(backup task 同先例)。
