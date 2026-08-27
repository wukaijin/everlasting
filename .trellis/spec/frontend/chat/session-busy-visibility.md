# Session busy 可见性契约(serverBusy + 流光边框,F6 2026-08-27 `08-27-f6-async-agent-task`)

## Convention: busy 双源合流 + 终结事件消解 + reload 兜底

detach 之后 SSE 流可能零订阅(他端/冷启动),`streamingSessionIds`(本地流)不再完备——「这个 session 在跑吗」必须合流两个来源:

```ts
// SessionList.vue isSessionBusy —— 驱动 `.session-item--busy` 流光边框
// (conic-gradient 旋转环,ChatInput 流式环同款配方,2026-08-28 从脉动圆点改)
isSessionBusy(s) = streamController.streamingSessionIds.has(s.id)  // 本端在流
                || s.busy === true                                   // 后端 session_active_request(claim 即亮,含 F3 等闸轮)
```

规则:

1. **`s.busy` 是后端权威、wire additive 可选**(`busy?: boolean`,旧 daemon 无字段不炸);只信 `=== true`,undefined 视为闲。
2. **消解点在 `finalizeRequest`**(全 rid 终态的公共出口):`summary.busy = false`。不要在 done/error 事件回调里各清一遍——foreign rid 认领(adoptForeignRequest)与本地 rid 走同一出口,分头清必漏。
3. **reload 兜底**:session 列表刷新(`loadSessions`)自然带回最新 busy,覆盖任何漏消解的 stale 态;MVP 接受 stale-busy 短暂悬挂,不做主动轮询。
4. **完成通知(current-session 抑制)**:toast/桌面通知按「事件到达时 `event.sessionId !== 当前会话」判定,`stop_reason === "cancelled"` 抑制(用户自己停的不该弹「已完成」);通知构建是纯函数(`buildTurnFinishedNotification`,sessionId 附载供点击跳转),便于单测。
5. **关闭确认(Tauri GUI 专属)**:`isTauriWebview()`(查 `window.__TAURI_INTERNALS__`,**不是 transport 种类**——daemon 化 Tauri 壳默认走 httpTransport 但仍是 GUI 进程,关窗即杀任务)才挂 `onCloseRequested`;Web/PWA 关闭(Ctrl+W)不影响 daemon 侧任务,无需确认。

## Wrong vs Correct

```ts
// Wrong — 只看本地流:他端/detach 后发起的轮次本地无流,忙指示全瞎
const busy = streamingSessionIds.has(s.id)

// Wrong — 在 done 和 error 回调里各清 busy:foreign rid 只走其一,漏清 stale
case 'done': chatStore.clearBusy(sessionId)

// Correct — 终结统一走 finalizeRequest 出口消解,列表读取双源合流
function isSessionBusy(s) { return streamingSessionIds.has(s.id) || s.busy === true }
```

## Tests

- `turnFinishedNotification.test.ts`:纯函数 4 例(文案/抑制/跳转/跨项目 fallback)+ 集成 6 例(foreign done 消 busy、error、cancelled 抑制、gate 关、群聊 mid-round vs group_chat_end)
- `CloseGuardDialog.test.ts`:countBusy 并集口径(streaming ∪ serverBusy,含等闸轮)
