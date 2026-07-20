# Research: `useErrorBus.routeByCategory` 5 stub 实际调用面与 toast 接入范围

- **Query**: `useErrorBus.routeByCategory` 5 个 stub 实际被哪些地方调用?是"全局兜底"还是"主链路"?
- **Scope**: internal (前端 error 总线)
- **Date**: 2026-07-17

## Findings

### Files Found

| File Path | Description |
|---|---|
| `app/src/utils/useErrorBus.ts:138-181` | `routeByCategory` + 5 stub(`showAuthToast` / `showRateLimitToast` / `showInlineError` / `showServerToast` / `showNetworkToast`)— 全部 `console.warn` |
| `app/src/utils/useErrorBus.ts:49-77` | `useErrorBus()` factory — 模块级单例 `errors: ref<AppCommandError[]>([])`,FIFO 50 |
| `app/src/utils/useErrorBus.ts:98-117` | `parseAppCommandError()` — 3 类输入容错解析 |
| `app/src/utils/useErrorBus.ts:128-134` | `extractErrorMessage()` — 统一错误消息提取 |
| `app/src/main.ts:17-27` | **唯一** `useErrorBus()` 调用点 — `window.addEventListener("error", ...)` + `"unhandledrejection", ...)` |
| `app/src/stores/streamController.ts:1147-1220` | ChatEvent::Error 主链路处理(`case "error"`),**不走 useErrorBus** |
| `app/src/stores/streamController.ts:2185-2192` | `invoke("chat", ...)` 同步失败 catch — **不走 useErrorBus** |
| `app/src/stores/chat.ts` | 36 处 IPC 调用全部自 `try/catch + projectsStore.showToast`,**不走 useErrorBus** |
| `app/src/stores/projects.ts:55-92` | `showToast` + 模块级 `toastTimer` |
| `app/src/components/layout/AppShell.vue:73-81` | 唯一 toast 渲染点 |

### Code Patterns

#### Pattern 1: 5 stub 全列表(本期实现 = `console.warn`)

`app/src/utils/useErrorBus.ts:138-181`:

```ts
function routeByCategory(err: AppCommandError): void {
  switch (err.category) {
    case "Auth":           showAuthToast(err); break;
    case "RateLimit":      showRateLimitToast(err); break;
    case "InvalidRequest": showInlineError(err); break;
    case "Server":         showServerToast(err); break;
    case "Network":        showNetworkToast(err); break;
  }
}

// TODO(A5 follow-up):接 reka-ui toast。当前 stub 只 console.warn,
// 保证错误不静默(开发可见)+ errors 数组收纳(组件可 watch 渲染)。
function showAuthToast(err: AppCommandError): void {
  // Auth 路由:引导去 Settings 检查 ANTHROPIC_API_KEY。
  console.warn(`[errorBus:Auth] ${err.message}`, err);
}
function showRateLimitToast(err: AppCommandError): void {
  console.warn(`[errorBus:RateLimit] ${err.message}`, err);
}
function showInlineError(err: AppCommandError): void {
  // InvalidRequest 由调用点 watch errors 决定内联渲染(表单字段下),不全局 toast。
  console.warn(`[errorBus:InvalidRequest] ${err.message}`, err);
}
function showServerToast(err: AppCommandError): void {
  // retryable=true 时附"重试"按钮(follow-up)。
  console.warn(`[errorBus:Server] ${err.message}`, err);
}
function showNetworkToast(err: AppCommandError): void {
  console.warn(`[errorBus:Network] ${err.message}`, err);
}
```

- **5 stub 设计意图**:
  | Category | 设计意图 | 实际行为 |
  |---|---|---|
  | Auth | 引导去 Settings | console.warn |
  | RateLimit | toast(消息) | console.warn |
  | InvalidRequest | inline(表单字段下)— 不全局 toast | console.warn |
  | Server | toast + 重试按钮 | console.warn |
  | Network | toast(检查网络) | console.warn |
