# PR6: markdown 渲染 (marked v18 + DOMPurify + 流式节流)

> Source spike: [`docs/spikes/2026-06-06-feature-requests.md`](../../../../../docs/spikes/2026-06-06-feature-requests.md) 第 5 条 (剩余部分)
> 父 task: `06-06-spike-005-follow-up`
> 父 prd: [../06-06-spike-005-follow-up/prd.md](../06-06-spike-005-follow-up/prd.md) (PR6 段)
> Priority: P0
> 关联 research: [`research/markdown-library.md`](research/markdown-library.md)

## Goal

把 LLM assistant 的文本消息从纯文本 `{{ message.content }}` 升级为**安全的 markdown 渲染**。
- 库: `marked@18.0.5` + `dompurify@3.4.8` (lockfile 锁版本)
- XSS 防护: marked v8+ 已删 `sanitize` 选项, **必须**外配 DOMPurify, **无例外**
- 流式期间也渲染 (B 方案), 50ms debounce 合并 delta 后再渲染
- **顺手**开 vitest 基础架构 (本项目无前端测试框架), PR5 cancel 测试会复用

## What I already know

- 父 prd 段已确认: marked@18.0.5 + dompurify@3.4.8, 流式 50ms debounce, 无 v-html sanitization = 直接 XSS
- `MessageItem.vue:86-91` 当前是 `<div v-if="showBubble" class="msg__bubble"><span v-if="hasVisibleBubble || message.content" class="msg__text">{{ message.content }}</span>...`
- PR7 已加 `displayContent` computed (`replace(/^\s+/, "")`), PR6 需要决定 trim 时机: `marked.parse()` 之前还是之后
- `package.json` 暂无 `marked` / `dompurify` / `vitest`
- 项目无 frontend 测试框架
- BACKLOG §5 generative UI 的 `code_block` 高亮本 v1 **不集成**, 留作 v2
- 流式期间: `message.content` 在 `chat.ts:360` `last.content += event.text` 持续累加, PR6 需要在该累加点引入 debounce
- 边界 case: `displayContent` (PR7) + markdown 顺序, 应该 markdown **解析前** trim (避免 markdown 把首字符当 syntax 吃)

## Requirements

### Markdown 渲染
- 装 `marked@18.0.5` + `dompurify@3.4.8` + `@types/dompurify`, **lockfile 锁精确版本** (v18 刚发 2 天, 自动升有 hotfix 风险)
- 新 `app/src/utils/markdown.ts`:
  - `renderMarkdown(text: string): string` — `marked.parse(text, { gfm: true, breaks: true })` → `DOMPurify.sanitize(html)`
  - **空字符串/纯空白** → 返回 `""` (避免渲染空 `<p></p>`)
- 新 `app/src/utils/markdown.test.ts`: vitest 单测
  - XSS fixture: `<script>alert(1)</script>`, `<img src=x onerror=alert(1)>`, `<a href="javascript:alert(1)">x</a>` 都不应执行
  - 基础: `**bold**` → `<strong>bold</strong>`, `` `code` `` → `<code>code</code>`
  - 代码块: `` ```py\nprint(1)\n``` `` → `<pre><code>...</code></pre>`
  - 链接: `[x](https://example.com)` → `<a href="https://example.com">x</a>`
- `MessageItem.vue`:
  - 删 PR7 的 `displayContent` computed (不再需要, 改在 `renderMarkdown` 内 trim)
  - 改 template: `{{ message.content }}` → `<span v-html="renderedContent" class="msg__markdown" />`
  - 加 `renderedContent` computed: 调 `renderMarkdown(displayContent)`, 50ms debounce
  - 删 `.msg__text` (v-html 不需要), 加 `.msg__markdown` (`:deep()` selector for code/pre/a)
- 删 `.msg__bubble` 的 `white-space: pre-wrap` (markdown 自己处理换行, pre-wrap 会破坏 `<pre>`)
- **CSS**: markdown 渲染的 HTML 元素 (`h1` `h2` `p` `ul` `ol` `li` `code` `pre` `a` `strong` `em` `blockquote`) 加 `.msg__markdown :deep()` 样式, 用项目 CSS 变量

### 流式 debounce
- `app/src/utils/markdown.ts` 加 `createMarkdownRenderer(debounceMs = 50)`:
  - 返回 `render(text: string): string` (同步, 但内部防抖)
  - 或返回 `renderDebounced(text): { value: string }` (reactive ref)
