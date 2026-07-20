# A5 错误处理完善 — Design

> **状态**:draft(2026-07-17)。配套 `prd.md`(scope B + 1 PR),基于 `research/01-05.md` 5 份事实。

## 1. Architecture

### 1.1 边界(scope B)

| 范围 | 改动 | 范围外(显式) |
|---|---|---|
| **R1 全局错误 → reka-ui Toast** | `useToast.ts`(新) + `useErrorBus.routeByCategory` 5 stub 接 toast(全局兜底调用 1 处) | 主链路 36 处 IPC 错误展示 |
| **R2 retryable 消费** | `MessageItemFooter.vue` + `chatStore.retryChat()`(新) + `error.ts` 增加 `categoryRetryable()` helper | 3 个其他错误渲染点(MessageItemEdit / ToolCallCard / SubagentDrawerErrorCard) |

显式不动的边界:
- **后端 wire**:`AppCommandError.retryable` 已透传,`ChatEvent::Error` 不带 retryable(正确,避免消息体膨胀,前端从 `category` 派生)
- **业务逻辑**:`useErrorBus.routeByError` / `routeByString` 现有调用源
- **错误分类**:`ErrorCategory` 5 类不增不减,InvalidRequest 路由策略保留(本地错误不打扰)
- **样式系统**:toast 走项目 CSS variables,不复用 settings 弹窗的样式 token(语义不同)

### 1.2 模块拓扑

```
┌─ app/src/utils/useErrorBus.ts (改)  ─────────────┐
│  routeByCategory(category, message)               │
│   ├─ Auth/RateLimit/Server/Network → useToast     │
│   └─ InvalidRequest → console.warn(原状)          │
└──────────────────────────────────────────────────┘
                       │
                       ▼
┌─ app/src/composables/useToast.ts (新) ───────────┐  ←── Vue composable
│  - toasts: Ref<Toast[]> (LIFO queue)              │
│  - show(category, message, opts?)                  │
│  - dismiss(id)                                     │
│  - dedupe 策略:同 category+message 5s 内不重复     │
│  - max 3 并发,溢出 FIFO                            │
└──────────────────────────────────────────────────┘
                       │
                       ▼
┌─ app/src/components/common/ToastProvider.vue (新) ─┐  ←── reka-ui 2.9.9
│  ToastRoot / ToastProvider / ToastViewport /       │
│  ToastPortal / ToastTitle / ToastDescription /     │
│  ToastClose(自动 dismiss)                          │
└───────────────────────────────────────────────────┘

┌─ app/src/stores/chat.ts (改)  ─────────────────────┐
│  新增 retryChat(sessionId, messageSeq)             │
│   - 复制 resendMessage 结构(chat.ts:1257-1341)     │
│   - 关键差异:不 push placeholder / mutate errored   │
│     assistant / 不带 resendSeq / 用 max+1 seq      │
└──────────────────────────────────────────────────┘
                       │
                       ▼
┌─ app/src/components/chat/MessageItemFooter.vue (改) ┐
│  当 categoryRetryable(category)===true:            │
│    <button>↻ 重试</button>                          │
│  loading 态:点击 → disabled, 文本切 "重试中..."    │
│  复位:session 流 enter 事件 → loading = false       │
└───────────────────────────────────────────────────┘
```

## 2. Contracts

### 2.1 `useToast()` composable API

```ts
// app/src/composables/useToast.ts
export type ToastCategory = 'Auth' | 'RateLimit' | 'Server' | 'Network';

export interface Toast {
  id: string;            // uuid
  category: ToastCategory;
  title: string;         // 短标题,如 "鉴权失败"
  description?: string;   // 详情,可空
  createdAt: number;     // Date.now()
  ttl: number;           // 默认 5000ms
}

export interface UseToastReturn {
  toasts: Ref<Toast[]>;
  show: (input: { category: ToastCategory; title: string; description?: string; ttl?: number }) => string | null; // 返回 id 或 null(deduped)
  dismiss: (id: string) => void;
  clear: () => void;
}

export function useToast(): UseToastReturn;
```

不变量:
- `toasts.length <= MAX_CONCURRENT`(默认 3)
- 同 `(category, title, description?)` 在 `DEDUPE_WINDOW`(默认 5s)内只弹 1 次
- Toast 自动 `ttl` 到期 → 自动 `dismiss`(由 `setTimeout` 触发,`onUnmounted` 清理)

### 2.2 `useErrorBus.routeByCategory` 新签名

