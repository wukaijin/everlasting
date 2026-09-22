# PRD — durable 授权出口:多 token 前缀 grant 项目级持久化(dev server 免沙箱启动)

> 2026-09-22 规划收敛完成(OQ 全部裁定,依据 research/spec-survey.md 勘察;
> 技术设计见 design.md,执行计划见 implement.md)。

## Goal

无 Landlock 内核用户(旧 WSL2 内核 / 旧发行版,内核 <6.7 或未编 Landlock)的
dev server 放行解法——**授权面而非执法面**:把 AllowAlways 从「首 token 粒度 +
session 级」升级为「多 token 前缀 + 项目级持久化」,批准语义 = 「该命令模式可
免沙箱**启动**」(结构性适配长驻进程)。零内核依赖,全平台可用。

产品裁定(2026-09-21,用户):工具不应要求用户折腾内核才能用基本功能;与
`09-21-sandbox-net-bindonly`(Landlock 执法面,已完成)正交互补。

## Background / Confirmed Facts

- **现状缺陷**(spec `sandbox-executor.md` §12.3 B):AllowAlways 粒度 =
  首 token——批过 `pnpm dev` = 整个 `pnpm` 免沙箱(含 install 脚本);
  升级重跑 = 逐字节 one-shot,长驻进程重启再批;grant 现为 session 级,
  不跨 daemon 重启。
- **sibling F3 衔接**:net-bindonly 任务的 remediation 文案已改为「收敛到
  ONE operator 指令然后停」——本任务是该指引的承接终点(用户按指引批准 →
  durable grant 生效 → dev server 此后免沙箱启动)。
- **机制事实**(勘察,详见 research/spec-survey.md):
  - grant 写入唯一通道 = `ask.rs:711` AllowAlways →
    `match_value_for_allow_always`(Shell → 首 token basename);
    `ask_path` 返回值不区分 AllowOnce / AllowAlways(escalation 拿不到)。
  - 读取三处:off 档 Tier 4 `check_prefix_grant` / 升级闭环
    `prefix_grant_hit`(仍是先失败后命中)/ worker `run_grants` 缓存;
    全部 session 级 + 首 token 精确等值 + `has_structural_metachar` 复合闸。
  - `session_tool_permissions` PK 首列 session_id(NOT NULL + CASCADE)
    ——durable 语义在此表不可表达,CHECK 不可扩域(sibling 走新列同动因)。
  - 沙箱执行层单点 = `sandbox::decide`(仅 shell.rs:481 /
    run_background_shell.rs:249 两调用点);求值序 Off/Yolo/kill-switch/
    非 Linux fail-open 都在 `Face` 分支**之前**短路。
- **安全边界沿用**(两场共识):D4 边界保留在**批准时点**(第一遍在沙箱内
  失败、危险部分未发生才轮到批准);免沙箱命令 = 该进程文件+网络全开
  (信任面比 BindOnly 宽,批准卡文案明示);字符串特征永不作授权依据。

## Requirements

- **R1 多 token 前缀语义**(已收敛;2026-09-22 评审补强 + W2 裁定):
  批准时**全量 token 序列**落库(上限 8 = 可用性护栏,前缀越长信任面
  越窄);命中 = 库内 token 序列是被检命令 token 序列的前缀(逐 token
  相等)。切分 = `split_whitespace` + **配对引号读写对称剥离**(W2:
  `--filter "@jjh/web"` 与 `--filter @jjh/web` 互通;只剥配对引号,
  不碰转义,规则单测钉死);basename 归一只做首 token。**复合命令闸
  扩为独立谓词 `grant_gate`**(评审发现:现状 `has_structural_
  metachar` 只查 `|`/`&&`/`;`,换行、单 `&`、命令替换均放行,
  `npm run\nrm -rf ~` 可命中前缀 grant——既有 session 级读侧同款缺口,
  修补须独立提交并回灌 session 级读侧)。
- **R2 项目级持久化**(2026-09-22 裁定):新表(表名 design 定),PK =
  `(project_id, worktree_key, prefix_tokens)`(**tool_name 降溯源列**,
  评审必改二:读侧跨 shell 族共享,前台批准须对后台启动生效——dev
  server 主线场景),worktree_key = `policy::worktree_key()` 单源
  (canonicalize + literal fallback;换分支/重检出 = 新键;**隔离**
  worker worktree 独立 key 不继承主 worktree 批准)。跨 session、跨
  daemon 重启有效;GUI 可查看 / 撤销(daemon route + Tauri command
  双端,参照 sibling `get_project_net_state` 写通道模式)。
  **非隔离 worker 继承主树 durable grant**(2026-09-22 W1 裁定:同
  worktree 同 project = 同信任域,worker shell 同样消费;隔离 worker
  天然不继承,key 不同)。
