# 调研 — 路线二:Landlock + seccomp(内核 LSM 级)WSL2 实测 ⭐ 主路线

> 探针:`../artifacts/ll_sbx.c`(~90 行 C);原始输出:`../artifacts/logs/landlock-matrix.log`。
> 环境快照:`../artifacts/logs/env-snapshot.txt`。
> 内核文档:[docs.kernel.org/userspace-api/landlock](https://docs.kernel.org/userspace-api/landlock.html)。

## 1. 为什么是它

- **零外部二进制依赖**:Landlock 是内核 LSM、seccomp 是内核 syscall 过滤器,
  两者都在微软 WSL2 内核 config 里(`CONFIG_SECURITY_LANDLOCK=y` 居 LSM 列表首位、
  `CONFIG_SECCOMP_FILTER=y`,见环境快照)——机群均匀性等同内核本身,「全新 WSL 开箱即用」。
- **纯 Rust 可实现**:裸 syscall 或 `landlock` crate;daemon 本来就是 Rust,fork 后
  pre-exec 子进程施加,与 Codex 的 `linux-sandbox` 同构。
- 不可逆性:ruleset 一经 `restrict_self` 施加,对进程树**不可撤销**,沙盒内无法自行解除。

## 2. 探针设计(即 P3b 的规则集雏形)

- **handled 权限** = `EXECUTE` + 全部写族(WRITE_FILE / REMOVE_DIR / REMOVE_FILE / MAKE_*);
  **读不控**(READ_FILE/READ_DIR 不进 handled → 全局放行)。这是 Codex 同款取舍:
  控写不控读,显著降误杀;read 限制连 Codex 都是开放项([#7657](https://github.com/openai/codex/issues/7657))。
- **exec 允许面**(探针硬编码,P3b 要做成探测):`/usr /bin /sbin /lib /lib64 /dev /tmp
  <项目> /home/linuxbrew/.linuxbrew /run/user/1000`——**显式不含 `/init`、`/mnt/c`**。
- **可写面**:`<项目> + /tmp`。
- **设备节点 per-file 规则**:`/dev/null /dev/zero /dev/full /dev/random /dev/urandom
  /dev/tty` 各给 `WRITE_FILE`。
- 施加顺序:fork → 子进程 `PR_SET_NO_NEW_PRIVS` → `landlock_restrict_self` → exec。

## 3. 矩阵结果

| # | 探测 | 结果 |
|---|---|---|
| — | Landlock ABI | v1 运行时可用 |
| 1 | exec `/mnt/c/.../whoami.exe` | ✅ `Permission denied`(**interop 不需要 namespace 就能掐**) |
| 2 | exec `/init` | ✅ `Permission denied` |
| 3 | exec git / python3 | ✅ 正常(设备节点放行后;git 无参打 usage = exec 成功) |
| 4 | 写项目目录 | ✅ |
| 5 | 写家目录 | ✅ 拦截 |
| 6 | 读 `/etc/passwd` | ✅ 不受限 |

对照 bwrap 篇 §2:interop 收口在 Landlock 下是 EXECUTE 拒绝面的**自然推论**
(允许面是白名单制,`/init` 与 `/mnt/c` 不在名单内),不需要额外的盖章技巧。
残余面同 bwrap 篇:裸 socket 协议直连(不 exec 任何东西)不归 EXECUTE 管——
但 P3b 同时上 seccomp 拦 socket 后,AF_UNIX 里只剩 interop socket 一个高危面,
把 socket 文件路径加进 WRITE 豁免之外即可(读不控,socket 连接不算 FS write,
如实记录:该残余面靠 seccomp `connect` 过滤按路径匹配不了 unix socket,只能挡
已知 interop socket 的 path 白名单化——P3b 实施时按此设计,超出 spike 范围)。

## 4. 实测陷阱(P3b 写 Rust 版时必读,全部踩过)

1. **Ubuntu 22.04 UAPI 头是 ABI v1,没有 `LANDLOCK_ACCESS_FS_APPEND`**(查了两次才认)。
   v1 权限位:EXECUTE / WRITE_FILE / READ_FILE / READ_DIR / REMOVE_* / MAKE_*;
   `WRITE_FILE` 已覆盖追加写。Rust 侧用裸常量或新版 `landlock` crate 按 ABI 降级即可。
2. **rule 请求不在 handled 集合里的权限位 → EINVAL**。设备规则想顺手给 `READ_FILE`,
   但读不在 handled → 规则挂失败,报错样子还长得像「设备节点不支持规则」。
   规则的 access 必须是 `handled_access_fs` 的**子集**。
3. **设备节点必须 per-file 放行**:O_RDWR 打开 `/dev/null` 算 WRITE_FILE,不放行则
   `git` 第一步就死(`could not open '/dev/null' for reading and writing`)。
   Landlock 的 `parent_fd` 可以指向单个文件。
4. **`restrict_self` 前必须 `PR_SET_NO_NEW_PRIVS`**,否则 EACCES——顺带把 suid 位
   提权面也封死,双赚。
5. **探测代码要容忍不存在路径**:如 linuxbrew 装系统位 `/home/linuxbrew`(非 `~/.linuxbrew`);
   open(O_PATH) 失败应跳过并记日志,不能 abort。

## 5. 断网:seccomp 部分(未单独写探针,记设计)

- 本内核(5.15,ABI v1)**没有** Landlock 网络规则(TCP bind/connect 要 ABI v4 /
  内核 6.7+;WSL 新内核 6.6 也只到 ABI 3)——断网一律走 seccomp,不赌内核版本。
- 过滤器:拦 `socket(AF_INET, *)` 与 `socket(AF_INET6, *)` → EPERM;
  **AF_UNIX 放行**(Docker CLI / X11 / pnpm 走 unix socket 的工具不受伤)。
  DNS(UDP socket)随之死亡 = 预期行为。
- seccomp 是古老且普适的内核特性(`CONFIG_SECCOMP_FILTER=y` 机群均匀),可行性无悬念,
  故未写独立探针;P3b 用手写 BPF 常量或 `seccompiler`,**不引 libseccomp C 依赖**。

## 6. 评估

- 可行性:✅ 全矩阵通过,interop 收口比 bwrap 路线更干净。
- 泛化性:✅ 内核 config 自带,零用户态依赖——**结构性解决泛化问题**。
- 隔离强度:LSM 级(无独立 ns:看得见全部进程、全局挂载表只读视角)——弱于 bwrap,
  但对「判定错了也限损」的目标(限制 FS 写 + 网络出)已足。
- 结论:**主路线**。bwrap 作为可选增强档并存(探测到 + 用户显式启用)。
