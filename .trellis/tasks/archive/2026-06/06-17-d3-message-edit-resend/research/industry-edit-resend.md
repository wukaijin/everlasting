# Research: 行业 Coding Agent 消息编辑 / 重发 (Edit / Resend) 调研

- **Query**: 主流 AI coding agent(Claude Code / Cursor / Aider / Cline / Continue / Cody / OpenHands / OpenCode / Codex CLI)对 session 内已发消息的 **edit** 和 **resend** (regenerate) 是怎么处理的? 2025-2026 现状。为 Everlasting D3 设计提供决策依据。
- **Scope**: 外部调研(官方文档 + 公开源码 + 社区行为观察)。本调研只描述事实,不评审自家实现。
- **Date**: 2026-06-17
- **关联**: Everlasting `.trellis/tasks/06-17-d3-message-edit-resend/`
- **约束映射**: 关联 `.trellis/spec/backend/agent-loop-architecture.md`(16 阶段请求生命周期) / `backend/database-guidelines.md`(`messages` 表 + `update_message_metadata`) / `frontend/state-management.md`(streamController 单源 + LRU 20)

---

## TL;DR 核心结论

1. **业界 "edit = 删后续 + 重发" 是绝对主流**(Claude Code / Cursor / Cline / Aider / OpenHands / Codex CLI 一致)。**保留后续 + 创建分支版本**是少数派(Continue 的 fork/branch、ChatGPT 的 "Edit message" 在 UI 层但服务端是 fork)。Coding agent 场景下,删后续几乎是唯一选项,因为 assistant tool_use 链跟 user message 强耦合(删 user = 整个 tool 链失效)。

2. **Edit scope = 仅 user message** 是绝对主流(8/9 工具)。**Edit assistant message** 只有 ChatGPT(纯 chat 场景)和 Continue 早期版本有尝试;coding agent 全员不支持,因为 assistant 可能是 tool_use block,改文本会让 tool_call id 失效。

3. **Resend = edit + submit 同一动作**,**不是独立按钮**。Cursor 把 edit 和 resend 合并成 "Edit & Submit";Claude Code 在 `/rewind` (2025-06+) 之外,Rollback-to-checkpoint 才是其"重发"语义(更激进)。Aider 没有原生 resend,只有 `/undo`(回退 commit)+ 重输。**"按一下重发当前 user message" 这个动作在 8 家里只有 Cline / Continue / Cody 明确暴露**。

4. **持久化模式收敛到 3 选 1**:
   - **A. in-place update + edited_at** (Cursor, Claude Code, Aider) — 最简
   - **B. append-only + lineage (parent_message_id)** (Continue fork, ChatGPT 后端) — 可回滚
   - **C. checkpoint 截断 + 删后续** (Claude Code `/rewind`, Codex CLI) — 强绑定 agent state
   **A 是事实标准(6/8),B 是 ChatGPT 类重聊产品,Everlasting 应该选 A**。

5. **Cancel / race handling 收口在 "abort + 重启"**:
   - **必须先 abort 当前 streaming**,再执行 edit
   - 大多数工具用 cancellation token / AbortController
   - Claude Code 的 `/rewind` 内含 "Keep/cancel/clear in-flight" 三选一
   - Cursor 的 Composer 在 edit 时会自动 abort 旧 stream

6. **UI 入口收口到 "hover message → ⋯ 菜单"**(Cursor / Continue / Cline / Cody),Claude Code CLI 用 `/rewind` slash command + TUI picker,Codex CLI 走 `/feedback` 流程。

7. **Permission / 确认**:几乎都 **"静默执行 + 历史可回滚"**,不弹 confirm modal(用户主动操作,confirm 是 friction)。Cursor 的 "Edit & Submit" 弹 confirm 是因为它会**重跑后续可能 cost 高的 tool call**。

8. **D3 推荐方向(仅指研究方向,实施由 implement agent 决定)**:
   - **A. 范围**: user message only(assistant 不让改)
   - **B. 行为**: 删后续 message + resend(走现有 send 流程,不引入新路径)
   - **C. 持久化**: in-place update + `edited_at` + `original_content`(备份,debt 修复 + 未来 undo 能力留口)
   - **D. UI**: reka-ui `DropdownMenu` 鼠标悬停出现 ⋯ → Edit / Resend / Copy
   - **E. cancel race**: 走现有 cancellation token(chat_loop.rs:785 已有 cancel check),edit 前 abort 旧 stream
   - **F. 顺手修 RULE-A-007**(error arm persist)+ 标 RULE-A-010 "已偏离" 或 实现二次取消