```ts
// app/src/utils/useErrorBus.ts (改 5 stub)
export type ErrorCategory = 'Auth' | 'RateLimit' | 'Server' | 'Network' | 'InvalidRequest';

export interface RouteByCategoryInput {
  category: ErrorCategory;
  message: string;
  cause?: unknown;       // 透传给 console.warn(InvalidRequest 路径)
}

// 内部依赖 useToast()(在 setup 阶段初始化一次,作为 module-level 单例 OR composable 内部 lazy init)
// module-level 单例优先 — 避免 main.ts 接入时需手动 setup
```

### 2.3 `categoryRetryable()` helper

```ts
// app/src/utils/error.ts (新文件 OR 扩展现有)
export function categoryRetryable(category: ErrorCategory): boolean {
  // 与后端 AppError::retryable() 默认派生保持一致
  // 后端 llm/error.rs:62-70 映射:
  return category === 'RateLimit' || category === 'Server' || category === 'Network';
  // Auth / InvalidRequest → false
}
```

### 2.4 `chatStore.retryChat()` 签名

```ts
// app/src/stores/chat.ts (新)
async function retryChat(sessionId: string, messageSeq: number): Promise<void> {
  // 1. 找当前 session + messageSeq 对应的 errored assistant 消息
  // 2. mutate 它的 status: 'error' → 'streaming'(临时)
  // 3. 计算新 requestId(uuid)
  // 4. 重建 messages 上下文(用 session.messages[:messageSeq+1] 截到 errored 那条)
  // 5. invoke('chat', { requestId, sessionId, messages, cwd }) — 同 resendMessage
  // 6. 不写 new placeholder;不写 resendSeq(用户级 retry,不是消息编辑)
}
```

与 `resendMessage` 关键差异(代码 review 时强校验):
| 维度 | resendMessage | retryChat |
|---|---|---|
| push placeholder | ✅ 是 | ❌ 否(mutate 已有) |
| 序列号 | 用 next seq(max + 1) | 用原 messageSeq(复用同位置) |
| resendSeq 标记 | ✅ 是(消息编辑) | ❌ 否(用户级 retry) |
| errored 消息处理 | 保留(用户编辑原消息) | 替换(mark errored → streaming) |

## 3. Data Flow

### 3.1 全局错误 → toast(R1)

```
[Vue app boot]
  main.ts
    ├─ 注册 window.onerror(原 useErrorBus.routeByString)
    │    ↓
    │  parseAppCommandError(error) → AppCommandError | null
    │    ↓
    │  routeByCategory(category, message)
    │    ├─ Auth/RateLimit/Server/Network → useToast().show(...)
    │    └─ InvalidRequest → console.warn(category, message, cause)
    │
    └─ 注册 unhandledrejection(原 useErrorBus.routeByString)
         ↓
         同上路径
```

### 3.2 retryable 消费(R2)

```
[用户点 ↻ 重试]
  MessageItemFooter.vue
    ├─ onClick: chatStore.retryChat(sessionId, messageSeq)
    │             ↓
    │           mutate errored → { status: 'streaming', text: '', error: null }
    │             ↓
    │           invoke('chat', { requestId, sessionId, messages: session.messages[:messageSeq+1], cwd })
    │             ↓
    │           streamController.startRequest(requestId, ...)
    │             ↓
    │           ChatEvent::Delta / ChatEvent::Done / ChatEvent::Error
    │             ↓
    │           runAccumulator 增量更新 message.text / status
    │
    └─ watch(session.activeRequestId): 流 enter → loading = false
                                          流 done/error → loading = false
```

边界:
- **重试触发时,已有 errored 消息**:mutate 状态而非 push 新条目 — 视觉上是"原地重试",不是"重发新消息"
- **重试期间用户又点重试**:button disabled(loading 态)防双发
- **重试失败又 emit ChatEvent::Error**:`MessageItemFooter` 重新显示 error + retry button(R1 兜底不弹新 toast,避免重复;由错误信息在卡片内显示)

## 4. Tradeoffs

### 4.1 reka-ui Toast vs 自研全量 vs 第三方 lib

| 选项 | 改动量 | 风险 | 选 |
|---|---|---|---|
| **reka-ui 2.9.9 Toast primitive(已装)** | 0 新依赖,~150-250 行 composable + 1 provider 组件 | 低(reka-ui 2.9.9 Toast API 已 stable) | ✅ |
| 自研 CSS-only toast | ~200-300 行 + a11y 工作(ESC / focus trap) | 中(a11y 容易漏) | ❌ |
| vue-toastification 第三方 | 0 自研代码 | 高(新依赖 + bundle size + 风格入侵) | ❌ |

**选 reka-ui**:项目已锁定 2.9.9,Toast primitive 完整可用,样式 token 与现有 Dialog/Menu 一致。

### 4.2 useToast() 放 module-level 单例 vs composable 注入

