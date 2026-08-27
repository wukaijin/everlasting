# Design — RULE-FE-001 staged 图片 objectURL 发送后 revoke

## 1. 选型:为何偏离 DEBT.md 记录的修复方向

DEBT.md Fix 栏记录"给 `reloadAfterFinalize` 加替换钩子",其前提(TODO
注释原话)是"乐观消息渲染依赖 `localUrl` 直到 finalize reload 替换"。
研究证伪了该前提(research §2):

- `MessageImages.vue:42` `urlFor` **file 优先**(daemon GET 路由
  `attachmentUrl`),自 B1 PR5(`3a2abc30`)首次落地即如此;
- send 流程 upload 先于乐观 push 完成、失败即整轮中止
  (`chatSendActions.ts:353-362`),故乐观 manifest 每个 entry 必有
  `file`,blob 回退分支从不执行;
- `MessageImages.vue` 头注释的设计意图本就是 "blob is revoked once
  the staged strip is cleared"——send 路径漏了 revoke 这一半。

因此 Option A(**send 时直接 revoke**)以 ~3 行改动达到债的完整闭合;
Option B(reloadAfterFinalize 钩子)要动 3 类调用方共用的高频枢纽
(stream finalize / interrupted 恢复 / 手动 compact),还要额外定义
reload 失败、queued 占位滞留多轮等边界语义——为一个不存在的渲染依赖
付这些复杂度,不成比例。**选 A。**

## 2. 改动点(唯一代码改动)

`app/src/stores/chatSendActions.ts` `send()` 尾部,现 `:471-478`:

```ts
// 现状(泄漏点):
stagedImages.value = [];

// 改为:
for (const u of uploaded) URL.revokeObjectURL(u.localUrl);
stagedImages.value = [];
```

- **为何遍历 `uploaded` 而非 `stagedForTurn`**:`uploaded` 仅在
  `stagedForTurn.length > 0` 且 `uploadStagedImages` 成功后非空
  (失败路径早退 return,R2 天然满足);其 `localUrl` 字段就是
  `staged[i].url` 的原值(`uploadStagedImages` :201),与乐观 manifest
  携带的逐一对应。
- **revoke 在 `startRequest` 之后**:strip 释放点本就在 `startRequest`
  之后(现状顺序),沿用;`startRequest` 异常则异常传播,strip 未清,
  与现状一致。
- 注释改写:TODO(B1 follow-up)段替换为三路 revoke 语义说明
  (remove / discard / send),并指出渲染层 file 优先
  (`MessageImages.urlFor`)使 revoke 对乐观渲染不可见。

注释同步:`chat.types.ts:251` `StagedImage.url` 文档
"revoked on remove / session switch" → "revoked on remove / session
switch / successful send"。

## 3. 不变量(为何 send 时 revoke 对一切后续路径安全)

| 后续路径 | 为什么不受影响 |
|---|---|
| 乐观消息渲染 | `urlFor` file 优先;entry 必有 file(research §2) |
| wire / DB | `toPayloadAttachments` 只读 `file`;rehydrate 产物 snake_case 无 localUrl(research §3) |
| F1 排队占位 | queued userMsg 复用乐观消息对象,dangling localUrl 无消费方 |
| cancel / interrupted / 手动 compact reload | reload 只替换消息数组,不触 blob URL |
| 点击放大(`window.open`) | `openImage` 走 `urlFor` → daemon 路由 |
| session 切换 | strip 已清,`discardStagedImages` no-op,幂等 |

## 4. 测试设计

新文件 `app/src/stores/chatSendActions.test.ts`(store 测试平铺惯例):
B1 strip 生命周期此前零覆盖,本任务一并补齐。

- **脚手架**:仿 `messageQueueStream.test.ts` —— `vi.mock("../transport")`
  (`load_session` → `{messages:[], ...}`、`save_attachment` →
  `{file:"att-x"}`、其余 null)+ `createPinia/setActivePinia` +
  `vi.stubGlobal` 或 `Object.defineProperty(URL, "createObjectURL"/"revokeObjectURL")`
  注入 spy(jsdom 无原生实现)。staged 对象手工构造(假 `url` 字符串),
  绕过 `addStagedImages` 的压缩/读尺寸链路。
- **用例**:
  1. `send(text, staged)` 成功:每个 localUrl revoke 恰好一次,
     `stagedImages` 清空(AC1);
  2. `save_attachment` reject:零 revoke、strip 保留、返回后 store
     状态可重试(AC2);
  3. `removeStagedImage(i)` / `discardStagedImages()`:对应 URL revoke
     (AC3);
  4. 纯文本 send(无 staged):零 revoke(防误伤)。
- **不做**:真实 blob / `createObjectURL` 端到端(泄漏本身用
  DevTools memory 面板人工验证即可,单测锁调用次数契约)。

## 5. 回滚

单文件 3 行 + 注释;`git revert` 单 commit 即回滚,无数据面 / wire 面
影响。
