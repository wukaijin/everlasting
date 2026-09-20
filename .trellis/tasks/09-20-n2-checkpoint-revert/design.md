# Design — N2 checkpoint/revert 闭环(悬空快照路线)

> PRD:`prd.md`(本文件只讲技术设计;需求与 AC 以 PRD 为准)。
> 路线总原则:**快照是 DB 寻址的悬空 git 对象,不是分支上的 commit**——零侵入既有 git 工作流是构造性不变量,不是事后补救。

## 1. 架构与模块边界

```
app/src-tauri/src/
├── git/
│   └── checkpoint.rs        # 新模块:纯函数核心(无 DB/无 daemon 依赖)
│       ├── build_state_tree(repo) -> TreeOid        # 内存 index 快照(零接触)
│       ├── append_snapshot(repo, parent, tree) -> CommitOid  # 悬空 commit 链
│       ├── set_umbrella_ref(repo, session_id, tip)  # refs/everlasting/<sid>
│       ├── delete_umbrella_ref(repo, session_id)
│       ├── diff_snapshots(repo, a, b) -> DiffResult  # 复用 FileDiff/DiffResult
│       ├── compute_restore_set(repo, target_tree) -> Vec<RestorePath>
│       └── restore_paths(repo, target_tree, paths)   # 统一还原原语
├── db/
│   ├── checkpoint.rs        # turn_checkpoints 表 CRUD(挂 db/mod.rs)
│   └── migrations.rs        # 新表迁移
├── agent/chat_loop/drive.rs # 挂钩点:finalize_turn_persist 成功后 best-effort
├── commands/checkpoint.rs   # 三命令(daemon routes + Tauri 双注册)
└── agent/permissions/audit.rs  # AuditKind::CheckpointReverted
```

分层规则(与 worktree-contract 同风格):

- `git/checkpoint.rs` 只认 `&Path` / `git2::Repository` / Oid,不 import db/daemon——单测用临时仓库直测(PR0 全量在此)。
- daemon 接线层负责:session→repo 路径解析(`none` → `project.path`;`active`/`detached` → worktree 路径)、config gate、DB 行、audit。

## 2. 快照机制(git2 细节)

**build_state_tree**(核心不变量:零接触;**2026-09-20 群聊评审修正:原 `Index::new()` 伪代码不可行**——bare index 无仓库背书,`add_all` 直接 fail,libgit2 官方文档 + 仓内先例 `git/worktree/sweep.rs:44-46` 相反写法双证):

1. `Repository::open(target_path)`(`none` = project 根;worktree 模式开 worktree 路径,git2 经 `.git` file 解析到共享 odb/refs)。
2. `repo.index()` 取**真索引句柄**,内存中 `read_tree(HEAD tree)`(无 HEAD 空仓库跳过)→ `add_all(["*"], INDEX_ADD_DEFAULT, None)`——**全程不调 `index.write()`**:libgit2 索引操作纯内存不落盘,stat cache 更新不持久,不毁用户索引文件(AC1 三不变断言钉死:index 文件字节 + mtime + inode)。
3. `write_tree_to(&repo)` → TreeOid(只写 tree 对象,不写任何 index 文件)。
4. 幂等去重:内容寻址——状态未变 → 同 TreeOid,调用方复用上轮 commit(见 §4),零新对象。
5. 冲突态已知行为:用户仓库存在未解合并冲突(index EUNMERGED)时 `write_tree_to` 返回 Err——fail-open 记 warn,该轮无快照,整链停摆可观测(PR1 断言:连续冲突轮无新行)。
6. DB 写入语义钉死(评审补):`INSERT OR REPLACE`——同 seq 重试幂等,撞键是信号不是错误路径。

**语义边界(评审核准)**:`add_all` 不作用于已 tracked 文件——准确边界是 **untracked 且 gitignored 不入快照;tracked 后进 gitignore 仍收录且删除照传**。gitignored 路径双重不可见(diff 不显示 + revert 不还原)须在确认弹窗常驻脚注披露。

**append_snapshot**:`repo.commit(None, sig, sig, msg, &tree, &[parent])` —— `update_ref = None` 是悬空关键:不更新任何 ref。commit message 形如 `everlasting checkpoint <session_id> seq=<n>`(仅诊断用)。sig 用固定 daemon 身份(如 `everlasting-daemon`),避免读用户 git config(零接触延伸:不依赖也不污染用户配置)。

**伞 ref**:`refs/everlasting/<session_id>` → 链头 commit。session_id 是 UUID,合法 ref 名。每次 append 后 `reference_set_target`。作用:GC 保护(parent 链经链头可达)+ delete 时的单一清理点。worktree 模式经共享 common refs 落同一处,`none` 模式在 project 主仓库——两态一致。