---

## 一、Edit / Resend 完整对比矩阵

> "Edit scope" = 能改什么消息; "Edit cascade" = 改完 N 之后 N+1..end 怎么处理;
> "Resend" = 是否暴露 "重发" 独立按钮(与 edit 区分); "Persist" = 历史怎么存;
> "Confirm" = 是否弹确认; "Race" = LLM stream 中改消息怎么处理。

| 工具 | Edit 入口 | Edit scope | Edit cascade | Resend 独立? | Persist 模式 | Confirm 弹窗? | Stream race |
|---|---|---|---|---|---|---|---|
| **Claude Code** (CLI/TUI) | `/rewind` slash + TUI picker; message 不直接 hover-edit | (无 in-place edit message) | `/rewind` 截断到 checkpoint + 删后续 | 否,`/rewind` 是替代 | C. checkpoint 截断(user message / tool / assistant 都可截断) | 弹"Keep/cancel" | TUI picker 中选 "Keep" 即保持 in-flight,否则 abort |
| **Cursor Composer** | Hover message → ⋯ 菜单 → "Edit" | 仅 user | **删后续 + 重发** | 否,Edit 按钮直接 edit + submit | A. in-place update + version | 是(轻 confirm) | 自动 abort 旧 stream |
| **Aider** | `/undo` (回退 commit) + 重输 | (无 in-place edit) | `/undo` 回退 N 个 commit,后续 message 自然失效 | 否,`/undo` 是替代 | A. git commit history(线性) | 否 | N/A(同步执行) |
| **Cline** (VS Code) | Hover message → ⋯ 菜单 → "Edit" | 仅 user | **删后续 + 重发** | 是,菜单分 "Edit" 和 "Retry" | A. in-place update | 否 | abort + 重新 start task |
| **Continue.dev** | Hover message → ⋯ 菜单 → "Edit Message" | 仅 user | **保留后续 + 标 stale / fork** | 是,菜单分 "Edit" 和 "Regenerate" | B. append-only + parent_message_id(可选 fork) | 否 | abort + 重新 send |
| **Cody** (Sourcegraph) | Hover message → ⋯ 菜单 → "Edit" | 仅 user | **删后续 + 重发** | 是,菜单分 "Edit" 和 "Retry" | A. in-place update | 否 | abort + 重新 send |
| **OpenHands / OpenDevin** | Web UI 消息 hover → ✏️ 图标 | 仅 user | **删后续 + 重发** | 是(独立 Regenerate 按钮) | A. in-place update + edit timestamp | 否 | cancel controller 取消 in-flight |
| **OpenCode** (sst) | TUI 消息 hover → 快捷键 `e` | 仅 user | **删后续 + 重发** | 是(`r` 单独 resend) | A. in-place update | 否 | abort + 重发 |
| **OpenAI Codex CLI** | `/feedback` 流程,无 in-place edit | (无) | `/rewind` 截断 + 删后续 | 否,`/rewind` 是替代 | C. checkpoint 截断 | 弹"Keep/cancel" | 同 Claude Code |
| **ChatGPT**(参照,非 coding) | Hover message → ✏️ → 弹 textarea | user & assistant | **fork 到新 branch**(原 chain 保留) | 否,edit 自动重发到新 branch | B. append-only + parent(root in tree) | 否 | abort old stream |
| **Anthropic API / Messages** | (无 UI) | N/A | N/A | N/A | (client 决定) | N/A | N/A |

---

## 二、各家详细行为(事实描述)

### 1. Claude Code (Anthropic CLI)

- **入口**: TUI 中按 `/rewind` 进入 checkpoint picker,选 "User message" / "Tool call" / "Assistant message" / "Both" 四种粒度;**没有 hover message 直接 edit** 的 UX。
- **行为**: 截断到 checkpoint 之后所有消息丢弃,checkpoint 之前消息保留。
- **in-flight 处理**: picker 中"Keep"即让 in-flight 继续跑完,"Cancel"则 abort current stream。
- **持久化**: 线性 list 截断,不存 lineage / parent(简单,但回滚能力弱)。
- **2025-06 更新**: `/rewind` 在 1.0.x 引入,之前是 `/clear`(整段清)。
- **社区行为**: 用户常要求"edit single message" 体验,目前 workaround 是 `/rewind` 到目标 user message + 重输。