- **选 reactive ref 模式**: `MessageItem.vue` 用 `const renderedContent = ref("")`, watch `displayContent` 变化, setTimeout 50ms 后更新 `renderedContent`
- 卸载时 `clearTimeout` 避免内存泄漏
- 流式结束 (streaming=false) 时立即 flush, 不等 debounce (UX 不卡)

### vitest 基础架构
- 装 `vitest@2.x` + `@vue/test-utils` + `jsdom`
- `app/vitest.config.ts`: jsdom env, path alias `@` 配 `./src`
- `app/package.json` 加 scripts: `"test": "vitest run"`, `"test:watch": "vitest"`
- 测试文件: `app/src/utils/markdown.test.ts` (作为 vitest 第一个用例)
- `tsconfig.json` 加 `vitest/globals` types (可选, 也可用 explicit import)
- **注意**: vitest 跑在 Node, 不在 Tauri webview; jsdom 模拟 DOM; DOMPurify 在 jsdom 下工作 OK

### XSS 留痕
- `docs/HACKING-markdown.md` (新文件): 记录 marked v18 删 sanitize 的陷阱 + DOMPurify 必须外配的原因, 以及已覆盖的 XSS fixture 列表

## Acceptance Criteria

- [ ] `pnpm build` (vue-tsc + vite) 通过
- [ ] `pnpm test` (vitest) 通过, XSS fixture 全绿
- [ ] 流式期间: 看到 markdown 实时渲染 (标题加粗/列表/链接等), 50ms debounce 肉眼无延迟
- [ ] XSS 防护: `<script>alert(1)</script>` 输入不执行 (vitest 断言)
- [ ] marked v18 + dompurify 在 `package.json` + `package-lock.json` 锁定精确版本
- [ ] 文档: `docs/HACKING-markdown.md` 记录 marked v18 sanitize 陷阱
- [ ] 用户消息渲染不受影响 (`chat.ts:710` `content: trimmed`, markdown 也安全)
- [ ] thinking / tool call / redacted thinking 块不受影响 (它们不是 markdown)

## Definition of Done

- 修改 ~5-7 个文件
- vitest 基础架构到位, 1+ 个测试文件 + 1 个 fixture
- pnpm build + pnpm test 双过
- 跑完 standard Trellis 流程到 archived

## Out of Scope

- BACKLOG §5 generative UI 的 `code_block` 高亮 (本 v1 用纯 `<pre><code>`)
- markdown 主题切换 (light/dark mode 自动适配)
- 自定义 markdown 扩展 (footnote / table of contents)
- LLM 输出的 `<think>...</think>` 等特殊标记
- 实时 markdown 预览 (仅 assistant 消息渲染)
- PR7 的 `displayContent` 留在原位 (commit 历史), PR6 改在 `renderMarkdown` 内 trim

## Technical Notes

- 改动文件:
  - `app/package.json` (+deps + scripts)
  - `app/package-lock.json` (auto)
  - `app/vitest.config.ts` (新)
  - `app/src/utils/markdown.ts` (新)
  - `app/src/utils/markdown.test.ts` (新)
  - `app/src/components/chat/MessageItem.vue` (template + script + CSS)
  - `app/tsconfig.json` (types, 如果用 globals)
  - `docs/HACKING-markdown.md` (新)
- 风险: marked v18 兼容性 — 如果 v18 跟项目 vite 6 不兼容, 降级 v17 (但 lockfile 已锁, 需手动改 package.json)
- 风险: v-html 跟 Vue scoped CSS 冲突 — 用 `:deep()` selector 处理
- 风险: DOMPurify 在 vitest jsdom 下未跑过 — 需要测试时确认
- 关联: PR7 的 `displayContent` 跟 PR6 markdown trim 顺序, **PR6 改在 `renderMarkdown` 内 trim**
- BACKLOG §5 (Generative UI) 中 Phase 1 必做 4 种 (`button` / `selector` / `diff` / `code_block`) 暂未做, 跟本 PR 独立

## Decision (ADR-lite)

(待 PR6 brainstorm / 实施时填充, 因为 PR6 在父 task brainstorm 阶段已对齐主要决策, 此处只记 PR6-specific 决策)

- **决策**: PR6 改在 `renderMarkdown` 内 trim, 不保留 PR7 的 `displayContent` 单独 computed
- **理由**: trim 必须在 markdown 解析前完成; 单一职责, 渲染层只关心渲染
- **后果**: PR7 的 `displayContent` 移除后, 用户消息首行 strip 也走 markdown 路径 (但 chat.ts:710 已 trim 用户消息, 所以是 no-op)
