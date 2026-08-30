# Design — A2+ P3b 执行期沙盒执行器

> 依据:P3a spike [p3b-design-notes](../08-31-a2-p3a-sandbox-spike/research/p3b-design-notes.md)
> + 代码实勘(2026-08-31,file:line 均已核对)。

## 1. 边界与落点

```
agent loop ──> permissions::check(Tier 4,零改动)
                    │ classify_prefix → Decision           [agent/permissions/check.rs]
                    ▼
              tool dispatch(execute_tool)
                    │
        ┌───────────┴────────────┐
        ▼                        ▼
 tools/shell.rs:393        background_shell(start → spawn)
 Command::new("sh")        crate::background_shell
        │                        │
        └───────────┬────────────┘
                    ▼
      sandbox::maybe_apply(&mut cmd, &SandboxSpec)   ← 本任务新增,唯一侵入点
                    │ (pre_exec: prctl + landlock + seccomp)
                    ▼
                 spawn(现状管线:apply_safe_env / process_group(0) /
                 管道预取排空 / 超时 / tool_output 截断 —— 全部不动)
```

**新增模块 `src/sandbox/`**(Linux 门控 `#[cfg(target_os = "linux")]`,非 Linux 编译
为 no-op `maybe_apply` 空实现,满足 C4):

| 文件 | 职责 |
|---|---|
| `mod.rs` | 公共 API:`SandboxSpec` / `maybe_apply` / `Capability::probe()`(OnceLock 缓存) |
| `landlock.rs` | ABI 常量(v1 子集,自写不引 crate)、`RulesetBuilder`(类型保证权限⊆handled)、施加 |
| `seccomp.rs` | 手写 BPF 程序字节(父进程构造)、施加 |
| `policy.rs` | 从 ctx 计算 `SandboxSpec`(可写根 / exec 面 / 设备清单) |
| `tests_sandbox.rs` | 单测 + 真实 spawn 集成测试(参照 `tests_shell.rs` 先例) |

## 2. 核心契约

### 2.1 SandboxSpec(父进程计算,纯数据)

```rust
pub struct SandboxSpec {
    writable_roots: Vec<PathBuf>,   // session cwd + /tmp + session_outputs_dir(data_dir, sid)
    exec_allow_roots: Vec<PathBuf>, // PATH 目录 + /dev + /tmp + writable_roots + 工具链探测目录
    extra_writable: Vec<PathBuf>,   // config sandbox_extra_writable(默认 ~/.cargo)
    // 设备清单固定常量:/dev/null zero full random urandom tty(per-file WRITE_FILE)
}
```

- **来源铁律**:cwd 取 session 的 validated cwd(worktree 路径由服务端状态给出),
  spill 用 `tools/tool_output.rs:62 session_outputs_dir`;**任何 tool 参数里的路径
  不得进入本结构**(CVE-2025-59532)。
- PATH 解析在父进程完成(`std::env::var("PATH")` split + realpath),产物是目录列表;
  沙盒内不依赖 PATH 语义。
- 工具链探测目录:`~/.cargo/bin`、`/home/linuxbrew/.linuxbrew`(存在才加,
  open 失败跳过——spike 陷阱 5)。

### 2.2 触发判定(R4)与 mode 数据流(评审 B1 修正)

**问题**:两条 spawn 路径现状都拿不到 mode——前台 `tools/shell.rs:336 execute` 的
`ToolContext`(tools/mod.rs:386)无 mode 字段;后台 `BackgroundShellRegistry::start`
(background_shell/mod.rs:261)签名只有 `(session_id, command, cwd, max_runtime_ms)`。
而 `session_mode` 只存在于 dispatch 层(`agent/chat_loop/tools.rs` 的 `DispatchCtx`)。

**解法(评审建议 1 + 参数下传,两条路径共用)**:

1. `ToolContext` 加 `mode` 字段,dispatch 层从 `DispatchCtx.session_mode` 灌入
   (第 9 个字段,构造点单一)——前台 `shell.rs` 直接读。
2. 后台路径:`Registry::start` trait 签名追加 `sandbox: Option<SandboxSpec>` 参数,
   由 `run_background_shell` **tool 层**(有 ctx.mode)算好下传;registry 内部
   spawn 点(in_memory.rs)只消费,不自行判定。trait 加参,全部 impl 同步改。
