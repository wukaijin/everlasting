# Design:Transport 抽象 + emit 散点收敛

> 配套 prd.md 的 R1-R7,给出技术契约和 trait 设计。

## 1. 前端 Transport 接口

### 1.1 接口契约

```typescript
// app/src/transport/types.ts
export interface Transport {
  /**
   * 调用一个后端 command(对应 #[tauri::command] / HTTP handler)。
   * @param cmd command 名(如 'chat' / 'load_session')
   * @param args 参数对象,字段保持 snake_case(与 Rust 端 + AppCommandError 一致)
   * @throws AppCommandError(序列化形态与 Rust 端一致,见 RESEARCH §1.2c)
   */
  invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>;

  /**
   * 订阅一个事件(对应 app.emit 事件)。
   * @param event 事件名(如 'chat-event' / 'permission:ask')
   * @param handler 事件回调,payload 是反序列化后的对象
   * @returns unlisten 函数(取消订阅)
   *
   * 语义:Tauri 全局广播,SSE 按 session 订阅 —— 错位在 httpTransport 内部消化
   * (httpTransport 维护单个全局 EventSource + 事件名→handler 分发表)
   */
  listen<T = unknown>(event: string, handler: (payload: T) => void): Promise<() => void>;
}
```

### 1.2 模块布局

```
app/src/transport/
├── types.ts        # Transport interface + 相关类型
├── tauri.ts        # tauriTransport 实现(包装 invoke/listen)
├── http.ts         # httpTransport stub(Phase 2 填充)
├── index.ts        # 默认 export:isTauri() ? tauriTransport : httpTransport
└── transport.test.ts  # tauriTransport 转发逻辑测试
```

### 1.3 tauriTransport 实现

```typescript
// app/src/transport/tauri.ts
import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { listen as tauriListen } from '@tauri-apps/api/event';
import type { Transport } from './types';

export const tauriTransport: Transport = {
  invoke: (cmd, args) => tauriInvoke(cmd, args),
  listen: (event, handler) => tauriListen(event, (e) => handler(e.payload)),
};
```

> **注意**:`tauriListen` 的回调收到的是 `Event<T>`(`{ event, id, payload }`),transport 层**只透传 payload**,与现有调用方的预期一致(现有代码都是 `(e) => e.payload`)。

### 1.4 httpTransport stub

```typescript
// app/src/transport/http.ts
import type { Transport } from './types';

export const httpTransport: Transport = {
  invoke: async (_cmd, _args) => {
    throw new Error('httpTransport not implemented (Phase 2)');
  },
  listen: async (_event, _handler) => {
    throw new Error('httpTransport not implemented (Phase 2)');
    return () => {}; // unreachable,类型满足
  },
};
```

### 1.5 默认 transport 选择

```typescript
// app/src/transport/index.ts
import { tauriTransport } from './tauri';
import { httpTransport } from './http';
import type { Transport } from './types';

// isTauri() 判断:Tauri webview 注入了 __TAURI_INTERNALS__,浏览器没有
const isTauri = (): boolean =>
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export const transport: Transport = isTauri() ? tauriTransport : httpTransport;
export type { Transport } from './types';
```

> **Phase 2 会扩展** `isTauri()` 之外加 query param / env 判断(如 `?transport=http` 强制走 httpTransport 用于测试)。

## 2. 前端迁移(21 文件 + 22 测试)

### 2.1 迁移模式

**调用点改造**(机械替换):
```typescript
// 改前
import { invoke } from '@tauri-apps/api/core';
const session = await invoke<SessionRow>('load_session', { sessionId });

// 改后
import { transport } from '@/transport';
const session = await transport.invoke<SessionRow>('load_session', { sessionId });
```

```typescript
// listen 改前
import { listen } from '@tauri-apps/api/event';
const unlisten = await listen<ChatEvent>('chat-event', (e) => handle(e.payload));

// listen 改后
import { transport } from '@/transport';
const unlisten = await transport.listen<ChatEvent>('chat-event', (payload) => handle(payload));
```

