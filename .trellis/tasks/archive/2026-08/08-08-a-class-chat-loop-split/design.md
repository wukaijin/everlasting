# A类单体重构:chat_loop 拆分 — Design

> **行号锚点均经实测核对(2026-08-08,commit ~28ef6b7)。** `for turn` 在 **L1485**,函数体结束 **L4605**。下文每阶段标注的 `L###` 为实测结构锚点(非初稿估算),实施时以代码为准但可用作切分起止。

## 1. 目标形态

`run_chat_loop`(L306–4605,~4300 行)→ 初始化调用序列 + turn 循环骨架(阶段函数调用)+ 收尾(≤ ~500 行)。

```
run_chat_loop(34 参数签名冻结)
  ├─ [主体] _cancel_guard = CancellationGuard { ... }     // RAII,留主体(L580)
  ├─ prepare_session(...) -> Result<SessionPrep, EarlyReturn>      // 初始化段 1(guard 构造之后)
  ├─ prepare_context(...) -> CtxPrep                               // 初始化段 2(含 forced_dispatch)
  ├─ ChatLoopState::new(...)                                       // 跨 turn 状态集中
  └─ for turn in 1..=turn_limit {              // L1485
       ├─ drive_turn_llm(&mut ChatLoopState, ...) -> TurnStreamOutcome  // LLM 流事件循环
       ├─ handle_loop_detection(&mut ChatLoopState, ...)                 // loop 检测 + question_store
       ├─ dispatch_tool_calls(&mut ChatLoopState, ...)                   // parallel L2 + serial(L3b 并发)
       └─ finalize_turn(&mut ChatLoopState, ...)                         // persist + 计时
     }
```

## 2. 初始化段提取(输出 struct 解构模式,batch1 已验证)

| 函数 | 对应原段(实测) | 输出 struct | 说明 |
|---|---|---|---|
| `prepare_session` | L581–~700(`_cancel_guard` 构造**之后**起) | `SessionPrep { messages, seq, ... }` | messages 准备(load_for_session / resend / compaction 检查)、seq 计算。**边界铁律:函数起点必须在 `_cancel_guard`(L580)构造之后**——guard 是 RAII(Drop 时 `cancellations.remove(&rid)`),移进子函数会在子函数返回时提前触发 Drop,破坏取消语义 |
| `prepare_context` | L700–1484(for 循环前) | `CtxPrep { current_ctx, permission_ctx, system_prompt, last_cwd, head_sha, last_usage_terminal, loop_window, loop_hit_count, forced 相关 }` | worktree/cwd、PermissionContext(L854)、system_prompt 组装(L993)、forced_dispatch 处理、loop 窗口初始化(L1472–1482) |

**RAII / 借用敏感项留主体(不进任何子函数)**:`_cancel_guard`(L580,CancellationGuard)、`cancellations.clone()`(L580 入参)。prepare_session 从 L581 起提取。

提取时以实际代码为准:函数边界按"输出被后续消费的局部变量集合"划定,调用处解构回原变量名(后续代码零改动)。

## 3. ChatLoopState(跨 turn 可变共享状态)

> **命名变更:原草案 `TurnCtx` 与函数体内既有局部变量 `turn_ctx`(L764,`let mut current_ctx = turn_ctx;` 的右值,类型 `ToolContext`)碰撞,易混淆。新 struct 改名 `ChatLoopState`,提取时 `turn_ctx` 局部保留不动。**

字段按实测锚点核对(pre-loop `let mut` L591–1482 + 循环内跨阶段读写项):

