# Design — NetPolicy 三态 / Landlock v4 BindOnly / ready_port

> 依据：`.trellis/spec/backend/sandbox-executor.md`（§1 求值序 / §2 规则集 / §3 信号安全 / §4 断网 / §5 fail-open / §9-11 面·升级闭环 / §12 listen 缺口）+ `research/` 两份固化。改动锚点沿用讨论核实过的 file:line。

## 1. 边界与不变量

- **判定层零改动**（spec §1 C2 延续）：NetPolicy 只影响 spawn 侧 `prepare()` 装什么执法器，`shell_trust` / 5-Tier / Tier 4 短路语义不动。`resolve_policy` 求值序不动，net 维度在**项目面读取处**伴随读出（与 `sandbox_policy` 同行），非新 gate。
- **三条硬不变量**：
  1. Block 档与改动前逐字节一致（AC1）——seccomp filter、guidance、审计全不动；
  2. seccomp 与 Landlock-net 两执法器互斥，同一 spawn 至多装一个（类型系统保证，非运行时约定）；
  3. containment 未知态倒向 containment：net 列 parse 失败 → Block；probe 不支持 BindOnly → 降 Block；**绝不静默落 AllowAll**（AllowAll 本期无写入口）。
- **安全模型**：`:7456` 零鉴权前提不变；BindOnly 的 connect 集 `{80,443}∪bind快照` + 快照钳位（∩ daemon 监听口 = ∅）= 结构性把控制面排除在 agent 可达集外。字符串特征（classify_block 族）只进 UX（guidance/升级触发），永不作授权依据——授权真源只有 operator 快照。

## 2. 数据模型与迁移

- `projects` 新列 `sandbox_net TEXT`（ nullable，NULL=Block 默认；不进 CHECK——正交维度无域校验需求，parse 层 fail-closed 已够）。迁移走既有 `add_project_column_if_missing`（columns.rs 模式），存量库零重建。
- 序列化格式：`block` / `bind_only:3001,3000` / `allow_all`（parse 仅认知三形；`bind_only` 后至少一端口、上限与端口范围 u16 校验，非法整串 fail-closed Block + warn）。
- `NetPolicy`（sandbox/policy.rs，与既有 `Face` 并列）：
  ```rust
  enum NetPolicy { Block, AllowAll, BindOnly(BindSet) }
  struct BindSet { ports: BTreeSet<u16> }   // 快照生效值，非用户自由输入
  ```
  `BindSet` 构造仅两处：DB 读（operator 已确认快照）+ 测试；LLM/manifest 提议走「建议 → 确认流 → 快照落库」管道，不直构。
- **bind 快照表**：`project_net_snapshots(project_id, worktree_key, ports, confirmed_by, confirmed_at)`，`worktree_key` = worktree 绝对路径（防换分支旧快照生效；先例 preset_key 快照语义）。读路径：`resolve` 时按 project+worktree 点查，无快照行 → BindOnly 无从谈起（配置面上 BindOnly 依赖快照存在，缺快照回 Block + 提示）。
- 写通道（daemon route + Tauri command 双端，IPC 白名单同 `update_project_sandbox_policy` 模式）：`propose_net_ports`（LLM/manifest 建议，pending 态）与 `confirm_net_snapshot`（operator 确认；此处执行钳位——命中 daemon 监听口整单拒绝并回显冲突口）。

## 3. 执法器：prepare() 三态

`sandbox::prepare` 父进程安全区产物扩展（spec §3 纪律不变——CString/Vec 分配仍在父进程，pre_exec 闭包零分配）：

- `PreparedSandbox` 新字段 `net: PreparedNet`，枚举三态：
  - `Block` → 内含现行 `Vec<sock_filter>`（现状字段搬入，字节不变）；
  - `BindOnly { bind_ports, connect_ports }` → 内含父进程预构造的 `landlock_net_port_attr` 数组（栈上 add_rule 逐条，单条失败整列中止 = spec §3 语义同款）；
  - `AllowAll` → 单元变体（本期构造不出，parse 面无入口）。
