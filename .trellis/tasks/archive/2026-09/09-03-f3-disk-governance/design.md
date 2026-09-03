# F3 磁盘治理 — 技术设计

> 依据:prd.md(R1 范围 / R2 删除边界)+ research/disk-usage-audit.md(缺口审计)+ research/integration-points.md(接线先例)。

## 1. 架构总览

新增 `disk` 模块(后端 `app/src-tauri/src/disk/`),核心 = **回收函数族(`_inner` 化)+ 每日节拍 + 立即清理按钮双入口共享**:

```
                       ┌────────────────────────────────────┐
                       │  disk/governor.rs 回收函数族 _inner │
                       │  · sweep_worker_worktrees(已有迁入)│
                       │  · sweep_orphan_session_worktrees  │
                       │  · sweep_outputs(孤儿 + 按龄)      │
                       │  · prune_backups(预算自适应原地改)│
                       └──────┬───────────────┬─────────────┘
                              │               │
              spawn_disk_governor        run_disk_cleanup IPC
              (daemon bin 每日节拍,      (设置面「立即清理」按钮,
               首拍延迟)                  daemon/Full 双宿主)
```

- **节拍宿主**:daemon bin(`bin/everlasting-daemon.rs` spawn_backup_task 旁装配一行)。照 `spawn_task_scheduler` 形态(CancellationToken 挂 AppState + shutdown_signal cancel;见 integration-points §2),周期 24h,**首拍延迟 5 分钟**(`interval_at(now + 5min, 24h)`)避开启动 IO(migrate/backup/orphan-guard)。
- **kill-switch**:`disk_governor_enabled`(app_config KV,默认 true,fail-open;每轮节拍重读,照 `scheduler_tick_with_fire` 先例——scheduler/mod.rs:236-247:get_config_value → `!= "false"`,仅字面 `"false"` 关;键常量声明在 :69)。false 时节拍空转,但「立即清理」按钮仍可用(手动语义)。
- **GUI Full 逃生通道**:lib.rs:231-235 现有一次性 `sweep_stale_workers` spawn 升级为一次性完整 governor pass(调同一 `_inner` 族),保持「GUI 零 timer」约束(不挂周期 timer)。
- **审计**:系统行为不进 AuditKind(非 agent 行为),每轮回收结果 `info!`/`warn!` 进 daemon 日志(轮转后天然受限损)。

## 2. 回收函数族设计

### 2.1 sweep_worker_worktrees(迁入,修 P0-a 断链)

现有 `git/worktree/sweep.rs:125` 的 `sweep_stale_worker_worktrees` 原地不动,governor 节拍调用它即完成「daemon 也跑」——**断链修复 = 装配问题,不改 sweep 本体**。7 天 mtime + env 覆盖策略保持。

### 2.2 sweep_orphan_session_worktrees(新,P1-a)

遍历 `<data_dir>/worktrees/<project_uuid>/<session_id>`(排除 `worker/` 子目录,`naming.rs:53-59` 已区分):

- DB session 行存在(Active 或 Detached)→ 不动(Detached 有意保留,commands/worktree.rs:238)
- DB session 行不存在 → 孤儿:destroy worktree(复用 `git::destroy`,lifecycle.rs:183 链:删目录 + prune 元数据 + 删 `session/<id>` 分支)
- `<project_uuid>` 目录在但 project 行不存在 → 整目录销毁(其下 session 必为孤儿)
- locked / best-effort 容错照 worker sweep 先例(sweep.rs:205-224)

**分支删除说明**:孤儿场景 DB 行已没了,`session/<id>` 分支无人引用,随 destroy 删除是数据一致性收尾,不是激进策略。

### 2.3 sweep_outputs(新,P1-b)

`<data_dir>/outputs/` 下按 session 目录分桶:

- **孤儿**(目录名 = session_id,DB 行不存在)→ `remove_dir_all`(复用 `sweep_session_outputs`,tool_output.rs:175-191)
- **有主按龄**(R2):目录 mtime > 30 天(常量 `OUTPUTS_AGE_DAYS = 30`,env `EVERLASTING_OUTPUTS_AGE_DAYS` 覆盖)且 `outputs_age_cleanup_enabled`(默认 true)→ 同样清空。目录 mtime 取桶内最新文件 mtime(spill 追加会刷新)
- `_no_session`(tool_output.rs:59)→ 按 `OUTPUTS_AGE_DAYS` 同龄回收
- **C6 恢复链路影响**:被回收 session 的旧 tool_result「恢复模式」降级为已删(spill 文件缺失时消费方已有容错——C6 落地时 spill miss 走标记降级);这是 R2 用户裁定的可接受代价,spec 更新时写明

### 2.4 prune_backups_adaptive(改,P2-a)

`db/backup.rs` 现有 `KEEP_BACKUPS=7` 固定份数(:38)改为**大小预算**:

