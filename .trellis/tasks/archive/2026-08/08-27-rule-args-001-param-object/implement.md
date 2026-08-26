# Implement — RULE-ARGS-001 执行计划

> 前置阅读顺序：prd.md → design.md → research/run-chat-loop-signature-inventory.md
> 每步一个 commit，独立可 revert；步骤内跑定向过滤器而非全量。

## Ordered Checklist

### Step 0 — 基线固化
- [x] 记录当前 HEAD 与基线测试结果：`cargo test -p everlasting --lib`（PKG_CONFIG_PATH 见 AGENTS.md）
- [x] 存档 grep 快照：四文件 allow 清单 + 全库 46 处分布（对照 AC 复核用）

### Step 1 — 新类型骨架（纯增量，不接线）
- [x] 定义 `ChatLoopDeps` / `ChatLoopRequest` / caller 角色结构（design.md §总体形态字段表）
- [x] 仅新增文件/类型，不改任何既有签名 → 本步编译零风险

### Step 2 — 测试构造 helper
- [x] `tests_common.rs` 增加 fixture builder（deps 缺省 + 差异覆盖），对照 tests_message_queue.rs 的 QueueDriverDeps 构造习惯
- [x] 不迁移任何调用点；helper 单元自验（类型可实例化）

### Step 3 — run_chat_loop 主签名替换（核心步）
- [x] 改 `run_chat_loop(request, deps, role)`；内部原 38 参的读取点逐一映射到新成员（机械对应，禁顺手改写取值时机——design D3）
- [x] 生产 4 调用点同步：chat.rs 直发 / run_queue_driver / group_chat_loop ×2 / subagent/dispatch/drive.rs worker 递归；chat_inner 的 AppState 解包代码升级为 `ChatLoopDeps::from(&state)` 型构造
- [x] 删除 chat_inner 与 emit_max_turns_terminal 的 too_many_arguments 豁免中属于本步的部分
- [x] 定向验证：`cargo test -p everlasting --lib agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`（RULE-A-014 回归）
- [x] commit（此后每步同式 commit）

### Step 4 — prepare_loop_state 与 dispatch_tool_calls 接线
- [x] init.rs 19 参瘦身（LoopInit 出参不变）
- [x] tools.rs 33 参 `dispatch_tool_calls` 收敛：新 ctx 结构复用 Step 1 同一 suite 实例（一石二鸟点）
- [x] `DispatchOutcome`（cwd 回传）语义不变
- [x] 定向验证：tools 相关过滤器 + parallel_dispatch + notifications 组

### Step 5 — drive_turn + TurnCarry
- [x] 49 参瘦身:`(&Deps 子集引用, &Request 身份, TurnCarry) -> Result<DriveTurnOutcome>`;drive.rs:253-262 十人 let mut 名单进 carry(见 design D2)
- [x] finalize_turn 11 参收编为 deps 子集引用(≤6 参,豁免删除)
- [x] attempt_summary_compaction(13)、ask_turn_limit_softcap(13)、emit_max_turns_terminal(8)收编
- [x] 定向验证:compaction_summary / softcap / turn_checkpoint / budget 过滤器

### Step 6 — 测试位点全量翻译
- [x] 70 处位参调用 → fixture 一行化;三个本地 run_loop 包装器重定向到 fixture
- [x] 其余 allow 清零核查(此时四文件应无残留)
- [x] 全量:`cargo test -p everlasting --lib`

### Step 7 — Spec 更新（Phase 3.3 合并执行）
- [x] 重写 signature-run-chat-loop.md:演进表补 38 参终点行,"Why 23 parameters"章节改为 parameter-object 形态;旧警告改写为"已兑现的重构契约(AC 为证)"
- [x] debt-linkage.md 无需改动(RULE-A-014/015/016 未变),如设计引入了新的传播约定则登记

### Step 8 — 收尾验收门
- [x] AC1: `cargo clippy -p everlasting --lib -- -D warnings`(PKG_CONFIG_PATH)
- [x] AC2/3: 全绿 + grep 断言(chat_loop* 内 allow 归零、无 v2 fork)
- [x] AC4: spec diff 就绪
- [x] AC5: `cargo test -p everlasting-remote`
- [x] DEBT.md 删除 RULE-ARGS-001 条目(闭合),优先级表同步 —— 在最终 check 通过后做

## Validation Commands

```bash
cd /usr/local/code/github/everlasting
export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
cargo test -p everlasting --lib                                # 全量(~1689)
cargo test -p everlasting --lib "agent::tests_agent_loop::"    # 定向示例
cargo clippy -p everlasting --lib -- -D warnings
cargo test -p everlasting-remote
```

## Risky Files / Rollback Points

| 文件 | 风险 | 回滚 |
|---|---|---|
| chat_loop.rs:319 起 | 核心 38 参映射错漏 | revert 单步 commit |
| drive.rs(drive_turn+TurnCarry) | 可变性边界搬移 | Step 5 独立提交 |
| tools.rs(dispatch_tool_calls) | 与主链共享 suite 的借用冲突 | 结构上按 D2 by-value 边界避让 |
| tests_common.rs fixture | 位序翻译错误(最大风险源) | 每场景测试垫底;坏一行修一行 |

## Pre-start Follow-up Checks

- [x] implement.jsonl / check.jsonl 已含真实条目(seed 行不计数)
- [x] 用户审查三件套后执行 `task.py start`
