# 调研 — 业界先例与替代方案评估

> 评审视角:每个方案问三件事——隔离原语依赖什么、**和文件工具共享文件系统视图吗**、
> 部署面假设是否与本项目(个人 WSL2 daemon、非特权、毫秒级 shell tool)相容。

## 1. OpenAI Codex(Landlock + seccomp)——主路线的作业对象

[linux-sandbox crate](https://github.com/openai/codex/blob/main/codex-rs/linux-sandbox/README.md)
([第三方分析](https://agent-safehouse.dev/docs/agent-investigations/codex)、
[Willison 评述](https://simonwillison.net/2025/Nov/9/codex-sandbox-investigation/)):

- **架构**:独立 `codex-linux-sandbox` 可执行包装器,父进程 spawn 命令经它施加
  Landlock(文件系统限写)+ seccomp(拦 socket 断网)后 exec。Rust 实现,随 CLI 分发,
  零外部二进制依赖。
- **模式**:read-only(全盘只读)/ workspace-write(cwd + tmp 可写,断网)/
  danger-full-access(不沙盒)。
- **读取我们的两个教训**:
  1. [CVE-2025-59532](https://advisories.gitlab.com/npm/@openai/codex/CVE-2025-59532/):
     可写根取了**模型给的 cwd**,LLM 可诱导把可写根指到攻击者路径 → 绕过。
     P3b 铁律:可写根一律服务端解析(worktree / spill 路径来自 daemon 状态),
     永不采信 tool call 参数里的路径。
  2. [#1039](https://github.com/openai/codex/issues/1039):环境不支持 seccomp/landlock
     组合时报错体验差(悬挂/难排查)→ P3b 能力探测必须 fail-open + 明确横幅
     (generalization 篇 §3)。
- **已知开放项**:read 限制没做([#7657](https://github.com/openai/codex/issues/7657))——
  与我们「读不控」的取舍一致。

## 2. Claude Code(bwrap + egress 代理)——对照与交互闭环参照

[官方 sandboxing 文档](https://code.claude.com/docs/en/sandboxing)、
开源 [sandbox-runtime (srt)](https://github.com/anthropic-experimental/sandbox-runtime):

- **架构**:Linux/WSL2 用 bwrap(`--die-with-parent --unshare-pid` 等实测参数,
  [issue#35986](https://github.com/anthropics/claude-code/issues/35986) 有实锤命令行);
  文件系统「默认拒绝、例外允许」,工作树可写 + 白名单目录。
- **网络**:不用内核包过滤——`--unshare-net` 掏空网络栈,再经 socat 桥到**沙盒外的
  本地 egress 代理**,按 `allowedDomains` 白名单放行(空列表 = 无网)。
- **已知弱点**(我们不抄的部分):DNS 经代理放行,存在 DNS 隧道外传的实锤研究;
  代理 hostname 匹配曾被构造串绕过。我们 v1 直接断网(AF_INET/6 全拦),
  不做白名单代理——无场景、且有前车之鉴。
- **抄的部分**:「沙盒内命令因限制失败 → 升级为询问用户」的交互闭环。
- 隔离强度确实高(独立 pid/mount/net ns)——这正是我们保留 bwrap 可选档的理由。

## 3. microVM 族(CubeSandbox / Zeroboot / microsandbox)——评估后不做

| | [CubeSandbox](https://github.com/tencentcloud/CubeSandbox)(腾讯云,Apache 2.0) | [Zeroboot](https://github.com/zerobootdev/zeroboot) |
|---|---|---|
| 原语 | RustVMM + KVM microVM,<60ms 冷启,控制面/数据面(FastAPI),E2B API 兼容 | Firecracker 快照 + CoW 内存 fork,声称 0.79ms 起沙盒 |
| 定位 | 云端多租户、大规模 agent 部署 | 跑**提交进来的**不可信代码片段(REPL 式),Python/TS SDK |

**三条否决判据**(对任何 microVM 方案通用,含 [microsandbox](https://github.com/tizkovatereza/awesome-ai-sandboxes) 族):

1. **文件系统视图错位(致命)**:agent 的 `edit_file`/`read_file`/git 集成在宿主 FS
   操作,shell 必须立即看到同一 worktree。microVM 独立内核 + 独立 rootfs——要么
   virtiofs/9p 共享(Firecracker 支持差、一致性坑),要么整个 agent core 搬进 VM
   (架构重写)。Codex/Claude Code 选 OS 级而非 VM,核心原因即此。
2. **工具链迁移**:每机工具链位置不同(fnm/linuxbrew/cargo),VM 内重装或网络挂载;
   Landlock 零迁移。
3. **KVM 命中率**:本机 `/dev/kvm` 存在(AMD svm 嵌套直通恰好开着)但用户不在 kvm
   组——连本机都要 root 干预;机群上依赖 Win11 + BIOS + 嵌套直通,命中率与 Landlock
   (内核 config 自带)不是一个量级。CubeSandbox 还带控制面服务,对单用户 daemon 是重运维。

**保留价值**(诚实记录,不是客套):
- 谱系完整:microVM(硬件级)→ bwrap(namespace 级)→ Landlock/seccomp(LSM 级)
  三档叙事,P3 选最低可用档、保留升级路径,取舍有据。
- 若未来出现「执行真正不可信的东西」(来路不明脚本 / 高危供应链命令如
  `npm install` 野包),正确隔离级别就是 microVM——届时回头看 Zeroboot 这类;
  那是新场景另立项,不是 P3 限损层。
- E2B API 是事实标准(CubeSandbox 都兼容它);若未来接云端沙盒跑重活,接口对齐 E2B。

## 4. firejail / 自研 overlayfs(源方案候选,快速否决)

- firejail:本机未装;setuid 路线为主,审计面大,近年 CVE 不断;无 WSL 特化。
- 自研 overlayfs:重造 bwrap;且 overlay 挂载对非特权 userns 的支持随内核版本漂移,
  泛化性不如 Landlock。均不取。

## 5. 综述索引(读材)

- [六路沙盒方案对比](https://addozhang.medium.com/ai-agent-code-execution-sandboxes-isolation-from-containers-to-microvms-e80848effea5)
- [Northflank: How to sandbox AI agents](https://northflank.com/blog/how-to-sandbox-ai-agents)
- [Manveer Chawla: 沙盒指南](https://manveerc.substack.com/p/ai-agent-sandboxing-guide)
- [awesome-ai-sandboxes](https://github.com/tizkovatereza/awesome-ai-sandboxes)
