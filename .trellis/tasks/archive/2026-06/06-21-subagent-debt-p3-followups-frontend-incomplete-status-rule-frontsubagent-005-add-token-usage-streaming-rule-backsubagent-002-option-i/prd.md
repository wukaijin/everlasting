# subagent P3 follow-ups: frontend incomplete 视觉 + add_token_usage_streaming 撒谎注释清理

## Goal

收尾 Session 60 留下的 2 条 P3 债(RULE-FrontSubagent-005 + RULE-BackSubagent-002 option i),让 DEBT.md P3 数 9→7,UX 误报(永久显「运行中」)和文档撒谎(`add_token_usage_streaming` 无 production callsite 但代码注释说在 streaming fold)同步消解。

## Requirements(MVP 最小闭环)

### R1 — RULE-FrontSubagent-005:frontend `SubagentStatus` 加 `'incomplete'`

补齐 Session 60 R2 引入的 `'incomplete'` 变体的前端视觉差异化(后端 + DB + sink 已落地,前端 type union 与 STATUS_META 漏配)。

- **`app/src/stores/subagentRuns.ts:65`** — `SubagentStatus` type 加 `"incomplete"` 变体
  - 当前:`"running" | "completed" | "cancelled" | "error"`
  - 改后:`"running" | "completed" | "cancelled" | "error" | "incomplete"`
  - 文档注释同步:wire enum 现为 5 值(对齐 `SubagentStatusDb`)
- **`app/src/components/chat/SubagentDrawer.vue:147-155`** — `STATUS_META` 加 incomplete 入口
  - `incomplete: { label: "未完成", color: "var(--color-tool-warn)" }`
  - 注释:对齐 `INCOMPLETE_MARKER` 中文文案(`[未完成]`),`--color-tool-warn` 视觉对称 `--color-tool-shell` warning tint

### R2 — RULE-BackSubagent-002 option i:删 `add_token_usage_streaming` 撒谎注释

承认现实:此函数无 production callsite(仅 `db/tests.rs` 测试使用,见 [research/r3-token-usage-root-cause.md §3](research/r3-token-usage-root-cause.md) 锁定证据)。production 路径是 `chat_loop.rs:907` `db::add_token_usage`(PR2 把 `skip_persist` gate 解耦后,worker per-turn usage 自然 fold 进 parent)。删掉所有"in streaming fold"的谎言。

**真实谎言位置**(DEBT 标的 567-569/838-843 已 drift,实际位置 576-586/870-876/18/128-137 — 见 §Technical Notes):

- **`app/src-tauri/src/agent/subagent.rs:576-586`** — `per_turn_usage` 字段 docstring
  - 删 "and to **streaming-fold** the per-turn usage into the parent session's `sessions.input_tokens_total` columns via `db::subagent_runs::add_token_usage_streaming` (B6 PR2). The sink does this fold itself so the parent's UI sees the worker burning tokens in real time" 整段
  - 改为简短真实描述:"Populated from `ChatEvent::Done { usage: Some(t) }` events; read by `run_subagent` at worker exit to write `subagent_runs.token_usage_json`. Per-turn fold into parent's `sessions.input_tokens_total` happens via `chat_loop.rs:907` `db::add_token_usage` (decoupled from `skip_persist` in PR2)."
- **`app/src-tauri/src/agent/subagent.rs:870-876`** — `run_subagent` 内 inline 注释
  - 删 "the sink's per-turn accumulator is the path that folds the worker's usage into the parent's `sessions.input_tokens_total` column via `db::subagent_runs::add_token_usage_streaming`."
  - 改为真实描述(无修改必要,只是删错的话)或精简为一行注释
- **`app/src-tauri/src/db/subagent_runs.rs:18`** — module docstring
  - 删 "(updated by `add_token_usage_streaming` as the worker runs)."
  - 改为 "(updated by `add_token_usage` at `chat_loop.rs:907` as the worker runs — `add_token_usage_streaming` is the PR2-API surface for a future worker↔parent session identity split.)"
- **`app/src-tauri/src/db/subagent_runs.rs:128-137`** — 顺手修(DEBT 未列,inspect 时发现同款谎言)
  - 删 "PR2's production wire-up uses `insert_run` + `update_run_finished` + `add_token_usage_streaming`"
  - 改为 "PR2's production wire-up uses `insert_run` + `update_run_finished`; per-turn usage fold goes through `db::add_token_usage` at `chat_loop.rs:907` (decoupled from `skip_persist` in PR2). `add_token_usage_streaming` is retained as the public PR2 API surface for a future worker↔parent session identity split, exercised by `db/tests.rs::add_token_usage_streaming_accumulates_in_parent`."

