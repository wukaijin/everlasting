# 磁盘治理(F3,disk governor + WebKitCache + 日志轮转)

> 2026-09-03 落地,任务 `09-03-f3-disk-governance`(PRD R1/R2 用户裁定:
> 孤儿+可再生全自动,有主 outputs 按龄 30 天默认开,设置面可视化)。
> 摸底审计 + 设计依据见该任务 `research/` 与 `design.md`。

## 1. 回收策略总表(`disk/governor.rs`)

| 消费点 | 策略 | 参数(env 覆盖) | 开关 |
|---|---|---|---|
| worker worktree | 现有 7 天 mtime sweep(本体零改动,装配修复) | `EVERLASTING_CLEANUP_PERIOD_DAYS` | governor kill-switch |
| session 孤儿 worktree | DB 行不存在 → destroy(目录+`session/<id>` 分支);行在(含 Detached)不动;project 行无 → 整目录销毁 | — | governor kill-switch |
| outputs 孤儿桶 / `_no_session` | DB 行不存在 → 清空;`_no_session` 按龄(孤儿语义,**不受开关管辖**) | `EVERLASTING_OUTPUTS_AGE_DAYS`(缺省 30) | governor kill-switch |
| outputs 有主桶 | 桶龄(桶内最新文件 mtime)> 30 天 → 清空 | 同上 | `outputs_age_cleanup_enabled`(fail-open,仅字面 `"false"` 关) |
| DB 备份 | `prune_backups` 预算自适应:从新到旧保留,超 200 MiB 停,**至少 2 份最多 7 份** | `EVERLASTING_BACKUP_BUDGET_MB` | 无(总是跑) |
| daemon 日志 | 进程内 `RotatingFileWriter` write 前 >10 MiB 滚动保 3 代 | 常量(10 MiB / 3 代) | 无 |
| WebKitCache | GUI 启动时递归大小 >50 MiB → `remove_dir_all`(恰好等于不动;webview 自重建) | `EVERLASTING_WEBKIT_CACHE_MB` | 无 |

护栏:worktrees/outputs 目录名非 UUID 形态保守跳过(`is_uuid_name`,
防误删未来布局);DB 读失败 fail-keep;删除失败 warn 不 panic。

## 2. 双宿主装配(谁在跑回收)

- **daemon bin(默认模式真正宿主)**:`server::spawn_disk_governor`
  (bin 在 scheduler 旁装配)——24h interval,**首拍延迟 5 分钟**
  (避开启动 IO);协作停机 `AppState.disk_governor_cancel`(
  `shutdown_signal` 与 scheduler 同段 cancel)。每轮重读 kill-switch
  `disk_governor_enabled`(fail-open)。
- **GUI Full 逃生模式**:`lib.rs` 启动一次性 pass
  (`run_governor_pass_once`,异步 spawn 不等)——**有意的** Full 分支
  装配(Thin return 之后):Thin 场景由 daemon bin 节拍兜底。「GUI 主
  进程零 timer task」硬约束保持(只一次性,无周期 timer)。
- **WebKitCache 是唯一例外**:装配在 `lib.rs` setup **公共区**(mode
  resolve 之后、Thin 分支 `return Ok(())` 之前)——WebKitCache 正是
  默认 Thin GUI 的 webview 产物,装 Full 分支 = 默认模式永不清理。
  **⚠ Thin 早退陷阱**:照 governor pass 的 Full 分支模式抄 WebKitCache
  装配是本任务评审抓出的最高风险缺陷;由
  `webkit_cache::tests::startup_clean_is_wired_in_the_thin_full_common_area`
  源码静态断言守护(沿 `transport/http.routes-sync.test.ts`「解析源码
  守卫装配」先例,函数级测试抓不到装配缺失)。

## 3. kill-switch 与手动入口(AC9 语义)

- `disk_governor_enabled`(app_config KV)只管**自动**面:daemon 节拍
  与 GUI Full 启动 pass;false 时空转。
- 设置面「立即清理」(`run_disk_cleanup` IPC)**不受 kill-switch 管辖**
  ——手动语义;直调 `run_governor_pass_inner`,不查开关。
- `outputs_age_cleanup_enabled` 细粒度只管有主 outputs 按龄面。

## 4. IPC 双注册(`commands/disk.rs` + `daemon/routes/disk.rs`)

- `get_disk_usage`:7 消费点条目(db 三文件/backups/outputs/worktrees/
  attachments/logs/WebKitCache)+ `totalBytes`,camelCase;
  `spawn_blocking` + `inspect_dir` 递归 walk(不跟随 symlink,不可读
  计 0)。与回收摘要同口径单源。
- `run_disk_cleanup`:返回 `DiskGovernorOutcome`(逐项 CleanupResult)
  ;**不含 WebKitCache**(GUI 启动时机专属)。
- 五处接线铁律照 background_shells 先例(`lib.rs` invoke_handler /
  `daemon/routes/mod.rs` nest / `transport/http.ts` CMD_TO_DOMAIN),
  漏加 http.ts 映射会被 `http.routes-sync.test.ts` 拦下。

## 5. 日志轮转与 daemon.sh 共存(P0-b)

- 落盘方 = daemon bin 自带文件 layer(`disk/log_rotation.rs`,
  零依赖手写,不引 tracing-appender——沿 glob.rs「不为此拉新 crate」
  先例);终端 layer 显式 stdout(tracing-subscriber fmt **默认 writer
  是 stdout 非 stderr**,0.3 源码 `fmt/mod.rs:8`,曾被误记)。
- `daemon.sh` bg 输出 `>/dev/null 2>&1`(双写者行交错+轮转打架);
  `rotate_log` 退役;`logs` 子命令不变。契约细节见
  [daemon-server RULE-DAEMON-001](daemon-server.md) 日志段。
- stat 节流:fd 打开时 stat 一次校准,之后自计数写入字节(稳态零
  stat);writer 内禁用 tracing 宏(防同订阅器递归),降级走 `eprintln!`。

## 6. outputs 回收对 C6 恢复链路的降级(PRD R2 用户裁定)

C6 大输出三恢复模式中「落盘恢复」依赖 `outputs/<session>/` 的 spill
文件;有主桶按龄回收后,旧消息的恢复降级为**已删**(spill miss 走
既有容错标记)。这是裁定可接受的代价——spill 是恢复性副本,不是
不可再生数据。消费方改动为零(降级路径 C6 落地时已存在)。

## 7. Out of scope 留档

DB VACUUM / WAL 治理、进程/内存限损、F1 反压联动(磁盘满 → agent
loop 反压)、attachments 年龄回收(已有 session 生命周期)、浏览器端
缓存、数值参数设置面编辑(常量+env 覆盖)。
