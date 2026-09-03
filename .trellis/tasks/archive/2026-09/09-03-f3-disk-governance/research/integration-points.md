# F3 设计前调研:设置面 / 节拍 / 日志 / IPC 接线点

> Explore 子代理调研,2026-09-03。为 design.md 提供接线证据。路径相对仓库根。

## 1. 设置面架构 + config 持久化链

- 壳 `app/src/components/settings/SettingsModal.vue`(搜索 + 全局/项目 scope 分段 :251-270 + 左导航右内容,`<component :is>` 动态渲染 :352)
- 纯数据注册表 `settings/registry.ts`:`SETTINGS_CATEGORIES`(:42-139),`SettingsGroup` 联合(:17)= 模型/智能体/集成/远程,`SETTINGS_GROUP_ORDER`(:35-40);组件映射在壳 `CATEGORY_COMPONENTS`(:71-84)
- **持久化 = SQLite KV 表 `app_config`**(无 ~/.config 配置文件;建表 `db/migrations/schema.rs:259`,读写 `db/config.rs:37-82` = get :37 / set :46 / delete :67 三函数)
- 读链:`stores/config.ts::load()`(:137-185)→ `get_app_config` → `commands/config.rs::get_app_config_inner`(:525-559)→ `AppConfigPayload`(:489-523,camelCase wire,**additive 加字段即可,不单开命令**)
- 写链:布尔 `set_app_config_flag`(:607-614,白名单 `SETTABLE_APP_FLAGS` :582-586;存储语义仅字面 `"false"` 关,fail-open);数组 `set_app_config_list`(:650-657)。**数值现无通道**(需新白名单命令或字符串解析)
- daemon HTTP 镜像:`daemon/routes/config.rs:141-143` 三路由,Q0 单源三层
- 开关行最简先例 `settings/GeneralTab.vue`:`FlagRow` interface :37 + `rows` :47 + `onToggle` :74(pending/toast 模式)
- per-project 先例 P3c:`settings/ProjectSandboxTab.vue`(props projectId + 乐观选中失败回拨)
- CRUD 面先例:`settings/ScheduledTasksTab.vue`(kill-switch 警告条消费 configStore :68/:89)

## 2. daemon 常驻节拍先例(daemon/server.rs)

| 先例 | 位置 | 形态 |
|---|---|---|
| backup 24h | `spawn_backup_task`(:72-117) | detached spawn + `interval(24h)`,**首拍立即=启动即跑**;失败仅 warn 无重试;无 shutdown 处理(可硬斩) |
| shell sweeper 5min | `spawn_shell_sweeper`(:139-153) | 同上,首拍空 no-op |
| F2 scheduler 30s | `spawn_task_scheduler`(:194-218) | `tokio::select! { biased; cancel.cancelled() => break; interval.tick() => ... }`;token 挂 `AppState.scheduler_cancel`(state.rs:263);shutdown 步骤 1.6(server.rs:498)cancel;kill-switch 键常量 scheduler/mod.rs:69,**每 tick 重读 fail-open 的实现在 `scheduler_tick_with_fire` :236-247**(get_config_value → `!= "false"`,仅字面 "false" 关) |

**宿主线**:
- daemon bin `bin/everlasting-daemon.rs`:tracing init(:53-58)→ orphan-guard → port probe → `load_daemon_state`(:151)→ spawn_backup_task(:158)→ spawn_shell_sweeper(:165)→ wire events(:171)→ spawn_task_scheduler(:179)→ tunnel(:189-212)→ serve(:214)。**所有 timer 都在这**
- GUI `lib.rs`:`GuiMode::resolve`(:161);Thin 早退(:163-183);Full(:185-251)= AppState + emitter + **一次性** sweep_stale_workers(:231-235,体 :94-138)+ 一次性 memory hygiene(:246-249)。**「GUI 主进程零 timer task」是硬约束**(server.rs:70-71/:137-138/:179-180 反复声明)
- ⚠ **Thin 早退陷阱(外部评审 2026-09-03 指出)**:setup 钩子 `.setup(|app|)`(:152)内 mode resolve 在 :161,**Thin 分支 :183 提前 `return Ok(())`**——现有 sweep(:234)/hygiene(:248)装配点全在其后的 Full 分支内。凡「Thin/Full 都要跑」的 GUI 侧逻辑(如 WebKitCache 清理),必须插在 mode resolve 之后、Thin return 之前的公共区;照 Full 分支模式装配 = 默认 Thin 模式永不执行
- 新 disk governor:装配点唯一 bin/everlasting-daemon.rs(backup 旁);GUI Full 只加一次性 startup pass(照 :231-235);延迟首拍用 `interval_at(now+delay, period)`