**保留不动的诚实注释**(已经在描述真实情况,删了反而误导):
- `subagent.rs:803-813` — `drain_per_turn_usage` 函数 doc(明说 "sink-side drain is not invoked by production")
- `db/subagent_runs.rs:554-566` — `add_token_usage_streaming` 函数 doc(明说 "production goes through chat_loop.rs:907 add_token_usage, retained as PR2 API surface")

### R3 — 回归保护(可选,P3 债清理顺手)

**注**:本任务 scope 是"清债",不主动加新功能。R3 仅做"别让谎言复活"的最小回归保护。

- 在 `app/src-tauri/src/db/subagent_runs.rs` module doc 加一行"do not call `add_token_usage_streaming` in production code paths"提示(防止后续 dev 误接)
- 不加新测试(option i 接受 live counter 不 streaming,无新行为需测试)

## Acceptance Criteria

### R1
- [ ] `SubagentStatus` type 包含 5 个变体(`"running" | "completed" | "cancelled" | "error" | "incomplete"`)
- [ ] `STATUS_META.incomplete` 存在,`label === "未完成"`,`color === "var(--color-tool-warn)"`
- [ ] frontend `coerceStatus` 对 `"incomplete"` 字符串不再 fallback 到 `"running"`(走 STATUS_META 拿真实 label/颜色)
- [ ] `pnpm exec vue-tsc --noEmit` 绿
- [ ] `pnpm exec vitest run` 全绿(包含 `subagentRuns` / `SubagentDrawer` 相关)
- [ ] 手动:worker 跑 max_turns 终止后,drawer 状态徽章显「未完成」+ 琥珀色(不是「运行中」+ 蓝绿色)

### R2
- [ ] 上述 4 处撒谎注释被改写为真实描述(全 production 路径指向 `chat_loop.rs:907` `add_token_usage`,而非 `add_token_usage_streaming`)
- [ ] `app/src-tauri/src/agent/subagent.rs` 与 `app/src-tauri/src/db/subagent_runs.rs` 不再有任何 production 路径(注释/docstring)暗示 `add_token_usage_streaming` 被 streaming 调入
- [ ] `cargo check` 绿,0 warning
- [ ] `cargo test --lib` 绿(782+ pass,不退化)
- [ ] `pnpm exec vue-tsc --noEmit` 绿(本次预计不触前端 type,仅 R1 触发)

### R3
- [ ] `db/subagent_runs.rs` module doc 顶部有"production-only path"明确指引
- [ ] `add_token_usage_streaming` 仍是 `pub`(PR2 API 表面保留,不动 wire)

### 收尾
- [ ] DEBT.md: `RULE-FrontSubagent-005` 标 closed + Closed At commit hash;`RULE-BackSubagent-002` 标 closed + Closed At
- [ ] DEBT.md 计数更新:P3 9→7,Total 14→12
- [ ] DEBT.md Re-evaluation Log 加 2 行 closure
- [ ] 4 段式 commit:fix → docs(debt) → archive → journal

## Definition of Done

- cargo test --lib 绿(782+ pass)
- vitest 绿(375+ pass)
- vue-tsc --noEmit 绿
- cargo check 0 warning
- DEBT.md 2 条债 closed + 计数 + Re-evaluation Log 更新
- archive 走标准 4 段式
- journal-2.md 续 session 记录

## Decision (ADR-lite)

**Context**: Session 60(RULE-A-017 closed @ `fd7dc79`)留 2 条 P3 follow-up:

- **RULE-FrontSubagent-005** — backend 加了 `incomplete` 状态(5-variant CHECK + `Incomplete` enum 变体),但 frontend `SubagentStatus` type 还是 4 值,`coerceStatus` 对 unknown 字符串 fallback 到 `"running"`,导致 incomplete run 永久显「运行中」(UX 误报与 R2 想解决的"误报成功"对称)。
- **RULE-BackSubagent-002** — `add_token_usage_streaming` 函数有完整 docstring + public API 表面,但**无 production callsite**(仅 `db/tests.rs` 测)。research `c27f3fd7` 案例锁定:production 路径走 `chat_loop.rs:907` `add_token_usage`(PR2 把 `skip_persist` gate 解耦后,worker per-turn usage 自然 fold)。live counter 几秒延迟可接受。

**Decision**:
- **R1**: 前端 type union + STATUS_META 加 incomplete,改动 ~5 行,纯补齐视觉差异化
- **R2 option i**(用户最终确认):**删撒谎注释**(~20 行),接受 live counter 不 streaming 行为。**不做** option ii(在 `chat_loop.rs:1004` 真接 `add_token_usage_streaming`)——价值/工作量不划算(200+ 轮 worker live counter 几秒延迟是已知接受的折衷,且切换函数会让 PR2 已有 `add_token_usage` 路径变重复)

