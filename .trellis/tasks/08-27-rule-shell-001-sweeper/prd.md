# RULE-SHELL-001 background shell 条目清扫 sweeper

## Goal

闭合 `.trellis/reviews/DEBT.md` §RULE-SHELL-001(P2):已结束的 background shell 条目
永不清除,daemon 长驻进程下 `shells` map 无上界增长(内存大头是 Done 条目携带的
完整 stdout/stderr 缓冲,即使已落盘也不释放),成为真实 OOM 面。本任务落地定时
清扫,消除无上界增长。

## Background

- 泄漏面画像、既有语义依赖(`status()` 已预留 "already cleaned up → NotFound")、
  触发机制选型(interval vs event-triggered)见 `research/codebase-findings.md`。
- 技术方案细节见 `design.md`。

## Requirements

- R1 定时清扫:daemon 进程内周期性移除**已结束(Done)且超过保留期**的 shell 条目,
  释放其持有的 stdout/stderr 缓冲与关联字段。
- R2 只扫 Done 条目;Running 条目绝不清除(防孤儿化 kill 通道)。
- R3 清扫后 `shell_status` / `shell_kill` 对该 shell 返回 NotFound——这是
  `BackgroundShellRegistry::status` 文档既有语义("or was already cleaned up"),
  非行为破坏。
- R4 清扫逻辑必须可独立单测(retention 可注入),不依赖真实等待 1 小时。
- R5 遵循架构约束 "no timer tasks in the GUI main process":sweeper 只在
  daemon bin 装配,GUI(Tauri)路径零改动、零新定时任务。
- R6 销账:移除 `in_memory.rs` 内两处 "TODO: PR3 or follow-up" sweeper 注释,
  模块文档同步更新;任务闭合时从 DEBT.md 删除该条目。

## Non-Goals

- **通知队列清理**:每 session cap 100 × 小结构(约几十 KB),且保留可让迟归
  session 仍拿到结果通知(outcome/exit_code 自包含)。不动。
- **spill 文件清理**:落盘文件生命周期挂在 session 上(delete_session 时
  `cleanup_outputs_dir`),与内存条目无关。不动。
- **Running 条目超龄清除 / max_runtime 上限收紧**:Running 有既有 24h 默认
  runtime 定时器兜底,不属于本债。
- **Done 条目 buffer 预收缩**(spill 后只留 head/tail preview):更激进的内存
  上限,收益与复杂度不成比例,留待真实内存压力出现再评估。

## Acceptance Criteria

- [x] AC1 `InMemoryBackgroundShellRegistry` 提供可注入 retention 的清扫方法;
      单测覆盖:retention=0 时 Done 条目被清(`status` 转 NotFound)、Running
      条目保留、近期完成的 Done 条目在 retention 内保留。
- [x] AC2 清扫方法在锁内完成纯 map 遍历 + 时间戳比较,无 I/O、无 await 嵌套。
- [x] AC3 daemon bin 装配 sweeper(仿 `spawn_backup_task` 模式:detached
      `tokio::spawn` + `tokio::time::interval`,wrapper 落在 `daemon/server.rs`);
      GUI/Tauri 路径无任何改动。
- [x] AC4 两处 "TODO: PR3 or follow-up" 注释删除,`shells` map 的文档注释
      改为描述现行清扫行为。
- [x] AC5 存量测试全绿:`cargo test -p everlasting --lib`(WSL 需
      PKG_CONFIG_PATH,见 AGENTS.md);clippy 无新告警。
- [x] AC6 闭合销账完整:DEBT.md 删除 RULE-SHELL-001 条目,**并同步** P2
      section 头计数(2→1)、底部优先级分布表(P2=1 / Total=11),仿
      RULE-ARGS-001 先例在 header 下加闭合注记(指向本任务);spec
      `daemon-server.md` §运维伴生物 Pattern 补 sweeper 一句(Phase 3.3,
      与 backup task 同页)。

## Constraints

- 保留期默认 1h、清扫间隔 5min(依据见 design.md §参数);常量与既有
  `DEFAULT_MAX_RUNTIME_MS` / `MAX_NOTIFICATIONS_PER_SESSION` 同风格锚定测试。
- `BackgroundShellRegistry` trait 面不加方法(清扫是 impl 私有生命周期管理)。
- 通知队列、spill 文件不在清扫范围(见 Non-Goals)。