- **每 stub 都标了"follow-up"注释**(line 158 显式 TODO),A5 R3.4 是路由键定义,真实 toast UI 留待后续 task。
- **`errors` ref 数组**(`useErrorBus.ts:46-47`)目前**没有任何 `watch` 消费者** — 主链路完全不读它。

#### Pattern 2: 唯一调用面 — `main.ts` 全局兜底

`app/src/main.ts:5, 17-27`:

```ts
import { useErrorBus } from "./utils/useErrorBus";
// ...
if (typeof window !== "undefined") {
  const { handle } = useErrorBus();
  window.addEventListener("error", (event) => {
    // event.error 是 Error 对象(或 undefined);fallback 到 event.message(string)。
    handle(event.error ?? event.message);
  });
  window.addEventListener("unhandledrejection", (event) => {
    // event.reason 是 rejection 原因(AppCommandError 对象 / Error / string)。
    handle(event.reason);
  });
}
```

- **唯一 `useErrorBus()` 调用点**。
- **监听 2 类全局事件**:`window.onerror`(JS 运行时错误)+ `unhandledrejection`(漏掉 `.catch` 的 promise rejection)。
- **典型触发场景**:
  - 任何 `invoke(...).then(...).catch(...)` 中途忘加 `.catch` 的 promise rejection
  - `setTimeout` 回调里 throw
  - 组件 setup() 内 throw(部分情况)
  - 第三方 lib bug
- **`handle` 内部**:`parseAppCommandError` 解析 → `null` 时静默丢弃(非 AppCommandError 形状);否则 `push(err)` → 触发 `routeByCategory` → 5 stub → `console.warn`。

#### Pattern 3: 主链路 — ChatEvent::Error **不走 useErrorBus**

`app/src/stores/streamController.ts:1147-1220`:

```ts
case "error":
  last.streaming = false;
  last.error = {
    message: event.message ?? "未知错误",
    category: event.category ?? "server",
  };
  last.retrying = undefined;
  // ... latency 记录 ...
  if (last.toolCalls) markRaw(last.toolCalls);
  if (last.toolResults) markRaw(last.toolResults);
  if (last.thinkingBlocks) markRaw(last.thinkingBlocks);
  if (last.redactedThinkingData) markRaw(last.redactedThinkingData);
  useChatStore().forceFollowActive = false;
  finalizeRequest(req.requestId, req.sessionId, true);
  break;
```

`app/src/stores/streamController.ts:2185-2192`(同步 IPC 失败 catch):

```ts
} catch (e) {
  const msgs = messagesBySession.get(args.sessionId);
  if (msgs) {
    const last = msgs[msgs.length - 1];
    if (last && last.role === "assistant") {
      last.streaming = false;
      last.error = { message: extractErrorMessage(e), category: "server" };
    }
  }
  finalizeRequest(requestId, args.sessionId, true);
}
```

- **ChatEvent::Error 直接写 `last.error` 字段**(in-memory placeholder),由 `<MessageItemFooter>` 渲染。
- **不调用 `useErrorBus().handle(e)`** — 即便 IPC rejection 是 `AppCommandError` 对象(有完整 wire 字段),也只走 `extractErrorMessage` 取 `e.message`。
- **关键观察**:**主链路 100% 自消费**;即便 `useErrorBus()` 接了 toast,**ChatEvent::Error 不会触发任何 toast**。

#### Pattern 4: 36 处 IPC `try/catch` — 各自 showToast,**不走 useErrorBus**

