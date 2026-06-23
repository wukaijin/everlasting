# 同步代码地图 + 文档引用(10 个文件 split 之后的漂移修复)

## Goal

2026-06-23/24 连续完成 10 个大型文件 split(单文件 1100+ → 2500+ 行级 6 处 / 子目录 / 子组件),commit 已合 main,但代码地图(STRUCTURE / CLAUDE.md)、spec(chat.md / subagent-runs-schema.md)、决策档案(IMPLEMENTATION.md / ARCHITECTURE.md)、源码内注释 / 引用**没系统重校**。本任务把 6 个文档 + 6+ 处源码注释的漂移点全部回填到 split 后的真实文件路径 + 标注 (拆分自 X) 注释,让新人按 CLAUDE.md → STRUCTURE.md → spec 链路无歧义定位代码。**零行为变更**,纯文档 + 注释维护。

## What I already know

### 10 个 split 的最终文件树(2026-06-24 现状,grep `ls` 验证)

| # | task | 拆前 | 拆后 |
|---|---|---|---|
| 1 | `06-23-split-chat-types` | `app/src/stores/chat.ts`(整文件) | `app/src/stores/chat.ts`(runtime 部分)+ **`chat.types.ts`**(~310 行) |
| 2 | `06-23-06-23-split-subagent` | `app/src-tauri/src/agent/subagent.rs` | `app/src-tauri/src/agent/subagent/{mod,sink,transcript,truncate_summary}.rs` |
| 3 | `06-23-06-23-split-chat-loop` | `app/src-tauri/src/agent/chat_loop.rs`(2586 行,`run_subagent` 混在主循环) | `chat_loop.rs` 缩到 ~2064 行 + **`subagent/dispatch.rs`**(~520 行,含 `run_subagent` + `resolve_project_id` + `SUBAGENT_MAX_TURNS`) |
| 4 | `06-23-06-23-split-agent-tests` | `app/src-tauri/src/agent/tests.rs`(6547 行 62 测试) | **`tests_common.rs`** + **`tests_cancellation.rs`** + **`tests_envelope.rs`** + **`tests_prompts.rs`** + **`tests_agent_loop.rs`** + **`tests_subagent.rs`** |
| 5 | `06-23-06-23-split-permissions` | `app/src-tauri/src/agent/permissions/mod.rs`(2814 行) | `mod.rs`(纯 re-exports)+ **`{types,store,payload,mode,audit,check,ask}.rs`** + 6 个 `tests_*.rs` + `tests_common.rs` |
| 6 | `06-23-06-23-split-subagent-runs` | `app/src/stores/subagentRuns.ts`(1416 行) | `subagentRuns.ts`(store 部分)+ **`subagentRuns.types.ts`**(~354 行)+ **`runAccumulator.ts`**(~537 行) |
| 7 | `06-23-06-23-split-message-item` | `app/src/components/chat/MessageItem.vue`(1099 行) | `MessageItem.vue` 缩到 ~770 行 + **`MessageItemEdit.vue`**(~180 行)+ **`MessageItemFooter.vue`**(~120 行) |
| 8 | `06-23-06-23-split-subagent-drawer` | `app/src/components/chat/SubagentDrawer.vue`(1257 行) | `SubagentDrawer.vue` 缩到 ~900 行 + **`SubagentDrawerHeader.vue`**(~250 行)+ **`SubagentDrawerErrorCard.vue`**(~100 行) |
| 9 | `06-23-06-23-split-db-tests` | `app/src-tauri/src/db/tests.rs`(3242 行 95 测试) | **`{projects,sessions,providers,permissions,messages,subagent_runs}_tests.rs`**(6 个 `tests_*.rs`,无 common) |
| 10 | `06-23-06-23-split-chat-input` | `app/src/components/chat/ChatInput.vue`(1834 行) | `ChatInput.vue` 缩到 ~712 行 + **`ChatInputLatencyPopover.vue`**(~365 行)+ **`ChatInputHintRow.vue`**(~251 行)+ **`app/src/utils/chatInputCodeMirror.ts`**(~564 行 composable) |

### 漂移面 9 类盘点(已与用户 confirm 修 1-7,跳 8-9)

