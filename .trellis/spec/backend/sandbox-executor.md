# Sandbox Executor Spec — 执行期沙盒(P3b 2026-08-31 + P3c 2026-09-01 + P3d 2026-09-01)

> 任务:P3b `.trellis/tasks/08-31-a2-p3b-sandbox-executor/`(三件套 + review 处置记录);
> P3c `.trellis/tasks/09-01-a2-p3c-sandbox-ux/`(三态 / Plan / 升级闭环,四 PR);
> P3d `.trellis/tasks/09-01-a2-p3d-background-escalation/`(后台 shell 升级闭环)。
> 2026-09-21 listen 识别缺口临时修复 + 长期方案 roadmap(§12,会话内直改,未立任务)。
> 上游依据:P3a spike `08-31-a2-p3a-sandbox-spike/research/`(wsl2-feasibility-landlock
> 五条陷阱 / p3b-design-notes / generalization fail-open 阶梯)+ prior-art(CVE-2025-59532)。
> 模块:`app/src-tauri/src/sandbox/`(mod / landlock / seccomp / policy / tests_sandbox)、
> `agent/permissions/escalation.rs`(P3c 升级闭环原语 + P3d tool_name 参数化)、
> `agent/chat_loop/background_escalation.rs`(P3d 注入点闭环)。
> 消费方:`tools/shell.rs`(前台)、`background_shell`(registry 只消费;P3d 起
> 等待任务产 EscalationOffer)、`agent/permissions/check/permission.rs`(Tier 4
> 面短路)、`agent/chat_loop/drive.rs`(Plan tool list + P3d drain 接线)、
> `commands/config.rs` + `daemon/routes/config.rs`(设置面读写)、
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
  命中(写串先行,`Operation not permitted` = 断网;2026-09-21 起识别输入
  扩到 stdout,见 §12)。**每 tool call 至多
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

## 11. P3d — 后台 shell 升级闭环(下轮注入时,B 案,2026-09-01)

- **载荷生成**(registry 等待任务,拥有全量 stderr):`trigger == Normal ∧
  outcome == Failed ∧ 沙盒启动 ∧ origin_tool_use_id 有 ∧ classify_block`
  命中(2026-09-21 起识别输入扩到 stdout,证据行 stderr 优先、stdout
  兜底,见 §12)→ 通知带 `EscalationOffer { tool_use_id, block, stderr_evidence }`。
  Killed / TimedOut / SpawnFailed / Skip / 成功恒 `None`(超时 kill 的部分
  stderr 不可信,与前台 `!timed_out` 同门)。**start() 折叠**:`sandboxed`
  与 origin 在 start 时折叠成单一 `escalation_origin`——非沙盒 shell 的
  origin 是死重。origin 来源 = `ToolContext.tool_use_id`(P3d 新字段,
  dispatch 对**所有工具**统一盖章,仅 run_background_shell 消费;不选
  registry 事后注册——echo 类毫秒级完成,先 start 后注册有丢载竞争)。
- **解析时机 = 下轮 drain 之后、组装 turn_messages 之前**
  (`chat_loop/background_escalation.rs::resolve_all`;呈递方案用户裁定
  B 案,"完成即弹卡"的 A 案被否:detached ask 生命周期 / 失败通知抑制 /
  前端 120s 计时三座山的成本不值)。此时用户刚发消息必然在场,前台 120s
  超时原样复用;turn token 天然 cancel-safe,零 detached 生命周期。
- **流程**(复用前台 §5.2 原语,`ask`/`audit_grant_rerun` 已参数化
  tool_name=`run_background_shell`):Plan 门(当轮 mode,与前台同源;Plan
  中启动、下轮已切 Edit 的升级合法——审批卡即用户同意面)→
  `escalation_source()` 查重跑输入(entry 既有预留字段 command/cwd/
  max_runtime,inherent getter 不进 trait)→ prefix-grant 先查(grant
  命名空间跨 shell 族共享是有意语义,读侧 `IN ('shell',
  'run_background_shell')`)→ Ask 卡(挂**原调用卡**,前端按 toolUseId
  匹配)→ 批准 → registry `start(sandbox=None, origin=None)` 一次性不沙盒
  重跑(重跑结构性不再升级)。
