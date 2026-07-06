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
