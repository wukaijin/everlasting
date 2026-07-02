# B9-B code_block primitive（hljs 高亮 + 复制）

> Child of `07-02-b9-generative-ui`。**blockedBy `07-02-b9-use-ui-infra`**（复用 A 的 registry + UiCard 框架）。
> 父决策 D1-D6 见 parent `prd.md`（D6 高亮库 = hljs）。

## Goal

落地 code_block primitive 的**两个高亮入口**，并补复制交互：
1. `use_ui({type:"code_block"})` primitive → `<CodeBlockPrimitive>`（hljs 独立高亮 + 复制按钮）
2. 现有 markdown 代码块（`utils/markdown.ts` 的 marked 管线）顺带获得 hljs 高亮（无回归）

共享同一份 hljs 配置（`utils/highlight.ts`）。

## Requirements

- **装依赖**：`highlight.js` + `marked-highlight`（接 marked 18）。
- **`utils/highlight.ts`**：导出 `renderCodeHtml(code, language)` — language 已知且 hljs 认 → `hljs.highlight`；否则 `hljs.highlightAuto`。用 `highlight.js/lib/common` 语言集（~主流语言，非 full ~900KB）。
- **`utils/markdown.ts`**：`marked.use(markedHighlight({ highlight(code,lang){ return renderCodeHtml(...) } }))`，现有 markdown 代码块获得高亮。`DOMPurify` 配置允许 hljs 的 `<span class="hljs-*">`（验证 markdown.test.ts XSS fixtures 不回归）。
- **`<CodeBlockPrimitive>`**（`components/chat/primitives/CodeBlockPrimitive.vue`）：props `primitive: UiPrimitive`，读 `primitive.code` + `primitive.language` + `primitive.title`；hljs 高亮渲染 + 复制按钮（`navigator.clipboard.writeText` + 2s "已复制" 反馈）。
- **registry 替换**：`uiPrimitiveRegistry.ts` 的 `code_block` 条目从 `MockPrimitive` 换成 `CodeBlockPrimitive`（`diff` 仍 MockPrimitive，等 Child C）。
- **use_ui schema 补充**：`use_ui.rs` 的 `definition()` description 补 code_block 的 `code` / `language` 字段说明（schema 仍 `additionalProperties: true`，不需改校验）。

## Acceptance Criteria

- [ ] `use_ui({type:"code_block", code, language})` 渲染 hljs 高亮代码 + 复制按钮
- [ ] 复制按钮点击 → `navigator.clipboard.writeText(code)` + "已复制" 反馈（2s 复位）
- [ ] 现有 markdown 代码块（```` ```lang ````）获得 hljs 高亮（markdown.test.ts 全绿，XSS fixtures 不回归）
- [ ] 未知 language → `highlightAuto` 兜底（不崩、不高亮也能展示纯文本）
- [ ] hljs 用 `common` 语言集（非 full），bundle 增量可控（~+30KB gzip 量级）
- [ ] `vue-tsc --noEmit` 0 err + vitest 全绿（含新 CodeBlockPrimitive.test + markdown.test 无回归）

## Dependencies

- `blockedBy: 07-02-b9-use-ui-infra`（registry + UiCard + USE_UI_TOOL_NAME 来自 A）
