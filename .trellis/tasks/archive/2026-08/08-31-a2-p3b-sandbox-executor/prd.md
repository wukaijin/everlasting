# PRD — A2+ P3b 执行期沙盒:Landlock+seccomp 执行器接入 shell 工具链

> 前置:P3a spike(已 completed,`08-31-a2-p3a-sandbox-spike`)。主路线与本任务的设计依据
> 全部来自 spike 产出:[spike PRD](../08-31-a2-p3a-sandbox-spike/prd.md) +
> [p3b-design-notes](../08-31-a2-p3a-sandbox-spike/research/p3b-design-notes.md)
> (含实测矩阵、五条实施陷阱、泛化性分析)。
> 源方案:[docs/_history/2026-08-28-a2-shell-classification.md](../../../docs/_history/2026-08-28-a2-shell-classification.md) §4.3。

## 背景与问题

判定层(A2+ P1+P2,`classify_prefix` 三档)只覆盖静态可判定的命令。变量展开 / `$()` /
`eval` / alias / 间接副作用是静态分析永远堵不上的盲区——判定层把 `FOO=rm x` 误判成
ReadOnly 时,今天它会被静默放行且无任何兜底。

P3 的定位:**判定层之下的执行期限损层**。判定为 ReadOnly 的命令在 Landlock+seccomp
约束下执行——即使误判,损害被限制在「worktree + tmp + spill 可写、其余只读、无出网、
无 `/init` 与 `/mnt/c` 执行」之内。判定层语义零改动。

## Goal

Landlock + seccomp 沙盒执行器落地到 shell 工具链(前台 `shell` + 后台
`run_background_shell` 两条 spawn 路径),**ReadOnly 档命令默认进沙盒**;
能力探测失败 fail-open;单一 kill-switch 可整体关闭回到现状行为。

## Requirements

- R1. **沙盒执行器模块**(纯 Rust,裸 syscall 走既有 `libc` crate,**零新增依赖**):
  Landlock ruleset(EXECUTE + 写族 handled,读不控)+ seccomp BPF
  (拦 `socket(AF_INET/AF_INET6)`,AF_UNIX 放行)+ `PR_SET_NO_NEW_PRIVS`。
- R2. **规则集内容**(spike 实测配方,陷阱见 design.md):
  - 可写根:session cwd(worktree)+ `/tmp` + `session_outputs_dir(data_dir, session)`
    (C6 spill 目录)——**全部服务端解析,永不采信 tool 参数中的路径**
    (CVE-2025-59532 铁律);
  - exec 允许面:PATH 解析目录 ∪ `/dev` ∪ `/tmp` ∪ 可写根 ∪ 探测的工具链目录;
    **显式不含 `/init`、`/mnt/c`**(WSL interop 收口);
  - 设备节点 per-file `WRITE_FILE`(`/dev/null` `/dev/zero` `/dev/full`
    `/dev/random` `/dev/urandom` `/dev/tty`)。
- R3. **接入点**:`tools/shell.rs` 与 `background_shell` 模块两处 spawn,在 pre-exec
  阶段施加;`shell` / `run_background_shell` 行为语义除沙盒约束外不变
  (超时 / 管道排空 / PGID / safe env / 截断契约全保持)。
- R4. **触发条件(四项与)**:命令经 `classify_prefix` 判为 `ReadOnly` 且 mode 非
  Yolo 且 kill-switch(`sandbox_enabled`)开启且能力探测通过。SideEffect / Ask 档
  不沙盒(它们本就经用户授权)。数据流落法见 design §2.2(D5)。
- R5. **能力探测**:进程启动后惰性探测(Landlock ABI ≥1 + seccomp 可用,`OnceLock`
  缓存);任一不可用 → fail-open(不沙盒、不报错、不悬挂)+ 日志一行。
- R6. **kill-switch**:app config 新增 `sandbox_enabled`(默认 `true`);
  关闭后 spawn 路径不设 pre_exec,行为与现状逐字节一致。
- R7. **缓存目录 allowlist**:`~/.cargo` 进默认可写清单;额外目录存 config
  (`sandbox_extra_writable`,默认空数组);沙盒内写被拦时,tool 输出给模型一句
  明确指引(路径被沙盒拦截 + 如何让用户放行)。
- R8. **可观测**:审计事件新增 `SandboxedShellExecution` kind(记录 ruleset 摘要);
  探测结果与 kill-switch 状态经 `get_app_config` 暴露给设置面。

## 约束

