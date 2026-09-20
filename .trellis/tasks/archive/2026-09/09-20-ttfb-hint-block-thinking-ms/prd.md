# 流式反馈两修复:TTFB 空窗思考占位 + thinkingMs 块级化

## 背景(用户报告,2026-09-20)

1. **TTFB 空窗无反馈**:web 端发送 prompt 后、LLM 首个思考 token 到达前(TTFB 窗口,
   常见数秒),消息区无任何变化——只有输入框流光 + Stop 键这类全局效果。
   根因:后端 `ChatEvent::Start` 在 LLM HTTP 响应头到达后才发射
   (`anthropic.rs:128-140` 等 provider 同理);前端 `showStreamingHint`("streaming…")
   挂在 ThinkingBlock 内、依赖首个 `thinking_delta` 才能渲染;且 assistant 占位在
   push 时不带 `streaming: true`,连 ▍ 光标都不亮。

2. **思考时长串写**:流式期间(未落盘 reload),消息里所有 thinking 块的
   "Thought for Xs" 会被**最后一次** `turn_complete` 的 `thinking_ms` 全部覆盖。
   根因:多 turn 共用一个 assistant 占位(非群聊 `start` 不 push 新占位),
   `thinkingDurationMs` 是消息级单值覆盖写;每个 ThinkingBlock 都绑同一个
   `message.thinkingDurationMs`。reload 后按行拆分 + 按 seq 挂回才恢复正确。

## 方案(纯前端,零后端改动)

### 修复 1:TTFB 空窗占位

- `chatSendActions` / `chatMessageActions`(resend)/ `streamEvents.turn_continuation`
  push 的 assistant 占位带 `streaming: true`(先例:`adoptForeignRequest` 的
  外来占位,注释明言 "delta-first 时 start 不会来设置")。
- `MessageItem` 空态(`streaming && !hasVisibleBubble && !useTimeline`)渲染
  "正在思考…" 占位行(accent-text + 慢呼吸动画)。

### 修复 2:thinkingMs 下沉块级

- `ContentBlockView` thinking 变体加 `thinkingMs?: number`;
  `TimelineItem` thinking 变体透传;`MessageItem` timeline 分支
  `item.thinkingMs ?? message.thinkingDurationMs`(块级优先,行级回退 ——
  reload 后行级即该 turn 的正确值,天然兼容)。
- `RequestState` 加 `turnStartBlockIdx`;`case "start"` 记录当前
  `contentBlocks.length`;`case "turn_complete"` 只给
  `[turnStartBlockIdx, len)` 区间内 thinking 块打 `thinkingMs`
  (消息级 `last.thinkingDurationMs` 保留最后写,不破坏现有消费者)。

## 验收标准(Acance Criteria)

- [ ] 发送后空窗期消息区出现"正在思考…"(停止键/流光之外的首个 assistant 侧反馈);
      首个 thinking_delta 到达后被真实思考块取代。
- [ ] 多 turn 工具循环流式中,每个 thinking 块显示各自 turn 的思考时长;
      reload 后显示不变(仍是各自 turn 值)。
- [ ] vitest:`streamController.test.ts`(块级打标多 turn 用例)+
      `MessageItem.test.ts`(空窗占位 + 块级 header)+ 既有套件全绿。