**注意**:refs 命名空间在 `refs/heads` 之外,`git log <branch>` / `git branch` / `git status` 天然不可见;仅 `git for-each-ref` / `ls .git/refs/everlasting` 可见(名字自解释,可接受)。

## 3. 数据模型

新表(`db/migrations.rs` 按既有表重建/加列惯例):

```sql
CREATE TABLE turn_checkpoints (
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  seq        INTEGER NOT NULL,      -- 对齐 finalize_turn_persist 的轮末 seq;0 = 基线(首轮开始前)
  tree_sha   TEXT NOT NULL,
  commit_sha TEXT NOT NULL,
  created_at INTEGER NOT NULL,     -- unix ms,与既有表同风格
  PRIMARY KEY (session_id, seq)
);
```

- 不复用 `turn_trace`:其 UNIQUE 键含 `run_id`(worker 维度)、生命周期跟 usage 记录,checkpoint 是 host 主链独立生命周期;FK CASCADE 使 session 删除自动清行(伞 ref 清理另挂 `delete_session_inner`,见 §7)。
- **seq 语义钉死(评审修正)**:`turn_checkpoints.seq` = **轮末 assistant 行的 messages.seq**(fresh session 首条 user 行占 seq=0,两表共享数值空间但不撞 PK);基线行挂**首轮 user 行 seq**(通常 0)。TurnCard 入口仅挂轮末 assistant 卡(MessageList 对所有行含 user 卡盖 `data-seq`,入口判定必须区分)。
- 全量保留(用户裁定):无剪枝、无 TTL。

## 4. 快照数据流(每轮,写触发门)

挂钩点:`drive.rs` `finalize_turn_persist` **成功返回后**(同函数错误路径不挂钩——错误轮本就无完整终态,checkpoint fail-open 语义见 PRD R1):

```
轮首(loop 入口,首轮 user 行已落库、早于任何 tool 执行)——基线门(评审 P0 修正):
  session 无基线行 → 恒建基线(checkpoint seq = 首轮 user 行 seq,通常 0)
  [修正依据:原设计基线在轮末建,首轮写入被吃进基线,「回到会话前」不可达,
   且去重使 seq0/seq1 同 commit 自我掩盖——PRD 价值 2/3(首轮即唯一轮)失效]

轮末:finalize_turn_persist(...) 成功
  └─ best-effort(checkpoints_enabled && 非 group_chat && repo 可开):
       写信号门(2026-09-20 DB 实证修正,jjh-mono 8 轮纯查询 session 观察):
         write_signal = 本轮执行过 write 家族 tool(write_file/edit_file/apply_diff…)
                    || 本轮执行过 A2+ 判写的前台 shell
                    || 自上次快照以来 drain 过后台 shell 完成事件(L1 异步写)
                    || 自上次快照以来 merge_worker 落地过(worker 产物收编)
       无 write_signal 且已有基线 → 跳过(不扫描、不落行、零成本)
       有 → build_state_tree → 去重(tree==上轮? 复用 commit)→ append → 伞 ref → INSERT OR REPLACE
  └─ 任一步 Err → tracing::warn,轮收尾照常(fail-open;EUNMERGED 冲突态见 §2.5)
```

写信号来源全部是 loop 内既有信号(tool 注册表的写家族标记 / A2+ 分类结果 / L1 完成事件队列 / merge_worker 调用点),无新增探测。语义推论:只读轮无 checkpoint 行,其「本轮 diff」为空(该轮本来就没改东西);revert 目标 = 最近有行快照,链 parent 跳接逻辑与 fail-open 一致。

**快照粒度语义**(2026-09-20 追记):checkpoint 粒度 = 轮边界,非文件操作。同一轮内对任意文件的任意多次写,只产生**一次**轮末快照,记录**净效果**(轮内中间态不进快照,操作过程归 ToolExecuted 审计层)。新增文件所在轮与后续每次变更所在轮各计一次;净效果为零的写轮仍落 DB 行(写轮恒有行,「本轮 diff」显示为空)但 tree 去重零新对象。

## 5. revert 数据流(UI 触发,两步命令)

**命令面**(daemon `routes/checkpoint.rs` + Tauri `commands/checkpoint.rs` 双注册,沿用 browse_dir 模式):