| 调用点 | 文件:行 | catch 行为 |
|---|---|---|
| `projects.ts:137-143` | ensureRefreshListener | `console.error("ensureRefreshListener failed:", e)` |
| `projects.ts:170-180` | pick_project_dir | `showToast('添加项目失败: ...', 'error')` |
| `projects.ts:229-232` | create_project | `showToast('添加项目失败: ...', 'error')` |
| `projects.ts:243-258` | hide_project | `showToast('关闭项目失败: ...', 'error')` |
| `projects.ts:269-282` | unhide_project | `showToast('重新打开项目失败: ...', 'error')` |
| `projects.ts:284-299` | rename_project | `showToast('重命名失败: ...', 'error')` |
| `chat.ts:709-713` | attachWorktree | `showToast('attach worktree 失败: ...', 'error')` + rethrow |
| `chat.ts:738-742` | publishSessionToMain | `showToast('publish 到 main 失败: ...', 'error')` + rethrow |
| `chat.ts:748-753` | detachWorktree | `showToast('detach worktree 失败: ...', 'error')` + rethrow |
| `chat.ts:769-774` | deleteWorktree | `showToast('delete worktree 失败: ...', 'error')` + rethrow |
| `chat.ts:1170-1225` | editMessage | (rethrow,parent 在 MessageItem.vue:848-860 catch + showToast) |
| `chat.ts:1257-1341` | resendMessage | (rethrow,parent 在 MessageItem.vue:803-809 catch + showToast) |
| `MessageItem.vue:803-809` | resend IPC | `showToast('重发失败: ...', 'error')` |
| `MessageItem.vue:854-860` | editMessage IPC | `showToast('编辑失败: ...', 'error')` |
| `MessageItem.vue:799, 885, 889` | resend guard | `showToast('重发失败: ...', 'error')` |
| `MessageActionsMenu.vue:190-193` | clipboard 失败 | `showToast('复制失败: ...', 'error')` |
| `ChatInput.vue:518` | /command 读取失败 | `showToast('命令 /X 读取失败: ...', 'error')` |
| `ChatPanel.vue:152` | fetchDiff | local `diffError.value = ...` (inline UI, 不 toast) |
| `WorktreeChip.vue:132` | clipboard 复制失败 | `showToast('复制失败: ...', 'error')` |
| `ModeSelect.vue:179, 209` | mode 切换失败 | `showToast` × 2 |
| `WorkerMergeControls.vue:153-180` | merge/discard 成功/失败 | `showToast` × 4 |
| `permissions.ts:344` | respond failed | `console.error(...)`(不 toast,store 内) |
| `ChatInput.vue:235-517` | 各种 command 失败 | `console.error(...)` |
| **合计 ~36 处** | | **0 处走 useErrorBus** |

- **每处 IPC 失败都自己 try/catch**,**全部走 `projectsStore.showToast`** 或 `console.error`,**完全没有 routeByCategory 参与**。
- **`useErrorBus.errors` FIFO 50 数组**(`useErrorBus.ts:47`)目前**没有任何 watch 消费者**。

#### Pattern 5: `extractErrorMessage` 是唯一跨模块复用面

`app/src/utils/useErrorBus.ts:128-134`:

```ts
export function extractErrorMessage(e: unknown): string {
  const parsed = parseAppCommandError(e);
  if (parsed) return parsed.message;
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return "(未知错误)";
}
```

- **27 处 import** `extractErrorMessage`(全仓 grep:`extractErrorMessage from .../useErrorBus`)— 唯一的"广用面"。
- **纯函数,无副作用**(不 push / 不 console);给 IPC catch 块一个统一文本提取接口。
- **scope B 不动**:这一层已经是稳定的契约。

### External References

- `.trellis/spec/backend/error-handling.md:117-120` — "前端:useErrorBus 收口 → parseAppCommandError → 按 category 路由 → Auth→Settings / RateLimit→toast / InvalidRequest→inline / Server→toast+重试 / Network→toast → 全局 window.onerror + unhandledrejection 兜底"。**spec 期望的最终态**就是 scope B 的目标。
- `.trellis/spec/backend/error-handling.md:172-174` — 5 类 category 默认 retryable + 前端路由建议(完整表)。
- `.trellis/spec/frontend/reka-ui-usage.md:619-634` — Toast motion token(`var(--duration-slow) var(--ease-out)`);scope B 接 toast 直接复用。
- `.trellis/spec/frontend/popover-pattern.md:619-622` — Toast 240ms 双向动画(已有约定)。

