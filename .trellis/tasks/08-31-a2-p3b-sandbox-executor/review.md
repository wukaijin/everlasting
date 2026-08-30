# 评审 — 08-31-a2-p3b-sandbox-executor(A2+ P3b 执行期沙盒)

> 评审日期:2026-08-31
> 评审对象:task.json / prd.md / design.md / implement.md / check.jsonl / implement.jsonl
> 评审方式:三件套通读 + 代码实勘核对(spike 移交件、spec 引用、锚点 file:line 全部过核)

## 结论:**有条件通过(带阻塞项)**

三件套整体质量高——前提、范围、约束、spike 依据、PR 切分、测试策略都清晰且相互咬合,spike 五条陷阱被正确内化为 design 约束(C5 类型系统保证权限⊆handled、design §2.3 pre_exec 信号安全)。但发现 **2 个阻塞项(会让 PR1/PR2 无法按 design 落地)** 和 **3 个重要缺口**,建议修完再 start。

---

## 阻塞项(修完才能 start)

### 🔴 B1. `maybe_apply` 触发条件的三重与里,`mode` 在两条 spawn 路径上都拿不到

design §2.2 的触发条件是 `classify_prefix(cmd)==ReadOnly && mode≠Yolo && sandbox_enabled`。实勘发现:

- **前台 `shell`**(`tools/shell.rs:336 execute`):签名是 `execute(input, ctx, session_id, cancel)`,而 `ToolContext`(tools/mod.rs:386)**没有 mode 字段**。`session_mode` 存在于 `chat_loop.rs` 的 dispatch 层(chat_loop/tools.rs:64),但没传入 `ToolContext`。
- **后台 `run_background_shell`**:`BackgroundShellRegistry::start()`(background_shell/mod.rs:261)签名只有 `(session_id, command, cwd, max_runtime_ms)`,同样拿不到 mode;`background_shell/in_memory.rs:237` 实际 spawn 点连 classify 判定上下文都没有。

design 里"shell.rs / background_shell 在 spawn 前做三重与"这一句没有对应的数据流支撑。三条解法(需在 design 里落一条):

1. `ToolContext` 加 `mode` 字段(dispatch 层灌入),spawn 前查询;
2. 判定前置到 dispatch 层、结果(是否沙盒)随 tool 调用传下去;
3. 后台路径放宽为"只判 classify==ReadOnly && config && capability,不看 mode"——但 Yolo 放行语义就丢了,与 PRD R4/AC3 冲突。

### 🔴 B2. spike 明确要求处理的 interop socket 残余面,PRD/design 只字未提

spike landlock 篇 §3 原文:"AF_UNIX 里只剩 interop socket 一个高危面……该残余面靠 seccomp `connect` 过滤按路径匹配不了 unix socket,只能挡已知 interop socket 的 path 白名单化——**P3b 实施时按此设计**"。设计里只有"AF_UNIX 放行"(§2.4),把 spike 留给 P3b 的那条路径判断静默丢掉了。要么把它收进 v1(seccomp 对已知 interop socket path 的 connect 拦截),要么在非目标里明确"interop socket 残余面留待 P3c"并说明理由——不能既不写也不做。

---

## 重要缺口(强烈建议修)

### 🟠 W1. `sandbox_extra_writable` 缺写入通道,PR3 前端列表编辑落不了地

design §2.6 说 `sandbox_extra_writable` 经 `get_app_config` 读、PR3 前端列表编辑。实勘发现现有写入口 `set_app_config_flag`(commands/config.rs:542)是**布尔专用**——`SETTABLE_APP_FLAGS` 白名单 + 扁平标量,写字符串数组需要新的写命令/通道。design 没提。需补一个 `set_app_config` 或列表专用的写路径(daemon route + Tauri 双端)。

### 🟠 W2. seccomp BPF 注入方式与 pre_exec 的约束不符

