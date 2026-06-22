# Review Backlog — 跨 review 债务整合

> **目的**: 集中追踪所有 review(审计 / SPEC 对照 / 历史 review)的 finding,避免下次 audit 重新独立复述
>
> **基线审计**: `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md`(commit `a4fb302`)
>
> **创建**: 2026-06-14(由 `.trellis/tasks/06-14-review-debt-consolidation` 启动)

---

## 新增 finding 流程

> **重要**: 任何新 audit / review / spec 对照,**第一步必须 diff 本文件**。

### 添加新 finding

```markdown
### RULE-{Subsystem}-{Seq}

- **Level**: P0 | P1 | P2 | P3
- **Subsystem**: Agent Loop | Permission | Memory | Provider | Tools | Cross
- **File**: `path/to/file.rs:LINE`
- **Description**: 一句话描述问题
- **Fix**: 修复方向(行数估算)
- **Status**: open | in_progress | closed
- **Owner**: carlos | 待分配
- **Related Task**: `.trellis/tasks/XX-YY-name` 或 null
- **Discovered In**: `docs/_reviews/REVIEW-XXX.md`
- **Closed At**: commit hash 或 null
- **Related PR**: #N 或 null
```

### 流程规则

1. **不重新展开已记录 finding**: 新 audit 中遇到已记录的 RULE-X-XXX,**只标一行** `// See DEBT.md §RULE-X-XXX`,不重新描述 file:line 和影响
2. **闭合时填 commit**: PR merge 后必须更新 `Closed At` 和 `Related PR`
3. **优先级重审**: 每次 audit 可重新评估,但需在本文件 `Re-evaluation Log` 段记录理由
4. **ID 一旦分配不变**: 即使 finding 后续证明不是问题,ID 保留但 Status 标 `wontfix`

### 复述检测

如果新 audit 复述了某条 finding 但未引用 DEBT.md:
- **轻度**: review 本身不扣分,但应在结论段标注"漏查 DEBT.md"
- **重度**: 如果是 P0/P1 漏查,review 应被打回修订

---

> **本文件仅记录当前 open 债项**。已 closed 条目不在此保留;通过 git log 或 §Re-evaluation Log 追溯。

## P1 — 重要(正确性 + 资源) [1 items]

### RULE-D-001 — API key 明文存储

- **Level**: P1
- **Subsystem**: Provider
- **File**: `app/src-tauri/src/db/migrations.rs:240` + `commands/providers.rs:38-42` + `db/providers.rs:62-82`
- **Description**: `api_key TEXT NOT NULL DEFAULT ''` 原样写读返回 IPC;`app_data_dir` 权限 0700 非绝对边界
- **Impact**: DB 文件泄露=全部 provider key 泄露
- **Fix**: keyring crate(macOS Keychain / Windows Credential Vault / Linux Secret Service)或应用层对称加密
- **Status**: open
- **Owner**: carlos
- **Related Task**: 待开 `06-14-p1-api-key-encryption`
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.4


## P2 — 中等(健壮性 + 债务) [3 items]

### RULE-A-005 — head_sha spawn 前查一次 50 轮不刷新

- **Level**: P2 (**P1 降级**)
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/chat.rs:362/528`
- **Description**: spawn 前一次性;每轮 clone 同一 system_prompt
- **Impact**: agent 在 turn 3 commit 后,turn 4 system_prompt 的 HEAD SHA 与 `git log` 不一致,LLM 认知漂移(原 P1,meta-review 降 P2)
- **Fix**: 每 N 轮或每次 tool 执行后刷新
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.1


### RULE-A-009 — 死代码抑制噪音

- **Level**: P2
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/chat.rs:432/512` + `types.rs:357`
- **Description**: `let _ = &base_prompt;` / `let _ = turn_send_at;` 警告抑制死代码;`ChatEvent::ToolResult` 变体从不构造
- **Fix**: 删除未用变量和构造路径
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.1


### RULE-B-003 — sqlite_glob_match 的 ? 分支 dead code

- **Level**: P2
- **Subsystem**: Permission
- **File**: `app/src-tauri/src/agent/permissions/mod.rs:766-783`
- **Description**: 内层 `if tbytes[ti] == b'/'` 永远 true(外层已判),`return false` 必达
- **Fix**: 删除冗余分支
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.2


### RULE-A-017 — max_turns 终端合成 Done 丢 last_usage

- **Level**: P2
- **Subsystem**: Agent Loop
- **File**: `app/src-tauri/src/agent/chat_loop.rs:1797-1804`(原 `usage: None` 硬编码)
- **Description**: `max_turns` 终端合成 `Done` 硬编码 `usage: None` 而不转发 `last_usage`,导致 `subagent_runs.token_usage_json` 在 max_turns 终止场景下全 0。同 model_id 的 researcher 正常完成 run 有 170879 token 证明 provider 解析正常,排除 (b)/(c) 嫌疑;research 锁定根因为 (a) max_turns 终端路径。
- **Impact**: max_turns 终止的 worker 子代理成本不可见;200+ 轮重型实施子代理场景下用户看不到已烧的 token
- **Fix**: `chat_loop.rs` 加 `last_usage_terminal` mirror + max_turns 终端 `Done` 转发 `last_usage_terminal`;`subagent.rs` sink `Done` arm 加 stop_reason guard(`max_turns`/`cancelled` 不 push `per_turn_usage`)防双累
- **Status**: closed
- **Owner**: carlos
- **Related Task**: `.trellis/tasks/06-21-subagent-max-turns-200-worker-token-incomplete`
- **Discovered In**: `research/r3-token-usage-root-cause.md` §Verdict (this task, 2026-06-21)
- **Closed At**: `fd7dc79fa50514d0be6c65a11636524d4d37fc4e`
- **Related PR**: null


## P3 — 轻微(文档/一致性) [5 items]

### RULE-B-006 — AuditKind docstring "10"→"11"

- **Level**: P3
- **Subsystem**: Permission
- **File**: `app/src-tauri/src/agent/permissions/mod.rs:140` vs `:152-179`
- **Description**: docstring 写"10 variants"实际 11(`ToolExecuted` C4 新增未更新 doc)
- **Fix**: 更新 docstring
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.2


### RULE-FrontProj-001 — 关闭项目后无法重新打开(addProject 撞 SQLite UNIQUE) ✅ closed

