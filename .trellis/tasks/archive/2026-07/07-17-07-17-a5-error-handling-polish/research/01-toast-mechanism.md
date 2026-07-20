# Research: Toast 机制现状与可复用面

- **Query**: 项目目前是怎么给用户做"瞬时提示"的?有现成 reka-ui Toast primitive 吗?还是自研?scope B 应走哪条路?
- **Scope**: internal (前端 + 后端 IPC 边界)
- **Date**: 2026-07-17

## Findings

### Files Found

| File Path | Description |
|---|---|
| `app/src/utils/useErrorBus.ts` | 全局错误总线(A5 主体),5 类路由分发(routeByCategory)当前全 `console.warn` stub |
| `app/src/stores/projects.ts` | **唯一**的 toast 状态持有者(`toast` ref + `showToast` action),无 toast lib |
| `app/src/components/layout/AppShell.vue` | 顶部容器,挂载 `<transition name="toast">` 渲染 `projectsStore.toast` |
| `app/src/main.ts` | 全局 `window.onerror` / `unhandledrejection` → `useErrorBus().handle(e)` |
| `app/src/stores/streamController.ts` | ChatEvent::Error 处理(`case "error":` line 1148-1220)— 把 `event.category` 写入 `last.error` |
| `app/package.json` | 依赖锁定:reka-ui ^2.9.9 + 无 toast lib / 无 sonner / 无 vue-toastification 等 |
| `app/node_modules/reka-ui/dist/Toast/` | reka-ui 2.9.9 **有** Toast primitive(`ToastRoot / ToastProvider / ToastViewport / ToastPortal / ToastAction / ToastClose / ToastTitle / ToastDescription`)|
| `app/node_modules/reka-ui/dist/index.d.ts` | Toast 类型签名齐全(`ToastRootProps`, `ToastProviderProps`, `ToastActionProps`, ...) |

### Code Patterns

#### Pattern 1: 现有 toast 机制 — projects store 单例 ref + AppShell fixed-position 渲染

`app/src/stores/projects.ts:41-92`:

```ts
export type ToastKind = "info" | "warn" | "error";
export interface ToastMessage {
  message: string;
  kind: ToastKind;
  sessionId?: string;  // 2026-07-08 cross-session-pending 扩展
}

let toastTimer: number | null = null;
const toast = ref<ToastMessage | null>(null);

function showToast(
  message: string,
  kind: ToastKind = "info",
  durationMs = 3500,
  opts?: { sessionId?: string },
): void {
  toast.value = { message, kind, sessionId: opts?.sessionId };
  if (toastTimer !== null) {
    window.clearTimeout(toastTimer);
  }
  toastTimer = window.setTimeout(() => {
    toast.value = null;
    toastTimer = null;
  }, durationMs);
}
```

- **单一全局 toast**,不是 toast stack(queue);新 toast 直接覆盖旧的(单 slot)。
- **固定 3500ms TTL**,`dismissToast()` 手动关闭;无 progress bar / 无 pause-on-hover。
- **kind: 三档**(`info` / `warn` / `error`),后端无对应语义映射(目前纯字符串提示)。
- **`sessionId` 扩展**(`cross-session-pending-indicator`,2026-07-08):点击 toast 跳转到对应 session;project 操作 toast 不带 sessionId → 仅 dismiss。

`app/src/components/layout/AppShell.vue:73-81`(渲染):

```vue
<transition name="toast">
  <div
    v-if="projectsStore.toast"
    :class="['toast', `toast--${projectsStore.toast.kind}`]"
    @click="onToastClick"
  >
    {{ projectsStore.toast.message }}
  </div>
</transition>
```

CSS 在 `app/src/components/layout/AppShell.vue:111-154`:

```css
.toast {
  position: fixed;
  bottom: var(--space-6);
  left: 50%;
  transform: translateX(-50%);
  padding: 10px 18px;
  border-radius: var(--radius-lg);
  background: var(--color-bg-elevated);
  color: var(--text-primary);
  font-size: var(--text-base);
  box-shadow: var(--shadow-md);
  cursor: pointer;
  max-width: 80vw;
  z-index: 9999;
  border: 1px solid var(--color-bg-border);
}
.toast--error { background: var(--color-tool-error); ... }
.toast--warn { background: var(--color-tool-shell); ... }
.toast--info { background: var(--color-accent); ... }
/* 240ms slow enter/leave */
```

#### Pattern 2: useErrorBus 5 类 stub(本期空实现)

`app/src/utils/useErrorBus.ts:138-181`:

