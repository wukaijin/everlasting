# Sandbox Executor Spec — 执行期沙盒(P3b 2026-08-31 + P3c 2026-09-01)

> 任务:P3b `.trellis/tasks/08-31-a2-p3b-sandbox-executor/`(三件套 + review 处置记录);
> P3c `.trellis/tasks/09-01-a2-p3c-sandbox-ux/`(三态 / Plan / 升级闭环,四 PR)。
> 上游依据:P3a spike `08-31-a2-p3a-sandbox-spike/research/`(wsl2-feasibility-landlock
> 五条陷阱 / p3b-design-notes / generalization fail-open 阶梯)+ prior-art(CVE-2025-59532)。
> 模块:`app/src-tauri/src/sandbox/`(mod / landlock / seccomp / policy / tests_sandbox)、
> `agent/permissions/escalation.rs`(P3c 升级闭环)。
> 消费方:`tools/shell.rs`(前台)、`background_shell`(registry 只消费)、
> `agent/permissions/check/permission.rs`(Tier 4 面短路)、`agent/chat_loop/drive.rs`
> (Plan tool list)、`commands/config.rs` + `daemon/routes/config.rs`(设置面读写)、
> `commands/projects.rs` + `daemon/routes/projects.rs`(P3c 项目档位写)。

## 1. 定位与不变量

- 沙盒是**判定层之下的限损层**:判定层(`shell_trust::classify_prefix` 三档 +
  `permissions::check` 5-Tier)语义零改动(C2)。即使判定错了,损害被限制在
  「可写面(见 §3)+ 其余只读、无出网(socket AF_INET/AF_INET6 → EPERM)、
  exec 不到 `/init` 与 `/mnt/c`」之内。
- **触发(P3c `resolve_policy`,design §1 单一真源)**:P3b 的四项与(ReadOnly 档)
  已废弃;现在 `classify_prefix` **不参与触发** —— 沙盒档下**全命令**进沙盒,
  判定层只服务 `off` 档的经典路径。求值顺序(capability → Yolo → 项目 off →
  kill-switch → Plan → 项目面)即惰性读序,勿重排 —— config 读(RULE-SBX-004)
  结构性落在 gate 通过后:
  1. `Capability::probe().ok()`(OnceLock 缓存,失败 → Off = fail-open);
  2. `mode != Yolo`(恒 Off,`ToolContext.mode`,`chat_loop::init` 单一构造点灌入);
  3. 项目档 `projects.sandbox_policy == 'off'` → Off(`read_project_sandbox_policy`
     经 `sessions.project_id` join projects 点查;**缺行(测试池/孤儿)→ Off**,
     未知值/DB 错 → warn + Off,fail-open);
  4. kill-switch `sandbox_enabled`(fail-open:仅字面 `"false"` 关;master,
     关 = 全局 Off 含只读档项目);
  5. `mode == Plan` → `Face(ReadOnly)`(session 级覆盖项目档;但项目 off 已在
     3 短路 → Plan + off = 回退工具过滤,**绝不落「Plan + 弹窗放行写」**);
  6. 否则 `Face(项目档)`。
- **Policy 消费两处、真源一处**(design §1.1):`resolve_session_policy(db, sid, mode)`
  被 (a) Tier 4 shell 分支头的**短路**(`Policy != Off` → 跳过 prefix-grant/三档
  分类/ask,直接 Allow + ToolAllowed 审计;短路点在 Tier 1–3 之后 —— kill list /
  敏感路径 / Plan 写工具硬拒不被取代)和 (b) spawn 侧 `decide` 各自调用(两次
  点查可接受,不跨层传 Decision)。
- 不变量:Yolo 恒不沙盒;kill-switch 关 = 不设 pre_exec(P3b 前逐字节一致);
  Tier 1–3 硬拒层不被任何档位取代 —— 沙盒只接管 shell 的审批层。

## 2. 规则集契约

- **来源铁律(CVE-2025-59532)**:`SandboxSpec` 只装服务端解析的路径 ——
  session worktree(`ctx.worktree_path`,boundary 校验过)、`/tmp`、
  `tool_output::session_outputs_dir(data_dir, sid)`(spill)、config
  `sandbox_extra_writable`(`~` 经 `boundary::resolve_path` 展开)。
  **tool 参数(command / working_directory)没有任何路径进入该结构的 API 面**。
