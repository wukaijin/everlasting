# PRD — A2+ P3a 执行期沙盒 spike:WSL2 可行性与主路线定案

> 源方案:[docs/_history/2026-08-28-a2-shell-classification.md](../../../docs/_history/2026-08-28-a2-shell-classification.md) §4.3 阶段 3(P3 执行期沙盒兜底)。
> ROADMAP 条目:第四档 A2+ P3,原标注「前置 WSL userns spike」——本任务即该 spike,已完成。
> 全部实测于 2026-08-31,本机(见 §2 环境);探针源码存 `artifacts/`。

## 背景与问题

A2+ P1+P2 判定层(复合命令拆分 + 写重定向检测)2026-07-04 已落地,但静态判定有固有盲区:
变量展开 / `$()` / `eval` / alias / 间接副作用(`curl /delete`)。P3 的定位是判定层**之下**
的执行期限损层——判定错了也限损。源方案给的候选是 bubblewrap / firejail / 自研 overlayfs,
且明确「WSL2 下 bubblewrap 的 userns 可用性需先验证」。

本 spike 要回答三个问题:

1. 各候选路线在 WSL2 部署面上是否可行(spike 本体);
2. 哪些结论能**泛化到任意一台全新 WSL 安装**(不依赖本机恰好装了什么);
3. 主路线定案 + P3b 的设计要点。

## TL;DR 结论

**主路线:Landlock + seccomp 进程内沙盒(纯 Rust,零外部二进制依赖)**,Codex 同款。
bwrap 降级为可选增强档(探测到才用,非依赖);firejail / 自研 overlayfs / microVM 均不做。
两条路线的实测矩阵全绿,WSL interop 逃逸(`.exe` 借 binfmt 拉起)在两条路线下都有已验证的收口配方。

## 1. 实测环境(2026-08-31 本机)

| 项 | 值 | 泛化性 |
|---|---|---|
| WSL 版本 | WSL 2.2.4.0,WSLg 1.0.61 | ✅ 机群参考(新版已切 6.6 LTS 内核,见 §5) |
| 内核 | `5.15.153.1-2-microsoft-standard-WSL2` | ✅ 微软统一分发,不走发行版 |
| 发行版 | Ubuntu 22.04.5,systemd PID1 | ❌ 用户态,机群各异 |
| 内核 config | `CONFIG_SECURITY_LANDLOCK=y`(LSM 列表首位)、`USER_NS=y`、`SECCOMP_FILTER=y`、无 apparmor | ✅ 微软内核侧,机群均匀 |
| Landlock 运行时 | ABI v1 可用(`landlock_create_ruleset` VERSION 探测) | ✅ 同上 |
| bwrap | 0.6.1 已装,但 `apt why` 零反向依赖 = 手动装的 | ❌ **全新 WSL 不会有** |
| /dev/kvm | 存在(AMD svm 嵌套虚拟化),但当前用户**不在 kvm 组** | ❌ 依赖 Win11 + BIOS + 嵌套直通 + root 改组,命中率不均匀 |

## 2. 路线一:bwrap(namespace 级)实测矩阵

配方:`bwrap --unshare-net --unshare-pid --die-with-parent --ro-bind / / --proc /proc --dev /dev --tmpfs /tmp --bind <项目> <项目> [--ro-bind /dev/null /init]`

| # | 探测 | 结果 |
|---|---|---|
| A | 沙盒内基本执行 | ✅ |
| B | `--unshare-net` 出网 | ✅ 拦截 |
| C | 项目目录写入 | ✅ |
| D | 项目外写入(`/usr/local/bin`) | ✅ `Read-only file system` |
| E | 家目录写入 | ✅ 拦截 |
| F | 工具链可见性(git/cargo/node/python3) | ✅ 全解析 |
| G | `NoNewPrivs` | ✅ =1(suid 提权面被内核挡住,`sudo` 失败) |
| I/J | **WSL interop 逃逸**:`.exe` 直执 | ❌ **照常执行,以 Windows 用户身份**;`--clearenv` 拦不住 |
| L | tmpfs 盖 `/proc/sys/fs/binfmt_misc` | ❌ 无效(handler 注册在内核侧,遮目录不注销) |
| O | 挂全新空 binfmt_misc 实例 | ❌ `must be superuser`(此内核不允许非特权挂 binfmt) |
| **P** | **`--ro-bind /dev/null /init` 盖住解释器** | ✅ **interop 立即 `Permission denied`** |
| R | 盖 `/init` 后回归 | ✅ git/python3 正常、项目写正常、断网保持 |

