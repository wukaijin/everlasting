# Research: 重试 chat 流的实现路径(resend 模式 / startRequest / abort)

- **Query**: 用户点"重试"后,前端怎么重新 invoke chat?已经有 helper 吗?abort / cancel 边界如何处理?
- **Scope**: internal (chat store + streamController)
- **Date**: 2026-07-17

## Findings

### Files Found

| File Path | Description |
|---|---|
| `app/src/stores/chat.ts:946-1110` | `send()` — 完整 send 路径(push placeholders + history + startRequest) |
| `app/src/stores/chat.ts:1120-1124` | `cancel()` — fire-and-forget IPC,等 `done` 事件真正 reset |
| `app/src/stores/chat.ts:1257-1341` | `resendMessage()` — D3 PR3 实现,user message resend 路径(已可用,可参考结构) |
| `app/src/stores/chat.ts:1170-1225` | `editMessage()` — 编辑模式 IPC + refresh |
| `app/src/stores/chat.ts:1294-1308` | resendMessage 的 seq 计算 + checklist clear + placeholders push |
| `app/src/stores/streamController.ts:2090-2197` | `startRequest()` — 唯一入口,生成 requestId + activeRequests + pin + invoke |
| `app/src/stores/streamController.ts:2199-2217` | `cancel()` — invoke `cancel_chat` + 等后端 `done{stop_reason:"cancelled"}` |
| `app/src/stores/streamController.ts:2220-2227` | `currentRequestId(sessionId)` — 查找 in-flight requestId |
| `app/src/stores/streamController.ts:1620-1671` | `finalizeRequest()` — 同步清理 activeRequests + reloadAfterFinalize |
| `app/src/stores/streamController.ts:1691+` | `reloadAfterFinalize()` — 重新从 DB load_session + rehydrateMessages 替换 buffer |
| `app/src/components/chat/MessageActionsMenu.vue:156-159` | `onResend` — 仅触发 `resend` emit 给 parent,不直接调 store |
| `app/src/components/chat/MessageItem.vue:794-810` | `onResend` handler — 调 `chatStore.resendMessage(sid, messageSeq, content)` |
| `app/src/components/chat/MessageItem.vue:880-900` | `handleResend` — 编辑模式里的 resend path |
| `app/src/stores/streamController.ts:1-100` | 文件头注释 — LRU 20 + activeRequests Map + per-session 事件路由架构 |

### Code Patterns

#### Pattern 1: `send()` 完整路径(可以原样复用)

`app/src/stores/chat.ts:946-1110`:

```ts
async function send(text: string) {
  const trimmed = text.trim();
  if (!trimmed || isCurrentSessionStreaming.value) return;
  const projectId = projectsStore.currentProjectId;
  if (!projectId) throw new Error("send: no current project");

  // @@-prefix dispatch parse (omitted for brevity)
  const parsed = parseForcedDispatchPrefix(trimmed, models);
  if (parsed === null) return;
  let forcedDispatch = parsed.forcedDispatch;
  let body = parsed.body;

  if (!currentSessionId.value) await createNewSession();
  const sessionId = currentSessionId.value!;
  const msgs = await controller.ensureLoaded(sessionId);

  useChecklistStore().clearForNewRun(sessionId);   // ← 清 checklist

  // seq 计算 + placeholders push + history 构造
  const nextSeq = msgs.reduce(/* max seq + 1 */) + 1;
  const userMsg: ChatMessage = { id: genId(), seq: nextSeq, role: "user", content: body };
  const assistantMsg: ChatMessage = { id: genId(), seq: nextSeq + 1, role: "assistant", content: "" };
  msgs.push(userMsg, assistantMsg);

  const history: ChatMessagePayload[] = msgs
    .filter((m) => m.id !== assistantMsg.id)
    .map((m) => ({ role: m.role, content: toPayloadContent(m) }));

  await controller.startRequest({ sessionId, projectId, userMsg, assistantMsg, history, forcedDispatch });
}
```

- **关键步骤**:
  1. ensureLoaded session(可能 hit DB 拉消息)
  2. clearForNewRun(per-run checklist 重置)
  3. 计算 nextSeq(已有消息 max seq + 1)
  4. push 新的 userMsg + assistantMsg placeholder 到 controller buffer
  5. 构造 history(filter 掉刚 push 的 assistant placeholder,因为 streaming 要 mutate 它)
  6. `startRequest` 注册 activeRequests + pin + invoke("chat", ...)