## 3. 日志/tracing 与文件 sink 共存

- 现状:GUI `main.rs:41-44` / daemon bin `:53-58`(缺省 `info,everlasting=debug`)均未配置 `.with_writer()`——fmt **默认 writer = stdout**(tracing-subscriber 0.3.23 `fmt/mod.rs:8`;外部评审 2026-09-03 指正,早期误记 stderr。daemon.sh `2>&1` 与 sidecar 双管道均两流全抓,stdout/stderr 对行为等价);**Rust 侧零文件 sink、零 XDG_STATE_HOME 引用**;sidecar 抽回 `sidecar.rs:191-223`(Stdout→info!/Stderr→warn! 转投 GUI 订阅器)
- daemon.sh:`STATE_DIR=$XDG_STATE_HOME|~/.local/state}/dev.everlasting.app`(:54-55);bg 模式 `nohup … >> daemon.log 2>&1 &`(:142-149);启动前 `rotate_log`(:73-87,>10MiB 滚 .1/.2/.3);前台 start `exec` 打终端(:164);`logs` 子命令 `tail -f`(:227-231)
- 依赖:仅 tracing + tracing-subscriber(env-filter);**无 tracing-appender**(全 workspace 空)
- **双写者冲突**:若 Rust 进程内写文件,daemon.sh bg 的 `>>` 必须去掉(否则 fd 交错+轮转打架);`rotate_log` 退役(Rust mv 滚动会自然覆盖消化旧代);`logs` 子命令路径不变
- 选型:(a) 加 tracing-appender(size-based Rotation 需核对版本 API)vs (b) 零依赖手写 MakeWriter(write 前 `metadata().len()` 检查 + mv 滚动)——**仓库有「不为此拉新 crate」先例**(tools/glob.rs:205-207 手写 walk_dir 替代 walkdir)
- sidecar 模式收益:daemon bin 自带文件 layer 后,打包 GUI 的 daemon 日志首次落盘(sidecar 抽回管线与文件 layer 并行,`.with()` 多 layer)

## 4. 目录大小统计 + IPC 双注册

- **无现成 du**:最接近 `tools/glob.rs:205-227` 手写 walk_dir(stack + read_dir,不可读跳过,不拉 walkdir);F3 目标目录有界已知,同步 walk + `spawn_blocking` 即可(files.rs @-walker 先例:command 层 spawn_blocking :16-17)
- IPC 双注册先例 = background_shell:list(2026-09-02)五处:`commands/background_shells.rs`(_inner + #[tauri::command] 薄包装)→ `lib.rs:429-430` invoke_handler → `daemon/routes/background_shells.rs`(handler **snake_case 扁平标量** body,嵌套 struct HTTP 模式静默 miss)→ `daemon/routes/mod.rs:100-103` nest + :48-50 pub mod → `transport/http.ts` CMD_TO_DOMAIN(整表 :54 起,background_shells 条目 :204-209)(**漏加被 `transport/http.routes-sync.test.ts` 解析 Rust 路由源的守卫拦下**);另 `commands/mod.rs` pub mod
- wire:`#[serde(rename_all="camelCase")]` + 顶层扁平;新域推荐 `commands/disk.rs` + `daemon/routes/disk.rs`(config.rs 已 800 行不推荐挂)
- 立即清理与节拍共享同一 `_inner` 回收实现;Tauri Full 模式按钮落在 GUI 进程的行为差异需注释(同 lib.rs:288-290「GUI Full 不跑 scheduler」先例)