| # | 类别 | 严重度 | 是否修 |
|---|---|---|---|
| 1 | STRUCTURE.md 整体 §2/§3 文件树(06-10 快照,顶部校注已认滞后) | 高(新人入门第一文档) | ✅ |
| 2 | CLAUDE.md Architecture 段(6-13 快照,缺 subagent/ + tests_*.rs + chat.types.ts + 新组件) | 高(AI 会话入口) | ✅ |
| 3 | .trellis/spec/frontend/chat.md(SubagentDrawer 1257 行 / ChatInput 1834 行 / subagentRuns.ts 含 RunAccumulator 等历史快照描述) | 中(spec 漂移) | ✅ |
| 4 | .trellis/spec/backend/subagent-runs-schema.md(引用 `agent/subagent.rs` / `chat_loop.rs:1989` / `permissions/mod.rs::ask_path` 仍指单文件) | 中(spec 漂移) | ✅ |
| 5 | docs/IMPLEMENTATION.md(20+ 处 `chat_loop.rs` 行号 + `permissions/mod.rs` 路径 + `agent/tests.rs` 9 测试 / `db/tests.rs` 2 测试 / `subagentRuns.ts` 等) | 中高(决策档案,新人查 ADR 必读) | ✅ |
| 6 | docs/ARCHITECTURE.md(⑨ `permissions/mod.rs::check()` + `chat_loop.rs:1486+` / `997-1168` / `1169+` 行号 + `permissions/mod.rs:560-571` path 解析) | 中(架构权威) | ✅ |
| 7 | 源码内注释(6+ 处旧路径引用,详见 §3.7) | 中(开发者查代码注释定位) | ✅ |
| 8 | docs/spikes/, docs/_reviews/, docs/_archive/(历史快照,保留当时语义) | 低 | ❌ 不动(用户已确认) |
| 9 | 10 个 split 自己的 PRD 已在 archive/(历史档案,不改) | 0 | ❌ 不动 |

### 漂移点详细清单(实施时逐条 grep + 改)

#### §3.1 STRUCTURE.md 漂移点(基线 06-10)
- **L72-76** 树:`ChatInput.vue / MessageList.vue / MessageItem.vue / ToolCallCard.vue / ...` 缺 `MessageItemEdit.vue` / `MessageItemFooter.vue` / `SubagentDrawerHeader.vue` / `SubagentDrawerErrorCard.vue` / `ChatInputLatencyPopover.vue` / `ChatInputHintRow.vue` + `chatInputCodeMirror.ts` 不在 §2.3 utils 节
- **L88-90** stores 树缺 `chat.types.ts` / `subagentRuns.types.ts` / `runAccumulator.ts` / `checklist.ts`
- **L92-94** utils 树缺 `chatInputCodeMirror.ts`
- **L121-126** db 树:`tests.rs` 单文件 → 实际 6 个 `tests_*.rs`
- **L127-137** llm/ + agent/ + commands/ + tools/ + git/ + projects/ 树:
  - `agent/tests.rs` 单文件 → 实际 6 个 `tests_*.rs`
  - `agent/` 缺 `subagent/` 目录(mod.rs / sink.rs / transcript.rs / truncate_summary.rs / dispatch.rs)
  - `agent/permissions/mod.rs` 单文件 → 实际 8 个模块 + 6 个 tests_*.rs
- **L155-168** 模块依赖图:`agent::*` 缺 `agent::subagent::*` + `agent::permissions::*` 8 模块
- **L212** IPC 数 54(2026-06-18 实测),但 subagent:event / subagent:finished / checklist / subagent_runs 相关 IPC 还要算上
- **L237-256** §6 schema 7 张表 → 实际还含 `subagent_runs` / `session_audit_events` / `messages` metadata 列(B2 PR3) / `subagent_runs.turn_count` 列
- **L335-337** §9.1 数据流引用 `ChatInput.vue 输入 → emit` OK(主组件还留),但 §9.3 引用 `permissions/mod.rs` → 现 `permissions/check.rs`