- 预算常量 `BACKUP_BUDGET = 200MiB`(env `EVERLASTING_BACKUP_BUDGET_MB` 覆盖)
- 规则:从新到旧保留,超出预算即停;**至少保留 2 份**(哪怕超预算),最多仍 7 份
- 本机现状 26M×7=182M ≤ 200M → 行为不变;DB 涨大后份数自适应下降
- 实现改 `prune_backups`(backup.rs:94-113)内:列目录 + 累计大小 + 保留判定,单函数改动

## 3. 日志:进程内文件 sink + 尺寸轮转(P0-b)

**选型:零依赖手写,不引 tracing-appender**(仓库「不为此拉新 crate」先例 glob.rs:205-207;轮转逻辑 30 行内,新依赖不成比例)。

- `disk/log_rotation.rs`:`RotatingFileWriter` 实现 `io::Write` + `MakeWriter`:
  - 路径 `$XDG_STATE_HOME|~/.local/state}/dev.everlasting.app/daemon.log`(照 daemon.sh:54-55 语义,失败降级 stderr-only 不 panic)
  - 每次 write 前检查 `metadata().len() > 10MiB` → 滚动 `mv log→.1→.2→.3`(覆盖旧代),重开 fd;参数常量 `LOG_MAX_BYTES = 10MiB` / `LOG_KEEP = 3` 与 daemon.sh 现行完全一致
  - 滚动检查做节流(如每 N 次 write 或记录上次检查长度)避免每条日志一次 stat
- `bin/everlasting-daemon.rs:53-58`:`fmt()` 改多 layer(终端流保留 + `with_writer(RotatingFileWriter)`)。**基线澄清(外部评审 2026-09-03 指正)**:现状两处 init 均未配 `.with_writer()`,走 tracing-subscriber 0.3.23 fmt **默认 writer = stdout**(`fmt/mod.rs:8`)——非 stderr;daemon.sh `2>&1` 与 sidecar 双管道均两流全抓,故 stdout/stderr 对落盘/抽回行为等价,实现时终端 layer 显式 `.with_writer(std::io::stdout)` 锁定基线即可。**GUI main.rs 不动**(GUI 进程保持默认终端输出)
- **daemon.sh 调整**(scripts/daemon.sh):
  - bg 模式 `>> "$LOG_FILE" 2>&1` → `>/dev/null 2>&1`(消除双写者;integration-points §3)
  - `rotate_log`(:73-87)退役删除(Rust mv 滚动会自然覆盖消化旧 .1/.2/.3)
  - `logs` 子命令 `tail -f`(:227-231)路径不变
- **sidecar 模式新收益**:daemon bin 自带文件 layer,打包 GUI 的 daemon 日志首次落盘(sidecar 抽回管线 sidecar.rs:191-223 与文件 layer 并行,零改动)
- **spec 同步**:RULE-DAEMON-001 定义在 `.trellis/spec/backend/daemon-server.md:255`(及 :284 引用),更新「日志落盘方从脚本重定向改为进程内 appender」

## 4. WebKitCache 治理(P2-b)

- **时机:GUI 启动早期异步清,带阈值**——检查 `app_data_dir/WebKitCache` 递归大小,**> 50MiB**(常量 `WEBKIT_CACHE_THRESHOLD`)才 `remove_dir_all`(webview 会重建;Linux webkitgtk 持 fd 场景下 unlink 语义安全)
- **装配位置(⚠ Thin 早退陷阱,外部评审 2026-09-03 指出)**:lib.rs `.setup(|app|)`(:152)内,**mode resolve(:161)之后、Thin 分支 `return Ok(())`(:183)之前的公共区**,异步 spawn。现有 sweep(:234)/hygiene(:248)装配点全在 Thin return 之后的 Full 分支内——**勿照搬该模式**:WebKitCache 正是默认 Thin GUI 的 webview 产物(136M 大头),照 Full 分支装配 = 默认模式永不清理,且函数级测试抓不到装配缺失。需装配级验证(如 mode=Thin 路径调用清理函数的测试,或 PR review checklist 显式核对装配点)
- Thin / Full 两条 GUI 模式都过(目录归 GUI webview 管,与 daemon 无关)
- **浏览器 remote 模式的缓存归浏览器,不归本任务管**(边界写死)
- 为什么不放节拍:目录归 GUI webview 进程活跃使用,daemon 跨进程删时机不可控;启动时 webview 尚未大量使用,是最安全窗口

## 5. IPC:两个新命令(照 background_shells 五处接线)

新域 `commands/disk.rs` + `daemon/routes/disk.rs`(integration-points §4 全清单):

