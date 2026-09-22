# Spec 勘察(2026-09-22,OQ5)——现状机制地图 + OQ 收敛建议

> 代码基线:main @ f6f9fa87。全部分布在 `app/src-tauri/src/` 下。

## 1. 现状机制地图

### 1.1 grant 写入面(唯一通道)

- `agent/permissions/ask.rs:711-743`:`ask_path` 收到 `AllowAlways` →
  `check/permission.rs::match_value_for_allow_always`(同文件 :859)计算
  `(match_kind, match_value)` → `db::grant_tool_permission` 落
  `session_tool_permissions`。
- Shell kind 的 match_value = `shell_trust::first_token_for_allow_always`
  (:791)= 首 token 的 basename(`split_whitespace` + 去 `./` 前缀 +
  取末段 `/` 后)。**首 token 粒度由此而来。**
- 升级闭环的弹卡(`escalation.rs::ask` :111)复用同一 `ask_path`——
  AllowAlways 落库通道天然共享。
- **结构限制**:`ask_path` 返回的 `Decision::Allow` 不区分 AllowOnce /
  AllowAlways(两者折同值);escalation 侧拿不到「用户选了哪个」。

### 1.2 grant 读取面(三处,全部 session 级 + 首 token 等值)

1. `check/permission.rs::check_prefix_grant`(:819)——Tier 4 **off 档**
   经典路径。前置闸 `has_structural_metachar`(:438,非 quote-aware,
   `|` / `&&` / `;`,false-positive-safe)。
2. `permissions/escalation.rs::prefix_grant_hit`(:191)——升级闭环
   grant-hit 免卡重跑(shell.rs:606)。同款闸 + 同款查询。
3. worker 路径 `run_grants` 内存缓存(`has_run_grant("prefix", ...)`,
   permission.rs:414)——per-run,与 DB grant 平行。

读侧 SQL 统一形态:`tool_name IN ('shell','run_background_shell') AND
match_kind='prefix' AND match_value = <first_token>`(精确等值)。

### 1.3 表结构(db/migrations/schema.rs:513)

```sql
CREATE TABLE session_tool_permissions (
  session_id TEXT NOT NULL,          -- FK sessions ON DELETE CASCADE
  tool_name  TEXT NOT NULL,
  match_kind TEXT NOT NULL CHECK (match_kind IN ('tool','prefix','path')),
  match_value TEXT,
  granted_at TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (session_id, tool_name, match_kind, match_value)
)
```

**session_id 是 PK 首列且 NOT NULL + CASCADE**——durable(跨 session)
grant 在此表上无法表达;加 project 列要动 PK + 存量迁移。CHECK 不可扩域
是 sibling 任务走新列(`projects.sandbox_net`)的同一动因。

### 1.4 P3c/P3d 结构(沙箱与审批的关系)

- **判定层短路**(`check/permission.rs:357`):Tier 4 shell 分支头
  `resolve_session_policy != Off` → 直接 `Allow`(跳过 prefix-grant /
  classify / ask)。沙箱档下审批层整体让位于执行层。
- **执行层单点**:`sandbox::decide(ctx, command, session_id)`
  (sandbox/mod.rs:354)——**仅两个调用点**:`tools/shell.rs:481`(前台)
  与 `tools/run_background_shell.rs:249`(后台)。返回
  `Decision::Sandbox(spec) | Skip { reason }`;`Skip` 不写审计
  (design §2.2 skips are not security events)。
- **求值序**(resolve_policy,capability → Yolo → 项目 off → kill-switch
  → Plan→ReadOnly → 项目面):durable grant 查询若放 `Face` 分支内,
  Off / Yolo / kill-switch 关 / 非 Linux(fail-open→Off)天然不查询、
  零路径——跨平台统一不用额外设计。
- **升级闭环是事后的**:先沙箱失败一次 → classify_block 命中 →
  grant_hit 先查(仍要先失败!)→ 或弹卡 → 批准 → 逐字节重跑。
  R3「免沙箱启动」= 把 grant 命中前移到 `decide`(**启动豁免**),
  升级闭环保留作未命中时的入口(承接 sibling F3 的「ONE operator
  指令」remediation:用户批准一次 → 以后免沙箱启动)。

### 1.5 project 维度 keying 先例

- `sandbox/policy.rs::read_project_sandbox_policy`(:244):
  `sessions.project_id JOIN projects` 点查——session→project 的既有路径。
- sibling `project_net_snapshots`:PK `(project_id, worktree_key)`,
  worktree_key = `ctx.worktree_path` canonicalize 后绝对路径,**换分支 /
  重检出 = 新键,快照不随行**(spec §13.1)。worktree-contract 下
  worker worktree 是独立 key,不继承主 worktree 的快照。

### 1.6 审计 kind 家族(audit.rs:34)

Tool 域:ToolDenied / ToolAllowed / ToolPermissionAsk / ToolExecuted /
SandboxedShellExecution / ToolDeniedYolo;Permission 域:
PermissionGranted / PermissionTimeout / RequestCancelled。追加变体零迁移
(wire 宽松);`audit_grant_rerun`(escalation.rs:151)用
ToolAllowed + reason 串作轻量来源标记。

## 2. OQ 收敛建议(逐项)

