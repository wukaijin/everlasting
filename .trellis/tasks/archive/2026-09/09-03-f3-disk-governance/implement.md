# F3 磁盘治理 — 执行计划

> 四个逻辑 PR,按依赖排序。验证命令速查:后端 `cargo test -p everlasting --lib`(WSL 需 PKG_CONFIG_PATH,见 AGENTS.md);前端 `cd app && pnpm test`;e2e `pnpm test:e2e`。

## PR1 — 回收核心 + 每日节拍(后端机制面)

- [ ] 新模块 `app/src-tauri/src/disk/mod.rs` + `governor.rs`:回收函数族
  - `sweep_orphan_session_worktrees`(新;DB 行存在性判定 + git::destroy 复用 + locked 容错)
  - `sweep_outputs`(新;孤儿桶 remove_dir_all + 有主按龄 30 天 + `_no_session` 同龄;常量 `OUTPUTS_AGE_DAYS` + env)
  - 备份预算自适应:**原地改 `db/backup.rs::prune_backups` 的保留判定**(不新增包装函数;预算 200MiB / 最少 2 份 / 最多 7 份;env 覆盖)
  - 各 `_inner` 化返回回收摘要(字节/条数)供节拍日志与 IPC 双消费
- [ ] `disk/governor.rs::spawn_disk_governor`:照 spawn_task_scheduler 形态(CancellationToken 挂 AppState + shutdown_signal 步骤 cancel;`interval_at(now+5min, 24h)`);每轮重读 `disk_governor_enabled`(fail-open)
- [ ] 装配:`bin/everlasting-daemon.rs` spawn_backup_task 旁一行;`lib.rs` Full 模式把现有一次性 sweep_stale_workers(:231-235)升级为一次性完整 governor pass(保持零 timer 约束)。注:该装配点在 Thin 分支 return 之后是**有意的**——Thin 场景由 daemon bin 节拍兜底;勿将此模式照搬到「Thin 也要跑」的逻辑(见 PR4 WebKitCache)
- [ ] 配置:`commands/config.rs` AppConfigPayload additive 两字段(diskGovernorEnabled/outputsAgeCleanupEnabled)+ SETTABLE_APP_FLAGS 白名单 + `stores/config.ts` 对应 ref/setter
- [ ] 测试:孤儿 worktree(真孤儿删/假孤儿不删/locked 跳过)、outputs(孤儿/按龄/有主开关关时不删/`_no_session`)、备份 prune(预算边界/最少 2 份)、节拍 kill-switch;迁移性——现有 sweep_worker_worktrees 测试不动只加调用方
- 验证:`cargo test -p everlasting --lib` + `cargo clippy -p everlasting -- -D warnings`
- 回滚点:PR1 独立可 revert,不影响现有 sweep 行为

## PR2 — 日志进程内轮转 + daemon.sh 调整(P0-b)

- [ ] `disk/log_rotation.rs`:`RotatingFileWriter`(io::Write + MakeWriter;10MiB×3 滚动;XDG_STATE_HOME 路径解析 + 降级 stderr;stat 节流)
- [ ] `bin/everlasting-daemon.rs:53-58`:fmt 多 layer(stderr + 文件);GUI main.rs 不动
- [ ] `scripts/daemon.sh`:bg 重定向 `>> $LOG_FILE` → `>/dev/null 2>&1`;`rotate_log` 退役;`logs` 子命令不动
- [ ] 测试:writer 轮转触发(写超阈值文件滚代/最多 3 代/滚动后继续写);路径解析单测(自定义 XDG_STATE_HOME)
- 验证:cargo test + `scripts/daemon.sh` 手动 bg 启动一次确认日志仍落 daemon.log(daemon 重启后)
- 风险文件:daemon.sh(用户习惯面)——改前 diff 确认 logs 子命令零改动

## PR3 — IPC 双注册 + 设置面 DiskTab

- [ ] `commands/disk.rs`:`get_disk_usage`(spawn_blocking 手写递归 walk,照 glob.rs 先例;不跟随 symlink)+ `run_disk_cleanup`(调 PR1 `_inner` 族)
- [ ] 五处接线:commands/mod.rs / lib.rs invoke_handler / daemon/routes/disk.rs(snake_case 扁平 body 铁律)/ routes/mod.rs nest / transport/http.ts CMD_TO_DOMAIN(routes-sync.test.ts 守卫会拦漏加)
- [ ] `settings/DiskTab.vue`:占用概览(条目+总量+刷新)+ FlagRow 两开关 + 立即清理 pending 按钮 + 回收摘要 toast + **resolve 成功后自动重新 get_disk_usage 刷新概览**(AC7 数字下降的实现步骤)
- [ ] registry.ts 新分类 `disk`(新「存储」SettingsGroup + SETTINGS_GROUP_ORDER)+ SettingsModal CATEGORY_COMPONENTS + registry.test.ts
- [ ] 测试:disk commands 单测(mock 目录布局)、DiskTab vitest(渲染/开关回拨/清理 toast)、registry 快照、http routes-sync 自动守护
- 验证:cargo test + `cd app && pnpm test` + `pnpm build`(vue-tsc);手动:设置面开关持久化 + 立即清理跑通(本机实测回收 backups)

## PR4 — WebKitCache 启动清理 + 文档销账

- [ ] lib.rs GUI setup 异步:WebKitCache 递归大小 > 50MiB → remove_dir_all(常量 + env)。**装配位置:setup(:152)内 mode resolve(:161)之后、Thin 分支 return(:183)之前的公共区**——勿仿 PR1 的 Full 分支装配(那些点在 Thin return 之后,WebKitCache 正是 Thin GUI 的产物,照搬 = 默认模式永不清理;design §4 陷阱警示)
- [ ] 测试:阈值上下界(不触发/触发)、目录缺失 no-op,**装配级验证**(mode=Thin 路径会调用清理函数——函数级测试抓不到装配缺失,外部评审 2026-09-03 指出的 AC6 风险)
- [ ] 文档:
  - ROADMAP §1.2 新行 + 第三档 F3 行更新(余留改「进程/内存 + F1 反压联动 + DB VACUUM follow-up」)
  - BACKLOG 无条目(F3 只在 ROADMAP)
  - 新 spec `.trellis/spec/backend/disk-governance.md`(回收契约:各消费点策略/参数/开关/双入口;outputs 回收对 C6 恢复链路的降级说明)
  - RULE-DAEMON-001 相关 spec 段落更新(日志落盘方变更)
- 验证:全量后端 + 前端 + e2e;`scripts/turn-smoke.sh` 确认 agent 链路无回归

## 完工门(全任务)

- [ ] `python3 ./.trellis/scripts/task.py validate 09-03-f3-disk-governance`
- [ ] 最终全量:cargo test -p everlasting --lib(约 2200+)+ clippy -D warnings + app pnpm test + vue-tsc + e2e
- [ ] 本机实测一轮:手工造孤儿 worktree/outputs → daemon 重启 → 5min 内节拍回收 → 设置面概览数字下降
