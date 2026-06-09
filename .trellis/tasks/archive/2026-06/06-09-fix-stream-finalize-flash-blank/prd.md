# fix-stream-finalize-flash-blank

## Bug

`streamController.ts` 的 `finalizeRequest` 在收到 `done`/`error` 事件时调用 `evict(sessionId)` 删除内存消息缓存，但不重新从 DB 加载。导致：
1. 页面闪白——`messages` computed 返回 `[]`，无内容可渲染
2. 用户切换 session 回来才恢复（从 DB 重新加载）

根因：`evict` 是为了防止 2013 wire invariant（streaming buffer 是单个合并 assistant，DB 是 per-turn 拆分），但副作用是消息瞬间消失。

## Fix

修改 `finalizeRequest`：不再 bare `evict`，改为从 DB 重新加载并替换 buffer。旧 buffer 在 DB 加载完成前一直可见（不闪白），加载后原子替换。

只改 `app/src/stores/streamController.ts` 一个文件。

## Scope

- `finalizeRequest` 函数：移除 `evict(sessionId)` 调用，改为 fire-and-forget DB reload
- 新增 `reloadAfterFinalize` helper：`invoke("load_session")` → `rehydrateMessages` → `putMessages`

## Non-goals

- Rust 端 error path 的 assistant 持久化（当前日志显示 API 成功，优先级低）
- LRU 策略变更