#### Pattern 2: `startRequest` 内部实现(streaming 入口唯一)

`app/src/stores/streamController.ts:2095-2197`:

```ts
async function startRequest(args: StartRequestArgs): Promise<string> {
  await start();
  const requestId = genId();
  activeRequests.set(requestId, {
    requestId,
    sessionId: args.sessionId,
    projectId: args.projectId,
    userMsgId: args.userMsg.id,
    assistantMsgId: args.assistantMsg.id,
    history: args.history,
    sendAt: Date.now(),
    firstDeltaAt: null,
    toolStartedAt: new Map(),
    currentTurnIndex: -1,
    latencyByTurn: new Map(),
  });
  pinnedSessions.add(args.sessionId);
  useMemoryStore().clearRecallHits(args.sessionId);
  void useTraceStore().resetForNewSession(args.sessionId);

  try {
    await invoke("chat", {
      requestId,
      sessionId: args.sessionId,
      messages: args.history,
      resendSeq: args.resendSeq,    // ← resend 标记,undefined = 普通 send
      forcedDispatch: args.forcedDispatch ?? null,
    });
  } catch (e) {
    // ← IPC 同步失败 fallback:把 last.error 标 server,finalize
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
  return requestId;
}
```

- **返回 requestId**:方便 caller 后续 `cancel(requestId)`。
- **`pinnedSessions`**:streaming 期间该 session 不被 LRU 淘汰(`streamController.ts:2136-2138`)。
- **`resendSeq` 字段**:undefined = 普通 send,number = D3 PR3 的 user message resend;**scope B 的 chat retry 应传 `undefined`**(不是 user message resend)。
- **错误 catch 块已写死 category:"server"** — scope B 修复点之一(见 research/02)。

#### Pattern 3: `cancel(requestId)` — 异步 cancel 路径

`app/src/stores/streamController.ts:2206-2217`:

```ts
async function cancel(requestId: string): Promise<void> {
  try {
    await invoke("cancel_chat", { requestId });
  } catch (e) {
    console.error("[streamController] cancel failed:", e);
    // fire-and-forget;cancel 失败靠 stream 自然结束
  }
}
```

`app/src/stores/chat.ts:1120-1124`(thin wrapper):

```ts
async function cancel() {
  const rid = currentRequestId.value;
  if (!rid) return;
  await controller.cancel(rid);
}
```

- **cancel 是 IPC fire-and-forget**;真正的 state reset 由后端的 `done{stop_reason:"cancelled"}` 事件回流触发(`finalizeRequest`)。
- **不需要 abort controller** — Tauri 的 `invoke("cancel_chat", ...)` 走独立 IPC 通道,与 `invoke("chat", ...)` 互不干扰。
- **同时存在两条流的窗口**:cancel 后,旧 request 的 `done` 事件可能仍在飞行(race);但 `finalizeRequest` 收到 done 时只清理 `activeRequests.delete(requestId)`,**不会触碰新 request 的 state**(`streamController.ts:1661-1665` 检查 `activeRequests.get(requestId)`)。

#### Pattern 4: `resendMessage` 已实现的完整路径(scope B retry 的最佳参考)

`app/src/stores/chat.ts:1257-1341`:

