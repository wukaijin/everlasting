# Implement — durable prefix grant 执行计划

> 前置:prd.md / design.md 已过收敛门 + 群聊评审(2026-09-22,7 项
> 必改已并入)。分层推进,每步可独立验证;PR 粒度按 5 个切。
>
> **2026-09-22 收尾**:PR0-PR4 + spec 沉淀全部落地(提交 a682bc3c /
> 42d75eee / 969123fe / edabae87);live E2E 三段全绿(证据见 prd.md
> AC 末条),顺带修复 classify_block stderr 识别缺陷(spec §12.2 补修)。

## PR0:grant 闸修补(独立提交,评审必改一)

- [x] `shell_trust.rs`:新谓词 `grant_gate(cmd)` = `has_structural_
  metachar`(现状三符号)+ `\n` / `\r` + 单 `&` + `$(` + 反引号
  (`has_command_substitution` 复用);矩阵单测(含 `npm run\nrm -rf ~`)。
- [x] 回灌三处既有读侧闸:`check_prefix_grant` 前置(permission.rs
  Shell 分支 a 段)/ `escalation::prefix_grant_hit` / worker run_grants
  前置。**动手前 enumerate tests_check / tests_ask 受影响断言**。
- 验证:`cargo test -p everlasting --lib`(全量);此 PR 独立可合入,
  是既有缺陷修复,不依赖 durable 任何部分。

## PR1:存储 + 纯函数地基

- [x] `db/migrations/schema.rs`:追加 `project_shell_grants` 建表
  (design §2 DDL,**PK 无 tool_name**——溯源列);`db/permissions.rs`:
  CRUD(`grant_project_shell_grant` UPSERT / `list_project_shell_
  grants` / `revoke_project_shell_grant` **三段 key** 删,全列 NOT
  NULL 无 NULL 分支)。
- [x] `agent/permissions/shell_trust.rs` 追加纯叶子:`prefix_tokens_
  hit` + `prefix_tokens_for_allow_always`(≤8 token / 首 token
  basename 归一 / **配对引号读写对称剥离**,W2;复合闸 = PR0 的
  `grant_gate`,由调用方持有)+ 单元矩阵(design §8 首条)。
- [x] `sandbox/policy.rs`:读函数 `durable_shell_grant_hit(db,
  session_id, worktree, command)`(**放 policy.rs 不放 permissions**,
  评审必改三:防依赖名义环;project_id join 复用 read_project_sandbox_
  policy 形态;worktree_key 复用既有 `policy::worktree_key()`;
  canonicalize/sqlx 失败 = miss 不冒泡)。sandbox → permissions 只允许
  import shell_trust 纯函数叶子(模块 doc + 静态守门)。
- 验证:`cargo test -p everlasting --lib prefix_match grant_gate` +
  `db::tests`(schema roundtrip)。

## PR2:消费点(decide + 升级闭环 + off 档)

- [x] `sandbox/mod.rs::decide` Face 分支:**extra/net 读取之前**插入
  durable 查询(design §4.1:命中即 Skip 省后续查询)+ Plan 显式不
  豁免 + Skip 审计行(ToolAllowed + reason)。
- [x] `escalation.rs::prefix_grant_hit`:durable 先查,回落 session 行
  (签名不变;**注明生产不可达的写面不变量注释**,仅防 decide 查询域
  漂移;不写生产集成测试)。
- [x] `check/permission.rs` Shell 分支 (a) 段:durable 前置查询(带审计),
  回落 `check_prefix_grant`。
- [x] 测试:tests_sandbox / tests_check / tests_escalation 增补
  (design §8,含 Face(ReadOnly)+Edit 有意语义锚 + 依赖方向静态守门)。
- 验证:`cargo test -p everlasting --lib`(全量,勿单模块循环)。

## PR3:写点(ask_path 零参内化 + 两卡文案)

- [x] `ask.rs` parent Shell AllowAlways 臂内化 eligibility(design §3:
  **不加参数**——Shell kind + token 合法 + project 解析成功 → durable
  行;超限/解析失败 → session 级行;worker 内存臂不动 + 孤儿行不可能
  单测钉死)。
- [x] `escalation_reason` 与 off 档卡 payload:**两卡**信任面文案 +
  `grant_pattern` 归一前缀回显(design §6)。
- [x] 超 8 token 命令:后端不落 durable(AllowOnce 语义),卡片文案说明。
- 验证:`cargo test -p everlasting --lib` + `tests_escalation` 断言
  durable 行内容。

## PR4:管理面(daemon route + Tauri command + GUI)

- [x] `commands/permissions.rs` + `daemon/routes/permissions.rs`:
  `list_project_shell_grants` / `revoke_project_shell_grant`(**三段
  key**)双端;撤销写 `GrantRevoked` 审计(audit.rs 追加变体,wire
  additive)。
- [x] GUI:Settings 项目区「免沙箱命令授权」列表 + 撤销按钮
  (app/src,参照群聊预设页模式);vitest 组件测试。
- [x] docs:DAEMON-API.md 契约段 + AGENTS.md 速查一句话。
- 验证:`cd app && pnpm test`;daemon route 测试;`node scripts/
  remote-e2e-smoke.mjs`(若涉 daemon 面)。

## 收尾

- [x] spec 沉淀:`.trellis/spec/backend/sandbox-executor.md` §12.3 B 标
  「已实施」+ 新 §14 契约正文(参照 sibling §13 形态)。
- [x] live 端到端(AC 末条):无 Landlock 内核本机跑 dev server 拦截 →
  批准 → 重启免沙箱启动;`scripts/turn-smoke.sh`。
- [x] PRD AC 全表勾验 → `task.py finish` / archive。

## 风险文件 / 回滚点

- 高风险:`sandbox/mod.rs::decide`(求值序敏感,spec §1「勿重排」)——
  durable 查询只加在 Face 分支内部,不动序。
- `ask.rs` AllowAlways 臂内化改写(零新参,但该臂是全部 grant 落库的
  单点)——PR3 一次做完;off 档/升级/worker 三路行为断言齐全后再合。
- PR0 闸修补会改既有读侧行为(换行/单&复合命令从「可享 grant」变
  「不享」)——本身是缺陷修复,但须独立提交可单独回滚。
- 行为回滚 = 清空 `project_shell_grants` 表(代码不回退);
  `sqlite3 ~/.local/share/dev.everlasting.app/everlasting.db -readonly`
  可查(WAL 安全)。

## 验证命令速查

```bash
cargo test -p everlasting --lib                        # 全量(PKG_CONFIG_PATH 见 AGENTS.md)
cd app && pnpm test                                    # 前端
cd app && pnpm test:e2e                                # Playwright(涉 GUI 时)
scripts/turn-smoke.sh                                  # 单轮 live 冒烟
```
