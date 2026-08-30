# Chat Components Frontend Spec

> 主 chat panel + subagent drawer + 各类 inline card 组件的前端执行性规范索引。
> 2026-08-10 doc-split:本文原为 877 行单体 spec,按 feature 簇拆分为 5 个
> 子文件(见下方 Scenario Index)+ 1 节挪入 [memory-ui.md](./memory-ui.md) §V2-2+。

---

## Scenario Index (2026-08-10 doc-split:按 feature 簇拆分为子文件)

- [subagent-drawer](./chat/subagent-drawer.md) — SubagentDrawer 重构 PR1-6(5 段分组 / 边界态 R23-R25 / ToolCallHeader 共享 / merge-discard UI)
- [per-agent-model](./chat/per-agent-model.md) — B6+ C per-agent model UI + B6+ B per-dispatch `@@agent --model=` override
- [generative-ui](./chat/generative-ui.md) — B9 `use_ui` primitive registry(diff/code_block/button + DiffPrimitive raw fallback RULE-FrontDiff-001 + B9+ D3/D4 apply)
- [request-mode-change](./chat/request-mode-change.md) — `request_mode_change` inline card(3 状态机 + Yolo 二次 modal 双 IPC)
- [streamcontroller-routing](./chat/streamcontroller-routing.md) — streamController.handleToolCall → feature store 按 tool name 路由(B12 / C2)
- [message-list-and-markdown](./chat/message-list-and-markdown.md) — MessageList TransitionGroup enter 动画契约 + `.msg__markdown` 跨组件复用节奏镜像(2026-08-14 两个静默失效 bug 沉淀)
- [search-history-card](./chat/search-history-card.md) — `search_history` 专属卡片(替换渲染先例 / 自取自查 vs streamController 边界 / 四态机 / useSearchModal prefill 契约 + bootingPrefill 防线)
- [shell-card](./chat/shell-card.md) — `shell`/`run_background_shell` 专属卡 ShellCard(命令块常驻 / 一体化审批 / description display-only 不变量 / toolHeaderChip 兜底链 / PermissionActions 共享审批,2026-08-30)
- [session-busy-visibility](./chat/session-busy-visibility.md) — session busy 双源合流(streamingSessionIds ∪ serverBusy)+ finalizeRequest 消解 + 完成 toast 抑制规则 + 关闭确认 isTauriWebview 门(F6)
- [memory-ui.md §V2-2+](./memory-ui.md) — 自主记忆可观测性(recall chip + RuntimeMemoryModal,2026-08-10 从本文挪入)

---

> **何时读哪个子文件**:
> - 改 `SubagentDrawer*` / `Drawer*` 组件 / worker merge-discard → `chat/subagent-drawer.md`
> - 改 per-agent/per-dispatch model 选择 → `chat/per-agent-model.md`
> - 改 `use_ui` / `UiCard` / `DiffPrimitive` / `ButtonPrimitive` → `chat/generative-ui.md`
> - 改 `RequestModeChangeCard` / mode 切换 IPC → `chat/request-mode-change.md`
> - 改 streamController `handleToolCall` tool-name 路由 → `chat/streamcontroller-routing.md`
> - 改 MessageList 结构 / run-group / enter 动画 / markdown 容器样式 → `chat/message-list-and-markdown.md`
> - 改 busy 红点 / serverBusy / 完成 toast / CloseGuardDialog → `chat/session-busy-visibility.md`
- 改 `SearchHistoryCard` / `useSearchModal` prefill / search 命中行 helper → `chat/search-history-card.md`
> - 改 recall chip / RuntimeMemoryModal → [memory-ui.md §V2-2+](./memory-ui.md)
