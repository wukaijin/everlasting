# PR5: LLM 打断机制 (cancel_chat command + CancellationToken + stop 按钮)

> Source spike: [`docs/spikes/2026-06-06-feature-requests.md`](../../../../../docs/spikes/2026-06-06-feature-requests.md) 第 4 条
> 父 task: `06-06-spike-005-follow-up`
> 父 prd: [../06-06-spike-005-follow-up/prd.md](../06-06-spike-005-follow-up/prd.md) (PR5 段)
> Priority: P0 (用户反馈"tool call 死循环只能强退 app"是体验事故)
> 锁定决策: 方案 A — `cancel_chat` Tauri command + `tokio_util::sync::CancellationToken` + 前端 stop 按钮, 已有 tokens 持久化不丢

## Goal

让用户能**主动中断**正在进行的 LLM 请求 (含 streaming + tool call 死循环场景)。
- Backend: `cancel_chat` Tauri command 接收 `request_id`, 取消对应 in-flight stream
- Backend: 取消时已收到的 assistant tokens / tool calls 仍然 `persist_turn` 落 DB (不丢)
- Frontend: ChatInput.vue 或 ChatPanel.vue 加 `Stop` 按钮 (sending=true 时显示), 调 `cancel_chat`
- Frontend: 状态机加 `cancelled` 区分 (区别于 `error`), UI 视觉上提示 "已停止"

## What I already know

- `app/src-tauri/src/lib.rs:506-734` Agent Loop 是 `for turn in 1..=MAX_TURNS { ... stream.next().await ... }` 串行结构
- `lib.rs:507` `let mut stream = Box::pin(chat_stream_with_tools(...))` — stream 是局部变量, 没存
- `lib.rs:526-611` `while let Some(event_result) = stream.next().await` 是阻塞点
- `lib.rs:366` `tauri::async_runtime::spawn(async move { ... })` — task 起来了, 但 `JoinHandle` 没存到 `AppState`
- `app/src-tauri/src/lib.rs:50-54` `AppState { config, tools, db }` 当前只有 3 个字段
- `chat.ts:751-756` `await invoke("chat", ...)` 是单向 invoke, 无 reverse 通道
- `chat.ts:209-220` `streamingSessionId` / `lastStreamedProjectId` / `streamingProjectIds` 已有 streaming 状态
- `chat.ts:693-767` `send()` 函数流是: push userMsg + assistantMsg → invoke("chat") → 后续 event 增量更新
- `chat.ts:380-399` `done` / `error` 事件 handler 把 `sending=false` + `currentRequestId=null`
- 用户消息 + 整 assistant turn + tool_result turn 都是 turn boundary `persist_turn` 落 DB (`lib.rs:496-503, 657-664, 723-731`)
- `tokio_util::sync::CancellationToken` 是标准模式, `tokio::select!` 可同时 await stream + token

## Requirements

### Backend
- `AppState` 加 `cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>` (`tokio_util::sync::CancellationToken`)
- `chat` Tauri command 入口:
  - 拿/建 `CancellationToken`, 用 `request_id` 作 key 存 map
  - 把 token clone 一份进 spawn task (持有 owned 引用, 取消时不会 drop)
  - 改 `while let Some(event_result) = stream.next().await` 为 `tokio::select!`, 同时:
    - `event_result = stream.next()` → 旧逻辑
    - `_ = token.cancelled()` → 跳出 stream loop, 设 `cancelled = true` 标志
  - spawn task 退出 (任何路径) 时: `state.db.clone()` 拿不到 (已 move), 改为 spawn task 内 clone AppState 引用, 退出时 `cancellations.lock().await.remove(&request_id)`
- 新 `cancel_chat(request_id: String)` Tauri command:
  - `state.cancellations.lock().await.get(&request_id).cloned()` → `token.cancel()`
  - 已 cancel 的 request_id 是 idempotent no-op
  - 不存在的 request_id 静默成功 (前端可能多次点击)
- `persist_turn` 时机:
  - 正常: 每个 assistant turn 边界 (现有逻辑, lib.rs:657-664)
  - 取消: 已收到的 `text_parts` / `tool_calls` / `finalized_thinking` 仍构造 `assistant_blocks` 并 `persist_turn` 落 DB, **不丢**
  - 部分 tool_result (tool 已执行但 turn 未结束) 仍 `persist_turn`, **不丢**
  - 用 `cancelled: bool` 标志决定 assistant message 的 metadata 是否带 "cancelled" 标记 (DB schema 是否需要新列? 见下)
- DB schema:
  - **不新加列** (避免 migration): 用现有 `MessageContent::Blocks(blocks)` 内的 `ContentBlock::Text { text }` 落库, 把 "已停止" 状态编码进 text 末尾: `format!("{}\n\n[已停止]", full_text)` — 不污染 schema, 渲染时 UI 自己 strip 后缀
  - 或者更干净: 加 `is_cancelled: bool` 列到 `messages` 表, 幂等 migration (跟 PR2 的 git_branch 列同 pattern)。本 PR 选**后缀方案**, 留 PR-future 评估
- 新增 `tokio-util` 依赖 (Tauri 2 默认就有 `tokio`, `tokio-util` 也大概率有, 验证 Cargo.toml)

### Frontend
- `app/src/components/chat/ChatInput.vue` (或 ChatPanel.vue, 看哪个更顺手): 加 `Stop` 按钮
  - 条件显示: `sending === true` 时显示, 替代或并列发送按钮
  - 调 `invoke("cancel_chat", { requestId: chatStore.currentRequestId })`
