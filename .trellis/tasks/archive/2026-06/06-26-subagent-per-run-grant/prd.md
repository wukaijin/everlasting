# subagent web_fetch 审批 per-run 持久

## Goal

让 subagent（worker）运行期间，对 `web_fetch` 的权限审批可以"一次放行、本次运行内不再弹窗"。当前 worker 的权限弹窗被故意隐藏了"始终允许"按钮（后端把 worker AllowAlways 降级成当次有效），导致每次 web_fetch 都要审，体验很差。

核心张力：既要让 worker 运行期间不反复打断用户，又**不能**把授权泄漏到 parent session 的持久授权表（跨权限边界）。解法是引入一个**随 subagent run 生灭的内存级 grant cache**——既持久（run 内），又隔离（run 结束即销毁，不写 DB）。

## What I already know（已查证代码）

- **前端藏按钮**：`app/src/components/chat/DrawerPermissionAskCard.vue:126` `hideAllowAlways = !!props.ask.workerRunId` —— worker ask 强制隐藏"始终允许"。`PermissionAskBody.vue:224` 的按钮 `v-if="!hideAllowAlways"`。
- **后端降级**：`app/src-tauri/src/agent/permissions/ask.rs:287-306` worker `AllowAlways` 分支当 `AllowOnce` 处理，**不写** `session_tool_permissions`，注释写明理由"跨权限边界"。
- **worker 共享 parent session 授权表**：`dispatch.rs:351` worker 的 `session_id = parent_session_id`；`check.rs:257` `check_tool_grant(db, &ctx.session_id, "web_fetch")` 不区分 is_worker —— 所以 parent session 有 web_fetch grant 时所有 worker 自动放行。用户"每次都审"的根因是 parent 从没建过 grant + worker 按钮被藏 = 死循环。
- **per-run 生命周期载体已存在**：`SubagentBufferSink`（`agent/subagent/sink.rs`）是每个 worker run 独有的 `ChatEventSink`，run 结束即销毁。它已经持有 transcript / token usage 等 per-run 状态，是挂 grant cache 的天然位置。
- **spec 约束**（`.trellis/spec/backend/permission-layer.md`）：
  - §5b Validation Matrix line 290：worker AllowAlways 当 AllowOnce，前端藏按钮 —— **本次要修改的规则**。
  - §5b Audit note（RULE-A-016）：worker 决策**不写** `session_audit_events`，transcript 是 worker 的审计式记录。per-run cache 命中放行也必须遵守这条（transcript-only / 不写 parent audit）。
  - §5b `emit_permission_ask_resolved` 是 transcript-only，outcome ∈ {allow, deny, timeout, cancel}，worker AllowAlways 统一记 `"allow"`。新增"写 run cache"不应破坏这条 outcome 编码。
  - §4.1 Web Fetch 走 `match_kind='tool'`；§5b `PermissionContext` 扩展位（is_worker / worker_run_id 已是 PR1 扩展字段）。

## Assumptions (temporary, 待 research 验证)

- `PermissionContext` 在 `run_chat_loop` 内部 per-turn rebuild（spec §5b 注明），所以 per-run cache 必须是 `Arc` 共享句柄挂在 sink 上，ctx 每次重建时从同一个 sink 借用 —— **关键实现风险**，research 阶段需确认 sink 句柄能否在 ctx 构建处可见。
- per-run cache 命中放行时，worker 的 web_fetch tool_call/result 本身已经进 transcript，因此无需额外审计行（遵守 RULE-A-016）。
- `run_chat_loop` 的 22 个参数里已有 `worker_run_id`（PR2b 加），加一个 run-grants 句柄（或复用 sink）成本可控。

## Requirements (evolving)