```ts
async function resendMessage(
  sessionId: string,
  messageSeq: number,
  contentText: string,
): Promise<void> {
  if (!sessionId) throw new Error("resendMessage: sessionId is required");
  if (typeof messageSeq !== "number") throw new Error("resendMessage: messageSeq is required");
  if (typeof contentText !== "string") throw new Error("resendMessage: contentText must be a string");

  // 1. Stream race — cancel any in-flight stream on this session
  if (sessionId === currentSessionId.value && isCurrentSessionStreaming.value) {
    await cancel();
  } else {
    const rid = controller.currentRequestId(sessionId);
    if (rid) await controller.cancel(rid);
  }

  // 2. ensureLoaded + checklist clear + nextSeq + placeholders push + history
  const projectId = projectsStore.currentProjectId;
  if (!projectId) throw new Error("resendMessage: no current project");
  const msgs = await controller.ensureLoaded(sessionId);
  useChecklistStore().clearForNewRun(sessionId);

  const nextSeq = msgs.reduce(/* max seq + 1 */) + 1;
  forceFollowActive.value = true;
  const userMsg: ChatMessage = { id: genId(), seq: nextSeq, role: "user", content: contentText };
  const assistantMsg: ChatMessage = { id: genId(), seq: nextSeq + 1, role: "assistant", content: "" };
  msgs.push(userMsg, assistantMsg);

  const history: ChatMessagePayload[] = msgs
    .filter((m) => m.id !== assistantMsg.id)
    .map((m) => ({ role: m.role, content: toPayloadContent(m) }));

  // 3. Start the request with resendSeq flag
  await controller.startRequest({
    sessionId, projectId, userMsg, assistantMsg, history,
    resendSeq: messageSeq,    // ← 唯一与 send() 不同的参数
  });
}
```

- **结构与 send() 90% 相同**,唯一差别:contentText 来自 caller + `resendSeq: messageSeq` 标记 audit。
- **cancel-then-fire 顺序**:`await cancel()` 后再 `startRequest`,保证旧 request 已经从 activeRequests 移除。

#### Pattern 5: `MessageItem.vue::onResend` 已用 resendMessage 模式

`app/src/components/chat/MessageItem.vue:794-810`:

```ts
async function onResend(messageSeq: number) {
  if (props.message.role !== "user") return;
  if (isStreaming.value) return;
  const sid = chatStore.currentSessionId;
  if (!sid) {
    projectsStore.showToast("重发失败: 无当前 session", "error");
    return;
  }
  try {
    await chatStore.resendMessage(sid, messageSeq, props.message.content);
  } catch (e) {
    projectsStore.showToast(
      `重发失败: ${extractErrorMessage(e)}`,
      "error",
    );
  }
}
```

- **try/catch + showToast**:scope B 的 retry handler 应沿用同 pattern。

#### Pattern 6: cancel 后旧流 `done` 事件到达的处理(race)

`app/src/stores/streamController.ts:1146-1147, 1219`(done / error case):

```ts
case "done":
  // ...
  finalizeRequest(req.requestId, req.sessionId, false);
  break;
case "error":
  // ...
  finalizeRequest(req.requestId, req.sessionId, true);
  break;
```

`app/src/stores/streamController.ts:1661-1666`(finalizeRequest 主体):

```ts
const req = activeRequests.get(requestId);
if (req) {
  completedRequests.set(requestId, req);
}
activeRequests.delete(requestId);
pinnedSessions.delete(sessionId);
```

- **旧 request 的 done/error 事件到达时**:`activeRequests.get(requestId)` 仍能命中(因为移到 completedRequests 但 activeRequests 可能已 delete)— **重要的是 finalizeRequest 只清理自己的 sessionId 的 LRU pin**,不影响新 request。
- **race 风险**:旧流 done 事件在新流 startRequest 之后到达,可能会在 `useChatStore().accumulateLatency` 等 hook 里"记到新流头上"。但实际看 `done` handler 是从 `activeRequests` 查 req,如果旧 rid 已 delete 就跳过(需 verify,但代码 `req = activeRequests.get(requestId)` 是空检查式)。**当前 controller 缺一个"finalized 后的迟来事件 silent skip"**;scope B 若发现 race 再补。

### External References

- `.trellis/spec/frontend/state-management.md` — D3 PR3 resendMessage 实现细节;同 spec 应被 retry helper 复用。
- `.trellis/spec/backend/agent-loop-architecture.md` — agent loop 的 `run_chat_loop` 23 参签名 + cancel 一次即终止(RULE-A-010)。
- `.trellis/spec/backend/error-handling.md:206-264` — Error path 持久化 + ERROR_MARKER;retry 后会重新 reload from DB,新 row 不带 marker。

### Related Specs

- `.trellis/spec/backend/error-handling.md` §"Agent Loop Error Paths"。
- `.trellis/spec/frontend/state-management.md` §"D3 PR2/PR3 (2026-06-17): inline message edit + resend"。

## Caveats / Not Found

