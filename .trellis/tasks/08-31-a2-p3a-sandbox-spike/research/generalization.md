# 调研 — 泛化性分析:哪些结论跨 WSL 机群成立

> 本篇回答立项评审时的核心质疑:「实测是不是只以本机为依据?项目要泛化。」
> 方法:把本机实测事实逐条分类——**内核侧**(微软统一分发,机群均匀)vs
> **用户态侧**(随发行版/用户操作漂移)。

## 1. WSL2 的内核分发模型(泛化论证的地基)

WSL2 的 Linux 内核由 **Windows 侧统一分发**(`wsl --update` / `Microsoft.WSL.Kernel`
包),**不随发行版走**——装 Ubuntu、Debian、Arch 得到的是同一个微软内核。
因此内核 config 层面的事实天然泛化:

- `CONFIG_SECURITY_LANDLOCK=y`(LSM 列表首位;微软[官方内核发布说明](https://learn.microsoft.com/en-us/windows/wsl/kernel-release-notes)
  亦明确列有 "Enable the Landlock LSM")
- `CONFIG_USER_NS=y`、`CONFIG_SECCOMP_FILTER=y`
- **无 apparmor**(LSM 列表里根本没有)

版本分布(2026-08 调研):本机 WSL 2.2.4 / kernel 5.15.153.1-2;WSL 2.3.0 起切
6.6 LTS(2.7.0 = 6.6.114,最新内核包到 6.18)。两个大版本都满足上述 config。
Landlock ABI 随内核走高(5.15 = v1,6.6 ≈ v3,6.7+ = v4 含 TCP 网络规则)——
P3b 按 ABI 探测降级,不要按版本号假设。

## 2. 逐条分类

### ✅ 能泛化(内核侧)

| 事实 | 依据 |
|---|---|
| Landlock/seccomp/userns 可用 | 微软内核 config,机群均匀(§1) |
| Ubuntu 24.04「AppArmor 限制非特权 userns 弄坏 bwrap」**在 WSL2 不存在** | 该限制是 Ubuntu 补丁内核 + apparmor 的组合([bubblewrap#632](https://github.com/containers/bubblewrap/issues/632)、[Ubuntu 官方说明](https://discourse.ubuntu.com/t/understanding-apparmor-user-namespace-restriction/58007));微软内核无 apparmor,本机实证 userns 直接过 |
| WSL interop 逃逸存在 | binfmt `WSLInterop`/`WSLInterop-late` 是所有 WSL 安装的默认注册;两条收口配方(bwrap 盖 `/init` / Landlock EXECUTE 白名单不含)因此泛化 |
| NoNewPrivs + suid 封锁 | bwrap 与 Landlock 路线均实证,机制在内核 |

### ❌ 不能泛化(用户态侧)

| 事实 | 漂移方向 | P3b 对策 |
|---|---|---|
| 「bwrap 已装」是假象 | `apt why` 零反向依赖 = 手动装;fresh WSL 没有 | bwrap 只做可选档,启动探测 |
| 工具链路径(fnm `/run/user/<uid>/...`、linuxbrew 系统位、`~/.cargo`) | 每机每 session 不同(见 bwrap 篇 F 测试的 node 路径) | exec 允许面/缓存可写面 = **探测 + settings 可配置**,daemon 侧解析 |
| `/dev/kvm` | 依赖 Win11 + BIOS 虚拟化 + 嵌套直通;本机存在但**用户不在 kvm 组**(连本机都要 root 干预) | microVM 路线否决依据之一(另两条见 prior-art 篇) |
| 发行版 UAPI 头版本 | 22.04 无 APPEND 位(landlock 篇陷阱 1) | Rust 裸常量 / crate 按 ABI 降级 |

## 3. 能力探测阶梯(P3b 的泛化落地形态)

```
daemon 启动:
  probe landlock_create_ruleset(VERSION) ─┬─ ≥v1 → Landlock FS 规则可用
  probe seccomp(PR_SET_SECCOMP 基础过滤) ─┼─ 可用 → 断网过滤器可用
  (可选)which bwrap ─────────────────────┼─ 存在 → 增强档可启用(默认关)
                                          └─ 任一失败 → fail-open(现状路径)+ 设置面横幅
```

- **fail-open 而非 fail-closed**:沙盒是限损层不是安全边界(判定层/审计/Tier 仍在),
  探测失败静默回退现状行为,但要在设置里可见,禁止悬挂(参照 Codex
  [#1039](https://github.com/openai/codex/issues/1039) 的报错体验教训)。
- WSL1(无真内核)天然落在 fail-open 分支,不做专项支持。

## 4. 对「业界方案是否也要看泛化」的答复

是——prior-art 篇按「部署面假设」重审了各家:Codex(Landlock+seccomp,零依赖,泛化面
与我们同构)、Claude Code(bwrap,依赖存在性同我们路线一)、microVM 族(KVM 命中率
+ 文件系统视图错位)。业界两大 harness 都把「不依赖用户装了什么」当成硬约束,
与本文结论互证。