结论:可行,但依赖 bwrap 存在(分发故事未解)且收 interop 必须叠 `/init` 盖章配方。
另:bwrap 0.6.1 无 `--overlay` 选项。

## 3. 路线二:Landlock + seccomp(内核 LSM 级)实测矩阵 ⭐ 主路线

探针:`artifacts/ll_sbx.c`(C,~90 行)。设计:handled 权限 = EXECUTE + 全部写族
(**读不控**,Codex 同款降误杀);fork 后子进程施加 `PR_SET_NO_NEW_PRIVS` +
`landlock_restrict_self`,再 exec 目标命令。规则对子进程树不可逆继承。

| # | 探测 | 结果 |
|---|---|---|
| 1 | exec `/mnt/c/.../whoami.exe` | ✅ `Permission denied`(**interop 不需要 namespace 就能掐**) |
| 2 | exec `/init` | ✅ `Permission denied` |
| 3 | exec git / python3 | ✅ 正常(设备节点放行后,见陷阱 3) |
| 4 | 写项目目录 | ✅ |
| 5 | 写家目录 | ✅ 拦截 |
| 6 | 读 `/etc/passwd` | ✅ 不受限 |
| — | exec 允许面 | `/usr /bin /sbin /lib /lib64 /dev /tmp <项目> /home/linuxbrew /run/user/1000`;**显式不含 `/init`、`/mnt/c`** |
| — | 设备节点 | `/dev/null /dev/zero /dev/full /dev/random /dev/urandom /dev/tty` per-file `WRITE_FILE` |

**实测陷阱(写 P3b 时必读)**:

1. Ubuntu 22.04 的 UAPI 头是 ABI v1,**没有 `LANDLOCK_ACCESS_FS_APPEND` 位**(记错两次);
   `WRITE_FILE` 已覆盖追加写。
2. rule 里请求**不在 handled 集合里的权限位 → EINVAL**。设备规则想给 `READ_FILE`,
   但读不在 handled → 全部规则挂失败,报错还长得很像「设备节点不支持」。规则权限必须是
   handled_access_fs 的子集。
3. 不放行设备节点写,`git` 第一步就死:`could not open '/dev/null'`——O_RDWR 打开
   设备节点算 WRITE_FILE。Landlock 支持对单个文件加规则,per-file 放行即可。
4. 可写目录探测时注意 linuxbrew 装在 `/home/linuxbrew`(系统位)而非 `~/.linuxbrew`,
   探测代码要容忍不存在路径。
5. `restrict_self` 前必须 `PR_SET_NO_NEW_PRIVS`,否则 EACCES(顺带把 suid 面也封了,双赚)。

## 4. 泛化性分析(核心增量:哪些事实跨机群成立)

**能泛化(内核侧,微软统一分发)**:

