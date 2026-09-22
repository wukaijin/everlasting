# Design — durable prefix grant(多 token 前缀 + 项目级持久化)

> 依据:prd.md(2026-09-22 收敛版)+ research/spec-survey.md 勘察 +
> **群聊评审 2026-09-22(session 0a1122b4,73 轮)7 项必改已并入**
> (转录:`~/.local/share/dev.everlasting.app/discussions/2026-09-22-评审
> 「durable prefix grant」…-0a1122b4.md`)。
> 分层不变量:判定层(permission::check 5-Tier)语义不因沙箱档变化;
> 本设计只动「grant 存储 + grant 消费点 + 弹卡落点」。

## 1. 架构总览

```
                 ┌─ 写点 1:off 档 Tier 4 弹卡 ask_path(AllowAlways)
  弹卡批准 ──────┤
                 └─ 写点 2:沙箱档升级弹卡 escalation::ask → ask_path(同通道)

  存储:new table project_shell_grants(PK project_id + worktree_key + prefix_tokens;
       tool_name 降为溯源列——评审必改二,保前后台族共享)

  消费点 ┬─ A. sandbox::decide Face 分支(免沙箱启动,前台+后台两调用点)
         ├─ B. escalation::prefix_grant_hit(生产不可达的写面不变量,防御保留)
         └─ C. check_permission::check_prefix_grant 前置(off 档免弹卡)

  管理:daemon route + Tauri command 双端(list / revoke + 审计)
```

## 2. 数据模型

```sql
CREATE TABLE IF NOT EXISTS project_shell_grants (
  project_id    TEXT NOT NULL,
  worktree_key  TEXT NOT NULL,   -- policy::worktree_key() 单源(canonicalize + literal fallback)
  prefix_tokens TEXT NOT NULL,   -- 空格 join 的规范 token 序列(≤8 token)
  tool_name     TEXT NOT NULL,   -- 溯源列(批准时 raw 名),读侧不参与匹配
  granted_at    TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (project_id, worktree_key, prefix_tokens)
)
```

- **PK 摘 tool_name**(评审必改二):读侧与现存 `check_prefix_grant`
  同款跨 shell 族共享(`IN ('shell','run_background_shell')` 是
  RULE-PERM-002 的有意语义)——按原案精确匹配会让**前台批准对后台启动
  失效**,自断 dev server 主线场景。tool_name 降为溯源列(审计/展示用);
  revoke 用三段 key。
- **无 CHECK、无 FK CASCADE**(同 sibling `project_net_snapshots`
  自治;孤儿行永不命中,可留)。
- `prefix_tokens` 序列化 = 空格 join(与切分算法 `split_whitespace`
  同源,roundtrip 无损)。首 token 存 basename 归一后形态。
- 命中算法(纯函数,放 `permissions/shell_trust.rs` 纯叶子,sandbox
  消费——见 §4.1 依赖约束):

```rust
/// 库内 token 序列 ⊆ 被检命令 token 序列的前缀(逐 token 相等)。
/// 前置:grant_gate(command) == false(调用方闸,见下)。
pub fn prefix_tokens_hit(stored: &str, command: &str) -> bool
```

- 切分 = `split_whitespace` + **配对引号读写对称剥离**(W2 裁定:
  `--filter "@jjh/web"` 与 `--filter @jjh/web` 归一同形态;只剥 token
  首尾配对的同种引号,不碰转义——规则单测钉死)。首 token basename
  归一用 `first_token` 既有算法。
- **8-token 上限理由更正**(评审):前缀越长信任面越**窄**(命中的命令
  集越小),上限不是安全护栏而是**可用性护栏**(防误批超长命令 +
  展示可读),PRD/design 措辞统一。

### 2.1 grant 闸 = 独立谓词(评审必改一)

现状 `has_structural_metachar`(shell_trust.rs:438)只查 `|` / `&&` /
`;`——**换行、单 `&`、`$(`/反引号均放行**,而 `split_whitespace` 把
换行当普通空白,`npm run\nrm -rf ~` 的前 2 token = `npm run` 可命中
前缀 grant(免沙箱执行整条复合命令)。**这是既有 session 级读侧同款的
缺口**(durable 使其跨 session 持久、面更大)。修法:

- 新谓词 `grant_gate(cmd)`:`has_structural_metachar`(三符号)+
  `\n` / `\r` + 单 `&` + `$(` + 反引号(`has_command_substitution`
  既有函数复用)。宁严勿滥:误拒 → 进沙箱/弹卡,fail-safe。
- **回灌 session 级读侧**(check_prefix_grant / prefix_grant_hit /
  worker run_grants 前置闸)须**独立提交先行**——design §7「回滚 =
  清空 durable 表」的前提是既有读侧语义不因本任务变化,闸修补是独立
  的既有缺陷修复,不与 durable 功能耦合(回归面:tests_check /
  tests_ask 受影响断言,动手前 enumerate)。

## 3. 写点改造(评审必改四:ask_path 零新参)