- `pre_exec_apply`：`Block` 分支 = 现 `prctl(PR_SET_SECCOMP)`；`BindOnly` 分支 = 文件规则 add_rule 后**追加** net 规则 add_rule（同 fd、`LANDLOCK_RULE_NET_PORT`）再 restrict_self，**不装 seccomp**。两分支代码上互斥（match on PreparedNet），不存在同装路径。
- `sandbox/landlock.rs` 新增：`LANDLOCK_ACCESS_NET_BIND_TCP/CONNECT_TCP` 常量（自写数值 + 单测钉死，spec 陷阱 1 纪律）、`NetAccessSet` 类型（对齐 `AccessSet` 的「权限⊆handled 由类型保证」——陷阱 2 免疫）、handled access 按 probe 报告的 ABI 条件纳入。
- **connect 派生集**：`{80,443} ∪ bind_ports`，`prepare()` 内计算，写死零配置；防御性钳位第二道（写入时已拒，prepare 再减 daemon 监听口——双保险）。
- `Capability::probe()` 扩展：现返回 ABI ≥1 判定之外，新增 `net_rules_supported()`（ABI ≥4 && BIND/CONNECT access 位 probe 成功）；OnceLock 缓存同款。BindOnly + 不支持 → 降 Block：**降级发生在 prepare 入口**（warn 一行 + `PreparedNet::Block`），summary 记 `net=bind_only→block(degraded)`。

## 4. ready_port 就绪探测（工具面）

- `tools/run_background_shell.rs` 参数新增 `ready_port?: u16`（zod/校验层非负 ≤65535，非必填）。**四铁律**实现位置：
  1. TCP connect-only：探测 = daemon 侧 `TcpStream::connect_timeout(("127.0.0.1", port))`，不 HTTP 不 DNS；
  2. loopback 字面量：`"127.0.0.1"` 常量硬编码，不吃配置；
  3. 随 spawn 绑死：`ready_port` 拷贝进 entry 构造后不可变（字段非 pub mut，无 setter）；
  4. 仅自注册 entry：探测循环持有 entry key，结果只写该 entry 的 `ready` 字段。
- `ShellEntry.ready: ReadyState`（in_memory.rs:44 区域扩字段）：
  ```rust
  enum ReadyState { Pending, Ready { port, listener_pid, comm, at_ms }, TimedOut }
  ```
- **归因**：命中 listen 后读 `/proc/net/tcp`（+tcp6）按 `local_address` 端口匹配拿 inode → 遍历 `/proc/*/fd` 找 socket inode 得 PID → `/proc/<pid>/stat` 取 PGID、`/proc/<pid>/comm` → 与 entry 进程 PGID 比对：不匹配 = 端口被他人占（**不报 Ready**，状态停留 Pending 至超时，日志记 collision）。该 /proc 机器实现为独立模块（后续 AllowAll capability token 的血统拒授复用同一套）。
- 超时：默认窗口（实现期定，建议 30s 起步、与 `max_runtime` 解耦）→ `TimedOut`，注入文本如实呈递（不折叠成功）；Ready 注入文本带 port/pid/comm。ready_port 参数**永不进入** sandbox/spec/授权任何面（铁律：探测≠授权）。

## 5. F1 exec 根 canonicalize + F3 识别

