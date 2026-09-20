# N2 checkpoint/revert 闭环(悬空快照路线)

## Goal

给 agent 会话补上**确定性的后悔药**:turn 边界的文件快照基线 + UI 触发的 revert,使"agent 把文件改坏"之后的恢复不再依赖 LLM 自觉修复或用户手工 git 手术。

用户价值(2026-09-20 会话收敛):

1. **补齐 D3 的另一半**——D3 已能对话回头(edit_user_message + Resend),磁盘不回头;N2 后对话与文件可一起回到第 N 轮(rewind 闭环)。
2. **无人值守止损边界**——F2 定时任务 / 后台 escalation / yolo 下,坏改动的爆炸半径被钉死在 turn 边界。
3. **新用户兜底**——不依赖 git 知识的恢复路径。
4. **审计确定性**——"turn N 改了什么"成为一句机械 diff,不靠问 agent。

## Background(已裁定决策)

| 决策点 | 裁定 | 出处 |
|--------|------|------|
| 意义裁定 | 成立(频率假设由用户确认) | 2026-09-20 会话 |
| 路线 | **悬空快照对象**,非分支 auto-commit | 会话:过程 commit 污染 + `none` 档直接写用户仓库两问题 |
| 「session 不 auto-commit」旧 ADR | 收窄翻案:**允许 daemon 创建不可见快照对象**(悬空 commit + 私有 ref),仍禁止在任何用户分支落 commit | 会话 |
| 快照语义 | 整树捕获(temp index + `add -A` → tree → commit 链),`none`/`active` 两态统一覆盖,untracked 收录 | 会话 |
| revert 触发面 | **仅 UI 触发、不进 agent 工具、走 dangerous 通道 + audit 归因** | BACKLOG 附录 B N2 行约束 |
| 并发协调 | revert 前树 hash 比对门禁:当前态 ≠ 本链最新快照 = 有外来写入 → 拒绝/强制确认;worker merge 产物被 host 下轮快照整树收编;revert host 不删 worker 分支 | 会话 |
| 归因分层 | git/diff 答"变了什么",既有写审计(⑨ 权限层 audit)答"谁改的",两层正交 | 会话 |
| 性能 | N9 B2 已证 DB 侧可忽略(persist 1.4ms);树构建 ≈ git status 量级;bench 对象 `finalize_turn_persist` 现成 | spec/backend/perf-baseline.md |

## Confirmed Facts(仓库证据)

- 快照挂钩点:`finalize_turn_persist`(`app/src-tauri/src/db/sessions/messages.rs:209`)——轮末持久化单点,N9 bench 同点。
- git 库:主路径 `git2`(libgit2);内存 index 建树可行(`Index` + `write_tree_to` 不落盘真索引),无需 spawn `git` 命令。
- `turn_trace` 表 `UNIQUE(session_id, run_id, seq)` 与轮 1:1 对齐(`app/src-tauri/src/db/trace.rs`);worker 轮走 `run_id` 路由,主路 `''`。
- 审计挂点:`AuditKind` 枚举(`app/src-tauri/src/agent/permissions/audit.rs:34`)已有 ToolExecuted 等先例,新增 revert 归因 kind 顺路。
- worker merge 语义:`merge_worker` 把 `worker/<run_id>` 分支合进父 session worktree 分支,父须 attached(lazy auto-attach 要求干净根)。
- worktree 三态:`none`(默认,工具 cwd = project.path,直接写用户仓库)/ `active` / `detached`。

## Requirements

### R1 轮末快照链(daemon 侧)

- **写触发门**(2026-09-20 DB 实证修正):仅当轮内出现写信号(写家族 tool / A2+ 判写前台 shell / 后台 shell 完成事件 / worker merge 落地)才扫描建快照;**基线恒建且在轮首**(群聊评审 P0 修正:原轮末建基线会把首轮写入吃进基线,「回到会话前」不可达)——基线钩在 loop 入口、首轮 user 行落库后、早于任何 tool 执行。只读轮零扫描、零对象、零行——成本与实际变更成正比,与对话长度解耦(实证背景:jjh-mono 纯查询 session 8 轮 15 次工具调用全部只读)。
- 建快照时:内存 temp index(`add -A`,收录 untracked)→ 写 tree → 建 commit 对象(parent = 上一快照)→ SHA 落 DB。
- 零接触不变量:不碰用户分支 / 真索引 / 工作区;`git log` / `git status` / `git diff` 行为与现状逐字节一致。
- GC 保护:私有 ref 命名空间(如 `refs/everlasting/<session_id>`)单伞 ref 指链头;session 删除时删 ref,对象交 git GC 自然回收。
- 内容寻址去重:状态未变的轮复用同 tree SHA,零额外对象。
- **已知边界**(群聊评审措辞核准):`add -A` 不作用于已 tracked 文件——准确边界是 **untracked 且 gitignored 不入快照;tracked 后进 gitignore 仍收录且删除照传**。gitignored 路径双重不可见(diff 不显示 + revert 不还原,含 `.env` 类),确认弹窗常驻脚注披露;include-ignored 档留 follow-up(是否提级见 OQ-A)。app_data_dir 下 spill/outputs 在仓库外,天然无关。
- **D3 交互**(群聊评审裁决,依赖 OQ-B 用户确认):`edit_user_message` 级联删尾后重跑经 seq 回落复用会产生孤儿 checkpoint 行(裸 INSERT PK 撞静默零行 + 死分支谱系)——D3 既有事务内跟随级联删 `turn_checkpoints` 行。
- 快照失败 fail-open:best-effort,仅告警日志,不影响轮收尾(fail 即该轮无快照,链 parent 跳接上一可用快照)。

