<!-- Moved from sandbox-executor.md 2026-09-28 (doc-split): §13 -->

# Sandbox NetPolicy — 网络维度三态与 BindOnly 执法(2026-09-21 起)

> task `09-21-sandbox-net-bindonly`;设计/证据:任务 design.md +
> research/(两场审议 + live probe)。[12-listen-gap-remediation §12.3 C](./12-listen-gap-remediation.md) 的落地契约正文。
> hub:[sandbox-executor.md](../sandbox-executor.md)(§1-§9 核心契约)。

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
  ≥6.7)。WSL2 6.6 = ABI v3(文件面可用 —— [12-listen-gap-remediation](./12-listen-gap-remediation.md) 实测的 EPERM/126 都来自
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