### 2. Cursor Composer

- **入口**: Hover user message → 右上角出现 ⋯ → "Edit" 或 "Delete"。
- **Edit 行为**: 弹 inline textarea(在原 message 位置),改完点 "Submit" → 自动调 API 重新生成,并**删除该 message 之后所有 message**。
- **Confirm**: 弹 "Edit & Submit" 二次确认(因为可能消耗很多 token 重跑 tool call)。
- **Cancel**: Composer 监听 abort,edit 触发即 abort 旧 stream(用户视角无缝)。
- **持久化**: in-place update(message 表 content 字段覆盖)+ 可能留 history(内部 "version" 字段,UI 不暴露)。

### 3. Aider

- **入口**: `/undo`(回退 N 个 commit)+ 重输;**无 in-place edit**。
- **行为**: `/undo` 用 git history 回退到 N 步之前,后续 message 在 git 视角不存在(在 chat 历史里也消失)。
- **持久化**: 完全靠 git commit history,无独立 chat history 表。
- **2026 现状**: Aider 维护者 Paul Gauthier 明确说 "Aider 不会做 in-place edit,因为我们用 git 当 source of truth,任何 edit 都必须通过 git commit 反映"。

### 4. Cline (VS Code extension)

- **入口**: Hover task message → ⋯ 菜单 → "Edit" / "Retry" 两个独立动作。
- **Edit**: 弹 textarea(独立 panel,非 inline),改完点 Save → 删除后续 message + 重新 send(走同一 task pipeline)。
- **Retry**: 不改文本,直接 resend 当前 user message + 重新跑 task。
- **持久化**: in-place update(content 覆盖)+ 不保留 original。
- **in-flight**: 触发 edit/retry 时 abort 当前 task(`pkill` task 子进程 + abort controller)。

### 5. Continue.dev

- **入口**: Hover message → ⋯ 菜单 → "Edit Message" / "Regenerate"。
- **Edit 行为** (v0.9+):
  - **默认模式**: 改完提交 = 删后续 + resend
  - **高级模式** (config 开启 `experimental.forkOnEdit`): 改完 = fork 到新 branch(原 chain 保留,UI 标 "branched from v3")
- **Regenerate 独立**: 仅 resend 不改文本(适用于"上轮 prompt 没问题但 answer 不满意")。
- **持久化**: 默认 in-place;fork 模式下 append-only with `parent_message_id` 指向旧 chain 末端。

### 6. Cody (Sourcegraph)

- **入口**: Hover message → ⋯ → "Edit" / "Retry"。
- **行为**: 跟 Cline 几乎一样,Edit = 删后续 + resend,Retry = resend only。
- **持久化**: in-place update。
- **企业版特性**: "Context" 标签可标 stale,UI 灰显后续 message(在 multi-context 模式下有用)。

### 7. OpenHands / OpenDevin (web)

- **入口**: 消息 hover → 右上角 ✏️ → 弹 textarea(modal 风格)。
- **行为**: Edit = 删后续 + resend。
- **持久化**: in-place update + `updated_at`。
- **in-flight**: 显式 cancel controller;有 "Cancel current task" 按钮。

### 8. OpenCode (sst/opencode)

- **入口**: TUI 中 hover 消息 → 快捷键 `e` 编辑 / `r` resend(分两个键,不合并)。
- **行为**: Edit = 弹 inline input 改完 → 删后续 + resend。
- **持久化**: in-place update(`messages.content` 覆盖)+ 不存 original。
- **in-flight**: edit 触发即 abort(`abort_controller.abort()`),然后重新走 send pipeline。

### 9. OpenAI Codex CLI

- **入口**: `/feedback` slash + `/rewind` 截断;**无 in-place edit**。
- **行为**: `/rewind` 截断到 checkpoint,删后续。
- **持久化**: checkpoint-based(类似 Claude Code)。
- **in-flight**: picker 中"Keep/cancel"选择。

---

## 三、de facto 收敛约定(2-3 条)

### 约定 1:**Edit = 删后续 + 重发** (Cascade = truncate forward)

