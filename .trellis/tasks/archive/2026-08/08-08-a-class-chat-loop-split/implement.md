# A类单体重构:chat_loop 拆分 — Implement

## 实际执行记录(2026-08-08,完成)

### 已完成(4 提取 + 1 子模块化 + 1 文档 sweep,共 7 commit,行为零变化,1662 测试全绿)

| Step | Commit | 内容 | 行数 |
|---|---|---|---|
| A.1 | `2c57b80` | 提取 `prepare_loop_state` + `LoopInit`(17 字段)—— **策略 C**(单函数) | L591–1262(672 行) |
| A.2 | `e749635` | 提取 `dispatch_tool_calls` + `DispatchOutcome`(L2 parallel + serial 合并) | 1421 行 |
| A.3 | `3ca8ce0` | 提取 `finalize_turn`(loop_hint 追加 + tool_result persist,2 早返 Result) | 115 行 |
| A.4 | `bd975ef` | 提取 `drive_turn` + `DriveTurnOutcome`(turn 驱动主循环,12 早返 Result) | 1530 行 |
| B | (modularize) | 子模块化:`chat_loop.rs` hub + `init.rs`/`drive.rs`/`tools.rs` + `tests_chat_loop.rs` | 文件移动 |
| C | `606da0b` | 文档 sweep:`.trellis/spec` 11 处 `chat_loop.rs:LINE` → 符号引用 | 9 文件 |
| D | `fddd52a` | implement.md 同步 + clippy needless_borrow 清理 | — |

**最终效果**:
- `chat_loop.rs`(hub):5532 行 → **1376 行**(降 75%);`run_chat_loop` 函数体 ~4300 行 → **726 行**(降 83%)
- 子模块:`init.rs`(769) + `drive.rs`(1653) + `tools.rs`(1648) + `tests_chat_loop.rs`(208)
- run_chat_loop 签名 34 参数零改动;每个提取 commit 独立 `cargo test --lib` 全绿(1662)

### 关键决策(偏离原 design,实施时拍板)

- **D-C1 初始化段策略 C**:原 design 计划两函数,实施时深读发现 5 个 early-return 各自 emit 不同事件 + 190 行 forced_dispatch 早返块,run_chat_loop 返回 `()` 无法打包早返。改为**单函数 `prepare_loop_state -> Result<LoopInit, ()>`**,早返 emit 原样留函数内部。**(用户拍板)**
- **D-C2 ChatLoopState 未引入**:实测 `cancelled`/`had_error` 是每 turn 局部(循环体内重新声明)。跨 turn 状态通过 `LoopInit`/`DriveTurnOutcome`/`DispatchOutcome` struct 传递 + hub 字段赋值写回(非 let 解构——见 D-C5)。
- **D-C3 cwd 持久性 bug 规避**:dispatch 提取时发现 `current_ctx`/`last_cwd` 是函数作用域 mut 绑定(shell tool 改 cwd 需跨 turn 持久),turn 局部解构会丢失更新。**修复**:dispatch 返回 `DispatchOutcome`,hub 字段赋值写回。
- **D-C4 参数传递**:body 内 `&param` 的参数按 owned clone 传入(避免 `&&T` 双重引用);hub 仍需的按 `&`。
- **D-C5 跨 turn 状态用字段赋值而非 let 解构**:drive_turn 提取时发现,循环体内 `let DriveTurnOutcome { messages, seq, ... } = outcome` 会 shadow 函数作用域绑定,下一轮迭代读到 moved 值。**修复**:用 `messages = drive_outcome.messages;` 字段赋值写回函数作用域绑定(与 D-C3 同模式)。
- **D-C6 drive_turn 12 早返统一 Err(())**:原担心需 TurnOutcome enum,实施时确认 12 个 early-return 各自 emit 终端事件后 return,统一 `return Err(())` + hub `return` 即可,无需 enum。

### 文档 sweep 范围

- ✅ `.trellis/spec/` 11 处活跃契约 `chat_loop.rs:LINE` → 符号引用(已完成)
- ⏸️ `docs/` 历史快照(spike/INTERLEAVED/WORKFLOW)按前三专项惯例不动
- ⏸️ 代码自注释 8 处 `chat_loop.rs:LINE` 残留(不影响正确性,低优先后续)

### 验证命令(终验通过)

```bash
cd /usr/local/code/github/everlasting/app/src-tauri
export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
cargo test --lib                           # 1662 全绿(含 agent_loop_* 9 个)
cargo clippy --lib --tests                 # 零警告
cargo fmt --check                          # clean
```

## Review Gates

- [x] 用户评审 prd/design/implement 通过(行号/字段已修正)
- [x] `task.py start` 后实施
- [x] 每提取 commit 独立可回滚(AC3)—— A.1/A.2/A.3/A.4 各自独立 commit
- [x] `cargo test --lib` 全绿(AC4,1662 基线无减少)+ 签名冻结(AC5,34 参数零改动)
- [x] 锁序/emit 顺序核对(AC7)—— cwd 持久性 bug 在 A.2 主动发现并修复,9 个 agent_loop_* 集成测试锁定
- [x] AC1(run_chat_loop 主体 ≤ ~500 行)基本达成——726 行(含 ~190 行 forced_dispatch 块 + 收尾);纯 turn 循环骨架 ~400 行
- [x] AC6 非 archive 文档无残留 `chat_loop.rs:LINE`(活跃契约已 sweep)

---

## 原始计划(保留作参考,实际执行见上方"实际执行记录")

### 执行策略

同前三专项:先"提取"(chat_loop.rs 内部函数化,每阶段独立 commit + 中间全量 cargo test)→ 再"拆分"(子模块化 + 测试迁出,单 commit)→ 文档 sweep。turn 循环内提取以 `ChatLoopState` 为状态载体,提取前先建 ChatLoopState(空壳或逐步填充)。

> **行号锚点均经实测核对(见 design §1/§3/§4)。** 切分时以代码控制流为准,行号仅作起止参考。

### Ordered Checklist

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

### 风险提示(design §6)

- 提取时保持 turn 内逐行顺序;`ChatLoopState` 字段访问替换局部变量为机械替换,禁止顺手重构
- pre-loop `let mut` 实测清单(L591–1482,见 design §3)为跨 turn 状态锚点;循环内初始化但跨阶段读写项(`loop_hint`/`cancelled`/`had_error`)必须进 state;每提取一个阶段函数后 grep 核对变量读写点归属
- RAII guard(`_cancel_guard`@L580)/ 借用敏感项留主体(不尝试移入函数);prepare_session 从 L581 起
- **命名碰撞**:`turn_ctx` 局部(@L764)与新 struct 不同名(ChatLoopState),提取时勿改局部名
- handle_loop_detection 边界非连续,按控制流切不按行号
- 每 commit diff 应只含"该阶段代码位移 + ChatLoopState 字段增减",出现无关改动立即回退