- C1. 零新增 crate 依赖(Landlock/seccomp 裸 syscall via `libc`,ABI 常量自写,
  **不引 `landlock` / `seccompiler` / `libseccomp`**)。
- C2. 判定层(`shell_trust.rs`)零改动——沙盒是执行层,不碰分类语义与 Tier 4 决策。
- C3. wire 变更 additive(`get_app_config` 加字段);daemon + Tauri 双 transport 同步。
- C4. 非 Linux 平台编译通过(模块 `#[cfg(target_os = "linux")]`,其余平台 no-op)。
- C5. 生产代码不得复现 spike 陷阱 2(rule 权限必须是 handled 子集,否则 EINVAL)——
  rule 构造以类型系统保证。

## 非目标

- **P3c UX**:Plan 模式全命令沙盒化、只读/读写/放行三态、per-project 配置、
  bwrap 可选增强档——P3b 落地后按真实体验另立。
- 沙盒失败 → 自动升级 Ask 的完整交互闭环(v1 以错误输出指引代替,闭环归 P3c)。
- 网络白名单 / egress 代理(v1 只做断网)。
- WSL1 专项支持(落入 fail-open)。
- read 限制、Landlock ABI ≥4 网络规则(不赌内核版本,断网一律 seccomp)。
- **interop socket 残余面**(评审 B2/D4):绕过 `/init` 直连 interop unix socket
  的原始协议路径 v1 不封——seccomp 无法按路径匹配 connect 的 sockaddr 指针,
  Landlock ABI v1 无 connect 权限位;易用逃逸路径已被 EXECUTE 拒绝面封死,
  原始协议逆向成本高。完整收口(namespace 路线 tmpfs 盖 socket)归 P3c
  bwrap 增强档;v1 在 spec 如实记录。

## Acceptance Criteria

- [x] AC1. 集成测试(Linux):ReadOnly 档命令进沙盒后——写 worktree/`/tmp`/spill 成功;
      写 `~`、`/usr/local` 被拒;exec `/init` 与 `/mnt/c/**/*.exe` 被拒;读任意路径不受限。
- [x] AC2. 集成测试:沙盒内 `bash -c 'echo > /dev/tcp/1.1.1.1/443'` 失败(EPERM);
      AF_UNIX socket(pnpm store 类路径模式)不受影响。
- [x] AC3. 集成测试:SideEffect 档(如 `mkdir`)与 Ask 档执行路径与 main 现状一致
      (无 pre_exec);Yolo 下 ReadOnly 命令同样不沙盒。
- [x] AC4. kill-switch 关闭 → spawn 命令与现状一致(不设 pre_exec),全量回归绿。
- [x] AC5. 能力探测:模拟探测失败(测试桩)→ fail-open 不沙盒、无 panic、无悬挂,
      日志留痕。
- [x] AC6. background_shell 路径同策略:`run_background_shell` 的 ReadOnly 档命令
      同样受沙盒约束(集成测试覆盖)。
- [x] AC7. 配置面:`get_app_config` 返回 `sandbox_enabled` / `sandbox_extra_writable`;
      沙盒拦截写入时 tool 输出含指引文案(单测钉死文案要点)。
- [x] AC8. 全量回归:`cargo test -p everlasting --lib` + e2e + 前端 vitest/build 绿;
      `turn-smoke.sh` live 过(真实 LLM 跑 ReadOnly 工具命令,确认无误杀,脚本含
      `SandboxedShellExecution` 审计 kind 计数断言)。
- [x] AC9. spec 收编:新增 `.trellis/spec/backend/sandbox-executor.md`
      (规则集契约 / 五条陷阱 / fail-open 语义 / kill-switch);ROADMAP A2+ P3 行移档。

## 决策点(评审时定,缺省按推荐)

| # | 问题 | 推荐 |
|---|---|---|
| D1 | `sandbox_enabled` 默认值 | `true`(fail-open + kill-switch 已足够安全;默认关则上线即死功能) |
| D2 | 审计 kind 新增 vs 复用 | 新增 `SandboxedShellExecution`(单 enum 追加无迁移) |
| D3 | bwrap 增强档是否留 P3b 钩子 | 不留(P3c 一起做,避免模块级 dead_code 伞——DEBT 历史教训) |
| D4 | interop socket 残余面 v1 处置(评审 B2) | 接受并文档化(见非目标;完整收口归 P3c bwrap 档) |
| D5 | mode 数据流方案(评审 B1) | `ToolContext` 加 `mode` 字段(dispatch 灌入)+ 后台 `Registry::start` 加 `sandbox` 参数下传(design §2.2) |