#### §3.2 CLAUDE.md 漂移点(基线 06-13)
- **L70-78** Architecture 段项目树: 缺 `subagent/` 目录 + 缺 6 个 `tests_*.rs` 标注 + 缺 `subagentRuns.types.ts` / `runAccumulator.ts` / `chatInputCodeMirror.ts` + 缺 6 个新组件
- **L97-104** Stores 节: `chat.ts` 写"facade" OK, 但缺 `chat.types.ts` 提到(就在 chat.ts 注释里)
- **L131-141** 关键架构决策节"Memory/指令文件系统"描述 OK;但"16 阶段请求生命周期"没标 ARCHITECTURE.md 引用(已有但可加强)
- **L143-149** "daemon 化"段是 🔴 第四档,不动

#### §3.3 .trellis/spec/frontend/chat.md 漂移点
- **L15** 表格行:`SubagentDrawer.vue ~1000 行` → 现 ~900 行(已拆 Header + ErrorCard)
- **L24** 表格行:`subagentRuns.ts 含 RunAccumulator + liveSections Map + TranscriptSection 类型` → 现 RunAccumulator 在 `runAccumulator.ts`;`TranscriptSection` 类型在 `subagentRuns.types.ts`
- **L74** Design Decision 段: 描述 `SubagentDrawer.vue 1257 行` 是历史快照;补 (拆分自 1257 行,2026-06-23 拆 Header + ErrorCard) 标注
- **L92** Design Decision 段: 描述 `ChatInput.vue 1834 行` 是历史快照;补 (拆分自 1834 行,2026-06-23 拆 composable + LatencyPopover + HintRow) 标注
- **L96-137** "Composable 接口形状"段是新增 OK(刚合);**但** L96 写 "composable ~564 行" 而 prd 估 ~340 — 这是合入实测,改 prd 不改 chat.md(以 chat.md 现状为准)
- **L15 表格** + **L24 表格** 拆后文件清单补 `SubagentDrawerHeader.vue` / `SubagentDrawerErrorCard.vue` / `ChatInputLatencyPopover.vue` / `ChatInputHintRow.vue` 4 个新文件行

#### §3.4 .trellis/spec/backend/subagent-runs-schema.md 漂移点
- **L14** "CRUD 层是 `app/src-tauri/src/db/subagent_runs.rs`" — OK 没动
- **L62-66** "Cascade test in `db/tests.rs`" → 现 `db/subagent_runs_tests.rs`(subagent_runs 域独立)
- **L66** "rid builds at `chat_loop.rs:1989`" → `run_subagent` 已搬到 `agent/subagent/dispatch.rs`,rid 构建位置改 `dispatch.rs`,**行号需重 grep**;标注 (拆分自 chat_loop.rs,2026-06-23)
- **L152** "`agent/subagent.rs`" → `agent/subagent/` 目录(整文件已拆 4 文件,build_subagent_event_payload 在 `subagent/mod.rs` 或 `transcript.rs`,**需 grep**)
- **L220** "`agent/subagent.rs::truncate_transcript_for_persistence`" → 现 `agent/subagent/truncate_summary.rs::truncate_transcript_for_persistence`
- **L270** "`permissions/mod.rs::ask_path`" → `permissions/ask.rs::ask_path`

#### §3.5 docs/IMPLEMENTATION.md 漂移点(20+ 处,grep 入口)
- 整文件 grep `permissions/mod\.rs` — 10+ 处(主要是 ADR 引用路径)
- 整文件 grep `chat_loop\.rs:[0-9]+` — 5+ 处(行号 + 路径)
- 整文件 grep `agent/tests\.rs` — 3+ 处
- 整文件 grep `db/tests\.rs` — 2 处
- 整文件 grep `subagentRuns\.ts` — 1 处(注释,类型已搬,引用源 OK)
- 整文件 grep `MessageItem\.vue` — 1 处(L269 提到 "frontend 5 文件" 列表,实际文件清单已存在 OK)
- 整文件 grep `SubagentDrawer\.vue` — 1 处(同上,清单 OK)
- 整文件 grep `ChatInput\.vue` — 1 处(同上)
- 整文件 grep `subagent\.rs` — 1 处(L152 决策列表)

