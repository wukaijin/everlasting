<!-- Moved from memory-ui.md 2026-09-19 (doc-split) -->

---

## V2-2+ 自主记忆可观测性 — recall chip + RuntimeMemoryModal (2026-07-06)

> **Source**: moved from `frontend/chat.md` §"V2-2+ 自主记忆可观测性"
> (2026-08-10 doc-split task)。本节是前端 cross-ref 锚点,后端契约见
> [memory.md §V2-2+](../../backend/memory.md)。

自主记忆(autonomous memory)之前是黑盒:agent 写、agent 召回、agent 提升 / 降级,用户看不到命中、改不了内容、转不了状态。V2-2+ 补**人**的入口。

### 数据流契约

- **recall event 路由**:`ChatEvent::Recall { hits }` ride on `chat-event` channel → `streamController.handleChatEvent` 的 `case "recall"` → `useMemoryStore().pushRecallHits(sessionId, hits)`(per-session 累积)。**不**写进 messages buffer(transient,同 `Retrying`)。新 user message(`startRequest`)清空该 session slice(per-turn,design D7)。
- **recall 状态归属**:`recallHitsBySession: reactive(new Map<sid, RecallHit[]>)` 在 `useMemoryStore`(**不**在 streamController)— `state-management.md:131-139` 跨切面领域状态归 feature store,controller 只路由。`recallHitsForSession(sid)` 是**纯读** computed(getter 不 mutate 自己 track 的 deps — `state-management.md:166-212` 硬规则)。
- **recall chip 挂载点**:`ChatPanel.vue`(header 下、`<main>` 上方)。`MessageList.vue` **无** banner slot(纯 scroll 容器);`ChatPanel` 是跨切面 overlay 宿主(WorkerAskBanner / ActivityPanel / AuditLogModal 等 Dialog modal 先例;旧 PermissionModal 全局审批与 ChecklistCard 浮层已分别 inline 化与并入 ActivityPanel)。
- **行点击 → modal**:`MemoryPreview` 的 runtime row `@click` emit `manage(id)` → `MemoryModal` forward → `ChatPanel.onMemoryManage` → 开 `RuntimeMemoryModal`。MemoryPreview **不**硬绑 modal(两处复用:ChatPanel 的 MemoryModal + 未来 Settings;宿主决定)。

### RuntimeMemoryModal.vue(新建)

reka-ui Dialog 6 件套(复刻 MemoryModal sizing 80vw/min640/max900/80vh + zoom 动画)。嵌套在 MemoryModal Dialog 内,reka-ui 2.9.9 支持嵌套 Dialog(焦点陷阱移到最内层)。
- **统计区**:hitCount / lastUsedAt / confidence / sourceSessionId / sourceRef / createdAt / updatedAt + 「人工编辑」徽标(`editedByUser`)。
- **状态下拉**:reka-ui Select(复刻 ModelForm 模式),options 由 `LEGAL_STATUS_TRANSITIONS[current]`(memory.ts 导出)驱动 — **前端只读副本**,backend `update_status` 矩阵是硬墙。转 demoted 弹内联 reason input(矩阵仅 →demoted 接受 reason)。
- **编辑区**:native `<input>` + `<textarea>`(reka-ui 2.9.9 无 TextField primitive,`reka-ui-usage.md` 强制 native),「编辑」toggle 切只读 / 可写,保存调 `store.updateMemory`(optimistic + IPC + 用返回的 authoritative row 覆盖避免时钟漂移)。
- **删除**:复用 `ConfirmDialog`(portal 到 body,z-index 1100 < Dialog 2000,层叠正确)。

### Common Mistakes

- **recall hits 写进 messages**:`ChatEvent::Recall` 是 transient,handler 路由进 store,**绝不**push 进 messages 数组(污染持久化 DB shape,同 `Retrying` rationale)。
- **前端做矩阵合法性检查**:`LEGAL_STATUS_TRANSITIONS` 只是 dropdown 的 OFFER 列表,不是安全边界。永远让 backend `update_status` 矩阵做权威;前端检查会跟 backend 漂移。
- **recall chip 挂 MessageList**:MessageList 无 banner slot;必须挂 ChatPanel。
- **`RecallHit.memory_id` 当 UUID**:是 SQLite auto-id(对齐 `MemoryRow.id`),非 UUID `memoryId` 字段。删记忆仍走 UUID。
- **streamController 持有 recall 状态**:违反 `state-management.md:131-139` — 领域状态归 feature store,controller 只路由。
