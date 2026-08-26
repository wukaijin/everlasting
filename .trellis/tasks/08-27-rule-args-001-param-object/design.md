# Design — RULE-ARGS-001 参数对象重构

> 基于 [research/run-chat-loop-signature-inventory.md](./research/run-chat-loop-signature-inventory.md)。

## 总体形态

```
现有：run_chat_loop(38 乱序裸参) ──透传──▶ dispatch_tool_calls(33) / drive_turn(49) / finalize_turn(11)

目标：
  run_chat_loop(
      request: ChatLoopRequest,      // 每请求值，入口构建
      deps:    ChatLoopDeps,         // AppState 派生长寿命套件（churn 极低）
      extras:  CallerRole,           // 调用方角色旗标（skip 三兄弟 + resend_seq 等散件）
  )
  ├─▶ prepare_loop_state(deps/request 相关子集…) → LoopInit     （现状结构，出参不变）
  ├─▶ drive_turn(&deps 子集引用, &request 身份, &mut TurnCarry)  （49 参瘦身）
  ├─▶ dispatch_tool_calls(ctx: DispatchCtx{…})                  （与主链共享同一 suite 实例）
  └─▶ finalize_turn(&deps 落库双子集, …≤6 参)
```

核心原则：**对象按"生命周期 + 所有权来源"切，而不是按功能主题硬凑**。三类成员的生命周期截然不同：

| 对象 | 生命周期 | 来源 | 内容 |
|---|---|---|---|
| `ChatLoopDeps` | 整个 loop ≥ 跨请求 | `AppState` 字段 `.clone()`（chat.rs:214-223 就是它的手工雏形） | db, cancellations, session_active_request, read_guard, memory_cache, skill_cache, permission_asks, token(Cancellation), background_shells, stub_loaded, question_store, subagent_cache |
| `ChatLoopRequest` | 单次请求 | 入口逐值构建 | messages, tool_defs, provider, context_window, provider_id, rid, session_id, sink, resend_seq, max_turns, workflow_ctx(mut), group_chat_state, current_speaker |
| `CallerRole`（暂名） | 单次请求·常量 | 调用方身份决定 | is_worker 相关 suite（worker_run_id, run_grants, worker_catalog, worker_event_sink, forced_dispatch, system_prompt_override, worktree_override, project_main_override, app_data_dir, skip_persist）＋ skip_session_active / skip_cancellations |

（字段级分配明细以审查为准：边界 case 如 `subagent_cache` 是进程单例故入 Deps,`worker_catalog`/`worker_event_sink` 仅 worker 角色使用故入 Role;若审查认为 Role 过肥可再拆 `SubagentInvocation` 子结构。）

## 关键设计决策

### D1 命名与布局沿库内先例
- 入口聚合命名用 `XxxDeps`（照 `QueueDriverDeps`,chat.rs:979——它就是这套思想的第一个成功应用,本次是把同一个聚合推广回主线）;
- IPC 入口解析、随 request 穿线的域上下文用 `XxxCtx` 惯例（`WorkflowCtx`/`GroupChatCtx` 为证）;
- **禁用** `RunChatLoopArgs` 式命名;
- 新类型放 `agent/chat_loop/mod.rs` 或新 `suite.rs`, derive Clone(Debug 按);

### D2 每-turn 状态不进对象
`drive.rs:253-262` 的 10 个 `let mut` 重绑定名单（messages/seq/head_sha/system_prompt/permission_ctx/loop_window/loop_hit_count/last_usage_terminal/workflow_ctx/summary_anchor）与 cwd 是唯一真可变集合。方案：打包 `TurnCarry`（by-value 进、`DriveTurnOutcome` by-value 出,完全继承现行 LoopInit→outcome 管道)。这样可变性边界反而比现在更清楚:常量进引用、可变进 carry。
`dispatch_tool_calls` 返回 `DispatchOutcome` 携带 cwd 回传的现状照旧（tools.rs:39-44）。

### D3 迁移是"编译器驱动的改名",不是行为改写
RULE-A-006 保证签名变化生产可见、编译失败强制全量迁移。因此:
- 不做新旧并存桥接（spec 防分叉条款,v1/v2 共存明令禁止）;
- 不保留旧签名的 deprecated wrapper;
- 所有 `Arc/Mutex` 克隆次数、调用次序、锁获取点保持逐一对应——目标是 sed 式对应关系,而非借机"顺手优化"任何取值时机。

### D4 测试侧:一个 builder 收口 70 个位点
- `tests_common.rs`（已存在,cfg(test) 不入 clippy gate）提供 `ChatLoopTestFixture::{default(deps)…with(worker_role).with(group_chat)…}` 风格构造器 + 类型别名(缺省 suite);
- 70 处位参调用改为一行 fixture 调用 + 显式差异字段。特殊调用点(queue driver / 群聊 / worker 递归 / 三个本地 run_loop 包装器)各自显式覆盖;
- 这是唯一"位参 → 具名字段"的翻译发生地,翻译错误会在对应场景测试中被抓(每个调用点都有场景测试垫底)。

### D5 spec 旧警告的正面回应
`signature-run-chat-loop.md` 的"Do not refactor into a struct without re-running all integration tests + cargo check"是基于 23 参时代的判断,其兑现条件被采纳为本任务的一等验收:全量 `cargo test --lib` + `cargo clippy -D warnings` 必须跑通,并且任务收尾必须重写该文(演进表 :167 补 38 参终点行 + 新增 parameter-object 章节),否则本设计与 spec 冲突而不自知。

### D6 事故防线复检单（语义保真）
每个历史债务对应一条必须存活的传播链,D6 表:
| 约束 | 保真手段 |
|---|---|
| RULE-A-014 is_worker | `CallerRole.is_worker` 字段显式传递;回归测试不动 |
| RULE-A-015 skip_persist 门(16 写点) | 跟随调用点迁移,行级 diff 可查 |
| RULE-A-016 worker 隔离 | run_grants 归入 worker suite,worker 角色强制 Some |
| entry invariant(尾条新鲜未落盘) | messages 归 Request,首回合代码不动 |
| cache_control 断点顺序 | messages[0] 构造逻辑(init.rs)不改 |
| R3 终态 Done usage 去重 | sink 调用点照旧 |

## 分期

- **Phase 1（本任务）**：chat_loop.rs + drive.rs + tools.rs + init.rs 四文件 9 处 allow 归零;核心三签名 + prepare_loop_state + softcap/compaction/emit 边缘函数收编。
- **Phase 2（后续任务）**：`subagent/dispatch/*` 镜像链 6 处——届时直接消费 Phase 1 的 suite 成员,机械化高。
- **Phase 3（远期排队）**：providers CRUD(4)、trace.rs(3)、budget.rs(2)、db 层(5)等非 agent-loop 家族,各为独立小额任务。

## Rollback / 风险

- 每个 step 独立 commit（见 implement.md）,任一步炸可以直接 revert 该步而无需整体放弃;
- 最大风险 = 70 个测试位点的机械翻译错序 → 缓解:D4 单点翻译 + 每步跑定向过滤器;
- 次风险 = AppHandle-era 参数（worker_catalog/app_data_dir）的消费者遗漏 → 编译器暴露,不会静默;
- 行为漂移最终闸门 = D6 复检单逐条人工过一遍 diff。