3. `maybe_apply(cmd, spec_opt)`:三重与 = `classify_prefix(cmd) == ReadOnly` **且**
   mode ≠ Yolo **且** `sandbox_enabled`(config)且 `Capability::probe().ok()`。
   任一不满足 → 不设 pre_exec,返回 `Applied::No { reason }`(reason 进 tracing,
   不发审计)。
4. §2.5 的拦截指引判定复用同一状态源:tool 层记 `applied: bool` 本地变量,
   后验时不再二次查询(评审 W3)。

`classify_prefix` 是纯函数(`agent/permissions/shell_trust.rs:394`),tool 侧重调一次
无副作用、与 Tier 4 判定同输入同结果——**判定层零改动**(C2)。

### 2.3 pre_exec:async-signal-safety(spike 之外的新硬约束)

`Command::pre_exec` 闭包运行在 fork 后、exec 前的**单线程信号上下文**,不得 malloc /
open / 持锁。因此施加分两段:

- **父进程(安全区)**:`RulesetBuilder` 完成 `landlock_create_ruleset`、逐路径
  `open(O_PATH | O_CLOEXEC)` 拿 raw fd、BPF 程序字节构造。产物 = `PreparedSandbox
  { ruleset_fd: RawFd, path_fds: Vec<RawFd>, bpf: Vec<sock_filter> }`。
- **pre_exec 闭包(纯 syscall)**:`prctl(PR_SET_NO_NEW_PRIVS)` → 逐 fd
  `landlock_add_rule`(PATH_BENEATH,权限位为编译期常量,天然 ⊆ handled——陷阱 2
  由构造层消灭;单条失败即整列中止,对齐 spike 探针 `_exit(99)` 语义)→
  `landlock_restrict_self` → seccomp 装载(见下)→ `close` 全部 fd。任何一步失败
  return Err(spawn 失败,走既有 `Failed to spawn command` 出口 + 追加 `[sandbox]`
  前缀定位)。
- **seccomp 装载细节(评审 W2 澄清)**:`prctl(PR_SET_SECCOMP, FILTER, prog)` 的
  `sock_fprog` 指向**父进程构造好的 BPF 字节数组**——内核在 prctl 瞬间复制过滤器,
  引用父进程内存无 malloc;闭包内仅在**栈上**构造 `sock_fprog { len, filter: 指针 }`
  结构 + 发一次 prctl。fd 量级:ruleset 1 + 路径 ~20(可写根 3 + exec 面 ~10 +
  设备 6),闭包内逐个 add_rule 后统一 close。

`PreparedSandbox` 生命周期 = 单次 spawn;fd 用后即关,不跨命令复用(ruleset fd
restrict 后不可复用,语义即如此)。

### 2.4 seccomp 过滤器

手写 BPF(约 6 条指令):`socket` syscall 且 `args[0] ∈ {AF_INET=2, AF_INET6=10}` →
`RET_ERRNO | EPERM`;其余 `RET_ALLOW`。**默认放行**(无 default kill)——过滤器只做
断网一件事,误杀面最小。AF_UNIX/DNS 语义见 spike landlock 篇 §5。

### 2.5 拦截指引(R7)

命令正常 spawn 后,写被拒表现为命令自身退出非零 + stderr(如 `Permission denied`)。
tool 侧后验:exit ≠ 0 且 stderr 命中 `Permission denied|Read-only file system` 且本命令
**已沙盒** → 在 tool 输出尾部追加一行指引(单测钉死文案要点:「写入被沙盒拦截;
如需在该路径写入,请向用户申请放行(非 ReadOnly 命令)或让用户调整
sandbox_extra_writable」)。启发式,宁缺勿滥——只追加,不改写命令原生输出。

### 2.6 审计与配置

- `AuditKind` 追加 `SandboxedShellExecution`(D2),由 tool 侧经既有
  `record_*_audit` 写手落一行(payload:cmd 哈希前缀 + ruleset 摘要,不存全命令)。
- `commands/config.rs AppConfigPayload` 加 `sandbox_enabled: bool`(默认 true,D1)、
  `sandbox_extra_writable: Vec<String>`(默认 `[]`,读取时并入 `~/.cargo` 默认项);
  `get_app_config` 双 transport additive 返回(C3)。
