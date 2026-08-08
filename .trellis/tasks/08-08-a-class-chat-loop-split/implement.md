# A类单体重构:chat_loop 拆分 — Implement

## 执行策略

同前三专项:先"提取"(chat_loop.rs 内部函数化,每阶段独立 commit + 中间全量 cargo test)→ 再"拆分"(子模块化 + 测试迁出,单 commit)→ 文档 sweep。turn 循环内提取以 `ChatLoopState` 为状态载体,提取前先建 ChatLoopState(空壳或逐步填充)。

> **行号锚点均经实测核对(见 design §1/§3/§4)。** 切分时以代码控制流为准,行号仅作起止参考。

## 验证命令

```bash
cd /usr/local/code/github/everlasting/app/src-tauri
export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
cargo check --lib                          # 每个提取 commit 后
cargo test --lib                           # 每个提取 commit 后:全量 1662 基线(含 agent_loop_* 9 个)
cargo fmt                                  # 每 commit 前
cargo clippy --lib --tests && cargo fmt --check   # 拆分 commit 后:零警告终验
```

## Ordered Checklist

### Phase A:提取(chat_loop.rs 内部,每步独立 commit)

1. **[commit] 建 `ChatLoopState` + 提取 `prepare_session` + `SessionPrep`** — 初始化段 1(L581 起,`_cancel_guard`@L580 构造**之后**):ChatLoopState struct 定义(design §3 字段,提取时按实测核对增删);messages 准备(L591)/ seq 计算(L619)/ resend / compaction 检查提取。**RAII guard(`_cancel_guard`)及 `cancellations.clone()` 留主体,不进子函数。**
2. **[commit] 提取 `prepare_context` + `CtxPrep`** — 初始化段 2(L700–1484):worktree/cwd、permission_ctx(L854)、system_prompt(L993)、forced_dispatch 处理、loop_window/loop_hit_count 初始化(L1472–1482)。
   - **Gate**:初始化段主体应只剩 guard 构造 + 调用序列;`cargo test --lib` 全绿
3. **[commit] 提取 `drive_turn_llm` + `TurnStreamOutcome`** — per-turn 请求构造 + LLM 流事件循环(L1485–~2720,`loop {`@L2001):事件 match 全分支 + turn 计时(Instant×5)。`&mut ChatLoopState` 参数化(本 commit 只接入 messages/seq 等已被消费的字段,其余后续接入)。
4. **[commit] 提取 `handle_loop_detection`** — loop_window 维护(L2607–2612)+ loop_verdict(L2612)+ loop_hint 赋值(L2624)+ question_store 交互(L2624–3018)。⚠️ **边界按控制流切(事件循环结束后、dispatch 前),不按行号硬切**;`loop_window`/`loop_hint` 进 ChatLoopState。
5. **[commit] 提取 `dispatch_tool_calls`(整体,parallel + serial 合并)** — L3021(`if is_parallel_eligible`)– L4441(`}` close match dispatch_batch):内部保留 `if is_parallel_eligible { ... } else { ... }` 结构原样平移;parallel@L3021–3374,serial@L3375–4441(含 `classify_dispatch_batch`@L3398、L3b 并发@L3498、serial for 含 `dispatch_subagent` 递归@L4128+)。**合并为单函数(design §4 决策),tools.rs 内可后续再拆 execute_parallel/execute_serial 私有函数。**
   - 若该 commit 过大(diff >1500 行),可拆 5a(parallel 段接入空 serial stub)+ 5b(serial 段填充)两 commit,但函数签名只定一次。
6. **[commit] 提取 `finalize_turn`** — turn 收尾(L4443–4558):loop_hint 追加(L4466)、persist_turn(L4489/L4542)、persist_turn_cwd(L4511/L4564)、计时上报。
   - **Gate**:run_chat_loop 主体应只剩 guard 构造 + 初始化调用 + turn 循环骨架 + 收尾(≤ ~500 行);`cargo test --lib` 全量全绿(1662)

### Phase B:拆分(单 commit)

7. **[commit] 子模块化** — 按 design §5 布局:init.rs / turn.rs / tools.rs;hub 保留 run_chat_loop 骨架 + 8 个已独立函数 + LlmRetrySink + dd_guard + re-export;内联 mod tests(L4943–5132,11 个测试,测 `user_message_matches`/`dd_guard_hit` 已独立函数)迁出 tests_chat_loop.rs。
   - 验证:`cargo check --lib`(chat.rs / dispatch.rs / tests.rs 调用零改动)→ `cargo test --lib`(1662 全绿)→ clippy + fmt

### Phase C:文档 sweep

8. **[commit] 引用 sweep** — grep `chat_loop.rs:[0-9]` 于 `.trellis/spec/`、`docs/`、`app/src-tauri/src/`(排除 `/_reviews/|/decisions-20|/archive/|/_deprecated/`),行号引用改符号引用(`chat_loop.rs::run_chat_loop` / `turn.rs::drive_turn_llm` 等)。
   - 残留核验:上述 grep 应无输出

### Phase D:收尾

9. 终验:`cargo test --lib` 全绿(1662)+ `cargo clippy --lib --tests` + `cargo fmt --check` 零警告
10. squash merge 回 main → `task.py archive` → 复验 `cargo test --lib`

## 风险提示(design §6)

- 提取时保持 turn 内逐行顺序;`ChatLoopState` 字段访问替换局部变量为机械替换,禁止顺手重构
- pre-loop `let mut` 实测清单(L591–1482,见 design §3)为跨 turn 状态锚点;循环内初始化但跨阶段读写项(`loop_hint`/`cancelled`/`had_error`)必须进 state;每提取一个阶段函数后 grep 核对变量读写点归属
- RAII guard(`_cancel_guard`@L580)/ 借用敏感项留主体(不尝试移入函数);prepare_session 从 L581 起
- **命名碰撞**:`turn_ctx` 局部(@L764)与新 struct 不同名(ChatLoopState),提取时勿改局部名
- handle_loop_detection 边界非连续,按控制流切不按行号
- 每 commit diff 应只含"该阶段代码位移 + ChatLoopState 字段增减",出现无关改动立即回退

## Review Gates

- [x] 用户评审 prd/design/implement 通过(行号/字段已修正)
- [ ] `task.py start` 后实施
- [ ] 每提取 commit 独立可回滚(AC3)
- [ ] 终验三绿(AC4)+ 签名冻结(AC5)+ 锁序/emit 顺序核对(AC7)
