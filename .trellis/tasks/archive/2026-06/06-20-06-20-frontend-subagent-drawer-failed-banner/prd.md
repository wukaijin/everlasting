# FT-F-005 — SubagentDrawer failed / cancelled 状态 banner

> **状态**:brainstorming → 准备 start(B1+B2 hotfix 阻塞已闭环)
>
> **Tracking**:`.trellis/reviews/DEBT.md` §Feature Follow-ups / FT-F-005
>
> **Origin**:2026-06-20 截图分析(Session 50,user 主动提供截图)
>
> **不依赖**:FT-F-001 typed-cards / FT-F-004 polish / 后端改动 / B1+B2 hotfix(已闭环)

---

## Goal(一句话)

`SubagentDrawer` 在 worker `failed` / `cancelled` 状态时,在 status badge 下方加一条 **inline warning 横条**,显示失败原因(`error` 时显示 backend `summary` 字段的错误文本,`cancelled` 时显示通用 "stopped by user" 文案)。让用户打开 drawer 后一眼能区分"是因为 X 失败" vs "还在跑" vs "正常完成",不再需要 scroll 到 transcript 最底自己判断。

---

## 现状(2026-06-20 截图观察,已含 B1 修复)

修复后(B1 commit `8c8ae47` 已合),`failed` 状态 drawer 头部:
- 红色 badge "failed at 11.9s"(正确数,非 14281.9s)
- worker name "researcher"
- startedAt / finishedAt(无失败原因)
- summary:"发现 node_modules 下有大量 .md 文件..."

**问题**(本 task 要解决的):
- 用户读不到"为什么失败"——是 timeout?是 tool error?是 cancelled by user?
- summary 是 worker 跑的内容(成功/失败都填),不能作为失败指标
- 主面板 dispatch_subagent 卡有清晰红框 + "× error 11.9s" + summary,drawer 端没对齐

---

## Decisions(2026-06-20 brainstorm 答完)

| # | 决策点 | 选择 | 备注 |
|---|---|---|---|
| **D1** | 后端是否需加字段 | **不改** | `summary` 字段已经是错误文本(`format_dispatch_result` 在 subagent.rs:968-976 直接写)。后端零改动。 |
| **D2** | Banner 形态 | **A: inline warning 横条** | 红色背景 8% alpha + 左 3px 红条 + ⚠ icon + 文案。always 展开(失败信息不该藏)。 |
| **D3** | Cancelled 也显 banner | **是** | failed + cancelled 都显 banner(共用样式,文案不同)。 |
| **D4** | Banner 文案来源 | **summary 字段 + 备用文案** | error 状态用 `summary` 字段(>80 字符 truncate + "…");空 summary fallback "Worker exited unexpectedly at X.Xs"。cancelled 状态:通用 "Worker stopped by user at X.Xs"。 |
| **D5** | Banner / badge 关系 | **共存** | badge 仍显 "failed at Ns"(时间事实);banner 在 badge 下方一行显原因。互不覆盖。 |

---

## Requirements

### Frontend only

- `SubagentDrawer.vue` template header 加 `<div class="subagent-drawer__banner" v-if="bannerText">` 条件渲染,放在 status badge row 下方、startedAt/finishedAt 行上方
- 新 computed `bannerText: { text: string; kind: "error" | "warning" } | null`:
  - `status === "error"` + `summary` 非空 → `{ kind: "error", text: "Worker exited with error: " + truncate(summary, 80) }`
  - `status === "error"` + `summary` 空 → `{ kind: "error", text: "Worker exited unexpectedly at X.Xs" }`(用 terminalDurMs 公式,同 B1)
  - `status === "cancelled"` → `{ kind: "warning", text: "Worker stopped by user at X.Xs" }`
  - 其他状态(`running` / `completed`)→ `null`,banner 不渲染
- 复用现有 `--color-tool-error` token(red palette 已统一)
- BEM class `subagent-drawer__banner` / `subagent-drawer__banner--error` / `subagent-drawer__banner--warning`
- 复用现有 `Icon` 组件的 alert-triangle(`AlertTriangle` 是 heroicons 命名空间已有)

### Type 扩展

- 不动 `SubagentRunRow` interface(后端无字段可读)
- 不动 `TranscriptEntry`(不读 transcript)
- 仅读 `run.value.summary` 和 `run.value.status` 已存在的字段

### 测试

- `app/src/components/chat/SubagentDrawer.test.ts` 加 4 个 test 覆盖 banner:
  - `failed_drawer_shows_error_banner_with_summary_text`
  - `failed_drawer_falls_back_when_summary_empty`
  - `cancelled_drawer_shows_stopped_banner`
  - `running_and_completed_states_do_not_render_banner`

### 文案