具体行号漂移(chat_loop.rs 缩 522 行,大部分行号下移):
- **L38** `chat_loop.rs:657` → 现 chat_loop.rs L657 已不在原位(Err 分支还在 chat_loop,但行号变了)
- **L46** 同上
- **L71** `agent/chat_loop.rs:655-682` 改 L133-160(行号下移)
- **L79** `agent/tests.rs` 9 测试 → 现 `tests_*.rs` 6 文件 9 测试分散
- **L154** `chat_loop.rs:258-263` checklist 注入 → 现 chat_loop.rs(行号变)
- **L200** `chat_loop.rs` 注释 `loop is about to emit terminal cancelled Done` → 仍有效(注释本身)
- **L218** `agent/chat_loop.rs` 改 error arm 行号 → 现 chat_loop.rs(行号变)
- **L233-270** D3 PR2 描述: `MessageItem.vue` `chatStore.editMessage` 等 OK,store 路径 OK
- **L251** `agent/permissions/mod.rs` 引用 `audit_kind_round_trip` → 现 `agent/permissions/audit.rs`(AuditKind 17 variants 还在 audit.rs)
- **L258** `chat_loop.rs body` 引用 → 行号变
- **L268-270** D3 PR3 "影响面" 列表: `agent/permissions/mod.rs` → `agent/permissions/audit.rs`;`agent/tests.rs` → `agent/tests_*.rs` 6 文件;`db/tests.rs` → `db/messages_tests.rs`(D3 resend 测试在 messages 域)
- **L316-330** 审批内联 ADR: `permissions/mod.rs enum/payload/签名/分支` → `permissions/{types,store,check,ask}.rs`(按 ADR 描述的具体面)
- **L411-422** A2+B7 re-grill ADR: `agent/permissions/mod.rs::check` → `agent/permissions/check.rs::check`;`shell_trust.rs` OK(没动)
- **L445-447** B4 Skill ADR: `agent/chat_loop.rs` `run_chat_loop` 加 `skill_cache` 参数 OK(行号变)
- **L471-472** A2+B7 Mode 3 档化 ADR: `agent/permissions/mod.rs` → `agent/permissions/{check,mode,types}.rs`;`agent/tests.rs` → `agent/tests_*.rs`;`db/tests.rs` → `db/*_tests.rs`
- **L505-507** F5 follow-up ADR: `db/tests.rs` → `db/sessions_tests.rs`(F5 latency 在 sessions 域)

#### §3.6 docs/ARCHITECTURE.md 漂移点
- **L453** ⑨ 关 dispatch: `app/src-tauri/src/agent/permissions/mod.rs::check()` → `app/src-tauri/src/agent/permissions/check.rs::check()`
- **L665-667** ⑨ 关 RULE-A-001/002:`chat_loop.rs 副本同步` OK 仍有效;但 `CompactResult` 相关行号要 grep
- **L767** `is_parallel_eligible(&tool_calls, &permission_ctx.cwd)`(纯谓词,`chat_loop.rs:1486+`) → 仍有效(chat_loop.rs 缩了 522,但 is_parallel_eligible 在 ~1500+ 段,行号下移 ~522 → 现 ~960+);**需 grep 确认新行号**
- **L775** `is_within_root` 与 `permissions/mod.rs:560-571` 完全一致 → 现 `permissions/check.rs:560-571`(实际需 grep)
- **L777** `app/src-tauri/src/agent/chat_loop.rs:997-1168`(并行路径)+ `1169+`(串行路径) → 行号下移 ~522(实际需 grep 确认)
- **L712-718** 数据流图 `agent::chat` `agent_loop::run_one_turn()` → 现 `agent::chat_loop::run_chat_loop`(已存在 OK)