### MVP（worker 全 Tier 4 ask 覆盖）
- [ ] worker 运行期间，对**任意 Tier 4 ask 工具**（`web_fetch` / `shell` Ask 档 / 仓库外 path 工具）的第一次"允许"可选择持久到当前 subagent run；同 run 内后续同类调用命中 cache 直接放行，不再弹 WorkerAskBanner。
- [ ] per-run grant **只存在于内存**，随 `SubagentBufferSink` 销毁而失效；**绝不写** `session_tool_permissions`（parent session 持久授权表零污染）。
- [ ] 前端 worker ask 卡片恢复"允许"档位的持久选项，文案明确表达"仅本次运行"语义（避免与主对话的"始终允许"混淆成 session 级）。
- [ ] AllowOnce（仅一次）**不写** cache；只有显式的"本次运行始终允许"才写。
- [ ] 遵守 RULE-A-016：cache 命中放行不写 parent `session_audit_events`；`emit_permission_ask_resolved` 的 outcome 编码不变（仍 `"allow"`）。
- [ ] Tier 2 hard kill（kill list）与 web_fetch SSRF blocklist **不受 per-run 授权影响**，仍先于/独立于 cache 生效（安全边界不变）。

### 机制（镜像 parent match_kind 语义）
per-run cache 数据结构镜像 `session_tool_permissions` 的三种 `match_kind`，只是存储 = 内存、scope = run：
- `tool` —— web_fetch（whole-tool，value=NULL）
- `prefix` —— shell（first token，如 `cargo test` → `cargo`）
- `path` —— 仓库外 path 工具（parent + `/*` glob，复用现有 `sqlite_glob_match`）

**写入**直接复用 `check.rs::match_value_for_allow_always`（已有函数，按 tool 类型产出 `(match_kind, match_value)`），不重复发明匹配规则。**查询**在 Tier 4 各分支"session grant miss → 将 emit ask"之前插一道 run-cache 查询，命中 → Allow。concurrent dispatch（L3a）下每个 worker 有独立 sink → 独立 cache，天然不串。

## Acceptance Criteria (evolving)

**web_fetch（match_kind=tool）**
- [ ] worker 第 1 次 web_fetch → 弹 WorkerAskBanner → 用户点"本次运行始终允许" → 放行 + 写 run cache（tool / NULL）。
- [ ] 同一 worker run 第 2 次 web_fetch → **不弹窗**，直接放行（命中 run cache）。

**shell（match_kind=prefix）**
- [ ] worker 第 1 次 `cargo test`（Ask 档）→ 点"本次运行始终允许" → 写 run cache（prefix / `cargo`）。
- [ ] 同 run 内 `cargo build` / `cargo check` → **不弹窗**（prefix `cargo` 命中）；`npm install`（不同 prefix）仍弹窗。

**path（match_kind=path glob）**
- [ ] worker 读仓库外 `/tmp/notes/a.md` → 点"本次运行始终允许" → 写 run cache（path / `/tmp/notes/*`）。
- [ ] 同 run 内读 `/tmp/notes/b.md` → **不弹窗**（glob 命中）；读 `/tmp/other/c.md`（glob 外）仍弹窗。

**通用**
- [ ] 同一 worker run 内点"仅一次"放行的调用，下一次同类调用仍弹窗（AllowOnce 不写 cache）。
- [ ] worker run 结束（Completed/Cancelled/Error/Incomplete）→ run cache 销毁；新开的 worker run 不继承上个 run 的授权。
- [ ] concurrent dispatch 下两个并行 worker 的 cache 互不串扰（各自独立 sink）。
- [ ] parent session 的 `session_tool_permissions` 表在整个流程中**无新行**（worker 授权零污染）。
- [ ] Tier 2 kill list 命中仍静默拒绝（不受 run cache 影响）；web_fetch SSRF blocklist 仍拦截（网络层独立于授权层）。
- [ ] parent 主对话的 web_fetch / shell / path 审批行为**不变**（仍是 session 级"始终允许"，仍写 DB）。
- [ ] `emit_permission_ask_resolved` outcome 仍是 `"allow"`，前端历史卡 badge 不回归。
- [ ] 前端：worker ask 卡片显示"本次运行始终允许"按钮；主对话 ask 卡片文案/行为不变。
- [ ] `cargo test --lib`（permissions 域）+ `pnpm test`（permissions store / DrawerPermissionAskCard）全绿。

