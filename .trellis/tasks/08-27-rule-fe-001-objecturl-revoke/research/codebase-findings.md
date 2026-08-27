# RULE-FE-001 代码链路研究(2026-08-27)

> 结论先行:DEBT.md 记录的修复方向("给 `reloadAfterFinalize` 加替换钩子")
> 建立在一个**已过时的前提**上——乐观渲染实际上**不依赖** `localUrl`。
> 渲染层从 B1 PR5(`3a2abc30`)落地第一天就是 `file` 优先(daemon GET 路由),
> blob URL 只是 file 缺失时的防御回退,而该回退窗口在现行 send 流程里不存在。
> 因此修复可以(且应该)收敛为:**send 成功释放 strip 时直接 revoke**,
> 与既有 `discardStagedImages` 同款,零 reloadAfterFinalize 改动。

## 1. 泄漏面画像

objectURL 的完整生命周期(`app/src/stores/chatSendActions.ts`):

| 路径 | 位置 | revoke? |
|---|---|---|
| 创建:`addStagedImages` | `chatSendActions.ts:145`(`url: URL.createObjectURL(staged)`) | — |
| ✕ 单张移除:`removeStagedImage` | `chatSendActions.ts:166` | ✅ |
| 整条弃置(session 切换):`discardStagedImages` | `chatSendActions.ts:173` | ✅ |
| **send 成功释放 strip** | `chatSendActions.ts:478`(`stagedImages.value = []`) | ❌ **泄漏点** |

库内其余 createObjectURL 均已自洽:`readImageDimensions`(:84,onload/onerror 双路 revoke)、
`utils/imageCompress.ts:120`(同款 throwaway + revoke)。**唯一泄漏就是 send 路径。**

量级:单轮最多 10 张 × 压后 ≤5MB(`MAX_STAGED_IMAGES` / `MAX_IMAGE_BYTES`),
即**每轮带图消息最多 ~50MB blob 挂到页面卸载**;长会话多轮累积,无上界。
Tauri WebView 与浏览器同引擎,blob 常驻进程内存。

## 2. 债条前提为何过时(关键证据链)

TODO 注释(`chatSendActions.ts:471-477`)与 DEBT.md 描述均称:"the optimistic
user message still renders from `metadata.attachments[].localUrl` until the
finalize reload replaces it"。逐条核对渲染链:

1. **渲染映射**:`MessageItem.vue:495-523`(`messageImages` computed)把
   `metadata.attachments` 透传为 `{file, localUrl, mediaType}`,file 与
   localUrl **都可能是 undefined**,两者取其一即可渲染。
2. **取 URL 规则**:`MessageImages.vue:42-45`(`urlFor`)——
   `if (img.file) return attachmentUrl(sessionId, file); return img.localUrl;`
   **file 优先**,blob URL 仅是 file 缺失时的回退。
3. **回退窗口不存在**:send 流程中 `uploadStagedImages` 在乐观消息 push
   **之前** await 完成(`chatSendActions.ts:353`),任一张失败则整轮中止
   不 push(:354-362)。所以乐观 manifest 的每个 entry 必有 `file`
   (`uploaded.map` 同时写入 `file` 与 `localUrl`,:399-404),
   `urlFor` 永远走 daemon 路由,**blob URL 从不被渲染**。
4. **历史核对**:`git show 3a2abc30:app/src/components/chat/MessageImages.vue`
   显示 file 优先自 B1 PR5 首次落地即如此——TODO 注释写错了,不是后来翻转。
5. **旁证**:`MessageImages.vue` 头注释本来就写着 "the daemon route outlives
   the blob (**which is revoked once the staged strip is cleared**)"——设计
   意图就是 strip 释放即 revoke,send 路径漏掉了这一半。

## 3. localUrl 全部消费方(复核无遗漏)

| 消费方 | 位置 | 对 revoke 的影响 |
|---|---|---|
| `MessageItem.messageImages` | `MessageItem.vue:495` | 透传,不取 URL |
| `MessageImages.urlFor` | `MessageImages.vue:42` | file 优先;entry 必有 file → 不触 localUrl |
| `toPayloadAttachments`(wire) | `chat.ts:840-869` | **只读 `file`**,localUrl 从不上 wire |
| `AttachmentView` 类型 | `chat.types.ts:282-290` | 纯类型 |

