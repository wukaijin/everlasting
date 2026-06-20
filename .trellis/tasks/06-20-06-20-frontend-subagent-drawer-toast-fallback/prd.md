# FT-F-002 — SubagentDrawer toolTip 1.5s miss 后 toast fallback

> **状态**:**planning — 等待 drawer 真实使用反馈**(B6 PR3b 已 race fix,toast 提示是 UX 增强,非功能性 bug)
>
> **Tracking**:`.trellis/reviews/DEBT.md` §Feature Follow-ups / FT-F-002
>
> **Origin**:Session 49 B6 PR3b "Next Steps" 段(commit `186e500`)

---

## Goal(一句话)

`ToolCallCard.openSubagentDrawer` 在 1.5s / 5 retry polling 后**仍 cache miss** 时,目前 silent 回退到默认 `点击查看 worker 详情` 视觉(零反馈)。改为 toast 提示用户手动 retry,或解释 cache miss 原因(IPC race / network / 后端 worker 还没 insert_run 等)。

---

## 现状(B6 PR3b 收尾时)

| 项 | 状态 |
|---|---|
| retry polling 行为 | `1.5s 总预算 / 300ms 间隔 / 最多 5 次`(D2 决策) |
| 5 次 polling 仍 miss 后 | silent 回退:UI 退回 `点击查看 worker 详情` 默认视觉,**无任何 toast / banner 提示** |
| 用户体验 | 用户 click 卡片 → 看到 waiting 状态 → 1.5s 后 waiting 消失 → 看到默认静态视觉 → 不确定"是我点错了?" "worker 死了?" "网络挂了?" |
| 现有测试 | `SubagentDrawer.test.ts` + `ToolCallCard.test.ts` 已覆盖 waiting 显示 + 命中,未覆盖 5 miss 后的 silent 回退路径 |

---

## Why deferred(为什么不在 B6 PR3b 做)

| 理由 | 细节 |
|---|---|
| **UX 增强非 bug** | 当前 silent 回退不算 broken,只是"用户不知情"。fix race 已是 B6 PR3b 核心,加 toast 提示会再扩 scope |
| **缺真实 miss 频率** | 1.5s 仍 miss 实际触发频率未知(可能 race 修好后就几乎不触发了) |
| **toast 方案未定** | 用现成 toast 组件?还是新做 banner?还是 inline warning?需先评估 UI 风格 |

---

## Open Questions(待 brainstorm 阶段回答)

1. **5 miss 后的 UI 反馈形式**:
   - 复用项目里现有 toast 组件(若有)? / 新做 `ToastService`?
   - 还是 inline warning(在卡片内显示"无法连接到 worker,点击重试")?
2. **toast 文案**:
   - "无法打开 worker 详情,请稍后重试" / "Worker 启动失败,点击查看 session 日志" / "网络异常,请检查连接"?
   - 是否暴露原因(internal error vs race vs network)?
3. **toast 自动消失 vs sticky**:
   - race-induced miss → 自动消失(toast 不打扰)
   - 后端 worker 真的没启动 → sticky(需要用户主动 dismiss)
   - 怎么区分?后端 emit error event 吗?
4. **retry button**:toast 上提供"再试一次"按钮,还是让用户重新 click 卡片?
5. **同 session 多个 dispatch_subagent 卡片**:如果 session 整体 IPC 挂了(后端 subagent:event listener 没起来),所有 dispatch_subagent 卡片都触发 toast,UX 怎么收口?

---

## 阻塞(进入 in_progress 之前需满足)

- [ ] B6 PR3b race fix 上线后 1.5s miss 实际频率统计(日志 / 用户反馈)
- [ ] 决定 UI 反馈形式(Q1)
- [ ] 决定 toast 文案 / 形式(Q2-Q4)
- [ ] 决定 session 级 IPC 整体挂掉的收口方案(Q5)

---

## Non-goals(明确不做)

- **不动 retry polling 策略本身**(1.5s / 300ms / 5 次已定)
- **不做后端 error 事件**(后端当前不区分 race miss 和 worker 启动失败,本 task 不引入)
- **不做 IPC 健康检查**(整体 IPC 挂掉收口是更宽的问题,本 task 只解决"5 miss 后无声"这一格)

---

## 启动 checklist(进入 in_progress 时)

- [ ] 走 `trellis-brainstorm` skill 讨论 Open Questions 5 项
- [ ] 真实 miss 频率数据
- [ ] 决定 UI 反馈形式(独立 component / inline warning / 复用现有 toast)
- [ ] 同步更新本 prd.md,把占位段全替换为 brainstorm 产物

---

## 关联

- **DEBT.md**:`.trellis/reviews/DEBT.md` §FT-F-002(open,即将添加)
- **journal**:`.trellis/workspace/Carlos-home/journal-2.md` Session 49 "Next Steps" 段
- **B6 PR3b commit**:`186e500`(race fix 内已留 retry polling 入口,本 task 消费之)
- **同源 follow-up**:
  - FT-F-001(drawer typed-cards 重做)
  - FT-F-003(`workerWaiting` ref unmount 清理)
