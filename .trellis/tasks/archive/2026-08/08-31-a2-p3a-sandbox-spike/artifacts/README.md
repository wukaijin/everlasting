# artifacts/ — spike 探针与原始证据

> 全部探测**非破坏性**:只写 `/tmp/sbx-test/`,不碰系统状态。
> 复现环境要求见 `logs/env-snapshot.txt`;矩阵结论解读见 `../research/`。

## 文件清单

| 文件 | 用途 | 复跑方式 |
|---|---|---|
| `ll_abi_probe.c` | Landlock ABI 探测(调 `landlock_create_ruleset(NULL,0,VERSION)`) | `cc ll_abi_probe.c -o /tmp/ll_abi && /tmp/ll_abi` |
| `ll_sbx.c` | Landlock 沙盒探针(~90 行):fork 子进程施加 ruleset + `PR_SET_NO_NEW_PRIVS` 后 exec/写/读目标 | `cc ll_sbx.c -o /tmp/ll_sbx && /tmp/ll_sbx exec /usr/bin/git`(动作 = exec/write/read + 路径) |
| `bwrap_matrix.sh` | bwrap 路线全矩阵复现脚本(A-R,含 interop 逃逸与收口对照) | `./bwrap_matrix.sh`(需 bwrap 已装;全新 WSL 没有) |
| `logs/env-snapshot.txt` | 实测环境快照:内核/config/LSM/WSL 版本/bwrap 版本//dev/kvm 与 kvm 组 | — |
| `logs/bwrap-matrix.log` | bwrap 矩阵原始输出(2026-08-31 02:2x 复跑) | — |
| `logs/landlock-matrix.log` | Landlock 矩阵原始输出 | — |

## 探针已知局限(解读日志前必读)

- `ll_sbx` 的 `exec` 动作只传 `argv[2]`、不带附加参数:目标命令无参运行(git 打 usage 退 1、
  python3 进 REPL 读 EOF 退 0)——**都证明 exec 成功**,exit 码是命令自身的。
- 跑矩阵时探针要 `</dev/null`,否则子进程继承循环的 heredoc stdin(python3 会把后续测试行
  当源码吃掉)。
- `ll_sbx.c` 里 exec 允许面/可写面是**探针硬编码**(含本机的 linuxbrew/fnm 路径),换机器
  复跑需按 `logs/env-snapshot.txt` 对照调整——这正是 P3b 要做成「探测 + 可配置」的原因。
- M 测试(显式 `/init` 当解释器)输出为空、未下结论,不作为证据引用;interop 收口以
  bwrap-P 与 Landlock-exec:/init 两项为准。