- **exec 允许面** = PATH 目录(父进程解析,**过滤 `/mnt/` 前缀** — WSL 把
  Windows 盘挂载进 PATH,不过滤 = exec 面静默重开 interop 逃逸;worktree 在
  /mnt/c 下的用户经可写根条款仍可 exec 自己的项目,**有意例外**)
  ∪ **`/lib` `/lib64` `/usr/lib` 静态根**(动态链接 ELF 解释器由内核在 execve
  时打开、需要 EXECUTE;正常 PATH 不含 lib 目录,漏掉 = 所有动态二进制 EACCES。
  此为实现期发现的 design 缺口,spike 探针硬编码了它们)∪ `/dev` `/tmp`
  ∪ 可写根 ∪ 工具链探测目录(`~/.cargo/bin`、`/home/linuxbrew/.linuxbrew`)。
  **显式不含 `/init`、`/mnt/c`(interop 收口 = EXECUTE 拒绝面的自然推论)。**
- **设备节点 per-file `WRITE_FILE`**(`DEVICE_WRITE_PATHS` 固定常量):
  `/dev/null /dev/zero /dev/full /dev/random /dev/urandom /dev/tty` —
  O_RDWR 打开 `/dev/null` 算 WRITE_FILE,不放行则 git 第一步就死(spike 陷阱 3)。
- **handled 权限** = EXECUTE + 全写族(WRITE_FILE / REMOVE_* / MAKE_*);读不控。
- 规则合并:同一路径多条 allow 在 `RulesetBuilder` 里按位或合并成一条(不依赖
  内核对同 path 重复 add_rule 的 union 语义);fd 数 = 唯一路径数 ≈ 20-40。
- **权限⊆handled 由类型系统保证(C5 / 陷阱 2)**:`landlock::AccessSet` 无
  raw-u64 构造器,只有 `EXECUTE` / `WRITE_FAMILY` / `WRITE_FILE` 三个常量,
  全是 `HANDLED_ACCESS_FS` 的子集 — 内核 EINVAL(报错长得像「设备不支持规则」)
  从构造层面不可能发生。

## 3. pre_exec 信号安全纪律(design §2.3,评审逐行核对点)

`Command::pre_exec` 闭包运行在 fork 后、exec 前的单线程信号上下文:**不得
malloc / open / 持锁**。落法 = 两段:

1. **父进程安全区**(`sandbox::prepare`):`landlock_create_ruleset` 拿
   ruleset fd、逐路径 `open(O_PATH|O_CLOEXEC)`、BPF 程序构造(CString/Vec
   分配都在这里)。产物 `PreparedSandbox { Arc<PreparedData> }`;
   `Drop` 在 spawn 返回后由父进程统一 close(std 保证 spawn 返回时子进程
   已 exec 或已死,父侧 close 不会与子侧使用竞态)。
2. **pre_exec 闭包**(`pre_exec_apply`):只做 raw syscall —
   `prctl(PR_SET_NO_NEW_PRIVS)` → 逐条 `landlock_add_rule`(栈上构造
   attr;**单条失败即整列中止** → spawn Err,对齐 spike 探针 `_exit(99)`
   语义)→ `landlock_restrict_self` → `prctl(PR_SET_SECCOMP, MODE_FILTER)`
   (sock_fprog 在栈上,filter 指针指向父进程构造的字节数组;内核在 prctl
   瞬间复制,W2)→ Err。闭包通过 `Arc` 只读引用父进程内存,零分配。

**失败语义**:能力探测失败 = fail-open(现状行为);prepare/pre-exec 失败 =
fail-closed(spawn 失败,tool 输出 `[sandbox] Failed to …`,绝不半沙盒执行)。

## 4. seccomp 断网契约

- 手写 8 指令 cBPF(seccomp.rs):`socket(args[0] low32) ∈ {AF_INET=2,
  AF_INET6=10}` → `ERRNO|EPERM`;其余 `ALLOW`(**default-allow**,不做
  default-deny syscall 面 — 限损交给 Landlock)。
- 低 32 位比较 = 内核语义:kernel 把 args[0] 截为 **signed int** family 并
  范围检查,低位精确匹配之外的情况内核本来就起不了对应 socket。
- AF_UNIX 放行(docker / pnpm / X11 类工具不受伤);**DNS 死亡 = 预期**
  (UDP socket 同被拦)。bash 文案:`socket: Operation not permitted`
  (EPERM,注意不是 `Connection refused` — 过滤器在 connect 之前就拦了)。
- 不赌内核版本:Landlock 网络规则要 ABI v4(6.7+),断网一律 seccomp。

## 5. fail-open 与可观测