```ts
function routeByCategory(err: AppCommandError): void {
  switch (err.category) {
    case "Auth":      showAuthToast(err); break;
    case "RateLimit": showRateLimitToast(err); break;
    case "InvalidRequest": showInlineError(err); break;
    case "Server":    showServerToast(err); break;
    case "Network":   showNetworkToast(err); break;
  }
}

// TODO(A5 follow-up):接 reka-ui toast。当前 stub 只 console.warn
function showAuthToast(err: AppCommandError): void {
  console.warn(`[errorBus:Auth] ${err.message}`, err);
}
// ... 其他 4 个 stub 同样 console.warn
```

**Stub 内部**完全 `console.warn`,**没**改 `projectsStore.toast`,所以 `AppShell` 的 `<transition>` 不会渲染任何东西。TODO 注释(`useErrorBus.ts:158`)明确写"接 reka-ui toast 在 follow-up 任务",本期没人接。

#### Pattern 3: reka-ui 2.9.9 Toast primitive 实际存在

`app/node_modules/reka-ui/dist/index.d.ts` 类型签名 + `dist/Toast/` 目录都有:

```
ToastAction, ToastActionProps,
ToastClose, ToastCloseProps,
ToastDescription, ToastDescriptionProps,
ToastPortal, ToastPortalProps,
ToastProvider, ToastProviderContext, ToastProviderProps,
ToastRoot, ToastRootEmits, ToastRootProps,
ToastTitle, ToastTitleProps,
ToastViewport, ToastViewportProps
```

API 形状对照(reka-ui 2.9.9,Radix 风格 Vue 移植):

```vue
<ToastProvider :duration="5000" :swipe-direction="'right'">
  <ToastRoot class="toast" :open="true" @update:open="...">
    <ToastTitle>Title</ToastTitle>
    <ToastDescription>Body</ToastDescription>
    <ToastAction alt-text="Retry" @click="onRetry">Retry</ToastAction>
    <ToastClose>×</ToastClose>
  </ToastRoot>
  <ToastViewport />
</ToastProvider>
```

- **六件套**(与 Tooltip / DropdownMenu 同构):`ToastProvider` + `ToastRoot` + `ToastPortal` + `ToastViewport` + `ToastTitle` + `ToastDescription`;**缺一不可**(`ToastProvider` 必须包,否则 `Injection Symbol(ToastProviderContext) not found` 运行时崩 — 见 `.trellis/spec/frontend/reka-ui-usage.md` 第 422 行起的 TooltipProvider 教训)。
- **`ToastViewport` portal 到 `<body>`**(`ToastContent / ToastViewport` portal),`:deep()` 强制(`reka-ui-usage.md` 第 178 行起)。
- **支持 swipe-to-dismiss**(`swipe-direction`)、自动关闭(`duration` ms)、`ToastAction` 内嵌按钮(可作 retry)。
- **多个 toast 同框**:项目现状是单 slot;若改 reka-ui,需要用 toast store queue(`useToast()` composable 内推/弹)。

#### Pattern 4: 调用频次统计 — 实际 `showToast` 调用点

全仓 grep `projectsStore.showToast` / `.showToast(`:

| 调用点 | 频次 | 类别 |
|---|---|---|
| `app/src/stores/chat.ts` | 5(attachWorktree / publishSessionToMain / detachWorktree / deleteWorktree / editMessage + resend) | IPC 失败 |
| `app/src/stores/projects.ts` | 7(addProject / hideProject / unhideProject / renameProject × 2) | project 操作 |
| `app/src/components/chat/MessageItem.vue` | 5(handleSave / handleResend × 2 / onResend × 2) | 用户消息编辑 / 重发 |
| `app/src/components/chat/MessageActionsMenu.vue` | 2(复制成功 / 复制失败) | clipboard 反馈 |
| `app/src/components/chat/ChatInput.vue` | 2(命令读取失败 / 模板为空) | B3 command |
| `app/src/components/chat/ChatPanel.vue` | 3(worktree attach / detach / delete) | worktree 操作 |
| `app/src/components/chat/WorktreeChip.vue` | 2(复制成功 / 失败) | clipboard |
| `app/src/components/chat/ModeSelect.vue` | 2(mode 切换) | mode 操作 |
| `app/src/components/chat/WorkerMergeControls.vue` | 4(merge / discard success + fail) | L3b worker |
| `app/src/stores/permissions.ts` | 1(120s timeout 自动拒绝) | 权限超时 |
| `app/src/components/chat/primitives/ButtonPrimitive.vue` | 3(apply_diff / copy success / fail) | B9 UiCard |
| **合计** | **~36 处**(单向,无并发) | 全是 success / fail / 操作反馈 |

`useErrorBus().handle(...)` 全仓只有 1 处真调用(`main.ts:18`) — 专门处理 `window.onerror` + `unhandledrejection` **全局兜底**。其余 IPC 错误全部走各自的 `try/catch + showToast`。

### External References

