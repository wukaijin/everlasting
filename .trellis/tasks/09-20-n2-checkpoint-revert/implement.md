# Implement — N2 checkpoint/revert 闭环

> 依赖:PR0 → PR1 → PR2 → PR3 串行(后三级分别依赖前级的模块/接线/读面)。
> **PR0 入场券(群聊评审 2026-09-20)**:design §2 快照机制已被评审证伪重写(`repo.index()` 句柄 + 无 `index.write()`,非 `Index::new()`)——实现一律以修正后 design 为准,警惕仓内 `sweep.rs:46` 的 `index.write()` 先例误导。
> 每级收尾跑该级验证命令;PR3 后跑全量终检(Final pass)。

## PR0 — `git/checkpoint.rs` 纯函数核心 + B6 bench(中立基建,零接线)

- [ ] 新建 `app/src-tauri/src/git/checkpoint.rs`,挂 `git/mod.rs`:design §2 全部原语(`build_state_tree` / `append_snapshot` / `set_umbrella_ref` / `delete_umbrella_ref` / `diff_snapshots` / `compute_restore_set` / `restore_paths`)
- [ ] `diff_snapshots` 复用 `git/diff.rs` 的 `FileDiff`/`DiffResult`(如需抽 `diff_tree_to_tree` 底层 helper,在 diff.rs 内加,不改 `diff_against_branch` 行为)
- [ ] 单测(临时仓库,design §10 PR0 清单):零接触(**index 字节+mtime+inode 三不变**,AC1 升级版)、去重、untracked 收录、删除传播(**含嵌套目录删除**,评审补)、**tracked 后进 gitignore 仍收录**(评审补)、**EUNMERGED 冲突态断言 Err 不 panic**(评审补)、restore 语义(改/删/还原集外不动)、伞 ref 生命周期、worktree 句柄建 ref 落共享 refs
- [ ] B6 bench:`build_state_tree` criterion 目标进既有 bench 基建(bench feature 门内),合成仓库 100/1k/10k 文件三档
- [ ] 基线数字落 `spec/backend/perf-baseline.md`(B6 段;不进门禁)
- [ ] 顺手数据(供 OQ-A 裁决):写家族 tool audit path 的 gitignore 命中率统计(一次性脚本,结果记 PRD OQ-A)

验证:
```bash
cargo test -p everlasting --lib "git::checkpoint::"    # PKG_CONFIG_PATH 见 AGENTS.md
cargo bench --features bench -- b6_                    # 出数即过,数字记录不门禁
```

## PR1 — daemon 接线:挂钩 + 表 + 清理 + config gate

- [ ] 迁移:`turn_checkpoints` 表(design §3),挂 `db/migrations.rs` 既有惯例
- [ ] `db/checkpoint.rs` CRUD(`upsert_checkpoint` / `latest_checkpoint` / `list_checkpoints` / cascade 靠 FK)
- [ ] drive.rs 挂钩:轮末 `finalize_turn_persist` 成功后 best-effort 快照,**写信号门**(design §4:写家族 tool / A2+ 判写前台 shell / 后台 shell 完成事件 / worker merge);**基线钩在轮首**(loop 入口、首轮 user 行落库后、早于 tool 执行——评审 P0 修正);群聊 session 与非 git 跳过;**EUNMERGED 连续冲突轮无新行可观测断言**(评审补)
- [ ] 写信号接线:tool 注册表写家族标记、A2+ 分类结果、L1 完成事件队列、merge_worker 调用点四处信号汇到挂钩点
- [ ] config gate:`app_config` 键 `checkpoints_enabled`,fail-open 缺省开(读法对齐 `sandbox_enabled` 单例模式);Settings 存储区块开关
- [ ] `delete_session_inner` 清伞 ref(**恒回退主仓库路径再试**,两路皆败才跳过 + warn——评审修正 detached 泄漏);FK CASCADE 清行验证
- [ ] D3 级联删(依赖 OQ-B 裁定):`edit_user_message` 事务内跟随删 `turn_checkpoints` 行 + 改写 `db/sessions/messages.rs:948` 注释;集成测试 AC11
- [ ] 集成测试:挂钩 fire / fail-open / 基线轮首可达(AC10) / 写触发门(AC9:纯只读 session 仅基线行) / worker merge 后 host 下轮快照收编(AC8) / delete 清理(AC6)

验证:
```bash
cargo test -p everlasting --lib
cargo clippy -p everlasting --lib -- -D warnings
```

## PR2 — 读面:三命令 + TurnCard 入口 + DiffView

