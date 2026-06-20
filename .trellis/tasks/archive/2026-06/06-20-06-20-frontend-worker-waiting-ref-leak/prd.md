# FT-F-003 — `workerWaiting` ref unmount 清理

> **状态**:**planning,prd 已 sync 实际代码结构**(2026-06-20 Session 53)。
> Session 51 原稿方案 A(clearTimeout)基于错误假设(以为是嵌套 setTimeout chain),
> 实际是 `async while loop + await new Promise(r => setTimeout(r, 300))`。已重选为 **unmounted flag 守卫**。
> 下一步:curate jsonl(Phase 1.3)→ `task.py start` → Phase 2。
>
> **Tracking**:`.trellis/reviews/DEBT.md` §Feature Follow-ups / FT-F-003
>
> **Origin**:Session 49 B6 PR3b "Next Steps" 段(commit `186e500`,自查发现)

---

## Goal(一句话)

`ToolCallCard.openSubagentDrawer` 的 retry polling(`async while loop + await setTimeout`)在 component unmount 时不会跳出,`await` resolve 后继续写 unmounted `workerWaiting` ref + 可能 `openDrawer` on unmounted card。加 unmounted flag 守卫,unmount 后 loop 跳出。

---

## 现状(实际代码,`ToolCallCard.vue:328-372`,Session 53 实读)

```ts
const workerWaiting = ref(false);
async function openSubagentDrawer(): Promise<void> {
  const immediate = workerSummary.value;
  if (immediate) { await subagentRuns.openDrawer(immediate.id); return; }
  workerWaiting.value = true;
  if (sid) await subagentRuns.fetchForSession(sid);
  const afterRetry = workerSummary.value;
  if (afterRetry) { workerWaiting.value = false; await subagentRuns.openDrawer(afterRetry.id); return; }
  // ⬇️ leak 点:这个 while loop
  const start = Date.now();
  while (Date.now() - start < 1500) {
    await new Promise((r) => setTimeout(r, 300));   // await,无 timer id 可 clearTimeout
    if (sid) await subagentRuns.fetchForSession(sid);
    const s = workerSummary.value;
    if (s) {
      workerWaiting.value = false;                  // ⬅️ 写 unmounted ref → Vue warning
      await subagentRuns.openDrawer(s.id);          // ⬅️ openDrawer on unmounted card
      return;
    }
  }
  workerWaiting.value = false;                       // ⬅️ 同上
}
```

| 项 | 实际 |
|---|---|
| retry polling 实现 | `async while loop` + `await new Promise(r => setTimeout(r, 300))`,**非**嵌套 setTimeout chain |
| unmount 行为 | loop 不检查 component 是否 unmount;`await` resolve 后继续执行后续语句 |
| Vue 3 表现 | 写 unmounted ref → console warning(版本相关,implement 阶段实测 Vue 3.5+ 是否仍出) |
| 实际影响 | 控制台 warning 噪音 + ghost ref;非功能性,prod 用户不可见 |
| 现有测试 | `ToolCallCard.test.ts` 覆盖 click 路径,**未覆盖** unmount-while-polling |

---

## Decision(2026-06-20 Session 53):unmounted flag 守卫

**Context**:Session 51 原稿候选 A(clearTimeout)假设是嵌套 setTimeout chain,但实际是 await loop,无 timer id 可 clear。