- **注入文本契约**:每个通知恰好产一条终态文本——无 offer 走 legacy 格式
  (`plain_text` 钉死逐字节,AC5 回归锚);批准(ask/grant 两分支措辞
  区分)/ 拒绝 / 重跑 spawn 失败如实上报 / entry 已 sweep 降级 plain。
  LLM 永远只看到连贯故事,不在升级悬而未决时行动,也不自己发起重跑
  (审批绑定确切命令文本,D4 同源)。
- **前端唯一改动**(推翻 PRD 初稿"零改动"假设):ShellCard
  `isPendingApproval` 的前台 `!hasResult` 守卫对后台不适用——升级卡挂在
  **已有结果**的 run_background_shell 卡上;前台 ask 先于执行的
  hide-on-result 语义保留,后台卡按 `isBackground` 豁免(vitest 双向
  回归锚)。
- **测试基建 gotcha(P3d 新增)**:ask.rs 先 `emit_permission_ask` 后
  `register_ask`——mock resolver 首轮 resolve 可能合法落空
  (`resolve_ask` 返回 false),必须重试到成功,否则并行负载下 120s 超时
  赢竞态(tests_escalation 的 approve 例曾因此 flaky)。
- **环境边界(本仓库首例)**:`tests_escalation::escalation_approve_*`
  的探针前提是"OS 拒 `/proc/1/mem`",root 下不成立(rerun exit 0)——
  `geteuid()==0` 大声 SKIP,循内核探测 SKIP 同款纪律。

## 12. listen 场景识别缺口 — 临时修复(2026-09-21)与长期方案(未实施)

### 12.1 缺口实证(为什么修)

jjh-mono 项目 session `23a8184b`(2026-09-20,edit 模式):`vite` dev server
报 `Error: listen EPERM: operation not permitted 0.0.0.0:3001` —— 全文在
**stdout**,`stderr` 为空;python `socket.socket()` 创建被拒、Chrome CDP
端口被拒同样只在 stdout。而 §10/§11 的升级触发与 guidance 全部只喂
`classify_block(&stderr)` → 前后台识别整体哑火:该 session 29 次
`sandboxed_shell_execution`、**零 escalation offer**。模型被迫自行诊断
("怀疑 listen 被沙箱按进程上下文拦"),花了多轮 node/python/chrome 三路
探测后才绕行(CDP pipe 方案)。根因两层:

1. **检测面**:dev server 工具链(vite/webpack/go/python)把启动失败报
   stdout 是普遍行为,stderr 单源必然漏;
2. **文案面**:Network guidance 原文只讲 "outbound / no egress",即使
   触发也会诱导模型改绑 127.0.0.1(seccomp 拦的是 `socket(AF_INET)`
   创建,比 bind 更早,绑哪都死)——session 里实际发生的无效尝试。

### 12.2 已实施(临时修复)

- `classify_block(stderr, stdout)` 两参化:stderr 特征不变(Write 两串
  优先、Network 一串);stdout **只认三条强特征**(`stdout_smells_net_block`
  ,与 classify 同文件同真源):node `listen EPERM` / go `listen tcp`+ONP /
  python `PermissionError`+`socket`。宁缺勿滥锚:stdout 里裸
  `Operation not permitted` / `Permission denied` 不认(grep、cat 日志的
  常见内容);**Write 识别保持 stderr-only**。
- `failure_guidance(stderr, stdout, mode)` 跟随两参;Network 文案改写为
  "no INET sockets: no outbound AND no listen — dev servers cannot
  start",点破 listen 语义,消除改绑 loopback 的误导。
- 证据行:`escalation::stdout_net_evidence_line`(与 stderr 版同 200 截
  断);前台 ask 实参 stderr 空时回退 stdout 行(shell.rs,`Cow` 组合);
  后台 offer 烘焙同款兜底(in_memory.rs);`guidance_suffix` 双槽同行喂入。