- error:`"Worker exited with error: <summary truncate>"`(英文,跟现有 badge "failed at X.Xs" 文案风格一致)
- error fallback:`"Worker exited unexpectedly at X.Xs"`(用 frozen duration 同 B1)
- cancelled:`"Worker stopped by user at X.Xs"`(中英混排 drawer 已有先例,见 status badge "已停止")

---

## Acceptance Criteria

- [ ] `failed` 状态 + `summary = "shell: timeout after 10.0s"` → drawer header 显示红色 banner: `⚠ Worker exited with error: shell: timeout after 10.0s`
- [ ] `failed` 状态 + `summary = ""` → banner: `⚠ Worker exited unexpectedly at 11.7s`
- [ ] `failed` 状态 + `summary = "<200 字符长文本>"` → banner 截断到 80 字符 + `…`
- [ ] `cancelled` 状态 → banner 显示 amber/warning 色: `⚠ Worker stopped by user at 5.3s`
- [ ] `running` 状态 → banner 不渲染(DOM 查询 `.subagent-drawer__banner` 返 null)
- [ ] `completed` 状态 → banner 不渲染
- [ ] 现有 status badge 行为不变(badge 仍显示 "failed at X.Xs" / "已停止 at X.Xs")
- [ ] 4 个新 vitest test 通过 + 现有 15 个 SubagentDrawer test 不破坏
- [ ] `pnpm vue-tsc --noEmit` 0 error
- [ ] 后端零改动(无 db migration / 无 IPC shape 变化)

---

## Definition of Done

- SubagentDrawer.vue header 在 error/cancelled 时显 banner,运行/完成时不显
- 4 个新 vitest test + 现有 test 全过(19/19)
- vue-tsc 0 error
- DEBT.md FT-F-005 状态 `open` → `closed (2026-06-2X)`,填 commit hash
- journal 记录

---

## Out of Scope(明确不做)

- 不修改后端 schema(`errorMessage` / `cancelledBy` 字段不开,数据够用)
- 不区分 cancelled 是 user-stop 还是 system-cancel(后端 schema 不记录,UX 可接受)
- 不做 banner 跳转到 transcript entry(用户自己 scroll,scope 控小)
- 不做可关闭 banner(always-on,fail 是重要信息)
- 不做 banner 内嵌入最后 tool 详情(只在 summary 截断时显示 "…",点击不展开)
- 不动 drawer body 的 typed-cards(FT-F-001 范围)

---

## Technical Notes

### 关键文件

- `app/src/components/chat/SubagentDrawer.vue`(目标 ~+50 行,header template + 1 new computed + BEM CSS)
- `app/src/components/chat/SubagentDrawer.test.ts`(目标 +4 test,~80 行)
- `app/src/components/Icon.vue`(查 `alert-triangle` 是否在现有 heroicons 命名空间)

### 已有 token 复用

- `--color-tool-error`(red,banner background tint)
- `--color-tool-shell`(amber,banner warning 状态 = cancelled)
- `--color-bg-elevated`(banner 容器背景)
- `--color-bg-border`(banner 边框)

### 已有模式对齐

- B1 fix 已在 `statusDisplay` computed 用 `terminalDurMs` 公式(同 banner fallback 文案)
- B2 fix 已用 `extractToolResultDisplay` 解 envelope(本 task 不读 transcript)
- 主面板 `ToolCallCard.vue` 的 error 视觉:`tool-card--error` + 左 border 3px + tinted bg(drawer banner 复用类似但简化为左侧条 + icon)

### 不依赖

- FT-F-001 typed-cards(banner 是 header 改动,不碰 body)
- FT-F-002 toast fallback
- FT-F-003 workerWaiting ref leak
- FT-F-004 polish bundle(C1+C2+C3+C5 都是 body / 时间格式相关,本 task 是 header 加 1 行 banner)
- B1+B2 hotfix(`8c8ae47` 已合,本 task 在它的基础上加 banner)

---

## ADR-lite

**Context**:Drawer 在 error/cancelled 状态视觉差异不够,用户读不到失败原因。要在 1 个 PR / 0 后端改动 / 前向兼容 FT-F-001 的范围内解决。

**Decision**:
- Banner 形态选 **inline warning 横条**(始终展开,红色/amber tint + ⚠ icon),不选折叠 details(失败信息不该藏)
- 文案来源选 **`summary` 字段**(后端已写错误文本)+ cancelled 通用文案(不区分 user/system,schema 不记录)
- Banner 与 badge **共存**(badge 报时间,banner 报原因)

**Consequences**:
- 前向兼容 FT-F-001:typed-cards 化后 banner 仍在 header,banner 不依赖 body 渲染
- 已知 limitation:cancelled 不区分 user/system — 未来如要区分需后端加 `cancelled_by` column + 新 migration(独立 task)
- Banner 文案英文("Worker exited with error" / "Worker stopped by user"),跟现有 badge 英文文案("failed at X.Xs")对齐,drawer header 整体保持英文。body 的 transcript 是原始 JSON 中英文混排,不被 banner 影响