- **Level**: P2 → P3(实际 UX 而非数据丢失)
- **Subsystem**: Frontend Project Store
- **File**: `app/src/stores/projects.ts:147-190`(`addProject`) + `app/src/components/chat/EmptyProjectState.vue`(`currentProjectId===null` 才挂) + `app/src/components/layout/AppHeader.vue`(无主 UI 入口)
- **Description**: 关闭项目(`hide_project` 设 `hidden=1`)后,UI 唯一可见的「重新打开」入口是 `EmptyProjectState`,但**只在 `currentProjectId === null` 时挂载**;多项目用户隐藏单个后,主 UI(ChatPanel)完全无入口。被隐藏的项目数据完整保留在 DB,用户无法在 UI 上恢复。唯一兜底路径是 `+ 添加项目` → 选同路径 → 但 `addProject()` 只查 `projects.value`(visible),**漏检 `hiddenProjects.value`** → 后端 `create_project` 撞 SQLite `projects.path` UNIQUE 约束 → 误报 `a project with path '...' already exists`。用户体感:"无法重新打开"。
- **Impact**: UX 阻断——关闭项目后用户失去恢复路径,误以为是数据丢失/系统 bug,信任成本高。
- **Fix**(2026-06-23 实施,见 commit `1b5e34e`):三处修补(`addProject` 命中 hidden 路径 → 调既有 `unhideProject(id)` + toast「已重新打开」,不再撞 UNIQUE;`unhideProject` 签名改 `Promise<boolean>` 失败时不冒假成功 toast;`EmptyProjectState.vue` `onMounted` 自动 `loadHiddenProjects` 首屏直显列表,无需点「查看最近隐藏的项目」兜底按钮;`AppHeader.vue` 引入 `HiddenProjectsMenu`(reka-ui `DropdownMenu`,**`:deep()` 解决 `<style scoped>` + portal 坑**——见 `.trellis/spec/frontend/reka-ui-usage.md` §"Gotcha: <style scoped> does NOT apply to portal children"),多项目场景主 UI 直接显 archive 图标 + count badge + popover 列表「重新打开」按钮,**事件只绑按钮不绑整行**(用户反馈)。后端零改动(`unhide_project` IPC 已存在且 `lib.rs:127` 注册)。`projects.test.ts` 新增 8 case 全 pass(vitest 475/0 含 4 pre-existing RULE-FrontTest-001 baseline,vue-tsc 0 error,cargo test --lib 813/0/0,cargo check 0 warning)。
- **Status**: closed(2026-06-23)
- **Owner**: carlos
- **Discovered In**: 用户反馈(2026-06-23 报错 `添加项目失败:create_project failed: ... already exists`)
- **Closed In**: `.trellis/tasks/06-23-create-project-already-exists`
- **Closed At**: `1b5e34e`
- **Related PR**: null(个人项目)


### RULE-B-007 — Background Mode 仍空壳

- **Level**: P3
- **Subsystem**: Permission
- **File**: `app/src-tauri/src/agent/permissions/types.rs:193` + `mod.rs:1214`
- **Description**: `#[allow(dead_code)]`,`mode_system_prefix` 占位字符串
- **Impact**: UI 已移除,enum 保留预留
- **Fix**: 路线图评估移除 or 保留
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.2


### RULE-C-008 — grill Q4 "AGENTS.md 物理顺序前置"未严格执行

- **Level**: P3
- **Subsystem**: Memory
- **File**: `app/src-tauri/src/memory/loader.rs:321`
- **Description**: 仍按 CLAUDE→AGENTS 顺序;优先级仅靠 `<primary>`/`<reference>` wrapper 标签
- **Impact**: 软提示 vs 硬提示,标签可能已足够
- **Fix**: 决定硬前置 or 维持当前
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-b5-memory-grill-2026-06-10 + REVIEW-agent-loop-full-audit-2026-06-14 §2.3


### RULE-D-007 — OpenAI 多 tool_call index 缺失默认 0

- **Level**: P3
- **Subsystem**: Provider
- **File**: `app/src-tauri/src/llm/provider/openai.rs:593-597`
- **Description**: `unwrap_or(0)`,两个无 index tool_call 都映射 index 0,后者覆盖前者
- **Impact**: 官方 API 总带 index,第三方兼容层风险
- **Fix**: index 缺失报错而非默认
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.4


### RULE-D-008 — parse_anthropic_usage 全零判 None 假设

- **Level**: P3
- **Subsystem**: Provider
- **File**: `app/src-tauri/src/llm/provider/anthropic.rs:617-627`
- **Description**: 全零判 None
- **Impact**: 极低,真实响应 input 永远 >0
- **Fix**: 防御性编程,改 None if not_present
- **Status**: open
- **Owner**: carlos
- **Discovered In**: REVIEW-agent-loop-full-audit-2026-06-14 §2.4

---

### RULE-FrontSubagent-001 — `.tool-card` / `.drawer-tool-card` header CSS 重复

- **Level**: P3
- **Subsystem**: Frontend Subagent
- **File**: `app/src/components/chat/DrawerToolCallCard.vue:196-320` + `app/src/components/chat/DrawerPermissionAskCard.vue:75-138` + `app/src/components/chat/ToolCallCard.vue:641-756`
- **Description**: PR5 redesign 后 SubagentDrawer 不再内联 `.tool-card` CSS(改 5 段分组布局);但 PR4 `DrawerToolCallCard` + PR6 `DrawerPermissionAskCard` 各自重声明了 header CSS(1:1 镜像 ToolCallCard,各 ~50-60 行)。项目纯 CSS 无法 @import 共享。组件头注释把重复作为 PR4「主路径 0 改动」约束下的显式取舍。
- **Impact**: 当前无功能影响;`.tool-card` header 视觉约定扩展时需同步改 3 处。
- **Fix**: 抽 `ToolCallHeader.vue` 共享组件(需同时改 ToolCallCard 本体——PR4 时违反主路径 0 改动约束故推迟,redesign 收尾后可做);或抽 `app/src/style.css` 全局工具类。
- **Status**: open (updated 2026-06-21 PR4/PR5/PR6:位置从 SubagentDrawer 迁移到 DrawerToolCallCard/DrawerPermissionAskCard)
- **Owner**: carlos
- **Discovered In**: B6 PR3 check phase → updated redesign PR4/PR5/PR6
- **Related Task**: `.trellis/tasks/06-21-refactor-redesign-sub-agent-drawer-grouped-view-markdown-modal`

---

### RULE-FrontSubagent-002 — `pairTranscript` / `pairSections` third-param 隐式状态

- **Level**: P3
- **Subsystem**: Frontend Subagent
- **File**: `app/src/utils/transcriptPairing.ts:128-225` (`pairTranscript`) + `pairSections` (PR5 新增,同模式)
- **Description**: `pairTranscript(entries, now, pendingFirstSeenAt)` / `pairSections(sections, now, pendingFirstSeenAt)` 第三个参数既是输入(共享状态)又是输出(被 `.set`/`.delete`)。功能正确但签名隐式。PR5 新增 `pairSections` 复用同模式,债范围扩大。
- **Impact**: 调用方必须保持同一 Map 引用跨调用才能让 30s timeout timer 推进;新调用方易踩坑(每次新建 Map → 永远 pending)。
- **Fix**: 抽 `useTranscriptPairing()` composable 返回 `{ pair, pendingMap }`,或 Map 移 module-level 单例。
- **Status**: open (updated 2026-06-21 PR5:范围扩到 pairSections)
- **Owner**: carlos
- **Discovered In**: B6 PR3 check phase → updated redesign PR5
- **Related Task**: `.trellis/tasks/06-21-refactor-redesign-sub-agent-drawer-grouped-view-markdown-modal`

---

### RULE-FrontSubagent-003 — worker permission_ask 无法 interactive(R24 降级)✅ closed

- **Level**: P2
- **Subsystem**: Frontend Subagent (cross Backend Permission)
- **File**: `app/src-tauri/src/agent/permissions/mod.rs` + `app/src-tauri/src/agent/subagent.rs` + `app/src-tauri/src/agent/chat_loop.rs` + `app/src/stores/permissions.ts` + `app/src/components/chat/DrawerPermissionAskCard.vue` + `app/src/components/chat/WorkerAskBanner.vue`
- **Description**: SubagentDrawer 的 permission_ask 卡片只能 historical 只读,无法 Allow/Deny 交互。根因:worker 的 `PermissionContext.is_worker=true` 让 Tier 4 `ask_path`/`ask_shell` 直接 collapse `Decision::Deny`,从不 emit `permission:ask` IPC;transcript 里的 historical permission_ask 用 synthetic rid(`Uuid::new_v4()`),不在 `permission_asks` oneshot map 中,`permission:response` IPC 无法路由;worker 复用 `parent_session_id` 无独立 permission session。
- **Impact**: worker 跑到需权限的 tool 时,用户无法在 drawer 内批准/拒绝(只能看到 historical「自动拒绝」)。
- **Fix**(2026-06-22 实施,见 commit `89e5ba1`):worker Tier 4 `ask_path` 改完整 `register_ask + tokio::select!{cancel, timeout, oneshot}` round-trip;`SubagentBufferSink::emit_permission_ask` 双发(`permission:ask` live 通道 + `subagent:event` transcript 通道);worker payload `session_id` = parent(banner 分组),内部 `register_ask`/`resolve_ask` key = composite `worker:{runId}`(oneshot 隔离,RULE-A-014 不回归);前端 `permissions.ts` 加 `pendingWorkerByRunId` 独立 map(避免覆盖 parent 主 chat 槽)+ drawer 按 rid 去重(interactive vs historical)+ `WorkerAskBanner` 顶部唤起。worker 卡隐藏「始终允许」(后端 AllowAlways 当 AllowOnce)。完整契约见 [permission-layer.md §5b](../spec/backend/permission-layer.md)。
- **Status**: closed(2026-06-22)
- **Owner**: carlos
- **Discovered In**: redesign PR6 (2026-06-21, Explore 后端调研)
- **Closed In**: `.trellis/tasks/06-22-06-22-worker-tool-approval-interactive`
- **Related Task**: `.trellis/tasks/06-21-refactor-redesign-sub-agent-drawer-grouped-view-markdown-modal` (PR6 R24 降级)

---

### RULE-WorkerAsk-001 — worker permission_ask historical 卡片不显示 resolve outcome

- **Level**: P3
- **Subsystem**: Frontend Subagent (cross Backend DB)
- **File**: `app/src-tauri/src/agent/subagent.rs` (`SubagentBufferSink::emit_permission_ask` transcript entry) + `app/src/components/chat/PermissionAskBody.vue` (historical 分支)
- **Description**: RULE-FrontSubagent-003 修复后,worker ask 的 resolve(allow/deny/timeout/cancel)只通过 live `permission:ask` 通道的 rid 消失来体现(drawer 卡片从 interactive 翻 historical)。但 transcript 里只存了 ask 本身(`TranscriptKind::PermissionAsk` payload_json),**没存 resolve outcome**。所以 worker run 结束后重开 drawer,历史 ask 卡片只能显示中性的「worker asked for X」(N2 文案),无法回溯「已允许 / 已拒绝 / 已超时 / 已取消」。
- **Impact**: UX 轻微——历史回放丢失审批结果维度。live 期间不影响(交互卡正常)。
- **Fix**: PR1 新增 `TranscriptKind::PermissionAskResolved` 第 5 变体(snake_case wire `"permission_ask_resolved"`,现有 `#[serde(rename_all)]` 自动处理) + `SubagentBufferSink::emit_permission_ask_resolved(&self, rid, outcome)` 方法(`ChatEventSink` trait 默认 no-op 仅 sink override,零 `Arc<dyn>` downcast 跨模块耦合,`AppHandleSink` + 测试 sink 零改动继承默认) + `ask_path` worker 分支 `tokio::select!{cancel, timeout, oneshot}` 返回后四态接线(oneshot `AllowOnce`/`AllowAlways` → `"allow"`;`Deny` → `"deny"`;timeout 臂 → `"timeout"`;cancel 臂/`OneshotDropped` → `"cancel"`)。`payload_json = { rid, outcome }` 只进 transcript,不双发 IPC(live 卡消失由 permissions store rid removal 驱动,Session 62 行为不变)。前端 `pairSections` 按 `rid` 预扫 resolved → `PermissionAskSection.outcome` 透传 → `<PermissionAskBody>` historical 分支显 ✓已允许/✗已拒绝/⏱已超时/⊘已取消 badge(色 token 复用 `--color-tool-write`/`--color-tool-error`/`--color-text-muted`)。pre-fix 老 transcript(无 `PermissionAskResolved`)降级中性,不 crash。
- **Status**: closed(2026-06-23)
- **Owner**: carlos
- **Discovered In**: 06-22-worker-tool-approval-interactive trellis-check N4(2026-06-22)
- **Closed In**: `.trellis/tasks/06-22-subagent-drawer-historical-ask-outcome-and-cancelled-turn-count`
- **Closed At**: `928e131`
- **Related PR**: null(个人项目)

---

### RULE-FrontSubagent-004 — cancelled 终态无 turn 数据(R23 降级)

- **Level**: P3
- **Subsystem**: Frontend Subagent (cross Backend DB)
- **File**: `app/src-tauri/src/db/migrations.rs:515-528` (subagent_runs schema) + `app/src/components/chat/SubagentDrawer.vue` (cancelled chip)
- **Description**: PRD R23 字面要求 cancelled 终态 Reply 段显示「⊘ Cancelled · at turn N」,但 `subagent_runs` 表无 turn 列(只有 started_at/finished_at + PR1 的 task/final_text),`SUBAGENT_MAX_TURNS=20` 是常量不持久化实际 turn。PR6 用 wall-clock `terminalDurMs` 降级显示「at X.Xs」。
- **Impact**: UX 轻微——用户看到耗时而非 turn 进度,语义略弱但不影响功能。
- **Fix**: PR2 `subagent_runs` 加 `turn_count INTEGER` 列(幂等 `add_subagent_runs_column_if_missing`,nullable 无 DEFAULT 向后兼容) + `SubagentBufferSink::turns_completed() -> u64`(`fetch_add(1)` 仅在 `stop_reason != Some("cancelled") && != Some("max_turns")` 守卫下,合成 terminal 不 increment,保证 cancelled 显真实执行 turn) + `run_subagent` 终态 `update_run_finished(..., Some(turns as i64))` 写入(8th 参数) + `<SubagentDrawer>` `statusDisplay` cancelled + incomplete 分支读 `run.value.turnCount !== null` 优先显 "at turn N",NULL 降级 wall-clock `at X.Xs`(`terminalDurMs`)。pre-PR2 老行(`turn_count` NULL)显 wall-clock,向后兼容。`turn_count` 与 `token_usage_json` 走同一 `stop_reason` 守卫,1:1 lockstep 锁定(regression test `subagent_runs_update_finished_round_trips_turn_count`)。
- **Status**: closed(2026-06-23)
- **Owner**: carlos
- **Discovered In**: redesign PR6 (2026-06-21, Explore 调研)
- **Closed In**: `.trellis/tasks/06-22-subagent-drawer-historical-ask-outcome-and-cancelled-turn-count`
- **Closed At**: `928e131`
- **Related PR**: null(个人项目)


### RULE-FrontSubagent-005 — frontend `SubagentStatus` type 缺 'incomplete',drawer 误显「运行中」

- **Level**: P3
- **Subsystem**: Frontend Subagent (cross Backend DB)
- **File**: `app/src/stores/subagentRuns.ts:65` (`SubagentStatus` type) + `app/src/components/chat/SubagentDrawer.vue` (`coerceStatus` + `STATUS_META`)
- **Description**: R2 在 backend 加了 `incomplete` status(`SubagentStatusDb::Incomplete`),但 frontend `SubagentStatus` type 仍是 4 值;`coerceStatus` 对 unknown 字符串 fallback 到 `"running"`,导致 `incomplete` run 在 drawer 里**永久**显「运行中」(`STATUS_META["running"]`)。本次 R2 显式决定不做前端视觉差异化(靠 `final_text` 文案提示),故作为 follow-up debt。
- **Impact**: 用户看到「运行中」但实际 worker 已 max_turns 终止 + DB 状态是 incomplete;UX 误报与 R2 想解决的"误报成功"对称。
- **Fix**: `subagentRuns.ts` `SubagentStatus` type 加 `"incomplete"`;`SubagentDrawer.vue` `STATUS_META` 加 `{ label: "未完成", color: "var(--color-tool-warn)" }`(对齐 `CANCELLED_MARKER` / `INCOMPLETE_MARKER` 的中文文案)。
- **Status**: closed
- **Owner**: carlos
- **Related Task**: `.trellis/tasks/06-21-subagent-debt-p3-followups-frontend-incomplete-status-rule-frontsubagent-005-add-token-usage-streaming-rule-backsubagent-002-option-i`
- **Discovered In**: trellis-check PASS-WITH-NITS nit #5 (2026-06-21)
- **Closed At**: `2eedfe2`
- **Related PR**: null

---

### RULE-FrontTest-001 — streamController.test.ts 4 个 pre-existing unhandled rejection

- **Level**: P3
- **Subsystem**: Frontend Test
- **File**: `app/src/stores/streamController.test.ts` (reloadAfterFinalize path) + `app/src/stores/streamController.ts:1256`
- **Description**: `reloadAfterFinalize` 内 `invoke("list_sessions")` 走真实 `window.__TAURI_INTERNALS__.invoke`,但该 test file 没在 `__TAURI_INTERNALS__` 上 mock invoke,导致 4 个 unhandled promise rejection。tests 本身全 pass(rejection 异步、测试结束才浮出),但 `Errors: 4` 给全量 vitest run 噪音。
- **Impact**: 当前不影响测试通过;未来 Vitest 升级可能把 unhandled rejection 变硬 fail。
- **Fix**: 该 file `beforeEach` 加 `vi.stubGlobal("__TAURI_INTERNALS__", { invoke: vi.fn() })` 或补 mock。
- **Status**: open
- **Owner**: carlos
- **Discovered In**: redesign PR4-6 check phase (2026-06-21, 多次 vitest run 出现,与 subagent-drawer 无关)
- **Related Task**: null (独立测试债)

---

### RULE-BackSubagent-001 — worker error 时 parent LLM 拿不到 partial transcript context ✅ closed

- **Level**: P2
- **Subsystem**: Backend Subagent
- **File**: `app/src-tauri/src/agent/subagent.rs` (`format_dispatch_result` + `summarize_worker_tool_actions`) + `app/src-tauri/src/agent/chat_loop.rs:2539` (接线)
- **Description**: B6 review `docs/review/b6-subagent-assessment.md` §4 defect B——worker LLM stream error 后，parent LLM 只看到 `[status: error]\n<error text>`，不知道 worker 已经执行了哪些 tool_call（可能部分文件已落地）。Parent 无法做补偿性修复。
- **Impact**: Worker 半成功半失败的场景下，parent agent 盲目重做或放弃，无法基于 worker 已落地的 edits 继续。
- **Fix**(2026-06-22 实施,见 commit `89d3ffd`):PR1 新增 `summarize_worker_tool_actions(transcript)` 纯函数(tool_call/tool_result 按 `tool_use_id` 配对 ok/failed/? + per-tool key_param 提取代表参数 + 2 KiB head+tail cap + `(N actions omitted)` 计数);PR2 `format_dispatch_result` 加 `partial_actions: Option<&str>` 参数,非 completed 三态(Error/Cancelled/Incomplete)append `Worker partial actions:` 段(`chat_loop.rs:2539` 接线,Completed/空摘要传 None)。摘要只进 tool_result wire,不进 `final_text`(drawer 已有 Tools 段,`format_final_text` 不变)。DEBT 原字面只点 Error,扩到三态(Incomplete 引入于本 finding 之后,重型 worker max_turns 场景需求最强)。完整契约见 [tool-contract.md §dispatch_subagent](../spec/backend/tool-contract.md)。
- **Status**: closed(2026-06-22)
- **Owner**: carlos
- **Discovered In**: B6 review (2026-06-20), defect B
- **Closed In**: `.trellis/tasks/06-22-backsubagent-001-worker-error-partial-transcript`
- **Closed At**: `89d3ffd`
- **Related PR**: null(个人项目)


### RULE-BackSubagent-002 — `add_token_usage_streaming` 文档撒谎（无 production callsite）

- **Level**: P3
- **Subsystem**: Backend Subagent
- **File**: `app/src-tauri/src/agent/subagent.rs:567-569,838-843` + `app/src-tauri/src/db/subagent_runs.rs:18,554`
- **Description**: 注释/docstring 描述 worker per-turn usage 通过 `add_token_usage_streaming` streaming fold 进 `sessions.input_tokens_total` 列,但该函数**没有 production callsite**（只在 `db/tests.rs` 测试里被调）。research 阶段查证发现。R3 max_turns 修复后 per-run JSON 已正确,但 parent session live counter 仍有几秒延迟（因为 `db::add_token_usage` 走 `chat_loop.rs:1004` 非 streaming fold）。
- **Impact**: 低——parent UI counter 比 worker `subagent_runs.token_usage_json` 慢几秒,功能不影响。
- **Fix**: 二选一——(i) 删 `subagent.rs:567-569,838-843` + `db/subagent_runs.rs:18` 撒谎注释,接受 live counter 不 streaming;或 (ii) 在 `chat_loop.rs:1004` per-turn `Done` handler 调 `add_token_usage_streaming` 真接上去。
- **Status**: closed
- **Owner**: carlos
- **Related Task**: `.trellis/tasks/06-21-subagent-debt-p3-followups-frontend-incomplete-status-rule-frontsubagent-005-add-token-usage-streaming-rule-backsubagent-002-option-i`
- **Discovered In**: `research/r3-token-usage-root-cause.md` §3 (2026-06-21)
- **Closed At**: `2eedfe2`
- **Related PR**: null

---

## 优先级分布

| Level | Count | 说明 |
|---|---|---|
| P0 | 0 | 全部 closed(详见 §Re-evaluation Log + git log) |
| P1 | 1 | 正确性 + 资源,影响功能或可靠性 |
| P2 | 3 | 健壮性 + 债务,中长期清理(2026-06-22: -RULE-BackSubagent-001 via 89d3ffd) |
| P3 | 5 | 文档 + 一致性,可延后(2026-06-23: -RULE-WorkerAsk-001 / -RULE-FrontSubagent-004 via 928e131) |
| **Total** | **9** | 当前 open items |

---

## subagent-drawer redesign 决策索引 (2026-06-21, Session 55)

> grill-me Q1-Q10 关键决策(grill-me session 40,记录于 `journal-2.md`)已落地于 `.trellis/tasks/06-21-refactor-redesign-sub-agent-drawer-grouped-view-markdown-modal/prd.md §Decision (ADR-lite)`(Decision 1/2/3)。本段仅索引决策产生的 open 债项(降级项);已实施决策详见 prd.md + `.trellis/spec/frontend/chat.md`。

| Grill Q | 决策结论 | 落地状态 / 债项 |
|---|---|---|
| Q1 数据层 | subagent_runs 加 task/final_text 列 | ✅ PR1 实施 |
| Q2 渲染 | 共享视觉原语,不整体复用 MessageItem | ✅ PR4/PR5 实施(见 chat.md Design Decision) |
| Q3 chat_event | 前端 accumulator 聚合(后端零改动) | ✅ PR2 实施 |
| Q4 Modal | 通用 MarkdownDetailModal | ✅ PR3 实施 |
| Q5 5 段布局 | 分组折叠,默认展开 Tools+Reply | ✅ PR5 实施 |
| Q6 live | 段级 live 指示器 | ✅ PR5 实施 |
| Q7 transcriptJson | 存原始 event,前端聚合 | ✅ PR2 实施 |
| Q8 accumulator 性能 | 累加式 + markRaw + live 不全量 | ✅ PR2 实施(20k events 13.4ms 实测 <500ms AC) |
| Q9 permission_ask | drawer 交互 | ⚠️ **降级** → RULE-FrontSubagent-003(worker is_worker 架构限制) |
| Q10 out of scope | 后端聚合 / 多 drawer / 折叠持久化等 | ✅ 明确不做 |
| R23 cancelled | (边界态)turn N | ⚠️ **降级** → RULE-FrontSubagent-004(无 turn 列) |

---

## Feature Follow-ups (FT) — 已全部 closed

> 全部 6 项 FT-F-001 / FT-F-002 / FT-F-003 / FT-F-004 / FT-F-005 已 closed (2026-06-20 ~ 2026-06-21)。详见 git log 与最近 commit(`9b685c8` / `6bb5060` / `272fbe9` / `3bf2b99` / `9e41594` / `586d4a5`)。

---

## Re-evaluation Log

> 记录后续 audit 对优先级的重新评估(每次必须填日期 + 理由 + 引用)

| Date | RULE ID | Old Level | New Level | 理由 | 引用 |
|---|---|---|---|---|---|
| 2026-06-14 | RULE-A-005 | P1 | P2 | head_sha 漂移是 UX 而非数据丢失,降级 | meta-review §2.2 |
| 2026-06-14 | RULE-A-006 | P2 | P1(**前移**) | P0 修复无回归保护等于盲修 | meta-review §2.4 |
| 2026-06-14 | RULE-A-011 | P2 | P3(wontfix) | 8MB 不是性能问题,Arc<Vec> 损害借用边界 | meta-review §2.1 |
| 2026-06-14 | RULE-C-001 | P1 | P1(确认) | PRD 承诺"立即生效"是产品诚信问题 | meta-review §2.5 |
| 2026-06-14 | RULE-C-003 | P1 | P3 | 过度压缩 ≠ 数据丢失 | meta-review §2.2 |
| 2026-06-14 | RULE-E-006 | P3 | P1 | `/tmp` fallback 重启清空 = 工作数据丢失 | meta-review §2.3 |
| 2026-06-15 | RULE-E-006 | open | **closed** | worktree data_dir 从 env/home/tmp 改 Tauri app_data_dir,对齐 DB,消除 /tmp 数据丢失;`git::data_dir()` 函数 + re-export + 模块 docstring 全部删除,Grill decision #2 不变式保留(`catalog` 紧跟 `db`,`app_data_dir` 落在 data-plane group 内) | .trellis/tasks/06-15-p1-worktree-data-dir-tauri |
| 2026-06-15 | RULE-A-006 | partial | **closed** | production `chat.rs` → `run_chat_loop` 迁移完成,副本消除,9 个 agent_loop_* 测试现覆盖 production 真实路径 | `.trellis/tasks/06-15-unify-chat-loop-dispatch` |
| 2026-06-15 | RULE-A-003 | open | **closed** | 5 处 persist 失败分类处理:正常路径 3 处 emit `Error{Server}`+return(对齐 RULE-A-002 StillOver),cancel 路径 2 处 log-only(避免与 cancelled `Done` 双终止事件);前端不基于 category 分支故复用 `Server`,零前端改动。`emit_persist_failure` helper 集中文案;`agent_loop_persist_failure_emits_error` 测试(trigger 拦截 INSERT) | `.trellis/tasks/06-15-p1-persist-emit-error-and-audit-cancel-order` |
| 2026-06-15 | RULE-A-004 | open | **closed** | `record_tool_executed_audit` 块从 cancel 检查前移到后(`else if` 串联),cancelled 的 tool 不落 audit;两检查背靠背无 await,token 状态一致。`agent_loop_cancel_skips_audit_for_cancelled_tool` 测试 | 同上 task |
| 2026-06-15 | RULE-E-005 | open | **closed** | `cancel_inflight_for_session` 加 `inflight_exits` 参数返回 `oneshot::Receiver`(单消费者),新增 `await_inflight_exit`(10s timeout backstop);`delete_worktree`/`detach_worktree`/`delete_session` 三处 await;chat.rs spawn 闭包 `run_chat_loop` 后 `done_tx.send` + 清 entry。独立 map + oneshot(不动 cancellations 值类型,规避 Tauri JoinHandle 存储语义不确定)。4 cancel 单测(3 改造 + 1 新增)。spec Ordering invariant 改写。487 tests pass | `.trellis/tasks/06-15-worktree-destroy-await-cancel-rule-e-005` |
| 2026-06-15 | RULE-C-001 | open | **closed** | 砍 notify watcher 改 read-through mtime fence;brainstorm 核实发现 watcher 疑似完全失效(返回值丢弃→handle drop→确定性读旧,严重性>>原"概率性 race")。W 方案:slot 加 `CachedLayer{layer,mtime}`,read 每次 stat 比较,read 路径成 freshness 权威。watcher.rs/invalidate_*/notify 依赖/前端 dead listener 全清,4 fence 测试,489 pass | `.trellis/tasks/06-15-p1-memory-watcher-appstate` |
| 2026-06-15 | RULE-C-002 | open | **closed** | 自动满足:watcher 删后新 project 首 `load_for_session` 即 stat,无需 watch/add_watch | 同上 task |
| 2026-06-15 | RULE-C-004 | open | **closed** | 自动满足:watcher 删除,无 handle 可丢弃;D2"AppState 加字段"方案被 D3 砍 watcher 推翻 | 同上 task |
| 2026-06-16 | RULE-D-002 | open | **closed** | is_o1_family 前缀分支(o1/o3/o4),o1 family 用 max_completion_tokens;+3 单测。与 E-004 合并 task(两项均小修 active bug) | `.trellis/tasks/06-16-p1-openai-o1-glob-spawn-blocking` |
| 2026-06-16 | RULE-E-004 | open | **closed** | glob walk+match+collect 包 spawn_blocking(GlobMatcher Send 验证);与 D-002 合并 task | 同上 task |
| 2026-06-16 | RULE-C-007 | open | **wontfix** | 引用的 watcher.rs 已随 RULE-C-001 删除(mtime fence 取代),fallback 逻辑不存在 | §收尾路径建议 |
| 2026-06-16 | RULE-C-009 | open | **wontfix** | watcher 删除后 freshness 走 mtime stat,不依赖 inotify,WSL 可靠性问题消解 | §收尾路径建议 |
| 2026-06-16 | RULE-B-004 | open | **closed** | DENY_PATTERNS 全加 (?i) + 新增 find -delete/-exec 硬墙;不动 shell_trust 分级(双层架构);长选项/子shell/env 留 Tier4 兜底;+3 测试,498 pass | §收尾路径建议 |
| 2026-06-16 | RULE-E-009 | open | **closed** | 4 处字节切片改 floor/ceil_char_boundary(对齐 diff.rs);+2 多字节测试,498 pass | §收尾路径建议 |
| 2026-06-16 | RULE-D-003 | open | **closed** | MAX_DATA_BYTES=1MiB cap(超限 drop 余下 data)+ strip_prefix(data:) 去空格容忍无空格版;+4 测试,502 pass | §收尾路径建议 |
| 2026-06-17 | RULE-A-010 | open | **closed** | D3 PR3 实施时收口,方式 = spec 偏离声明(非实现二次取消语义):`docs/ARCHITECTURE.md §2.5.1` 加 "已知偏离" 注释 + `docs/IMPLEMENTATION.md §4` 2026-06-17 D3 ADR 完整说明偏离理由 + 未来实现路径(tool 取消分支 + cancel check 之间加 N 状态机)。原 "实现二次取消语义" 选项不在本批次做(MVP 简化决策:tool 取消窗口短 + 二次取消 UX friction + 单用户误点概率低) | `.trellis/tasks/06-17-d3-message-edit-resend` |
| 2026-06-17 | RULE-A-007 | open | **closed** | error arm 对称 cancel 路径 persist partial turn。设计决策 A/B/C:**A** = `ERROR_MARKER`(`"[生成出错中断]"`)text 追加,对称 `CANCELLED_MARKER` 既有模式(否决 metadata 双表达);**B** = persist 失败 log-only(对称 cancel tool_result persist 失败,否决 `emit_persist_failure`——error 路径已 pre-emit Error,再 emit 第二个 terminal Error 冲突);**C** = persist 成功后 emit TurnComplete(seq + latency 指向 partial turn),Error + TurnComplete 并存不冲突。新增 `ERROR_MARKER` const 在 `agent/helpers.rs`;改 `chat_loop.rs`(RULE-A-006 闭环后单一权威,不动 chat.rs)。5 新测试覆盖 partial text/empty/thinking+tool_use/persist-fail-log-only/TurnComplete;567 tests 全 pass,0 warning。spec 同步:`agent-loop-architecture.md` 加 "Turn-boundary persist symmetry" pattern + 测试表 5 行;`error-handling.md` 加 "Agent Loop Error Paths" 段 + persist 失败处理矩阵 | `.trellis/tasks/06-17-a-007-error-arm-partial-text` |
| 2026-06-18 | RULE-D-005 | open | **closed** | openai_caps() 从 config.reasoning_effort 派生 caps(替代硬编码 true);gpt-4o 无 thinking_effort → caps.supports_reasoning_effort=false → strip 丢弃历史 Reasoning 块。未直接调 from_model_row(send 签名不带 model_row,config.reasoning_effort 等价);+2 测试 | `.trellis/tasks/06-18-p2-reasoning-caps-estimator-dedup` |
| 2026-06-18 | RULE-D-004 | open | **closed** | 删 WireRequest.reasoning_effort 死字段(OpenAI-specific 不属 wire 层;真参数走 config)+ docstring + 初始化 + 9 处测试构造 | 同上 task |
| 2026-06-18 | RULE-A-008 | open | **closed** | 抽 push_message_tokens helper,estimate_messages_tokens 与 _iter 共用;case_1~7 回归通过 | 同上 task |

| 2026-06-19 | RULE-A-012 | open | **closed** | 双根因合并 single RULE。**A** provider reqwest `.timeout(60s)`(总 deadline,reqwest 文档明示不适合 SSE)改 `.read_timeout(60s)`(per-chunk,resets per SSE event),`anthropic.rs:209-211` + `openai.rs:424-426` 同步改,保留 `.connect_timeout(10s)`;**D** `chat_loop.rs:657` `Err(err)` 静默包装补 `tracing::warn!(request_id, turn, category=?err.category(), error=%err, "chat: LLM stream errored")`(`LlmErrorCategory` 只有 Debug 没有 Display,故 `?` 走 Debug,产出五类 variant name 同 Display 行为)。incident `mz8s3hqwx6rmqjswgte` / `messages.seq=37`(seq=36→37 间隔 60.403s 实锤);fix commit `05037ac`,cargo check + 6 个 agent_loop_error_* 集成测试全 pass(622 总数,0 warning)。Out of scope:抬总超时到 600s(LiteLLM 风格)——否决,`read_timeout=60s` 已 cover 慢代理,真 60s 无 chunk 是代理死了该报错;per-provider timeout 列(`providers` / `models` 表加列)——否决本次做,DB schema 改动有迁移成本,等真有多 provider 用户被掐再上。spec 沉淀:`.trellis/spec/backend/error-handling.md` §RULE-A-012 + `docs/IMPLEMENTATION.md §4` 2026-06-19 ADR | `.trellis/tasks/2026-06/06-19-fix-llm-streaming-timeout-and-tracing` |

| 2026-06-19 | RULE-E-013 | open | **closed** | system prompt 工具清单:删除硬编码枚举改通用表述(比原"动态生成"更治本,PRD D2);`build_system_prompt_no_hardcoded_tool_list` 回归保护;随 behavior_prompt 同 task 落地 | `.trellis/tasks/06-19-system-prompt` |

| 2026-06-20 | RULE-A-014 | open | **closed** | B6 PR2b 收口。`is_worker: Option<bool>` 加为 `run_chat_loop` 第 21 参;worker 嵌套调用 `chat_loop.rs:2155` 传 `Some(true)`,run_chat_loop 内部构造 `PermissionContext` 读 `is_worker.unwrap_or(false)`;PR1b 的 dead-code `_worker_permission_ctx` 块删除;production `chat.rs:249` + 33 个 `agent_loop_*` 集成测试调用点更新 `Some(false)`。端到端测试 `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`(`/tmp/everlasting_worker_escape.txt` 路径 + Edit mode + general-purpose + write_file,`tokio::time::timeout(15s)` 包裹)验证:worker Tier 4 ask_path 收到 `is_worker=true` → 立即 `Decision::Deny` 无 oneshot 等待无挂起,tool_result `is_error=true` + deny 原因,1 行 `tool_denied` audit 落地,0 行 `tool_permission_ask`(ask 路径 collapse 验证)。PR2a 修 RULE-A-015 拆出 2 处 `skip_persist` gate,精确 gate 数 = 16(原 spec 18 处,Phase 3 spec commit 同步 `agent-loop-architecture.md` + `tool-contract.md`)。cargo test --lib 726 pass(PR2a 725 + PR2b 1),0 新 warning(对比 PR2a 4 pre-existing background_shell + 1 pre-existing `permission_ctx` unused)。P2 22→21,Total 47→46 | `.trellis/tasks/06-20-b6-pr2-subagent-persistence` |
| 2026-06-20 | RULE-A-016 | open | **closed** | B6 PR3a 顺手修。`permissions::ask_path` worker 分支(原 line 1002-1009 `record_audit(ToolDenied)`)删除 + 改 emit `PermissionAskPayload` via sink → `SubagentBufferSink::emit_permission_ask` 写 transcript `PermissionAsk` entry(PR3 drawer 可见)。`audit_not_polluted_by_worker` 测试断言不变(delta == 2,researcher silent allow 本就不写 audit);`agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied` 测试断言反转:parent `tool_denied` count 0(原 1)+ transcript `PermissionAsk` count 1(原 0)+ audit delta ≤ 2(原 ≤ 3)。cargo test --lib 732 pass(PR2b 726 + PR3a 6 = 2 新 db tests + 4 新 PR2 hotfix subagent tests;agent_loop_* tests 数量未变只更新断言)。0 新 warning(对比 PR2b 4 pre-existing)。P2 21→20,Total 46→45 | `.trellis/tasks/06-20-b6-pr3-frontend-expand` |

| 2026-06-21 | RULE-A-017 | open | **closed** | `chat_loop.rs:1797-1804` max_turns 终端合成 `Done` 改 forward `last_usage_terminal`(原硬编码 `None`);`subagent.rs:835-849` sink `Done` arm 加 stop_reason guard(`max_turns`/`cancelled` 不 push `per_turn_usage`)防双累;同时 R1 把 `SUBAGENT_MAX_TURNS` 20→200,R2 加 `SubagentStatus::Incomplete` + DB migration 5-variant status CHECK + `INCOMPLETE_MARKER` `[未完成]`。11 新测试(9 sink-level + 2 db-level),782 cargo test --lib pass(771 旧 + 11 新)。个人项目无 PR URL | `.trellis/tasks/06-21-subagent-max-turns-200-worker-token-incomplete` |

| 2026-06-22 | RULE-FrontSubagent-005 | open | **closed** | frontend `SubagentStatus` type union 4→5 加 `"incomplete"` + `coerceStatus` 显式 recognize;`SubagentDrawer.vue` `STATUS_META` 加 `incomplete: { label: "未完成", color: "var(--color-tool-shell)" }`(用现有 amber 不用 --color-tool-warn:design-tokens spec 显式禁为 one-off use 新增 --color-* token);`statusDisplay` / `bannerText` / `isEmpty` 同步加 incomplete 分支(避免 incomplete run 在空 transcript 时显「Worker is starting...」误报 / 缺 banner / 缺 terminal 状态语义)。范围扩大超出 PRD 字面 AC 但必要(对称 Session 60 R2 解决「误报成功」的初衷)。+118/-33 总计 4 文件;782 cargo / 427 vitest / 0 vue-tsc / 0 warning;4 pre-existing errors in streamController.test.ts(RULE-FrontTest-001 债,stash 验证 baseline 一致)。NIT-1 `chat_loop.rs:1019-1022` 引号内"streaming" 描述未修(非 DEBT 范围);NIT-2 chat.md spec 不覆盖 SubagentStatus / STATUS_META 状态语义(Session 60 R2 引入的 pre-existing spec drift,本次不修)。个人项目无 PR URL | `.trellis/tasks/06-21-subagent-debt-p3-followups-frontend-incomplete-status-rule-frontsubagent-005-add-token-usage-streaming-rule-backsubagent-002-option-i` |
| 2026-06-22 | RULE-BackSubagent-002 | open | **closed** | option i 路线,改写 4 处撒谎注释(DEBT 列的 3 处 + inspect 阶段 bonus 1 处):`subagent.rs:576-598` per_turn_usage 字段 docstring + `:879-894` ChatEvent::Done arm inline 注释 + `db/subagent_runs.rs:18-27` module doc(含 ⚠️ production-only path warning block,显式禁止 production code 走 add_token_usage_streaming)+ `:139-155` SubagentRunRow type doc。全部改写为指向 `chat_loop.rs:1031 db::add_token_usage` production 路径(PR2a 把 skip_persist gate 解耦后,worker 复用 parent_session_id,per-turn usage 自然 fold,见 RULE-A-015)。保留 `subagent.rs:803-813` + `db/subagent_runs.rs:554-586` 2 处已诚实注释 + `add_token_usage_streaming` 函数体仍 `pub`(PR2 API 表面保留,未来 worker↔parent session identity split 时用)。DEBT 行号 567-569/838-843 已 drift 实际在 576-586/870-876;DEBT 文件 path 漏列 `db/subagent_runs.rs:139-155` 一处(本 task 顺手修)。lying-language grep 0 match in production paths。个人项目无 PR URL | `.trellis/tasks/06-21-subagent-debt-p3-followups-frontend-incomplete-status-rule-frontsubagent-005-add-token-usage-streaming-rule-backsubagent-002-option-i` |
| 2026-06-22 | RULE-BackSubagent-001 | open | **closed** | PR1 新增 `summarize_worker_tool_actions(transcript)` 纯函数(tool_call/tool_result 按 `tool_use_id` 配对 ok/failed/? + per-tool key_param 提取代表参数 + 2 KiB head+tail cap + `(N actions omitted)` 计数;chat_event/permission_ask 跳过)。PR2 `format_dispatch_result` 加 `partial_actions: Option<&str>` 参数,非 completed 三态(Error/Cancelled/Incomplete)append `Worker partial actions:` 段;`chat_loop.rs:2539` 接线(Completed/空摘要传 None)。摘要只进 tool_result wire,不进 `final_text`(drawer 已有 Tools 段,`format_final_text` 不变)。DEBT 原字面只点 Error,扩到三态(Incomplete 引入于本 finding 之后,重型 worker max_turns 场景需求最强)。spec 同步 tool-contract.md wire 表 + Tests + agent-loop-architecture.md。+10 单测(PR1)+ 3 单测 + 1 集成(PR2);803 cargo test --lib pass,prod cargo check 0 warning。个人项目无 PR URL | `.trellis/tasks/06-22-backsubagent-001-worker-error-partial-transcript` |

---

## 子 task 编排建议

| PR 顺序 | Task 名 | 包含 RULE | 依赖 |
|---|---|---|---|
| PR1 | `06-14-p0-shell-env-clear` | RULE-E-001 | — |
| PR2 | `06-14-p0-shell-process-group` | RULE-E-002 | — |
| PR3 | `06-14-p0-web-fetch-redirect-ssrf` | RULE-E-003 | — |
| PR4 | `06-14-p0-c3-tail-pair-orphan` | RULE-A-001 + RULE-A-002 | — |
| **PR5** | **`06-14-p1-agent-loop-integration-tests`** | **RULE-A-006** | **必须在 P0 修复后立刻补,为后续 P1 提供回归保护** |
| **PR5b** | **`06-15-unify-chat-loop-dispatch`** | **RULE-A-006(闭环)** | **依赖 PR5 — production `chat.rs` → `run_chat_loop` 迁移,副本消除** |
| PR6+PR7 | `06-15-p1-persist-emit-error-and-audit-cancel-order` | RULE-A-003 + RULE-A-004(合并一个 task) | ✅ closed (2026-06-15) — PR5(RULE-A-006)解阻后合并实现 |
| PR8 | `06-16-p1-permission-asks-cleanup` | RULE-B-001 + RULE-B-002 | ✅ closed (2026-06-16) → `3b16528` — store value 加 session 绑定,delete_session 接入 |
| PR9 | `06-15-p1-memory-watcher-appstate` | RULE-C-001 + RULE-C-002 + RULE-C-004 | ✅ closed (2026-06-15) — W 方案:砍 watcher 改 mtime fence,C-002/C-004 自动满足 |
| PR10 | `06-14-p1-api-key-encryption` | RULE-D-001 | — |
| PR11+PR12 | `06-16-p1-openai-o1-glob-spawn-blocking` | RULE-D-002 + RULE-E-004(合并一个 task) | ✅ closed (2026-06-16) → `361336e` — 两项均小修 active bug,合并 PR |
| PR13 | `06-15-worktree-destroy-await-cancel-rule-e-005` | RULE-E-005 | ✅ closed (2026-06-15) — 依赖 PR5(RULE-A-006 已 closed,解阻) |
| PR14 | `06-15-p1-worktree-data-dir-tauri` | RULE-E-006 | — |
| PR-N+ | P2 各项子 task | RULE-*-P2 | — |
| PR-N+ | P3 各项子 task | RULE-*-P3 | — |

---

## 收尾路径建议(基于 ROADMAP 耦合,2026-06-16 评估)

> 维度:按"与接下来 ROADMAP 里程碑的耦合"给债务排收尾节奏。**不替代**上方"子 task 编排建议"(那是按 PR 依赖顺序),两者互补——编排建议看依赖,本段看功能契机。
>
> **现状判断**:P0 已清零(5/5 closed);P1 仅剩 `RULE-D-001`(API key 明文);**无任何债务阻塞 ROADMAP 第二档功能**。

### 三梯队

| 梯队 | RULE | 处置 |
|---|---|---|
| 🟢 可一直挂 | A-005 / A-008 / A-009 / B-003 / B-006 / C-003 / C-006 / D-004~D-008 / E-007 / E-010 / E-011 | 卫生债,不坏功能 |
| 🟡 看到顺手修 | **B-004**(`find / -delete` 漏网,P2 唯一偏安全)、**E-009**(read_file UTF-8 panic,同 repo diff.rs 已修纯不一致)、**D-003**(SSE data_buf 无上限,第三方代理可踩) | 独立便宜活,任意时点可挑 |
| 🔴 需决策 | **D-001**(API key 明文,P1) | 接受风险 vs 引入 keyring 依赖(Linux 走 Secret Service/D-Bus,WSL 体验存疑),建议先标"已知接受" |

### 按 ROADMAP 里程碑的收尾契机

| ROADMAP 里程碑 | 耦合债务 | 建议 |
|---|---|---|
| **B2 / B3**(输入/触发层;**D2 已于 2026-06-17 降档到第三档**,见 [IMPLEMENTATION §4 2026-06-17](../docs/IMPLEMENTATION.md)) | 无直接耦合 | 零负担推进,不顺手不修 |
| **D3**(消息编辑/重发) | 会重走 turn 边界 + message 持久化 → 自然碰到 **A-007**(error 路径 partial text 丢失)、**A-010**(二次取消语义) | 做 D3 时是修这俩的天然窗口 |
| **B6 Subagent**(第三档,harness 学习价值最高) | worker agent 独立 context/token 预算 → **A-008**(estimator 两版重复)、**D-004/D-005**(capabilities 派生错误会污染 subagent 上下文) | **进 B6 前先抽 A-008 helper + 修 D-004/D-005** |

### 已失效债务清理(本次评估发现)

`RULE-C-001`(2026-06-15)Resolution Notes:watcher 已**整文件删除**改 read-through mtime fence。以下 2 条 finding 引用的 `watcher.rs` 已不存在,本次标 wontfix:

- **RULE-C-007**(`watcher.rs:331-339` 路径表 fallback)→ wontfix
- **RULE-C-009**(WSL/9p inotify 可靠性)→ wontfix(无 watcher 可失效)

### 建议执行节奏

1. ✅ **B-004 + E-009 + D-003 全部完成**(2026-06-16,502 tests pass;执行节奏第 1 条三项收口)。
2. 推 B2/B3/D2(零耦合功能)。
3. 做 D3 时顺手清 A-007 + A-010。
4. 进 B6 前抽 A-008 + 修 D-004/D-005。
5. D-001 待个人威胁模型决策(暂标已知接受)。

---

## 维护说明

- **每次 audit 必须 diff 本文件** 第一步
- **每次 PR merge 必须更新 Closed At + Related PR**
- **每条 finding 闭合后状态变更不可逆**(除非重新打开)
- **子 task 创建时在本文件 Related Task 字段填 task 路径**
- **下次 audit 模板**: 第一段写 "DEBT.md diff 结果",已记录 finding 仅引用,新 finding 按模板加入

---

**最后更新**: 2026-06-21 by carlos — Session 54:**FT-F-002 closed**(`3bf2b99`)— SubagentDrawer 1.5s miss 后 inline 提示(原 toast fallback):grill 前提校准 —— retry polling(B6 PR3b)已是 race 吸收层,FT-F-003(unmount guard)不影响 miss 频率,1.5s miss=真实故障(worker 没启动/IPC 挂/ID 漂移)。收窄 drop toast/ToastService/session banner → 最小 inline(`workerMissed` 三态 default/waiting/missed + warn icon"worker 未响应,点此重试"+ `--color-tool-shell` warning tint + 复用卡片 @click 重试 + per-card)。miss 路径 `workerMissed=true` 在 FT-F-003 unmount guard 之后(不写 unmounted ref)。290 pass,vue-tsc 0 error。**同 Session 54 前序:FT-F-004 closed**(`9e41594`,UX polish bundle C1+C2+C3)。**FT-F family 全部 closed**(001/002/003/004/005)。
**最后更新**: 2026-06-21 by carlos — Session 56:**RULE-A-017 closed**(`fd7dc79`)+ **RULE-BackSubagent-002 open** + **RULE-FrontSubagent-005 open**:`subagent: MAX_TURNS 20→200 + max_turns→Incomplete + worker token 统计修复` 收尾。R1 `SUBAGENT_MAX_TURNS` 20→200 支撑重型实施子代理(trellis-implement 级 200+ 工具调用);R2 max_turns 软终止改记 `incomplete` + DB CHECK 5-variant + `INCOMPLETE_MARKER` `[未完成]`,不再误报 completed;R3 research 锁定 max_turns 终端合成 `Done` 硬编码 `usage: None` 丢 `last_usage` 是 `c27f3fd7` token=0 根因,加 `last_usage_terminal` mirror + sink stop_reason guard 防双累。11 新测试(9 sink + 2 db),782 cargo test --lib pass(771 旧 + 11 新),0 warning。DEBT 12→14(RULE-A-017 closed 不计数 + BackSubagent-002 / FrontSubagent-005 open +1 P3 each)。spec 同步 3 个 backend spec(`agent-loop-architecture.md` 3 处 `MAX_TURNS=20` → 200,`tool-contract.md` 2 处 + `format_dispatch_result` 加 Incomplete 行,`subagent-runs-schema.md` CHECK + wire shape + count 全部 5 值)。Bonus debt 两条:`add_token_usage_streaming` 文档撒谎(删注释 OR 真接上);frontend `SubagentStatus` 没渲染 incomplete 视觉(drawer 永久显「运行中」)。
**最后更新**: 2026-06-21 by carlos — Session 55(redesign 收尾):subagent-drawer 重构 PR1-6 完成。DEBT 新增 RULE-FrontSubagent-003(worker permission_ask 无法 interactive,P2)/004(cancelled 无 turn 数据,P3)/RULE-FrontTest-001(streamController 4 pre-existing unhandled rejection,P3);更新 001(CSS 重复位置迁移到 DrawerToolCallCard/DrawerPermissionAskCard)/002(范围扩 pairSections)。grill-me Q1-Q10 决策见上 §subagent-drawer redesign 决策索引。PR1-6 commits: 86a81b2/6e077b3/a39ad00/e66001e/3db2be2/d9f999f。
**最后更新**: 2026-06-22 by carlos — Session 61(Session 60 收尾 follow-up task):**RULE-FrontSubagent-005 closed**(`2eedfe2`)+ **RULE-BackSubagent-002 closed**(`2eedfe2`)。R1 frontend `SubagentStatus` type union 4→5 加 `incomplete` + `coerceStatus` 显式 recognize + `SubagentDrawer.vue` `STATUS_META` / `statusDisplay` / `bannerText` / `isEmpty` 同步加 incomplete 分支(范围超出 PRD 字面 AC 但必要,避免 incomplete run 在空 transcript 时 UX limbo)。color token drift: PRD AC 提 `--color-tool-warn` 不存在 → 改用 `--color-tool-shell`(amber #f59e0b;design-tokens spec 显式列其作 warning tint 等价物 + 禁为 one-off use 新增 --color-*)。R2 option i(删注释,4 处:DEBT 列 3 处 + inspect 阶段 bonus 1 处)改写撒谎注释指向 `chat_loop.rs:1031 db::add_token_usage` 真 production 路径(PR2a 把 skip_persist gate 解耦,worker 复用 parent_session_id,per-turn usage 自然 fold — 见 RULE-A-015);`db/subagent_runs.rs:18` module doc 加 ⚠️ production-only path warning block 防止未来 dev 误接。+118/-33 4 文件;782 cargo / 427 vitest / 0 vue-tsc / 0 warning;4 pre-existing errors in streamController.test.ts(RULE-FrontTest-001 债,baseline 一致)。DEBT P3 9→7,Total 14→12。NIT(留 follow-up): `chat_loop.rs:1019-1022` 引号内"streaming"描述(非 DEBT 范围);chat.md spec 不覆盖 SubagentStatus / STATUS_META(Session 60 R2 pre-existing spec drift)。Spec 同步:无新行为需 spec 化,纯 P3 收面。
**最后更新**: 2026-06-22 by carlos — Session 62:**RULE-BackSubagent-001 closed**(`89d3ffd`)。worker 非正常终态(Error/Cancelled/Incomplete)时 parent LLM 拿不到已执行 tool 摘要(B6 review defect B)→ PR1 `summarize_worker_tool_actions` 纯函数(tool_call/tool_result 按 `tool_use_id` 配对 ok/failed/? + per-tool key_param 提取代表参数 + 2 KiB head+tail cap + `(N actions omitted)` 计数)+ PR2 `format_dispatch_result` 加 `partial_actions: Option<&str>` 三态 append `Worker partial actions:` 段(`chat_loop.rs:2539` 接线,Completed/空摘要传 None)。摘要只进 wire 不进 `final_text`(drawer 已有 Tools 段,`format_final_text` 不变 → wire/DB 出口分家)。DEBT 原字面只点 Error,扩到三态(Incomplete 引入于 finding 之后,重型 worker max_turns 场景需求最强)。+10 单测(PR1)+ 3 单测 + 1 集成(PR2);803 cargo test --lib pass,prod cargo check 0 warning。spec 同步 tool-contract.md wire 表 + Tests + agent-loop-architecture.md 补全 4 态。DEBT P2 4→3,Total 12→11。四段式 commit: fix `89d3ffd` + docs(debt) + archive + journal。
**最后更新**: 2026-06-23 by carlos — Session 64:**RULE-WorkerAsk-001 closed** + **RULE-FrontSubagent-004 closed**(`928e131`)— subagent drawer historical 回放补两个丢失维度。PR1 worker permission_ask resolve outcome 走新 `TranscriptKind::PermissionAskResolved` 第 5 变体(snake_case wire + `ChatEventSink` trait 默认 no-op 仅 `SubagentBufferSink` override,零 `Arc<dyn>` downcast 跨模块耦合)+ `ask_path` worker 分支 `tokio::select!` 四态接线 + 前端 `pairSections` 按 rid 配对 outcome 到 `PermissionAskSection` + `PermissionAskBody` historical 分支显 ✓已允许/✗已拒绝/⏱已超时/⊘已取消 badge(色 token 复用 `--color-tool-*`)。PR2 `subagent_runs` 加 `turn_count INTEGER` 列(幂等 migration,nullable 向后兼容)+ `SubagentBufferSink::turns_completed()` 真实 per-turn `Done` 自增(synthetic cancel/max_turns terminal 不 increment,1:1 lockstep 与 `per_turn_usage`)+ `run_subagent` 终态 `update_run_finished(..., Some(turns))` 写入 + `SubagentDrawer` statusDisplay cancelled + incomplete 优先读 `turnCount` 显 "at turn N",NULL 降级 wall-clock。23 文件改动(17 code + 6 spec),+1462/-45;cargo 813/0/0(was 803),vitest 467 pass / 4 pre-existing streamController(RULE-FrontTest-001 baseline),vue-tsc 0 error,0 新 warning。trellis-check PASS verdict。spec 同步:subagent-runs-schema.md + tool-contract.md §TranscriptKind wire + §audit 决策 + agent-loop-architecture.md §"Worker ask resolve outcome + turn counting" 新 subsection + permission-layer.md §5b Audit note 扩展 outcome transcript entry + chat.md R23 RESOLVED + R24 evolved(原降级三 blocker 全部解决)+ state-management.md `TranscriptKind` 4→5 + `TranscriptSectionKind` 6→7 + `outcome?` 字段。DEBT P3 7→5,Total 11→9。NIT(refactor 机会,follow-up,非 blocker):4-state outcome union literal 在 `utils/transcriptPairing.ts:322/417/423` + `components/PermissionAskBody.vue:77/104` + `components/DrawerPermissionAskCard.vue:102` + tests 重复未导入规范 `PermissionAskOutcome` type(从 `stores/subagentRuns.ts:330` 导出),5th outcome 加时 drift 风险。四段式 commit:fix `928e131` + docs(debt) + archive + journal。
**下个 review**: REVIEW-XXX-2026-XX-XX(待定)