- `get_disk_usage` → `Vec<DiskUsageEntry{ key, label, bytes }>` + 总量。条目:db(主库+wal+shm)、backups、outputs、worktrees、attachments、logs(state 目录)、webkit_cache。实现:`spawn_blocking` + 手写递归 walk(照 glob.rs:205-227 扩累计,不跟随 symlink,不可读跳过)
- `run_disk_cleanup` → `Vec<CleanupResult{ key, reclaimed_bytes, detail? }>`。调 governor 同一 `_inner` 族(不含 WebKitCache——那是 GUI 启动时机;含孤儿+按龄 outputs+worker sweep+孤儿 session worktree+备份 prune)
- wire:snake_case 扁平标量 body 铁律(daemon/routes);返回 `rename_all="camelCase"` 顶层扁平数组
- 五处接线:`commands/mod.rs` pub mod / `lib.rs` invoke_handler / `daemon/routes/disk.rs` router / `daemon/routes/mod.rs` nest / `transport/http.ts` CMD_TO_DOMAIN(routes-sync.test.ts 守卫)

## 6. 配置(app_config KV,additive)

| key(AppConfigPayload camelCase 字段) | 类型 | 默认 | 语义 |
|---|---|---|---|
| `diskGovernorEnabled` | bool | true | 每日节拍 kill-switch(fail-open);false 时机停但手动清理可用 |
| `outputsAgeCleanupEnabled` | bool | true | 有主 outputs 30 天按龄回收开关(R2 细粒度) |

- 走既有 `set_app_config_flag` 白名单通道零后端新命令;`AppConfigPayload` additive 加两字段 + `get_app_config_inner` 读
- **数值参数不上设置面**:OUTPUTS_AGE_DAYS(30)/ BACKUP_BUDGET(200MiB)/ LOG_MAX(10MiB×4)/ WEBKIT_CACHE_THRESHOLD(50MiB)全部常量 + env 覆盖(项目先例:cleanup_period_days、KEEP_BACKUPS)。理由:数值写通道是新基建(白名单命令 + 类型解析),为低频参数不值;设置面只有开关 + 概览 + 按钮(Key Decision,final summary 呈现)

## 7. 前端:设置面 DiskTab

- `settings/DiskTab.vue`(global scope):三段式
  1. **占用概览**:`get_disk_usage` 渲染条目列表(标签 + 人类可读大小 + 总量行);进入 tab 或手动刷新按钮触发
  2. **开关**:照 GeneralTab `FlagRow[]` 模式两行(diskGovernorEnabled / outputsAgeCleanupEnabled),写走 configStore setter(写成功才更新本地 ref 先例)
  3. **立即清理**:pending 态按钮 → `run_disk_cleanup` → toast 展示逐项回收摘要(字节数);**resolve 成功后自动重新 `get_disk_usage` 刷新概览**(AC7「数字同步下降」的实现闭环,外部评审 2026-09-03 补)
- registry.ts 新分类 `disk`(新 SettingsGroup「存储」或挂「全局」组;倾向新组「存储」,同步 SETTINGS_GROUP_ORDER + registry.test.ts)
- SettingsModal.vue CATEGORY_COMPONENTS 加映射
- vitest:DiskTab 渲染/开关回拨/清理 toast;registry 快照

## 8. 兼容与迁移

- 无 DB schema 变更(app_config 是 KV,additive)
- daemon.sh 变更向后兼容:bg 模式启动命令行为不变,仅输出去向变了
- 首次运行:Rust 轮转接管的 daemon.log 若存在旧代 .1/.2/.3,滚动循环 2-3 轮内自然覆盖消化,无需迁移
- rollback:整体 revert 即可,无破坏性状态(被回收数据本就是裁定可删的)

## 9. 权衡记录(Why not)

- **不用 tracing-appender**:新依赖 vs 30 行手写;仓库先例倾向后者
- **不用 scheduler 30s tick 挂回收**:磁盘回收是日级语义,30s tick 里做年龄判断是浪费;独立 24h interval 与 backup 对称
- **WebKitCache 不进 governor 节拍**:跨进程删活跃 webview 缓存时机不可控;启动窗口最安全
- **数值参数不进设置面**:数值写通道新基建 vs 低频参数;常量+env 先例充分
- **有主 outputs 回收不做逐文件细粒度**:目录 mtime 分桶足够(spill 追加刷新 mtime),逐文件 walk 的复杂度不值

## 10. 风险

- 孤儿 session worktree 判定依赖 DB 行存在性查询:destroy 不可逆。缓解:仅「目录在且 DB 行确实不存在」双条件 + locked 跳过 + info! 日志留痕;测试覆盖假孤儿(行在)不删
- daemon.sh 变更影响用户习惯:bg 模式日志去向不变(同一路径同一命令),`logs` 子命令不动,感知面小
- run_disk_cleanup 在 Tauri Full 模式落 GUI 进程执行(与 scheduler 同款行为差异):注释写明,不影响 Thin/remote 主路径
