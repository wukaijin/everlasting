# B9-C diff primitive（只读展示 + 复制）

> Child of `07-02-b9-generative-ui`。**blockedBy `07-02-b9-use-ui-infra`**（复用 A 的 registry + UiCard）。
> 父决策 D1-D6 见 parent `prd.md`（D4 diff 只读 + 复制，应用推后期）。

## Goal

落地 diff primitive：`use_ui({type:"diff", diff_text})` → `<DiffPrimitive>` 复用现有 `DiffView` 只读渲染 + 复制按钮。MVP 不做"应用"（D4）。

## Requirements

- **`<DiffPrimitive>`**（`components/chat/primitives/DiffPrimitive.vue`）：props `primitive: UiPrimitive`，读 `primitive.diff_text`（unified diff 字符串）+ `primitive.title`。
  - 用 jsdiff `parsePatch(diff_text)` 拆成多文件 patch → 每 patch 重组为 `FileDiff`（path 清洗 `a/`/`b/` 前缀、added/removed 计数、status 推断 added/deleted/modified）→ 传 `<DiffView>` 渲染（复用其行级着色 + 折叠 + raw fallback）。
  - parsePatch 失败 / 空 → 单文件 raw fallback（DiffView 的 `<pre>` 路径）。
  - 复制按钮：`navigator.clipboard.writeText(diff_text)` + 2s "已复制" 反馈。
- **registry 替换**：`uiPrimitiveRegistry.ts` 的 `diff` 条目从 `MockPrimitive` 换成 `DiffPrimitive`（MockPrimitive 保留为 fallback）。
- **use_ui schema 补充**：`use_ui.rs` description 补 diff 的 `diff_text` 字段说明（schema 仍 `additionalProperties: true`，校验不变）。

## Acceptance Criteria

- [ ] `use_ui({type:"diff", diff_text})` 渲染 DiffView（行级 +/- 着色）
- [ ] 多文件 unified diff（一个 `diff_text` 多 patch）→ 多个 file 卡片
- [ ] path 清洗 `a/`/`b/` 前缀；added/removed 计数正确；status 推断合理
- [ ] 复制按钮 → `clipboard.writeText(diff_text)` + "已复制" 反馈
- [ ] 非法/空 diff_text → raw fallback（不崩）
- [ ] `vue-tsc --noEmit` 0 err + vitest 全绿（含新 DiffPrimitive.test + 无回归）+ cargo test use_ui 绿（description 改动）

## Dependencies

- `blockedBy: 07-02-b9-use-ui-infra`（registry + UiCard 来自 A）