**删除 DurableGrantScope 参数方案**——eligibility 内化到 `ask_path`
的 parent Shell AllowAlways 臂,调用方无逃逸路(安全正资产:不存在
某个调用方「忘了传 scope」而落回 session 级的口子):

```rust
// ask.rs AllowAlways 臂(parent 路径,非 worker 分支),按 classify_tool
// 判定 eligibility,内部决策落点:
// - Shell kind + prefix_tokens_for_allow_always(cmd) = Some
//     + project 解析成功(sessions.project_id 点查)
//     → 落 project_shell_grants(durable 行;不双写 session 级行——
//       durable 是 session 级的超集,同 session 内必命中)
// - Shell kind 但 token 超限(>8)或解析失败 → session 级行(现状)
// - worker 分支(ask.rs 内存 run_grants 臂)完全不动 —— 只写内存
//   cache,孤儿 durable 行结构性不可能(单测钉死)
// - Path / WebFetch / GitMutation kind → 现状,零变化
```

- **worker 孤儿行证伪**(评审必改五):worker 的 AllowAlways 走
  `ask.rs:508` 附近的内存 run_grants 臂,不触 DB durable 写——
  「worker 批准产生 durable 行」结构性不可能,单测钉死该不变量。
- `prefix_tokens_for_allow_always(cmd) -> Option<String>`(>8 token /
  空首 token → None;放 shell_trust.rs 纯叶子)。
- **决策信号不变**:`ask_path` 返回值仍不区分 Once/Always(调用方
  无需感知)。
- **非隔离 worker 读侧继承**(评审必改五后半,W1 裁定:继承):非隔离
  worker(与 parent 同 worktree)的 shell 消费同一批 durable 行 = 同
  worktree 同 project 同信任域的有意语义(PRD R2 落字);隔离 worker
  key 不同,天然不继承。

## 4. 消费点改造

### 4.1 A:decide Face 分支(免沙箱启动,R3 核心)

**依赖方向纠错**(评审必改三):design 初稿「sandbox 已依赖 permissions」
为事实错误——实边是 `permissions::escalation → sandbox`(escalation.rs:43
`use crate::sandbox::SandboxBlockKind`),sandbox/mod.rs 顶层零 permissions
引用。若 durable 读函数放 permissions 且 decide 调之,新增 sandbox →
permissions 边与实边构成名义环。修正:

- durable 读函数 `durable_shell_grant_hit(db, session_id, worktree,
  command)` 放 **`sandbox/policy.rs`**(与 `read_effective_net_policy`
  同址;project_id 点查复用 `read_project_sandbox_policy` 的 join 形态;
  `worktree_key` 复用既有 `policy::worktree_key()`,policy.rs:383)。
- **硬约束(写进模块 doc + 单测守门)**:sandbox 仅可 import
  `permissions::shell_trust` 的纯函数叶子(`grant_gate` /
  `prefix_tokens_hit` / `first_token`),不 import permissions 其余模块
  ——防名义环深化。
- canonicalize 失败(worktree_key 落 literal)= 可能 miss,接受;
  sqlx 错误 warn + 静默降级不 miss 冒泡(fail-safe 方向 = 不豁免)。

```rust
// sandbox/mod.rs decide(),Policy::Face(face) 分支内、extra/net 读取
// **之前**(评审裁定:grant 命中即 Skip,后面的 extra_writable/net
// 查询省掉;未命中才继续现路径):
if ctx.mode != Mode::Plan
    && durable_shell_grant_hit(&ctx.db, session_id, &ctx.worktree_path, command).await {
    // 命中审计(R5):ToolAllowed + reason="durable prefix grant: unsandboxed start"
    return Decision::Skip { reason: "durable prefix grant" };
}
```

- **Plan 不豁免**:`ctx.mode == Plan` 显式跳过(resolve_policy 把 Plan
  归一到 Face(ReadOnly),此判必须在 decide 内显式做)。
- **Face(ReadOnly)+ Edit 模式命中 = 绕过 readonly 面是有意语义**
  (评审裁定,补 AC 防误修):项目 readonly 档下,operator 明确批准过的
  dev server 前缀仍免沙箱启动——批准语义优先于项目面默认。
- **审计**:Skip 路径本不写审计(design §2.2);grant-hit Skip 是安全
  事件,例外显式写一行(ToolAllowed + reason;三消费点同款,读侧可区分)。

### 4.2 B:升级闭环 grant-hit —— 生产不可达的写面不变量(评审降级)

**不可达论证**:decide(A)与 prefix_grant_hit(B)查询同域——A 命中 →
Skip → 无沙箱失败 → 无升级触发;A 未命中 → 进沙箱 → 失败 → B 对同一
命令同域查询亦未命中 → 弹卡。唯一例外是 A 与 B 之间另一 session 写入
grant 的竞态窗口(极窄)。处置:

- **保留实现**(防御性:防未来 A 的查询域被改动后静默出现「该弹卡却
  grant-hit 直跑」),代码注释写明「写面不变量:与 decide 同域,生产
  不可达,仅防 A 域漂移」。