- **F0 验证（implement 第 0 步）**：沙箱内跑 `command -v pnpm && readlink -f "$(command -v pnpm)"` + 实测包装脚本 exec 的目标路径，比对 `SandboxSpec` exec 根清单输出，确认「目标目录不在 exec 面」归因；结果记 research。**预期目标路径**：`/root/.local/share/pnpm/global/v11/*/@pnpm/exe/`（pnpm global 目录非 PATH 目录）。
- **F1**：`prepare()` 构建 exec 允许面时，PATH 各目录与工具链探测目录先 `fs::canonicalize`（失败跳过 + 日志，陷阱 5 纪律同款）再加规则；`summary()` exec 段从计数扩为 canonical 路径列表（格式实现期定，注意 private 路径暴露面——只列根目录不列文件）。若 F0 证明目标目录仍不在 canonical 后的面（如 pnpm global 非 PATH 前缀），F1 不充分 → 按挂账启用 F2 讨论，任务内只做 F1 + 记录。
- **F3**：`classify_block` 签名扩 exit_code（调用点 shell.rs / in_memory.rs 已有）；新分支 `exit_code == 126 ∧ stderr 命中 Permission denied` → exec-face-miss 类 guidance（文案指向「exec 面缺口，收敛到一条 operator 指令」）。宁缺勿滥锚不动：stdout 裸串、非 126 的 Permission denied（fs 权限问题）不进网络/exec 归类。
- **remediation 文案**（R7）：升级/remediation 的「逐字节重跑」提示在 net/exec 面缺口场景替换为「收敛到一条 operator 指令然后停」——实现位置 `sandbox::failure_guidance` 文案表 + P3d offer 注入文本，双处测试锚。

## 6. summary/审计与错误契约衔接

- `SandboxSpec::summary()` 增 `net=` 段：`net=block` / `net=bind_only(3000,3001)` / `net=allow_all`（+降级后缀）。审计 payload（`AuditKind::SandboxedShellExecution`）自动带出，零新 kind。
- **R9 合取项**：`classify_block` 网络/exec 归类的调用方在判定「沙箱拦截」前 require `sandbox_applied ∧ summary 证明对应执法器确实装载`（服务端事实，不可被命令输出伪造）；net=BindOnly 项目上 stdout 出现 listen EPERM 文本不再触发 Network guidance（没装 INET block，文本必属他因）——修正第一场 L1 身份判定的「服务端不可伪造合取项」落地。

## 7. Settings / 前端面

- 项目 Settings 沙箱区新增「网络」档位选择（Block / BindOnly——AllowAll 灰显挂账标注）+ 端口快照管理（当前快照、建议待确认列表、确认/拒绝动作）。确认流 = propose→operator 卡→confirm（复用权限卡视觉，非新弹窗体系）。
- 文案三硬要求（vitest 可断言）：① 放行端口 = 数据可外发面（保密性保护不适用）；② UDP/DNS 在 BindOnly 下不受控（seccomp 未装）；③ `Capability::probe()` 不支持时档位区显示「本平台不生效」并禁写。

## 8. 兼容 / 回滚

- 存量库/存量项目：`sandbox_net` NULL → Block = 现状，零行为变更；迁移 add-column 幂等。
- 回滚三档独立：净策略面回滚 = drop 列读写（或全项目回 Block）；Landlock net 执法回滚 = prepare 三态 match 改走 Block 分支（一行）；ready_port 回滚 = 参数非必填、不传即现状（探测线程不启动）。F1 回滚 = canonicalize 开关化或 revert（行为面只增不减，风险低）。
- 测试基建：真内核集成用例挂在现有「内核不支持时大声 SKIP」纪律下（AC3）；PGID/`/proc` 归因单测在 CI 容器内可跑（进程族自造）。

## 9. 关键取舍记录

| 取舍 | 决定 | 为什么 |
|---|---|---|
| sandbox_policy 扩值 vs 正交新列 | 新列 `sandbox_net` | CHECK 内联建表不可扩域（verified）；文件/网络正交，两维度组合表达力 3×3 |
| seccomp 端口粒度 | 放弃（物理不可表达） | BPF 不能解引用 sockaddr*（§12.3 C 红线，独立核实） |
| connect 单独快照面 vs 派生 | 派生 `{80,443}∪bind` 写死 | 端口语义审不出回环/远端；假控制比没控制糟 |
| AllowAll 进第一刀 | 不进 | 等同 off 级信任；前置 capability token + 控制面不变量测试化 |
| 就绪探测放 LLM 自组合 vs 工具面 | 工具面 ready_port | Block 档沙箱内无 in-band 自证手段；观察点必须在 daemon（讨论④） |
| 快照信任 LLM manifest | 仅建议 | 声明伪造面；operator 确认是唯一 durable 授权出口 |