```rust
pub(crate) struct ChatLoopState {
    // —— pre-loop 初始化,循环内累积(L591–L1482)——
    pub(crate) messages: Vec<ChatMessage>,              // L591
    pub(crate) seq: i64,                                // L619
    pub(crate) current_ctx: ToolContext,                // L764(原局部 turn_ctx)
    pub(crate) last_cwd: Option<PathBuf>,               // L765  ⚠️ 类型是 Option<PathBuf>,非 String
    pub(crate) last_usage_terminal: Option<crate::llm::types::TokenUsage>, // L777
    pub(crate) permission_ctx: PermissionContext,       // L854
    pub(crate) head_sha: String,                        // L986
    pub(crate) system_prompt: String,                   // L993
    pub(crate) loop_window: VecDeque<loop_detection::ToolCall>, // L1472  ⚠️ 原草案漏列,handle_loop_detection 核心
    pub(crate) loop_hit_count: u32,                     // L1482
    // —— 循环内初始化,但跨阶段读写(必须进 state)——
    pub(crate) loop_hint: Option<String>,               // L2624 赋值(handle_loop_detection 段)→ L4466 消费(finalize 段)
    pub(crate) cancelled: bool,                         // L1952 初始化 → 事件循环/dispatch 多处写
    pub(crate) had_error: bool,                         // L1951 初始化 → 事件循环写
}
```

> 字段清单以实测为准;提取每个阶段函数时,grep 该函数体内每个 `let mut` / 跨阶段读写变量,确认归属(`state` 字段 vs 每 turn 局部入参 vs 外部借用透传)。

**阶段函数入参三分类**(提取时按实际借用定):
- `&mut ChatLoopState` —— 跨 turn 可变共享(上表字段)
- 每 turn 局部入参/出参 —— `TurnStreamOutcome` 等具名 struct(`text_parts` / `tool_calls` / `stop_reason` / `thinking 状态` / turn 计时 `Instant`)
- 外部借用透传 —— `db` / `sink` / `rid` / `provider` / `read_guard` / `memory_cache` / `skill_cache` / `permission_asks` / `background_shells` 等签名参数(按需 `&`/`&mut`)

阶段函数形态:`async fn drive_turn_llm(state: &mut ChatLoopState, locals: ..., externals: ...) -> TurnStreamOutcome`。

## 4. turn 循环阶段提取(行号均为实测结构锚点)

| 函数 | 对应原段(实测) | 行数 | 说明 |
|---|---|---|---|
| `drive_turn_llm` | L1485–3018 | ~1500 | per-turn 请求构造(memory_recall 注入 L1617–1824)+ stream 构造(L1981)+ 事件循环(`loop {`@L2001,`match event_result`,thinking/text/usage/tool_calls 累积)→ `TurnStreamOutcome { text_parts, tool_calls, stop_reason, thinking 状态, turn 计时 Instant×5 }`。**注:此段较大,含 per-turn memory_recall 注入,提取时若 >800 行可考虑 memory_recall 子段再内聚** |
| `handle_loop_detection` | L2607–3018(与 drive 末尾交错) | — | ⚠️ **真实结构非连续**:`loop_window` 维护(L2607–2612)+ `loop_verdict`(L2612)+ `loop_hint` 赋值(L2624)在事件循环**之后**、dispatch **之前**;question_store 交互(打断/提示)穿插到 L3018。提取时这段与 drive_turn_llm 边界需按"事件循环结束后、dispatch 开始前"的实际控制流切,不能按行号硬切。`loop_window`/`loop_hint` 进 ChatLoopState |
| `dispatch_tool_calls` | L3021–4441 | ~1420 | **最大块**,分 2 commit 见下 |
| `finalize_turn` | L4443–4558 | ~115 | turn 收尾:loop_hint 追加(L4466)、persist_turn(L4489/L4542)、persist_turn_cwd(L4511/L4564)、计时上报 |

### dispatch_tool_calls 拆分(2 commit)

| 子段 | 实测行号 | 说明 |
|---|---|---|
| parallel L2 | L3021(`if is_parallel_eligible`)– L3374(`}` close) | 只读批并行,`FuturesUnordered`@L3088 |
| serial | L3375(`} else {`)– L4441(`}` close `match dispatch_batch`) | `classify_dispatch_batch`@L3398 / `match dispatch_batch`@L3400;含 L3b 并发(`FuturesUnordered`@L3498)+ serial for 循环(各 tool 执行含 `dispatch_subagent` 递归调用 L4128+) |