**Decision**:**unmounted flag 守卫**(方案 A')—— `onUnmounted` 设 flag,while loop 在 `await` 后 + 任何副作用前(写 `workerWaiting` / 调 `openDrawer` / `fetchForSession`)检查 flag 跳出。

**为什么排除其他**:
- ❌ clearTimeout(delay helper 重构):只为 cancel 1 个 await 的 timer 要重构 loop 结构,over-engineering
- ❌ AbortController:跟 flag 本质一样多一层抽象,本场景无 fetch 可 cancel,略重
- ❌ composable 抽出(useWorkerDrawerPolling):只有 1 个调用点,over-engineering(Q3)

**实现**(~8 行):
```ts
let unmounted = false;
onUnmounted(() => { unmounted = true; });

async function openSubagentDrawer(): Promise<void> {
  // ... immediate / afterRetry 分支不变 ...
  workerWaiting.value = true;
  const start = Date.now();
  while (Date.now() - start < 1500) {
    await new Promise((r) => setTimeout(r, 300));
    if (unmounted) return;                 // ⬅️ unmount 后跳出,不写 ref / 不开 drawer
    if (sid) await subagentRuns.fetchForSession(sid);
    if (unmounted) return;                 // fetch 后再检查一次(fetch 期间可能 unmount)
    const s = workerSummary.value;
    if (s) {
      workerWaiting.value = false;
      await subagentRuns.openDrawer(s.id);
      return;
    }
  }
  if (unmounted) return;
  workerWaiting.value = false;
}
```

**守卫点**:每个 `await` 之后 + 每次写 `workerWaiting` / 调 `openDrawer` 之前都检查。await 是让出点,unmount 只能在 await 期间发生,所以 await 后立即检查是充分的最小守卫集。

---

## Requirements

- **R1**:`ToolCallCard.vue` 加 `let unmounted = false` + `onUnmounted(() => { unmounted = true; })`
- **R2**:`openSubagentDrawer` 的 retry while loop 内,每个 `await` 之后 + 写 `workerWaiting` / 调 `openDrawer` 之前检查 `if (unmounted) return`
- **R3**:不改动 retry polling 策略本身(300ms interval / 1500ms window / 5 ticks 不变);immediate / afterRetry 分支不变(它们在 polling 前 return,unmount race 概率极低,但若保守也可在 immediate/afterRetry 的 openDrawer 前加守卫 —— implement 判断)
- **R4**:新增回归 test `unmount_during_polling_clears_loop` —— mount dispatch_subagent card + click(触发 polling)+ 立即 unmount + 用 `vi.useFakeTimers` 推进 300ms tick → 断言 loop 跳出、`workerWaiting` 不被写、`openDrawer` 不被调

---

## Acceptance Criteria

- [ ] **AC1**:`ToolCallCard.vue` 有 `onUnmounted` 清理(设 unmounted flag)
- [ ] **AC2**:retry while loop 内 `await` 后 + 副作用前有 `if (unmounted) return` 守卫
- [ ] **AC3**:`pnpm vitest run src/components/chat/ToolCallCard.test.ts` → 现有 14 test + 新增 unmount test 全 pass
- [ ] **AC4**:`pnpm vue-tsc --noEmit` → 0 error
- [ ] **AC5**:`pnpm vitest run` 全集 → ≥ 基线 278(Session 53 后)
- [ ] **AC6**(可选,implement 实测):Vue 3.5+ 在 unmounted ref 上写值是否仍出 warning;若仍出,确认修复后该 test 场景不再出 warning(`vi.spyOn(console, "warn")` 断言)

---

## Non-goals(明确不做)

- 不动 retry polling 策略本身(300ms / 1500ms / 5 ticks)
- 不抽 composable(Q3 决定,1 调用点 over-engineering)
- 不做 React-style cleanup hook 抽象
- 不做性能 profiling(5 tick × 1.5s × 几字节 = 噪声级)

---

## 启动 checklist(进 `task.py start` 前)

- [x] 实读 `ToolCallCard.vue:328-372` 确认实际是 await loop(Session 53)
- [x] 决定修法 = unmounted flag 守卫(用户确认,Session 53)
- [x] 确定测试范围 = unmount_during_polling 回归 test(R4)
- [ ] **Phase 1.3:curate `implement.jsonl` + `check.jsonl`**(当前 seed,待 curate)
- [ ] `task.py start` 进 Phase 2

---

## 关联

- **DEBT.md**:`.trellis/reviews/DEBT.md` §FT-F-003(open)
- **journal**:`.trellis/workspace/Carlos-home/journal-2.md` Session 49 "Next Steps"
- **B6 PR3b commit**:`186e500`
- **关键文件**:`app/src/components/chat/ToolCallCard.vue:328-372`(`openSubagentDrawer` + retry polling)
- **同源 follow-up**:FT-F-001(closed)/ FT-F-002(toast fallback)/ FT-F-004(UX polish)
- **memory 坑**:`subagentdrawer-banner-test-gotchas.md` 的 `vi.useFakeTimers` 影响 Date 解析坑 —— 本 task test 用 fake timer 推进 300ms,注意 Date.now() 解析