## Definition of Done

- 后端单测：run cache 命中/失效、AllowOnce 不持久、run 结束销毁、parent session 零污染、outcome 编码不变。
- 前端单测：worker ask 按钮显隐 + 文案、主对话 ask 不回归。
- spec 更新：`permission-layer.md §5b` Validation Matrix + Audit note 同步改动（worker AllowAlways 不再降级 → 写 per-run cache）。
- DEBT.md 检查（无新增 / 无相关 open 项）。
- lint / typecheck / cargo check 绿。

## Out of Scope (explicit)

- per-URL / per-domain 的 web_fetch 授权（parent 侧也还是 deferred to PR3+，per-run 同步 OOS）。
- parent 主对话 web_fetch / shell / path 行为变更。
- parent session 级"手动 grant"UI（方案 ②，独立任务）。
- `subagent_audit_events` 独立表（spec 标注的未来工作）。
- SubagentDef frontmatter `preauthorized_tools` 字段（worker 启动即预填 cache，跳过首次审批）—— 有价值的未来演化，但 MVP 不预留死字段（YAGNI），加它只需 SubagentDef 增字段 + loader 解析 + dispatch 预填，不破坏当前设计。

## Technical Approach（self-research 后定稿）

**核心优化**：run cache 用独立 `Arc` 经 `run_chat_loop` 参数注入 `PermissionContext`，**`SubagentBufferSink` 零改动**（无需 downcast `dyn ChatEventSink`）。已核实 `permission_ctx` 在 `chat_loop.rs:428` **loop 外一次构建**（spec §5b "per-turn rebuild" 说法过时），三个 `check()` 调用点（1581/1838/1964）共用 `&permission_ctx` → ctx 加字段后三处自动生效。

1. **数据结构**：新类型 `RunGrantCache`（`Arc<Mutex<Vec<RunGrant>>>` + `grant_for_run(tool, input, path_or_cmd)` / `has_run_grant(tool, kind, value)` 方法），放 `agent/permissions/`（与 `store.rs` 同层，或并入）。`RunGrant { tool_name, match_kind, match_value }`。`grant_for_run` 内部复用 `check.rs::match_value_for_allow_always` 算 kind/value；`has_run_grant` 内部按 kind 做 tool/prefix/glob 匹配，glob 复用 `sqlite_glob_match`。
2. **传递**：`run_chat_loop` 增新参数 `run_grants: Option<Arc<RunGrantCache>>`；`PermissionContext` 增 `run_grants: Option<Arc<RunGrantCache>>` 字段，在 `chat_loop.rs:428`（一次构建）填入。dispatch.rs::run_subagent 创建 Arc 传 `Some`；parent 路径（chat 命令）+ 9 处 `tests_subagent.rs` 调用传 `None`（机械补参）。
3. **check.rs**：Tier 4 三分支（Path / Shell / WebFetch）在"session grant miss → 将 emit ask"之前各插一道 `if ctx.effective_is_worker() && ctx.run_grants.has(tool,...) → Allow`（+ audit/transcript 遵 RULE-A-016）。三 check 调用点自动生效。
4. **ask.rs worker 分支**：`AllowAlways` arm 改为 `ctx.run_grants.grant_for_run(...)`（写 run cache）而非丢弃；仍不写 DB。`AllowOnce` arm 不变。`emit_permission_ask_resolved` outcome 仍 `"allow"`。
5. **前端**：`DrawerPermissionAskCard.vue` `hideAllowAlways` 不再因 `workerRunId` 强制隐藏；`PermissionAskBody.vue` 按钮文案按 `workerRunId` 分流（主对话"始终允许" / worker"本次运行始终允许"）。decision wire 仍发 `"allow_always"`，语义在后端按 `is_worker` 分流。

## Implementation Plan（分 2 步，同 task）

