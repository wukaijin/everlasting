# GCE-M3 控制面:打断/注入/跟随对外暴露

## Goal

讨论从「放电影」变「可驾驶」:外部调用方(MCP 宿主 agent、脚本)与 GUI 用户**同权**获得打断 / 注入 / 实时跟随三种控制能力。

上游依赖已全部就绪:P0 打断最小语义(task `09-06-gc-p0-preempt-min-semantics`)交付了内核——`preempt_group_chat(session_id)` 收束式打断 + busy 注入(controls 缓冲,双轨标记落库)。M3 本体 = **暴露层**,定位见 `docs/GROUP-CHAT-API-ROADMAP.md` §4。

## 已定决策(brainstorm 2026-09-06)

| 决策 | 结论 | 依据 |
|---|---|---|
| follow 暴露形态 | **裸 SSE 透传**——仅补 DAEMON-API.md 消费专章,daemon 零改动;不做聚合进度事件 | daemon 已在全局流发出群聊事件(Speaker/Delta/Done),M1/M2 轮询已够用;延续 M0-M2「daemon 零改动」原则 |
| 打断/注入权限粒度 | **沿用 v1 零鉴权,全域可打断/注入** | 与 M2 五决策「v1 零鉴权(本机)」同构;粒度推迟到 M4 远程暴露时随认证一体议 |
| MCP wire 预算 | **2300 → 3200 字符(≈800 token),两侧锁同步** | 对照实际使用(一场讨论数十万 token,清单只注入一次)增量可忽略;描述保持现有密度不刻意压缩 |
| 评审门(review.md 甄别后) | **P1-1 采纳**:inject 加前置 busy guard(对已收官 session 的 cancel 止损有损——重启编排器无条件抹旧 summary);**P2-1 采纳**:AC1 断言放宽(同刻自然收官竞态);**P3-4 部分驳回**:「store 层无 transport 引用」不成立(`chat.ts:788` `diff_worktree` 先例),其余锚点修正照单 | review.md 2026-09-06;P1-1 机制经源码复核(`session_crud.rs:925-943` + `group_chat_loop.rs:322` + `chat.rs:698-703`) |

P0 已承(不再议):收束策略 = 等在途 speaker → moderator 收束轮 → `stop_reason=preempted`(失败重试 1 次兜底立断);注入 schema = user 行 + `metadata.kind="user_inject"` + `[用户插入] ` 前缀(spec `pattern-group-chat-preempt-inject.md`)。

## Confirmed Facts(代码勘察结论)

1. **preempt 端点已全链落地**:Tauri command(`commands/cancel.rs:135`)+ daemon HTTP `POST /api/v1/cancel/preempt_group_chat`(`daemon/routes/cancel.rs:39`,body `{session_id}` → `{"preempted": bool}`)+ DAEMON-API.md §4 已文档化。无进行中讨论时报错。**缺 JS 封装**(`group-chat-run.mjs` 只有 rid 域 `cancelChat`)。
2. **注入 HTTP 路径已存在**:`POST /api/v1/agent/chat` 恒返回 JSON `ChatAcceptance`(daemon/routes/agent.rs:46),三态 `Started | Queued{id,position} | Injected`(chat.rs:164-174);busy 群聊 → `Injected`(controls 缓冲,双轨标记由编排器轮头 `insert_user_inject` 落,`db/sessions/session_crud.rs:853`)。纯图片注入被拒。**MCP `inject_message` = 该路径薄包装,零 daemon 改动**。
3. **GUI 两模式调用形状统一**:Tauri transport 纯透传(`tauri.ts:19`,command 已注册);http transport 顶层 key 自动 camelCase→snake_case(`http.ts` 头注)→ `invoke("preempt_group_chat", { sessionId })` 两模式通用。**全前端现无调用方**(P0 明言按钮随 M3)。
4. **SSE follow 基础在**:单全局流 `GET /api/v1/stream`,`ChatEventPayload` 带 `request_id` + `session_id`(后端回填,sse.rs:22-27)→ 消费方可按 session 过滤;群聊事件 = `chat-event` 通道 `Speaker{speaker}` / `Delta` / `Done{stop_reason,usage}`(event.rs:64-141);Last-Event-ID 重放 + `stream-resync` sentinel + snapshot 补齐机制已有(http.ts 头注);文档零散(DAEMON-API §4/§5/§7),**无外部消费专章**。
5. **MCP wire 预算撞锁**:`TOOLS_BUDGET_CHARS = 2300`(`group-chat-mcp.mjs:265` + smoke `:20`)已用 1945,加两工具必超(评审实测六工具 2881 chars,3200 余量 ≈10%);`group-chat-mcp.test.mjs` 从模块 import 该常量自动跟随。
6. **归因现状**:`created_via` 只在建群时写入且 Rust 不读;打断/注入零归因载体。Q2 已决沿用零鉴权 → **不补归因**。
7. **inject 对已收官 session 的误发是真损失**(评审 P1-1,源码复核成立):空闲群聊 session 收到 chat 消息会**重启编排器**(chat.rs:698-703),开头无条件 `clear_group_chat_lifecycle`(group_chat_loop.rs:322 → `db/sessions/session_crud.rs:925-943` 抹 `stop_reason` + `discussion_summary`)——上一场收官 summary **永久丢失**,事后 cancel 救不回(本场被 cancel 无 summary,finalize 落 `cancelled`)。而「拿 result 的 session_id 顺手注入」恰是 MCP 宿主最可能的误用。故防护必须前置(见 R1);`request_id` 由调用方生成(buildChatBody),cancel 仅作竞态兜底。
8. **GUI 群聊状态源**:群聊讨论中 streaming=true(整场,`isCurrentSessionStreaming`,chat.ts:599);终态 `Done{stop_reason=preempted}` 已随 P0 交付——finalize 白名单在 `streamEvents.ts:616-627`(isTerminal 含 preempted)+ notice 在 `streamController.ts:543`。按钮可见性可挂 `streaming && isGroupChat`。

