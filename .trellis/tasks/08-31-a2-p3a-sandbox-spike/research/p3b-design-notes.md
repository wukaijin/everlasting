# 调研产出 — P3b 设计要点(spike → 实施的移交件)

> 本文是 spike 的前瞻性结论,供 P3b 立项时直接引用改写为 design.md。
> 属建议性质:P3b 立项评审时可推翻,推翻处需注明理由。

## 主张

**主路线:Landlock + seccomp 进程内沙盒,纯 Rust 实现,daemon fork 后 pre-exec 子进程施加。**
bwrap 可选增强档;判定层(⑨ classify)不动。依据见 landlock / generalization / prior-art 三篇。

## 规则集

- **handled 权限**:EXECUTE + 写族(WRITE_FILE / REMOVE_* / MAKE_*);读不控。
- **可写根**:worktree + `/tmp` + `app_data_dir/outputs/<session>`(spill)。
  **一律服务端解析,拒绝 LLM 提供的 cwd/路径**(CVE-2025-59532 铁律)。
- **exec 允许面**:PATH 目录 ∪ /dev ∪ /tmp ∪ worktree ∪ 探测到的工具链目录;
  **显式不含 `/init`、`/mnt/c`**(interop 收口)。设备节点 per-file WRITE_FILE
  (`/dev/null /dev/zero /dev/full /dev/random /dev/urandom /dev/tty`)。
- **断网**:seccomp 拦 `socket(AF_INET/AF_INET6)` → EPERM;**AF_UNIX 放行**。
  DNS 死亡 = 预期。Landlock 网络规则要 ABI v4(内核 6.7+),不赌,一律 seccomp。
- **NoNewPrivs**:restrict_self 前必须,顺带封 suid。

## 工程要点

- **能力探测**(generalization 篇 §3 阶梯):启动探测 Landlock ABI + seccomp;
  失败 → fail-open(现状路径)+ 设置面横幅,禁止悬挂(Codex #1039 教训)。
  可选探测 bwrap(增强档,默认关)。
- **缓存目录 allowlist**:`~/.cargo`、pnpm store、`~/.cache` 等默认探测 + settings
  可配置——这是「误杀」重灾区(cargo build 第一步就写 `~/.cargo`)。
- **失败升级闭环**:沙盒内命令因写限制 EACCES → 升级为 Ask(参照 Claude Code /
  Codex 的 sandbox-fail→escalate 交互),不能让用户看裸报错猜原因。
- **实现选型**:Landlock 用裸 syscall 常量或 `landlock` crate(按 ABI 降级,
  勿信发行版 UAPI 头);seccomp 手写 BPF 常量或 `seccompiler`;
  **不引 libseccomp C 依赖**。
- **Tier 集成**:ReadOnly 档 shell 进沙盒免弹窗(判定层语义不变);
  Plan 模式全命令沙盒化的三态 UX(只读沙盒/读写沙盒/放行)属 **P3c**,
  P3b 落地后拿真实使用体验再设计,现在不做。

## 回归要点(测试矩阵雏形)

- 沙盒内写项目外路径 → 失败;写 worktree/spill/tmp → 成功。
- `.exe` 与 `/init` → 拒(EXECUTE 面);`/mnt/c` 下任何 PE → 拒。
- git 在 worktree 内正常(含 `.git` 写、`/dev/null` 打开)。
- cargo build:allowlist 生效 → 成功;清空 allowlist → EACCES → 升级 Ask。
- 断网:`curl` / `git fetch` 失败;AF_UNIX(pnpm store 类)不受影响。
- 能力探测:人为 disable(LD_PREVIEW 探测桩 / 降级开关)→ fail-open 路径行为与现状一致。

## 规模预估

与 C6 同量级(执行器模块 + shell tool 分发接线 + 测试矩阵 + spec 收编);
UX(P3c)另计。P3b 是否立项、何时立项,由用户在任务评审时定。