**Consequences**:
- `add_token_usage_streaming` 仍是 `pub` API 表面(PR2 承诺的 wire shape,未来 worker↔parent session identity split 时会切到它)
- live counter 延迟 ~几秒(非功能问题,parent session counter 走 `add_token_usage` 一次性 fold,在 worker `Done` 事件到达后)
- 新增 DEBT 计数 0(关闭 2 条)
- 不开新 task / 不开新 spec(纯 P3 收面,无新行为需 spec 化)

## Out of Scope (explicit)

- 方案②:`SubagentDef` 加 per-subagent `max_turns` 字段(Session 60 持续搁置)
- 方案 C:子代理结构化外部记忆(Session 60 持续搁置)
- option ii:`chat_loop.rs:1004` per-turn `Done` handler 真接 `add_token_usage_streaming`(用户明确拒绝,价值不抵复杂度)
- 新增 `add_token_usage_streaming` 的真 production 接线
- `add_token_usage_streaming` 函数体删除(保留为 PR2 API 表面)
- 前端 drawer 的 cancelled run `at turn N` 语义(那是 RULE-FrontSubagent-004 的事,独立债)
- worker permission_ask interactive(那是 RULE-FrontSubagent-003 的事,P2 更大范围)
- L3 并行 subagent + worktree 隔离(ROADMAP 第三档,需新 task)

## Technical Notes(行号现场 — DEBT 标的已 drift)

### R1 — Frontend 现状

- `app/src/stores/subagentRuns.ts:63-65` — `SubagentStatus` type 定义(4 值,缺 `"incomplete"`)
- `app/src/stores/subagentRuns.ts` — `coerceStatus` 函数位置待 check 阶段定位(grep `coerceStatus` 找定义)
- `app/src/components/chat/SubagentDrawer.vue:147-155` — `STATUS_META` 4 项,缺 incomplete

### R2 — Lying comments 真实位置(DEBT 标的 567-569/838-843 → 实际 576-586/870-876)

| 真实位置 | DEBT 标的 | 内容 | 改写 |
|---|---|---|---|
| `app/src-tauri/src/agent/subagent.rs:576-586` | 567-569 | `per_turn_usage` 字段 docstring 末段 "streaming-fold ... via add_token_usage_streaming" | 改写为真实 production 路径(`chat_loop.rs:907` `add_token_usage`) |
| `app/src-tauri/src/agent/subagent.rs:870-876` | 838-843 | `run_subagent` inline 注释 "via add_token_usage_streaming" | 删撒谎行,留精简指引 |
| `app/src-tauri/src/db/subagent_runs.rs:18` | 18(未 drift) | module docstring "(updated by `add_token_usage_streaming` as the worker runs)" | 改写为真实路径 |
| `app/src-tauri/src/db/subagent_runs.rs:128-137` | (DEBT 未列) | type docstring "production wire-up uses ... add_token_usage_streaming" | 改写为真实路径(inspect 阶段 bonus 发现) |

**保留不动的诚实注释**:
- `subagent.rs:803-813` `drain_per_turn_usage` doc — 已正确描述 "sink-side drain is not invoked by production"
- `db/subagent_runs.rs:554-566` `add_token_usage_streaming` 函数 doc — 已正确描述 production 走 `add_token_usage`,本函数是 PR2 API 表面

### Production 路径证据(research 锁定)

- `chat_loop.rs:907` `add_token_usage` 是 per-turn 真实 fold 点(R2 PR 解耦 `skip_persist` gate)
- `add_token_usage_streaming` 在 `db/subagent_runs.rs:568` 定义 + 567 `#[allow(dead_code)]`,无生产 caller
- `grep -rn "add_token_usage_streaming" app/src-tauri/src/` 全量 9 处:3 处在 subagent.rs(本次改 2)+ 3 处在 db/subagent_runs.rs(本次改 2,留 1 函数 def)+ 3 处在 db/tests.rs(测试,不动)

### 关键文件

- `app/src/stores/subagentRuns.ts:65` — type union
- `app/src/components/chat/SubagentDrawer.vue:147-155` — STATUS_META
- `app/src-tauri/src/agent/subagent.rs:576-586, 870-876` — lying comments
- `app/src-tauri/src/db/subagent_runs.rs:18, 128-137` — lying comments
- `app/src-tauri/src/db/subagent_runs.rs:568` — `add_token_usage_streaming` 函数 def(不动)
- `app/src-tauri/src/chat_loop.rs:907` — production `add_token_usage` 调用点(参考,不动)
- `.trellis/reviews/DEBT.md` — 回填 closure
