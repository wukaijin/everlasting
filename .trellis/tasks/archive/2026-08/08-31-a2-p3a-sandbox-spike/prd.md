# PRD — A2+ P3a 执行期沙盒 spike:WSL2 可行性与主路线定案

> 源方案:[docs/_history/2026-08-28-a2-shell-classification.md](../../../docs/_history/2026-08-28-a2-shell-classification.md) §4.3 阶段 3。
> ROADMAP:第四档 A2+ P3 行(原标注「前置 WSL userns spike」= 本任务)。
> 状态:completed(2026-08-31);P3b 是否立项另评审。

## 文档导航

| 文档 | 内容 |
|---|---|
| [research/wsl2-feasibility-bwrap.md](research/wsl2-feasibility-bwrap.md) | 路线一实测:矩阵 A-R、WSL interop 逃逸分析与收口配方、评估 |
| [research/wsl2-feasibility-landlock.md](research/wsl2-feasibility-landlock.md) | 路线二实测(主路线):规则集设计、矩阵、五条实施陷阱、seccomp 设计 |
| [research/generalization.md](research/generalization.md) | 泛化性分析:内核侧/用户态侧事实分类、能力探测阶梯 |
| [research/prior-art.md](research/prior-art.md) | 业界先例:Codex / Claude Code / microVM 族评估与否决判据 |
| [research/p3b-design-notes.md](research/p3b-design-notes.md) | P3b 设计要点移交件(建议性质,立项时可推翻) |
| [artifacts/](artifacts/) | 探针源码(ll_sbx.c / ll_abi_probe.c / bwrap_matrix.sh)+ 原始日志(见 artifacts/README.md) |

## 背景与问题

A2+ P1+P2 判定层(复合命令拆分 + 写重定向检测)已落地,但静态判定有固有盲区:
变量展开 / `$()` / `eval` / alias / 间接副作用。P3 是判定层**之下**的执行期限损层
——判定错了也限损。源方案候选 bubblewrap / firejail / 自研 overlayfs,并标注
「WSL2 下 bubblewrap 的 userns 可用性需先验证」。

## Goal

回答三个问题并成文,使 P3b 可以直接立项:

1. 候选路线在 WSL2 部署面上是否可行;
2. 结论能否**泛化到任意全新 WSL 安装**(不依赖本机恰好装了什么);
3. 主路线定案 + P3b 设计要点。

## Requirements

- R1. bwrap 路线全矩阵实测:基本执行 / 断网 / 项目可写 / 项目外与家目录拦截 /
  工具链可见 / NoNewPrivs / suid 封锁。
- R2. **WSL interop 逃逸**(沙盒内拉 `.exe` 绕到 Windows 侧)必须被发现、根因分析、
  并对每条候选路线给出**已实测**的收口配方。
- R3. Landlock + seccomp 路线同矩阵实测,含 EXECUTE 位收口 interop 的验证与
  实施陷阱记录。
- R4. 泛化性:实测事实逐条分类(内核侧 vs 用户态侧),给出能力探测与 fail-open 策略。
- R5. 业界先例调研,每个方案按「原语依赖 / 文件系统视图 / 部署面假设」评审;
  microVM 族给出明确否决判据。
- R6. 探针源码与原始日志留档,可复跑。

## 约束

- C1. 全部探测非破坏性(只写 `/tmp/sbx-test/`),不动系统状态。
- C2. 主路线**必须零外部二进制依赖**(泛化硬约束)。
- C3. spike 只定路线与设计要点,不写生产代码。

## 非目标

- P3b 实施本身(另立项)。
- 网络白名单 / egress 代理(v1 只做断网)。
- WSL1 专项支持(落入 fail-open 分支即可)。
- read 限制(Codex 同款开放项)。

## 结论(TL;DR)

**主路线 Landlock + seccomp(纯 Rust,Codex 同款)**:微软 WSL2 内核机群均匀自带
Landlock/seccomp/userns,零用户态依赖,泛化问题结构性解决;interop 用 EXECUTE
拒绝面收口,比 bwrap 的 `/init` 盖章更干净。**bwrap 降为可选增强档**(探测到 +
用户显式启用)。**firejail / 自研 overlayfs / microVM 不做**(判据见 prior-art 篇)。
Ubuntu 24.04 的 AppArmor userns 限制在 WSL2 不存在(微软内核无 apparmor)。

## Acceptance Criteria

- [x] AC1. R1 bwrap 矩阵通过(bwrap 篇 §1)。
- [x] AC2. R2 interop 逃逸发现 + 两路线收口配方实测(bwrap 篇 §2 / landlock 篇 §3)。
- [x] AC3. R3 Landlock 矩阵通过 + 五条陷阱(landlock 篇 §3/§4)。
- [x] AC4. R4 泛化性分类与探测阶梯(generalization 篇)。
- [x] AC5. R5 业界评审与否决判据(prior-art 篇)。
- [x] AC6. R6 探针与日志留档(artifacts/,含 README 与复跑方式)。
- [x] AC7. P3b 设计要点移交件(p3b-design-notes 篇)。
