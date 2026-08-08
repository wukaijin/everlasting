# A类单体重构:chat_loop 拆分

## Goal

把 `app/src-tauri/src/agent/chat_loop.rs`(5132 行)中 `run_chat_loop`(L306–4605,~4300 行单体,守 RULE-A-006)按 turn 阶段拆分:初始化段提取为阶段函数(输出 struct 解构模式),turn 循环体内提取 per-stage 函数(`ChatLoopState` 持有跨 turn 可变共享状态),最后子模块化(hub + `chat_loop/` 子目录)。行为零变化——A 类专项收官(剩余 4 个 A 类全部完成)。

## Background / 已确认事实

- `chat_loop.rs` 结构(5132 行):
  - `LlmRetrySink`(L146–195)、`user_message_matches`(L196–237)、`DdGuardHit` + `dd_guard_hit`(L238–305)
  - **`run_chat_loop`(L306–4605,~4300 行)**——`pub async fn`(注意:pub,非 pub(crate);**34 参数**签名 ~270 行)
  - 已独立函数:`build_turn_latency`(L4623)、`instant_delta_ms`(L4638)、`emit_persist_failure`(L4663)、`load_for_session`(L4677)、`finalize_pending_tool_results`(L4702)、`is_parallel_eligible`(L4795)、`delegation_max_concurrent_children`(L4860)、`classify_dispatch_batch`(L4912)
  - `mod tests`(L4943–5132,11 个测试,全测 `user_message_matches`/`dd_guard_hit` 两已独立函数;agent_loop_* 9 个集成测试在 `agent/tests.rs`,不在本文件)
- `run_chat_loop` 内部骨架(行号经实测核对):
  - **初始化段**(L306–1484):`_cancel_guard`@L580(RAII,留主体)、messages 准备(L591)/ seq(L619)、system_prompt(L993)、permission_ctx(L854)/ current_ctx(L764,原局部名 `turn_ctx`)构造、forced_dispatch 处理、loop_window/loop_hit_count 初始化(L1472–1482)——顺序代码,单向数据流
  - **turn 循环**(L1485–4605,`for turn in 1..=turn_limit`@L1485):每 turn 内:
    - per-turn 请求构造 + memory_recall 注入(L1485–1999)
    - LLM 流事件循环(`loop {`@L2001–~2720):事件 match 全分支(thinking/text/usage/tool_calls 累积)、turn 计时(Instant×5)
    - loop_detection(loop_window 维护 L2607–2612 + verdict L2612 + loop_hint 赋值 L2624)+ question_store 交互(穿插至 L3018)——边界非连续,按控制流切
    - **tool dispatch(L3021–4441,~1420 行,最大块)**:`if is_parallel_eligible`@L3021 分 L2 parallel path(L3021–3374,`FuturesUnordered`@L3088)+ Serial path(L3375–4441,`classify_dispatch_batch`@L3398 / `match dispatch_batch`@L3400,含 L3b 并发@L3498 + serial tool 执行含 `dispatch_subagent` 递归@L4128+)
    - turn 收尾(L4443–4558):loop_hint 追加@L4466、persist_turn@L4489/L4542、计时上报
  - **跨 turn 可变状态**(实测 L591–1482 pre-loop `let mut` + 循环内跨阶段读写):`messages` / `seq` / `current_ctx` / `last_cwd`(Option<PathBuf>) / `last_usage_terminal` / `permission_ctx` / `head_sha` / `system_prompt` / `loop_window` / `loop_hit_count` + 循环内初始化但跨阶段的 `loop_hint`(L2624 赋值→L4466 消费)/ `cancelled`(L1952)/ `had_error`(L1951)
  - **每 turn 局部状态**(不进 state):`stream` / `tool_calls` / `result_blocks` / `req` / `pending_thinking` / `text_parts` / `stop_reason` 等(~15 个)
