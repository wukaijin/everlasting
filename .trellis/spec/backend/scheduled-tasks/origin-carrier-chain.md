<!-- Moved from scheduled-tasks.md 2026-09-28 (doc-split): origin 载体链 scenario -->

# Scheduled Tasks — origin 载体链(跨层契约)

> hub:[scheduled-tasks.md](../scheduled-tasks.md)(F2 调度判定与 fire 主契约)。

## Scenario: origin 载体链(跨层契约)

### 1. Scope / Trigger

- 触发:任何「给用户消息附加来源/上下文标记」的新需求(F2+ `schedule_task` tool、未来其他自动注入方)。
- 为什么:标记必须穿越「路由临界区 → 内存队列 → 另一个请求的驱动器 → persist」,载体选错(只加在 `ChatEntry`)在忙时路径**必然失效**。

### 2. Why(关键推理,勿回退)

忙时 fire 的条目由*另一个*请求的驱动器在 round>0 消费;驱动器对 round>0
一律丢弃请求级上下文(resend_seq/forced_dispatch 同款,`chat.rs` round 分支)
——所以载体必须在 `QueuedMessage`(队列条目自有字段),不能只在
`ChatEntry`。闲时路径 round 0 也从 drained 尾条取 origin,两条路统一。

### 3. Contracts

- 链路:`ChatEntry.origin` →(路由临界区内 `push_with_origin` 纯赋值拷入)→ `QueuedMessage.origin` →(驱动器每轮 move 全量 drained)→ `ChatLoopRequest.drained: Vec<QueuedMessage>` → init.rs:尾条 origin 派生 `drained.last()` 供 persist 门控 + `metadata.scheduled` 信封;非尾条由 persist 循环(RULE-QUEUE-001 根治,2026-08-29)逐条补写并各带各的 `scheduled` 信封。
- `TaskOrigin` 是 internally-tagged enum(`#[serde(tag="kind")]`),**会随 `QueuedMessage: Serialize` 进入 `list_queued_messages` wire**(前端排队占位「定时」徽标的依据)——这是有意定案,不是泄漏;但**不进 chat 事件主链**。
- `drained` 恒空的路径(用户发送/群聊/worker/legacy)行为逐字节不变;多 drain 时每行 origin 各随各行落 metadata(persist 循环 + 尾条 persist 点双写),不再有「只有尾条 origin 生效」的缺口(该缺口即 DEBT §RULE-QUEUE-001,已根治)。

### 4. Wrong vs Correct

#### Wrong

```rust
// 只在 ChatEntry 加字段
pub struct ChatEntry { ..., pub origin: Option<TaskOrigin> }
// 期望忙时入队后仍能取回 → round>0 驱动器重建请求,字段丢失,标记静默消失
```

#### Correct

```rust
// ChatEntry(入口)+ QueuedMessage(载体)+ ChatLoopRequest(传递)三点齐加,
// 临界区内纯赋值拷入;init.rs 侧尾条派生 + 非尾条 persist 循环各带各的 origin
let task_origin = request.drained.last().and_then(|qm| qm.origin.clone());
```

### 5. Tests Required

见 hub §6 origin 全链测试;新增携带来源的场景**必须**同时有对照组断言
(无 origin 路径 metadata 恒 None),防 additive 字段向既有路径漂移。