- `.trellis/spec/frontend/reka-ui-usage.md:13` — "Pinned reka-ui version: 2.9.9";明确 TooltipProvider / DropdownMenuRoot 必须 wrap 的"`Injection Symbol(...) not found`"教训。
- `.trellis/spec/frontend/reka-ui-usage.md:127-176` — "Sheet 不存在 2.9.9"的同款版本陷阱,但 Toast **不在 trap 列表里**(已确认存在)。
- `.trellis/spec/frontend/popover-pattern.md:619-634` — "Animation: Toast 用 `var(--duration-slow) var(--ease-out)` (240ms)" — 已记入 popover 文档的"其它弹窗"段,可直接复用 timing。
- `.trellis/spec/frontend/popover-pattern.md:706-799` — `ConfirmDialog` 手写 modal 范例(无 toast lib)。
- `.trellis/spec/backend/error-handling.md:118-119` — "前端:useErrorBus 收口 → 按 category 路由(Auth→Settings / RateLimit→toast / InvalidRequest→inline / Server→toast+重试 / Network→toast)" — 这是 spec 期望的最终态。
- `docs/IMPLEMENTATION.md §4 2026-07-02` — A5 主体的 commit `bab84bd`(目前 `routeByCategory` stub 仍是 follow-up)。

### Related Specs

- `.trellis/spec/backend/error-handling.md` — `AppCommandError` schema(category / kind / message / retryable / requestId 5 字段)+ 5 类路由建议(Auth/RateLimit/Server/Network→ toast,InvalidRequest→ inline)。
- `.trellis/spec/frontend/reka-ui-usage.md` — Toast primitive 使用守则(provider 必 wrap / portal `:deep()`)。
- `.trellis/spec/frontend/popover-pattern.md` — 自研 popover 范例(若决定不接 reka-ui Toast,可仿此写一个 `<ToastContainer>`)。

## Caveats / Not Found

1. **reka-ui Toast 在 2.9.9 实际可用** — `index.d.ts` 类型 + `dist/Toast/*.js` 双确认。但项目代码库**目前 0 处**直接 import Toast(只有 Tooltip / Dialog / DropdownMenu / Popover / Select / Tabs / Checkbox / RadioGroup / Label),所以"项目是否真在用 toast"答案 = **没在用**,只是底层 primitive 在。
2. **`projectsStore.toast` 是单 slot,不是 queue** — 现有架构不支持"3 个错误同时来"的不丢失(后到的覆盖先到的)。`useErrorBus.errors` 是 50 长 FIFO(`useErrorBus.ts:46-47`),但路由 stub 没消费它去排队 toast。
3. **`showToast` 36 处调用全部 fire-and-forget 文本提示,0 处需要 retry 按钮** — 现状是 "info / warn / error" 3 档纯文案。要支持"Server/Network 出错时给用户一个 retry 按钮",需要**新加 `retryable?: () => void` 字段**到 ToastMessage(配合 `AppCommandError.retryable` 字段,见 research/02-retryable-wire-flow.md)。
4. **`window.confirm/alert/prompt` 在 Tauri webview 静默 no-op**(`popover-pattern.md:837-883`)— toast 是 in-app DOM,不受影响。
5. **样式 token 已就绪**:`--color-tool-error / --color-tool-shell / --color-bg-elevated / --color-bg-surface / --color-bg-border / --shadow-md / --duration-slow / --ease-out` 全部现成(`popover-pattern.md:619-622` "Toast (AppShell)" row)。
6. **未找到 `notification` API 调用** — 无 `new Notification(...)` / 无 service worker push,toast 是唯一的瞬时提示通道。

## 给 design.md 的输入

项目现状:reka-ui 2.9.9 有完整 Toast primitive(`ToastRoot/ToastProvider/ToastViewport/ToastPortal/ToastAction/ToastClose/ToastTitle/ToastDescription`),但代码库 **0 处 import**;`projectsStore.toast` 是单 slot 自研实现,`useErrorBus.routeByCategory` 5 类 stub 全 console.warn。scope B 推荐**方案 A:基于 reka-ui Toast 自研一个 `useToast()` composable + 升级 `projectsStore.toast` 为 queue**,复用 reka-ui 6 件套(provider 必须 wrap,portal `:deep()` 必须),把 `useErrorBus.errors` FIFO 接入 `useToast().push`,Auth → 引导去 Settings,Server/Network → 带 Retry 按钮(消费 `AppCommandError.retryable`)。改动量 ~150-250 行(1 个 composable + 1 个 `<ToastViewport>` mount + 5 个 stub → 真分发)。**复用清单**:reka-ui Toast 6 件套、`--color-tool-error/shell/accent` token、`--duration-slow var(--ease-out)` motion、`popover-pattern.md` 的 `:deep()` 规则、`extractErrorMessage()` 文本兜底。