- 测试锚:`tests_sandbox::classify_block_reads_stdout_for_listen_denials`
  (三实证形态命中 + 两个宁缺勿滥锚 + stderr-Write 优先)、
  `guidance_network_variant_names_listen`。全量 `--lib` 2536 绿。
- **2026-09-22 补修(09-21-durable-prefix-grant live E2E 实证)**:裸
  node 脚本 dev server 的 listen EPERM 打在 **stderr**(libuv errno 文案
  小写 `operation not permitted`),旧分类 stderr 只认大写 O 字面量 +
  强特征只喂 stdout → 整条升级链哑火(无卡无指引)。修复三件:stderr
  errno 字面量大小写不敏感;三条 listen 强特征改 `stream_smells_net_
  block` **两流都喂**(dev 工具链报哪条流是任意的);`stderr_evidence_
  line` MARKERS 大小写不敏感 + 补 listen 锚(否则卡上证据行取到
  "Node.js v24.15.0" 尾行)。宁缺勿滥方向不变:stdout 裸字面量(无论
  大小写)依旧不认。锚:`classify_block_reads_stderr_for_listen_
  denials`(live 形态逐字节入锚)。

### 12.3 长期方案(roadmap,未实施;按优先级)

**A. 项目级「网络放行」档 `readwrite_net`(首选)** — `sandbox_policy`
加第四值:landlock 文件锁照旧,seccomp INET filter 不装。动机:escalation
「批准一次重跑」对长驻 dev server 不友好(重启再批;后台重跑结构性
one-shot;AllowAlways 粒度 = 首 token,`pnpm dev` → 整个 `pnpm` 免沙箱)。
信任语义必须在 GUI 文案明示:文件写入仍受控,**网络 = 任意外联(数据
外发面)**,与 `off`(文件+网络全开)是两个不同的放行面。改动面:policy
枚举 + DB CHECK 迁移 + `resolve_policy` 分支 + `prepare()` 条件装 filter +
Settings 一档 + daemon routes。

**B. prefix-grant 项目级持久化(细粒度补充)** — 批准卡加「本项目记住」,
存 projects 维度。**前置**:grant 语义先从首 token 升级到多 token 前缀,
否则持久化 `pnpm` = 项目内所有 pnpm 命令(含 install 脚本)永久免沙箱;
免沙箱重跑连 landlock 一起免,信任面比 A 宽,只作 A 的补充。

> **[2026-09-22 已实施]** — task `09-21-durable-prefix-grant`(B 的完整
> 落地;契约正文见 §14)。要点:新表 `project_shell_grants`(PK
> `(project_id, worktree_key, prefix_tokens)`,tool_name 降溯源列,读侧
> 跨 shell 族共享);多 token 前缀(批准时全量 token ≤8,配对引号读写
> 对称剥离,首 token basename 归一);消费点 A = `decide` Face 分支内
> 免沙箱**启动**(Plan 不豁免,extra/net 读之前);B = 升级闭环扩查
> (生产不可达的写面不变量);C = off 档 Tier 4 免弹卡。写点内化在
> `ask_path` parent AllowAlways 臂(零新参,worker 只写内存 cache)。
> grant 闸沿 PR0 扩为 `grant_gate`(换行/单&/命令替换也拦)。