#### §3.7 源码内注释漂移点(6+ 处,grep 验证)
- **`app/src/stores/subagentRuns.ts:288`** 注释 `chat_loop.rs::run_subagent` → 现 `agent::subagent::dispatch::run_subagent`;补 (拆分自 chat_loop.rs,2026-06-23)
- **`app/src/components/chat/DrawerPermissionAskCard.vue:139`** 注释 `permissions/mod.rs` → 现 `permissions/ask.rs` (原 ask_path 所在);补 (拆分自 permissions/mod.rs,2026-06-23)
- **`app/src/components/chat/SubagentDrawer.vue:82`** 注释 `app/src-tauri/src/agent/permissions/mod.rs:406` → 现 `permissions/ask.rs:406` 之类(实际需 grep `permissions/ask.rs` + 搜索 406)
- **`app/src/utils/audit.ts:35`** 注释 `agent/permissions/mod.rs` → `agent/permissions/audit.rs`;补 (拆分自 permissions/mod.rs,2026-06-23)
- **`app/src/components/chat/DrawerThinkingBlock.vue:10`** 注释 `(ThinkingSection, from stores/subagentRuns.ts)` → `(ThinkingSection, from stores/subagentRuns.types.ts)`;补 (拆分自 subagentRuns.ts,2026-06-23)
- **`app/src/components/chat/ModeSelect.vue:30`** 注释 `Shift+Tab cycle is registered in ChatInput.vue via` → 现注册在 `app/src/utils/chatInputCodeMirror.ts` (composable);补 (拆分自 ChatInput.vue,2026-06-23)
- **`app/src/components/ChatWindow.vue:54`** 注释 `subagentRuns.ts + the PR3 PRD` → `subagentRuns.types.ts + PR3 PRD`(types 路径已变);补 (拆分自 subagentRuns.ts,2026-06-23)
- **`app/src/utils/duration.ts:1`** 注释 `extracted from MessageItem.vue` — 实际是 2026-06-19 PR 抽的(早于本次 split),且 Footer 子组件也用 duration.ts,补 (拆分自 MessageItem.vue,2026-06-19 PR);**非本次 split 范围,可选**

### 用户已 confirm 决策(2026-06-24)

1. **修复范围**: 全量(类 1-7),跳过类 8-9(spikes/reviews/archive 历史快照不动)
2. **旧路径标注方式**: 加 (拆分自 X,2026-06-23/24) 注释,**不重写**为新行号(保留历史行号作追溯锚点)
3. **代码质量门**: `vue-tsc --noEmit` + `cargo check`(纯文档 / 注释,无逻辑变更,但 vue-tsc 验证 import 路径仍可解析)

## Requirements

### 必含(每条对应 AC 项)

* **R1**: STRUCTURE.md 顶部校注更新到 06-24 现状(不再"06-10 快照已滞后"),§2/§3 文件树按真实结构重写(11 个新文件 + 1 个 subagent/ 目录 + 6 个 tests_*.rs),§5 IPC 数 + §6 schema 表(7 → 8/9),§9.3 路径更新
* **R2**: CLAUDE.md Architecture 段补全 `subagent/` 目录 + 6 个 `tests_*.rs` + `chat.types.ts` + `subagentRuns.types.ts` + `runAccumulator.ts` + 6 个新组件 + `chatInputCodeMirror.ts` composable
* **R3**: `.trellis/spec/frontend/chat.md` 文件清单表格补 4 个新文件行;§74/§92 历史描述加 (拆分自 X 行) 标注
* **R4**: `.trellis/spec/backend/subagent-runs-schema.md` 5 处旧路径(行 62/66/152/220/270)全部更新 + 加 (拆分自 X) 标注
* **R5**: docs/IMPLEMENTATION.md grep 全文件,把 `permissions/mod.rs` / `agent/tests.rs` / `db/tests.rs` / 关键 `chat_loop.rs` 行号全部按"旧路径 + (拆分自 X,新位置)"格式标注;**不删任何 ADR**(决策档案不可再生)
* **R6**: docs/ARCHITECTURE.md 3-4 处路径 + 关键 `chat_loop.rs` 行号更新 + 加 (拆分自 X) 标注
* **R7**: 6+ 处源码内注释路径更新 + 加 (拆分自 X,2026-06-23/24) 标注
* **R8**: 修复后跑 `cd app && pnpm vue-tsc --noEmit` 验证 import 路径仍可解析(纯文档不应破坏 typecheck,但保守验证)
* **R9**: 修复后跑 `cd app/src-tauri && PKG_CONFIG_PATH=... cargo check` 验证 Rust 注释内路径仍可解析(注释不参与编译,但仍 grep sanity)

### 不变

* 任何 ADR 内容、决策、影响面、关联 RULE 引用 — 决策档案是**不可再生历史**,**只**更新路径 + 行号标注,**不重写**理由
* 任何代码逻辑、API、wire shape
* 10 个 split 已合的 commit message + archive PRD(历史)
* DEBT.md(B-003/B-006/B-007 已正确指向新路径,不在修复范围)
* 任何 spikes / reviews / _archive(用户已 confirm 跳过)

