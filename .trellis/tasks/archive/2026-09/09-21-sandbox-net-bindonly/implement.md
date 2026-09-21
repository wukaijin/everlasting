# Implement — 执行计划

> 验证基线（每步后跑，最终全量）：`cargo test -p everlasting --lib`（WSL 需 `PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"`，从仓库根跑）。前端涉及步骤另跑 `cd app && pnpm test`。

## Step 0 — F0 归因验证（只读勘察，不写代码）

- [x] 起一个沙箱 shell（走 daemon 或单测探针）执行 `command -v pnpm && readlink -f "$(command -v pnpm)"`，比对包装脚本 exec 目标（`/root/.local/share/pnpm/global/v11/*/@pnpm/exe/pnpm`）与 `SandboxSpec` exec 根清单（summary 输出）。
- [x] 结论（成立/不成立 + 证据）记入 `research/live-probe-2026-09-21.md` §4。不成立 → Step 5 的 F1 范围重审（可能需 F2 提前），本步结论决定。

## Step 1 — NetPolicy 类型与存储（R1）

- [x] `sandbox/policy.rs`：`NetPolicy` + `BindSet` + parse（fail-closed Block + warn），单测：三形 parse、非法整串、bind_only 空端口/超范围。
- [x] `db/migrations/columns.rs` + `schema.rs`：`add_project_column_if_missing` 加 `projects.sandbox_net TEXT`（NULL=Block）；新表 `project_net_snapshots`（project_id, worktree_key, ports, confirmed_by, confirmed_at）。
- [x] 读路径：项目行带出 net（join/点查，与 sandbox_policy 同源）；无快照行时 BindOnly 回 Block 的降级点。
- 验证：新增单测 + 既有迁移测试全绿。

## Step 2 — Landlock net 执法器（R2/R3，核心）

- [x] `sandbox/landlock.rs`：`LANDLOCK_ACCESS_NET_BIND_TCP/CONNECT_TCP` 常量自写钉死（`abi_net_*` 单测，陷阱 1 纪律）；`NetAccessSet` 类型（⊆handled 类型保证）；net_port_attr 构造。
- [x] `Capability::probe()` 扩展 `net_rules_supported()`（ABI ≥4 + access 位 probe；OnceLock 同款缓存）。
- [x] `sandbox/mod.rs`：`PreparedNet` 三态；`prepare()` 按 NetPolicy 装配（Block=现行 filter 搬入不动、BindOnly=net attrs + connect 派生集 + 防御性钳位、probe 不支持降 Block + warn）；`pre_exec_apply` 互斥 match。
- [x] `SandboxSpec::summary()` 加 `net=` 段。
- 验证：单测（三态互斥构造、派生集计算、钳位、降级、summary 三态锚）+ 真内核集成用例（声明端口 bind 过/未声明拒/connect 白名单外拒/**7456 不可达**；本机无 Landlock 大声 SKIP——AC3）。

## Step 3 — ready_port 就绪探测（R5）

- [x] `/proc` 归因模块：`/proc/net/tcp{,6}` inode → PID → PGID/comm；独立单测（自造进程族）。
- [x] `background_shell/in_memory.rs`：`ShellEntry.ready: ReadyState` 字段 + 探测循环（四铁律：connect-only/loopback 常量/spawn 绑死/仅自注册 entry）；超时 TimedOut；collision（PGID 不匹配）不误报 Ready。
- [x] `tools/run_background_shell.rs`：`ready_port` 可选参数 + zod/校验；Ready/TimedOut 注入文本。
- 验证：AC4 四铁律回归锚 + 既有 `sandboxed_background_shell_enforces_write_face` 不回归。

## Step 4 — F3 识别 + remediation 文案 + R9 合取项（R7/R9）

- [x] `classify_block` 签名扩 exit_code；126+Permission denied → exec-face-miss 分支；宁缺勿滥锚不动（既有测试不改语义）。
- [x] `failure_guidance` 文案：net/exec 面缺口场景「收敛到一条 operator 指令然后停」；P3d offer 注入文本同步。
- [x] R9：网络/exec 归类前置合取 `summary 证明执法器装载`；net=BindOnly 项目不触发 Network guidance 的负例测试。
- 验证：AC6 全部锚 + `tests_sandbox` 既有 `classify_block_reads_stdout_for_listen_denials` 不动。

## Step 5 — F1 canonicalize（R6，视 Step 0 结论）

- [x] `prepare()` exec 面构建：目录 canonicalize（失败跳过 + 日志，陷阱 5 同款）。
- [x] `summary()` exec 段含 canonical 根路径（格式实现期定，只列目录不列文件）。
- [x] 若 F0 证实 pnpm global 目标不在 canonical 面内：记录 + F2 提前到挂账评审，不擅自扩白名单。
- 验证：AC5（沙箱内 `pnpm --version` 过，或如实记录改道）。

## Step 6 — 写通道 + Settings（R4/R8，改动面最大，最后做、独立可回滚）

- [x] daemon route + Tauri command：`propose_net_ports` / `confirm_net_snapshot`（钳位在此拒绝 + 回显冲突口）；IPC 白名单同 `update_project_sandbox_policy` 模式。
- [x] Settings 项目沙箱区：网络档位（Block/BindOnly；AllowAll 灰显挂账标注）+ 快照确认流（复用权限卡视觉）。
- [x] 文案三硬要求落地（vitest 断言：保密性不适用/UDP 不受控/平台不支持禁写）。
- 验证：roundtrip route 测试 + vitest；`docs/DAEMON-API.md` 若新端点则补契约。

## Step 7 — 收尾全量

- [x] `cargo test -p everlasting --lib` 全绿；`cargo test -p everlasting-remote`（不应受影响，确认）。
- [x] `cd app && pnpm test` 全绿。
- [x] `scripts/turn-smoke.sh --sandbox-probe` 本机跑通（Block 不回归；BindOnly SKIP 出 WARN 为预期）。
- [x] spec 更新（Phase 3.3）：`sandbox-executor.md` §12.3 C 标记已实施 + 新增 NetPolicy 章节（或 §13）；AGENTS.md 无需动。
- [x] 任务结论记录 live BindOnly 冒烟 = blocked-on-kernel（换内核后以 jjh-mono web:3001 验）。

## 风险文件 / 回滚点

| 文件 | 风险 | 回滚 |
|---|---|---|
| `sandbox/mod.rs` prepare/pre_exec | 高（fork 上下文纪律） | 三态 match 改走 Block 分支 |
| `sandbox/landlock.rs` 新常量 | 中（ABI 数值错=EINVAL） | 单测钉死；错则该档 SKIP |
| `db/migrations/` | 低（add-column 幂等） | drop 列/表 |
| `background_shell/in_memory.rs` | 中（并发 registry） | ready_port 不传即现状 |
| `tools/shell.rs` classify 签名 | 中（多调用点） | 签名扩展向后兼容 |

## start 前检查

- [x] prd.md / design.md / implement.md 齐
- [x] implement.jsonl / check.jsonl 有真实条目
- [x] 用户已批准最终 planning summary