- WSL2 内核不随发行版走。本机 5.15;WSL 2.3.0+ 已切 6.6 LTS(2.7.0 = 6.6.114,内核包
  最新到 6.18)。微软[官方内核发布说明](https://learn.microsoft.com/en-us/windows/wsl/kernel-release-notes)
  明确写了 "Enable the Landlock LSM"——**整个 WSL2 机群上 Landlock/USER_NS/SECCOMP_FILTER 均匀存在**。
- Ubuntu 24.04 著名的「AppArmor 限制非特权 userns 弄坏 bwrap」
  ([bubblewrap#632](https://github.com/containers/bubblewrap/issues/632)、
  [Ubuntu 官方说明](https://discourse.ubuntu.com/t/understanding-apparmor-user-namespace-restriction/58007))
  **在 WSL2 不存在**——微软内核 CONFIG_LSM 无 apparmor(本机已验证)。
- WSL interop 逃逸是所有 WSL 安装的默认行为(binfmt `WSLInterop`/`WSLInterop-late`),
  非本机特例;§2/§3 两个收口配方因此泛化。

**不能泛化(用户态侧)**:

- 「bwrap 已装」是假象(手动装的);fresh WSL Ubuntu 没有 bwrap,路线一在别的机器第一步就断。
- 工具链路径每机不同(fnm 在 `/run/user/...`、linuxbrew 系统位、`~/.cargo` 等)——
  exec 允许面/可写缓存面必须**探测 + 可配置**,不能写死。
- `/dev/kvm` 本机存在但用户不在 kvm 组;机群上还依赖 Win11 + BIOS 虚拟化 + 嵌套直通。
  KVM 系(microVM)路线命中率与 Landlock 不是一个量级。

## 5. 业界调研

| 方案 | 做法 | 对本项目的启示 |
|---|---|---|
| [OpenAI Codex](https://github.com/openai/codex/blob/main/codex-rs/linux-sandbox/README.md)([分析](https://agent-safehouse.dev/docs/agent-investigations/codex)、[Willison 评述](https://simonwillison.net/2025/Nov/9/codex-sandbox-investigation/)) | **纯 Rust Landlock(限写)+ seccomp(拦 socket 断网)**,独立 wrapper 可执行;三档 read-only / workspace-write / danger-full-access | 主路线直接抄作业的对象。[CVE-2025-59532](https://advisories.gitlab.com/npm/@openai/codex/CVE-2025-59532/):**可写根取了模型给的 cwd 导致绕过**——可写根必须服务端自定。[#1039](https://github.com/openai/codex/issues/1039):环境探测失败要有明确报错与降级,不能悬挂 |
| [Claude Code](https://code.claude.com/docs/en/sandboxing)([sandbox-runtime](https://github.com/anthropic-experimental/sandbox-runtime)) | bwrap + `--unshare-net`;网络白名单走本地 egress 代理 + socat 桥 | 隔离更重(独立 pid/mount ns)但依赖 bwrap;代理方案有 DNS 外传实锤。参照其「沙盒失败→升级询问」交互闭环 |
| [CubeSandbox](https://github.com/tencentcloud/CubeSandbox)(腾讯云,Apache 2.0) | RustVMM+KVM microVM,<60ms 冷启,控制面/数据面,E2B API 兼容 | 云端多租户定位;见 §6 不采纳判据。E2B API 是事实标准,未来接云端沙盒可对齐 |
| [Zeroboot](https://github.com/zerobootdev/zeroboot) | Firecracker 快照 + CoW 内存 fork,声称 0.79ms 起沙盒 | 「跑提交进来的不可信代码」模型,非「宿主目录跑任意 shell」;见 §6 |

## 6. 替代方案评估:microVM(CubeSandbox / Zeroboot / microsandbox 族)为什么不做

1. **文件系统视图错位(致命)**:agent 的 `edit_file`/`read_file`/git 集成在宿主 FS 上操作,
   shell 必须立即看到同一 worktree。microVM 独立内核 + 独立 rootfs,要么 virtiofs 共享
   (Firecracker 支持差、一致性坑),要么整个 agent core 搬进 VM(架构重写)。Codex /
   Claude Code 选 OS 级而非 VM,核心原因即此。
2. **工具链迁移**:每机工具链位置不同,VM 内重装或网络挂载;Landlock 零迁移。
3. **KVM 命中率**:本机有 `/dev/kvm` 但用户不在 kvm 组(连本机都要 root 干预);
   机群依赖 Win11 + BIOS + 嵌套直通。且 CubeSandbox 是带 FastAPI 控制面的**服务**,
   对单用户个人 daemon 是重运维。

保留价值:若未来出现「执行真正不可信的东西」(来路不明脚本 / 高危供应链命令),
microVM 是正确的隔离级别,届时回头看 Zeroboot 这类方案——那是新场景,不是 P3 限损层。

## 7. P3b 设计要点(从本 spike 直接继承)

- **主路线**:daemon(Rust)fork 后 pre-exec 子进程施加 Landlock ruleset + seccomp BPF;
  `PR_SET_NO_NEW_PRIVS` 先行。零外部二进制,随 daemon 分发,泛化问题结构性解决。
- **handled 权限**:EXECUTE + 写族(WRITE_FILE / REMOVE_* / MAKE_*);读不控。
- **可写根**:worktree + `/tmp` + `app_data_dir/outputs/<session>`(spill);**一律服务端
  解析,拒绝 LLM 提供的 cwd/路径**(CVE-2025-59532 教训)。
- **exec 允许面**:PATH 目录 ∪ /dev ∪ /tmp ∪ worktree ∪ 探测到的工具链目录;**显式不含
  `/init`、`/mnt/c`**(interop 收口)。设备节点 per-file WRITE_FILE(§3 陷阱 3)。
- **断网**:seccomp 拦 `socket(AF_INET/AF_INET6)`,**AF_UNIX 放行**(Docker CLI/X11 不受伤);
  DNS 随之死亡 = 预期行为。Landlock 网络规则要 ABI v4(内核 6.7+),本内核面用不上,不等。
- **缓存目录 allowlist**:`~/.cargo`、pnpm store、`~/.cache` 等默认探测 + settings 可配置;
  命令因写缓存 EACCES 失败 → 升级为 Ask(参照 Codex 的 sandbox-fail→escalate 闭环)。
- **能力探测**:daemon 启动探测 `landlock_create_ruleset(VERSION)` + seccomp;不可用
  (WSL1 / 异常内核)→ **fail-open** + 设置面可见横幅。禁止悬挂(参照 Codex #1039)。
- **Tier 集成**:判定层(⑨ classify)不动;**ReadOnly 档 shell 进沙盒免弹窗**;
  Plan 模式全命令沙盒化的三态 UX 属 P3c,P3b 落地后拿真实体验再设计。
- **bwrap 可选档**:探测到 bwrap 且用户显式启用 → pid/mount ns 级更强隔离(配方 §2,
  含 `/init` 盖章);非依赖。
- **实现选型**:Landlock 走 `landlock` crate 或裸 syscall;seccomp 手写 BPF 常量或
  `seccompiler`,**不引 libseccomp C 依赖**。
- **回归要点**:沙盒内写项目外路径应失败;`.exe` 与 `/init` 应拒;git 在项目内正常
  (含 .git 写);cargo build 走 allowlist 成功、清空 allowlist 后 EACCES→Ask;
  断网下 `curl`/`git fetch` 失败;AF_UNIX(pnpm store)不受影响。

## 非目标

- P3b 实施本身(另行立项,PRD 评审时定)。
- 网络白名单 / egress 代理(Claude Code 式)——v1 只做断网,白名单无场景。
- WSL1 支持(探测 + fail-open 即可)。
- read 限制(Codex 至今也没做,[#7657](https://github.com/openai/codex/issues/7657) 同款开放项)。

## Acceptance Criteria

- [x] AC1. bwrap 路线全矩阵实测通过,含 interop 逃逸发现与 `/init` 盖章收口配方(§2)。
- [x] AC2. Landlock 路线全矩阵实测通过,含 EXECUTE 位收口 interop 与三条实施陷阱(§3)。
- [x] AC3. 泛化性分析:内核侧/用户态侧事实分类,机群均匀性结论(§4)。
- [x] AC4. 业界方案调研 + microVM 替代方案三判据评估(§5/§6)。
- [x] AC5. 主路线定案(Landlock+seccomp,bwrap 可选档)+ P3b 设计要点成文(§7)。

## 参考

- [Landlock 内核文档](https://docs.kernel.org/userspace-api/landlock.html)
- [WSL 内核发布说明(Landlock LSM)](https://learn.microsoft.com/en-us/windows/wsl/kernel-release-notes)
- [Phoronix: WSL 2.7.0 / 6.6 LTS](https://www.phoronix.com/news/Microsoft-WSL-2-7-0)
- [沙盒方案六路对比](https://addozhang.medium.com/ai-agent-code-execution-sandboxes-isolation-from-containers-to-microvms-e80848effea5) · [Northflank 综述](https://northflank.com/blog/how-to-sandbox-ai-agents)
- 探针源码:`artifacts/ll_abi_probe.c`(ABI 探测)、`artifacts/ll_sbx.c`(沙盒矩阵)
