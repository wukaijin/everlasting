# PR7: 首行空行排查 (定位 LLM 行为 vs 渲染问题)

> Source spike: [`docs/spikes/2026-06-06-feature-requests.md`](../../../../../docs/spikes/2026-06-06-feature-requests.md) 第 5 条
> 父 task: `06-06-spike-005-follow-up`
> 父 prd: [../06-06-spike-005-follow-up/prd.md](../06-06-spike-005-follow-up/prd.md) (PR7 段)
> Priority: P2 (低风险, 5-30 行 CSS / content 处理)

## Goal

排查并修复 assistant 消息首行空行问题。三种根因 + 修法对应:

1. **LLM `\n` 开头**: LLM 第一字符是换行符 → `MessageItem.vue` 渲染前 `content.trimStart()`,或 `chat.ts` 落 DB 前 strip
2. **CSS padding**: `.msg__bubble` 顶部 padding 让第一行被压下 → 调 padding-top
3. **Vue transition 时机**: 入场动画导致首帧被压 → 排查/关 transition

## What I already know

- `MessageItem.vue:86-91` 当前 bubble 渲染: `<div v-if="showBubble" class="msg__bubble"><span>{{ message.content }}</span>...</div>`
- `MessageItem.vue:145` `.msg__bubble { padding: 10px 14px; ... }` — 顶部 10px padding
- `chat.ts:710` `content: trimmed` (用户消息已 trim,但 assistant 流式内容从 LLM 直接来,没有 trim)
- `lib.rs` LLM stream 进入 store 的 text 也不 trim
- `white-space: pre-wrap` 保留换行 → LLM 的 `\n` 开头的首字符会渲染为视觉空行
- PR6 (markdown) 尚未实施 (按 P0 顺序 PR7 在 PR6 之后),所以本 PR 修法不能在 v-html 层面做

## Requirements

- 抓一次实际 LLM 输出复现 (RUST_LOG=info 看首字符)
- 三选一修法:
  - **A (推荐 v1)**: CSS-only fix — `.msg__bubble { padding-top: 0 }` + 视觉调 (如果根因是 CSS)
  - **B**: LLM 行为层 fix — `MessageItem.vue` 渲染前 `content.trimStart()`,或 `chat.ts` 落 DB 时 strip (如果根因是 LLM `\n` 开头)
  - **C**: transition 排查 — 临时关 transition 验证 (如果根因是动画)
- 根因落地到 `docs/spikes/2026-06-06-feature-requests.md` 第 5 条 (`[RESOLVED 2026-06-XX: <root cause>]`)

## Acceptance Criteria

- [ ] 复现首行空行,根因记录到 spike 文件或 commit message
- [ ] 修法后,流式期间首行不再有空行
- [ ] `pnpm build` 通过
- [ ] 视觉验证截图/screen capture 留痕 (可选,人工确认)

## Definition of Done

- 修改 1-2 个文件 (`MessageItem.vue` 或 `chat.ts` 之一)
- spike 第 5 条更新
- 父 task 06-06-pr7-first-blank-line 走 standard Trellis 流程到 archived

## Out of Scope

- PR6 (markdown 渲染) 范围内的 marked / DOMPurify 引入
- 整段消息首尾空白处理 (本 PR 只针对首行)
- i18n (无)

## Technical Notes

- 改动文件可能:
  - `app/src/components/chat/MessageItem.vue` (CSS 或 template trim)
  - `app/src/stores/chat.ts` (DB 写入前 strip, 影响所有消息)
- 注意: PR6 markdown 实施后, `v-html` 模式下 `trimStart()` 必须在 marked.parse 之前调用,避免 trim 错位
- 注意: 修法选 B 的话要小心不要 trim 掉用户**故意**的换行 (例如代码块内缩进) — 应该只 strip leading whitespace