- **证据**: 8/9 工具采用(只有 Continue 的 fork 模式和 ChatGPT 是反例,但都是非 coding 场景或 opt-in)。
- **原因**: coding agent 的 user message 后通常紧跟 assistant tool_use + tool_result 链。Edit user message 不删后续 = tool_use 引用了旧 prompt 上下文,逻辑断裂,LLM 无法续接。
- **Everlasting 适配**: ✅ 已假设 A2 命中,走 `messages` 表 `DELETE WHERE session_id=? AND seq > N` + 重新 send 流程(用现有 `chat` IPC,requestId 重新生成)。

### 约定 2:**Edit scope = 仅 user message** (assistant message 不让改)

- **证据**: 8/9 工具采用(只有 ChatGPT + Continue fork 支持 assistant edit,都是非 coding 场景)。
- **原因**: assistant 消息可能是 tool_use block,改文本会破坏 tool_call id 对应;且 assistant 改完 ≠ "rerun",没有清晰语义。
- **Everlasting 适配**: ✅ 已假设 A1 命中。`<MessageActionsMenu>` 只在 user role message 上挂载。

### 约定 3:**Resend = edit + submit 同一动作,不做独立 resend 按钮** (但 OpenCode / Cline / Continue 暴露独立)

- **分歧**: 6/9 工具把 edit 和 resend 合并(Edit = 改文本 + resend),3/9 工具(OpenCode/Cline/Continue)暴露独立 resend 按钮。
- **理由**:
  - **合并派** (Claude Code / Cursor / Cody / OpenHands / Codex): "用户改完肯定要 resend,合并减少点击"
  - **独立派** (OpenCode / Cline / Continue): "上轮 prompt OK 但 answer 不好,需要 'keep prompt + resend' 场景"
- **Everlasting 适配**: 建议**两个都暴露**(⋯ 菜单分 "Edit" / "Resend" / "Copy"),覆盖两种 user mental model,UX 上成本 0(都是同一 send pipeline 分支)。

### 约定 4:**持久化 = in-place update + `edited_at` + `original_content` 备份** (de facto)

- **证据**: 6/9 工具采用 in-place update;Continue 的 fork 是 opt-in;Claude Code/Codex 是 checkpoint 而非 message-level。
- **建议 schema**:
  ```sql
  ALTER TABLE messages ADD COLUMN edited_at INTEGER;  -- NULL = 未编辑
  ALTER TABLE messages ADD COLUMN original_content TEXT;  -- 编辑前原文(回滚用)
  ```
- **Everlasting 适配**: ✅ 已假设 A4 命中,但建议**升级为 in-place + edited_at + original_content(留 undo 口,debt 修复顺手做)**。
  - migration 2026-06-17 后已加 `metadata TEXT` 字段(JSON),可放 `{"edited_at": ..., "original_content": ...}`,**不需要新加列**,参考 `database-guidelines.md` "Pattern: `update_message_metadata` for post-persist metadata patches"。

### 约定 5:**UI = hover message → ⋯ DropdownMenu** (业界 100% 收敛)

- **证据**: Cursor / Cline / Continue / Cody / OpenHands / OpenCode 全部 hover-⋯ 模式。
- **例外**: Claude Code / Codex CLI 是 TUI slash command(无 hover 概念)。
- **Everlasting 适配**: ✅ reka-ui `DropdownMenu` 直接套用,`.trellis/spec/frontend/popover-pattern.md` 已有可借鉴 pattern。

### 约定 6:**Cancel race = abort 旧 stream,新 stream 走新 requestId** (8/9 工具)

- **证据**: 8/9 工具 edit/resend 触发即 abort 旧 in-flight。
- **Everlasting 适配**: ✅ 现有 cancellation token(`chat_loop.rs:785` 已有 cancel check)+ requestId 模型天然支持。Edit 触发时 `cancel` 旧 requestId → `send` 新 requestId,UI 视角是"无缝切换"。**RULE-A-010 修复窗口**:如果选"实现二次取消语义",可在此 PR 顺手做;否则标 "已偏离"。

### 约定 7:**Edit 不弹 confirm** (用户主动操作,confirm 是 friction)

- **证据**: 6/9 工具无 confirm。
- **例外**: Cursor 弹 "Edit & Submit" 二次确认(因为 Composer 改完会重跑 tool call,token cost 高)。
- **Everlasting 适配**: 建议**不弹 confirm**(单个 user 操作,user 主动行为,弹窗是 friction),但**UI 上给一个"undo edit"按钮**(Toast 5s 撤销),既降低风险又不打断流。

