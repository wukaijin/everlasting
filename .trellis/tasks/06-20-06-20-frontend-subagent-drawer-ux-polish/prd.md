# FT-F-004 — SubagentDrawer UX polish bundle (C1+C2+C3+C5)

> **状态**:**planning** — 4 项独立小 UX 优化打包,等真实使用反馈 / 调度排期
>
> **Tracking**:`.trellis/reviews/DEBT.md` §Feature Follow-ups / FT-F-004
>
> **Origin**:2026-06-20 截图分析(Session 50,user 主动提供主面板 + drawer 截图)
>
> **不依赖**:FT-F-001 typed-cards(D1-D7 决策);不依赖后端改动;不依赖 drawer 重构
>
> **可独立 PR**:每项 ~10-30 行改动,4 项可拆 4 个 micro-PR 或 1 个 bundle PR

---

## Goal(一句话)

把 2026-06-20 截图分析暴露的 4 个 drawer 独立 UX 优化(C1 宽度 / C2 时间格式 / C3 事件计数 / C5 scroll 提示)打包成一个 task,统一排期统一 PR,跟 FT-F-001 typed-cards 完全解耦。

---

## 现状(2026-06-20 截图观察)

| ID | 问题 | 影响 |
|---|---|---|
| **C1** | drawer 480px 太窄,`path` 长字符串 wrap 截断(JSON 里 `path: "/usr/local/code/lejing/precaution/.../pattern.md"` 整行 wrap 成 3-4 行,call/result 配对视觉难读) | drawer 内容 50% 行被 wrap 占,scroll 量翻倍 |
| **C2** | drawer 头部 finishedAt 是 raw ISO8601 字符串 `2026-06-20T05:39:05.053532364+00:00` | 头部 80px 宽被时间戳占,summary 被挤换行 |
| **C3** | drawer 没"事件 N / M" 进度显示 | 打开 drawer 不知道 top 还是 bottom,scroll 时无定位感 |
| **C5** | drawer 底部没 scroll 渐变提示(可滚但看不出) | 用户不知道 body 内容还能滚 |

---

## 4 项优化方案

### C1 — drawer 宽度调整

**改法**:
- `SubagentDrawer.vue:450` `width: min(480px, 90vw)` → `width: min(640px, 90vw)`
- `<pre>` payload `word-break: break-all` → `overflow-x: auto`(允许横向 scroll,不强制 wrap)
- 估算 ~5 行

**取舍**:
- 加宽到 640px 后,drawer 占主 chat 区 40%(在 1600px 屏上),仍可接受
- 横向 scroll 对 path 类长字符串更友好(JSON tree 结构不破坏)
- 不加到 720px+(挡太多 chat 区)

### C2 — drawer 头部时间格式化

**改法**:
- `formatTime(iso: string)` helper:ISO8601 → `HH:MM:SS` 显示
- 头部 meta 行:raw ISO8601 → `开始: 05:38:54` / `结束: 05:39:05`
- 估算 ~15 行(加 helper + 替换)

**取舍**:
- 丢日期(同 session 内多次 drawer 打开不会跨日,可接受)
- 不显示毫秒(精度无意义,人类读 5:39:05 就够)
- 不做相对时间(`2 分钟前`)(drawer 内时间精度更重要,绝对时间更直)

### C3 — drawer 事件计数

**改法**:
- drawer 头部 status badge 旁加 `<span> · {{ visibleTranscript.length }} events</span>`
- 默认 `tool_call + tool_result + permission_ask` 计数(`visibleTranscript` 已计算)
- 勾 "Show chat events" 时加 `+ N chat` 副计数
- 估算 ~5 行

### C5 — drawer 底部 scroll 渐变

**改法**:
- `.subagent-drawer__body` 加 `mask-image: linear-gradient(to bottom, transparent 0, black 8px, black calc(100% - 8px), transparent 100%)`(顶部 8px + 底部 8px 渐变)
- 仅在 body 可滚时生效(`mask-image` 自动 only-render-when-overflowed,无需 JS)
- 估算 ~3 行 CSS

---

## Why deferred(为什么不在 B6 PR3b 做)

| 理由 | 细节 |
|---|---|
| **PR3b scope 已是 race fix + 3 polish** | 加这 4 项 → scope 爆 |
| **缺真实使用反馈** | 截图是 2026-06-20 第一次真实 use case,4 个问题在 PR3b review 阶段都没人提(因为没人用过) |
| **独立小改,可独立 PR** | 4 项都不依赖其他,等真正要做时挑一项起 30 行 micro-PR 即可 |

---

## Open Questions(待 brainstorm 阶段答)

1. **C1 宽度具体值**:560 / 600 / 640?(推荐 640,平衡 chat 区遮挡)
2. **C2 时间格式**:`HH:MM:SS`(绝对)/ `X 分钟前`(相对)/ 两种并存?
3. **C3 计数 vs 进度条**:纯数字 / 进度条 / 数字 + 小进度条?
4. **C5 渐变方向**:仅底部 / 仅顶部 / 上下都有(本 task 默认上下都有)
5. **打包 vs 拆 PR**:4 项 1 个 bundle PR / 4 个 micro-PR?

---

## 阻塞(进入 in_progress 之前需满足)

- [ ] 答 Open Questions 5 项
- [ ] 决定打包 / 拆 PR 策略

---

## Non-goals(明确不做)

- **不动 drawer 内容渲染**(FT-F-001 typed-cards 范围,本 task 只调样式 / 文案)
- **不动 drawer header 文字以外的内容**(summary / status badge)
- **不动 drawer 行为**(race / waiting / retry polling —— FT-F-002/003 范围)
- **不做 drawer 全屏模式**(留作未来 PM 决策)
- **不做 drawer 宽度用户可调**(SettingsModal 加 slider 留独立 task)

---

## 启动 checklist(进入 in_progress 时)

- [ ] 走 `trellis-brainstorm` skill 答 Open Questions 5 项
- [ ] 决定 bundle vs micro-PR
- [ ] 同步更新本 prd.md 把占位段全替换为 brainstorm 产物
- [ ] Phase 1.3 curate `implement.jsonl` + `check.jsonl`(workflow.md 要求)

---

## 关联

- **DEBT.md**:`.trellis/reviews/DEBT.md` §FT-F-004(open,即将添加)
- **截图分析**:`Session 50 in-progress`(本回合)
- **关键文件**:`app/src/components/chat/SubagentDrawer.vue`(681 行,改动预计 < 50 行)
- **不依赖**:FT-F-001 / FT-F-002 / FT-F-003 / FT-F-005(独立 task)
- **同源 family**:
  - FT-F-005(drawer failed state banner,D2)
  - FT-F-001(typed-cards,blocked by PR1)
  - FT-F-002(toast fallback)
  - FT-F-003(workerWaiting ref leak)