**parallel / serial 是否各自独立函数?** 建议先合并为单个 `dispatch_tool_calls(&mut ChatLoopState, tool_calls, externals...) -> DispatchOutcome`,内部保留 `if is_parallel_eligible { ... } else { ... }` 结构(原样平移)。理由:两段共享大量外部借用(provider/db/sink/rid/read_guard/...),拆成两个函数会让入参清单重复一遍,收益不大;合并后函数虽 ~1420 行,但后续可在子模块内进一步分解(见 §5 文件布局,tools.rs 内可再拆 execute_parallel/execute_serial 私有函数)。**此决策记入 design,不留 implement 即兴。**

每阶段函数内部保持原代码逐行顺序;`ChatLoopState` 字段访问替代局部变量(机械替换,禁止顺手重构)。

## 5. 文件布局(Rust 2018 module 模式)

参照前三专项惯例(`subagent/dispatch.rs` + `subagent/dispatch/`、`subagent/sink.rs` + `subagent/sink/`):

```
agent/chat_loop.rs          # hub:run_chat_loop 骨架 + 已独立函数(load_for_session/build_turn_latency 等 8 个)+ LlmRetrySink + dd_guard + re-export
agent/chat_loop/init.rs     # SessionPrep / CtxPrep + prepare_session / prepare_context + ChatLoopState
agent/chat_loop/turn.rs     # drive_turn_llm + handle_loop_detection + finalize_turn(+ TurnStreamOutcome)
agent/chat_loop/tools.rs    # dispatch_tool_calls(parallel L2 + serial;内部可再拆 execute_parallel/execute_serial 私有函数)
agent/tests_chat_loop.rs    # 内联 mod tests 迁出(L4943–5132,11 个测试,全测 user_message_matches / dd_guard_hit 两已独立函数,无 run_chat_loop 内部依赖 → 纯移动,无可见性风险)
```

- hub re-export 全量(对照 dispatch/anthropic/sink 惯例:`#[allow(unused_imports)] pub(crate) use ...`);`run_chat_loop` 定义留 hub(签名冻结)。
- 子模块访问 hub 私有项需 `pub(crate)`(对照 sink 可见性模式)。
- `tests_chat_loop.rs` 迁出后 `use super::chat_loop::*` 经 hub re-export 解析(测试只引用 `user_message_matches` / `dd_guard_hit`,两者已是独立 `fn`,迁出零风险)。

## 6. 风险与回滚

| 风险 | 缓解 |
|---|---|
| ~4300 行单体提取行为漂移(turn 语义) | 9 个 agent_loop_* 集成测试 + 1662 基线全量验证;每提取 commit 独立回滚 |
| ChatLoopState 字段遗漏(跨 turn 状态漏进/误进) | pre-loop `let mut` 实测清单(L591–1482,见 §3)为锚;提取时 grep 核对每个跨 turn 变量的所有读写点;循环内初始化但跨阶段读写项(`loop_hint`@L2624→L4466 / `cancelled` / `had_error`)必须进 state |
| **命名碰撞**(turn_ctx 局部 @L764 vs struct) | struct 命名 `ChatLoopState`(已改);`turn_ctx` 局部保留不动 |
| RAII / 借用冲突(CancellationGuard、permission_ctx 借用) | `_cancel_guard`(L580)及 `cancellations.clone()` 留主体;prepare_session 从 L581 起;state 字段 owned |
| handle_loop_detection 边界非连续(loop_window 操作在事件循环后) | 不按行号硬切;按控制流("事件循环结束后、dispatch 开始前")切;`loop_window`/`loop_hint` 进 state 跨阶段传递 |
| 签名冻结与 state 引入冲突 | run_chat_loop 签名不变;state 在函数内部构造(参数不动) |
| 测试迁出可见性(glob 只传播 pub) | 文件级显式 import + `#[allow(unused_imports)]`(sink 专项 gotcha) |
| 大规模文件移动(拆分 commit 涉及 ~3500 行) | 拆分前提取完成(所有阶段函数已在 chat_loop.rs 内验证绿),拆分 commit 纯移动 + hub re-export |

**回滚**:每提取 commit 独立 `git revert`;拆分 commit 前 `cargo check` + 全量测试验证。

## 7. 明确不做

- 不改 `run_chat_loop` 签名(34 参数债务冻结)。
- 不重排 turn 内阶段顺序 / 不优化(纯平移)。
- 不拆 `group_chat.rs` / `chat.rs`(非 A 类范围)。