design §2.4 说 BPF 在"父进程构造",§2.3 说闭包"纯 syscall"。但 seccomp 过滤器的实际安装是 `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &bpf_prog)`——注意 `struct sock_fprog` 指针会在 `prctl` 调用瞬间被内核**复制**,因此"闭包里直接用父进程构造好的 BPF 字节"是成立的、无需 malloc。design 的表述(闭包内纯 syscall 但 BPF 字节怎么带进去)需要说清:闭包内只有 prctl,BPF 是编译期/父进程常量数组。当前写法会让实现者困惑,建议在 §2.3 明确"sock_fprog 引用父进程构造的静态数组,闭包内仅一个 prctl"。

### 🟠 W3. 拦截指引(§2.5)的判定输入 `mode` 同样缺失

"exit≠0 且 stderr 命中且**本命令已沙盒**"的后验逻辑,依赖与 B1 相同的 mode/沙盒状态查询。B1 解决后这里自然可通,但 design 应显式说明指引判定复用同一状态源,否则实现时容易在 tool 侧再埋一遍查询。

---

## 通过项(核对属实)

- **代码锚点全部真实**:shell.rs:393 spawn、in_memory.rs:260 spawn、tool_output.rs:62 `session_outputs_dir`、shell_trust.rs:394 `classify_prefix`、audit.rs:34 AuditKind、commands/config.rs:491 AppConfigPayload——全部存在且与 design 描述一致。
- **五条陷阱与设计对齐**:`ll_sbx.c`(控制 EXECUTE+写位、设备 per-file、exec 白名单)与 design §2.1 规则集一一对应;陷阱 2 以 C5 类型系统约束收口;陷阱 5 探测容忍缺路径在 §2.1 有对应。
- **CVE-2025-59532 铁律出处真实**(prior-art.md),且在 design §2.1 以"来源铁律"落地。
- **AuditKind 序列化确认无迁移**:`as_str()`(audit.rs:208)序列化为小写字符串,DB 列存字符串,追加变体零迁移——design §5 的"零迁移"论断成立。
- **配置读写对与 PR3 可复用**:`get_app_config` / `set_app_config_flag` / `SETTABLE_APP_FLAGS` 白名单 + GeneralTab 开关先例都在,布尔开关 `sandbox_enabled` 完全可走既有通道(W1 只影响数组字段)。
- **fail-open 语义出处扎实**:generalization.md §3 阶梯与 Codex #1039 教训,PRD R5/AC5 一致;CI runner(24.04,内核 6.8+)Landlock v1 必在,implement.md 风险表对 CI 断网测试的预案(AC2 放宽为连接失败即可)合理。
- **spec manifests 引用全部存在**,且选材对口(permission-layer 管判定层零改动、tool-contract 管 shell 契约零回归、project-cwd-boundary 管敏感路径、daemon-server 管双 transport additive)。
- **ROADMAP P3 行**在第四档,A2+ P3 状态记载完整,PR3 的"移档"有明确落点。

---

## 其他小建议(非阻塞)

1. **PR3 的 `pnpm test` 数字口径**:implement.md 写 vitest ≈1486,AGENTS.md 只给了后端 ≈2135+ 的预估。前端数字建议以实际为准,别在完成门写死。
2. **AC8 的 turn-smoke 验证点**:implement.md 说"turn_trace 里审计行存在"——`SandboxedShellExecution` 是新增 kind,turn-smoke.sh 的断言脚本若不认识会静默忽略,建议 PR3 顺手在脚本里加该 kind 的计数校验。
3. **design §2.3 的 fd 数量**:PreparedSandbox 持有 ruleset fd + 每路径一个 fd(可写根 3 + exec 面 ~10 + 设备 6 ≈ 20 个),闭包内逐个 `landlock_add_rule` 后 close。实现时建议按 spike 探针先例,一个路径一次 add_rule,失败即 abort spawn(与 ll_sbx.c 的 `_exit(99)` 语义对齐)。

---

## 建议动作

把 B1 的数据流方案(推荐 `ToolContext` 加 `mode` 字段,dispatch 层灌入,两条 spawn 路径共用)、B2 的 interop socket 残余面决策、W1 的数组写通道补进 design.md,更新 PRD 决策点表(可加 D4 对应 B2),再 start。PR 切分本身不用动。
