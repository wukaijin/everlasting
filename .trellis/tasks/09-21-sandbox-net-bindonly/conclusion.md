# 结论 — 09-21-sandbox-net-bindonly

**状态**:实现完成,全量验证绿(见下);归档走 finish-work 流程。
**实现 session**:ZCode(Carlos),2026-09-21,单日 Steps 0-7 顺序完成。

## 交付面(vs PRD R1-R9 / AC1-AC8)

| 需求 | 落点 | 状态 |
|---|---|---|
| R1 NetPolicy 维度 | `sandbox/policy.rs`(NetPolicy/BindSet/parse fail-closed)+ `projects.sandbox_net` 列(add-column 零重建)+ `project_net_snapshots`/`project_net_proposals` 表 | ✅ |
| R2 三态互斥执法 | `sandbox/mod.rs` `PreparedNet`(Block=现行 filter 字节不变 / BindOnly=NET_PORT attrs 不装 seccomp / AllowAll=构造不出) | ✅ 类型系统保证 |
| R3 能力门降级 | `Capability.landlock_net`(真建 ruleset 探针);不支持 → prepare 入口降 Block + warn + summary 记 degraded;**本机(ABI 3)live 实测降级路径** | ✅ |
| R4 bind 快照 | `(project_id, worktree_key canonicalize)` 键;写通道 confirm 时钳位拒绝回显冲突口 + prepare 双道防御减除;建议(propose)永不直接生效 | ✅ |
| R5 ready_port | `procnet` 模块(/proc→inode→pid→pgid/comm)+ registry 探测任务(四铁律)+ `shell_status` Ready/TimedOut 呈递 | ✅ |
| R6 F1 canonicalize | build_spec exec 根 canonicalize(失败跳过 trap 5)+ summary `exec_roots_canonical=[..]` | ✅(对 pnpm 不充分,见下) |
| R7 F3+remediation | `classify_block` exit 126 分支 → `ExecFace`;网络/exec 文案改「收敛到 ONE operator 指令然后停」 | ✅ |
| R8 可观测+文案 | summary `net=` 三态段(+降级后缀);Settings 网络档 + 快照/建议管理 + 文案三硬要求(vitest 断言) | ✅ |
| R9 合取项 | `SandboxSpec::net_enforcement()` 一处真源(summary/classify/prepare 三读);Network 归类仅 InetBlock | ✅ 负例有锚 |

## 验证

- `cargo test -p everlasting --lib`:**2572 通过 / 1 失败(既有环境败,非本任务)** — 失败的是 `commands::evl_cli::tests::install_creates_managed_layout_and_is_idempotent`,断言「PATH 上无 evl」,而本机今晨装了 evl(4dba444b 的功能);本任务零改 evl_cli/PATH,CI 无 evl 不触发。
- `cargo test -p everlasting-remote`:89 绿(未受影响,确认)。
- `cd app && pnpm test`:**149 文件 / 2029 测试全绿**(含新增 ProjectSandboxTab.net 7 例 + routes-sync 守卫)。
- `scripts/turn-smoke.sh --sandbox-probe`:**通过**(sandboxed_shell_execution 审计行在,Block 语义零回归,真 LLM 轮)。
- 真内核 BindOnly 矩阵(`integration_bind_only_net_matrix`):本机 **大声 SKIP**(ABI 3 < 4),降级路径已 live 单测覆盖。

## 两个重要的研究修正(research/live-probe §4-§5)

1. **F0 归因假设不成立**:pnpm 126 的 exec 目标在**所有 PATH 目录之外**(global/ 与 bin/ 平级),canonicalize 前后都不进面 —— F1 不充分已如实记录;**F2(`sandbox_extra_exec`)提前进挂账评审**。反向验证:exec 面补 global+store 两根后沙箱内 `pnpm --version` exit 0(证明 exec 缺口是唯一阻塞,seccomp 不干扰 pnpm)。
2. **本机有 Landlock ABI v3**(非研究初稿所记「无 Landlock」;`/sys/kernel/security/lsm` 不存在只是 securityfs 未挂载,syscall 探针返回 3)。文件面沙箱在本机活跃 —— 与 evl 实测 EPERM/126 自洽。**BindOnly 仍 blocked-on-kernel**(需 ABI ≥4 / 内核 ≥6.7;解锁:`wsl --update` 后 `uname -r` ≥6.7 + syscall 探针 ≥4)。

## 已知限制 / 挂账移交

- **Live BindOnly 端到端冒烟 = blocked-on-kernel**:换内核后以 jjh-mono web(Vite:3001)验证 —— 声明端口 bind 过 / 未声明拒 / 7456 不可达(PRD 预告,非验收阻塞)。
- **F2 exec 白名单**:pnpm/JDK(global 目录型安装)的真正解法;operator 供应链风险先写进 spec(§13.6)再评审。
- **AllowAll**:capability token + 「daemon TCP 回路是控制面唯一路径」不变量测试化(§13.6)。
- 并行任务 `09-21-durable-prefix-grant`(prefix-grant 持久化)与本任务正交;本任务 F3/remediation 文案是其触发入口。