### 2.2 迁移清单(21 非测试文件,按类别)

| 类别 | 文件 | invoke | listen |
|---|---|:---:|:---:|
| stores | streamController.ts | ✓(6 个) | ✓(6 个) |
| | chat.ts | ✓ | — |
| | permissions.ts | ✓ | ✓(1) |
| | projects.ts | ✓ | ✓(1) |
| | subagentRuns.ts | ✓ | ✓(2) |
| | audit.ts / config.ts / memory.ts / models.ts / permissionGrants.ts / providers.ts / subagents.ts / traceStore.ts | ✓ | — |
| utils | toolModeChange.ts / toolQuestion.ts / toolTaskStateTransition.ts / uiDiffApply.ts / useErrorBus.ts | ✓ | — |
| components | ChatInput.vue / ModelSelect.vue / AskUserQuestionCard.vue / ModelsTab.vue | ✓ | — |
| layout | TitleBar.vue | — | —(**不改**,window/os API 不在 transport 范围) |
| entry | main.ts | — | —(仅 import 类型,可不动) |

### 2.3 测试 mock 改造(22 文件)

**改前**:
```typescript
import { vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));
import { invoke } from '@tauri-apps/api/core';
```

**改后**:
```typescript
import { vi } from 'vitest';
vi.mock('@/transport', () => ({
  transport: {
    invoke: vi.fn(),
    listen: vi.fn().mockResolvedValue(() => {}),
  },
}));
import { transport } from '@/transport';
```

**统一辅助**(可选,若 22 个文件重复太多):
```typescript
// app/src/test/helpers/mockTransport.ts
export function createMockTransport() {
  return {
    invoke: vi.fn(),
    listen: vi.fn().mockResolvedValue(vi.fn()),
  };
}
export function mockTransportInstance(mock: ReturnType<typeof createMockTransport>) {
  vi.mock('@/transport', () => ({ transport: mock }));
}
```

## 3. 后端 emit 散点收敛

### 3.1 当前 emit 路径全景

```
10 个 emit 事件
├─ 7 个经 AppHandleSink(state.rs:647-711,实现 ChatEventSink trait)
│   chat-event / tool:call / tool:result / permission:ask / tool:question
│   mode:change:request / task:state:transition:request
│
└─ 6 处直调 app.emit(绕过 sink)── 本 task 收敛目标
    ├─ chat.rs:187          chat-event(pre-flight error 路径)
    ├─ helpers.rs:160       emit_chat_event(疑似遗留)
    ├─ helpers.rs:170       emit_tool_result(疑似遗留)
    ├─ subagent/sink.rs:279 subagent:event(有意绕过,走 collector)
    ├─ subagent/sink.rs:698 permission:ask(subagent 路径)
    └─ dispatch.rs:1192     subagent:finished(有意绕过)
```

**另**:`state.rs:317` 的 `projects:refreshed` 也是直调,但它在 `AppState::load` 后台任务里(不在 agent loop),本 task **保留不动**,Phase 2 daemon 化时统一处理。

### 3.2 散点处理方案

| 散点 | 处理方式 | 风险 |
|---|---|---|
| **chat.rs:187** | pre-flight error 时,已有的 `AppHandleSink`(或临时构造一个)走 `emit_chat_event(Error)` | 低,纯改 emit 入口 |
| **helpers.rs:160,170** | **删除 `emit_chat_event` / `emit_tool_result` 两个 helper 函数**,调用方改为持有 sink 后调 sink 方法。需先 grep 调用点确认安全 | 低,疑似死代码或早期遗留 |
| **subagent/sink.rs:279,698 + dispatch.rs:1192** | **抽象 `SubagentEventSink` trait**,新增 `AppHandleSubagentSink` 实现(包装现有 emit 逻辑)。subagent 路径持有 `Arc<dyn SubagentEventSink>` 而非 `AppHandle` | **中**,subagent collector 双通道语义微妙,需集成测试保底 |
| **state.rs:317** | **保留**,不动 | 无 |

