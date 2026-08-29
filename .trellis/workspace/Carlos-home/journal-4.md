# Journal - Carlos-home (Part 4)

> Continuation from `journal-3.md` (archived at ~2000 lines)
> Started: 2026-07-05

---



## Session 89: E1 CI 测试自动化管线(双 job + drain race / mtime fence 2 flaky 修复)

**Date**: 2026-07-05
**Task**: E1 CI 测试自动化管线(双 job + drain race / mtime fence 2 flaky 修复)
**Branch**: `main`

### Summary

V2 第三档 E1 落地。双 job CI。首跑暴露 2 预存 flaky(drain race 生产代码 + mtime fence 测试侧),修复后 3x 全量 1274/1274 稳定。项目首次 PR #1。

### Main Changes

V2 第三档 E1 落地,首次上 CI。双 job 并行。CI 首跑暴露 2 个预存 flaky,修复后 3x 全量 1274/1274 稳定。

### Main Changes

- `.github/workflows/ci.yml`(+86):双 job — rust(apt webkit2gtk-4.1-dev 等 + dtolnay/rust-toolchain stable + Swatinem/rust-cache + `cargo fmt --check` + `cargo test --lib`)/ frontend(pnpm 10 + node 22 + cache + `pnpm install --frozen-lockfile` + `pnpm test` + `pnpm build` 含 vue-tsc)。paths-ignore 忽略 `**/*.md`/`docs/**`/`.trellis/**`;concurrency cancel-in-flight。
- `README.md`:CI badge(`wukaijin/everlasting`)。
- cargo fmt 全量格式化 118 文件(机械,单独 commit `314702f`)。
- **drain race 修复**(`background_shell/in_memory.rs::drain_notifications`,+53/-5,commit `84d607e`):destructive pop 与 shell 完成 push 竞速,echo fork+exec+exit+push 可能晚于 turn 切换(μs)→ drain pop 空,notification 丢失(**真实生产 race**:快 shell + loop 早结束 → LLM 不知道 shell 完成)。修复 = 队列空 + 近期(<200ms)running shell 时 yield+poll(5ms,cap 100ms);dev server(>200ms)不受影响;队列非空/无 running 立即返回(原行为)。
- **mtime fence 修复**(`memory/tests.rs::loader_mtime_fence_sees_file_change`,+30/-2,commit `7eb5e81`):FS mtime 精度 flaky(两次连续 write 间隔过短,ext4 ns 但 overlay/tmpfs + 并行负载弱化),原 sleep 15ms 在写 v2 后对 v2 mtime 无效;改 spin until mtime 真推进(cap 2s,确定性)。

### Testing

