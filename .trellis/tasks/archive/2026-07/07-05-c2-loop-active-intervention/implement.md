# Implement — C2+ 循环检测主动干预

> PRD: `prd.md` · Design: `design.md`
> 接入面：`chat_loop.rs:1968-1984`（detect 接入点，C2+ 在此加 count + register + select）

## PR 拆分

3 PR，每个独立可测可 revert。

### PR1 — AuditKind::LoopIntervention + record fn（铺路）

- [ ] `agent/permissions/audit.rs`：
  - enum 加 `LoopIntervention` 变体（Message 域之后或新 Loop 域）
  - `as_str()` 加 `Self::LoopIntervention => "loop_intervention"`
  - 加 `pub async fn record_loop_intervention_audit(pool, session_id, run_id, hit_count, verdict_kind, action)` —— 仿 `record_message_resend_audit`（best-effort，非事务内）
- [ ] `audit.rs` 单测：`record_loop_intervention_audit` 落表 payload 正确（hit_count/verdict_kind/action 三字段）
- [ ] 验证：`cargo test --lib audit`（或全量）

**验证命令**：
```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib audit
```

### PR2 — chat_loop 核心（count + register + select + 三分支）

- [ ] `agent/chat_loop.rs:1112` 旁（`loop_window` 声明处）加 `let mut loop_hit_count: u32 = 0;`
- [ ] `chat_loop.rs:1980` detect 之后，加 C2+ 状态机：
  - `verdict == None` → `loop_hit_count = 0`
  - `verdict != None` → `loop_hit_count += 1`
  - `loop_hit_count >= 3` → 构造 payload（PRD R2 文案）+ `record_loop_intervention_audit(action="asked")` + `QuestionStore::register` + `emit_tool_question` + `select!{token.cancelled(), rx}`
  - 三分支：终止 / 继续（含 count=0 + 增强 hint） / cancel
  - `AlreadyPending` → 降级走原 hint
- [ ] emit Done 处加 `loop_terminated` 分支（`stop_reason="loop_terminated"`）
- [ ] 增强 hint 文案（「继续」后注入）：明确含「用户已确认你在循环，请立即改变策略或停止」
- [ ] 集成测试（`tests_agent_loop.rs`）：
  - `c2plus_terminates_after_3_consecutive_hard_loops`
  - `c2plus_continue_resets_count`
  - `c2plus_none_resets_count`
  - `c2plus_session_cancel_during_ask`
  - `c2plus_already_pending_skips`
- [ ] 验证：`cargo test --lib`（全量，确保零回归）

**验证命令**：
```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo fmt --check
```

### PR3 — worker dispatch_result 告知父 + 前端 audit 分发

- [ ] `agent/subagent/dispatch.rs::format_dispatch_result_with_model`（或 caller）：检测 worker `Done.stop_reason="loop_terminated"` → result content 追加 `[loop terminated: worker 因循环重复操作被自动终止，未完成全部步骤]`
- [ ] 集成测试：`c2plus_worker_breaks_and_notifies_parent`（worker 5 连相同 → assert worker stop_reason + dispatch_result 含提示）
- [ ] 前端 `AuditLogModal` / audit store：加 `loop_intervention` kind 分发渲染（payload 三字段）
- [ ] 前端 vitest：AuditLogModal 渲染 `loop_intervention`
- [ ] 验证：`pnpm test` + `pnpm build`（vue-tsc 0 err）

**验证命令**：
```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app && pnpm test
cd app && pnpm build
```

## spec 更新（PR3 收尾，trellis-update-spec）

- [ ] `backend/agent-loop-architecture.md`：C2 章节扩展 C2+（count + register + select + 三分支 + worker 直接 break）
- [ ] `backend/tool-contract.md`：加 `AuditKind::LoopIntervention` Scenario（payload 契约 + 落表时机）
- [ ] 前端 `frontend/audit.md`（若存在）：`loop_intervention` kind 分发

## 风险文件 / Rollback 点

| 文件 | 风险 | Rollback |
|---|---|---|
| `chat_loop.rs` | select! 阻塞期间 cancel token 必须 select 进去，否则 Stop 不响应 | PR2 单 commit，revert 即回原 C2 行为 |
| `audit.rs` | `as_str` match 必须穷尽（加变体漏 match → compile err，编译期拦截） | PR1 单 commit |
| `dispatch.rs` | worker stop_reason 检测要在 format_dispatch_result 之前 | PR3 单 commit |

每个 PR 独立 commit，可单 PR revert。PR2 是核心，PR1/PR3 是铺路 + 收尾。

## Review Gates

- PR1 后：audit 单测绿
- PR2 后：全量 `cargo test --lib` 绿（含 5 新集成测试 + 零回归 C2 既有测试）
- PR3 后：`pnpm test` + `pnpm build` 绿
- 全部完成后：trellis-check 跑 spec 合规 + 跨层数据流

## Follow-up（非本任务）

- E2 trace viewer 实际实现时复用 `AuditKind::LoopIntervention`（C2+ 已铺路）
- 增强 hint 文案若 LLM 不响应，考虑加 system-level 信号（当前 design 用 result message text 注入）