- **Step 1 · 后端核心**（可独立验证）：`RunGrantCache` 类型 + `PermissionContext.run_grants` 字段 + `run_chat_loop` 新参数 + `dispatch.rs` 创建 Arc 传 `Some` + parent/chat 命令/`tests_subagent.rs` 9 处传 `None` + `check.rs` Tier 4 三分支查 cache + `ask.rs` worker `AllowAlways` 写 cache + 后端单测（命中/失效/AllowOnce 不持久/run 结束销毁/parent 零污染/shell prefix/path glob/outcome 不变）。**parent 路径 `run_grants=None` → 完全不查不写，行为零回归**。
- **Step 2 · 前端 + spec**：`DrawerPermissionAskCard.vue` 恢复 worker "允许"按钮（`hideAllowAlways` 不再因 `workerRunId` 强制隐藏）+ `PermissionAskBody.vue` 文案按 `workerRunId` 分流（"本次运行始终允许"）+ 前端单测（按钮显隐/文案/主对话不回归）+ `.trellis/spec/backend/permission-layer.md §5b` 同步（Validation Matrix line 290 + Audit note）。

## Research References（self-research，codebase 内部事实，已核实）

- `permission_ctx` 在 `chat_loop.rs:428` loop 外**一次构建**（非 per-turn），三处 `check()` 调用（1581/1838/1964）共用 → ctx 加字段零调用点改动。
- `sink: Arc<dyn ChatEventSink>` 是 trait object → run cache 走独立 Arc 注入 ctx，避免 downcast → **sink.rs 零改动**。
- `run_chat_loop` 已 23+ 参数（注释自承 "too many parameters"），加 `run_grants` 是同模式补参；调用点：dispatch.rs（worker，传 Some）+ chat 命令（parent，传 None）+ `tests_subagent.rs` 9 处（测试，传 None）。
- `SubagentBufferSink` 有 3 个构造器（new / new_without_app_handle / new_with_collector）—— 本方案不动它。
- 写入规则可复用 `check.rs::match_value_for_allow_always`（已有，按 tool 类型产出 `(match_kind, match_value)`）；glob 匹配可复用 `check.rs::sqlite_glob_match`（已有 `pub(crate)`）。

## Open Questions

- _（Q1 已关闭：MVP 覆盖 worker 全 Tier 4 ask —— web_fetch + shell Ask + 仓库外 path）_

## Technical Notes

### Expansion Sweep（diverge → 已收敛）
- **未来演化**：SubagentDef frontmatter 预授权工具（worker 启动即跳过首次审批）—— 记为未来演化，MVP 不预留死字段（见 Out of Scope）。
- **相关场景**：concurrent dispatch（L3a）多 worker 并行 —— 各自独立 sink → 独立 cache，天然隔离，无需额外处理。
- **失败/边缘**：worker 被 cancel / 崩溃 → sink 销毁 → cache 自动失效（无泄漏）；Tier 2 kill list + SSRF blocklist 独立于授权层，安全边界不变；worker 单 turn 串行执行，cache 读写无竞争（仍用 Mutex 保险）。

### 涉及文件
- 后端：`agent/permissions/{check,ask,types}.rs` + 新 `RunGrantCache`（新文件或并入 `store.rs`）、`agent/subagent/dispatch.rs`（创建 Arc + 传参）、`agent/chat_loop.rs`（签名 + ctx 构建 + parent 调用点补 None）、`agent/tests_subagent.rs`（9 处补 None）。
- 前端：`components/chat/DrawerPermissionAskCard.vue`、`components/chat/PermissionAskBody.vue`（文案分流）。
- spec SOT：`.trellis/spec/backend/permission-layer.md §5b`（Validation Matrix line 290 + Audit note 需同步：worker AllowAlways 不再降级 → 写 per-run cache；前端不再藏按钮）。

### 安全边界
per-run cache 是"用户在本次 run 内的显式授权"内存缓存，不绕过 Tier 2 hard kill（kill list 先于一切），不绕过 web_fetch SSRF 防护（`tools/web_fetch.rs` blocklist 独立生效）。隔离原则（RULE-A-016）保持：不写 parent `session_audit_events`、不写 `session_tool_permissions`。
