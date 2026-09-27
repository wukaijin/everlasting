<!-- Moved from sandbox-executor.md 2026-09-28 (doc-split): §12 -->

# Sandbox listen/网络失败识别缺口 — 临时修复与长期方案

> hub:[sandbox-executor.md](../sandbox-executor.md)(§1-§9 核心契约)。
> sibling 分篇:[10-escalation-loops](./10-escalation-loops.md)(升级闭环消费这些识别特征)/ [13-net-policy-bindonly](./13-net-policy-bindonly.md)(方案 C 落地)/ [14-durable-prefix-grant](./14-durable-prefix-grant.md)(方案 B 落地)。

## 12. listen 场景识别缺口 — 临时修复(2026-09-21)与长期方案(未实施)

### 12.1 缺口实证(为什么修)

jjh-mono 项目 session `23a8184b`(2026-09-20,edit 模式):`vite` dev server
报 `Error: listen EPERM: operation not permitted 0.0.0.0:3001` —— 全文在
**stdout**,`stderr` 为空;python `socket.socket()` 创建被拒、Chrome CDP
端口被拒同样只在 stdout。而 [10-escalation-loops](./10-escalation-loops.md)(§10/§11)的升级触发与
guidance 全部只喂 `classify_block(&stderr)` → 前后台识别整体哑火:该
session 29 次 `sandboxed_shell_execution`、**零 escalation offer**。模型
被迫自行诊断("怀疑 listen 被沙箱按进程上下文拦"),花了多轮
node/python/chrome 三路探测后才绕行(CDP pipe 方案)。根因两层:

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

- **2026-09-25 补修(DNS 失败特征族,task `09-25-sandbox-dns-block-detect`)**
  :listen 族(bind 方向)收口后的姊妹缺口——connect/resolve 方向(git
  pull/fetch、curl 等短命出网命令)。实证 jjh-mono session `ce51a3ba`
  (`git pull --no-rebase 2>&1`,复合命令):seccomp INET filter 拦的是
  `socket(AF_INET/AF_INET6)` 创建(含 resolver 自身 UDP socket),EPERM
  被 getaddrinfo 吞掉,表面化为 DNS 失败文案——listen/EPERM 特征一个不
  匹配,升级链整体哑火。修复四件:
  - **特征**:`dns_smells_net_block` 两族、大小写不敏感、双流都喂(与
    listen 族同纪律;`2>&1` 把报告重定向到 stdout,实证 session 即此形
    态):curl/git/wget 族 `could not resolve host`、glibc getaddrinfo
    族 `temporary failure in name resolution`(EAI_AGAIN)。
  - **宁缺勿滥**:`Name or service not known`(EAI_NONAME)刻意不收——
    健康宿主真 NXDOMAIN 同文案,误归因面大;Block 档下 resolver 不可达
    必然表现为上两族,收窄无损覆盖。stdout 裸 `Operation not permitted`
    / `Permission denied` 依旧不认;**Write 识别保持 stderr-only**。
    残余误报面(grep/cat 日志自引用 + 命令自身失败)由调用侧
    `exit_code != 0` 闸消掉大半,代价 = 一张可拒的卡 + 一行引导。
  - **R9 合取不变**:DNS 双流调用点在 `net != InetBlock → None` 早退
    **之后**(BindOnly spawn 上 UDP/DNS 根本不受控,DNS 文案必属他因,
    不归网络)。锚:`classify_network_requires_inet_block_enforcement`。
  - **卡证据行 + 文案**:`stderr_evidence_line` MARKERS 与
    `stdout_net_evidence_line` 补两族锚(否则卡上取任意尾行);Network
    Edit 档 guidance 补 durable prefix grant 出路(见
    [14-durable-prefix-grant](./14-durable-prefix-grant.md)):单命令形态
    (grant_gate 永不命中复合命令)+ 卡上「始终允许」;Plan 档不动。
  锚:`classify_block_reads_dns_failure_families`(jjh 实证行逐字节入
  锚 + 宁缺勿滥)、`guidance_network_edit_names_prefix_grant_exit`、
  escalation 侧 `stderr_evidence_line_picks_dns_denial` /
  `stdout_net_evidence_line_picks_dns_denial`。

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
> 落地;契约正文见 [14-durable-prefix-grant](./14-durable-prefix-grant.md))。要点:新表 `project_shell_grants`(PK
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
capability probe 渐进启用;当前 WSL 5.15 不满足,不做主路径(hub §4 "不赌
内核版本" 不变)。

> **[2026-09-21 已实施]** — task `09-21-sandbox-net-bindonly`(C 的完整
> 落地,含 B/D 的替代性收口;契约正文见
> [13-net-policy-bindonly](./13-net-policy-bindonly.md))。要点:正交新列
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