### 3.3 SubagentEventSink trait 设计

```rust
// app/src-tauri/src/agent/subagent/sink.rs(或单独 trait 文件)

/// subagent 事件注入通道。
/// 与父 agent loop 的 ChatEventSink 是**两套语义**:
/// - ChatEventSink 服务于父 loop 的 LLM 流(chat-event / tool:call 等)
/// - SubagentEventSink 服务于 worker transcript 流(subagent:event / subagent:finished)
///   以及 worker 内部的 permission:ask(经 collector 路径注入,不走父 sink)
#[async_trait]
pub trait SubagentEventSink: Send + Sync {
    async fn emit_subagent_event(&self, run_id: &str, session_id: &str, kind: &str, payload: serde_json::Value);
    async fn emit_subagent_finished(&self, run_id: &str, payload: serde_json::Value);
    async fn emit_permission_ask(&self, payload: PermissionAskPayload);
}

/// 生产实现:包装 AppHandle,保持现有 emit 逻辑零变化
pub struct AppHandleSubagentSink {
    app: AppHandle,
}

#[async_trait]
impl SubagentEventSink for AppHandleSubagentSink {
    async fn emit_subagent_event(&self, run_id: &str, session_id: &str, kind: &str, payload: serde_json::Value) {
        // 原 subagent/sink.rs:279 的逻辑搬到这里
        self.app.emit("subagent:event", serde_json::json!({
            "runId": run_id, "sessionId": session_id, "kind": kind,
            "payload": payload, "timestamp": now(),
        })).ok();
    }
    // ... 其他方法同理
}
```

**Phase 2 用途**:daemon 化时,新增 `HttpSseSubagentSink` 实现同一 trait,subagent 路径零改动(只换 sink 注入)。

### 3.4 收敛后的 emit 路径

```
10 个 emit 事件
├─ 7 个经 AppHandleSink(ChatEventSink trait)── 不变
├─ chat.rs:187 → 走 AppHandleSink(收敛)
├─ helpers.rs:160,170 → 删除(收敛)
├─ subagent 3 处 → 经 SubagentEventSink trait(收敛,保留双通道语义)
└─ projects:refreshed(state.rs:317)── 保留,Phase 2 处理
```

## 4. 兼容性与回滚

### 4.1 行为兼容性

- **前端**:`tauriTransport` 的 `invoke` / `listen` 是对 Tauri API 的**纯转发**,无逻辑变化。唯一差异:`listen` 回调从 `Event<T>` 解包成 `T`,与现有调用方代码一致。
- **后端**:散点收敛只改 emit 入口,不改 emit 的**内容、时机、payload 结构**。

### 4.2 回滚

- **前端**:git revert 即可,无数据迁移。
- **后端散点收敛**:若 `SubagentEventSink` trait 抽象引入 regression,可回滚到直调 `app.emit`(trait 保留为 dead code,Phase 2 再用)。

## 5. 未决子问题(brainstorm 阶段澄清)

- **Q1**:`helpers.rs:160,170` 的 `emit_chat_event` / `emit_tool_result` 是否真的死代码?需 grep 调用点确认。若仍有调用,改为转发到 sink 而非删除。
- **Q2**:`AppHandleSubagentSink` 是新建结构体,还是让现有 `AppHandleSink` 同时 impl 两个 trait?倾向新建(职责分离),但若现有代码已经让 `AppHandleSink` 持有 subagent emit 能力,则合并 impl 更省。
- **Q3**:22 个测试文件的 mock 改造,是否值得抽 `createMockTransport` 辅助?若 22 个文件重复代码 > 5 行/文件,值得抽。