- **写通道(评审 W1)**:既有 `set_app_config_flag` 是布尔专用
  (`SETTABLE_APP_FLAGS` 白名单 + `value: bool`,config.rs:542),数组字段需新增
  `set_app_config_list` 命令(key 白名单同款模式,daemon route + Tauri 双端同步);
  归 PR3。布尔开关 `sandbox_enabled` 直接进 `SETTABLE_APP_FLAGS` 既有通道。

## 3. 数据流(单命令生命周期)

```
LLM tool_use(shell, cmd)
  → Tier 4 check(不变;ReadOnly → 静默 Allow)
  → shell.rs execute
      1. 参数校验 / cwd 校验(不变)
      2. sandbox::spec_for(ctx) → Option<SandboxSpec>(父进程,安全区)
      3. classify_prefix(cmd) == ReadOnly && !yolo && config && capability
           → prepare(spec) → cmd.pre_exec(纯 syscall 闭包)
      4. spawn → 既有管线(env/PGID/管道预取/超时/截断)
      5. 退出后:后验指引(§2.5)+ 审计一行(§2.6)
```

worker(L3)与群聊 / 定时任务(F2)走同一 `execute_tool` 汇合点,**自动继承**,
无专项代码。

## 4. 权衡记录

- **不采用独立 wrapper 可执行**(Codex `codex-linux-sandbox` 形态):daemon 内
  fork+pre_exec 更简、零分发物;代价是 §2.3 的信号安全纪律,以「父进程全准备 +
  闭包纯 syscall」内化。
- **不引 `landlock` crate / `seccompiler`**(C1):ABI v1 子集常量 ~30 行,依赖
  面换来的便利有限;自写常量按内核 UAPI 头逐一对齐并以单测钉死数值。
- **seccomp 默认放行**:Codex 同款思路的更保守版——我们过滤器只拦 socket 两族,
  不做 default-deny syscall 面,限损交给 Landlock,syscall 面不掺和。
- **审计不存全命令**:审计已有 shell 执行行,本 kind 只记 ruleset 摘要,避免重复
  敏感面。
- **interop socket 残余面 v1 接受(评审 B2,PRD D4)**:spike landlock 篇留给 P3b 的
  「已知 interop socket path 白名单化」经设计复核**不可行于 v1**——seccomp BPF 只能
  检查标量参数,`connect(fd, sockaddr*)` 的路径在指针背后,过滤器无法按路径匹配;
  Landlock ABI v1 也没有 connect 类权限位。残余面 = 绕过 `/init` 直接实现 interop
  线协议直连 socket——攻击成本高(需逆向协议),且容易路径(exec `/init`、`.exe`)
  已被 EXECUTE 拒绝面 + NoNewPrivs 封死。**完整收口归 P3c bwrap 增强档**
  (tmpfs 盖 socket 路径,namespace 路线可行为);v1 在 spec 与审计指引中如实记录
  该残余面。

## 5. 兼容与回滚

- wire:additive(`get_app_config` 新字段);旧前端忽略新字段无影响。
- schema:零迁移(AuditKind 是 Rust enum 序列化为字符串,追加变体不触发 DB 变更)。
- 回滚:一级 = `sandbox_enabled=false`(配置层,行为回到现状,spawn 不设 pre_exec);
  二级 = git revert 单个 PR。PR 切分保证每个 PR 独立可回滚(见 implement.md)。
- 平台:`#[cfg(target_os = "linux")]`;macOS/Windows 编译 no-op(本产品 daemon
  部署面是 Linux/WSL2,非 Linux 走 fail-open 语义)。

## 6. 测试策略

- 单测:spec 计算(路径来源铁律:tool 参数注入不进去——构造攻击用例)、
  BPF 字节 golden、ABI 常量对齐、权限位⊆handled 的类型测试。
- 集成(`tests_sandbox.rs`,Linux-only):真实 spawn `sh -c`,覆盖 AC1/2/3/4/6 的
  行为断言(写拒/放行、interop 拒、断网、SideEffect 不沙盒、开关行为)。
- 探测桩:capability 注入失败 → fail-open(AC5)。
- live:`scripts/turn-smoke.sh` 真实 LLM 轮, ReadOnly 工具命令(ls/git diff/cat)
  无误杀(AC8)。