- `app/src/stores/chat.ts`:
  - `currentRequestId` 已有, 用作 `cancel_chat` 参数
  - 取消成功后: 前端 `sending=false` + `currentRequestId=null` + streaming 项目点清除
  - **不** 单独区分 cancelled/error 视觉: 后端会把 `done` 或 `error` event 发出来 (取决于取消时机), 前端已有 handler 处理
- UI 状态: 取消后, 当前的 assistant 消息显示 "[已停止]" 视觉 (从后端写入的 text 后缀渲染)
  - 渲染时: `MessageItem.vue` 检测末尾 `[已停止]` marker, 加 CSS class `msg--cancelled` (灰色斜体)
  - 或者: 不在 MessageItem 改, 留后端写 marker 就够了, 用户能直接看到文字

### 测试
- **cargo test** for backend:
  - cancel token 创建 → cancel → stream 立即返回 (mock stream 跑 1s, cancel 100ms 后, 测试在 200ms 内结束)
  - 取消时 `persist_turn` 仍被调 (用 mock db 或 spy)
  - 不存在的 request_id cancel 是 no-op (idempotent)
  - 多个并发 request 互不干扰 (2 个 request_id, 取消 A 不影响 B)

## Acceptance Criteria

- [ ] 流式期间点击 Stop 按钮: 1s 内前端 `sending=false`, 后续 delta 不再追加到 UI
- [ ] 取消时已收到的 assistant tokens 持久化到 DB (后续 reload session 还在)
- [ ] 取消后 `currentRequestId` 清空
- [ ] 切换 session 不影响正在取消的 stream (每个 request_id 独立)
- [ ] 多次点 Stop 不报错 (idempotent)
- [ ] `pnpm build` (vue-tsc + vite) 通过
- [ ] `cargo test` 通过, 含 cancel token 并发用例
- [ ] 视觉: 取消后 assistant 消息末尾有 "[已停止]" marker (UI 一眼可辨)
- [ ] 已收到的 tool_result 不丢 (tool 已执行的 tool_result 仍 persist_turn)

## Definition of Done

- 修改 ~5-7 个文件
- cargo test + pnpm build 双过
- 跑完 standard Trellis 流程到 archived
- 视觉验证: 起一个真 LLM 流, 看到 Stop 按钮可点 + 点击后流断 + DB 不丢内容

## Out of Scope

- Tool 执行中的中断 (read_file 读大文件 / shell 跑 5min) — 留 v2, 现有 tool 5min timeout 硬卡
- 多 device 同步 (BACKLOG §4 跨设备)
- 已 cancel 的 stream 清理 (CancellationToken 在 cancelled 后仍占内存, 等 spawn task 退出时清)
- 后端流式 retry / reconnect — 跟打断独立
- 移动端 Stop 按钮 (Tauri 2 桌面应用, 暂不考虑)

## Technical Notes

- 改动文件:
  - `app/src-tauri/Cargo.toml` (验证 tokio-util 已有, 可能加 features)
  - `app/src-tauri/src/lib.rs` (AppState + cancel_chat command + select 包裹 + cancelled 后缀写入)
  - `app/src-tauri/src/lib.rs` 测试模块 (新增 cargo test, 文件内 `#[cfg(test)] mod tests`)
  - `app/src/components/chat/ChatInput.vue` (Stop 按钮)
  - `app/src/stores/chat.ts` (cancel_chat 调用 + 状态机)
  - 关联: 父 prd §"Decision 5: cancel 机制方案 A" 是锁定方案, 实施直接套
- 风险: `tauri::async_runtime::spawn` 起的 task 拿不到 `AppState` 的 reference 怎么办 — 解决: `let state = state.inner().clone()` 在 invoke handler 内 clone, spawn 内持有 `Arc<AppState>` clone
- 风险: `Mutex<HashMap>` lock 持有过久 — 解决: 取消时只持锁 clone token, 立刻 drop 锁, 调 `token.cancel()` 不持锁
- 风险: stream 已经 Done 之后才 cancel — token cancel 是 no-op, 不会 panic
- 风险: `tokio::select!` 在 streaming 期间 cancel — stream 在 `next()` 等待时, select 收到 cancel 分支, stream 仍持有, drop 时会尝试关闭 HTTP connection (可能几秒延迟) — 接受这个 trade-off, 不强求立即断网

## Decision (ADR-lite)

- **决策 1**: DB 不新加 `is_cancelled` 列, 改用 text 末尾 `[已停止]` marker
  - **理由**: 避免 migration, 跟 PR2 节奏保持一致 (类似策略); 未来如要统计 cancelled 率再开列
  - **后果**: 取消的 message 跟正常 message 在 schema 上无区别, UI 渲染时识别 marker
- **决策 2**: tool 执行中的 cancel 留 v2
  - **理由**: 现有 5min tool timeout 硬卡已能处理 read_file 大文件 / shell 长命令; 5min 比用户愿意等的 cancel 时长还长, 实际触发概率低
  - **后果**: cancel 只能断 stream, 不能断正在执行的 tool (tool 跑完前 cancel 不会生效)
- **决策 3**: 前端 stop 按钮放在 ChatInput.vue (不放在 ChatPanel header)
  - **理由**: 跟 send 按钮并列, UX 一致; ChatPanel header 空间 28px 紧, 不再加元素
  - **后果**: ChatInput.vue 需 conditional render: sending 时显示 Stop, 平时显示 Send
