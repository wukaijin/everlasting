# Design — GCE-M3 控制面暴露层

> 原则:M0-M2 一脉承——**daemon(Rust)零改动**是本设计的验收基线;所有新面都在 JS 暴露层(MCP/脚本)+ 前端 + 文档。

## 1. 架构与边界

```
MCP 宿主 agent ─┐                       ┌─ POST /api/v1/cancel/preempt_group_chat (已有)
M1 脚本 ────────┼─ group-chat-run.mjs ──┼─ POST /api/v1/agent/chat (已有,acceptance JSON)
(GUI 打断按钮)─┘   (共享实现层,新增      └─ GET /api/v1/stream (已有,SSE 透传,只补文档)
                     preemptGroupChat helper)
```

- **Rust 侧预期零 diff**。若实现中发现必须动 Rust 才能达成 AC,停下来回炉 design(信号:某个 AC 建立在对既有端点的错误理解上)。
- MCP 层纪律沿用 M2:纯逻辑区零 SDK import;工具 handler 透传到 `coreInterrupt` / `coreInject`;测试走 `node --test`(vitest 不收 scripts/)。

## 2. MCP 工具设计(group-chat-mcp.mjs)

### 2.1 `interrupt_discussion`

- 入参 `{ session_id: string }`(zod);handler → `coreInterrupt(deps, args)` → `preemptGroupChat(base, session_id)`。
- 成功响应(工具文案,非裸 JSON 透传):
  ```
  已请求收束打断:在途发言完成后 moderator 收束(约 1-3 分钟),随后 stop_reason=preempted。
  请轮询 discussion_status 至终态,再取 discussion_result 读 summary。
  ```
- 端点报错(无进行中讨论 / session 不存在)原样透传为工具错误。

### 2.2 `inject_message`

- 入参 `{ session_id: string, text: string(min 1) }`;**不设 images 参数**(P0 裁定纯图片注入不支持)。
- handler → `coreInject(deps, args)`:
  1. **前置 busy guard(主防护,评审 P1-1)**:`findSession`(mcp.mjs:145,coreStatus/coreResult 共用,零新机制)判定 `busy=true`;非 busy / 已收官 → **不 fireChat** 直接语义报错「目标不是进行中的群聊」——fireChat 打在已收官群聊 session 会重启编排器并无条件 `clear_group_chat_lifecycle` 抹掉旧 summary(prd Fact 7,cancel 救不回),guard 前置则零副作用(连 Started 都不产生);
  2. 生成自有 `request_id`(`inject-<ts>-<rand>`,同 M1 先例)→ `fireChat(base, buildChatBody({ requestId, sessionId, topic: text }))`;
  3. 按 `ChatAcceptance.status` 分派:
     - `injected` → 成功文案(「已注入,下一 moderator 轮可见」);
     - `started` / `queued` → 自有 rid 即时 `cancelChat`(**降级为竞态兜底**:guard 判定与落库间讨论恰好收官的窗口)+ 语义报错 + 指引「发起新讨论用 start_discussion」。
- 注入的落库与双轨标记全在编排器轮头(P0 已交付),工具层不做任何标记加工。

### 2.3 wire 预算

- `TOOLS_BUDGET_CHARS`:2300 → **3200**,`group-chat-mcp.mjs:265` 与 `group-chat-mcp-smoke.mjs:23` 两侧同步;描述密度保持现状(AC4 锁语义不变:锁是上限护栏,不是抠字数目标)。

## 3. 共享实现层(group-chat-run.mjs)

- 新增 `preemptGroupChat(base, sessionId)` → `api(base, 'cancel/preempt_group_chat', { body: { session_id: sessionId } })`,与 `cancelChat`(rid 域)并列,注释标注 session 域 vs rid 域分工。
- `inject` 不新增 helper:guard(findSession)+ `fireChat` + acceptance 判定留在 mcp.mjs(判定语义是 MCP 工具的,不是共享层的;M1 脚本无注入需求)。guard 与 acceptance 分派共用纯函数 `interpretAcceptance`(三态 → injected / misfire),guard 的「已收官」分支由 `findSession` 结果直接判定、不走 fireChat。
- 纯函数区(可 `node --test` 覆盖):acceptance 分派逻辑提为纯函数 `interpretAcceptance(acceptance, ownRequestId)` → `{ kind: 'injected' | 'misfire' }`,misfire 时返回应执行的止损动作;SDK 链路只做 IO。

## 4. GUI 打断入口