| 选项 | 调用方式 | 优 | 劣 |
|---|---|---|---|
| **module-level 单例** | `import { useToast } from '@/composables/useToast'; useToast().show(...)` | 与 `useErrorBus` 风格一致;`main.ts` 全局兜底不需 setup | 测试需 mock module;非 Vue 上下文也能用 |
| composable 注入 | `const { show } = useToast()` | 标准 Vue 风格;测试易 | `main.ts` 全局兜底需先 setup,绕 |

**选 module-level 单例**:`useErrorBus` 已是这个模式(`research/01` 方案 A),一致优先;`main.ts` 接入零摩擦。

### 4.3 ChatEvent::Error 不带 retryable vs 带

`research/02` 已确认:**前端从 `category` 派生 `retryable`**,因为:
- 后端 `AppError::retryable()` 默认派生规则与 `category` 1:1(Auth→false, RateLimit/Server/Network→true, InvalidRequest→false)
- 加 `retryable` 到 wire 字段 → SSE 消息体膨胀 + 与 category 重复
- 派生逻辑仅 ~3 行,可单测全盖

**结论**:不动后端 wire,纯前端 `categoryRetryable(category)` helper。

### 4.4 重试按钮放 MessageItemFooter vs 抽公共组件

| 选项 | 复用 | 改 | 选 |
|---|---|---|---|
| **MessageItemFooter 内联 button** | 0 复用(仅此 1 处用) | ~40 行 | ✅ |
| 抽 `MessageRetryButton.vue` 公共组件 | 未来 L3b worker retry 可复用 | ~60 行 + 1 文件 | ❌(scope B 不为未来买单) |

`research/03` 明确:**仅 MessageItemFooter 1 处渲染**,无需抽公共。

## 5. Compatibility

### 5.1 向后兼容

- **现有 `useErrorBus.routeByCategory` 调用方**:`main.ts:17-27` 1 处。新签名只是内部路由策略变,导出符号不变,外部调用零影响。
- **现有 IPC 错误展示**:`MessageItemFooter.vue:128-137` 红字 inline 保留(R1 仅加 toast 兜底,不动 inline);`ToolCallCard` / `MessageItemEdit` / `SubagentDrawerErrorCard` 不动。
- **现有 reka-ui 2.9.9 Toast 使用方**:`reka-ui-usage.md` 列出的 Dialog/Tabs/Select/Checkbox/RadioGroup/Label 7 类不增不减,新增 Toast 是第 8 类。
- **Tauri 后端**:零改动。

### 5.2 性能影响

- **Toast queue**:max 3 + dedupe 5s — 内存上限 ~5 条 Toast 对象,DOM 增 3 个 `<ToastRoot>`,可忽略
- **`useErrorBus` 全局监听**:原状,只是内部从 `console.warn` 切到 `useToast().show()`,单次开销 O(1)
- **`retryChat`** 复用 `resendMessage` 路径,无新增 IPC 类别

### 5.3 测试覆盖

- `useToast` composable 单测:`vitest` `@/composables/useToast.test.ts`
  - show / dismiss / clear 三操作
  - queue max 3 + FIFO 溢出
  - dedupe 同 `(category, title, description)` 5s 内只 1 次
  - ttl 自动到期
- `MessageItemFooter` retry button 单测:`vitest` `@/components/chat/MessageItemFooter.test.ts`(新)
  - retryable 显隐(categoryRetryable 派生 5 类)
  - 点击触发 `chatStore.retryChat`
  - loading 态(synthetic session 流 enter 事件 → 复位)
- `categoryRetryable` helper 单测:`@/utils/error.test.ts`(新或扩展)
  - 5 类 × 期望值表

## 6. Rollout / Rollback

### 6.1 Rollout

1. 1 PR 一次 merge(用户已确认 1 PR)
2. Merge 后:
   - 重启 Tauri(前端 `useToast` 注入,`main.ts` 启动 hook)
   - 验证:dev 模式打开 devtools 跑 `throw new Error('test')` → 屏幕右上 toast 出现
3. 默认启用(R1 全局兜底是新增能力,不是 toggle 开关)

### 6.2 Rollback

- **R1 关闭**:commit revert 一行(`useErrorBus.routeByCategory` 回到 `console.warn`);toast 组件保留(无副作用)
- **R2 关闭**:commit revert `MessageItemFooter` button + `chatStore.retryChat`;无其他耦合点
- **R1 + R2 同时关闭** = revert 整 PR

### 6.3 Feature flag(预留,不在本 PR)

若未来想灰度 toast(避免全量上线扰民),可加 `localStorage['error-toast-enabled']` 开关。本 PR 不引入,留 V3 评估。
