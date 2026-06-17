# B2 PR1.5 输入框 token 着色

## Goal

输入框内 `/command` / `@file` / skill token **带颜色高亮**，让用户一眼区分引用类 token 与普通文本。优先于 PR2（后端注入）。B3 显式 handoff 横切议题。

## Context

- B3 `/command` + B2 PR1 `@file` 已实现（textarea 纯文本无高亮）。skill（B4）未来加第三类 token。
- B2 brainstorm Q4 当时决定"不做、留独立 task"；现用户改主意：PR1.5 先做（在 PR2 之前）。

## Decision（已决 2026-06-17：CodeMirror 6）

用 CodeMirror 6 替换 ChatInput 的 textarea。理由：中文 IME 由专业编辑器原生处理（避免 overlay caret 同步坑）+ B4 skill 未来 token 扩展一次到位 + decoration API 着色专业。代价：重构 ChatInput，回归风险高，引入 `@codemirror/*` 依赖——接受。~~textarea overlay~~ 方案否决（IME/光标对齐坑中文项目反复修）。

## Research 结论（见 Research References）

- **集成**：直接手写 `@codemirror/view`+`@codemirror/state` 包装（~60 行，复刻 vue-codemirror v-model 双向同步模式，两 `!==` 守卫防循环），**不引第三方 Vue 包装**（vue-codemirror 6.1.1 自 2022 未更新 + 强依赖 116KB codemirror meta-package）。
- **IME**：CM6 `view.composing` 原生处理 → 现有 `isComposing` ref + `onCompositionStart/End` **全删**。
- **autosize**：纯 CSS（`.cm-editor` max-height + `.cm-scroller` overflow:auto）→ JS `autosize()` 删除。
- **最小依赖**：`@codemirror/state`(15.9KB) + `@codemirror/view`(77KB)（未 tree-shake 上界，build 实测回填）。
- **popover 不用改**：anchor 在 `.chat-input__row`（非 textarea）→ CM 替换后 popover CSS 全保留，唯一改动 TriggerMenu `:trigger-el` 从 `textareaEl` → `view.dom`。
- **Shift+Tab**：`registerShiftTabCycle` 需补 `stopPropagation()`，否则 CM `defaultKeymap` Shift+Tab unindent 先跑。

## Technical Approach

### 着色（PR-B）
- `ViewPlugin` + `Decoration.mark` + `RangeSetBuilder`，正则标记 `/\w+`（command，`--color-accent`）和 `@[\w/.-]+`（file，`--color-tool-read`）；skill 预留。映射 design-tokens 到 decoration `Theme.spec`。

### 迁移（PR-A）
- 手写 CM 包装组件（或 ChatInput 内联）：`new EditorView` + `updateListener` 双向 v-model。
- 删除 `isComposing`/`onComposition*`/`autosize()`；IME 由 CM 原生。
- **trigger 面板接入 CM**（保功能等价，不临时移除）：`updateListener` 监听 doc/selection → 替代 `onTextareaInput` 的 syncCommandPalette/syncFilePalette；`doc.lineAt(head)` 替代 `currentLineInfo`；`keymap.of(Prec.highest [...])` 拦截 ↑↓Enter/Tab/Esc 路由 trigger 面板，Enter 发送用 `view.composing` 门控。
- TriggerMenu `:trigger-el` → `view.dom`。

## PR 切分（两步，保 main 功能不回退）

- **PR-A**（CM 迁移 + 面板接入，功能等价无着色）：CM 骨架 + v-model + autosize(CSS) + IME-safe Enter 发送 + trigger 面板（/command + @file）接入 CM + Tab/Shift+Tab + Mode/Model/latency popover 保留。**验收 = 现有所有行为等价**（重点 IME-safe Enter + trigger 面板 + popover）。
- **PR-B**（token 着色）：ViewPlugin + Decoration.mark + design-tokens 映射。纯加法，不碰迁移逻辑。

> research 原建议三步（PR-A 临时移除面板），本 prd 改为两步——避免 main 上 `/command`+`@file` 在 PR 间回退。

## 风险

1. **IME-safe Enter**（最大）：`view.composing` ≠ textarea `isComposing`，需 Win WebView2 + WSLg + macOS 三平台实测中文 IME（拼音/双拼候选窗 Enter 不误发）。
2. **Shift+Tab**：`registerShiftTabCycle` 补 `stopPropagation()` 防 CM unindent。
3. **trigger 面板接入**：`updateListener` 触发频率高（每次 transaction），sync 逻辑要轻；`doc.lineAt(head)` 行检测正确性。
4. **bundle 体积**：CM view 77KB 上界，build 实测真实 gzip 回填 TECH.md。

## Acceptance Criteria

- [ ] PR-A：CM 替换 textarea，现有全部行为等价（Enter 发送/IME-safe/Shift+Tab Mode/trigger 面板 /command+@file 互斥+Tab 确认/Mode/Model/latency popover/autosize）。
- [ ] PR-B：`/command` token 高亮（accent）+ `@file` token 高亮（read）+ skill 着色位预留。
- [ ] `vue-tsc` 0 错误 + `cargo check` 0 warning（前端纯改）+ build 实测 CM gzip 体积。
- [ ] IME 三平台手测（中文输入法 Enter 不误发）。

## Out of Scope

- B4 skill 实现（只预留 token 着色位）。
- PR2 后端 @token 注入（独立 task）。

## Research References

- [`research/codemirror-vue-integration.md`](research/codemirror-vue-integration.md) — 手写 CM 包装 ~60 行 + 最小 2 包 + IME/autosize 删除 + v-model 双向。
- [`research/codemirror-token-highlight-migration.md`](research/codemirror-token-highlight-migration.md) — 着色 ViewPlugin + 面板接入 updateListener/keymap/doc.lineAt + 迁移风险清单 + popover anchor 不变。

## References

- B3 PRD §Out of Scope：`.trellis/tasks/archive/2026-06/06-16-b3-command-palette/prd.md`
- TECH.md §1.2 CodeMirror 6 候选
- 现有 `app/src/components/chat/ChatInput.vue` + `TriggerMenu.vue`