### 关键决策

* **D1**: 旧路径保留 + 标注(用户已选)vs 直接重写为新路径。保留旧路径可让历史 commit / git blame 仍可追溯;**选保留**
* **D2**: 行号处理 = 旧行号 + (现新行号 X, 拆分自原 L_Y) 格式。如 `chat_loop.rs:997-1168` → 留旧行号 + `(现 chat_loop.rs:475-646,拆分自原 chat_loop.rs:997-1168,2026-06-23 run_subagent 抽走 522 行)` 标注。理由:旧行号对应 commit `x` 当时代码状态,git blame 可定位;新行号对应当前代码状态
* **D3**: 源码注释更新用 `(拆分自 X,2026-06-23)` 注释尾巴,**不重写**为新路径,保留旧路径在主文本。例如 `chat_loop.rs::run_subagent` → `chat_loop.rs::run_subagent(拆分自 chat_loop.rs,2026-06-23 → 现 agent::subagent::dispatch::run_subagent)`。这样 git blame `chat_loop.rs::run_subagent` 仍能找到原 commit
* **D4**: 修复顺序:STRUCTURE.md / CLAUDE.md(类 1-2 严重)→ spec 文档(类 3-4)→ IMPLEMENTATION/ARCHITECTURE(类 5-6)→ 源码注释(类 7)。每类完成后跑一次 `git diff --stat` 估算改动量,避免单次 commit 过大
* **D5**: 提交策略 = 单 PR 多 commit(每个 split 对应一个 docs commit,便于 review):
  - commit 1: docs(STRUCTURE / CLAUDE / chat.md / subagent-runs-schema) — 4 文件 1 commit
  - commit 2: docs(IMPLEMENTATION / ARCHITECTURE) — 2 文件 1 commit
  - commit 3: docs(source-code comments) — N 文件 1 commit
  - commit 4: chore(task): archive 本任务
  - commit 5: chore: record journal

## Acceptance Criteria

- [ ] STRUCTURE.md 顶部校注更新(不再"06-10 快照已滞后");§2 前端树 / §3 后端树反映 10 个 split 后的真实文件结构
- [ ] STRUCTURE.md §5 IPC 数更新(如新增了 `get_subagent_run` / `list_subagent_runs_by_session` 等需补;查 `app/src-tauri/src/commands/` 现状)
- [ ] STRUCTURE.md §6 schema 表更新(增加 `subagent_runs` / `session_audit_events` 表 + 关键列如 `turn_count` / `metadata` JSON)
- [ ] CLAUDE.md Architecture 段补 6+ 新文件 + 1 新目录
- [ ] `.trellis/spec/frontend/chat.md` 文件清单 + 历史描述更新
- [ ] `.trellis/spec/backend/subagent-runs-schema.md` 5 处旧路径更新
- [ ] docs/IMPLEMENTATION.md 20+ 处旧路径加 (拆分自 X) 标注
- [ ] docs/ARCHITECTATION.md 3-4 处旧路径更新
- [ ] 6+ 处源码内注释路径更新
- [ ] `vue-tsc --noEmit` 全绿
- [ ] `cargo check` 全绿
- [ ] 单 PR 多 commit,commit message 格式对齐项目惯例(`docs(STRUCTURE): ...` / `chore(task): archive ...`)
- [ ] DEBT.md 无新增 RULE(本任务纯文档维护,无新债)

## Definition of Done

- 6 个文档 + 6+ 处源码注释全部更新到 split 后真实路径 + 旧路径加 (拆分自 X, 2026-06-23/24) 标注
- 决策档案(IMPLEMENTATION.md / ARCHITECTURE.md)内容零变更,仅路径 + 行号标注
- 类型检查 + 编译检查全绿(无逻辑变更,验证 sanity 即可)
- 单 PR 合入 main,commit 切分清晰(4 类文件 + 1 archive + 1 journal)
- `.trellis/workspace/Carlos-home/journal-3.md` 追加本次任务 summary
- DEBT.md 维持当前 12 项 open 状态(本任务不开新债)
- prd.md 收口 + jsonl curated → `task.py start` → 实现 → check → archive(四段式 commit)