- **R3 免沙箱启动**:`sandbox::decide` 的 `Face` 分支内查 durable grant,
  命中 → `Skip` = 免沙箱启动(前台 + 后台两调用点一次覆盖,判定层
  permission::check 零改动);**Plan 下不豁免**(与升级触发 `mode != Plan`
  对齐);升级闭环 `prefix_grant_hit` 扩查 durable(重启后再拦时 grant-hit
  直接重跑,不弹卡)。
- **R4 授权面收口**(2026-09-22 裁定:统一升 durable;评审补强):
  **两处弹卡的 AllowAlways 都落 durable grant**(多 token 前缀)——
  (a) 沙箱档升级弹卡(P3c 前台 / P3d 后台);(b) off 档经典 Tier 4
  弹卡。**eligibility 内化到 ask_path 的 Shell AllowAlways 臂**(评审
  必改四:零新参数,调用方无逃逸路);**两张卡都补信任面文案 +
  `grant_pattern` 前缀回显**(评审必改六:off 档卡语义更重,不能只
  覆盖升级卡)。**读侧必须同步扩查**(见 R6,防 RULE-PERM-002 同族
  「只写不读」死行);session 级存量行(首 token)保留,读侧向后
  兼容(durable 先查,未命中回落 session 级)。
- **R6 off 档审批面生效**(OQ-B 裁定的推论):off 档 Tier 4 读侧
  (`check_prefix_grant` 前置)扩查 durable——命中直接 Allow(免弹卡)。
  语义:durable grant 在沙箱档 = 免沙箱启动,在 off 档 = 免审批执行;
  用户知情选定(便利优先)。GUI 查看/撤销面(R2)因此成为主信任管理面。
- **R5 审计**:建立(既有 permission_granted)/ 命中(启动豁免 + 升级
  grant-hit + off 档审批豁免各写一行,ToolAllowed + reason 形态或新
  kind,design 定)/ 撤销(GUI 撤销动作写行)三事件可查。

## Acceptance Criteria(2026-09-22 实现完成 + live E2E 全绿;✅ = 测试锚 / live 证据)

- [x] 批准 `pnpm --filter @jjh/web dev` 后:同前缀命令(`pnpm --filter
  @jjh/web dev --port 3000`)在新 session 直接免沙箱启动(无弹卡、无先
  失败);`pnpm install`(不同前缀)仍进沙箱。
  ✅ `decide_durable_grant_hit_skips_sandbox`
- [x] 前后台族共享(评审必改二):前台弹卡批准的前缀,`run_
  background_shell` 启动同前缀命令同样免沙箱(dev server 主线场景)。
  ✅ PK 摘 tool_name + `durable_shell_grant_hit` 单源(`decide` 两调用点
  一次覆盖,测试锚同上)
- [x] daemon 重启后 grant 仍生效;GUI 撤销后恢复沙箱。
  ✅ DB 持久(CRUD 测试)+ `project_shell_grant_routes_roundtrip`(撤销
  + `grant_revoked` 审计)
- [x] 复合命令(管道 / `&&` / `;` / 换行 / 单 `&` / 命令替换)不命中
  durable grant;换行形态 session 级读侧同样不命中。
  ✅ `grant_gate_*` 矩阵 + decide/check 消费点测试
- [x] Plan 模式下命令即使命中 durable grant 仍走沙箱。✅ decide 测试
- [x] Face(ReadOnly)+ Edit:有意语义测试锚。✅ 同 decide 测试
- [x] 两卡文案 + `grant_pattern` 回显;审计三事件可查。
  ✅ 落库/回显断言(`parent_shell_allow_always_writes_durable_grant` /
  over-cap 回落)+ 命中审计(`tier4_off_tier_durable_grant_skips_modal`
  断言 `durable prefix grant hit` 行)+ 撤销审计(route 测试)
- [x] 非隔离 worker(W1):同 worktree worker 同 key 命中(结构性:worker
  的 decide 走同一读函数);隔离 worker 不命中。✅ decide 测试 foreign-key
  分支 + W1 语义(PRD R2 落字)
