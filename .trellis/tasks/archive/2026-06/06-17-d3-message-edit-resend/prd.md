# brainstorm: D3 session 内消息编辑/重发

## Goal

允许用户在 session 内编辑或重发已发消息,重走 turn 边界 + agent loop 续编。是 V2 第二档最后一项,跟 A2+B7(权限)/ C3(压缩)/ B5(Memory)/ C4(审计)都强耦合——做 D3 时会自然碰到 RULE-A-007(error 路径 partial text 丢失)和 RULE-A-010(二次取消语义未实现)的修复窗口(DEBT §收尾路径建议第 3 条)。

## What I already know

### D3 范围(V2 第二档 §2 描述)
- "session 内消息编辑 / 重发"
- 备注: "session 灵活交互"
- 当前实现: **零** — `app/src/components/chat/` 全部搜过,`edit`/`resend`/`重发` 0 命中

### 强耦合债(DEBT.md)
- **RULE-A-007**(P2 open,Agent Loop):`app/src-tauri/src/agent/chat.rs:741-756` Error arm 不 persist 已累积 text。SSE 流中途 error 时已渲染的 delta,reload 后从 DB 读不到(与 cancel 路径不对称)。
  - 2026-06-14 行号,实际位置需 `chat_loop.rs` 重核(06-15 迁移后副本消除,新位置在 `chat_loop.rs` 868 附近)
- **RULE-A-010**(P3 open,Agent Loop):`docs/ARCHITECTURE.md §2.5.1` 写"取消不立即终止,把'取消'作为 tool_result 回传 LLM,二次取消才真终止",当前实现单次 cancel 即 emit `Done("cancelled")` 终止。Fix 给 2 选项:**实现二次取消语义,OR 更新 spec 标'已偏离'**。
- DEBT §收尾路径建议第 3 条: "D3 自然碰 A-007 + A-010,做 D3 时是修这俩的天然窗口"

### 关键约束(代码现状)
- **chat_loop 已是单一权威**: 06-15 RULE-A-006 闭环后,`chat.rs` 缩减为薄 pre-flight 包装,agent loop body 全部路由到 `chat_loop::run_chat_loop`,改 1 处全生效
- **persist_turn 5 处**(`chat_loop.rs:284/638/674/831/871`):都是 `try { persist_turn } else { emit_persist_failure + return }` 或 cancel 路径 `tracing::error!`-only
- **emit_persist_failure** helper 在 `chat_loop.rs:957`,emit `ChatEvent::Error{Server}` + 终止
- **cancel 检查**:`chat_loop.rs:785` 是 tool 执行前 cancel check,RULE-A-004 闭环后 audit 移到 cancel check 之后
- **A-007 实际位置**:A-007 DEBT 行号 741-756 是 `chat.rs` 副本位置,迁移后应在 `chat_loop.rs` 868-880(error arm 持久化逻辑)

### 相关 spec(.trellis/spec/)
- **backend/agent-loop-architecture.md** — 16 阶段请求生命周期,turn 边界定义
- **backend/llm-contract.md** — `ChatEvent` variant 清单(2026-06-16 e410b67 增 Serde newtype 坑 + variant checklist)
- **backend/tool-contract.md** — tool_result / permission:ask IPC 协议
- **backend/error-handling.md** — Error 分类(InvalidRequest/Server/...)
- **backend/database-guidelines.md** — SQLite 表结构 + migration 流程
- **frontend/state-management.md** — Pinia store 拆分
- **frontend/memory-ui.md** / reka-ui-usage.md / popover-pattern.md — 弹窗/菜单参考

### 已实施参考(模式)
- **C4 审计日志 UI**(2026-06-14,`<AuditLogModal>`):reka-ui Dialog + 按 session 绑 + kind 下拉 + 计数 + 刷新,可借鉴 modal 模式
- **B2 PR3 InjectionRecord**(2026-06-17):前端 streamController + MessageItem 渲染消息附件,展示 metadata 与 message 共存,可借鉴"消息增强 UI"模式
- **审批内联到 ToolCallCard**(2026-06-16):从全局 modal 改为内联,可借鉴"减少弹窗"原则(但 edit/resend 用户主动操作,可能仍要 modal)