## Out of Scope

* 改任何代码逻辑 / API / wire shape
* 改 ADR 内容(决策档案内容不可再生)
* 改 10 个 split 已合的 commit message
* 改 archive/ 目录下的 PRD
* 改 DEBT.md(无新债)
* 改 spikes/ / reviews/ / _archive/ 历史快照(用户已 confirm)
* 改 tests(无测试逻辑变更)
* 改 `package.json` / `Cargo.toml`(无依赖变更)
* 跑 `vitest` / `cargo test --lib`(纯文档,无 test 变更)
* 拆任何文件(本任务纯 sync,不引入新重构)
* 提任何新 RULE 到 DEBT.md(无新债)
* 触碰 `.trellis/workspace/Carlos-home/journal-3.md` 之外的其他 journal 文件

## Technical Notes

### 实施步骤(顺序)

1. **类 1-2** (STRUCTURE.md + CLAUDE.md)
   - 顶部校注:STRUCTURE.md L7-15 警告段改写为"快照基线 06-24,本文件由本任务同步"
   - §2 前端树(L60-95):加 6 个新组件 + `chatInputCodeMirror.ts`
   - §2.4 store 树:加 `chat.types.ts` / `subagentRuns.types.ts` / `runAccumulator.ts` / `checklist.ts`
   - §3 后端树(L119-153):加 `subagent/` 5 文件 + `permissions/` 8 文件 + `tests_*.rs` 6+6 文件
   - §5 IPC 表:补 `get_subagent_run` / `list_subagent_runs_by_session` / `update_checklist` 等
   - §6 schema:补 `subagent_runs` / `session_audit_events` 表
   - §9.3:更新 `permissions/check.rs` 引用
   - CLAUDE.md Architecture 段 L70-78 + L97-104:对应补 6+ 新文件

2. **类 3-4** (chat.md + subagent-runs-schema.md)
   - chat.md L15 / L24:补 4 个新文件行(SubagentDrawerHeader / SubagentDrawerErrorCard / ChatInputLatencyPopover / ChatInputHintRow)
   - chat.md L24:`subagentRuns.ts` 行补 (RunAccumulator 拆分自此,2026-06-23)
   - chat.md L74 / L92:加 (拆分自 X 行,2026-06-23) 标注
   - subagent-runs-schema.md L62:`db/tests.rs` → `db/subagent_runs_tests.rs`
   - subagent-runs-schema.md L66:`chat_loop.rs:1989` → 加 (现 dispatch.rs, 拆分自 chat_loop.rs)
   - subagent-runs-schema.md L152:`agent/subagent.rs` → `agent/subagent/` 目录(具体行号需 grep)
   - subagent-runs-schema.md L220:`agent/subagent.rs::truncate_transcript_for_persistence` → `agent/subagent/truncate_summary.rs::truncate_transcript_for_persistence`
   - subagent-runs-schema.md L270:`permissions/mod.rs::ask_path` → `permissions/ask.rs::ask_path`

3. **类 5-6** (IMPLEMENTATION.md + ARCHITECTURE.md)
   - 整文件 grep 入口(已列在 §3.5)
   - 每个命中点:旧路径 + 行号 + `(拆分自 X, 2026-06-23, 现 L_Y)` 标注格式
   - **不删任何 ADR 内容**

4. **类 7** (源码注释)
   - 6+ 处具体文件 + 行号(已列在 §3.7)
   - 每个注释:旧路径保留 + `(拆分自 X, 2026-06-23)` 尾巴

5. **验证**
   - `cd app && pnpm vue-tsc --noEmit`
   - `cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check`
   - `git diff --stat` 估算改动量

6. **commit 切分 + archive + journal**(4 段式)

### 关键文件路径速查(实施时 grep 用)