| 命令 | 入 | 出 | 说明 |
|------|----|----|------|
| `list_turn_checkpoints` | session_id | `Vec<{seq, prev_seq, created_at, files_changed}>` | files_changed = 与 prev_seq 快照 diff 的文件数(徽标用);**prev_seq = 单 SQL「前一个存在行」**(写触发门稀疏链下字面 seq-1 必挂,评审修正),list 回传供 diff/文案用 |
| `get_turn_checkpoint_diff` | session_id, seq | `DiffResult` | `diff_snapshots(prev_seq → seq)`;**基线行无 diff 入口**(vs 空树会把整仓列成新增) |
| `revert_to_checkpoint_preview` | session_id, target_seq | `RevertPreview` | 见下 |
| `revert_to_checkpoint_execute` | session_id, target_seq, preview_token | `RevertResult` | dangerous 语义,前端确认后调 |

**破链降级(评审补)**:DB 行在而 git 对象不在(如用户手动 gc / ref 被清)→ 三命令统一返回 `CheckpointBroken`,UI 隐藏入口,warn 日志;不 panic 不半渲染。

**RevertPreview**:

```rust
{
  files: Vec<{ path, status,           // RestorePath 集内逐文件
               attribution: ToolWritten | ShellWrite | Unknown }>,
  foreign_delta: Option<Vec<FileDiff>>, // 当前态 ≠ 链头快照的部分(外来/轮间隙写入)
  target_seq, target_created_at,
}
```

- 还原集 = `compute_restore_set(target_tree)`:`diff(当前态树, target_tree)` 路径全集(内容异者 checkout 内容;目标树无者标记删除)。
- **门禁 + TOCTOU 双窗口封堵(评审修正)**:preview 时重算 `build_state_tree` 与链头 `tree_sha` 比对,记 `gate_tree_oid`;`RevertPreview` 回传 `preview_token = hash(target_tree_oid, gate_tree_oid)`。不一致(foreign_delta 非空)仅当 preview 所见。execute 入参带 `preview_token`,重验:重算 gate 树,`gate_tree_oid` 与 token 不符 → `Err(StalePreview)`(旧确认不得授权新还原集);foreign_delta 语义仅覆盖 preview 时刻。
- **busy 拒绝(评审修正)**:execute 拒绝本 session busy 中执行(agent 轮中 revert = 换 agent 脚下地板);共享 cwd 下**他 session** busy 是否也拒 = OQ3 留裁,MVP 先只拒本 session。
- 归属标记(增强,非过滤——PRD R2 修正案):写审计 ToolExecuted 行中 write 家族 tool(write_file/edit_file/apply_diff 等)的入参路径 → `ToolWritten`;A2+ 判定为写的 shell 命令 → 该轮标记 `ShellWrite`(路径不可得);其余 `Unknown`(可能是用户自改,共享 cwd 下是常态,badge 用中性色)。audit 查询走既有 `list_session_audit_events` 内部路径(非分页 UI 版)。

**execute 语义**(统一原语,两模式同一实现):

1. 重验:busy 拒绝 + `preview_token` 比对(防 preview→execute 之间新落外来写入,评审 TOCTOU 修正)。
2. `restore_paths`:还原集内逐 path——目标树有 → checkout blob 内容写盘;目标树无 → 删文件。**不碰分支、不碰索引、不碰还原集外的任何文件**。
3. 快照链**不回滚不截断**:revert 后下一轮照常 append(revert 本身也会出现在后续轮间 diff 里,历史可审计)。
4. `AuditKind::CheckpointReverted` 落账:payload 含 target_seq、还原 paths、foreign_delta 摘要、`gate_tree_oid`、触发来源 `ui`。
5. 返回 `RevertResult{restored: usize, deleted: usize}` → 前端 toast。

不做成 agent tool(不注册进工具面,PRD 约束);不走 ⑨ tool 权限流(它是用户发起的 dangerous UI 操作,模式对齐 `delete_worktree`:前端确认弹窗 + 后端执行 + 审计)。

## 6. 前端面

- **TurnCard**(或轮末 assistant 消息卡)菜单加两项:「本轮 diff」「回到此轮后」;seq 对齐 `data-seq`(N4 已命令化,取当前轮 seq 直通)。
- 「本轮 diff」→ `get_turn_checkpoint_diff` → 复用 DiffView 渲染(`DiffResult` 结构同构,`diff_against_branch` 的前端消费链零改动)。
- 「回到此轮后」→ preview → 确认弹窗(还原集列表 + 归属 badge + foreign_delta 警告区)→ execute → toast + audit 可查(AuditLogModal 现成)。
- 非 git 项目 / group_chat session:入口隐藏(命令返回 `CheckpointsUnavailable`,前端按能力隐藏)。

## 7. 生命周期与清理

