# FT-F-003 — `workerWaiting` ref unmount 清理

> **状态**:**planning — 阻塞 B6 PR3b race fix 上线 + 实际 leak 频率观测**
>
> **Tracking**:`.trellis/reviews/DEBT.md` §Feature Follow-ups / FT-F-003
>
> **Origin**:Session 49 B6 PR3b "Next Steps" 段(commit `186e500`,自查发现)

---

## Goal(一句话)

`ToolCallCard` retry polling 的 `setTimeout` chain 在 component unmount 时未清理,可能 fire 后写 unmounted `workerWaiting` ref。非功能性 leak(prod 路径影响小),但 Vue 3 reactivity 在 unmounted ref 上写值有 [warning 噪音 + 调试误导] 风险。

---

## 现状(B6 PR3b 收尾时)

| 项 | 状态 |
|---|---|
| retry polling 实现 | `setTimeout` chain(300ms × 5 次)from `ToolCallCard.openSubagentDrawer` |
| unmount 行为 | `setTimeout` callback 仅检查 `workerWaiting.value`,**不**检查 component 是否 unmount |
| Vue 3 表现 | 写 unmounted ref → Vue 内部仍接受写(无 throw),但 console 出 warning(`Write operation failed: computed value is readonly` / `setup() returned a ref that is destroyed`) |
| 实际 leak 量 | 1 click 失败 = 5 个孤儿 `setTimeout` × 300ms = 最长 1.5s 内存占用,callback body 极小(几个属性赋值) |
| 用户可观测影响 | 控制台 warning 噪音 / DevTools React DevTools-like 工具看到 ghost ref |
| 现有测试 | `ToolCallCard.test.ts` 已覆盖 click 路径,未覆盖 unmount-while-polling 路径 |

---

## Why deferred(为什么不在 B6 PR3b 做)

| 理由 | 细节 |
|---|---|
| **非功能性 leak** | prod 路径无 user-visible 后果,纯 console warning + 内存短暂占用 |
| **修法单调** | 加 `onUnmounted(() => clearTimeout(...))` + tracking 当前 timer id,~5 行 |
| **缺回归测试基线** | 当前 unmount 路径无测试,需先确认 unmount 不会中断正常 polling 命中 |

---

## 候选修法(待 brainstorm 阶段选)

### 方案 A:onUnmounted + clearTimeout
```ts
let pollTimer: ReturnType<typeof setTimeout> | null = null
// 在 retry 块内:
pollTimer = setTimeout(() => { ... }, 300)
onUnmounted(() => { if (pollTimer) clearTimeout(pollTimer) })
```
- 优:Vue 3 idiomatic,~5 行
- 缺:多个并行 timer 要 tracking list(本场景是 1 个,可接受)

### 方案 B:把 polling 抽到 composable(useWorkerDrawerPolling)
```ts
const { waiting, startPolling, stopPolling } = useWorkerDrawerPolling()
// onUnmounted 自动调 stopPolling(在 composable 内部)
```
- 优:可测试性提升(纯函数),复用给其他 draw trigger
- 缺:over-engineering?本场景只有一个调用点

### 方案 C:AbortController + setTimeout 包装
```ts
const ac = new AbortController()
scheduleNext(ac.signal)
onUnmounted(() => ac.abort())
```
- 优:与后端 fetch cancellation 一致(已用 AbortController 模式)
- 缺:本场景是纯 setTimeout,不上 fetch,Ac 略重

---

## Open Questions(待 brainstorm 阶段回答)

1. **方案 A / B / C 选哪个**?(推荐 A:最简单,scope 最小)
2. **测试覆盖**:unmount-while-polling 测试要 verify 什么?
   - console 不出 warning?
   - ghost ref 不被写?
   - 内存不增长?
3. **多 timer 场景**:如果未来 retry 改成"多 key 并行 retry",是否值得提前抽 composable(Q2 = 选 B 方案)?
4. **DevTools warning 噪音**:Vue 3.5+ 是否还会出 warning?(需实测当前版本)

---

## 阻塞(进入 in_progress 之前需满足)

- [ ] Vue 3 当前版本下,写 unmounted ref 是否仍出 warning(实测)
- [ ] 决定方案 A / B / C
- [ ] 决定测试覆盖范围

---

## Non-goals(明确不做)

- **不动 retry polling 策略本身**
- **不做 React-style cleanup hook 抽象**(非 React,Vue 3 idiomatic 优先)
- **不做性能 profiling**(5 timer × 1.5s × 几字节 = 噪声级)

---

## 启动 checklist(进入 in_progress 时)

- [ ] 实测 Vue 3.5+ 在 unmounted ref 上写值的实际行为
- [ ] 决定 A / B / C 方案
- [ ] 写 `unmount_during_polling_clears_timer` 测试
- [ ] 同步更新本 prd.md,把占位段全替换为 brainstorm 产物

---

## 关联

- **DEBT.md**:`.trellis/reviews/DEBT.md` §FT-F-003(open,即将添加)
- **journal**:`.trellis/workspace/Carlos-home/journal-2.md` Session 49 "Next Steps" 段
- **B6 PR3b commit**:`186e500`
- **关键文件**:`app/src/components/chat/ToolCallCard.vue:300-380`(`openSubagentDrawer` + retry polling 入口)
- **同源 follow-up**:
  - FT-F-001(drawer typed-cards 重做)
  - FT-F-002(toast fallback)