```
# 后端 split 后
app/src-tauri/src/agent/chat_loop.rs              # 主循环
app/src-tauri/src/agent/subagent/mod.rs           # 类型 + helpers
app/src-tauri/src/agent/subagent/sink.rs          # SubagentBufferSink
app/src-tauri/src/agent/subagent/transcript.rs    # transcript 类型
app/src-tauri/src/agent/subagent/truncate_summary.rs  # 4 MiB cap
app/src-tauri/src/agent/subagent/dispatch.rs      # run_subagent + resolve_project_id + SUBAGENT_MAX_TURNS
app/src-tauri/src/agent/tests_common.rs           # 共享 helper
app/src-tauri/src/agent/tests_cancellation.rs     # 9 测试
app/src-tauri/src/agent/tests_envelope.rs         # 4 测试
app/src-tauri/src/agent/tests_prompts.rs          # 11 测试
app/src-tauri/src/agent/tests_agent_loop.rs       # 26 测试
app/src-tauri/src/agent/tests_subagent.rs         # 12 测试
app/src-tauri/src/agent/permissions/{types,store,payload,mode,audit,check,ask,dangerous,shell_trust}.rs
app/src-tauri/src/agent/permissions/tests_common.rs
app/src-tauri/src/agent/permissions/tests_{check,ask,audit,store,payload,types,mode}.rs
app/src-tauri/src/db/{projects,sessions,providers,permissions,messages,subagent_runs}_tests.rs

# 前端 split 后
app/src/stores/chat.ts                  # runtime
app/src/stores/chat.types.ts            # ~310 行 types
app/src/stores/subagentRuns.ts          # store
app/src/stores/subagentRuns.types.ts    # ~354 行 types
app/src/stores/runAccumulator.ts        # ~537 行 RunAccumulator + parseTranscriptJson
app/src/components/chat/MessageItem.vue        # ~770 行
app/src/components/chat/MessageItemEdit.vue     # ~180 行
app/src/components/chat/MessageItemFooter.vue   # ~120 行
app/src/components/chat/SubagentDrawer.vue      # ~900 行
app/src/components/chat/SubagentDrawerHeader.vue    # ~250 行
app/src/components/chat/SubagentDrawerErrorCard.vue # ~100 行
app/src/components/chat/ChatInput.vue           # ~712 行
app/src/components/chat/ChatInputLatencyPopover.vue  # ~365 行
app/src/components/chat/ChatInputHintRow.vue         # ~251 行
app/src/utils/chatInputCodeMirror.ts          # ~564 行 composable
```

### 改动量估算

| 文件 | 改动行数 | 备注 |
|---|---|---|
| STRUCTURE.md | ~50 行(树 + 校注 + §5 + §6) | 重写 §2/§3 树 |
| CLAUDE.md | ~15 行(Architecture 段补) | 增量 |
| chat.md | ~10 行(4 个文件行 + 2 个历史描述标注) | 增量 |
| subagent-runs-schema.md | ~10 行(5 处路径) | 增量 |
| IMPLEMENTATION.md | ~30 行(20+ 处标注) | 增量(每处加 (拆分自 X) 注释) |
| ARCHITECTURE.md | ~10 行(3-4 处路径) | 增量 |
| 源码注释 | ~10 行(6+ 处) | 增量 |
| **合计** | **~135 行** | 7 文件 |

### 关键参考(已知先例)

- DEBT.md B-003 / B-006 / B-007 已正确指向新路径(B-003 `permissions/check.rs:386` / B-006 `permissions/audit.rs:21` / B-007 `permissions/mode.rs:26-28`)— 本任务不在 DEBT.md 开新债
- 10 个 split 自己的 PRD 都在 archive/ 下,描述与最终文件树一致
- 项目惯例:旧路径 + 标注 格式(用户已 confirm)
- 项目 commit 惯例:`docs(<scope>): <subject>` / `chore(task): archive <date-name>` / `chore: record journal`

### Reference

- [`.trellis/spec/guides/cross-layer-thinking-guide.md`](../../spec/guides/cross-layer-thinking-guide.md) — 跨层一致性验证(本任务不引入跨层改动,但 drift 影响新人跨层查找)
- [`.trellis/spec/guides/debt-status-evolution-guide.md`](../../spec/guides/debt-status-evolution-guide.md) — DEBT.md 维护规则(本任务不开新债)
- `.trellis/workspace/Carlos-home/journal-3.md` — 追加 summary 的目标文件
- 用户确认记录:本任务"修复范围" + "旧路径标注"两个决策由 AskUserQuestion 在 2026-06-24 收口