## Requirements

- **R1 MCP 工具 ×2**(JS 层,daemon 零改动):
  - `interrupt_discussion(session_id)` → 共享层新增 `preemptGroupChat(base, sessionId)` helper → preempt 端点 1:1;响应带「收束中,轮询 discussion_status 到终态后取 discussion_result」指引。
  - `inject_message(session_id, text)`(仅文本,zod 校验)→ **前置 busy guard(主防护)**:复用 `findSession`(mcp.mjs:145,coreStatus/coreResult 共用)判定 busy=true 才继续;非 busy / 已收官 → **不 fireChat** 直接语义报错(零副作用,连 Started 都不产生);guard 通过后复用 `fireChat`,acceptance 必须 `Injected`,非 Injected 时用自有 rid 即时 cancel(降级为竞态兜底)+ 语义报错(「目标不是进行中的群聊,发起新讨论请用 start_discussion」)。
- **R2 GUI 同权入口**:群聊讨论中(sending && 群聊 session)暴露打断入口,调 `preempt_group_chat`;Stop(Esc/按钮,cancel_chat 硬停)语义不动。
- **R3 SSE follow 消费文档**:DAEMON-API.md 新专章——全局流端点、事件通道表、群聊消费序列(Speaker→Delta→Done)、session_id 过滤、Last-Event-ID 重放 / stream-resync / snapshot 补齐、零订阅者 8s 快拒对 ask 的影响(M0 已档)。
- **R4 预算与回归**:wire 预算锁两侧同步 3200(`mcp.mjs` + smoke);`node --test` 两套全绿;GUI store 层单测;live 全链验收。

## Acceptance Criteria

- [x] AC1 live 两场(session `2ba779ec` / `dc9f518b`):preempt 受理 `{"preempted":true}`,终态分别实测 `stop_reason=preempted` 与 `group_chat_end`(后者 = P2-1 预测的同刻自然收官竞态,放宽断言被实跑验证),两场 `discussion_summary` 均非空;空闲 session 报错由端点语义 + 单测透传路径锁定。
- [x] AC2 live:注入受理 `{"status":"injected"}`,`[用户插入]` 行落库(seq=6/7),DB 实证 `metadata={"kind":"user_inject"}`(load_session wire 不吐 message.metadata,驱动首查 MISS 系断言错位非后端缺陷),moderator 收束轮 summary 明确吸收插入指令;guard 零 fireChat / 竞态止损由 `group-chat-mcp.test.mjs` 单测锁定。
- [x] AC3 GUI 按钮走同一 `preempt_group_chat`(store action `chatPreempt.test.ts` 4 例锁两 transport 调用形状 + toast 三态);`pnpm build` 类型链过;Stop/Esc/打字注入零回归(全量 1596 vitest 绿);「讨论中可见」为平凡 v-if(Playwright 按 implement.md 裁定跳过:SSE 流状态无法确定性 route-mock)。
- [x] AC4 预算锁两侧 3200(smoke 实测 2876 chars ≈719 token,余量 ≈10%);`node --test` 两套 11+16 全绿;smoke 六工具名单 PASS。
- [x] AC5 live 按 §6.2 文档照做消费(`out/gce-m3-live-driver.mjs`):speaker/delta/done 事件流 + 按 session_id 过滤 + 场级终态 done;文档 kind 全集按实测补全(`start`/`turn_usage`/`turn_complete`/`signature_delta` 原漏,已补)。

## Out of Scope

- per-speaker child token 立杀(P0 PRD 标记 M3 增强,非本体;收束式是已决产品行为)。
- P1a checkpoint 落库 / 续跑(内部线独立项,M4 定时审议容错前置)。
- 远程暴露认证 + 打断权限粒度机制(M4 随认证一体议);transport 扩 http(M4)。
- 纯图片注入(P0 已明确不支持,路由层报错;MCP 工具不设 images 参数)。
- 打断/注入的归因载体(Q2 已决不补;`created_via` 维持建群时写入现状)。
