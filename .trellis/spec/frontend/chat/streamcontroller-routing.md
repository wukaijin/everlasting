# streamController.handleToolCall → feature store 路由 (B12 / C2)

> **Source**: extracted from `frontend/chat.md` §"Convention: streamController.handleToolCall → feature store 按 tool name 路由（B12 / C2）" (2026-08-10 doc-split task).

## Convention: streamController.handleToolCall → feature store 按 tool name 路由（B12 / C2）

> 来源:B12 checklist (2026-06-19,`update_checklist` tool_use 实时驱动 floating ChecklistCard)
> + C2 review-viz (2026-07-27,`write_file` 写 review-state.json 实时驱动 ReviewMatrix)。

### 模式

当某个 feature store 需要在一个工具被调用时**实时刷新**(无需后端领域事件),
不要让 store 自己 `transport.listen("tool:call")`(会与 streamController 的
全局监听重复 + 丢失 sessionId 上下文)。而是:

1. streamController 在 `handleToolCall`(`streamEvents.ts`,经 streamController
   re-export)已有全局 `transport.listen("tool:call")`,payload 含 `name` + `input`
   + 通过 `req.sessionId` 可拿当前 session。
2. 在 `handleToolCall` 里按 `payload.name` 加路由分支,调对应 feature store
   的入口方法:
   ```ts
   if (payload.name === "<tool_name>") {
     use<X>Store().handle<Y>(req.sessionId, payload.input);
   }
   ```
3. feature store 暴露的入口方法做**幂等 + 守门**(slug/sessionId/模式匹配),
   内部 debounce 防一轮多 chunk。

### 已有实例

| tool_name | feature store | 入口 | 触发条件 | 位置 |
|---|---|---|---|---|
| `update_checklist` | `useChecklistStore` | `handleToolCall(sessionId, name, input)` | 总是(B12 checklist 是该工具的唯一消费者) | `handleToolCall` 内 `CHECKLIST_TOOL_NAME` 分支(`streamEvents.ts`) |
| `write_file` | `useReviewStateStore` | `handleReviewStateWritten(sessionId, slug)` | `matchesReviewStatePath(input.path, slug)` 命中 review-state.json | `handleToolCall` 内 `write_file` 分支(`streamEvents.ts`) |

### 关键纪律

- **路由分支必须放在 tool 跟踪逻辑之前/之后都不影响 `req.toolStartedAt.set`**:
  `handleToolCall` 末尾的 `toolStartedAt.set(payload.id, Date.now())`(F5
  per-tool duration)是**无条件**的,路由分支不能 early-return 跳过它。
  C2 的 write_file 路由在 checklist 路由之后、toolStartedAt 之前,不 return。
- **path 匹配要保守**:工具的 path 参数可能是相对路径(相对 `ctx.cwd`)。
  matchesReviewStatePath 用 basename + 子串 + 兜底,误触发只是多一次 IPC
  (读不到返 Missing,无害),漏触发由用户手动操作兜底。不追求 100% 精确。
- **store 不 self-listen**:刷新完全由 streamController 路由驱动。store 只
  暴露入口方法给 streamController 调。这是「领域状态归 feature store,
  controller 只路由」原则(见 state-management.md)的具体落地。
- **守门防跨 session 误触发**:streamController 是全局监听(收所有 session
  的 tool:call),store 入口必须检查 sessionId/slug 是当前活跃的,否则 return。

### 何时用这个模式 vs 后端事件

- 用这个模式:工具调用本身就是数据变更的唯一来源,且工具不发领域事件
  (如 write_file 是通用工具,不为单一 feature 发事件)。
- 用后端事件(`transport.listen("<domain>:<event>")`):数据变更有多个来源
  (含后端自发、外部编辑、git checkout),或后端有明确的领域事件语义
  (如 `subagent:finished` / `permission:ask`)。subagentRuns/permissions/
  projects store 都是后端事件驱动。
- **否决**:给通用工具(write_file)加路径识别发专用事件 —— blast radius 大
  (C2 design §2 否决);让 store self-listen tool:call —— 重复监听 + 丢上下文。