### OQ1 前缀语义 → 多 token 前缀序列,批准时全量落库

- **语义**:批准 `pnpm --filter @jjh/web dev` → 落 4 token 序列;命中 =
  被检命令 token 序列的**前 N 个 token 逐个相等**(库内序列是被检命令
  的前缀)。`pnpm dev --port 3000` 命中(前 4 token 一致需要被检命令
  前 4 token = pnpm/--filter/@jjh/web/dev——注意方向:库内 tokens 必须
  是被检命令 tokens 的前缀)。
- **切分算法**:沿用 `shell_trust` 的朴素 `split_whitespace`(与
  first_token 同源);引号形态(`--filter "@jjh/web"` vs `--filter @jjh/web`)
  不互通 → 不命中 → 进沙箱。**方向安全**:漏命中 = 进沙箱(fail-safe),
  误命中 = 免沙箱(危险)。basename 归一只做首 token。
- **上限**:落库 token 数 ≤ 8(防超长滥用);被检命令不限长。
- **复合命令闸**:`has_structural_metachar` 读写两侧沿用,语义不变。
- 为什么不是「完整文本哈希」:dev server 场景参数变化(换端口/换 flag)
  会要求重批,可用性回到 one-shot;多 token 前缀是「命令模式」的自然
  形态,spec §12.3 B 原文即此方向。
- 为什么不是固定 N token 截断:`pnpm --filter X` 截断前缀会覆盖
  `pnpm --filter X build`——把构建也免沙箱,信任面无谓扩大。

### OQ2 持久化载体 → 新表(挂 project + worktree 维度)

- **新表** `project_shell_grants`(名字 design 定),PK
  `(project_id, worktree_key)` + 前缀列;**不碰**
  `session_tool_permissions`(session_id NOT NULL + CASCADE 与 durable
  语义冲突;CHECK 不可扩域同 sibling 动因)。
- **keying**:`(project_id, worktree_key)`,worktree_key =
  `ctx.worktree_path` canonicalize(sibling net snapshot 同款)。
  效果:worker worktree(独立 worktree_path)不继承主 worktree 的
  grant——LLM 驱动的隔离子代理不因用户在主 worktree 的批准获得免沙箱。
  **待用户裁定**(见问题 1)。
- 跨 daemon 重启天然成立:DB 是 daemon 持久层,无内存态。

### OQ3 与既有路径关系 → 扩展三点,不平行第四通道

- **读点 A(新,R3 核心)**:`sandbox::decide` 的 `Face` 分支内查
  durable grant,命中 → `Decision::Skip { reason: "durable prefix grant" }`
  = 免沙箱**启动**。两调用点(前台 + 后台)一次覆盖;判定层(permission::
  check)零改动——与「沙箱是判定层之下的限损层」分层一致(grant 决定
  的是「是否套限损层」,不是「是否审批」)。
  - 同分支内需写一行命中审计(R5):Skip 本不写审计,grant-hit Skip
    属安全事件,须显式写(ToolAllowed + reason,audit_grant_rerun 形态;
    或新 kind,design 定)。
  - **Plan 不豁免**(与 §10 触发条件 `mode != Plan` 对齐):Plan 的
    价值 = 确定性只读面,启动豁免在 Plan 下保持关闭。
- **读点 B(改造)**:升级闭环 `prefix_grant_hit` 扩查 durable 表
  (未命中 durable 才走弹卡)——已批准命令重启后再次被拦时,grant-hit
  直接重跑,不再弹卡。
- **读点 C(不动)**:Tier 4 off 档 `check_prefix_grant` **不吃 durable**:
  off 档无沙箱,弹卡是审批面;durable grant 的批准语义是「免沙箱启动」,
  不是「免审批」。混用会让用户的一次沙箱豁免批准变成项目内免审批通行证。
- **写点(R4)**:升级闭环弹卡 AllowAlways 落 durable。落点选择:
  `ask_path` 加 scope 参数(Some(durable)时 match_value 用多 token 版)
  vs escalation 侧旁路落库——design 阶段定(依赖「ask_path 需向调用方
  暴露 AllowOnce/AllowAlways 之分」这一结构改动,两案都绕不开)。

### OQ4 跨平台 → 结构性免设计

- 查询只在 `decide` 的 `Face` 分支;非 Linux / capability fail →
  `Policy::Off` → 不查。macOS/Windows 用户 grant 表存在但无消费路径,
  GUI 管理面全平台一致。无平台分支代码。

### OQ5 → 本文档

## 3. 安全边界复核(两场审议共识的落点)

- **D4 双执行边界**:启动豁免(读点 A)表面上「第一遍就不进沙箱」,
  但批准发生在升级闭环(第一遍沙箱内失败、危险部分未发生之后)——
  边界保留在**批准时点**,不在执行时点。PRD 须明写。
- **信任面**:免沙箱 = 该进程文件 + 网络全开(比 BindOnly 宽),
  批准卡文案明示(R4);与 `off` 档(项目级全命令放行)区分。
- **字符串特征永不作授权依据**(R9 精神):durable grant 依据的是
  operator 批准的命令 token 序列,不是输出特征。
- worker(worktree keying)+ Plan(不豁免)+ 复合命令闸(沿用)
  三个收缩面在。