- **session 删除**(`delete_session_inner`,commands/sessions.rs:319):FK CASCADE 清 `turn_checkpoints` 行;新增伞 ref 删除——**恒回退主仓库路径再试**(评审修正:worktree 共享 refs/ 物理落主仓库,detached 态 worktree 路径开不出来时只试 worktree 会静默跳过 = 共享 refs 永久泄漏、击穿 AC6;两路皆败才跳过 + warn)。ref 删除后 commit 链不可达,交 `git gc` 自然回收(AC6)。
- **D3 交互(评审裁决,依赖 OQ2 用户确认)**:`edit_user_message` 级联删尾后重跑,init.rs 的 max+1 现算使 seq 回落复用——幸存 checkpoint 孤儿行会在裸 INSERT 下 PK 撞静默零行 + 展示死分支谱系。修法:D3 既有事务内跟随级联删 `turn_checkpoints` 行,并同步改写 `db/sessions/messages.rs:948` 的「无他表引用」注释。
- **config gate**:`app_config` 键 `checkpoints_enabled`,fail-open 缺省开(仅字面 `"false"` 关,读法对齐 `sandbox_enabled` 单例模式);Settings 存储区块加开关(F3 面板旁)。

## 8. 关键权衡记录

| 权衡 | 选择 | 弃案与理由 |
|------|------|-----------|
| 快照载体 | 悬空 commit + 私有 ref | 分支 auto-commit:污染 `none` 档用户仓库历史、与用户暂存区打架(会话裁定) |
| 还原语义 | 全集还原 + 确认框归属标记 | 审计 path 过滤:shell 重定向写无路径属性,静默漏还原比过度还原危险(规划期修正) |
| SHA 存储 | 独立 `turn_checkpoints` 表 | `turn_trace` 加列:键含 run_id(worker 维度)、生命周期耦合 usage 记录 |
| revert 后链 | 不截断,继续 append | 截断回滚:丢审计历史;不截断则 revert 自身可审计 |
| 快照触发 | 写信号门(写 tool / A2+ 判写 shell / 后台 shell 完成 / worker merge) | 每轮无条件扫描:DB 实证(2026-09-20)纯查询 session 8 轮零文件变更,多数 session 写调用 ≤2——成本应与实际变更成正比,与对话长度解耦 |
| 路线本体 | 悬空 commit 链(2026-09-20 群聊评审裁决保留) | 机制分解后与"无链纯 DB 行"共享全部主体,链独有增量仅伞 ref 簿记 + prev_seq 单 SQL,砍链丢 PRD 价值 1/4 并堵死 rewind 联动;替代方案死因:stash 破 AC1、反向补丁死于二进制与脏工作区漂移 |
| gitignore | 尊重(add -A 语义) | force-add:会把 node_modules 类目录灌进快照;untracked+ignored 双重不可见已记 §2 边界并进确认弹窗脚注 |
| 签名身份 | 固定 `everlasting-daemon` sig | 读用户 git config:引入对用户配置的依赖(零接触原则) |

## 9. 兼容 / 回滚

- **零迁移风险**:新表纯增量,无既有表改列;旧 session 无 checkpoint 行 → 查询面返回空列表,UI 入口自然隐藏(渐进可用,无回填)。
- **功能回滚**:`checkpoints_enabled=false` 即停建快照;既有链与 ref 留置无害(不可见对象),session 删除时随清。代码整体回退的残留 = 已建 ref + 对象,同理无害,可 `git for-each-ref refs/everlasting` 手清。
- **性能带内**:B2 已证 DB 侧可忽略;树构建成本进 N9 bench 基建新增 B6 目标(`build_state_tree` criterion,合成仓库 100/1k/10k 文件三档),数字落 `spec/backend/perf-baseline.md`,不进门禁(既有裁定)。

## 10. 测试策略

- **PR0 单测**(`git/checkpoint.rs`,临时仓库,全量不变量):零接触(index 文件字节不变/HEAD 不动/status 输出一致)、去重(空轮同 tree)、untracked 收录、删除传播、restore 语义(改/删/还原集外不动)、伞 ref 生命周期、worktree 句柄建 ref 落共享 refs。
- **PR1 集成**:drive 挂钩 fire(既有 agent loop 测试 harness)、fail-open(shutdown 注入失败轮收尾不受影响)、基线 seq=0、delete_session 清 ref + 行。
- **PR3 e2e**(RULE-TEST-001 route-mock 确定性档):TurnCard 入口渲染、确认弹窗文件列表 + foreign 警告区、非 git session 入口隐藏。
- AC↔测试映射:AC1-AC6 落 PR0/PR1 单测与集成;AC7 落 B6 bench;AC8 落 PR1 worker merge 场景集成测试。