- **仅直调单测**,不做生产集成测试;**R5 审计覆盖不依赖 B**(A/C 两
  消费点承担全部审计断言)。

### 4.3 C:off 档 Tier 4

`permission.rs` Shell 分支 `check_prefix_grant` 调用点(a 段)前置
durable 查询(读函数同源调 `sandbox::policy::durable_shell_grant_hit`,
闸 `grant_gate` 内部保证):命中 → Allow(带审计行)。未命中 → 现状
session 级首 token 查询(存量行兼容)。

### 4.4 消费顺序总结

| 场景 | durable 查询时机 | 效果 |
|---|---|---|
| 沙箱档 + Face + 非 Plan | decide 内(启动前) | 免沙箱启动 |
| 沙箱档 + 失败升级 | prefix_grant_hit(§4.2,生产不可达防御) | (写面不变量) |
| off 档 + Tier 4 | check_prefix_grant 前置 | 免弹卡 |
| Face(ReadOnly)+ Edit | decide 内(有意语义,补 AC) | 免沙箱启动 |
| Plan | 不查 | ReadOnly 面不受影响 |
| Yolo / kill-switch / 非 Linux | 不查(policy Off 短路在前) | 零路径 |

## 5. 管理面(R2)

- daemon route + Tauri command 双端(参照 sibling §13.1 写通道):
  - `list_project_shell_grants(project_id)` → 行集(含 worktree_key /
    prefix_tokens / granted_at / tool_name 溯源)。
  - `revoke_project_shell_grant(project_id, worktree_key,
    prefix_tokens)` → 三段 key 删行(§2 PK 摘 tool_name 后的精确删)
    + 审计行(新 kind `GrantRevoked`,wire additive 零迁移,与 R5
    撤销事件对齐)。
- GUI:Settings 项目详情区(或 sandbox 设置页)新增「免沙箱命令授权」
  列表;行操作 = 撤销。**不做**编辑(撤销 + 重批即可)。

## 6. 批准卡文案(R4,评审必改六:两张卡 + 前缀回显)

- **升级卡**(沙箱档,`escalation_reason`)追加信任面明示段:
  「Approving always = this command pattern starts WITHOUT the sandbox
  for this project (full file + network access), persisting across
  sessions and daemon restarts.」
- **off 档 Tier 4 卡**(语义更重:免审批)同款补信任面文案——评审
  指出初稿只覆盖升级卡,off 档卡的授权语义(免审批通行证)更需要明示。
- **前缀回显**:两张卡的 payload 增量字段 `grant_pattern`(归一后
  prefix_tokens join 形态,如 `pnpm --filter @jjh/web dev`)——用户
  批的到底是什么,卡上直读;前端渲染字段即可,零逻辑。

## 7. 兼容 / 迁移 / 回滚

- 新表 `CREATE TABLE IF NOT EXISTS`(schema.rs 追加,无列变更、无数据
  迁移);存量 session 级行不迁移、不删除(off 档读侧回落兼容)。
- 行为变更面 = 弹卡 AllowAlways 的落点(两处)+ 三个消费点;全部由
  「表里有行」驱动——**回滚 = 清空 project_shell_grants 表**(GUI 撤销
  或 SQL),代码无需回退。kill-switch(sandbox_enabled)不动 durable
  语义:kill-switch 关 = policy Off = durable 无沙箱消费点(off 档
  Tier 4 消费点仍在,与现状 session grant 同面)。
- wire:daemon routes 新增;IPC 权限相关 payload 不变(弹卡前端零改动,
  除文案)。

## 8. 测试锚点(对应 PRD AC)

- **PR0(闸修补,独立提交)**:`grant_gate` 矩阵(换行 / 单 `&` /
  `$()` / 反引号 / 三符号);回灌 session 级读侧后 tests_check /
  tests_ask 受影响断言的 enumerate 清单(动手前定)。
- 单元:`prefix_tokens_hit` 矩阵(前缀命中 / 非前缀 / 引号形态不互通 /
  basename 归一 / 8-token 上限);`prefix_tokens_for_allow_always`
  边界(空 / 超限);worker AllowAlways 臂只写内存 run_grants(孤儿
  durable 行结构性不可能,钉死)。
- `tests_check`:off 档 durable 命中免弹卡 / 未命中回落 session 行 /
  存量首 token 行兼容 / 复合命令(含换行形态)不吃 durable。
- `tests_sandbox`:decide Face + durable 命中 → Skip + 审计行;Plan
  不豁免;Face(ReadOnly)+Edit 命中绕过 readonly 面(有意语义锚);
  kill-switch/Yolo 零路径;依赖方向约束(sandbox 仅 import shell_trust
  纯函数叶子)静态守门。
- `tests_escalation`:升级弹卡 AllowAlways 落 durable 行(断言表内容);
  消费点 B 直调单测(注明生产不可达,不做集成);>8 token 命令卡片降级。
- daemon route 测试:list / revoke(三段 key)roundtrip + 撤销审计行。
- live:`scripts/turn-smoke.sh` + 手动 dev server 场景(无 Landlock 内核
  端到端,AC 最后一条)。
