# RULE-FE-001 staged 图片 objectURL 发送后 revoke

## Goal

闭合 `.trellis/reviews/DEBT.md` §RULE-FE-001(P2):带图消息发送成功释放
staging strip 时,乐观 manifest 携带的图片 objectURL 从不 revoke,压缩后
blob(单轮上限 10 × 5MB)在内存里挂到切 session / 页面卸载才释放,长会话
多轮累积无上界。本任务把 strip 释放点补上 revoke,消除无上界占用。

## Background

- 泄漏面画像、**债条前提证伪**(乐观渲染 file 优先,不依赖 localUrl)、
  localUrl 全消费方清单、方案对比见 `research/codebase-findings.md`。
- 要点:DEBT.md 记录的修复方向(给 `reloadAfterFinalize` 加替换钩子)
  建立在过时前提上——`MessageImages.urlFor` 自 B1 PR5 落地起就是
  `file` 优先(daemon GET 路由),blob URL 是永不触发的防御回退;故
  修复收敛为 send 时直接 revoke,零 `reloadAfterFinalize` 改动
  (选型论证见 `design.md`)。

## Requirements

- R1 send 成功路径释放 strip 时,对本轮每个 `uploaded[].localUrl` 调
  `URL.revokeObjectURL`,镜像 `discardStagedImages` 先例
  (`chatSendActions.ts:471-478`)。
- R2 上传失败早退路径不 revoke、strip 保留——P1-3 既有契约
  (无部分发送)不变。
- R3 既有路径零回归:`removeStagedImage`(✕)与 `discardStagedImages`
  (session 切换)的 revoke 语义、F1 排队占位 / cancel / interrupted
  reload / LRU 驱逐各路径的渲染不受影响(渲染层 file 优先,见 research §3)。
- R4 删除 / 改写 `chatSendActions.ts:471-477` 的 TODO(B1 follow-up)
  注释为现行语义;`StagedImage` 类型注释(`chat.types.ts:251`
  "revoked on remove / session switch")同步补 "send"。
- R5 测试:send 成功后逐 URL revoke 恰好一次;上传失败不 revoke 且
  strip 保留;顺带补 `removeStagedImage` / `discardStagedImages` 的
  revoke 断言(补齐 B1 生命周期回归锁)。
- R6 销账:任务闭合时从 DEBT.md 删除 RULE-FE-001 条目,并同步 P2
  section 头计数(1→0)、底部优先级分布表(P2=1→0,Total=13→12),
  仿 RULE-ARGS/SHELL-001 先例在 P2 header 下加闭合注记。

## Non-Goals

- **reloadAfterFinalize 替换钩子**:DEBT.md 原方向,前提已证伪
  (research §2/§4),不做。
- **乐观 manifest 去掉 `localUrl` 字段**:`AttachmentView` 文档化的
  pre-upload 防御形态,无消费成本,保留。
- **readImageDimensions / imageCompress 的 throwaway objectURL**:
  已自洽 revoke,不在本债范围。
- **tool_result 图片(`ToolResultImageRef`)**:走服务端 file 引用,
  无前端 blob,无泄漏面。

## Acceptance Criteria

- [ ] AC1 `send` 成功路径:本轮每个 staged URL 被 revoke 恰好一次,
      单测锁定(spy 注入,jsdom 无原生实现,见 research §6)。
- [ ] AC2 上传失败路径:零 revoke 调用、`stagedImages` 保留,单测锁定。
- [ ] AC3 `removeStagedImage` / `discardStagedImages` 既有 revoke 行为
      有断言锁定(本任务前无覆盖)。
- [ ] AC4 TODO(B1 follow-up)注释删除,`chatSendActions.ts` 与
      `chat.types.ts` 注释描述现行三路 revoke 语义(remove / discard /
      send)。
- [ ] AC5 前端存量测试全绿:`cd app && pnpm test`;lint / type-check
      通过。
- [ ] AC6 销账完整:DEBT.md 删条目 + 计数两处同步 + P2 header 闭合注记
      (Phase 3 收尾时执行,与 RULE-SHELL-001 先例同款)。

## Constraints

- 只动 `app/src/stores/chatSendActions.ts`(+ 注释同步
  `chat.types.ts`);不改 `streamEvents.ts` / `reloadAfterFinalize` /
  渲染组件 / wire 层。
- 不引新依赖、不加 store 字段(revoke 在 action 内联完成)。
- 排队 / 群聊抢占 / slash-command 各 send 分支不单独处理:revoke 点
  在所有分支汇合的 strip 释放处,天然全覆盖。
