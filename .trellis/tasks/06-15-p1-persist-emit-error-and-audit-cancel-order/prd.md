# p1-persist-emit-error-and-audit-cancel-order

> 关联 DEBT.md：RULE-A-003 (P1) + RULE-A-004 (P1)
> 依赖：✅ 已解阻 — RULE-A-006 (PR5 集成测试) closed 2026-06-15,9 个 `agent_loop_*` 测试现覆盖 production `run_chat_loop` 真实路径

## Goal

修复 agent loop 两处静默正确性 bug:
- **RULE-A-003**:`persist_turn` 失败只 `tracing::error!` 后继续,前端无任何信号,DB 与内存永久分叉(磁盘满/DB 锁竞争时,下次打开 session 空白)。兑现一笔 2026-06-12 就提出的旧账(REVIEW-sse §8 P3,至今 0 落地)。
- **RULE-A-004**:`record_tool_executed_audit` 在 cancel 检查之前调用,cancel 短路的 tool 仍记一行 `tool_executed`,审计撒谎,误导追责/回放分析。

两条 RULE 都改 `chat_loop::run_chat_loop` —— RULE-A-006 闭环后的**唯一** agent loop body,production + 9 个集成测试共享。改一处即测试真实路径(非副本)。

## What I already know

### RULE-A-003 — 5 处 persist_turn 调用(迁移后真实行号,`chat_loop.rs`)

| # | 行 | 上下文 | 当前失败行为 | seq 副作用 |
|---|---|---|---|---|
| 1 | `:263-269` | loop 开始前持久化初始 user message | `tracing::error!` 静默 | `seq += 1`(`:268`)仍推进 |
| 2 | `:513-538` | 持久化 assistant turn | 静默,**且不** emit TurnComplete(成功才 emit,`:524-536`) | `messages.push` + `seq += 1`(`:537-538`)仍推进 → **内存/DB seq 错位** |
| 3 | `:544-555` | cancel 路径 synthetic tool_result | `tracing::error!` 静默 | 无 seq 推进(紧接 return) |
| 4 | `:687-698` | cancelled tool_result turn | `tracing::error!` 静默 | 无 seq 推进(紧接 return) |
| 5 | `:723-734` | tool_result turn | `tracing::error!` 静默 | `messages.push` + `seq += 1`(`:735-736`)仍推进 → seq 错位 |

**seq 错位是 A-003 的隐藏次生 bug**:persist 失败时内存 seq 推进了、DB 没推进,下次 `run_chat_loop` 从 `messages.iter().map(seq).max()+1` 算 next_seq(`:140-147`),会得到一个 DB 里实际不连续的 seq。当前 SQLite schema 的 seq 是否唯一约束 → 实现阶段需确认(若 PK 含 seq,重连后续写会撞约束;若不含,则只是空洞)。

### RULE-A-004 — audit 时序(`chat_loop.rs:643-657`)

```rust
let duration_ms = tool_exec_start.elapsed().as_millis();           // :642
if let Err(e) = permissions::record_tool_executed_audit(...).await { // :643-652
    tracing::warn!(...);                                            // :653
}                                                                   // —— audit 在此已记
if token.is_cancelled() {                                           // :655 —— cancel 检查滞后
    cancelled = true;                                               // :656
}
```

cancel 在 tool 执行期间触发时(token 被 `cancel_chat` 或 `cancel_inflight_for_session` trigger),`execute_tool` 返回后 audit 先落库,然后才进 cancelled 分支。被取消的 tool 记成"已执行"。

**Fix**:把 `:643-654` 的 audit 块整体移到 `:655-657` cancel 检查**之后**。仅未取消的 tool 记 audit。

### 既有 Error 事件语义(参考点)

- `ChatEvent::Error` 在 agent loop 里是**终止性信号**:`:438-444` LLM stream Error → `had_error = true; break` → `:457` `if had_error { return; }`。
- RULE-A-002(closed)的 `StillOver` 模式(`:304-334`):context 超窗 → emit `ChatEvent::Error { InvalidRequest }` + `return`。建立"数据完整性失败 → emit Error + 终止"先例。
- 前端收到 `Error` 会停 spinner / 渲染错误 toast(与 cancelled/max_turns 的 `Done` 区分)。

## Decisions (resolved)

