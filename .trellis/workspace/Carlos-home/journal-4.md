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