### R2 revert(UI 侧)

- revert 仅 UI 触发(dangerous 通道 + audit 归因),不进 agent 工具面。
- **统一还原语义**(两模式同一原语):还原集 = `diff(当前态, 目标快照树)` 路径全集;逐 path 从目标树 checkout 内容、删除目标树中不存在的 path。**永不触碰分支指针与索引**。
- 确认框逐文件列出还原集,带**归属标记**(来自写审计:session 期间 tool 写过的文件 vs 未归属/可能是用户自改),人来决策。
- 依据(2026-09-20 规划修正):原设想"`none` 模式按写审计 path 级还原"存在证据洞——shell 重定向写(`echo x > f`)在审计中无路径属性(A2+ 判定层只判"是写"不知"写哪"),path 过滤会静默漏还原;全集还原 + 确认框人工把关更安全。

### R3 并发门禁

- revert 前比对当前工作态树 hash 与本链最新快照:不一致(外来写入,含用户在轮间隙的手改)→ **强制确认**(展示外来 delta 专属警告区),绝不静默通过。
- 归属区分交给确认框标记(见 R2),树 hash 门禁只负责"不静默"。
- 共享 cwd 争用是既有问题,本任务不使它恶化、不负责治它。

### R4 轮间 diff 查看面

- SHA 的查询出口:TurnCard(或等价 UI)「本轮 diff」→ 复用现有 DiffView 渲染 `diff <ckpt-N-1> <ckpt-N>`。

### R5 降级

- 非 git 项目:功能不可用,UI 隐藏,不报错。
- 群聊 session:不建链(写冲突本低,语义另行评估)。

## Acceptance Criteria

- [ ] AC1 快照零侵入:开启快照前后,同仓库 `git status --porcelain` / `git diff` / `git log` 输出逐字节一致,**且 index 文件字节 + mtime + inode 三不变**(群聊评审升级:仅断言 git 输出盖不住"同内容重写毁 stat cache";进 PR0 门禁)。
- [ ] AC2 轮间 diff 正确:改 3 文件 + 新建 1 文件跨 2 轮,`diff ckpt(N) ckpt(N+2)` 恰含 4 文件且新文件可见(裸 `git diff` 看不到的它能看到)。
- [ ] AC3 revert 语义:revert 到第 N 轮后,还原集内文件内容 = 第 N 轮快照树、目标树中不存在的文件被删除;分支指针与索引未被触碰(前后 `git rev-parse HEAD` 与 staged 内容一致);确认列表 = diff 路径全集且带归属标记。
- [ ] AC4 门禁生效:快照后注入外来写入(直接改文件),revert 被拦下并展示外来 diff;确认后按确认语义执行。
- [ ] AC5 audit 落账:每次 revert 落 `AuditKind` 新 kind 行(payload 含目标轮 seq、还原 paths/树、触发来源 UI)。
- [ ] AC6 回收:session 删除后伞 ref 消失、DB 行消失;快照对象可被 `git gc` 回收(验证 ref 删除后 `git cat-file` 不可达)。
- [ ] AC7 性能带内:轮末快照增量 ≤ N9 B2 既有 `finalize_turn_persist` 基线的约定余量(bench 数字不进门禁,落 perf-baseline.md 记录)。
- [ ] AC8 worker 交互:host 当轮 merge 进来的 worker 产物出现在该轮轮间 diff;revert host 到 N 不删 `worker/<run_id>` 分支,可重新 merge。
- [ ] AC9 写触发门:纯只读 session(无任何写信号)仅产生基线 1 行、**零新增 git 对象**(基线 tree 除外——评审修正措辞)、零额外扫描(集成测试断言)。
- [ ] AC10 基线可达(评审 P0):首轮含写入的 session,revert 到基线 = 回到会话开始前状态(首轮写入不被吃进基线)。
- [ ] AC11 D3 级联(依赖 OQ-B):edit_user_message 删尾重跑后,被删轮的 checkpoint 行同步消失,无孤儿行、无 PK 撞静默。

## Out of Scope(follow-up 候选)

- worker 自身链(按 run_id 各挂一条)——MVP 只做 host 主链。
- `evl` CLI 查询面(`evl checkpoint ...`)。
- N7 DiffView 增强(行级高亮 / side-by-side / 按文件折叠)。
- rewind 产品闭环(D3 消息回退与 N2 磁盘回退的一键联动)。
- 群聊 session 快照。
- 快照浏览/回放 UI(时间轴视图)。

## Open Questions(2026-09-20 群聊评审遗留 → 同日用户全部裁定,无阻塞项)

- ~~**OQ-A include-ignored 提级**~~ **裁定:不提级进 MVP**;PR0 期间顺手量写家族 tool audit path 的 gitignore 命中率,数据落 PRD 备未来翻案。**实测(2026-09-20,本机 DB `session_audit_events`,写家族 tool_executed(write_file/edit_file/apply_diff)带路径 19 条):gitignore 命中 0/16 = 0%**(16 = 19 − 项目目录已删的测试残留 2 − 非 git 仓库 1;唯一路径口径 0/6 同为 0%)——零命中不支持提级,维持裁定。
- ~~**OQ-B D3 事务内级联删**~~ **裁定:采纳**——`edit_user_message` 事务内跟随删 `turn_checkpoints` 行 + 改写 messages.rs:948 注释(PR1 交付,AC11 生效)。
- ~~**OQ-C busy 拒绝粒度**~~ **裁定:MVP 只拒本 session** busy;共享 cwd 他 session 场景留观察。