- **探测**:`Capability::probe()` = landlock_create_ruleset(VERSION) ≥ 1 +
  `prctl(PR_GET_SECCOMP)` ≥ 0(只读探针,不装过滤器 — 装 allow-all 探针会
  顺带把 daemon 的 NoNewPrivs 永久置位,不做)。WSL1 / 老内核 / 非 Linux
  天然落 fail-open 分支。探测结果一行 info 日志(进程内仅首次)。
- **审计**:`AuditKind::SandboxedShellExecution`(wire `sandboxed_shell_execution`,
  追加变体零迁移),两条 spawn 路径的 **tool 侧**写(registry 无 DB 句柄),
  payload = `command_sha256_12`(哈希前缀,**不存全命令** — 全文已在
  `tool_executed`)+ `ruleset` 摘要(`SandboxSpec::summary()`,两路同形)+ tool_name。
- **设置面**:`get_app_config` additive 三字段 `sandboxEnabled` /
  `sandboxExtraWritable`(生效清单,含后端并入的 `~/.cargo` 默认项)/
  `sandboxCapability`(只读派生,不落盘)。写:`sandbox_enabled` 走
  `set_app_config_flag` 白名单;数组走新命令 `set_app_config_list`
  (`SETTABLE_APP_LISTS` 白名单同款防呆,daemon route + Tauri 双端)。
- **拦截指引**(R7/§2.5;P3c §5.3 参数化):已沙盒命令 exit≠0 且 stderr 命中
  特征 → tool 输出尾部追加一行 `sandbox::failure_guidance(stderr, mode)`
  (append-only,宁缺勿滥)。特征与文案分三路(`classify_block` 共享给升级
  触发):写(`Permission denied|Read-only file system`)× Edit/Plan、断网
  (`Operation not permitted`)× Edit/Plan —— Plan 文案明确「设计使然 +
  diff 提案 + /tmp 逃生口 + 无审批卡」,断网文案独立不再混入写指引。
  判定复用本轮 `decide` 结果(W3),不二次查询。

## 6. 已知陷阱(全踩过,勿复现)

1. **distro UAPI 头不可信**:libc 0.2 无 `PR_*`/landlock 常量(gnu target 连
   `prctl` 函数都没有),全部自写 + 单测钉死数值(`abi_*` 测试);ABI v1 到
   MAKE_SYM 为止,勿引入 APPEND 等 v6 位。
2. **rule access ⊄ handled → EINVAL** — 用 `AccessSet` 类型消掉(§2)。
3. **设备必须 per-file 放行**(§2 设备清单)。
4. **restrict_self 前必须 NoNewPrivs,否则 EACCES** — 闭包第一步固定是它。
5. **规则路径 open 失败要跳过并留日志**,不能 abort(临时 fnm 目录、可选
   设备都会失踪);例外:**可写根/worktree 失败也会被跳过** — 若上游语义
   改成 fail-closed,须重新评估误杀面。
6. **exec 面漏 `/lib64` → 一切动态二进制 EACCES**(strace 定位:execve 报
   EACCES 而非 ENOENT = 规则面缺东西;PATH 齐全仍 EACCES 时先查 lib 根)。
7. **WSL PATH 含 `/mnt/c/*`** — PATH 面必须过滤,否则与「不含 /mnt/c」铁律
   冲突(§2)。

## 7. Interop socket 残余面(v1 如实记录,评审 B2/D4)

AF_UNIX 放行后,残余面 = 绕过 `/init` 直接以原始线协议 connect interop unix
socket。v1 **不封**且无法用现有机制封:seccomp BPF 只能检查标量参数,
`connect(fd, sockaddr*)` 的路径在指针背后;Landlock ABI v1 无 connect 权限位。
易用逃逸路径(exec `/init`、`/mnt/c/**/*.exe`)已被 EXECUTE 拒绝面 + NoNewPrivs
封死;原始协议逆向成本高。完整收口 = P3c bwrap/namespace 档(tmpfs 盖 socket 路径)。

## 8. 测试锚点

- `sandbox::tests_sandbox`:ABI 常量钉死(`abi_*`)/ BPF golden + 迷你解释器
  逻辑走查(`bpf_*`)/ AccessSet ⊆ handled / spec 来源铁律(command 无法影响
  spec,编译层无此参数)/ resolve_policy 全矩阵(P3c §1)/ 真内核集成矩阵
  (写 allow/deny、/init 与 .exe 拒、/dev/tcp EPERM、AF_UNIX 过、git 流程,
  内核不支持时大声 SKIP)。