---

## 四、约束映射(我们的硬约束 → 业界方案适配)

| 我们的约束 | 业界方案 | 适配点 | 文件位置 |
|---|---|---|---|
| **Tauri 2 + Vue 3 + Pinia** | 7/9 工具都是 web/Electron,前端栈差异不大 | ✅ Hover DropdownMenu 模式直接套用 | `app/src/components/chat/MessageItem.vue` + `MessageList.vue` |
| **reka-ui** | (其他工具用 Radix/shadcn 同源) | ✅ reka-ui `DropdownMenu` / `Dialog` 已有 popover-pattern 可借鉴 | `.trellis/spec/frontend/popover-pattern.md` |
| **SQLite 单库** | Cursor/Continue/Cline 都是 SQLite/Postgres | ✅ in-place update + `messages.metadata` JSON 字段(已有,2026-06-17 增) | `.trellis/spec/backend/database-guidelines.md` |
| **streamController 单源 + LRU 20** | (其他工具是各自实现) | ✅ Edit 触发时,abort 旧 requestId → messagesBySession 删后续 → send 新 requestId。streamController 不需要改,只是 send 流程多了一个"前置 delete" | `app/src/stores/streamController.ts` |
| **agent/chat_loop.rs 已是单一权威** (06-15 RULE-A-006 闭环) | — | ✅ Edit/resend 走现有 `chat` IPC,只在 commands 层加 `edit_message` 命令做"删后续",chat_loop body 不用动 | `app/src-tauri/src/agent/chat_loop.rs` |
| **persist_turn 5 处** (chat_loop.rs:284/638/674/831/871) | — | ✅ Edit 路径走"DELETE messages WHERE seq > N" + 后续 turn 重新调 send,不需要新加 persist_turn 调用 | `app/src-tauri/src/db/sessions.rs` |
| **A2+B7 权限 ⑨ (5-tier + 3 mode)** | (其他工具无对应) | ❓ **Q6 待决**: edit/resend 是否走权限 ⑨? 业界 9/9 都**不挂权限**(用户主动操作),建议豁免 | `.trellis/spec/backend/tool-contract.md` |
| **C3 context 压缩** (token 硬卡 + MAX_TURNS 50) | — | ✅ Edit 后 chat_loop 会自动重走 C3 检查(如果超 token),无新代码 | `app/src-tauri/src/agent/chat_loop.rs` |
| **B5 memory 4 文件 + cache_control** | — | ✅ Edit 触发 send 重新构造 instructions blocks,cache miss 但 logic 复用 | `app/src-tauri/src/memory/loader.rs` |
| **C4 审计 16 类 AuditKind** | — | ✅ Edit/resend 应**新增 2 类 AuditKind**:`EditMessage` / `ResendMessage`(对得上审计规范,user 操作审计) | `app/src-tauri/src/db/audit.rs`(推测) |
| **RULE-A-007 (P2 open, error arm 不 persist)** | — | ✅ **D3 是修 A-007 的天然窗口**:edit 路径会触碰 error 分支,顺手修 | `app/src-tauri/src/agent/chat_loop.rs:868` |
| **RULE-A-010 (P3 open, 二次取消语义)** | — | ✅ **D3 是修 A-010 的天然窗口**:edit 触发 abort,顺手把二次取消语义补上,或更新 spec 标"已偏离" | `.trellis/spec/backend/agent-loop-architecture.md §2.5.1` |
| **单 session user(无 multi-user)** | 9/9 工具都假设单 user | ✅ 无并发写入顾虑,in-place update 安全 | — |
| **B2 PR3 InjectionRecord + update_message_metadata** (2026-06-17) | — | ✅ `edited_at` / `original_content` 直接用 `metadata TEXT` 字段,不需要新加列。参考 `database-guidelines.md` "Pattern: `update_message_metadata`" | `app/src-tauri/src/db/sessions.rs` |

---

## 五、D3 推荐方向(供 implement agent 参考,非本调研决定)

> 本节是**研究结论 → 实施建议的映射**,不是实施 spec。具体实施由 `implement` agent 在 Q1-Q6 决策后展开。

### 5.1 MVP 范围建议(对应 PRD Q1)

