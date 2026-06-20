# FT-F-002 — SubagentDrawer 1.5s miss 后 inline 提示(原 toast fallback)

> **状态**:**planning → grill 完成(2026-06-21),待 `task.py start` 进 in_progress**
>
> **Tracking**:`.trellis/reviews/DEBT.md` §FT-F-002
>
> **Origin**:Session 49 B6 PR3b "Next Steps"(commit `186e500`)
>
> **Grill**:2026-06-21 Session 54 grill 拷问,前提被动摇,方案从 toast fallback 收窄为 inline 提示
>
> **范围**:`ToolCallCard.vue` 单文件,< 20 行改动

---

## Goal(一句话)

`ToolCallCard.openSubagentDrawer` 在 1.5s retry polling 后仍 cache miss 时,加一行 inline 提示"⚠ worker 未响应,点此重试"(卡片第三态),替代现状的 silent 回退到默认视觉。

---

## ⚠️ prd 校准(grill 揭示的前提动摇)

原 prd 前提"race fix 后 miss 可能几乎不触发"**因果搞反**:

1. **retry polling(步骤 2+3,B6 PR3b)本身就是 IPC race 的吸收层** —— 1.5s 窗口已覆盖 race。FT-F-003(unmount guard)修的是 ref leak,**不影响 miss 频率**。所以"race fix → miss 减少"假设不成立。
2. **1.5s 后仍 miss ≠ race,是真实故障**(worker 没发首 event / DB insert failed / IPC 挂)。这些该后端修,前端 toast 是创可贴。
3. **现状不是 silent 死锁**:ToolCallCard.vue:412 注释"回退默认视觉,让用户重新点击"。已有隐式反馈(视觉变化 + 可重试),FT-F-002 只是变显式"失败"提示。

**grill 决议**:不 drop(用户偶尔遇到 miss + 想要显式反馈),但**收窄为最小 inline 提示**(drop toast 组件 / ToastService / session 级 banner —— over-engineer)。

---

## 方案(grill 决议)

**形态**:`ToolCallCard.vue` 加 `workerMissed` ref,卡片视觉从两态(waiting/default)扩为**三态**:

| 状态 | 条件 | 视觉(template:553) |
|---|---|---|
| default | !workerWaiting && !workerMissed | 点击查看 worker 详情 |
| waiting | workerWaiting | 等待 worker 注册… |
| **missed**(新) | workerMissed | ⚠ worker 未响应,点此重试 |

**改动点**:
1. 加 `workerMissed = ref(false)`
2. `openSubagentDrawer` 开头:`workerMissed = false`(retry 时清除旧 missed 态)
3. `openSubagentDrawer` line 418 miss 路径:`workerWaiting=false` → 追加 `workerMissed=true`
4. `openSubagentDrawer` hit 路径(immediate/afterRetry/poll 命中):`workerMissed=false`(drawer 打开,清除 missed)
5. template line 553:三态分支,missed 态加 warn icon + 文案

**文案**:`⚠ worker 未响应,点此重试`(中性,不猜原因 —— 前端无法区分 race/network/worker 没启动,prd Non-goal 不做后端 error 事件)

**icon**:`warn`(ExclamationTriangleIcon,registry 已有,FT-F-005 用过,零新增)

**retry**:复用卡片 `@click`(template:450,`openSubagentDrawer`)。不加独立 button —— 卡片本身已可点击重试,加 button = 两个重试入口重复。

**生命周期**:missed 是组件 ref,切 session(unmount 重建)自动重置为 default。per-card 各自反馈 —— session 级 IPC 挂时每个卡片各自显示 missed,可接受(drop prd Q5 session 级 banner 收口)。

---

## Open Questions 决议(grill)

| 原 Q | 决议 |
|---|---|
| Q1 UI 形式(toast/ToastService/inline) | **inline**(卡片内一行,drop toast 组件) |
| Q2 文案/原因暴露 | "worker 未响应,点此重试",**不暴露原因**(前端无法确认) |
| Q3 自动消失 vs sticky | inline 自然 sticky(显示到重试命中或切 session) |
| Q4 retry button | **复用卡片 @click**,不加 button |
| Q5 session 级 IPC 挂收口 | **drop**,per-card 各自反馈 |

---

## Non-goals(明确不做)

- 不动 retry polling 策略(1.5s/300ms/5 次已定)
- 不做后端 error 事件(后端不区分 race/worker 启动失败)
- 不做 IPC 健康检查(session 级收口是更宽问题)
- **不做 toast 组件 / ToastService**(grill 收窄为 inline)
- **不做 session 级 banner**(per-card 已够)
- **不做 retry 次数限制 / 多次失败升级提示**(scope 膨胀)
- 不暴露 miss 原因(前端无法确认)

---

## 测试范围

- `ToolCallCard.test.ts` 新增:5 miss 后 `workerMissed=true` → 显示"worker 未响应"文案 + warn;retry(再 click)→ `workerMissed` 重置 + 重新 polling;hit 路径清除 `workerMissed`
- 既有 ToolCallCard waiting/hit 测试不破

---

## 启动 checklist

- [x] grill 拷问 ROI + 5 Open Questions(2026-06-21 Session 54)
- [x] 决定方向:inline 提示(drop toast)
- [x] 更新 prd.md
- [ ] Phase 1.3 curate `implement.jsonl` + `check.jsonl`
- [ ] `task.py start` 进 in_progress

---

## 关联

- **DEBT.md**:§FT-F-002(open)
- **关键文件**:`app/src/components/chat/ToolCallCard.vue`(`openSubagentDrawer` line 354-419,template line 553)
- **测试**:`app/src/components/chat/ToolCallCard.test.ts`
- **retry polling origin**:B6 PR3b `186e500`
- **race fix**:FT-F-003 `272fbe9`(unmount guard,不影响 miss 频率)
- **同源 family**:FT-F-001/003/004/005(均 closed)