1. **`chatStore.resendMessage` 已存在且完整可用**,**结构上就是 scope B chat retry 的最佳模板**。**关键差异**:resend 是 user message resend(content 不变 + `resendSeq` audit),chat retry 是 assistant error turn 重发(同 history + **不带** resendSeq)。可以复制 resendMessage 实现改名为 `retryChat(sessionId, messageSeq)`(参数化:用 `seq-1` 定位 user message,从 in-memory msgs 拿 history 而非 push 新 user message)。
2. **新 retry 不能 push 新的 user/assistant placeholder**(否则会出现"原 user message 后面跟 2 个 assistant 消息"的奇怪序列)— 应该**清掉 errored assistant 的 error 标记 + 清掉 ERROR_MARKER text 尾巴**,然后 startRequest 复用同一 assistantMsg id(让 delta events 覆盖原 row)。**这是 resendMessage 模式的关键修改点**。
3. **错误消息 seq 计算**:resendMessage 的 nextSeq 是"所有 msgs max seq + 1" — 如果 errored assistant 的 seq 已经存在(DB 持久化过 partial turn),新 retry 不能复用该 seq,必须用 max+1 推一个新高;否则后端 `persist_turn` 会撞 UNIQUE。
4. **不是 abort controller**:`cancel_chat` 是独立 IPC,跟 `chat` invoke 互不干扰,**前端不需要 AbortController**。cancel 旧流后 await 完成,再 fire 新流即可。
5. **`isStreaming` 检查**:MessageItem 已经在 `onResend` 里检查 `if (isStreaming.value) return`,**error footer 的 retry 按钮需要同样的 gate**(或者 disable button)— 否则用户点击时旧流还在跑,startRequest 的 `isCurrentSessionStreaming` guard 会先 cancel + 重发(可能 OK,但 UX 上应明示)。
6. **`onResend` 失败已用 `showToast`** — retry 失败同样应 toast(`重试失败: ...`)。
7. **`startRequest` 的 catch 块目前写死 `category: "server"`**(`streamController.ts:2191`)— 但实际 `e` 可能是 `AppCommandError`(Tauri IPC rejection 自动序列化为对象),里面有 `retryable` 字段;**scope B 可同时修这个 catch 块**,从 `parseAppCommandError(e)` 拿 retryable/category 写 `last.error`。
8. **race:旧流 done 事件迟到** — finalizeRequest 只清自己的 sessionId pin,不影响新流;但 `useChatStore().accumulateLatency(req.sessionId, totalMs)` 之类的 hook 是按 sessionId 而非 requestId 累加的,**会双计 latency**。scope B 若发现应补 `req.requestId` 检查。
9. **`forceFollowActive.value = true`**(F2 自动滚动)— `send()` 和 `resendMessage()` 都设了;chat retry 也应设(用户期望新流自动滚到底)。
10. **`clearForNewRun(sessionId)`** — checklist 重置,resend 路径有;chat retry 同样需要(新 run 清掉旧 checklist)。

## 给 design.md 的输入

`chatStore.resendMessage`(chat.ts:1257-1341)是现成模板,scope B retry helper 应**复制其结构但参数化为 `retryChat(sessionId, messageSeq)`**:(1) cancel 旧流(`controller.currentRequestId` 拿 rid,`controller.cancel(rid)`);(2) ensureLoaded + clearForNewRun + forceFollowActive;(3) **关键差异**:**不 push 新 user/assistant placeholder**,而是**直接 mutate 已有 errored assistant message**(`error = undefined`、`streaming = true`、text 去 ERROR_MARKER suffix),nextSeq = errored assistant seq + 1;(4) 构造 history 时**过滤掉 errored assistant**(让新 stream 覆盖原 row);(5) `startRequest({ history, resendSeq: undefined })`。MessageItem 的 retry click handler 沿用 `onResend` 的 `try/catch + showToast` pattern。**改动量 ~70 行**(1 个 `retryChat` method + 1 个 click handler + 复用 `clearForNewRun` / `ensureLoaded` / `cancel` / `forceFollowActive`)。**不引入 AbortController**(Tauri cancel_chat 是独立 IPC);**race 风险**(旧流 done 迟来双计 latency)scope B 不补(OOS)。