## Research References

* [`research/industry-edit-resend.md`](research/industry-edit-resend.md) — 8/9 主流工具收敛到"Edit = 删后续 + 重发"+ "Edit scope = 仅 user message" + "UI 入口 = hover message → ⋯ DropdownMenu"(reka-ui 直接套);持久化收敛到 in-place update + `edited_at` + `original_content`(可利用现有 `messages.metadata` JSON 字段,**无需新加列**)
* [`research/persist-patterns.md`](research/persist-patterns.md) — 推荐模式 1(in-place update)+ 模式 3 light(单列 `edited_at`);**否决** 模式 2(append-only+lineage,跟 `UNIQUE(session_id, seq)` 冲突)/ 模式 4(branch,跟单 session 心智冲突)/ 模式 5(snapshot,写放大);SQLite gotcha:FTS5 sync / 单事务包裹 / seq 续号(跟 `insert_system_event` `MAX(seq)+1` 模式一致);`PRAGMA journal_mode = WAL` 可同步加(零成本联动)

## Research Notes(关键收敛)

### 业界事实(2025-2026 现状)

| 工具 | Edit scope | Edit cascade | Resend | 持久化 | UI 入口 |
|---|---|---|---|---|---|
| Claude Code | user only | delete N+1..end | 跟 Edit 合并 | in-place + edit_history | hover → ⋯ menu |
| Cursor Composer | user only | delete N+1..end | 跟 Edit 合并 | in-place + edit_field | hover → ⋯ menu |
| Aider | user only | delete N+1..end | 跟 Edit 合并 | in-place | /regen 命令 |
| Cline | user only | delete N+1..end | 暴露独立 Resend | in-place | hover → ⋯ menu |
| OpenCode | user only | delete N+1..end | 暴露独立 Resend | in-place | hover → ⋯ menu |
| Continue | user only | **fork**(反例) | 暴露独立 Resend | lineage | hover → ⋯ menu |
| ChatGPT | user only | delete N+1..end | 跟 Edit 合并 | in-place | hover → ✎ icon |
| Slack/Discord | text only | n/a (chat) | n/a | in-place + edit_history | hover → ⋯ menu |

**绝对主流(8/9)**:Edit = 删后续 + 重发。Continue 的 fork 模式是反例(场景非 coding)。

### 我们 repo 的契合度(零成本映射)

| 业界方案 | 我们的对应 | 增量 |
|---|---|---|
| Edit 入口 = hover → ⋯ menu | reka-ui `DropdownMenu` | ~30 行前端 |
| Edit cascade = delete N+1..end | 新 `edit_user_message` Tauri command(pre-flight delete) | ~50 行后端 + 单事务 |
| 持久化 = in-place + edit history | `messages.metadata` JSON 字段(2026-06-17 增 `update_message_metadata` helper) | **0 列 migration** |
| Resend = 重新 send | `streamController.send()`(已有) | 0 行 |
| Stream race = abort + resend | cancellation token(已有 `chat_loop.rs:785`)+ busy lock(已有) | 0 行 |
| audit = 记 edit/resend | 新 `AuditKind::EditMessage` / `ResendMessage` | ~10 行 |
| 权限 = 豁免 ⑨(用户主动) | 不接 path-based check | 0 行 |

**结论**:D3 实施面 **后端 ~100-150 行 + 前端 ~100 行 + spec 3-4 份同步**,跟最近 B2 PR3 / C4 审计日志 UI 同量级。**D2 + D3 不应同 PR**(research 明确)。

## Assumptions (updated after research,待用户决策)

