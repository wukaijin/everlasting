<!-- Moved from sandbox-executor.md 2026-09-28 (doc-split): §14 -->

# Sandbox Durable Prefix Grant — 多 token 前缀授权项目级持久化(2026-09-22 起)

> task `09-21-durable-prefix-grant`;设计/证据:任务 design.md +
> research/spec-survey.md + 群聊评审(session 0a1122b4,7 项必改已并入)。
> [12-listen-gap-remediation §12.3 B](./12-listen-gap-remediation.md) 的落地契约正文。sibling F3 remediation 文案(「收敛到 ONE
> operator 指令」)的承接终点。
> hub:[sandbox-executor.md](../sandbox-executor.md)(§1-§9 核心契约)。

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
  escalation,防名义环加深,durable 读函数放 `sandbox/policy.rs`)。
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

- **Plan 永不豁免**(与 [10-escalation-loops](./10-escalation-loops.md) 触发 `mode != Plan` 对齐);Yolo /
  kill-switch / 非 Linux(fail-open→Off)结构性不查询(hub §1 求值序零改动)。
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
  `durable prefix grant hit`(**工具层**按 `SkipReason::DurableGrant`
  匹配写——sandbox 模块保持 audit-free,`record_durable_grant_hit_audit`;
  N5 起原 `DURABLE_GRANT_SKIP_REASON` 字符串常量由结构化枚举取代);
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
  stderr + stdout 双流 + 大小写不敏感 errno 字面量([12-listen-gap-remediation §12.2](./12-listen-gap-remediation.md) 补修,live
  E2E 实证裸 node stderr 崩溃形态);其余栈(rust/浏览器引擎等)仍依赖
  stderr 的通用 errno 字面量——覆盖面可后补,不阻塞主线。
- 引号剥离只做「配对同种引号一层」;嵌套引号 / 转义形态保持字面
  (miss → 沙箱,fail-safe 方向,不再收窄)。