- [OK] `cargo test --lib`: 1274 passed(3x 全量稳定)
- [OK] `pnpm test`: 718 passed;`pnpm build`: vue-tsc 0 err + vite build
- [OK] `cargo fmt --check`: 干净
- [OK] CI(GitHub Actions PR #1):双 job 全绿
- [OK] drain race 10/10 单跑稳定

### Notes

- CI 首跑暴露的 2 个 flaky 都是预存(项目此前无 CI,本地 N 次侥幸过):background_shell drain race(生产代码,真实 race)+ loader mtime fence(FS 精度)。**这正是 CI 该发现的**。
- drain race 修复改了生产语义(drain best-effort wait),但仅在"队列空 + 近期 running shell"边缘 case,常见 case 零开销。
- 走分支 + PR #1(项目首次 PR),merge commit `c2ba7ce`。CLAUDE.md "branch first" 全局指令 vs 项目 main 直接 commit 惯例:本次走 branch(更安全 + PR 触发 CI 验证 workflow 自身)。
- gh CLI 未装,PR/merge 走 GitHub UI。
- follow-up:clippy gate(先本地清 warning)+ release.yml(tag-triggered 出包)。


### Git Commits

| Hash | Message |
|------|---------|
| `314702f` | (see git log) |
| `c64df6c` | (see git log) |
| `84d607e` | (see git log) |
| `7eb5e81` | (see git log) |
| `c2ba7ce` | (see git log) |
| `20b71b9` | (see git log) |
| `e1764d3` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 90: C2+ 循环检测主动干预(loop_hit_count + QuestionStore 复用 + 三分支)

**Date**: 2026-07-06
**Task**: C2+ 循环检测主动干预(loop_hit_count + QuestionStore 复用 + 三分支)
**Branch**: `main`

### Summary

C2+ 循环检测主动干预落地。C2(06-24)软提示只注入 hint 不终止 loop,MAX_TURNS=200 是唯一硬兜底,死循环烧满 200 turn 敞口未堵。补中间主动层:per-run-local loop_hit_count 连续 3 轮 detect 命中 → chat_loop 顶层(harness-driven,非 LLM tool 路径)复用 QuestionStore + emit_tool_question + 前端 AskUserQuestionCard 询问是否终止。Hard/Soft 共用 N=3 单一计数器,verdict None 一轮即归零。select! 三分支:终止→Done{loop_terminated}/继续→count 清零+增强 hint/cancel→Done{cancelled}。worker effective_is_worker gate 直接 break(dispatch_result caller-append 告知父,不写 audit)。新增 AuditKind::LoopIntervention 无 migration(payload hit_count/verdict_kind/action/run_id)。AlreadyPending 降级走原 hint。run_chat_loop 28 参签名零改动。3 偏差(run_id 占位 future-proof / dispatch caller-append 不加 format 第 5 参 / worker 复用 SubagentStatus::Incomplete 避免 migration)trellis-check 评估可接受+代码注释。1282 后端 cargo test --lib + 728 前端 pnpm test + vue-tsc 0 err + fmt 干净。loop_detection.rs 零改动(31 单测)。trellis-check 零 finding 一次过关。完整 PRD 走 archive/2026-07/07-05-c2-loop-active-intervention/。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `943d951` | (see git log) |
| `212aa1b` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 91: 跨 session 待处理交互 UI 提醒(角标+徽章+toast 三档)

**Date**: 2026-07-08
**Task**: cross-session-pending-indicator
**Branch**: `main`

### Summary

消除「session A 用着、session B 有 mode change/ask_user_question 申请却视觉感知不到」的盲区。三档提醒:A=SessionList 条目角标(按 `PendingInteraction.kind`:mode_change=`refresh`+target_mode 配色 / question=`info`,复用 `session-item__pending-approval` CSS+pulseDot);B=AppHeader 顶栏新建 `PendingBadge`(全局计数 `pendingBySession.size`,跨 project 天然成立,点击切当前 project 内 ts 最大 pending session);C=非当前 session 新 pending 到达 → 复用 `projectsStore.showToast`(扩展第四参 `opts.sessionId` 支持同 project 点击跳转,AppShell `onToastClick` 同 project 切 / 跨 project 仅 dismiss)。Q3 gate(当前 session 不 toast)抽为顶层纯函数 `buildPendingNotification` 单测。4 决策(Q1 mode+ask_user_question 不并入权限审批 / Q2 顶栏徽章 / Q3 仅非当前 session toast / Q4 仅同 project 跳转)全部**零后端改动**,复用既有 questionCards 全量缓存 + 角标范式 + toast。trellis-check 自修复 1 个 R5 偏差:角标 mode 配色原写反(plan=accent/edit=muted),沿用 `RequestModeChangeCard` 实际映射 edit=accent / plan=tool-read / yolo=error。806 vitest 全绿(+12 例)+ vue-tsc/vite build 零错。归档 archive/2026-07/07-08-cross-session-pending-indicator/。

### Main Changes

- `app/src/stores/projects.ts`:`ToastMessage` +`sessionId?`;`showToast` +第四参 `opts?:{sessionId?}`(durationMs 仍是第三参,向后兼容)
- `app/src/stores/streamController.ts`:顶层导出纯函数 `buildPendingNotification`(Q3 gate 可单测)+ `handleModeChangeRequest`/`handleToolQuestion` 在 `addPending` 后触发 toast
- `app/src/components/layout/AppShell.vue`:toast 点击改 `onToastClick`(同 project 切 session,跨 project / 无 sessionId 仅 dismiss)
- `app/src/components/SessionList.vue`:`pendingBadge` helper(kind 分发)+ 两处 title-row 角标 + `.session-item__pending-interaction` CSS
- `app/src/components/layout/PendingBadge.vue`:**新建**(全局计数徽章)
- `app/src/components/layout/AppHeader.vue`:TitleBar slot 内挂 `<PendingBadge />`
- 3 新测试:`pendingNotification.test.ts`(5)/ `PendingBadge.test.ts`(4)/ `projects.toast.test.ts`(3)

### Git Commits

| Hash | Message |
|------|---------|
| `b95e8c5` | feat(pending-indicator): 跨 session 待处理交互 UI 提醒(角标+徽章+toast 三档) |
| `802cde7` | chore(task): archive 07-08-cross-session-pending-indicator |

### Testing

- [OK] vitest 806 tests / 50 files 全绿(新增 12 例)
- [OK] vue-tsc --noEmit + vite build 零类型错
- [TODO] 手测 `pnpm tauri dev`(角标脉冲 / 徽章位置 / toast 文案 / compact 三角标并列)— 自动化测试覆盖核心逻辑,真实视觉待眼验

### Status

[OK] **Completed**

### Next Steps

- None - task complete(手测眼验可随时补)


## Session 91: C3 review plugin resource pack 实施

**Date**: 2026-07-27
**Task**: C3 review plugin resource pack 实施
**Branch**: `feat/review-plugin-pack-c3`

### Summary

交付 review epic 的 C3（review plugin 内容层）：4-state 状态机 + reviewer 只读角色 + 4 skill + builtin.rs 内置化（含修 07-09 留下的两处 loader 硬编码 dev 隐藏依赖）+ dev skill 衔接指引 + review-state.json schema 定稿（C2 跨任务契约）。design §4 砍掉 emit_review_state_updated 工具方案改 write_file（贴 PRD）。对抗 check 修 1 个 schema 缺陷（status=failed→error）。1590 tests pass，clippy 零新增。沉淀 workflow-plugin-builtin.md spec。本轮连归档遗留的 C0/C1。

### Git Commits

| Hash | Message |
|------|---------|
| `83207f4` | (see git log) |
| `79711d1` | (see git log) |
| `83c4b49` | (see git log) |

### Status

[OK] **Completed**


## Session 92: C2 review visualization 实施（review epic 子任务收官）

**Date**: 2026-07-27
**Task**: C2 review visualization 实施（review epic 子任务收官）
**Branch**: `feat/review-viz-c2`

### Summary

交付 review epic 最后一个子任务 C2（review-state 矩阵视图）：前端「轮次×模型」矩阵 + 维度对比 + triage + source_run_id 跳转。核心决策——C3 砍了 emit_review_state_updated 工具后，刷新机制改用 streamController.handleToolCall 路由（照 B12 checklist 模式：write_file 命中 review-state.json → reviewStateStore.refresh，slug 守门 + 200ms debounce），零后端事件改动。后端两个读取 IPC（get_review_state 三态 + get_current_task_slug）。ReviewState TS/Rust 类型严格对齐 C3 R7 schema（跨任务契约）。对抗 check PASS 零缺陷。沉淀 streamController→store 路由模式 spec。1599 cargo + 983 pnpm tests pass。至此 review epic 4 子任务（C0/C1/C2/C3）全部完成，父 epic 仅剩集成验收（端到端跑通 review 流）。

### Git Commits

| Hash | Message |
|------|---------|
| `6523aef` | (see git log) |
| `14d2ae0` | (see git log) |
| `b4e9fb9` | (see git log) |

### Status

[OK] **Completed**


## Session 93: tests_agent_loop.rs 目录化拆分(5674 行 → hub + 9 簇,41 测原样)

**Date**: 2026-08-09
**Task**: tests_agent_loop.rs 目录化拆分(5674 行 → hub + 9 簇,41 测原样)
**Branch**: `main`

### Summary

把 agent/tests_agent_loop.rs(5674 行,41 测 + 5 helper)目录化为 tests_agent_loop/(hub mod.rs + 9 簇文件)。纯搬迁:agent/mod.rs:63 零改动,函数集 46 项前后逐名比对零增删改。规划期发现并修正 PRD 事实错误(原表 32 测,实测 41——漏算 error_after_tool/c3_compaction/c3_still_over/p5_soft_block×2/a5plus_retry×3),据此重建簇映射。mock_provider 与 error_path 在源文件交错,故按测试函数逐个 sed 提取而非连续区间切片。验证:AC1-AC6 全 PASS,模块 41/全量 1662 全绿,clippy+fmt 零警告,最大文件 basic.rs 1085 <1200。共享 helper 仅 messages_to_text 真跨文件(checklist+notifications)提 hub pub(super);load_assistant_rows/p5_seed 留使用处私有。tests_common re-export 在 hub 透传(已核实 load-bearing)。

### Git Commits

| Hash | Message |
|------|---------|
| `3d77217` | (see git log) |
| `a104bc2` | (see git log) |

### Status

[OK] **Completed**


## Session 94: memories_tests.rs 目录化拆分(2241 行 → hub 35 + 9 簇,49 测)

**Date**: 2026-08-09
**Task**: memories_tests.rs 目录化拆分(2241 行 → hub 35 + 9 簇,49 测)
**Branch**: `main`

### Summary

将 db/memories_tests.rs(2241 行,49 测)按被测功能簇目录化拆分为 db/memories_tests/(hub mod.rs 35 行 + 9 簇文件)。纯搬迁:hub 用 #[allow(unused_imports)] use super::memories; re-export trick(沿用 tests_agent_loop 先例)让簇文件 use super::memories::{...} 零改动;test_pool/make_pool 提 hub pub(super),input/reseat_created_at 留簇内私有。db/mod.rs:60 声明零改动。订正 PRD 笔误(测试数 40→49,helper 3→4 含 input)。全量验收:1662 passed(基线持平)、49 测守恒、clippy 0 warning、fmt clean、db/mod.rs diff 空、AC6 文档 sweep 0 引用、最大簇 list_delete_search.rs 657 < 1200。收官表 directory-structure.md 同步。

### Git Commits

| Hash | Message |
|------|---------|
| `acb1480` | (see git log) |
| `93a0f7d` | (see git log) |

### Status

[OK] **Completed**


## Session 95: sessions_tests.rs 目录化拆分(hub + 5 簇, 35 测)

**Date**: 2026-08-09
**Task**: sessions_tests.rs 目录化拆分(hub + 5 簇, 35 测)
**Branch**: `main`

### Summary

db/sessions_tests.rs(1493 行 / 35 测)按被测功能簇拆为 sessions_tests/ 目录(hub mod.rs + session_crud/fields_worktree/system_events/model_usage/latency_message 5 簇),纯搬迁零逻辑改动。关键差异点:hub re-export 从 memories 先例的 1 兄弟扩到 6 兄弟(migrations/models/projects/providers/sessions/types)。验收全绿:35 测过滤 + 1662 全量、clippy+fmt 0 警告、db/mod.rs 零改动、各文件 <1200(最大 latency_message 564)。收官表进度 3/4(agent_loop/memories/sessions ✅,subagent ⏳)。

### Git Commits

| Hash | Message |
|------|---------|
| `6a6c96c` | (see git log) |
| `d20ff7b` | (see git log) |

### Status

[OK] **Completed**


## Session 96: directory-structure 收官表文档订正(loader 错配 + subagent_runs_tests 漏网登记)

**Date**: 2026-08-10
**Task**: directory-structure 收官表文档订正(loader 错配 + subagent_runs_tests 漏网登记)
**Branch**: `main`

### Summary

审计 08-07~08-09 大文件拆分收官表发现三处与代码现状偏移,逐条 wc -l/git show 核实后订正 directory-structure.md。纯文档,零代码逻辑变更。

### Main Changes

- line 72 loader 行三列错配两个同名 loader → 拆为两行(agent/subagent/loader.rs 2290→批1 7b60b55→319 扁平拆 frontmatter.rs+cache.rs;skill/loader.rs 1660→批2 dfcb9ba→649 loader/frontmatter.rs)
- 新增遗留条目 db/subagent_runs_tests.rs 1219(范围外漏网,group-chat Phase 4 + subagent resume 迭代长成)
- line 93 措辞 全仓…全部<1200 → 批范围内…全部<1200,附范围外遗留清单(subagent_runs_tests + 前端 4 文件)
- 表头日期 08-09 → 08-10

### Git Commits

| Hash | Message |
|------|---------|
| `25fb619` | (see git log) |
| `cd79d7a` | (see git log) |

### Testing

- [OK] wc -l 复核改动引用的 10 个行数全部与当前代码一致(9/9 评审核验 + 1 处双 loader 拆分史)

### Status

[OK] **Completed**

### Next Steps

- 可选:拆 db/subagent_runs_tests.rs 1219(沿用 tests_* 目录化模式,另立任务)


## Session 97: remote-control epic 收尾归档 + 下一任务推荐

**Date**: 2026-08-14
**Task**: remote-control epic 收尾归档 + 下一任务推荐
**Branch**: `main`

### Summary

08-13 合入 main 的 remote-control epic(94828cb)任务树收尾:归档 08-13-mobile-polish + 08-11-remote-control-epic;依据 ROADMAP/DEBT 推荐下一任务:D2 跨 session 全文搜索(首选)/ C7 tools[] token 治理 / L1b 真 PTY / A4+ 成本聚合

### Git Commits

| Hash | Message |
|------|---------|
| `0141a44` | (see git log) |

### Status

[OK] **Completed**

### Next Steps

- D2 跨 session 全文搜索(用户驱动 MVP 单 PR + Agent 驱动 search_history tool)


## Session 98: C7 tools[] 上下文 token 治理:R1 度量 + R3 静态裁剪

**Date**: 2026-08-14
**Task**: C7 tools[] 上下文 token 治理:R1 度量 + R3 静态裁剪
**Branch**: `feat/c7-tools-token-governance`

### Summary

把 tools[] 当作与 messages 并列的上下文治理对象。R1:turn_trace.tools_token 列 + drive.rs cl100k 估算 + TracePanel tools cell(占比 = tools/context_input,禁 double-count)。R3:filter_tools_for_session_type 经典聊天裁 nominate_speaker/end_discussion。R2/D 降级 Phase 2。

### Main Changes

- R1:turn_trace.tools_token 列(add_turn_trace_column_if_missing migration + upsert/list CRUD)+ drive.rs freeze 后 cl100k 估算 post-filter ToolDef JSON + TracePanel TurnCard tools cell
- R3:filter_tools_for_session_type 经典聊天砍 nominate_speaker/end_discussion(~465 tok/轮),group_chat no-op(白名单优先);drive.rs 过滤链加第三环 mode→workflow→session_type
- spec:token-usage-tracking.md 加 C7 scenario(7 段 code-spec + cache 率 no-double-count Wrong/Correct)

### Git Commits

| Hash | Message |
|------|---------|
| `10ad4f8` | (see git log) |

### Testing

- [OK] cargo test -p everlasting --lib 1692(1 个无关 daemon tunnel timing flake,隔离通过)
- [OK] pnpm test 1037 + pnpm vue-tsc clean + cargo clippy clean

### Status

[OK] **Completed**

### Next Steps

- Phase 2 ①:R1 度量数据 tools[] 占 context 窗口 >15% → 启动 D(Stub 注册)
- Phase 2 ②:配原生 Anthropic provider 后重启 R2(cache 断点;relay 实测 cache_creation=0 零收益)
- live AC1 烟测:重编 release + 重启 :7456 daemon(用 pid kill,别用 pkill -f 端口串)+ 跑一轮看 TracePanel tools_token ≈ 7-8k

## Session 99: C7 live 烟测(AC1 验证)+ Phase 2 触发判定 + 归档

**Date**: 2026-08-14
**Task**: C7 tools[] token 治理收尾(live AC1 烟测 → 数据判定 → 归档)
**Branch**: `main`

### Summary

用户重编 release + 重启 :7456 daemon 后,经 daemon HTTP API(create_session + /api/v1/agent/chat 单轮"早上好")实跑 live 烟测:`turn_trace.tools_token=6773` / `context_input=17602` = **38.5%**,AC1 验证通过(≈7-8k 预期带内,R3 已裁 ~465)。**Phase 2 判定:D(Stub 注册)触发线(>15%)已过**;同轮 memory 指令块估算 ~7-8k(≈42%,CLAUDE.md 27.8KB + AGENTS.md 3.2KB 复核)→ BACKLOG §3.1 评估结论:memory 块治理确认值得排期。烟测 session 已删,ROADMAP(C7 → §1.2,22 项)+ BACKLOG §3.1(评估结论)同步,task.py archive 归档。

### Git Commits

| Hash | Message |
|------|---------|
| `88853eb` | docs: C7 live 烟测数据(tools 38.5% → D 触发线已过)+ ROADMAP/BACKLOG 同步 |
| `099e699` | chore(task): archive 08-14-c7-tools-token-governance |

### Testing

- [OK] live:daemon API 建会话 + 单轮 chat,messages 落库(seq 0/1)+ turn_trace seq=1 落 tools_token=6773(列由 migration 幂等添加)
- [OK] smoke session 经 delete_session API 清理;docs/archive 提交过 lefthook pre-commit

### Status

[OK] **Completed**

### Next Steps

- **下一任务二选一**:① C7 Phase 2 之 D(Stub 注册,tools 38.5% > 15% 触发线已过,省窗口大头 use_ui/ask_user_question 静态裁不动只能 Stub 治);② memory 指令块治理(BACKLOG §3.1,~7-8k ≈ 42%,手段 = 按相关性裁剪/分段加载)
- R2(Anthropic cache 断点)继续挂起,等原生 Anthropic provider

## Session 100: turn-smoke.sh 单轮烟测脚本沉淀 + AGENTS.md DB 速查

**Date**: 2026-08-14
**Task**: 把 Session 99 的手工烟测流程(daemon API 建会话 → 单轮 → 查 turn_trace → 删会话)沉淀为脚本;AGENTS.md 补 DB 位置(用户建议,省下次找 DB 的弯路)
**Branch**: `main`

### Summary

`scripts/turn-smoke.sh`:health 预检 → list_projects 按路径解析 project_id(查不到自动 create_project)→ 建临时 session → /api/v1/agent/chat 单轮 → 每 5s 轮询 turn_trace(默认 180s 超时;tools_token 列缺失 = 二进制早于 C7 R1 的明确报错)→ 报 seq/tools_token/input/ctx/占比 → EXIT trap 自动删 session(--keep 保留)。实跑验证 tools_token=6773 / ctx=17603 = 38%,与手工一致。AGENTS.md 加「DB / 单轮烟测速查」节(everlasting.db 路径 + WAL writer 注意 + 脚本入口)。

### Git Commits

| Hash | Message |
|------|---------|
| `dcb3f9a` | chore(scripts): turn-smoke.sh 单轮烟测自动化 + AGENTS.md 补 DB 位置速查 |

### Testing

- [OK] bash -n 语法检查 + 端到端实跑(真实 LLM 一轮,session 自动清理)

### Status

[OK] **Completed**

### Next Steps

- 下一任务仍待定:D(Stub 注册,38.5% > 15% 触发线已过)vs memory 指令块治理(BACKLOG §3.1)


## Session 99: 前端样式优化 R1:VLM 评审流水线 + WP1-4 实施

**Date**: 2026-08-14
**Task**: 前端样式优化 R1:VLM 评审流水线 + WP1-4 实施
**Branch**: `main`

### Summary

固化 scripts/ui-review.sh(headless 截图 + mmx vision 评审,含方法局限提示);建 frontend-ux-polish-r1 任务,子代理实施 WP1-4:移动端 44px hit-area(::after 外扩)/顶栏收纳/markdown 节奏/微字号/盘古之白/路由 fade;check 复查修 1 bug。顺手修 3 个真实 bug:DiscussionSummaryCard scoped 零间距、MessageList enter 动画失效(5b1fc81 回归)、ChatInput 移动端 0px gap。拦 2 条 VLM 误判(侧栏选中态/Tab 下划线已存在)。spec 沉淀:message-list-and-markdown.md 新增 + responsive-mobile hit-area 模式 + AGENTS.md 速查。

### Git Commits

| Hash | Message |
|------|---------|
| `e24c616` | (see git log) |

### Status

[OK] **Completed**


## Session 100: memory 指令块窗口治理:WP1 度量 + WP2 分级注入 digest 落地

**Date**: 2026-08-15
**Task**: memory 指令块窗口治理:WP1 度量 + WP2 分级注入 digest 落地
**Branch**: `feat/memory-block-governance`

### Summary

BACKLOG §3.1 治理落地:WP1 turn_trace.memory_token 列(与 tools_token 同写点/契约)+ TurnCard mem cell + turn-smoke memory 列与 --turns N;WP2 memory/digest.rs fence-aware 切节 + CLAUDE.md(reference)目录化、AGENTS.md(primary)全量、≤600 小层豁免,load_memory_sections 元工具(banner label 寻址,serial 顶部拦截,独立 stub gate),MemoryDigestRegistry OnceLock 单例粘性(免 72 个 run_chat_loop 穿参),delete_session_inner 清理,gate=开关(缺省 on fail-open)&&!worker&&!群聊(prompt.rs 一行不动,digest_off 逐字节一致单测锁)。live 实测:memory 10124→2080(-79.5%),首轮 context -47%,双轮 cache 99.8% vs off 99.7% 不劣化,Tech Stack 探针确认模型主动拉节遵循。1722 后端(1 预存 flake,main worktree 复现同挂)+1039 前端绿。坑:Bash 工具进程组会杀 daemon 子进程(setsid 无效,需 run_in_background 常驻任务承载);8-14 旧 daemon 残留占 7456 需 pid kill。spec:memory/decisions + token-usage-tracking 新 Scenario。

### Git Commits

| Hash | Message |
|------|---------|
| `be873d4 8dafc71` | (see git log) |

### Status

[OK] **Completed**

## Session 101: B1 图片支持(multimodal)全程落地

**Date**: 2026-08-16/17
**Task**: `.trellis/tasks/archive/2026-08/08-16-b1-image-multimodal`
**Branch**: `feat/b1-image-multimodal`

### Summary

议程 8 决议(粘贴+@双入口/占位降级/文件系统存储/双 adapter/token 切片/DOMPurify 收紧/群聊支持/交互细节)→ 评审独立核验(1 P0 口径修正 + P1-4 修法驳回改两级闸)→ 5 PR 实施(数据列/wire 双形态/附件存储+首个 GET 二进制路由/chat 链路+images_token/前端全套,前后端 2815 测试绿)→ live 验证(turn-smoke + 降级路径模型正确拒答)→ 收口归档。用户实测暴露两笔 hotfix(CMD_TO_DOMAIN 漏域映射;路由名不合"命令名即路径段"惯例,curl 直测掩盖)——已修并确认全链路可用。顺手:P3.3 读写不对称正式取消。教训两则:机械改 109 处字面量的脚本必须以编译器反馈闭环;live 验证必须走 transport 真实 URL 形态而非手拼 curl。

### Git Commits

| Hash | Message |
|------|---------|
| `2b647c0..0191947` | 11 commits(PR1-5 + 收口 + 归档 + 2 hotfix,见 git log) |

### Status

[OK] **Completed**(正向视觉路径待真实 vision 模型;三切片齐备,统一 token 预算表可排期)

## Session 102: D2① 跨 session 全文搜索(用户驱动 MVP)全程落地

**Date**: 2026-08-17
**Task**: `.trellis/tasks/08-17-cross-session-search`
**Branch**: `main`

### Summary

brainstorm 四决议(全局 Modal 接管 Cmd+K / 全部 project / Modal 内只读预览定位 / <3 字 LIKE 兜底)→ 外部评审 3 项独立核实全采纳(P1 GET→POST 为真 bug、P2 方案甲并补 currentCwd 置空+watcher 竞态两坑、P3 延伸修出 design §2 struct 缺 kind)→ 3 PR:① messages_fts(external-content+trigram,`UPDATE OF text` 防写放大,**docsize 守卫回填**——`COUNT(*)` 穿透/integrity-check 放行两探针实证皆废,`%_docsize` 才精确,live 1192=1192)+ db/search.rs 双路分派 + title 附带;② search_messages 6 处注册链(POST);③ SearchModal 两态 + SearchPreviewBody(MessageItem readonly 结构禁编辑菜单 + buildRunGroups 提取)+ openSessionInProject + Cmd+K 接管 + 移动端入口。1758 后端 + 1079 前端绿,clippy/fmt/build 净。live:2 字中文跨 project 命中/FTS 命中/title 倒序/project 过滤/回填精确。坑:自动标题使单查询天然双 hit(测试预期修正);浏览器后端本 session 不可用,UI 点按留用户真机验。

### Git Commits

| Hash | Message |
|------|---------|
| (见 git log) | PR1 后端 FTS + 查询层 / PR2 IPC+路由 / PR3 前端 / 收口 |

### Status

[OK] **Completed**(② Agent 驱动 search_history tool 为 follow-up;UI 视觉真机复验待用户)


### Session 102 补记:用户实测三笔 hotfix(commit 1e5e3e4)

用户 Edge 真机反馈:① Ctrl+K 与浏览器搜索冲突;② 输入后不知"搜没搜过/怎么触发"。诊断:headless Chromium 复验功能全通(29 命中渲染正常),根因是交互反馈缺失而非搜索缺陷;另抓到预存 get_home_dir 405(浏览器模式 console 污染,curl 证实 daemon 缺路由)。修复:回车立即搜(bypass debounce,IME isComposing 守卫)+ "找到 N 条命中"状态行 + 空态回显查询词;Ctrl+Shift+F 别名 + 浏览器/PWA 桌面常显 AppHeader 搜索按钮(Tauri 桌面保持隐藏);config 域补 POST /get_home_dir 路由 + oneshot 测试。headless 复验:双快捷键开 modal/Enter 状态行/空态回显/console 零错误;1081 前端测试绿。教训:静默成功(结果悄悄替换占位)在搜索类 UI 等于失败——状态行是刚需,不是装饰。

### Session 102 补记二:搜索状态机两洞(commit aa831d4)

用户二轮实测:① 输入未回车期间显示"没有找到与 \"\" 匹配"——防抖 250ms 窗口内 searching=false + hits=[] 落进空态分支,searchedQuery 还是空串;② 点 project chip 后 chips 整行消失且过滤卡死——chips 列表从"当前命中"派生,带过滤重搜后命中只剩单 project,`v-if length>1` 塌缩。修:① 补 staleQuery 派生态(searchedQuery ≠ 当前 query ⇒ 待搜索),gap 显示"回车立即搜索 xxx",有旧结果时状态行追加"回车搜索"提示;② chips 源改 availableProjects 独立 ref,仅未过滤搜索刷新,过滤重搜不塌缩。教训:从响应数据派生的 UI 控件(chips)在过滤后必然自我吞噬——控件数据源必须与被过滤的数据集解耦;状态机要给"过渡窗口"显式命名,否则每个窗口都是一个新 bug。


## Session 101: D2② search_history 全链路:后端 tool + 前端 SearchHistoryCard 双任务

**Date**: 2026-08-17
**Task**: D2② search_history 全链路:后端 tool + 前端 SearchHistoryCard 双任务
**Branch**: `main`

### Summary

D2 双驱动收官两任务同日落地。①后端 search_history tool(a005b51/8d88261):薄封装复用 db::search::search_messages(SQL/IPC/前端零改动);{query, scope, limit≤50} → 紧凑一行一 hit 文本(title 命中标注/this session 标记/零命中非 error);权限链零改动(ToolKind::Other Tier 5 + plan 保留);READONLY_TOOL_ALLOWLIST 第 6 员(researcher 硬编码 5 项有意不动);注册 +178 tok → C7D AC1 预算线二次校准 3700→3900(stub 化只省 ~140 仍超线;沉淀守则:新工具先评估扩 STUB_CANDIDATES,平移线最后手段);l3a 守卫 5→6 显式跟进;10 新单测,1768 后端绿(1 预存 subagent guard flake 隔离通过)。②前端 SearchHistoryCard(3755a98/408ca9a):用户实测 tool_result 文本坨不可读 → 专属卡片替换渲染(end_discussion 先例),自取 tool_use.input 重查 search_messages IPC 拿结构化 hits(live/replay 同路,streamController 零改动;沉淀'自取自查 vs 事件路由'边界 spec);四态机含重查失败降级;CTA 经 useSearchModal prefill 扩参开①modal 预填即搜;两真 bug:prefill 双触发(bootingPrefill guard + nextTick 清除,一次性 counter 同 query 重复 open 会吞按键故否决)+ @click 裸绑 PointerEvent 当 prefill(vue-tsc 抓到);timeLabel/splitSnippet 抽 utils/searchHits 共享。+16 前端测,1099 全绿。spec:tool-contract 15-search-history + frontend chat/search-history-card;ROADMAP D2 双驱动全勾。待用户真机验收(重编 daemon 后问历史问题看卡片+CTA)。

### Git Commits

| Hash | Message |
|------|---------|
| `a005b51` | (see git log) |
| `8d88261` | (see git log) |
| `3755a98` | (see git log) |
| `408ca9a` | (see git log) |

### Status

[OK] **Completed**


## Session 102: C3 摘要式上下文压缩 — LLM 摘要取代机械丢组 + 立项后续三任务

**Date**: 2026-08-18
**Task**: C3 摘要式上下文压缩 — LLM 摘要取代机械丢组 + 立项后续三任务
**Branch**: `feat/llm-context-compaction`

### Summary

Session summary was not supplied.

### Main Changes

- 3 PR + P1 修复:摘要行水位替换(cutoff_seq 精确折叠)/ LLM 摘要生成(handoff 模板 + 增量合并 + 熔断)/ 前端摘要行最低渲染 + TracePanel 徽标

### Git Commits

(No commits - planning session)

### Testing

- [OK] 1813 后端(1 预存 flake)+ 1099 前端

### Status

[OK] **Completed**

### Next Steps

- live 烟测(重编 daemon 后构造超线 session);后续:max-turns-softcap / manual-compact / handoff 已立项待 brainstorm


## Session 103: 手动 /compact 命令入口完成(08-18-manual-compact-command)

**Date**: 2026-08-19
**Task**: 手动 /compact 命令入口完成(08-18-manual-compact-command)
**Branch**: `main`

### Summary

三任务排序推荐后开工 compact-command。brainstorm 决议 D1-D7(命令名/focus 语法/通用直输拦截/进行中拒绝/失败零写入/熔断绕过+记账/观测走 metadata);代码探索发现 resource_loader 已预留 /compact 位、lookup_provider_for_session 可命令层复用。实现:后端 run_manual_compaction 空闲期编排(共享 send_summary_completion helper,drive.rs 同源改造;seq=MAX+1 空闲期语义;水位 prior 增量合并;失败零 DB 写入)+ compact_session 四处注册 + gate 链(群聊/config/in-flight/provider);前端 executeBuiltin 统一 palette 与直输分发(matchBuiltinCommandInput 拦截,顺带修 /help /clear /new 直输发 LLM 不一致)+ reloadSessionMessages 复用 done-reload 管线。测试:后端 manual_compaction 6 集成 + route 冒烟 + compaction 50 全绿(全量 1829 过,2 个预存 flaky 隔离通过归因);FE 1114 过 + vue-tsc 0 + clippy 0。live:turn-smoke.sh 加 --compact 模式(大消息轮撑保留区预算 + 全量 wire 续跑验证水位)——发现单条 wire 会致水位对齐 fail-open 后改全量 wire 复验:manual compaction applied 无 watermark_miss、保留区存活、摘要行契约(seq=MAX+1/trigger=manual/focus/cutoff 精确)全中。已知边界记录 spec:待压区极小时摘要净增长。spec 回写 pattern-llm-compaction §手动入口;任务已归档。

### Git Commits

| Hash | Message |
|------|---------|
| `79b7e56` | (see git log) |

### Status

[OK] **Completed**


## Session 104: MAX_TURNS 软卡化——撞线询问替代硬终断

**Date**: 2026-08-19
**Task**: MAX_TURNS 软卡化——撞线询问替代硬终断
**Branch**: `main`

### Summary

单聊主 loop 200 轮撞线由硬终断改为 QuestionStore 软卡询问(继续+200/压缩后续跑/停止),10min 无响应超时停止与今日行为等价;worker 与群聊硬卡零回归(群聊 speaker 段经 group_chat_state 门排除——实现中发现的缺口,曾致全量 30min 挂起)。压缩联动走 drive_turn force 穿参只绕 token 触发线,metadata trigger=softcap 区分。新增 TurnLimitSoftcap PendingInteraction/audit kind(无 migration)、前端浮动卡四点位、softcap.rs 8 用例;live 冒烟 PASS(boundary env 实跑弹卡→resolve 停止→audit asked→stopped)。spec 沉淀 pattern-turn-limit-softcap。全量 1836/1838(2 失败为 main 既有 flaky,基线复现无关),前端 1108/1108。

### Git Commits

| Hash | Message |
|------|---------|
| `ae2f3b4` | (see git log) |
| `df34842` | (see git log) |
| `b6fa382` | (see git log) |
| `26a3040` | (see git log) |

### Status

[OK] **Completed**


## Session 105: worker turn_trace 度量盲区闭合——run 维度唯一键 + drawer Token 明细

**Date**: 2026-08-20
**Task**: worker turn_trace 度量盲区闭合——run 维度唯一键 + drawer Token 明细
**Branch**: `main`

### Summary

unified-context-budget 留下的小额 follow-up 全程落地。研究先行发现两件事:① worker loop seq 与父后续轮共享区间,旧 UNIQUE(session_id,seq) 下开闸必互撞——run 维度必须并入唯一键(表重建迁移,空串哨兵非 NULL,显式列清单拷贝修正 widen 先例 SELECT* 不可复用点);② 机械压缩 record_compaction 与 loop 软提示 record_loop_hint 原无 worker 门,worker 撞线时以父名义污染主行(既有跨归因 bug)——同钥匙一并归位。交付:PR1 存储层(run_id 列+表重建+db 四 upsert 扩参+list_worker_turn_traces,子代理);PR2 写点开闸(Done 臂 run 行落值 usage/tools/system/window,memory/images/@文件 按契约 NULL——研究论断被实证推翻,worker 经共享 init 实际注入 memory 是 Some,写点显式归 NULL;compaction/loop_hint 旁路传 run_key;IPC 全链 Tauri+daemon+transport;集成锁 run 行落值/主行隔离/sessions.last_* 不被 worker 写);PR3 前端(SubagentDrawer「Token 明细」折叠区+runTracesByRunId 粘性缓存,running 态 expand 即 force 重拉)——子代理撞 5h 限额由主会话接手。验证:后端 1874/1875(唯一失败为基线已知负载型 flaky,失败名两轮确认+隔离过),前端 1133/1133(+11 新),vue-tsc 0,clippy stash 对照与基线逐条一致零新增,fmt 净,e2e 路由冒烟过。spec 回写三处:token-usage-tracking(worker 行语义+seq 冲突根因)/database-guidelines(表约束加宽迁移守则)/subagent-runs-schema(run 关联)。坑:describe 插错测试组致 mock 计数跨测试泄漏(纯类组无 beforeEach);python 搬移代码块丢闭合行;AC『零改动』用可选字段实现(runId?: string)比改 fixture 更符合回滚容错语义。

### Git Commits

| Hash | Message |
|------|---------|
| `a76b6b7` | (see git log) |
| `38c8e63` | (see git log) |
| `5e5999a` | (see git log) |
| `24da69e` | (see git log) |

### Status

[OK] **Completed**


## Session 106: docs 同步 08-19/20 落地——五特性文档回归

**Date**: 2026-08-20
**Task**: docs 同步 08-19/20 落地——五特性文档回归
**Branch**: `main`

### Summary

代码 08-19/20 连续落地五特性(unified-context-budget 统一预算+关卡⑤硬卡 / MAX_TURNS 软卡 / 手动 /compact / handoff 接力 / worker per-turn 度量+turn_trace 表重建),而 docs/ 全部停在 08-18,多处表述与代码现状直接冲突。按优先级回归六个文档:DEBUG_DB(turn_trace 唯一键 UNIQUE(session_id,run_id,seq)+run_id 空串哨兵+at_files/system/context_window 三列+idx_turn_trace_run+查询示例补 worker 行)、CONTEXT(MAX_TURNS 硬兜底改软卡询问语义+AuditKind 25→27+命名约定补 5 条目)、ARCHITECTURE(状态块/§1.6 补 5 续接特性+§2.5.5 触发口径改三部件加法+新增 §2.5.14 budget 硬卡/§2.5.15 softcap·compact·handoff·worker trace 两节+§2.5.8 AuditKind 列表)、DESIGN(三处 MAX_TURNS 兜底表述+C3+ 触发口径)、ROADMAP(§1.2 补 08-19/20 五行)、BACKLOG(统一预算待办标完成)。spec 侧(.trellis/spec)本就与代码同步,无需回写。坑:ROADMAP C2 06-24 历史行保留原文+加软卡指注,不改写历史;INTERLEAVED-THINKING-DESIGN 的 UNIQUE(session_id,seq) 是 messages 表约束,与 turn_trace 无关不动;ARCHITECTURE AuditKind 列表首次遗漏 27 类更新,复检 grep 兜住。验证:grep 复查无遗留旧表述(仅历史指注)、ROADMAP 表格管道数 5 列一致、git diff 6 文件 +66/-24。提交 e352704。

### Main Changes

- DEBUG_DB.md: turn_trace 表 12 + 索引 + 查询示例 #8/8b 同步 run 维度
- CONTEXT.md / ARCHITECTURE.md / DESIGN.md: MAX_TURNS 软卡 + AuditKind 27 类 + 触发口径
- ROADMAP.md §1.2 五行 + BACKLOG.md 预算待办标完成

### Git Commits

| Hash | Message |
|------|---------|
| `e352704` | (see git log) |

### Status

[OK] **Completed**

## Session 107: token-usage 用量弹层迁移——AppHeader QuotaChip 并入 input hint chip

**Date**: 2026-08-21
**Task**: `.trellis/tasks/08-21-token-usage-popover-relocate/`
**Branch**: `main`

### Summary

用户对 08-20 落的 QuotaChip UI 不满,对话内直接提改版:panel 迁到 input 下方 token chip 的弹出 popover、变大、上下文容量占用进度条 + 明细列表 + 平均缓存命中率 + 窗口聚合。落地:新 `ChatInputTokenUsage.vue`(手写 popover 族,向上开 420px,translateX(-50%) 居中于 chip——Transition transform 需合成 translateX 否则动画偏移)四分区仪表盘:上下文占用(进度条 + usageLevel 着色 + 剩余)/ 上轮明细(四计数 + cacheRatePercent 命中率)/ 滚动窗口聚合(总量 + 额度占比条 + 平均命中率 Σcr/Σ(in+cc+cr)——provider 同族不混,跨 provider 求和口径仍准 / per-provider 主-worker 拆分 + 各自命中率 + 小时柱 / top sessions 跳转)/ 设置行。HintRow 的 reka Tooltip 移除(生产 Tooltip 仅剩 MessageItemFooter),S6a 隐藏规则改 :deep(.chat-input__token);AppHeader QuotaChip 摘除删除。quota store 零改动,刷新链不变(mount/开弹层/streamEvents done)。验证:前端 1146/1146 + vue-tsc 0;真机 headless 截图(VLM 复核居中/无裁切/分区渲染)+ 移动端 390px 隐藏生效。坑:①旧 QuotaChip 用了不存在的 token(--color-status-warning/--radius-full)一直吃 fallback,新组件改正确名;②首截图弹层报 405,排查为旧 daemon 二进制早于 usage 路由提交(curl 422 vs 405 对照定位),非前端回归;③daemon.sh bg 在工具会话里被进程组回收,setsid nohup 直拉二进制绕过。spec 回写两处:popover-pattern.md(新实例 + 居中几何变体)/reka-ui-usage.md(Tooltip pattern 生产实例换主)。

### Main Changes

- `app/src/components/chat/ChatInputTokenUsage.vue` 新增(chip + 大号弹层;同日二轮加构成堆叠条 + 常显图例,traceStore 最近一轮取数,消息=残差,技能归并在 system 标注「system+技能」,共 9 测试用例)
- `app/src/components/chat/ChatInputHintRow.vue` Tooltip → 弹层组件;`app/src/components/layout/AppHeader.vue` 摘 QuotaChip
- 删除 `QuotaChip.vue` / `QuotaChip.test.ts`;`quota.ts` 注释更新
- spec:`popover-pattern.md` + `reka-ui-usage.md` 实例同步。二轮:构成可视化弃 TurnCard hover 式,常显图例(用户点名不要悬停);图例 % 各自 Math.round 可能 ≠ 100 合计属正常

### Git Commits

| Hash | Message |
|------|---------|
| (未提交) | 待用户确认后提交 |

### Status

[OK] **Completed**


## Session 107: B1 图片收尾——自动压缩/拖拽/read_file 工具读图全链落地

**Date**: 2026-08-21
**Task**: B1 图片收尾——自动压缩/拖拽/read_file 工具读图全链落地
**Branch**: `main`

### Summary

B1 三 follow-up 一次收口。brainstorm 五决议(D1 拖拽纳入/D2 压缩仅前端 canvas 零依赖/D3 长边1568+JPEG q0.85+压后判5MB/D4 拖拽只收图片/D5 缩略图呈现);探索推翻两假设(read_file 对图是裸 UTF-8 error 非占位;Anthropic tool_result 现发纯字符串未用 block array)。实现:前端 imageCompress(fail-open)+闸序压后判定+已压缩 chip+ChatPanel drop;后端 read_file 图片臂(魔数共享 helper)+ToolResultData 双形态 serde(DB=refs/wire=Anthropic block array,无图路径逐字节 fixture 锁,flatten 结构体+手动 Serialize 三分支)+wire 全链(strip caps 降级/OpenAI 占位/from_wire 还原)+estimate 内联 tokens_est+budget 臂2 双清;前端 ToolResultImages 缩略图双卡+wire history 回传 images refs(否则次轮丢图)。过程:子代理因 5h 限额双双中断改主代理直做;批量补字段脚本有 12 行窗口重复 bug,写检测器清了 28 处;live 三连败根因是误启 app/src-tauri/target 下 Aug9 陈旧二进制(pre-08-20 upsert SQL 撞新键形)——daemon 必须用根 target/release(daemon.sh 同款路径),换正确二进制后 turn-smoke --assert-turn-usage PASS。AC4 视觉 live:MiniMax-M3 read_file UI 截图准确读出标签页与侧栏文字(项目正向视觉首次实证),images_token=1728=(w×h)/750 精确入账,附件副本+GET 路由 200。验证:后端 1894 过(subagent_guard/plan_mode 两预存 flaky 隔离归因)+前端 1175 过+vue-tsc 0+clippy 零新增+fmt 净。spec 回写三处:llm-contract §Tool-Result Image Blocks/token-usage-tracking 工具图计费/tool-contract/16。压缩的 canvas 编码路径留用户下次 UI 会话手动验(逻辑已单测锁)。

### Git Commits

| Hash | Message |
|------|---------|
| `36d46de` | (see git log) |

### Status

[OK] **Completed**

## Session 108: Button CSS 家族收敛——167 个 <button>/69 文件统一 .btn 原语

**Date**: 2026-08-24
**Task**: `.trellis/tasks/archive/2026-08/08-24-btn-family-convergence/`
**Branch**: `main`

### Summary

08-24 键盘任务遗留的 UI 清单最后一项(三项候选中最大一单)。规划期子代理全量盘点(167 钮/69 文件/约 135 条规则,6 variant 可映射 ~92%),PRD/design/implement 三件套 + 单任务全量收敛、视觉保守归一两决议。实施 6 个 WP:style.css `.btn` 家族(基类 + primary/danger/danger-soft/ghost/muted/tint/outline 七变体 + sm/lg 尺寸 + pill/circle/icon 形状,焦点零声明)→ modal 簇 28 钮 → settings 簇 16 钮(顺手修 PluginSelect `--color-bg-overlay` 失效 token)→ chat 簇 54 钮(卡片批 + 重几何批,FAB 阴影/呼吸动画/弹簧按压全保留)→ 布局导航簇(hover 红实底 ×3 本地覆写)→ 长尾 8 文件 + 债清扫。终态:141 处钮级家族消费点,未迁恰为 6 个注释特例(MessageImages 瓦片/DefaultTab 表单/chip 三件套/ui-prim ×2)。净删约 1300 行 scoped 按钮样式。债:裸 rgba 红 ×6、fallback ×2、裸 radius/font/transition、disabled 五档全部清零;另清 `--color-text` 失效引用。验证:1175 测试 ×6 轮全绿 + vue-tsc 0 错 + 中间 VLM 检查点 ×2 + 终盘全量 ui-review 7 界面零渲染级缺陷 + :focus 规则零增删(键盘环完好)。坑:①WP5 子代理撞 5h 限额中断,主代理接手;②python replace 两处静默失败(样式删了模板类没挂)——「无家族类文件清单」终盘审计兜住,审计已沉淀进 AC 证据文件;③daemon 在本 harness 必须单命令内联起停(08-24 键盘任务 evidence 已有档案,setsid 也不逃)。spec 回写 design-tokens.md「Button Family」节(API + 所有权规则 + 审计命令)。生成式 ui-prim 是否消费 .btn 写回 BACKLOG 附录 A 候选待办。

### Main Changes

- `app/src/style.css`:`.btn` 家族区(~150 行,含注释体例)
- 63 个组件文件迁移(保留 BEM 类追加家族类,删家族拥有的 scoped 声明)
- `.trellis/spec/frontend/design-tokens.md`:Button Family 节
- 债清理:失效 token ×3、裸 rgba ×6、fallback ×2、disabled 五档归一

### Git Commits

| Hash | Message |
|------|---------|
| `0a1e2a0`..`727ddd7` | WP1 家族落地 → WP2 modal → WP3 settings → WP4 chat → WP5 布局 → WP6 长尾(6 提交)+ archive |

### Status

[OK] **Completed**


## Session 108: DB 备份(VACUUM INTO)+ daemon 日志轮转(P1/P2 债收口)

**Date**: 2026-08-24
**Task**: DB 备份(VACUUM INTO)+ daemon 日志轮转(P1/P2 债收口)
**Branch**: `main`

### Summary

收口 RULE-DB-001(P1)与 RULE-DAEMON-001(P2):新增 db/backup.rs(VACUUM INTO 在线快照,同秒后缀避让,prune 保 7 份;实测坑:sqlx :memory: 池上 VACUUM INTO 静默 no-op,测试须 file-backed 建池)+ daemon/server.rs spawn_backup_task(启动即备份+24h 周期,失败仅 warn);daemon.sh 日志移 ~/.local/state/dev.everlasting.app/daemon.log(>> 追加+启动 >10MiB 滚动保 3 代)。质检修两个真 bug:rotate_log 首滚 mv 报错在 set -e 下中断启动、STATE_DIR 在 HOME 缺失时 set -u 杀脚本。backup 测试 6/6,全量 1899 passed/2 failed(tests_subagent 预存 flaky,stash 对照无关)。spec 沉淀 database-guidelines(DB 快照备份 Scenario)+ daemon-server(运维伴生物 Pattern);DEBT.md 闭合两条(P1 剩 RULE-PERSIST-001)。

### Git Commits

| Hash | Message |
|------|---------|
| `5ef8ff0` | (see git log) |
| `88ec85c` | (see git log) |

### Status

[OK] **Completed**


## Session 109: turn 流式持久化 + 崩溃恢复(RULE-PERSIST-001 P1 闭合)

**Date**: 2026-08-25
**Task**: turn 流式持久化 + 崩溃恢复(RULE-PERSIST-001 P1 闭合)
**Branch**: `main`

### Summary

DEBT 最后一条 P1 全链收口。规划期研究推翻两认知:① 崩溃有双窗口——W2 工具执行中崩溃留孤儿 tool_use 行,该 session 此后每次请求 400(pair atomicity),比丢内容更重,今天零修复;② subagent_runs 已是'running 占位+启动 reap'完整先例,设计直接贴它零新机制。交付四 WP:WP1 messages.status 列(NULL/in_progress/interrupted+partial index)+ db 层 upsert/finalize/delete + recover_interrupted_messages(空占位删/有内容加 [异常中断已恢复] marker/孤儿尾合成 is_error tool_result)+ state.rs 启动 pass;WP2 drive.rs 三写点(stream ready 占位/Delta·ThinkingDelta 臂 1s 时间门只读克隆检查点/assistant 落库点独占 upsert——persist_turn 保持裸 INSERT 保 seq 漂移告警语义);WP3 前端 stream-resync 消费者+DB 死亡预言机;WP4 sse.rs 哨兵决策表。check 链抓三真问题:检查点失败重试风暴(改按 attempt 关门)、哨兵重启后根本不发(原研究'必收哨兵'结论错误,补空 buffer/跨进程陈旧高 id 两哨兵臂)、活请求被哨兵误终结(死亡预言机:末尾行 status 判死活,活请求 no-op 由 done 自愈)。验证:后端 1925+1 基线 flaky(stash 对照)/前端 1179+build/remote 89/fmt/clippy 零新增;隔离实例(--data-dir DB 副本)kill -9 真演 PASS(11s 杀,检查点 402 字→重启恢复 interrupted 413 字含 marker 零残留)+turn-smoke 过;意外收获:真实 DB 副本上 orphan_repaired=4——存量 4 个孤儿 session 下次真实 daemon 重启自动修复。坑:首演 6s 杀太早 provider 未吐 delta,改轮询检查点落库再杀;daemon 生命周期必须单命令内联。spec 回写五处:agent-loop-architecture(+pattern-turn-checkpoint)/database-guidelines(检查点 Scenario)/daemon-server(哨兵决策表)/llm-contract(崩溃孤儿修复)/frontend-transport(哨兵=oracle-trigger 非 proof)。DEBT P1 清零(剩 4 P2+1 P3)。

### Git Commits

| Hash | Message |
|------|---------|
| `3c4850b` | (see git log) |
| `76765cd` | (see git log) |

### Status

[OK] **Completed**


## Session 110: F4 web_search 工具全量落地(WP1-WP4)

**Date**: 2026-08-25
**Task**: F4 web_search 工具全量落地(WP1-WP4)
**Branch**: `main`

### Summary

web_search 工具从规划审查到全量开闸:enum dispatch 双后端(Tavily keyed/DDG 兜底)+ 30s 整体预算重试环 + STUB 第 11 员 token 线零平移;key 三态 AEAD 配置(app_config,aad=web_search)+ Tauri command/daemon route/CMD_TO_DOMAIN 四处 IPC + Settings 第 7 tab(masked 回显);开闸五面(readonly allowlist/researcher vec+prompt/群聊白名单+prompt 枚举/frontmatter 四处含项目层)配运行时断言防 builtin-only 假绿。后端 1961 passed+1 已知 flaky、前端 1187+vue-tsc 0、clippy 零新增(clippy 抓到 execute_on cfg(test) 真 bug);live 冒烟经 7457 debug daemon 实跑 DDG 搜索全链路通(attribution/审计两行);spec 落档 tool-contract/16-web-search.md + stub 校准史补记。

### Git Commits

| Hash | Message |
|------|---------|
| `7508228` | (see git log) |
| `5a3ae99` | (see git log) |
| `a9a78d2` | (see git log) |
| `eb1be36` | (see git log) |
| `a50d3f9` | (see git log) |

### Status

[OK] **Completed**


## Session 111: F1 消息队列·用户连发档落地 + 三轮评审收口归档

**Date**: 2026-08-25
**Task**: F1 消息队列·用户连发档落地 + 三轮评审收口归档
**Branch**: `main`

### Summary

F1-A 用户连发档:流式期间解锁输入,发送进后端 per-session 队列(agent/message_queue.rs),驱动器 turn 边界批量注入续轮(DriverSink 单 rid 保活 + TurnContinuation 事件);R8 撤销/退回 + Stop 清队 toast。评审三轮收口:Round 2 修 P0 DriverSink 丢事件 + R8 uuid 寻址 + 水合;收尾再修 CMD_TO_DOMAIN 三命令映射缺失与 ChatInput 旧守卫致 AC1 不可达。live 冒烟 + curl REST 排队分支归档当日真机实测通过,重审门槛 3 关闭。

### Main Changes

### Summary

F1-A 用户连发档全程落地并归档。**核心链路**:经典 session 流式期间解锁输入(原编辑器整体只读),发送进后端 per-session 内存队列(`agent/message_queue.rs`,FIFO/uuid 寻址/上限 20),`run_queue_driver` 驱动器在 turn 边界 drain 全队批量注入下一轮(每条独立 user message APPEND,cache 断点不变量保持);单 rid 跨内层轮保活靠 `DriverSink` 吞中间 Done 真退出补发 + 新 `ChatEvent::TurnContinuation` 续轮渲染边界;guard 双抑制(`skip_cancellations` 尾参,70 处调用点机械补 false)驱动器自持 slot/token 生命周期。**配套**:R8 单条撤销/退回输入框三 IPC + 视图水合;Stop/edit/resend/retry 清队返回 `clearedQueued` 驱动 toast。**评审三轮**:review-glm 两 P1 实锤采纳(闲也入队 + streamEvents 渲染缺口),review-d4f 误读驳回;Round 2 修 P0 DriverSink 丢事件(Error 分支自转发双发)+ R8 占位按 uuid 寻址 + 水合可见性;Round 3 全套验证后同日又抓两收尾 bug——transport 层 F1 三命令漏 CMD_TO_DOMAIN 映射(开 session 即 unknown cmd)与 ChatInput 两道旧守卫吞掉流式发送(AC1 物理不可达),均已修复。

### 遗留(follow-up)

- ~~review-glm 重审门槛 3:live 冒烟 + curl REST 排队分支~~ ✅ 2026-08-25 归档当日用户真机实测通过(P0 DriverSink 修复真机兜底完成)
- ARCHITECTURE/spec 深度沉淀(锁序文档化等评审亮点未进 spec)


### Git Commits

| Hash | Message |
|------|---------|
| `92a480b` | (see git log) |
| `f5fc9ba` | (see git log) |
| `d687273` | (see git log) |
| `48471e8` | (see git log) |

### Testing

- [OK] vitest 1195/1195(含 6 新增)+ vue-tsc --noEmit 零错
- [OK] cargo test --lib 1971 过 / 1 预存 flaky(plan_mode 满载,复跑过,stash 对照确认预存)
- [OK] clippy 零新增(干净 HEAD 对照)+ cargo fmt 干净

### Status

[OK] **Completed**

### Next Steps

- F1-B/C 档(优先级分档、daemon 统一入口服务化)仍开放;下一功能候选 F5 PDF/Office 阅读(无依赖、B1 通道可复用)


## Session 112: F5 PDF/docx 原生文本提取落地(第一档)

**Date**: 2026-08-26
**Task**: F5 PDF/docx 原生文本提取落地(第一档)
**Branch**: `main`

### Summary

F5 第一档(PDF+docx)全程落地:PR0 spike 判定 pdf-extract 过关(中文零乱码/扫描件 0 字符判据成立,不买 pdfium);doc_extract.rs 纯函数提取 + at_file Degraded 前分流(提取即注入,三级 cap 20MiB/32字符/150k)+ wire Extracted 变体 + 指令式自助占位文案(D3 分层)。后端 1982/前端 1201 全绿;live 冒烟 at_files_token=432 精确吻合 + 模型正确答出 PDF 标题。

### Main Changes

### Summary

F5 第一档(PDF + docx)从 brainstorm 到 live 验证全程落地。**决策链**:六决议(范围 PDF+docx / 不引 Node.js(daemon 单二进制不变量)/ 高频内置+长尾 agent 自助分层 / PDF 库 spike 闸门 / 扫描件 MVP 占位 / 三级 cap),业界对照搜证(Claude Code=平台内置 vs Codex=skills 自助)支撑分层设计。**PR0 spike(PASS)**:headless Chromium 制中文样本 + pdftotext 对照,pdf-extract 中文零乱码、英文语义等价、扫描件 0 字符 → 不买 pdfium。**PR1 后端**:`agent/doc_extract.rs` 纯函数模块(pdf-extract 0.12 + lopdf re-export 页数;docx = zip deflate-only + quick-xml 提 w:t)+ at_file Degraded 前分流 + `<doc>` marker + wire additive `Extracted{format,chars,truncated}` + 占位文案指令式升级。**PR2 前端**:FileInjectionsHint extracted 三态(格式标签/截断徽标)。坑:quick-xml 0.42 实体走独立 GeneralRef 事件(不映射则静默丢字)、zip 默认拉 zstd-sys(收紧 deflate-only)、serde tag 字段撞名(format≠kind)、cargo init 在 workspace 内自挂 members、daemon.sh start 是前台命令。

### Testing

- [OK] 后端 cargo 1982/1982(doc_extract 7 + at_file F5 集成 3 新增;首跑 1 失败为已知满载 flaky 复跑过)
- [OK] 前端 vitest 1201/1201(extracted 3 新增)+ vue-tsc 零错 + build 干净;clippy 零新增(fmt 过)
- [OK] live:turn-smoke at_files_token=432(553 字符中文 PDF 精确吻合);手动轮 manifest {"kind":"extracted","format":"pdf","chars":553} + 模型正确答出「大语言模型 Agent 系统设计白皮书」

### Next Steps

- follow-up 档:xlsx/pptx 提取(docx 管线已通,表格形态需设计)/ pdfium 渲染扫描件走 B1 通道 / 正式 document skill(B4)


### Git Commits

(No commits - planning session)

### Testing

- [OK] cargo 1982/1982 + vitest 1201/1201 + vue-tsc 零错
- [OK] live:turn-smoke at_files_token=432(553 字符中文 PDF)+ manifest extracted 变体 + 模型正确答出文档标题

### Status

[OK] **Completed**

### Next Steps

- follow-up:xlsx/pptx 提取、pdfium 扫描件渲染、document skill
- **B2 @面板缓存 bug**(2026-08-26 F5 真机验证发现,记入 task notes):同项目内新放入的文件 @ 不到 —— 先放的 docx 能找到,后 copy 进项目根的 pdf 永远不出现。根因两层缓存只进不出:`chatInputCodeMirror.ts` 的 shallowLoaded(重开面板跳过重拉)+ `ChatInput.vue` 模块级 fileCache(按 projectId),仅切项目往返/重载窗口才失效;`files.rs` 头注释「frontend re-fetches on each @ open」与实际行为不符。绕过:切项目往返或 Ctrl+R;修复方向:面板打开即重拉浅层 walk(3 层毫秒级,保留 in-flight 去重)或 fileCache 加 TTL,并同步修正后端注释。
- **P0:pdf-extract panic × panic=abort 击穿 daemon**(2026-08-26 03:25 真机验证 xxx.pdf 踩中,详见 task notes):非嵌入 STSong-Light(UniGB-UCS2-H,WPS/Word 中文导出典型形态)触发 pdf-extract 0.12 panic;release profile `panic="abort"` 令 doc_extract 的 catch_unwind 失效 → daemon 整进程 abort,无注入提示、无 LLM 请求、SSE 无 Done → 前端流永久挂起。已重启 daemon 恢复(03:35)。修复方向:patch pdf-extract 补 CMap / 按D4 预案升级 pdfium-render / panic=unwind 或子进程隔离;spike 样本需补 WPS/Word 真实导出形态。
- **UX 缺口:@文档引用视觉上像纯文本**(2026-08-26 验证发现,详见 task notes):输入框 onFileSelect 纯文本插入、CodeMirror 无 Decoration;消息气泡内 @token 无高亮,唯一标识是小号次要色的 hint 行(图片通道有缩略图,文档通道没有同级视觉元素)。数据链路完整非 bug,是 B2/F5 的设计缺口;方向:@token chip 化 + 附件卡片。
- 上述三问题(含 @面板缓存 bug)已立 follow-up 任务 **08-26-f5-verify-followups**(P1,PRD 含完整诊断链 + AC1-AC6,三问题独立成 PR、问题 1 优先)。
- **三问题当日修复完毕**(主线派发三个子代理串行实现,主线终验):① P0 = pdf-extract vendor patch(UCS2 家族码值即 Unicode 两层修:Name arm 收编 + decode_char identity 回退;未知编码降级不 panic)+ release 删 panic=abort(catch_unwind 复活,二进制 +1.52MiB);② P1 = @面板两层缓存拆除(shallowLoaded→in-flight promise + 代数守卫,fileCache 移除,system_root 例外保留);③ P2 = 输入框 chip(复用既有 cm-token-file 装饰层升级 CSS,顺修 `/` 路径正则)+ 气泡 @token 行内 code 包裹着色。终验:cargo 1983(flaky 复跑过)+ vitest 1219 + vue-tsc/build/clippy/fmt 全绿;**live 冒烟:真实 xxx.pdf(03:25 杀死 daemon 的同一输入)at_files_token=13862、模型正常回复、daemon 存活**。未 commit,改动在工作区。

## Session 113: F5 验证三问题修复收口 + 双任务归档

**Date**: 2026-08-26
**Task**: 08-26-f5-verify-followups(+ 08-26-f5-doc-reading 归档)
**Branch**: `main`

### Summary

三问题经三个子代理串行修复 + 主线终验全绿后,用户 GUI 实测又回捞一个 CJK 正则缺口,补修后双双归档(F5 本体 + follow-up)。vendored 上游 29 条预存警告以 crate 级 `[lints]` 声明式压制(path 依赖被完整 lint 而 registry 版被 --cap-lints 静默),clippy 信噪比恢复。

### Main Changes

- **P0**:pdf-extract 0.12 vendor 进 `vendor/pdf-extract/`(workspace exclude + path 依赖):UCS2 家族(Uni{GB,CNS,JIS,KS}-UCS2-{H,V},码值即 UCS-2 码位)两层修——Name arm 收编 + `decode_char` identity 回退(只修 panic 的话无 ToUnicode 字体会静默丢字);未知编码/兄弟 panic 全降级;release 删 `panic="abort"`(catch_unwind 复活,二进制 +1.52MiB/+9.2%)。
- **P1**:面板两层缓存拆除:`shallowLoaded` → in-flight promise + 代数守卫(切项目丢弃迟到写回),模块级 `fileCache` 移除,`@` 打开即重拉;`@/` system_root 保留会话缓存;`files.rs` 错误注释对齐。
- **P2**:输入框 chip(既有 `cm-token-file` 装饰层升级 CSS,顺修 `/` 路径正则)+ 气泡 @token 行内 code 包裹 + `file-ref` 着色。**CJK 回捞(用户实测)**:token 正则共三份且全 ASCII(`\w` 不匹配中文)→ pdf 有色、中文 docx 无色;抽 `FILE_TOKEN_BODY`(`\p{L}\p{N}`+`u`)单一常量三处共用,注释钉死「前端字符集不得窄于后端 `@([^\s@]+)`」。
- vendored `[lints]` 压制(unused_variables/dead_code/non_upper_case_globals/hidden_glob_reexports/mismatched_lifetime_syntaxes/too_many_arguments)。

### Testing

- [OK] cargo --lib 1983(1 失败为 tunnel 时序 flaky,隔离复跑过,与改动无交集);doc_extract 9 + at_file 35;clippy 自有代码零警告;fmt 全绿
- [OK] vitest 1221/1221(新增:doc_extract 3、面板重拉 5、chip/高亮 13、CJK 2)+ vue-tsc 零错 + build 过
- [OK] live:真实 xxx.pdf(03:25 杀死 daemon 的同一输入)经 daemon 实跑 at_files_token=13862、模型正常回复、daemon 存活;CJK 补修后 dist 重建,用户 GUI 确认

### Status

[OK] **Completed**(08-26-f5-doc-reading + 08-26-f5-verify-followups 双归档)

### Next Steps

- F5 follow-up 档仍开放:xlsx/pptx 提取、pdfium 渲染扫描件(老编码家族 GB-EUC/B5pc 等届时连带覆盖)、正式 document skill


## Session 114: 自定义 tunnel node_id/显示名 + Settings 编辑(修同 hostname 双机互踢)

**Date**: 2026-08-26
**Task**: 自定义 tunnel node_id/显示名 + Settings 编辑(修同 hostname 双机互踢)
**Branch**: `main`

### Summary

排查公司/本机 hostname 同为 carlos 导致 node_id 撞车、remote 侧互踢循环的问题;实施 08-26-custom-node-id 任务:tunnel_node_id 改为设置即优先(三级派生:自定义/fallback UUID → hostname → UUID 兜底,顺带修正注释宣称 DB 即身份但实现漂移的矛盾),新增 set_tunnel_node_id / set_tunnel_display_name 双 IPC(三态,校验失败不写库),get_remote_config 回显 nodeId/displayName,RemoteTab 节点信息区双编辑框;remote nodes 表 upsert 刷新显示名。Rust 1989 测试 + 前端 1227 测试全绿(1 个预存负载型 flaky 隔离复跑通过),spec daemon-server.md 沉淀 node_id 派生契约 + 同 hostname 撞车 gotcha Scenario。用户后续需重建 daemon 生效显示名编辑;公司机需另设 node_id。

### Git Commits

| Hash | Message |
|------|---------|
| `0eb5d89` | (see git log) |

### Status

[OK] **Completed**


## Session 115: PWA 多节点配对修复 + 配对/节点页互跳

**Date**: 2026-08-26
**Task**: PWA 多节点配对修复 + 配对/节点页互跳
**Branch**: `main`

### Summary

修复远程配对只支持单节点:手机端 token 单值存储换成 nodeId→token map(auth.ts),配对按节点累积,/nodes 按 token 逐查合并多卡片;transport(invoke/SSE/附件 URL)按选中节点解析 token,401 只修剪失效节点;legacy 单 token 由 loadNodes 惰性迁移。新增 /nodes↔/pairing 互跳入口,点卡片切节点改整页重载(各 PC 数据隔离)。服务端零改动;1264 测试全绿 + vue-tsc 干净;spec transport-and-pwa-modes.md Signal 1 更新为多 token 模型

### Git Commits

| Hash | Message |
|------|---------|
| `a0253e6` | (see git log) |
| `5860d5d` | (see git log) |
| `cb1cd63` | (see git log) |

### Status

[OK] **Completed**


## Session 116: RULE-ARGS-001 参数对象化落地 + 全库技术债盘点收编

**Date**: 2026-08-27
**Task**: RULE-ARGS-001 参数对象化落地 + 全库技术债盘点收编
**Branch**: `main`

### Summary

盘点扫描收编 11 条未入账债进 DEBT.md(P2×2:background_shell sweeper OOM 面/objectURL 泄漏;P3×9)。RULE-ARGS-001 落地:run_chat_loop 38→3 参三套件模型(ChatLoopDeps/ChatLoopRequest/CallerRole 经 from_app_state 统一构造),drive_turn 49→6、dispatch 33→4,70 测试位点 fixture 化,chat_loop 家族 too_many_arguments 豁免清零(全库 46→34),净删 ~3.2k 行。trellis-check 捕获并修复 workflow_ctx 入口快照漂移 P0(多轮 workflow 角色门会拿过期 task 状态),盲区登记 RULE-TEST-002。plan_mode 兜底 2s→15s 消除满载计时误报(基线即有竞态)。验证:clippy -D warnings 绿+全量 lib 回归+remote 89 绿+turn-smoke 实链路一轮 LLM 通过(turn_trace 台账正常)。spec signature-run-chat-loop.md 重写为三套件契约(旧拆 struct 警告按其兑现条件封档)。

### Git Commits

| Hash | Message |
|------|---------|
| `21776cb` | (see git log) |
| `abd9662` | (see git log) |
| `a6a71f7` | (see git log) |
| `8d1b529` | (see git log) |

### Status

[OK] **Completed**


## Session 117: RULE-QUEUE-001 多 drain 丢消息根治(非尾 drained 条目补持久化)

**Date**: 2026-08-29
**Task**: RULE-QUEUE-001 多 drain 丢消息根治(非尾 drained 条目补持久化)
**Branch**: `main`

### Summary

F1 队列多 drain 丢消息病灶根治:ChatLoopRequest.origin(尾条单传)替换为 drained: Vec<QueuedMessage> 全量载体(驱动器唯一非空点,其余 5 构造点空 vec);init 段尾条 persist 块前循环补写非尾条 user 行,seq 自 next_seq 连续自增,失败镜像 RULE-A-003 可见终止;带 origin/附件行随行写 metadata 信封(scheduled/attachments),手动条目零写入;FTS trigger/auto-title/skip_persist 全部复用既有契约。钉现状测试改写为根治断言 + 全 manual 对照;spec 三处收口(driver pattern/scheduled-tasks origin 链/signature 字段表),DEBT P2 归零。验证:2076 后端测试 + clippy -D warnings + fmt 全绿。

### Git Commits

| Hash | Message |
|------|---------|
| `ff54c1f` | (see git log) |

### Status

[OK] **Completed**


## Session 118: schedule_task 家族:LLM detached dispatch 收口 F2 + 外部评审甄别 + HTTP daemon E2E

**Date**: 2026-08-29
**Task**: schedule_task 家族:LLM detached dispatch 收口 F2 + 外部评审甄别 + HTTP daemon E2E
**Branch**: `main`

### Summary

ROADMAP F1/F2 点名的 follow-up 落地:LLM 调度三件套(schedule_task/status/cancel,plain dispatch + Tier 5 silent Allow,作者面分离 created_by='agent')。规划经 trellis-brainstorm 三问定案(工具面三件/权限全静默/per-project 上限 20)+ 外部模型评审甄别:7 项中 6 采纳(含 P1 群聊过滤方向写反——filter_tools_for_session_type 是从普通 chat 剥群聊专属工具的名单,加错方向会毁掉特性;群聊隔离改走 group_chat_tool_defs 白名单天然排除零改动)、1 论据证伪(P3-4 称 search_history 输出含 session id,实际只内部打标不出现在输出)。关键实施范式:ToolContext 无 AppState → 抽 pool 级核心(create_scheduled_task_in_pool / create_session_in_pool)+ 薄包装;tool 侧双 gate(kill switch 同键常量、上限 20 TOCTOU 有意接受)不进核心保用户路径零变化;C7D 扩员 11→14 + 预算线 3960→4100(实测 4031)。验证:后端 2097 / 前端 1339 / vue-tsc / clippy / fmt 全绿;新增工具内联 13 例 + 集成 6 例(execute_tool 真分发臂 + worker/群聊双态过滤 + 尾部追加序)+ Tier 5 钉住测试;live E2E(e2e.sh 留档)经 HTTP daemon 真实 LLM 两轮:create 回复原样复述服务端 task_id(tool_result 消费实证)+ status→cancel 闭环。spec 收口 tool-contract/17 + scheduled-tasks agent 作者面 scenario,ROADMAP F1/F2 行更新。遗留:daemon::tunnel remote_cancel_stops_stream_forwarding 为既有 load 型 flake(SSE ping 竞态,隔离 3 连绿,本任务未触碰 tunnel,建议另立小债)。

### Git Commits

| Hash | Message |
|------|---------|
| `6e3d2c2` | (see git log) |

### Status

[OK] **Completed**