DB 侧:后端从 wire `AttachmentRef` 重建 manifest,`rehydrateMessages`
(`streamRehydrate.ts:190`)产出 snake_case `{file, media_type, ...}`,
**blob URL 从不落库、rehydrate 产物从不含 localUrl**。
=> send 后 revoke,乐观消息里残留的 localUrl 字符串是 dangling 引用,
无任何渲染/wire/落库消费方,安全。

## 4. 方案对比

### Option A(选定):send 释放 strip 时直接 revoke

在 `chatSendActions.ts:478` 处,`stagedImages.value = []` 前对
`uploaded[].localUrl` 逐个 `URL.revokeObjectURL`(成功路径才有 `uploaded`,
上传失败早退保条,天然不 revoke)。

- 镜像 `discardStagedImages` 先例,改动 ~3 行 + 注释改写。
- 覆盖 send 的一切后续路径:正常 finalize、F1 排队占位
  (queued userMsg 携带 dangling localUrl 直到 `TurnContinuation` 物化,
  渲染 file 优先不受影响)、cancel / interrupted reload、LRU 驱逐、
  session 切换——因为 revoke 早在 send 时刻已完成。

### Option B(否决):reloadAfterFinalize 替换钩子

DEBT.md 记录的原方向:reload 替换消息数组时扫描旧消息 manifest revoke。

- `reloadAfterFinalize`(`streamEvents.ts:1233`)是高频枢纽,3 类调用方
  (stream finalize `finalizeRequest` / interrupted 恢复 :1534 / 手动
  compact `chatSessionActions.reloadSessionMessages`),且函数内还有
  checklist rehydrate、quota refresh、latency IPC 等职责——为一个已证
  不存在的渲染依赖动它,风险与收益不成比例。
- 还需处理 reload 失败(`load_session` 返回 null → 消息不替换 → revoke
  与否的语义)、乐观消息在 queued 状态滞留多个 turn 等边界。
- 前提("乐观渲染依赖 localUrl")已被 §2 证伪。

## 5. 边界与不变量

- **上传失败**:整轮中止 + strip 保留(P1-3 既有契约),此时**不得**
  revoke——图还要在暂存条上渲染。Option A 天然满足(revoke 只在
  `uploaded` 构建成功后执行)。
- **staged 参数**:`ChatInput.vue:265` emit → `ChatPanel.vue:960` →
  `ChatWindow.vue:80` `store.send(text, staged)`,`staged` 即
  `chatStore.stagedImages`(同一数组);slash-command 路径
  (`ChatInput.vue:679`)不传 staged,无图不受影响。
- **F1 排队**:流式中发送走后端队列,乐观 userMsg 先以 queued 占位渲染,
  `TurnContinuation` 物化——全程 file 优先渲染, dangling localUrl 无消费。
- **乐观 manifest 保留 localUrl 字段本身**:不动。它是
  `AttachmentView` 文档化的防御形态(pre-upload 回退),删字段收益为零。

## 6. 测试现状与策略

- 现存测试**零覆盖** send 的 staged 路径:`grep createObjectURL|revokeObjectURL
  *.test.ts` 无结果;`chat.test.ts` 不驱动 send;`MessageImages.test.ts`
  只测组件层 file/localUrl 取 URL 规则(纯字符串,不涉 revoke)。
- jsdom 无 `URL.createObjectURL`/`revokeObjectURL` 实现,测试需
  `vi.stubGlobal` 或 `Object.defineProperty(URL, ...)` 注入 spy
  (参考 `messageQueueStream.test.ts` 的 transport mock + pinia 模式,
  直接构造 `StagedImage`(假 url)经 `send(text, staged)` 驱动,
  绕过 `addStagedImages` 的真实 createObjectURL)。
- 顺带补 `removeStagedImage` / `discardStagedImages` 的 revoke 断言
  (同文件、同 spy,边际成本≈0,补齐 B1 生命周期的回归锁)。

## 7. 相关 spec / 文档

- `.trellis/spec/frontend/state-management.md` §Stream Controller Pattern
  (chatSendActions 拆分契约、store-owned strip 生命周期 design §5.1)
- `.trellis/spec/frontend/index.md`(前端 Pre-Development Checklist)
- `.trellis/spec/frontend/quality-guidelines.md`(Quality Check)
- `app/src/stores/chat.types.ts:249-290`(StagedImage / AttachmentView 契约注释)