### 4.1 位置(提案,实现时可按视觉微调)

群聊 chip 区(`ChatPanel.vue` template `:718-727`,现有「群聊 (N 参与者)」chip(`v-if="isGroupChat"`)旁;锚点修正承评审 P3-4——`:211-218` 是 TS computed 区)**新增「打断」按钮**,可见性 = `streaming && isGroupChat`(chat store 的 `isCurrentSessionStreaming`,chat.ts:599)。

- **否决备选**:把 ChatInput 的 Stop 按钮在群聊会话换形为「打断」——会牵动 F1 单按钮三态 / Esc(P2-5)/ 群聊 Esc 语义三处既有拍板,回归面大;且硬取消(Stop)与收束(preempt)并存才是「同权」的完整语义。
- 按钮无确认弹层:收束式打断不毁场(在途发言完成 + summary 保留),误触代价低;硬止损仍有 Stop 把守。

### 4.2 调用链

- chat store 新增 action:直接 `transport.invoke("preempt_group_chat", { sessionId })`。store 直用 transport 有同文件先例(`diff_worktree`,chat.ts:788;chat.ts:38 / chatSendActions.ts:16 均 import transport)——评审 P3-4 建议「经 controller 中转」与该先例不符,部分驳回;若实现时发现需要 controller 态(与 cancel 同构诉求)可改走 controller 新增 `preempt(sessionId)`,两形等价。**两 transport 通用**(Tauri 透传 command;http 顶层 key 自动扳正为 `session_id`,prd Fact 3)。
- 终态呈现零新工作:`Done{stop_reason=preempted}` 的 finalize 白名单(`streamEvents.ts:616-627`)+ notice(`streamController.ts:543`)已随 P0 交付;按钮可见性跟 streaming 自然收。
- 返回值 `{"preempted": true}`:true → toast「收束打断已请求,讨论将在在途发言完成后收束」;false/error → toast 错误。

## 5. SSE follow 消费文档(DAEMON-API.md 新 §)

内容清单(全部为既有机制的文档化,零代码):

1. 端点与连接:`GET /api/v1/stream`,单全局流,`Last-Event-ID` 断连重放,30s keepalive;
2. 事件通道表(7 父 + 2 worker,群聊消费方关注 `chat-event`);
3. `chat-event` 载荷:`request_id` + `session_id` + event(`Speaker` / `Delta` / `ThinkingDelta` / `Done{stop_reason,usage}`),**按 session_id 过滤**的消费范式;
4. 群聊消费序列范例:fire chat → 订阅流 → 逐轮 `Speaker` → `Delta` 流式 → `Done`(轮界)→ 终态 `Done` 与轮询 `stop_reason` 同值对照(GC1/GC2 语义);
5. 断连恢复:重放窗口 / `stream-resync` sentinel → snapshot 补齐;
6. 注意事项:零订阅者时权限 ask 8s 快拒(M0 档)——follow 连接本身会让无人值守 ask 走 attended 路径(120s),消费方应知悉。

## 6. 测试与验证面

| 层 | 内容 | 锚点 |
|---|---|---|
| JS 纯函数 | `interpretAcceptance` 三态分派 + 止损动作 | `group-chat-run.test.mjs` 新增 |
| MCP 工具 | `coreInterrupt` / `coreInject` 成功与误发路径 + **前置 guard(已收官群聊注入 → 报错且零 fireChat 调用)** + InMemoryTransport 全链 + 预算 3200 | `group-chat-mcp.test.mjs` 新增(AC4 文案测试同步) |
| 冒烟 | tools/list 六工具名单 + 预算锁 | `group-chat-mcp-smoke.mjs`(非 live 零成本) |
| GUI | store action 调用形状(两 transport)+ 按钮可见性 | vitest(store 层;组件级是否上 Playwright 实现期定) |
| live | 一场真讨论:inject → follow(SSE 消费)→ interrupt → 轮询终态(`preempted` 或同刻自然收官 `group_chat_end`)→ result;**已收官 session 注入 → 报错且旧 summary 原样** | AC1/AC2/AC5 验收 |

## 7. 兼容与回滚

- 全部为**增量暴露**:既有四 MCP 工具、GUI Stop/Esc/打字注入、DAEMON-API 既有章节不动。
- 预算放宽是唯一「破坏性」变更(锁数字变化),影响面 = smoke 断言一处。
- 回滚 = revert 单个实现 commit;无 schema / 无持久化变更,daemon 二进制不换。