- [x] 引号互通(W2):带引号批准 ↔ 不带引号命中;不配对引号不误剥。
  ✅ `prefix_tokens_for_allow_always_normalizes_command` +
  `prefix_tokens_hit_matches_extending_commands_only` +
  `prefix_tokens_hit_quoting_edge_shapes_stay_literal`
- [x] off 档:新 session 同前缀免弹卡;session 级存量首 token 行仍生效。
  ✅ `tier4_off_tier_*` 两测试
- [x] 无 Landlock 内核环境端到端 live:dev server 被拦 → remediation
  指引 → 批准 → 免沙箱启动成功;重启命令后不再拦。
  ✅ 2026-09-22 live E2E(本机 WSL2 5.15,landlock 有 / landlock_net 无
  / seccomp 有,net=block):Phase1 沙箱拦裸 `node dev.js`(seccomp
  inet_block,exit 1 `listen EPERM`)→ 升级卡(信任面文案 + `grant_
  pattern` 回显)→ allow_always → durable 行 + 三审计 → 免沙箱重跑
  listening;Phase2 新 session 同前缀**零弹卡直接免沙箱启动**
  (`tool_allowed` + `durable prefix grant hit`,无 sandboxed 行),
  不同前缀 `cat` 仍进沙箱;Phase3 list 可见 / revoke 删行 + `grant_
  revoked` 审计。**顺带发现并修复既有缺陷**:`classify_block` 对
  裸 node 的 stderr 小写 `operation not permitted` + listen 强特征
  只喂 stdout 哑火(升级链零触发)——修复见 spec §12.2 补修段,
  锚 `classify_block_reads_stderr_for_listen_denials`。

## 评审结论摘要(2026-09-22 群聊,session 0a1122b4)

- **总体判定**:方案结构可进入实现;7 项文档级必改已并入 R1-R6 与
  design(闸集合缺口 / PK 摘 tool_name / 依赖方向纠错 / ask_path 零参
  内化 / worker 孤儿行证伪 / 两卡文案补全 / 消费点 B 定性不可达)。
- **场景判定**:**实质帮助而非仅省弹卡**——把摩擦从每 session × 每次
  重启(长驻进程逐字节重跑结构性无效)改为每项目每前缀一次,并解锁
  无人值守路径(无人值守 8s 快拒下,现状 AllowAlways 是唯一出路且不
  跨 session);闭环质量悬于前后台族共享(必改二)。
- **已知边界**(如实,不阻塞):升级触发的 stdout 识别仅 node/go/
  python 三条强特征,非 node 栈 dev server 可能零 escalation offer
  (用户仍可经 off 档路径批准);stdout 识别锚扩集挂账归属未定。

## 待裁定项(已全部裁定,2026-09-22)

- **W1 非隔离 worker 继承** → **继承**(同 worktree 同 project = 同
  信任域;评审倾向简单路,用户裁定)。
- **W2 引号对称剥离** → **采纳**(读写两侧同归一化,配对引号剥离;
  评审三方无反对,用户裁定)。
- 评审未决项中不属于本任务范围的:非 node 栈 dev server 的 stdout
  识别锚扩集(挂账归属未定,不在本任务);闸修补回灌的确切回归面
  (PR0 动手前 enumerate,属实现步骤非裁定)。

## Out of Scope

- Landlock BindOnly / NetPolicy(sibling 已完成)。
- AllowAll 档 / capability token(挂账不变)。
- 通配符前缀、正则匹配(如需要另立)。
- worker `run_grants` 内存缓存通道的 durable 化(worker worktree 天然
  走独立 key,worker 路径按 session 内现状语义运行)。

## 已裁定的设计决策(2026-09-22,均经用户确认)

- **keying 绑 worktree**(→R2):接受「同项目多 worktree 各批一次」的
  代价,换取 worker 隔离面无授权旁路。
- **off 档统一升 durable**(→R4/R6):接受语义并存——durable grant
  在沙箱档 = 免沙箱启动、在 off 档 = 免审批执行(知情选便利优先);
  GUI 撤销面因此升为主信任管理面。
- 其余 OQ(前缀语义 / 既有路径关系 / 跨平台 / spec 勘察)由代码证据
  直接收敛,见 R1 / R3+R6 / research/spec-survey.md。
