# subagent resume mechanism (C1)

> 父任务：`07-26-workflow-review-plugin`（review epic）
> 依赖：无（独立基建）。C2、C3 都依赖本任务。

## Goal

为 subagent 引入 **resume（续接）机制**：让一次 `dispatch_subagent` 能续接上一次某个 worker run 的对话历史，而非每次全新构造 messages。主 LLM 在续接时对新 worker「澄清现状 + 变化点 + 工作目的」，省去重读文件上下文的 token。

**动机**：review workflow 的 `reviewing ↔ revising` 循环里，第 2+ 轮 reviewer 若全新派会重读 prd 全文；resume 后续接上轮会话，主 LLM 只需说明「修订了什么、本轮目的」，大幅省 token。本机制是独立基建，dev 的 implement ↔ check 循环同样受益（checker 续接上次校验上下文）。

## Background（代码事实）

- 现状：`build_worker_messages`（定义在 `agent/subagent/mod.rs:642`，`dispatch.rs:645` 是调用点）每次 dispatch 全新构造 messages，无续接路径。搜遍 `agent/subagent/` 无 resume/continuation/follow_up 机制。
- worker run 已持久化到 `subagent_runs` 表（含 `transcript_json`，`dispatch.rs:1127` `truncate_transcript_for_persistence`），但 transcript 格式是给前端展示的 `TranscriptEntry`，**不是可直接续接的 LLM messages 结构**。
- worker 可能跑在隔离 worktree（`parallel: bool` 路径，`dispatch.rs:523`），resume 需考虑 worktree 复用或重建。

## Requirements

### R1. resume 能力

- 新增机制：`dispatch_subagent` 可指定「续接某个历史 worker run」，新 worker 的初始 messages = 上轮 worker 的对话历史 + 主 LLM 注入的「现状/变化/目的」说明。
- 续接的 worker run 标识：建议复用 `subagent_runs.run_id` 作为 resume 句柄。
- 主 LLM 注入的澄清文本由主 LLM 在 dispatch 的 task 参数里提供（不自动生成）。
- **fallback 路径**：resume 机制不可用（持久化缺失 / run 损坏）时，降级为全新派（`build_worker_messages` 现状行为），保证功能不阻断。fallback 触发时打 warn 日志。

### R2. 消息历史持久化与重建

- worker 的 LLM messages（非 transcript）需可重建为续接起点。两个方向：
  - **方向 a**：持久化原始 LLM messages（新增存储，区别于 transcript_json）。**注意**：transcript_json 是截断展示格式（>4MB 只保留 head+tail，见 `truncate_summary.rs`），超长 run 从 transcript 重建不可行，故**默认选方向 a**。
  - **方向 b**：从 transcript_json 反向重建 LLM messages（复用现有存储，但需转换逻辑，仅作短 run 的 fallback）。
- design.md 明确选方向 a 为主路径（评审 MiniMax §4.6 指出方向 b 对长 run 实质不可行）。

### R3. worktree / 隔离处理

- 若上轮 worker 跑在隔离 worktree，resume 时需决策：复用同一 worktree / 重建新 worktree / 不隔离。
- **关键区分（reviewer vs implementer 场景）**：resume 的「复用」对只读 agent 和写 agent 含义不同：
  - **写 agent（如 dev implementer）**：上轮有文件改动留在 worktree，复用 worktree = 保留工作产物，有意义。
  - **只读 agent（如 review reviewer）**：上轮无文件改动（只读工具集），worktree 里没有「产物」可保留；它要复用的是**对话历史**（上轮读过的文件内容、思考过程），与 worktree 无关。对这类 agent，resume 可完全不涉及 worktree（共享主项目根即可），降低复杂度。
- design.md 需按 agent 类型（只读 vs 写）分别定策略，不能一刀切「倾向复用 worktree」——那只对写 agent 成立。

### R4. API 兼容

- resume 是 opt-in（不指定则保持现有全新派行为，零回归）。
- `dispatch_subagent` 工具 schema 增 resume 参数：
  ```jsonc
  {
    "resume_from": "<run_id>",
    "resume_clarification": {
      "current_state": "当前 PRD 摘要",
      "changes_since_last": ["修订点1", "修订点2"],
      "this_round_purpose": "本轮评审目的"
    }
  }
  ```
- **stale context 处理**（评审议题 3 采纳）：resume clarification 必须显式列出上轮以来的变更点（changes_since_last），让续接的 worker 先 update 上下文。reviewer 场景特别重要 —— 上轮 messages 含旧 prd 引用，必须靠 changes_since_last 显式覆盖。
- 错误码：`resume_run_not_found` / `resume_run_still_running`（不允许 resume running）/ `resume_run_other_session`（跨 session 限制，见 OQ3）。

### R5. 适用范围

- 本任务是通用基建，不绑 review。验收时需同时验证：review 场景（reviewer 续接）+ dev 场景（checker 续接）+ 现有非 resume 路径不回归。

## Acceptance Criteria

- [ ] `dispatch_subagent` 支持 resume 参数，续接指定历史 run 的对话历史。
- [ ] 主 LLM 注入的澄清文本能正确拼接到续接 messages。
- [ ] worktree 复用/重建策略明确且实现（R3）。
- [ ] **回归**：不指定 resume 时行为与现状完全一致（现有 subagent 测试全绿）。
- [ ] 单测：resume 续接的 messages 正确性、空历史 run 的 resume 边界、resume 不存在 run_id 的错误处理。
- [ ] `cargo test --lib` + `cargo clippy --lib --tests` 通过。

## Open Questions（design.md 需解决）

1. ~~R2 方向 a vs b~~ → **已定方向 a 为主**（transcript 重建对长 run 不可行，见 R2）。design 阶段定存储 schema（新表 vs subagent_runs 加列）。
2. R3 worktree 策略 —— 需按 agent 类型分别定：只读 agent（reviewer）resume 可不涉及 worktree；写 agent（implementer）resume 需复用 worktree 保留产物。
3. resume 跨 session 是否允许（上轮 worker 在 session-1，本 session-2 能否 resume）——影响 review 独立 session 的形态。

## Notes

- 本任务是 review epic 的硬前置（C2 视图、C3 资源包都依赖 resume 的续接形态）。
- resume 也是 dev 循环的增强（checker 续接），收益不限于 review。