**C. landlock ABI v4 端口白名单(kernel 6.7+,长期)** —
`LANDLOCK_ACCESS_NET_BIND_TCP` 按端口放 bind、connect 仍拦,是唯一能做
「只放 listen 不放出网」的机制。**技术红线(勿绕)**:seccomp 永远做不了
端口级 —— 端口藏在 `sockaddr*` 指针里,BPF 不能解引用用户内存;现行
seccomp 拦的是 `socket()` 创建,连 bind 都没到。若上 C,网络执法点应整体
从 seccomp 迁到 landlock(socket 创建放行、bind/connect 按端口执法),
capability probe 渐进启用;当前 WSL 5.15 不满足,不做主路径(§4 "不赌
内核版本" 不变)。

> **[2026-09-21 已实施]** — task `09-21-sandbox-net-bindonly`(C 的完整
> 落地,含 B/D 的替代性收口;下节 §13 为契约正文)。要点:正交新列
> `projects.sandbox_net`(NULL=block=现状,parse fail-closed);BindOnly =
> 不装 seccomp、装 Landlock ABI v4 TCP 规则(bind=operator 快照口,
> connect=`{80,443}∪bind` 派生,双道钳位减除 daemon 监听口 7456);
> 执法器互斥由 `PreparedNet` 类型保证;probe 不支持(ABI<4)→ prepare
> 入口降级 block(warn + summary 记 degraded),绝不落 allow_all;
> `run_background_shell` 增 `ready_port` 就绪探测(daemon 侧观察 +
> `/proc` PGID 归因,四铁律);F1 exec 根 canonicalize + F3 exit-126
> exec 面缺口识别 + R9 归因合取项(网络归类前置「summary 证明装了
> INET block」)。红线语义不变:seccomp 仍不做端口级。

**D. 独立模型意图判断(远期)** — 引入独立(小)模型对失败命令做意图
分类:是否需要 listen/connect、是否构建工具 vs 数据外发,用于(a)特征
未命中/边界命中时的二次识别(静态字符串启发式的天花板就是 §12.2 的
宁缺勿滥——误报与漏报只能二选一),(b)批准卡上的推荐档位(如建议切
A 档)。**硬约束**:模型判断**不可作为安全边界** —— 命令文本与输出都是
可注入面(prompt injection),只能作为 UX 分层减少打扰;硬边界永远是
landlock/seccomp。接入点 = 升级触发前、特征 gray-zone 才调用(控成本);
判断结果入 `session_audit_events` 留痕。

## 13. NetPolicy — 网络维度三态与 BindOnly 执法(2026-09-21 起)

> task `09-21-sandbox-net-bindonly`;设计/证据:任务 design.md +
> research/(两场审议 + live probe)。§12.3 C 的落地契约正文。

### 13.1 数据模型与读路径

- `projects.sandbox_net TEXT`(NULL = block = 现状;**无 CHECK** —— 正交
  维度,parse 层 fail-closed 已够;存量 CHECK 不可扩域是走新列的动因)。
  序列化恰三形:`block` / `allow_all` / `bind_only:<p>,<p>`(裸
  `bind_only` 无效;端口 1..=65535、上限 32;`NetPolicy::parse` 单源)。
- **bind 快照 = 唯一 durable 授权出口**(R4):`project_net_snapshots`
  (PK `(project_id, worktree_key)`,key=worktree canonicalize 后绝对路径
  —— 换分支/重检出 = 新键,旧快照不随行;先例 preset_key 快照语义)。
  建议队列表 `project_net_proposals`(pending/confirmed/rejected)。
- **读路径** `policy::read_effective_net_policy(db, sid, worktree)`:列
  parse(失败 warn+block)→ BindOnly 时按 (project, worktree) 点查快照,
  **快照 ports 胜过列内联 ports**(真源),无快照行/坏行 → warn + block
  (「BindOnly 无从谈起」)。net 维度在 `decide()` 的项目面读取处伴随
  读出,**不是新 gate** —— resolve_policy/5-Tier 语义零改动。
- 写通道(daemon route + Tauri command 双端,§DAEMON-API 7):
  `set_project_sandbox_net`(仅 block/bind_only,**allow_all 无写入口**
  —— capability token 前置挂账)/ `propose_net_ports`(建议,永不直接
  生效)/ `confirm_net_snapshot`(**钳位在此拒绝**:
  `快照 ∩ {7456 ∪ EVERLASTING_DAEMON_PORT} = ∅`,命中整单 400 回显
  冲突口;成功 = 快照 REPLACE + 列置 bind_only + 建议标 confirmed)/
  `reject_net_proposal` / `get_project_net_state`(含
  `bind_only_supported` = probe)。

### 13.2 执法器三态(prepare/pre_exec)

- `PreparedNet` 互斥三态,**类型系统保证 seccomp 与 landlock-net 不同装**:
  `Block(Vec<sock_filter>)`(现行 8 指令程序,字节不变)/
  `BindOnly{bind, connect: Vec<NetPortAttr>}`(父进程预构 attr;ruleset
  以 `handled_access_net=BIND|CONNECT` + 16 字节 attr 创建;pre_exec 在
  文件规则后追加 NET_PORT add_rule,再 restrict_self,**不装 seccomp**)/
  `AllowAll`(单元变体,本期构造不出)。
- `RulesetAttr` 扩 `handled_access_net`(8 字节 v1 形 / 16 字节 v4 形按
  handled 集选择 —— Block 路径字节不变);`LANDLOCK_RULE_NET_PORT=2`、
  `NET_BIND_TCP=1<<13`、`NET_CONNECT_TCP=1<<14` 自写钉死(`abi_net_*`);
  `NetAccessSet` 对齐 AccessSet 的「权限⊆handled 类型保证」。
- **connect 派生集写死**:`{80,443} ∪ bind`(`NetPolicy::connect_ports`),
  零配置;bind 与 connect 双道**防御性钳位**减 daemon 口(第一道在
  confirm 写入时拒绝)。
- **能力门(R3)**:`Capability::probe()` 增 `landlock_net`(以「真建
  net-handling ruleset」探针,EINVAL=不支持;OnceLock 同款)。BindOnly +
  不支持 → **prepare 入口降级 Block**(warn;绝不 AllowAll)。判定与
  `summary()` 打印、classify 合取共用 `SandboxSpec::net_enforcement()`
  (一处真源三处读)。

### 13.3 summary/审计与 R9 归因合取

- `summary()` 增段:`net=block` / `net=bind_only(3000,3001)` /
  `net=allow_all` + 执法器标记(`seccomp:inet_block` /
  `landlock_net:bind_connect`);降级记
  `net=bind_only(...)->block(degraded)` —— summary 永不宣称未装的执法。
  F1:exec 段从计数扩为 canonical 根列表(`exec_roots_canonical=[..]`,
  只列目录)。
- **R9 合取**:网络/exec 归类前置服务端事实 —— `classify_block(stderr,
  stdout, exit_code, net_enforcement)`:`Network` 仅当
  `net==InetBlock`(没装 INET filter 的 spawn 上 listen EPERM 文本必属
  他因,不归网络);`ExecFace`/`Write` 是 Landlock 文件面事实,任何 net
  档下可归。字符串只进 UX,永不作授权依据。

### 13.4 F3 识别 + remediation 文案(R7)

- `classify_block` 新分支:`exit 126 ∧ stderr "Permission denied"` →
  `ExecFace`(wrapper 脚本 exec 面外二进制,pnpm 型;126 是强信号,先于
  Write 匹配)。宁缺勿滥锚不动:stdout 裸串仍不认、非 126 的
  Permission denied 仍归 Write。
- 网络与 ExecFace 的 guidance 文案 =「**收敛到 ONE operator 指令然后
  停**」(R7):逐字节重跑对长驻进程/exec 缺口结构性无效。P3d
  guidance_suffix 改从 offer 携带的 kind 直达
  (`failure_guidance_for_kind`),不再从证据行重分类。
- `EscalationBlock` 增 `ExecFace`(block_label「exec 面缺口」)。

### 13.5 ready_port 就绪探测(R5,工具面)

- `run_background_shell` 可选 `ready_port`(1..=65535)→ registry
  `ShellEntry.ready: ReadyState`(Pending / Ready{port, listener_pid,
  comm, at_ms} / TimedOut),`shell_status` Running 态带出
  (`ready: None` = 未配置,wire-additive)。
- **四铁律**:① TCP connect-only(裸 `TcpStream::connect`,非 HTTP/DNS);
  ② loopback 字面量(`probe_addr` 从 IpAddr 字节构造,无 resolver 路径);
  ③ 随 spawn 绑死(字段无私有 setter,无二次读工具输入);④ 仅自注册
  entry(探测任务只写自己的 key)。
- **归因**:`procnet::find_listeners(port)`(/proc/net/tcp{,6} LISTEN →
  inode → /proc/*/fd → pid → stat 取 pgid/comm),PGID == shell 进程组
  (child 是 group leader)才算 Ready;**端口碰撞(PGID 不匹配)停
  Pending 不误报**;窗口 30s(与 max_runtime 解耦)到点 = TimedOut
  (失败信号,绝不折叠成功);entry 终态即停探(Pending 留实)。
  `ready_port` 只喂探测,永不进授权面。/proc 机器独立成 `procnet`
  模块(后续 AllowAll capability token 血统拒授复用)。

### 13.6 已知边界 / 挂账(如实)

- **BindOnly live 验证 blocked-on-kernel**:需 Landlock ABI ≥4(内核
  ≥6.7)。WSL2 6.6 = ABI v3(文件面可用 —— §12 实测的 EPERM/126 都来自
  它;**注意 `/sys/kernel/security/lsm` 不存在 ≠ 无 Landlock**,以
  syscall 探针为准,任务 research §5 修正过该误判)。真内核矩阵测试
  `integration_bind_only_net_matrix` 在 ABI<4 大声 SKIP。
- **UDP/DNS 在 BindOnly 下不受控**(seccomp 未装,Landlock v4 只管 TCP)
  —— Settings 文案如实写;收紧是后续维度。
- AllowAll 档:枚举/parse 支持但**无写入口**(挂账:daemon 内存态
  capability token + 「daemon TCP 回路是控制面唯一路径」不变量测试化)。
- F1 canonicalize 对「PATH 目录是符号链接」是防御 + 审计改进,但
  **救不了 pnpm 型 exec 缺口**(wrapper exec 目标在所有 PATH 目录之外,
  F0 实测);F2(`sandbox_extra_exec` 白名单,收
  `~/.local/share/pnpm/{global,store}` 类叶子目录)挂账待评审。

## 14. Durable Prefix Grant — 多 token 前缀授权项目级持久化(2026-09-22 起)

> task `09-21-durable-prefix-grant`;设计/证据:任务 design.md +
> research/spec-survey.md + 群聊评审(session 0a1122b4,7 项必改已并入)。
> §12.3 B 的落地契约正文。sibling F3 remediation 文案(「收敛到 ONE
> operator 指令」)的承接终点。

### 14.1 数据模型与命中语义

- **新表** `project_shell_grants`:PK `(project_id, worktree_key,
  prefix_tokens)`;`tool_name` 降为溯源列(批准时 raw 名,读侧跨 shell
  族共享——RULE-PERM-002 语义;PK 含 tool_name 会让前台批准对后台启动
  失效,评审必改二);FK CASCADE 删项目级联清。存量
  `session_tool_permissions` 不迁移(off 档读侧 durable 先查、回落
  session 首 token 行,向后兼容)。
- **worktree_key** 复用 `policy::worktree_key()`(canonicalize + literal
  fallback,与 net snapshot 同源):隔离 worker worktree 独立 key 不继承
  主树批准;**非隔离 worker(同 worktree)继承**(W1 裁定:同树同信任域)。
- **多 token 前缀**:批准时全量 token 序列落库(≤8 token = 可用性护栏,
  前缀越长信任面越窄);切分 `split_whitespace` + **配对引号读写对称
  剥离**(W2:`--filter "@jjh/web"` 与 `--filter @jjh/web` 互通;不配对
  保持字面 → miss → 进沙箱,fail-safe)+ 首 token basename 归一
  (`first_token`)。纯函数族(`grant_gate` / `prefix_tokens_hit` /
  `prefix_tokens_for_allow_always`)全部在 `shell_trust.rs` —— sandbox 模块
  **仅可 import 这些纯函数叶子**(实边是 permissions → sandbox 经
  escalation,防名义环深化,durable 读函数放 `sandbox/policy.rs`)。
- **grant 闸 `grant_gate`**(PR0,独立可合入的既有缺陷修复):旧
  `has_structural_metachar` 只查 `|`/`&&`/`;` —— 换行、单 `&`、
  `$()`/反引号均放行,`split_whitespace` 吃 `\n` 使 `npm run\nrm -rf ~`
  可命中 `npm run` grant。新谓词 = 旧三符号 + `\n`/`\r` + 任意 `&` +
  命令替换;**读写两侧 + 既有 session 级读侧三处全部回灌**。

### 14.2 消费点(三处,读函数 `policy::durable_shell_grant_hit` 单源)

| 消费点 | 位置 | 语义 | 生产可达 |
|---|---|---|---|
| A 免沙箱启动 | `sandbox::decide` Face 分支内、extra/net 读之前;`mode != Plan` 门 | durable 命中 → `Skip{reason: DURABLE_GRANT_SKIP_REASON}` = 免沙箱**启动**(R3 核心:长驻进程无「先失败再重跑」) | ✅ 主路径 |
| B 升级闭环 | `escalation::prefix_grant_hit` durable 先查 | 同域查询,A 命中则命令根本没进沙箱 | ❌ 写面不变量(防 A 域漂移,仅直调单测) |
| C off 档 Tier 4 | `check/permission.rs` Shell 分支 (a) 段前置 | 命中 → Allow 免弹卡(off 档语义 = 免审批,PRD OQ-B 用户裁定) | ✅ |

- **Plan 永不豁免**(与 §10 触发 `mode != Plan` 对齐);Yolo /
  kill-switch / 非 Linux(fail-open→Off)结构性不查询(§1 求值序零改动)。
- **Face(ReadOnly)+ Edit 命中 = 有意绕过 readonly 面**(operator 批准
  优先于项目面默认;测试锚 `decide_durable_grant_hit_skips_sandbox` 防误修)。
- miss 语义全方向 fail-safe:复合命令 / 无 session 行 / NULL project_id /
  sqlx 错误 → miss(warn 不冒泡)。

### 14.3 写点与审计

- **写点内化**:`ask_path` parent Shell AllowAlways 臂(`try_grant_durable_
  shell_prefix`)——**零新参数,调用方无逃逸路**(评审必改四);eligibility
  = Shell 族 + token 形态合法 + session 解析到 project,否则回落 legacy
  session 行(超 8 token 命令 = AllowOnce 语义)。**worker 内存臂不动**
  (per-run cache;worker 批准结构性不产生 durable 行,单测钉死)。
- **两卡文案 + `grant_pattern` 回显**(评审必改六):升级卡
  (`escalation_reason`)与 off 档卡(`durable_trust_face_suffix`)都明示
  信任面(免沙箱 = 文件+网络全开 / off 档 = 免审批);payload 增量字段
  `grantPattern`(wire-additive,旧前端忽略)。
- **审计三事件闭环**(R5):建立 = `permission_granted` + reason
  `durable shell prefix grant: <pattern>`;命中 = `tool_allowed` + reason
  `durable prefix grant hit`(**工具层**按 `DURABLE_GRANT_SKIP_REASON`
  匹配写——sandbox 模块保持 audit-free,`record_durable_grant_hit_audit`);
  撤销 = `GrantRevoked`(wire additive;管理面带 session 上下文时落行,
  无则 best-effort 跳过)。

### 14.4 管理面与回滚

- 双端(daemon route + Tauri command):`list_project_shell_grants` /
  `revoke_project_shell_grant`(三段 key 删;HTTP 面 snake_case,行
  camelCase);GUI = Settings「项目沙箱」页「免沙箱命令授权」区块
  (列表 + 撤销,`CMD_TO_DOMAIN` 已登记,http.routes-sync 守卫覆盖)。
- **行为回滚 = 清空 `project_shell_grants` 表**(代码无需回退;kill-switch
  与 durable 正交:Off 路径仍有 C 消费点,与 session grant 同面)。

### 14.5 已知边界 / 挂账(如实)

- 升级触发的识别锚(node/go/python 三条强特征)2026-09-22 起已覆盖
  stderr + stdout 双流 + 大小写不敏感 errno 字面量(§12.2 补修,live
  E2E 实证裸 node stderr 崩溃形态);其余栈(rust/浏览器引擎等)仍依赖
  stderr 的通用 errno 字面量——覆盖面可后补,不阻塞主线。
- 引号剥离只做「配对同种引号一层」;嵌套引号 / 转义形态保持字面
  (miss → 沙箱,fail-safe 方向,不再收窄)。
