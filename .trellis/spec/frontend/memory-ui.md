# Memory UI — MemoryPreview Panel + Settings Tab + ProjectTabs Dropdown

> **基线**:2026-06-10 (B5 PR2, frontend)
> **同源文档**:
> - [llm-contract.md](../backend/memory/scenario-two-layer-memory-injection.md) — backend `read_memory_layers` / `read_memory_content` / `open_memory_in_editor` IPC contract
> - [state-management.md](./state-management.md) — Pinia store patterns + listener registration
> - [reka-ui-usage.md](./reka-ui-usage.md) — reka-ui Tab / popover conventions
> - [design-tokens.md](./design-tokens.md) — color / spacing / typography tokens
> - [popover-pattern.md](./popover-pattern.md) — hand-rolled popover pattern
> - [cross-layer-thinking-guide.md](../guides/cross-layer-thinking-guide.md) — Tauri command contracts
>
> **何时读本文**:实现 B5 Memory PR2(前端 UI 预览)或 V2-2+ 自主记忆可观测性(recall chip / RuntimeMemoryModal),或修改 `useMemoryStore` / `<MemoryPreview>` / `<MemoryLayerItem>` / `<MemoryTab>` / `<RuntimeMemoryModal>` 时。

> **⚠️ Updated 2026-06-15 (RULE-C-001)**: the backend `notify`
> watcher was removed (freshness is now an mtime fence in
> `load_for_session`). The `memory:reloaded` event is therefore
> **never emitted**; the defensive `listen("memory:reloaded")` in
> `useMemoryStore` was deleted. Re-fetch now happens only via
> `loadForProject` (mount / project switch) and `refresh()` (刷新
> button) — both call `read_memory_layers`, which is always
> current thanks to the fence. The `memory:reloaded` / watcher
> mentions below are the **old** design. See
> `.trellis/tasks/06-15-p1-memory-watcher-appstate/`.

---


> **分篇**(2026-09-19):B5 Scenario、Design Decisions/Common Mistakes/Anti-Patterns/Future Work 与 V2-2+ 可观测性已按 tool-contract 模式拆至 `memory-ui/` 子目录(一主题一文件,原锚点以 stub 保留)。

## Scenario: B5 Memory Preview UI (PR2)

> **已拆出**(2026-09-19 doc-split):完整契约见 [`memory-ui/scenario-b5-memory-preview-ui.md`](./memory-ui/scenario-b5-memory-preview-ui.md)。

## Design Decisions

> **已拆出**(2026-09-19 doc-split):完整决策记录(含 Common Mistakes / Anti-Patterns / Future Work)见 [`memory-ui/decisions-and-antipatterns.md`](./memory-ui/decisions-and-antipatterns.md)。

## Common Mistakes

> **已拆出**(2026-09-19 doc-split):见 [`memory-ui/decisions-and-antipatterns.md`](./memory-ui/decisions-and-antipatterns.md)。

## Anti-Patterns

> **已拆出**(2026-09-19 doc-split):见 [`memory-ui/decisions-and-antipatterns.md`](./memory-ui/decisions-and-antipatterns.md)。

## Future Work (Deferred from B5 V2 1 期)

> **已拆出**(2026-09-19 doc-split):见 [`memory-ui/decisions-and-antipatterns.md`](./memory-ui/decisions-and-antipatterns.md)。

## Related

- `.trellis/spec/frontend/state-management.md` — Pinia store
  模式,`unlisten*` 模块级守卫
- `.trellis/spec/frontend/reka-ui-usage.md` — reka-ui Tab /
  Popover 模式;本期不用 Popover (手写 popover)
- `.trellis/spec/frontend/popover-pattern.md` — Memory dropdown
  ~~沿用 hand-rolled 模式~~(OBSOLETED 2026-06-11,见
  `06-11-memory-modal-appheader-entry`;Memory 改为 modal,
  popover-pattern 规则仍适用于其它真正的 popover 场景)
- `.trellis/tasks/06-11-memory-modal-appheader-entry/prd.md` —
  Memory 入口从 popover 迁移到 AppHeader corner action +
  reka-ui Dialog modal 的设计文档
- `app/src/components/memory/MemoryModal.vue` — 当前 Memory
  快查入口的 reka-ui Dialog 实现(本期 PR2 是 popover,2026-
  06-11 follow-up 替换)
- `app/src/components/chat/ChatPanel.vue` — Brain 图标 trigger
  挂载点(WorktreeChip 右侧);`useProjectsStore().currentProjectId`
  存在时才显示。MemoryModal 实例化在同一文件,`memoryModalOpen`
  ref 控制开关。
- `.trellis/spec/frontend/design-tokens.md` — Memory 状态点颜色
  走 token (`#4ade80` / `#fbbf24` 是 token-usage 同一调色板)
- `app/src/stores/streamController.test.ts` — vitest 单测模式
  参照;`memory.test.ts` 用同样的 setActivePinia / createPinia
  套路
- `app/src/utils/markdown.ts` — `renderMarkdown` 走 marked +
  DOMPurify,XSS 防护已锁定
- `app/src/components/chat/MessageItem.vue` — markdown 渲染
  参考(同一 `renderMarkdown` 路径)
- `app/src-tauri/src/memory/types.rs` — Rust 类型 → TS 镜像
  的源头
- `app/src-tauri/src/commands/memory.rs` — 3 个 Tauri command
  的 contract 本期的前端契约


## V2-2+ 自主记忆可观测性 — recall chip + RuntimeMemoryModal (2026-07-06)

> **已拆出**(2026-09-19 doc-split):完整契约见 [`memory-ui/v2-observability-recall-chip.md`](./memory-ui/v2-observability-recall-chip.md)。