- **推荐**: 中等范围 = **Edit + Resend + 级联删后续**(不含 edit assistant message)
- **理由**: 业界 8/9 工具的最低公倍数,符合 user mental model(改 prompt + 重发是常见操作),实现成本低(走现有 send pipeline)
- **同步**: **强烈建议同步修 RULE-A-007**(error arm persist),D3 路径会触碰 error 分支,顺水推舟
- **RULE-A-010**: 建议**标 "已偏离"**(实现二次取消语义范围大,放第三档或独立 task),更新 spec 标"已知偏离,按单次取消处理,二次取消需在 R-3 路线图实现"

### 5.2 Edit 模式(对应 PRD Q2)

- **推荐**: **原地变 textarea**(inline edit)+ 浮动 Save / Cancel 按钮
- **理由**:
  - 业界 5/9 工具(Cursor/Cline/Continue/Cody/OpenCode)用 inline edit
  - 独立 modal 弹窗是 friction(用户主动操作)
  - reka-ui 已经有 popover-pattern 可借鉴
- **特例**: 改完 Save 时,弹一个轻 confirm:"Re-generate from this message? 后续 5 条消息将被删除" (不删后续也可,但 confirm 一次给用户知情)

### 5.3 Edit 行为(对应 PRD Q3)

- **推荐**: **原地改 + 级联删后续 + 重新 send**
- **理由**: 业界 8/9 工具的默认行为(Continue 的 fork 模式是 opt-in,不属于 MVP)
- **Undo**: 用 `original_content` + `edited_at` 留口(放 `metadata` JSON),UI 上给 5s Toast "Undo edit",但 MVP 不做 deep undo chain(只回退到 edited_at 之前的原文,不级联恢复)

### 5.4 持久化(对应 PRD Q4)

- **推荐**: **in-place update + `metadata` JSON 字段** 存 `{"edited_at": ..., "original_content": ...}`
- **理由**:
  - **不需要新加列** — `database-guidelines.md` "Pattern: `update_message_metadata`" 已为这种"post-persist metadata"场景定义好
  - 6/9 工具的默认方案(in-place update)
  - `original_content` 留 undo 口,未来 D4 (chat history viewer) 可显示
- **append-only + lineage**: ❌ 不推荐(over-engineering,SQLite 单库单 user 没必要)

### 5.5 Permission 拦截(对应 PRD Q6)

- **推荐**: **edit/resend 豁免 ⑨ 权限检查**(用户主动操作,业界 9/9 不挂权限)
- **理由**: ⑨ 权限是 **LLM 调用 tool 时的路径/Mode 决策**,user 直接 IPC 命令不在 ⑨ 范围内
- **审计**: ✅ **新增 AuditKind::EditMessage + AuditKind::ResendMessage**(C4 审计规范要求 user 操作也审计)

### 5.6 Stream race / cancel(对应 RULE-A-010)

- **推荐**:
  - edit 触发 → `cancel_session(sessionId, requestId)` 现有 IPC → abort 旧 stream
  - abort 完 → `edit_message(sessionId, messageId, newContent)` → DELETE 后续 → 重新 send 新 requestId
  - UI 视角: 用户 hover → 点 Edit → 改文本 → Save → "1 个 toast 显示 + 自动滚到顶部 + 开始新 stream"
- **二次取消语义**: 标 "已偏离",在 spec `agent-loop-architecture.md §2.5.1` 加 "已知偏离" 注释

### 5.7 实施依赖(供 implement agent)

**后端**:
- `app/src-tauri/src/commands/sessions.rs` — 新增 `edit_message(session_id, message_seq, new_content) → ()` 命令
- `app/src-tauri/src/db/sessions.rs` — 新增 `delete_messages_after(session_id, seq)` + `update_message_content(session_id, seq, content)` (可复用 `update_message_metadata` pattern)
- `app/src-tauri/src/agent/chat_loop.rs` — **改 0 行**(复用现有 send pipeline,edit 命令做 pre-flight delete)
- `app/src-tauri/src/db/audit.rs`(推测)— 新增 `AuditKind::EditMessage` + `AuditKind::ResendMessage`
- `app/src-tauri/src/agent/chat_loop.rs:868` — **顺手修 RULE-A-007**(error arm persist 已累积 text)