- **[Q1] persist 失败语义 = emit Error + 终止 loop**。对齐 RULE-A-002(StillOver)模式。代价:瞬时 DB 锁竞争中断本次 chat —— 可接受(优于静默丢数据)。
- **cancel 路径 corner case**:`#3`(`:544-555`)、`#4`(`:687-698`)两处 persist 失败 → **只 `tracing::error!`,不 emit Error**。理由:cancel 是用户主动放弃,紧随其后要 emit cancelled `Done`;若再 emit Error 会产生两个终止性事件冲突。cancel 路径不落盘的后果用户已知(他取消了)。
- **category 复用 `LlmErrorCategory::Server`**。存储失败属系统侧,`Server` 语义准;前端不基于 category 分支(已验证),用户只见 `message` 文案。不新增 enum 变体 → 零 serde/IPC/前端改动。
- **scope = 5 处 persist 全覆盖**,其中正常路径 3 处(#1/#2/#5)emit Error+return,cancel 路径 2 处(#3/#4)降级为 log-only。
- **A-004 audit 移位**:把 `chat_loop.rs:643-654` 的 `record_tool_executed_audit` 块整体移到 `:655-657` cancel 检查之后。仅未取消的 tool 记 audit。

## Requirements (locked)

**RULE-A-003**(5 处 persist_turn 失败路径):
| # | 行 | 失败处理 |
|---|---|---|
| 1 | `:263-269` 初始 user msg | emit `Error{Server}` + `return` |
| 2 | `:513-538` assistant turn | emit `Error{Server}` + `return`(保留"成功才 emit TurnComplete") |
| 3 | `:544-555` cancel synthetic tool_result | `tracing::error!` only(cancel 路径) |
| 4 | `:687-698` cancelled tool_result | `tracing::error!` only(cancel 路径) |
| 5 | `:723-734` tool_result | emit `Error{Server}` + `return` |

Error 文案(中文,对齐项目惯例):`"保存对话记录失败(可能磁盘满或数据库被占用),请重试。详情: {e}"`

**RULE-A-004**:audit 调用移到 cancel 检查之后。

保留所有 `tracing::error!`(运维侧);Error 事件面向用户。

## Acceptance Criteria (locked)

- [ ] **A-004**:`record_tool_executed_audit` 在 `token.is_cancelled()` 检查之后;集成测试断言 cancel 短路的 tool 不落 audit 行。
- [ ] **A-003 #1/#2/#5**:persist 失败 → emit 恰一个 `ChatEvent::Error{ category: Server }` + loop 终止(MockEmitter 可断言)。
- [ ] **A-003 #3/#4**:cancel 路径 persist 失败 → 不 emit Error,仍 emit cancelled `Done`(无冲突)。
- [ ] persist 失败测试可触发(schema 损坏 fixture / drop table 后调用,实现阶段选最轻)。
- [ ] `cargo test --lib`(带 PKG_CONFIG_PATH)全套 pass,`cargo check` 0 warning。

## Definition of Done

- 测试新增/更新(集成测试走 `run_chat_loop` + MockProvider + MockEmitter)。
- `cargo check` 0 warning / `cargo test` 全 pass(带 WSL PKG_CONFIG_PATH)。
- DEBT.md 两条 RULE 更新 Status → closed + Closed At commit。
- 不改前端(Error 事件已存在,前端 toast 路径复用)。

## Out of Scope (explicit)

- **不**扩到 `persist_turn_cwd` / `touch_session` / `add_token_usage` —— 这些本就是 best-effort(`let _ =`),DEBT A-003 只点名 `persist_turn`。
- **不**实现 persist retry / WAL 调优 —— 治标(可见性)不治本(根因磁盘满/锁),后续若复现再单开。
- **不**改 SQLite schema(不在本任务确认 seq 唯一约束的影响范围 —— 即使有空洞也不修)。
- RULE-A-007(error 路径 partial text 丢失,P2)不并入 —— 独立 task。

## Technical Notes

- **改动文件**:仅 `app/src-tauri/src/agent/chat_loop.rs`(production + test 共享单一 body)。
- **行号基线**:RULE-A-006 闭环迁移后,DEBT.md 里 `chat.rs:439-447/875-886/1205-1216` / `:1094-1116` 全部失效,真实位置在 `chat_loop.rs`(见上表)。
- **回归保护**:RULE-A-006 的 9 个 `agent_loop_*` 集成测试(`agent/tests.rs`)现覆盖 `run_chat_loop` 真实路径,改完即被测。
- **persist 失败如何触发(测试)**:现有测试用内存 SQLite + 正常 schema。需构造 persist 必败 —— 候选:(a) 注入一个 schema 损坏的表名;(b) 用 sqlx 关闭 pool 后调用;(c) mock db 层。实现阶段选最轻的(倾向 a 或一个 drop table 的 fixture)。