- `background_shell::in_memory::tests::sandboxed_background_shell_enforces_write_face`:
  AC6 后台路径同策略(None spec = 现状对照)。
- `commands::config::tests::set_list_*` + `daemon::routes::config::tests::
  set_app_config_list_route_*`:列表写通道 roundtrip / 白名单拒绝。
- live:`scripts/turn-smoke.sh --sandbox-probe`(AC8)— 真实 LLM 轮执行
  ReadOnly shell 命令,断言审计行存在且无误杀(不支持内核降级为 WARN)。
- P3c:`resolve_policy_full_matrix`(24 行矩阵)/ 面 spec 构造(ro 面
  worktree 出可写进 exec)/ `decide_sandboxes_all_tiers_under_readwrite`
  (SideEffect/Ask 档全进沙盒 = 触发面扩展)/ 真内核 ro 面集成
  (worktree 写拒 + 项目脚本 exec 过 + /tmp 写过;worktree 须放 $HOME
  面外 —— tempdir 在 /tmp 下会被 /tmp 可写根吞掉,断言无效)/
  `tests_check` 面短路三例(短路 Allow + ToolAllowed 审计、Tier 2 fork
  bomb 先序、off 档经典 ask)/ `tools/tests_escalation.rs` 四路
  (approve 单卡重跑 / deny 原失败+指引不落盘 / grant-hit 零卡 /
  复合不享 grant)+ 无句柄退化。`tests_common::worker_test_pool` 把
  backstop 项目钉 `off` = 经典路径测试锚(P3c 前 P3b 语义),面行为
  测试显式 re-seed 档位。

## 9. P3c — 面(Face)与只读变体

- `SandboxSpec.face ∈ {ReadWrite, ReadOnly}`,进 `summary()` 审计段
  (`face=rw|ro`)。**ReadWrite**(默认)= worktree + /tmp + spill + extras
  可写;**ReadOnly**(Plan / 项目 readonly 档)= worktree 移出可写根、
  **显式补进 exec 面**(项目脚本仍可运行 —— 旧实现靠 writable_roots extend
  间接获得,拆面后该 push 是 load-bearing)/tmp + spill + extras 两面均可写
  (/tmp = Plan 调查型构建逃生口,如 `CARGO_TARGET_DIR=/tmp/...`)。
- 配置载体:`projects.sandbox_policy TEXT NOT NULL DEFAULT 'readwrite'
  CHECK (in ('off','readwrite','readonly'))`。**默认 readwrite = 行为变更**
  (存量项目全命令进沙盒);回滚 = kill-switch 或单项目切 off。写通道
  `update_project_sandbox_policy`(daemon route + Tauri command,IPC 入口
  白名单先拒);读侧 ProjectRow 带出。

## 10. P3c — 升级闭环(前台 shell,design §5)

- **触发**(§5.1):`sandbox_applied ∧ exit≠0 ∧ mode≠Plan ∧ classify_block`
  命中(写串先行,`Operation not permitted` = 断网)。**每 tool call 至多
  一次**(重跑在结构上不再进升级分支);Plan 排除 = D3 确定性只读身份。
- **流程**(§5.2):prefix-grant 先查(`escalation::prefix_grant_hit`,
  `has_structural_metachar` 复合闸同 Tier 4)→ 命中直接不沙盒重跑零弹卡;
  未命中 → `EscalationHandle::ask` 复用 `ask_path` 弹卡(`reason_override`
  新参:拦截原因 + 原命令 + stderr 证据行)→ AllowOnce/AllowAlways(grant
  经 ask_path 既有通道落库,kind↔类别矩阵天然合法)→ **逐字节同
  command/env/cwd 重跑**(RULE-E-001/002 不变,仅无 pre_exec)/ Deny →
  原失败 + 模式感知指引。
- **注入**:EscalationHandle(sink+store+PermissionContext+db+token+
  tool_use_id)由 serial dispatch **仅对 shell** 灌入(shell 永不进并行批;
  后台壳维持模型介导);`Default`(None)= 测试路径 → 退化为指引。
- **双执行边界**(D4 接受):升级仅在面外写/断网被拒后触发 —— 危险部分
  第一遍未发生;重跑失败按普通失败返回。审计零新 kind:ask 侧既有 kinds +
  首个 `sandboxed_shell_execution` 行 + `tool_executed` 终态。
- worker 路径免费成立(ask_path 的 worker store keying / transcript-only
  审计原样复用)。