- **决策(用户拍板,2026-08-08)**:混合方案——初始化段用 batch1 已验证的"阶段输出 struct 解构"模式(单向数据流);turn 循环体引入 `ChatLoopState`(原草案 `TurnCtx`,因与局部 `turn_ctx`@L764 命名碰撞而改名)持有跨 turn 可变共享状态,阶段函数收 `&mut ChatLoopState`。循环场景下散参数(34)不可行,state 是合理例外。
- 外部引用:run_chat_loop 被 `chat.rs`、`agent/subagent/dispatch.rs::drive_worker`、`group_chat` 等调用(符号引用);`agent_loop_*` 9 个集成测试在 `agent/tests.rs` 直接调用——**签名不能变**。
- 基线:`cargo test --lib` = 1662 全绿;`clippy --lib --tests` + `fmt --check` 零警告。

## Requirements

- R1 初始化段提取为阶段函数(输出 struct,调用处解构回局部变量);turn 循环体按阶段提取(drive_turn / loop 检测 / tool dispatch / 收尾),`ChatLoopState` 持有跨 turn 状态。每提取独立 commit、独立回滚。
- R2 按 Rust 2018 module 模式拆分:`chat_loop.rs` 保留 hub + `chat_loop/` 子目录;`run_chat_loop` 签名**不变**(pub async fn,34 参数冻结——调用方 chat.rs / dispatch.rs / tests.rs 零改动)。
- R3 行为零变化:turn 循环语义、事件 emit 顺序、persist 顺序、loop 检测/打断语义、日志均不变。
- R4 测试:内联 `mod tests`(11 个,测 `user_message_matches`/`dd_guard_hit` 已独立函数)迁出 tests_chat_loop.rs;`agent_loop_*` 9 个集成测试零改动全绿(1662 基线)。
- R5 被测私有项升 `pub(crate)`,禁止为可测性改公开 API。
- R6 文档引用 sweep:`chat_loop.rs:LINE` 行号引用改符号引用;archive/历史快照不改。
- R7 收尾 `cargo fmt` + `clippy --lib --tests` + `fmt --check` 零警告;`cargo test --lib` 全绿(1662 基线)。

## Acceptance Criteria

- [ ] AC1 `run_chat_loop` 主体 ≤ ~500 行:初始化调用序列 + turn 循环骨架(阶段函数调用)+ 收尾;每阶段一个具名函数调用。
- [ ] AC2 `ChatLoopState` 持有全部跨 turn 可变状态(pre-loop L591–1482 + 循环内跨阶段读写项);每 turn 局部状态留在阶段函数内部(不进 state)。
- [ ] AC3 每个提取/拆分 commit 单独可回滚(独立 commit)。
- [ ] AC4 `cargo test --lib` 全绿(1662 基线无减少,含 agent_loop_* 9 个集成测试);`cargo fmt --check` + `clippy --lib --tests` 零警告。
- [ ] AC5 `run_chat_loop` 签名不变(pub async fn,34 参数零改动);无其他公开 API 变更。
- [ ] AC6 非 archive 文档/注释无残留 `chat_loop.rs:LINE` 行号引用。
- [ ] AC7 锁序/emit 顺序核对:工具执行路径(parallel L3021–3374 / serial L3375–4441)、persist 顺序、loop 打断语义在提取前后一致(代码平移核对 + 9 个集成测试锁定)。

## Out of Scope

- 不改 `run_chat_loop` 签名 / 返回类型(34 参数债务冻结,另立任务)。
- 不重排 turn 内执行顺序 / 不优化逻辑(纯平移)。
- 前端、其他 A 类已全部完成,本任务为收官。
- 不新增 feature / 不修 bug / 不改行为。

## 已决决策(2026-08-08 用户拍板)

- D1 状态组织:混合方案——初始化段输出 struct 解构(batch1 模式);turn 循环 `ChatLoopState`(&mut 透传)。(原草案名 `TurnCtx`,因与局部 `turn_ctx`@L764 碰撞改名。)
- D2 提取粒度:初始化段 2 个函数(prepare_session / prepare_context)+ turn 循环 4 个阶段函数(drive_turn_llm / handle_loop_detection / dispatch_tool_calls / finalize_turn);dispatch 内 parallel+serial 合并为单函数(design §4 决策);每阶段独立 commit。
- D3 执行节奏:先提取(chat_loop.rs 内部)→ 再拆分(子模块化 + 测试迁出,单 commit)→ 文档 sweep。
