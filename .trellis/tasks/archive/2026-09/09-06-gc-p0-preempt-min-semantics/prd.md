# 群聊 P0 打断最小语义:preempt 信号 + schema 区分

## Goal

给群聊编排器补上「打断 / 注入」的最小内部语义——SharedTurnState preempt 信号 +
preempt/inject schema 区分。这是 GCE-M3 控制面(interrupt_discussion / inject_message
API+MCP 工具)的**硬前置**(GROUP-CHAT-API-ROADMAP §4/§6 依赖矩阵),属群聊**内部改进线**,
不含 M3 的对外暴露。

**修的真缺陷(现状盘点实证)**:

1. **busy 期间新消息 = 毁场**。群聊不经 F1 消息队列(`chat.rs:346` `group_chat_ctx.is_some()`
   → legacy 路径),busy 时新的 `chat` 调用走 3a 防御性取消(`chat.rs:505`
   `cancel_inflight_for_session` + await)——杀掉在跑的整场讨论再起新轮。GUI 前端对这个
   路径**有意包装成抢占式打断**(`chatSendActions.ts` D9-Q4:群聊流式中发送先 `await cancel()`
   再发);**daemon HTTP API 无此包装,headless 调用方(M1/M2 消费语境)误发一条消息即毁场**。
2. **只有全场死,没有收束式打断**。现有唯一干预是 CancellationToken 整场取消
   (stop_reason=`cancelled`,GC2 落库)——在途发言被斩、无 summary,与 M3 AC①
   「打断后 stop_reason 可区分且 summary 不丢」无对应机制。

## Background(已确认事实)

- **编排器**:`group_chat_loop.rs` 外层循环 = moderator 轮(max_turns=1,持 nominate/end
  拦截)→ 读 SharedTurnState → participant 轮(max_turns=20)→ 循环;
  `MAX_ORCHESTRATION_ROUNDS=30`;GC5 错误熔断 3 连;轮头有 `token.is_cancelled()` 检查。
- **SharedTurnState** = `Arc<Mutex<GroupChatTurnState{next_speaker, discussion_ended,
  end_summary}>>`(`nominate_speaker.rs:28-45`),在 `run_group_chat_loop` 内创建
  (`group_chat_loop.rs:275`),由 moderator 工具拦截回写、编排器轮间读。preempt 信号的
  自然挂点(roadmap 原文点名)。
- **注入投递几乎免费**:`role_history` 对 `Role::User` 行原样透传
  (`group_chat_prompts.rs:142`)——注入消息只要落库,下一轮 moderator reload 天然可见
  (当轮 in-flight speaker 看不到,轮边界语义,可接受)。
- **落库通道现成**:`db::persist_turn(pool, session_id, Role::User, ...)` 支持 user 行,
  seq caller-managed。
- **stop_reason 现值集**:终态 `group_chat_end` / `max_rounds` / `cancelled` / `error`
  (+非终态 `nominee_unknown` / `participant_unresolved`);新增值涉及 GC2 落库、前端
  finalize 白名单 + notice;M1/M2 驱动对任意终态值天然兼容(DAEMON-API §4 文档补列即可)。
- **GUI 打字现状 = 毁场式抢占**:`chatSendActions.ts`(D9-Q4,07-29 Phase 4)群聊流式中
  发送先 `await cancel()` 再走正常发送 = cancel 整场 + 新消息作 round-0 重开
  (旧场 stop_reason=`cancelled`、无 summary);F1 落地时明示「群聊保持抢占语义不变」。

## Requirements

- **R1 注入通道(非破坏)**:群聊 busy 期间到达的用户消息不再走 3a 毁场路径——落库
  (带注入标记)+ 编排器在下一 moderator 轮边界自然投递,讨论继续。GUI 打字即注入
  (去掉先-cancel 分支);不 busy 时行为不变(正常开新场)。
- **R2 preempt 信号 + 收束轮**:GroupChatTurnState 增 `preempt_requested`;新命令
  `preempt_group_chat(session_id)` 经 session 键注册表置位;编排器轮头检测到 →
  **等在途 speaker 跑完**(不立杀流)→ moderator 收束一轮(复用 end_discussion 落
  discussion_summary)→ 终态 `stop_reason="preempted"`;收束轮失败 1 次重试后强制立断
  兜底(summary 可能缺,如实呈现)。
- **R3 schema 区分**:注入消息与普通用户消息在持久化形状上可区分(role=user 归属不变,
  标记方案 design 定),moderator prompt 增对应识别指引——为 M3 `inject_message` 的
  schema 决议打底(M3 AC②)。
- **R4 注册表生命周期**:preempt 注册表随讨论生灭(spawn 注册 / 编排器全退出路径清理),
  同 `cancellations` 先例;复用 session 二跑不残留。
- **R5 回归闸**:机制层 MockProvider 单测覆盖新语义(C3.1 先例:破坏剧本确定性断言);
  prompt 结构变更 → 跑一次 turn-smoke 行为层验证。

## Acceptance Criteria

- [ ] AC1 群聊 busy 期间经 API 发消息:讨论不死(stop_reason 不变、busy 持续),消息
      落库且在下一 moderator 轮可见并影响后续走向(MockProvider 确定性断言 + 转录可查)。
- [ ] AC2 `preempt_group_chat` 后:在途 speaker 跑完 → 收束轮产出 discussion_summary →
      终态 `stop_reason="preempted"`(与 `cancelled` / `group_chat_end` 可区分);
      收束轮失败路径走立断兜底且 stop_reason 仍可区分。
- [ ] AC3 注入消息持久化形状与普通用户消息可区分;schema 决议落 spec(.trellis/spec)。
- [ ] AC4 GUI:群聊 busy 打字 = 注入(消息即时可见、无 cancel 副作用);经典聊与
      群聊不 busy 路径行为逐字节不变;群聊外 3a 语义不动。
- [ ] AC5 既有回归全绿:`cargo test -p everlasting --lib`(tests_group_chat 家族)、
      前端 vitest + vue-tsc、clippy;prompt 结构变更后 `scripts/turn-smoke.sh` live 一轮。

## Out of Scope

- M3 本体:daemon 端点文档化 + MCP 工具(`interrupt_discussion` / `inject_message`)、
  SSE follow 外部消费(等本任务落地后立项;`preempt_group_chat` 命令即其 1:1 内核)。
- P1a checkpoint 落库 / 续跑(打断的信任底座,独立项)。
- 立杀在途 speaker 流(per-speaker child token;M3 interrupt 增强)。
- C1.1 ask-free / C1.2 token 预算(止损包,另一条线)。
- 「打断(收束轮)」的 GUI 按钮(随 M3 GUI/API 同权一起)。