- **A1**: ✅ Edit 仅限 user message(8/9 工具一致)
- **A2**: ✅ Edit = 级联删后续 message + 重发(8/9 一致)
- **A3**: ✅ Resend = 重新 send 现有 user message(共用 send 流程,无新路径)
- **A4**: ✅ 持久化用 `messages.metadata` JSON 字段存 `edited_at` + `original_content`(零 migration,跟 `update_message_metadata` 模式一致)
- **A5**: ❓ A-007 同步修(error arm persist)— **同 PR 修是天然窗口,但增加 PR 体量**
- **A6**: ❓ A-010 选"更新 spec 标已偏离"(实现二次取消语义范围大,放独立 task)
- **A7**: ✅ UI = hover message → ⋯ DropdownMenu(reka-ui)+ 原地 textarea + 浮动 Save/Cancel
- **A8**: ✅ 权限豁免 ⑨(用户主动操作) + 新增 `AuditKind::EditMessage` / `ResendMessage`
- **A9**: ✅ Stream race 用 cancellation token 解决(已有)+ busy lock(已有)
- **A10**: ✅ seq 续号(跟 `insert_system_event` `MAX(seq)+1` 模式一致,`migrations.rs:512-516`)

## Open Questions (high-value,逐个问)

- **Q1(MVP 范围决定一切)**: 范围 = 极简(只 Resend)/ 中等(Edit user + Resend + 级联删,= A1+A2+A3+A4+A7+A8+A9+A10)/ 完整(中等 + A-007 修复 + A-010 spec 偏离声明)?
- **Q2**: Edit 模式 = 原地 textarea(简单)/ 浮动 popover(隔离)/ 全屏 modal(独立)— 影响前端实现量
- **Q3**: 持久化 `original_content` 是否保留?— 简单(resend 用)/ 完整(支持 undo edit)
- **Q4**: 新增 `AuditKind::EditMessage` / `ResendMessage` 是否进 C4 审计日志 UI?— 跟 C4 集成还是先只落表不暴露

## Requirements (evolving)

- 需求待 Q1 决策后细化

## Acceptance Criteria (evolving)

- 验收标准待 MVP 范围确认后定

## Definition of Done (team quality bar)

- Tests added/updated(后端 `chat_loop` 集成测试 edit 场景 + cascade delete 单事务 + stream race 取消;前端 MessageList/MessageItem 单测 edit UI)
- Lint / typecheck / CI green(`vue-tsc --noEmit` + `cargo check` 0 warning + `cargo test --lib` 全 pass)
- spec 同步更新:`backend/agent-loop-architecture.md` §turn 边界 + `backend/llm-contract.md` ChatEvent variant + `backend/database-guidelines.md` edit 持久化模式 + `frontend/state-management.md` edit flow
- 文档:`docs/IMPLEMENTATION.md` §4 加 ADR(本任务设计决策)
- 风险:edit race(用户编辑时 LLM 正在 stream)→ cancellation token + busy lock 验证;edit + cascade delete 单事务,失败走 `emit_persist_failure` 模式
- **`PRAGMA journal_mode = WAL`** 同步加(独立优化,跟 D3 几乎零成本联动,research 建议)

## Out of Scope (updated after research)

- edit assistant message(A1 一致)
- message version history / undo / branch(A2+A4 决定)
- 二次取消语义**实现**(A6 假设,先标"已偏离",放独立 task)
- 多 message 批量编辑
- 跨 session 复制 message(D2 范围,已降档)
- D2 + D3 同 PR(research 明确不应同做)

## Technical Notes

### 实施依赖(预想,精确行号以实施时 Read 为准)
- 后端:`agent/chat_loop.rs`(L957 `emit_persist_failure` helper 可复用)+ `commands/sessions.rs`(新 `edit_user_message` command)+ `db/sessions.rs`(in-place update,跟 `update_message_metadata` L742-763 同构)+ `db/migrations.rs`(可能加 `AuditKind` variant)
- 前端:`MessageList.vue` + `MessageItem.vue` + 新 `<MessageActionsMenu>` 组件 + `chat.ts` store(edit flow)
- spec:4 份(见 DoD)

### 关键不变量
- 改 chat_loop 改 1 处全生效(06-15 RULE-A-006 闭环)
- persist 失败必须 emit Error(06-15 RULE-A-003 闭环)+ audit 必须在 cancel check 之后(06-15 RULE-A-004 闭环)
- 前端 store 多 listener 增量更新,edit 不能破坏 stream 流式状态
- cascade delete + edit_user_message 单事务包裹(failure → `emit_persist_failure` 模式)
- seq 续号,业务 id 锁定 `(session_id, seq)`(跟 `find_message_id_by_seq` `sessions.rs:714-727` 一致)
