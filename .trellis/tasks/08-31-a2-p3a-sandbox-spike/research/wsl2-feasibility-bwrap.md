# 调研 — 路线一:bwrap(namespace 级)WSL2 实测

> 探针:`../artifacts/bwrap_matrix.sh`;原始输出:`../artifacts/logs/bwrap-matrix.log`。
> 环境:`../artifacts/logs/env-snapshot.txt`(bwrap 0.6.1,kernel 5.15.153.1-2,Ubuntu 22.04)。

## 配方

```bash
bwrap --unshare-net --unshare-pid --die-with-parent \
  --ro-bind / / --proc /proc --dev /dev --tmpfs /tmp \
  --bind <项目> <项目> \
  --ro-bind /dev/null /init        # interop 收口,见 §2
```

语义:整根只读、项目与 /tmp 可写、无独立网络栈、独立 pid ns;`--die-with-parent`
防孤儿沙盒。bwrap 0.6.1 **无 `--overlay` 选项**(copy-on-write 沙盒层做不了)。

## 1. 矩阵结果(A-R)

| # | 探测 | 结果 |
|---|---|---|
| A | 沙盒内基本执行 | ✅ |
| B | `--unshare-net` 出网(`> /dev/tcp/1.1.1.1/443`) | ✅ NET-BLOCKED |
| C | 项目目录写入 | ✅ |
| D | 项目外写入(`/usr/local/bin/escape.txt`) | ✅ `Read-only file system` |
| E | 家目录写入 | ✅ 拦截 |
| F | 工具链可见性 | ✅ git / cargo(linuxbrew)/ node(fnm)/ python3 全解析 |
| G | `NoNewPrivs` | ✅ =1 |
| H | `sudo` 提权 | ✅ 失败(`/etc/sudo.conf is owned by uid 65534`,userns uid 映射所致) |
| P | **`--ro-bind /dev/null /init` 后 `.exe` 直执** | ✅ **`Permission denied`(收口)** |
| R | 盖 `/init` 后回归 | ✅ git/python3 正常、项目写正常、断网保持 |

注意 F 的 node 路径是 `/run/user/1000/fnm_multishells/<pid-tick>/bin/node`——
**当前会话的 fnm multishell 路径**,换 session/机器即失效。P3b 若走 bwrap 档,
PATH 解析必须发生在沙盒外、由 daemon 解析后按结果路径挂规则,不能信沙盒内 PATH。

## 2. WSL interop 逃逸(本 spike 最重要的单点发现)

**现象**:沙盒内执行 `.exe` 照常运行、以 Windows 用户身份输出(`whoami.exe` →
`carlos\kaijin`)。`--clearenv` 拦不住(I/J 两测)。文件系统隔离只约束 Linux 侧,
Windows 侧完全绕开——「损害限制在沙盒可写路径内」的承诺对 WSL 不成立,除非收口。

**根因链**:WSL 默认在 `/proc/sys/fs/binfmt_misc` 注册 `WSLInterop` / `WSLInterop-late`
(K 实测),内核 exec 一个 PE 文件时经 binfmt 走解释器 `/init`,由它连 interop socket
完成 Windows 侧进程拉起。env(`WSL_INTEROP`)不是必要条件。

**尝试过的无效/受限手段**:

| 手段 | 结果 | 原因 |
|---|---|---|
| `--clearenv` | ❌ 无效 | socket 发现不依赖 env(J 实测) |
| tmpfs 盖 `/proc/sys/fs/binfmt_misc` | ❌ 无效 | binfmt 注册表在内核侧(按超级块),遮目录不注销 handler(L 实测) |
| 沙盒内挂全新空 binfmt_misc 实例 | ❌ `must be superuser` | 此内核(5.15 MS)不允许非特权挂 binfmt(O 实测) |
| 显式 `/init <exe>` 直调 | 未下结论 | 输出为空,不作证据(M;已从矩阵移除) |

**有效配方**:`--ro-bind /dev/null /init` —— 解释器变成不可读的 /dev/null,exec 链
在内核解析解释器阶段即失败(P 实测 `Permission denied`)。

**残余面(诚实记录)**:interop 的裸 unix socket 协议若被直接实现(不经过 `/init`),
理论上仍可拉起 Windows 进程。稳妥的纵深是再盖 socket 文件(`$WSL_INTEROP` 指向的
路径)+ `--clearenv`。P3b 若启用 bwrap 档,应三件全上;Landlock 路线用 EXECUTE 拒绝面
覆盖(见 landlock 篇)。

## 3. 评估

- 可行性:✅ 全矩阵通过,含 interop 收口。
- 泛化性:❌ **bwrap 不预装**——本机 `apt why bubblewrap` 零反向依赖 = 手动装的;
  全新 WSL Ubuntu 第一步就断。分发故事(apt 依赖 / 静态二进制 vendor,LGPL)都要额外成本。
- 隔离强度:namespace 级(独立 pid/mount/net ns),强于 Landlock(无 ns)。
- 结论:**降级为可选增强档**——探测到 bwrap 且用户显式启用时提供更强隔离;不是依赖。