### Related Specs

- `.trellis/spec/backend/error-handling.md` §"前端 useErrorBus 收口"。

## Caveats / Not Found

1. **5 stub 实际只服务 1 个调用点**(`main.ts` 全局兜底) — 0 处业务代码走 `useErrorBus().handle()`。**scope B 接 toast 影响范围极小**(只覆盖"JS 运行时错误" + "漏 catch 的 promise rejection"两类),不覆盖任何业务 IPC 失败。
2. **主链路 `ChatEvent::Error` 完全不走 useErrorBus** — `streamController.ts:1147-1220` 直接写 `last.error`,由 `<MessageItemFooter>` 渲染。**scope B 若希望"chat 出错时弹全局 toast",需要新接一条线**:在 `case "error":` 里加 `useErrorBus().push(parsed)`(但 `parsed` 是 LlmErrorCategory 不是 AppCommandError,需要包一层)。
3. **36 处 IPC catch 各自 showToast** — 不走 routeByCategory,scope B 即使接 toast **不影响这 36 处**(它们的 toast 文案已经是 caller 自定义的 prefix: `'添加项目失败: ...'` / `'attach worktree 失败: ...'`)。
4. **`useErrorBus.errors` FIFO 50 数组无消费者**(`useErrorBus.ts:47`)— scope B 若需要"toast queue"(多个错误排队显示),可以让新的 `useToast()` composable watch `errors` ref 实现。但**目前是单 slot 架构**(projectsStore.toast),需重构为 queue。
5. **`extractErrorMessage` 是稳定的 API** — 27 处 import,scope B 不应破坏契约(可加新函数,但不改旧函数签名)。
6. **`routeByCategory` 5 stub 中 InvalidRequest 的注释**:`"由调用点 watch errors 决定内联渲染(表单字段下),不全局 toast"`(`useErrorBus.ts:170-171`)— 这条**spec 期望** scope B 不必实现(无 forms 接 inline error)。
7. **全局兜底**实际上是**最后一道防线** — 漏 catch 的 rejection 通常是 bug,不是用户主动操作;toast 出现时往往用户已经不在原地。scope B 接 toast 价值**有限但有价值**(把"页面 blank + devtools console"提升为"页面 + toast 提示")。

## 给 design.md 的输入

`routeByCategory` 5 stub 实际只服务 `main.ts:17-27` 1 个调用点(全局 `window.onerror` + `unhandledrejection`),**主链路 100% 不走 useErrorBus**(ChatEvent::Error 直接写 `last.error`,36 处 IPC 各自 try/catch + showToast)。scope B 接 toast 推荐**分两层**:**(a) 仅接全局兜底**(最低成本,改动量 ~20 行,只让 5 stub 调 `projectsStore.showToast` + 1 个 InvalidRequest 的 console 留作异常):Auth → toast + 引导文案;RateLimit → toast + "稍后重试"文案;Server → toast + "重试"按钮;Network → toast + "检查网络"文案;InvalidRequest → console.warn 保留(注释说"调用点 inline",scope B 无 forms 接 inline,保持现状);**(b) 可选**:在 `streamController.ts::case "error"` 末尾加 `useErrorBus().push({ category, message, kind, retryable, requestId })` 把 chat 流错误也推给 errorBus(由 stub 决定 toast),但需要把 `LlmErrorCategory` 转 `ErrorCategory`(5 类映射一一对应,见 `error.rs:41-51`),并且 stub 的 Server 路径要带 retry 按钮(消费 `chatStore.retryChat(sid, seq)`)。**推荐 (a) 单独实施,b OOS**。改动量 ~30-50 行(5 stub → 5 真分发 + 1 个 InvalidRequest 保留 + 1 个 ErrorCategory-toast mapping helper),复用 `projectsStore.showToast` + `--color-tool-error/shell` tokens + 240ms motion。