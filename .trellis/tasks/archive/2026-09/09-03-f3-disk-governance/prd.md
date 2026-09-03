# F3 磁盘治理:worktree / outputs / 日志 / 备份 / 缓存限损

## Goal

ROADMAP 第三档 F3 磁盘余留项收口:给 agent 后台作业产生的磁盘数据建立自动限损机制,消除「无限增长无回收」缺口。用户价值:长期使用的个人机器不会因 agent 数据缓慢膨胀吃满磁盘,且占用状况在设置面可见、可控、可手动清理。

## Background(2026-09-03 摸底,详见 research/disk-usage-audit.md)

实测本机大头是辅助数据:DB 备份 175M > WebKitCache 136M > daemon 日志 59M > DB 本体 26M;worktree 本机占比小但机制缺口最严重。关键缺口:

- **P0-a** worker worktree sweep 宿主断链:只在 GUI Full 逃生模式跑(`lib.rs:231-235`),daemon bin(默认模式真正宿主)不调
- **P0-b** 日志运行期不轮转:唯一轮转是 daemon.sh bg 启动前 >10MiB 检查,连续运行期无限涨(单份 29M 实证);Rust 侧零文件 sink
- **P1-a** session 孤儿 worktree(目录在、DB 行没了)无回收;sweep 只遍历 `worker/` 子目录
- **P1-b** outputs spill 仅随 session 删除;`_no_session` 无人扫;session 永不删则永不删
- **P2** 备份 7 份固定保留(175M,单份已涨到 26M 超原估算)、WebKitCache 136M 零治理

## Requirements

**R1 范围**(用户裁定 2026-09-03):P0+P1 全部 + P2 便宜两项(备份大小自适应、WebKitCache 治理)。

**R2 删除边界**(用户裁定 2026-09-03,「推荐档 + 设置面」):

- 孤儿(无 DB 行的 worktree/outputs)+ 可再生数据(日志滚动/WebKitCache/旧备份)全自动回收
- 有主 outputs(session 还在)按 30 天年龄自动回收,默认开、设置面可关
- 设置面新增磁盘治理区块:占用概览(只读)+ 回收开关 + 手动「立即清理」按钮

**R3 机制形态**(design.md §1-7 详细设计):

- 每日节拍 daemon bin 装配(首拍延迟 5 分钟);kill-switch `diskGovernorEnabled`(fail-open);GUI Full 保持一次性 startup pass(零 timer 约束)
- 日志进程内文件 sink + 10MiB×3 尺寸轮转(零依赖手写);daemon.sh bg 重定向退役
- 备份预算 200MiB(最少 2 份/最多 7 份);WebKitCache GUI 启动阈值清理(>50MiB)
- 数值参数常量 + env 覆盖(不上设置面;Key Decision 见 design §6)
- 新 IPC `get_disk_usage` / `run_disk_cleanup` 双注册(Tauri + daemon HTTP)

## Acceptance Criteria

- [ ] AC1 daemon bin 启动后磁盘节拍在跑:worker worktree sweep 在 daemon(非 GUI Full)宿主生效——伪造超龄 worker worktree,daemon 重启 + 等待首拍后被回收
- [ ] AC2 孤儿 session worktree(DB 行不存在)被回收;DB 行存在(含 Detached)的 worktree 不被触碰;locked 跳过
- [ ] AC3 outputs:孤儿桶与 `_no_session` 被回收;有主桶超 30 天被回收;`outputsAgeCleanupEnabled=false` 时有主桶不删
- [ ] AC4 daemon.log 连续写入超 10MiB 触发滚动,最多保留 3 个旧代;daemon.sh bg 模式日志仍落同一路径,`logs` 子命令行为不变
- [ ] AC5 备份总量超 200MiB 预算时旧份被 prune,但始终保留最近 2 份;预算内保持现状(≤7 份不删)
- [ ] AC6 GUI 启动时 WebKitCache > 50MiB 被清空,< 50MiB 不动;目录缺失 no-op
- [ ] AC7 设置面磁盘区块:占用概览显示各消费点字节数;两个开关持久化(重启仍生效);「立即清理」执行后 toast 展示逐项回收摘要,概览数字同步下降
- [ ] AC8 浏览器 remote 模式设置面区块可用(HTTP 路由可达,transport 映射不缺)
- [ ] AC9 `diskGovernorEnabled=false` 时节拍空转,但手动「立即清理」仍可用
- [ ] AC10 全量门禁绿:cargo test --lib(既有 ~2240+ 新增)/ clippy -D warnings / pnpm test / vue-tsc / e2e;turn-smoke 无回归

## Out of Scope

- DB VACUUM / WAL 大小治理(26M 不紧张,留 follow-up)
- 进程/内存限损、F1 反压联动(磁盘满→agent loop 反压;ROADMAP F3 正交维度,留 follow-up)
- Provider API 限流(ROADMAP F3 边界,C5 已移除)
- attachments 年龄回收(已有 session 级生命周期,实测非大头)
- 浏览器端缓存(remote PWA 归浏览器管)
- 数值参数(天数/预算/轮转大小)的设置面编辑(常量+env;Key Decision 见 design.md §6)