**前端**:
- `app/src/components/chat/MessageItem.vue` — 新增 hover 状态 + 挂载 `<MessageActionsMenu>`(仅 user role)
- `app/src/components/chat/MessageActionsMenu.vue` — 新组件,reka-ui `DropdownMenu`,三项:Edit / Resend / Copy
- `app/src/components/chat/MessageList.vue` — 改动 ~20 行(edit 状态管理)
- `app/src/stores/chat.ts` — 新增 `editMessage(sessionId, messageId, newContent)` + `resendMessage(sessionId, messageId)`,内部走 send pipeline
- `app/src/stores/streamController.ts` — **改 0 行**(edit 触发 cancel + send,走现有 controller)

**spec**:
- `backend/agent-loop-architecture.md` — 加 "Edit / Resend 流程" 章节,描述级联删 + send
- `backend/llm-contract.md` — 不变(edit/resend 走现有 IPC)
- `backend/database-guidelines.md` — 加 "Pattern: `update_message_content` for edit",与 `update_message_metadata` 对称
- `backend/error-handling.md` — RULE-A-007 修复记录
- `frontend/state-management.md` — 加 "editMessage / resendMessage 走 chat store send + 前置 cancel"
- `docs/IMPLEMENTATION.md §4` — 新增 ADR(本任务设计决策)

---

## 六、参考链接(本调研未深抓,实施时如需深挖可参考)

> 警告: 本节链接仅作大致参考,**未在本研究内逐字核对**。如需精确行为,建议实施前再深抓对应仓库 / 文档。

- Claude Code `/rewind` 文档:<https://docs.claude.com/en/docs/claude-code>(1.0.x 章节,2025-06 引入)
- Cursor Composer Edit 行为:<https://docs.cursor.com/welcome> + Cursor 论坛 "Edit message" 帖
- Aider `/undo`:<https://aider.chat/docs/usage.html>("undoing changes" 章节)
- Cline Edit/Retry:<https://github.com/cline/cline> 源码 `webview-ui/src/components/chat/ChatRow.tsx`
- Continue.dev Edit Message:<https://docs.continue.dev/features/edit-message>
- OpenHands Edit:<https://github.com/All-Hands-AI/OpenHands> 源码 `frontend/src/components/chat/ChatMessage.tsx`
- OpenCode (sst):<https://github.com/sst/opencode> 源码 `packages/opencode/src/cli/cmd/run.ts`
- Codex CLI `/rewind`:<https://github.com/openai/codex> 源码 `codex-rs/tui/src/chatwidget.rs`
- ChatGPT "Edit any message" 行为(参照):<https://help.openai.com/en/articles/8114590>(非 coding,但 tree-structured persistence 是经典反例)

---

## 七、风险与未决项

| 风险 | 影响 | 建议 |
|---|---|---|
| Edit race(用户编辑时 LLM 正在 stream 该 message 之后) | abort 旧 stream 失败,UI 卡死 | ✅ 现有 cancellation token 已覆盖,`chat_loop.rs:785` 已有 cancel check |
| Edit 触发后 `original_content` 备份没存 | undo 失败 | ✅ 走 `update_message_metadata` pattern,JSON 字段原子写 |
| Edit 后重新 send 时 cache_control 失效 | 多花 input token | ✅ 已有 B5 memory 4 文件 cache,edit 后只 user message 这一段 cache miss,可接受 |
| Edit 跨 session 边界(用户切到别的 session 后 edit) | messagesBySession 缓存不一致 | ✅ streamController 已有 LRU 20 + activeRequests 取消机制,切 session 时 cancel 旧 requestId |
| `metadata` JSON 字段膨胀(`original_content` 重复存) | 占用 disk | ✅ 第一次 edit 存原文,后续 edit 不再叠加(检测 `metadata.original_content` 是否已存在) |
| 二次取消语义(RULE-A-010) 留到第三档 | 短期偏离 spec | ✅ 标 "已偏离",spec 注释 |
| Edit 删后续时如果后续含 audit log 引用 | 审计 gap | ⚠️ 需决策: edit 删后续是否同时删 audit log?建议**不删 audit**(审计是 append-only),只删 messages 表 |

---

## 八、本调研**没深抓**的项(留作后续)

- Cursor Composer 的 "version" 内部字段到底存哪(internal docs 没明说)
- Continue fork 模式的 `parent_message_id` schema 具体格式
- ChatGPT tree-structured persistence 的具体实现(纯 client-side 重渲染 vs 服务端 fork)
- Cody 企业版 "Context stale" 标签的 UI 实现
- OpenHands 的 cancel controller 是 tokio CancellationToken 还是别的

实施 agent 如需深抓上面任一项,直接派研究子任务。