- [ ] `commands/checkpoint.rs` + daemon `routes/checkpoint.rs` 双注册:`list_turn_checkpoints`(**回传 prev_seq,单 SQL「前一个存在行」**)/ `get_turn_checkpoint_diff`(design §5 契约;不可用态返回 `CheckpointsUnavailable`,破链返回 `CheckpointBroken`——评审补降级)
- [ ] 前端:TurnCard 菜单「本轮 diff」→ `get_turn_checkpoint_diff` → DiffView 渲染(`DiffResult` 同构消费,评审 verified 前端链零改动);**文案用 prev_seq 语义「自上一快照以来」**(写触发门稀疏链下漏判轮改动会归位到后续写轮,评审修正);基线行无 diff 入口;**入口仅挂轮末 assistant 卡**(user 卡也有 data-seq,评审修正)
- [ ] 能力隐藏:非 git / group_chat / 无行 session / 破链 入口不渲染
- [ ] vitest:store 层命令封装 + 入口条件渲染单测

验证:
```bash
cd app && pnpm test
cd app/src-tauri && cargo test -p everlasting --lib "checkpoint"
```

## PR3 — revert:preview/execute + 确认弹窗 + audit + e2e

- [ ] `revert_to_checkpoint_preview` / `revert_to_checkpoint_execute` 双注册(design §5;**preview_token = hash(target_tree_oid, gate_tree_oid)**,execute 重验 `Err(StalePreview)`;**本 session busy 拒绝**——TOCTOU 双窗口评审修正)
- [ ] 归属标记:审计 ToolExecuted 查询(write 家族 tool 路径 → ToolWritten;A2+ 判写 shell → ShellWrite;余 Unknown)
- [ ] `AuditKind::CheckpointReverted`(payload:target_seq / paths / foreign 摘要 / **gate_tree_oid** / 来源 ui)
- [ ] 前端:「回到此轮后」→ preview → 确认弹窗(**评审重排:foreign 警告区仅非空渲染;按钮文案带还原文件数;Unknown badge 中性色——共享 cwd 下是常态;foreign 文案「非本会话快照内变更」;gitignore 双重不可见常驻脚注**;不做逐文件勾选——勾选会造出任何轮次都不存在过的状态)→ execute → toast
- [ ] e2e(app/e2e/,route-mock 确定性档):入口渲染(仅轮末 assistant 卡) / 弹窗列表 / foreign 警告区 / 非 git 隐藏
- [ ] revert 后链不截断的回归测试(revert → 新一轮 → 轮间 diff 含 revert 影响)

验证:
```bash
cd app && pnpm test && pnpm test:e2e
cd app/src-tauri && cargo test -p everlasting --lib
```

## 终检(Final pass,PR3 合入前)

- [ ] 全量:`cargo test -p everlasting --lib` + `cargo test -p everlasting-remote` + `cd app && pnpm test` + `pnpm test:e2e` + `pnpm build` + CI 门 `cargo check --features bench --benches`
- [ ] spec 更新(Phase 3.3):worktree-contract 或新 `checkpoint-contract` spec(零接触不变量 + 命令契约 + revert 语义);`docs/DAEMON-API.md` 命令节;ROADMAP §1.2 N2 行 + BACKLOG 附录 B N2 划线
- [ ] live 冒烟:`scripts/turn-smoke.sh` 单轮后查 `turn_checkpoints` 行 + `.git/refs/everlasting/`;revert 一轮实走

## 风险文件与回滚点

| 风险 | 位置 | 缓解 |
|------|------|------|
| **设计文档自身有错**(评审新增类) | design.md(已修正 §2) | PR0 入场券:以修正后 design 为准;实现者遇到与 sweep.rs 先例冲突时以 AC1 三不变断言为最终裁判 |
| drive.rs 挂钩影响轮收尾主路径 | `agent/chat_loop/drive.rs`(finalize 调用点) | best-effort 包裹 + fail-open 单测;出问题 config 关门 |
| add_all 删除传播语义(libgit2 版本差异) | `git/checkpoint.rs` | PR0 专项单测钉死(删文件跨轮,含嵌套目录);不符则补 `update_all` 显式两段 |
| 真索引误碰 / stat cache 毁损 | `git/checkpoint.rs` | AC1 三不变断言(index 字节+mtime+inode)进 PR0 门禁 |
| D3 事务级联删引入回归 | `db/sessions/messages.rs` | 既有 D3 测试族全量回归 + AC11 新断言;OQ-B 裁定后才做 |
| worktree/none 双态 ref 落点漂移 | checkpoint.rs + delete 链 | PR0 worktree 句柄单测 + PR1 delete 恒回退主仓库再试(评审修正) |

回滚:PR0/PR2 纯增量可独立回退;PR1 挂钩出问题先 `checkpoints_enabled=false`(零代码回滚),再回退代码(残留 ref/对象无害,可手清)。
