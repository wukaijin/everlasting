# subagent: MAX_TURNS 提至 200 + worker token 统计修复 + incomplete 终止状态

## Goal

让 Everlasting 的 worker subagent 能支撑 trellis-implement 级别的重型实施任务（200+ 工具调用），并补齐可观测性——否则 200 轮预算既看不见成本（token 统计全 0），又在预算耗尽时误报"成功"（max_turns 软终止被记成 completed）。

## Requirements（MVP 最小闭环）

- **R1**: `SUBAGENT_MAX_TURNS` 常量 20 → 200（`chat_loop.rs:2019`，全局，**不**加 per-subagent 字段）
- **R2**: max_turns 软终止 status 由 completed → incomplete
  - 新增 `SubagentStatus::Incomplete` 变体 + `"incomplete"` 映射（`subagent.rs:378-380`）
  - DB migration：`subagent_runs.status` CHECK 加 `'incomplete'`（`migrations.rs:520`）
  - `Done{stop_reason:"max_turns"}` 路径走 Incomplete（`subagent.rs:850-864`）
  - final_text 带文案提示（沿用 B6 status prefix，前端不改）
- **R3**: 修复 max_turns 终端路径丢 `last_usage` 的 bug（research 已锁定根因）
  - `chat_loop.rs:1797-1804`：把 `usage: None` 改成 `usage: last_usage`，让终端合成 `Done` 携带最后一轮的累计 usage
  - `subagent.rs:835-849` Done arm：加 stop_reason guard——当 stop_reason 是 `"max_turns"` 或 `"cancelled"`（合成终端）时**不** push 进 `per_turn_usage`，避免双累（per-turn Done 已经把最后一轮 push 过）
  - 验证：`c27f3fd7` 类型的 worker 终止后 `token_usage_json` 非全 0

## Acceptance Criteria

- [ ] `SUBAGENT_MAX_TURNS == 200`
- [ ] worker 跑到 max_turns 时 `subagent_runs.status == 'incomplete'`（非 completed）
- [ ] worker 跑完（任何终止路径）后 `token_usage_json` 反映已发生 turn 的累计 usage，不再全 0
- [ ] 修复后**没有双累**：累计值 = 真实 LLM usage（无最后一轮重复）
- [ ] 现有 researcher/general-purpose 正常路径不回归（cargo test 绿，含 subagent sink / db tests）
- [ ] 新增 R3 回归测试：`Done{end_turn, u1}` × N + `Done{max_turns, u_last}` → `cumulative_usage() == u1+…+u_last`
- [ ] DB migration 幂等（已有 dev.everlasting.db 可升级）

## Definition of Done

- cargo test 绿（包含新增的 R3 回归测试）
- vue-tsc --noEmit 绿（本次预计不触前端）
- spec/DEBT 回填

## Decision (ADR-lite)

**Context**: worker subagent 的 20 轮预算无法支撑 trellis-implement 级重型实施（200+ 工具调用）；且 max_turns 软终止被记成 completed（误报成功），token 统计在 max_turns 场景全 0（成本不可见）。

**Decision**:
- 全局常量提至 200（不加 per-subagent 字段——方案②用户未定，搁置）
- max_turns → 新增 incomplete 状态（语义清晰，与 completed 区分）
- token 修复路径由 research 锁定为 `chat_loop.rs:1797-1804` max_turns 终端合成 `Done` 丢 `last_usage` + sink 端加 stop_reason guard 防双累
- 前端视觉差异化不纳入本次（靠 final_text 文案）

**Consequences**:
- 200 轮的失控成本上升（C3 防 context 爆但不防烧钱）—— token 统计修复让成本可见，是必要配套；真正的成本阀（token/wall-clock 第二道）留 follow-up
- incomplete 状态最终也要前端区分（本次靠文案，follow-up 补视觉）
- **`add_token_usage_streaming` 是已发现的文档/接线债**（research 彩蛋），comments at `subagent.rs:567-569 / 838-843` + `db/subagent_runs.rs:18` 描述了不存在的 streaming fold 行为——本次不修，留 follow-up（删除撒谎注释 OR 真接上）

## Out of Scope (explicit)

- 方案②：`SubagentDef` 加 per-subagent `max_turns` 字段（用户方案未定）
- 方案 C：子代理结构化外部记忆（防 C3 压缩失忆）——用户明确搁置
- 新增 implement 型子代理
- token/wall-clock 第二道成本阀
- 前端 drawer 的 incomplete 视觉区分（本次靠 final_text status prefix 文案）
- `add_token_usage_streaming` 文档/接线债（research 彩蛋，留 follow-up）

## Technical Notes（根因现场 — 已 research 锁定）

**R3 根因（research 锁定）**:
- 4 个终端 `Done` 合成点：
  1. **Normal completion**（`chat_loop.rs:1277-1282`）：`usage: last_usage` ✓
  2. **Cancel**（`:1226-1233`, `:1751-1758`）：`usage: None` ✓（故意，cancel 时确实没 usage）
  3. **Error**（`:1246-1254`）：不发终端 Done ✓
  4. **`max_turns`**（`:1797-1804`）：`usage: None` ✗ —— **丢 last_usage**
- `last_usage` 生命周期：`chat_loop.rs:873` 初始化（每 turn 重置）→ `:966-968` inner stream `Done` 更新 → 终端合成时引用
- 双累风险：per-turn `Done{usage:Some}` 已经被 sink push 进 `per_turn_usage` Vec（`subagent.rs:844-849`），如果 max_turns 终端 Done 也 push `last_usage`（值等于最后一轮的 per-turn usage），会双计最后一轮——所以 sink 必须加 stop_reason guard

**R3b（provider 解析）已排除**：research 跑了 sibling `4588194e` researcher run（同 model_id）有 170879 token，证明 Anthropic/OpenAI SSE usage 解析在 production 正常工作；`c27f3fd7` 全 0 是终端合成路径丢 last_usage，与 provider 解析无关。

**关键文件**:
- `chat_loop.rs:873` `last_usage` init；`:966-968` inner stream Done 更新；`:1797-1804` max_turns 终端（待修）
- `subagent.rs:835-849` sink Done arm（待加 stop_reason guard）；`:378-380` SubagentStatus 映射（待加 Incomplete）；`:782-806` token 求和
- `migrations.rs:515-520` subagent_runs CREATE + status CHECK（待加 'incomplete'）
- `research/r3-token-usage-root-cause.md` 完整 evidence + 4 选项修复形态

现场数据：`c27f3fd7`（general-purpose, max_turns 终止，token 全 0），`4588194e`（researcher, end_turn 终止，170879 token）