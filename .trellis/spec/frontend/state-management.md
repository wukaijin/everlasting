# State Management

> How state is managed in this project.

---

## Overview

State management is Pinia-based (`defineStore` in `app/src/stores/`). The two
core stores are `chat.ts` (thin facade over the controller) and
`streamController.ts` (owns in-memory message buffers + SSE event handling).

---

## Stream Controller Pattern (2026-08-08, 08-07-large-file-splitting 沉淀)

`streamController.ts` is the single source of truth for in-memory messages
(reactive `Map<sessionId, ChatMessage[]>`), in-flight request state
(`activeRequests` / `completedRequests`), and the SSE event pipeline.

**Module layout** (拆分后):

- `streamController.ts` — store: state + internal helpers + public API + return block
- `streamEvents.ts` — event handling block (`createStreamEventHandlers(ctx)` factory)
- `streamRehydrate.ts` — `rehydrateMessages` 纯函数(DB 载荷 → 内存 ChatMessage)

**事件块拆分契约(必须遵守)**:事件处理函数与 store 状态/公共 API
互相引用(`handleChatEvent` → `start`/`ensureLoaded`,`refresh` →
`finalizeRequest`),因此:

- 事件块以 **工厂 + ctx 注入** 拆出:`createStreamEventHandlers(ctx)` 顶部
  一次解构 ctx,函数体原样保留(零逐函数改动)。
- ctx 在 store 的 helpers 之后**一次填充**:公共 API 全是提升的
  `function` 声明,可在定义前引用;state/helpers 是 const,须先定义。
- **return 块导出约束**:`handleChatEvent` / `handleToolCall` /
  `finalizeRequest` / `putMessages` / `pinnedSessions` / `loadedFromDb`
  是测试直调入口(`streamController.test.ts` /
  `streamController.review.test.ts`),拆模块后必须经 return 块
  re-export,不允许测试改从事件模块直接 import 内部符号。
- store 内调用事件函数一律走 `events.X`,return 块用
  `finalizeRequest: events.finalizeRequest` 形式保留原导出名。

---

## Chat Store Action Clusters Pattern (2026-08-10, 08-10-chat-store-split 沉淀)

`chat.ts` 是 Pinia **setup store**(`defineStore("chat", () => {...})`),
所有 action 是闭包内 `function` 声明、共享同一份 state(refs / computeds /
controller)。当 store 体量超 1200 行时,按职责簇拆为 `createXxxActions(ctx)`
工厂文件,**复用 streamEvents 的 factory + ctx 注入契约**(见上节)。

**Module layout** (拆分后,2026-08-10):

- `chat.ts` (~960 行) — hub:模块级 export(`resolveModelInput` /
  `parseForcedDispatchPrefix` / `thinkingBlocksToText`,被测试直接 import)
  + setup 头(state refs / computeds / project-change watcher)+
  `onProjectChange` + diff 簇 + `toPayloadContent` + `cancel` +
  4 个 `createXxx(ctx)` 调用 + return 块
- `chatSessionActions.ts` — 会话 CRUD + worktree(13 个 action,
  `createSessionActions(ctx)`)
- `chatModeActions.ts` — mode / yolo / workflow(2 个 ref + 6 个 action,
  `createModeActions(ctx)`)
- `chatMessageActions.ts` — edit / resend / retry(3 个 action,
  `createMessageActions(ctx)`)
- `chatSendActions.ts` — send + B1(2026-08-16)起拥有的 paste-staging
  strip 状态(`stagedImages` + add/remove/discard,`createSendActions(ctx)`;
  生命周期契约见下节)

**拆分契约(必须遵守)**——与 streamEvents 同构,补充 chat 特有约束:

- **factory + ctx 注入**:每个簇 `createXxxActions(ctx)`,顶部一次解构 ctx,
  函数体原样保留(**零逐函数改动**,含 JSDoc)。
- **ctx 在 hub 的 state/helpers 之后一次填充**:action 全是提升的
  `function` 声明;state/helpers 是 const,须先定义。
- **循环依赖消除**:跨簇 action 经 ctx 互注(sessions 簇的
  `createNewSession` 注入 send 簇;send 簇留 hub 的 `cancel` 注入
  sessions/message/send 三簇)。`cancel`(5 行循环枢纽)留 hub——
  搬走会造成工厂初始化顺序的死锁。
- **模块级共享 helper 留 hub**:`toPayloadContent`(send/message 共用)+
  `genId` / `ContentBlockPayload` / `ChatMessagePayload` 加 `export`
  供簇 `import`(对标 streamController.ts `export genId` 先例)。
- **watch immediate 与工厂初始化时机**:hub 的 `watch(currentProjectId,
  ..., { immediate: true })` 在定义时同步触发回调,回调调工厂产物
  (`loadSessions`)。因此 sessions 工厂调用须在 watch 定义之前——
  `diffCache` 声明上移到 watch 前(它无依赖),让 sessions 工厂的 ctx
  在 watch 触发时已就绪。
- **return 块导出约束**:store proxy 入口(send/cancel/editMessage/
  resendMessage/retryChat/会话 CRUD/mode-workflow 全套)必须经 return 块
  re-export;三个 chat 测试文件(chat.test.ts / chatMode.test.ts /
  chatSend.test.ts)全经 `useChatStore().X` 调用,**拆分后零改动**。

### Convention: staging strip objectURL 生命周期(RULE-FE-001,2026-08-27 沉淀)

**What**:`chatSendActions.ts` 里每个 `URL.createObjectURL` 都有确定的
revoke 点,共三路——`removeStagedImage`(✕ 单张)、`discardStagedImages`
(session 切换 / 项目变更 watcher)、`send` 成功释放 strip 时(遍历
`uploaded[].localUrl`)。上传失败早退**不 revoke**(strip 保留供重试)。

**Why**:send 时 revoke 之所以安全,是因为渲染层 **file 优先**——
`MessageImages.vue` 的 `urlFor` 优先走 `attachmentUrl(sessionId, file)`
(daemon GET 路由),`localUrl` 只是 file 缺失的防御回退,而 send 流程
upload 先于乐观 push 完成、失败即整轮中止,故乐观 manifest 每条必有
`file`,blob URL 实际从不被渲染。`localUrl` 也从不上 wire
(`toPayloadAttachments` 只读 `file`)、从不落库(rehydrate 产物是
snake_case 服务端形态)。**教训**:RULE-FE-001 登记时记录的修复方向
("给 reloadAfterFinalize 加替换钩子")建立在"乐观渲染依赖 localUrl"的
错误前提上——动 strip 生命周期前先核对 `urlFor` 的实际取 URL 顺序,
别信 TODO 注释里的渲染链描述。

**Test gotcha**:jsdom 无 `URL.createObjectURL` / `revokeObjectURL`
原生实现——测 strip 生命周期时用 `Object.defineProperty(URL,
"revokeObjectURL", ...)` 注入 spy,并手工构造 `StagedImage`(假
`url` 字符串)经 `send(text, staged)` 驱动,绕过 `addStagedImages`
的压缩/读尺寸链路(先例:`chatSendActions.test.ts`)。另注意 hub 的
`watch(currentSessionId, discardStagedImages)` 是 deferred watcher:
测试里 seed strip 前先 `await nextTick()` 排干它,否则 watcher 回调会在
send 的首个 await 处清空 strip 污染断言。

### Convention: 过滤下推服务端后,「全集派生值」全部换服务端口径(RULE-PERM-001,2026-08-30 沉淀)

**What**:把一个 store 的列表过滤从客户端迁到服务端(分页/下推)时,所有曾从
「全集数组」派生的值——计数 chip、空态判据、hasMore——必须换成服务端返回的
口径,逐一排查,不能只改过滤本身。

**Why**:audit store 迁移时踩中的回归——空态文案曾用
`events.length === 0` 区分「会话没有事件」和「过滤无命中」;客户端过滤时代
`events` 恒为全集所以成立,服务端过滤后 `events` 只含命中行,过滤无命中被
误报成「暂无审计事件」。这类 break 是**静默的**:不报错、只在特定数据态显示
错误文案,vitest 不测文案就漏。

**AuditLogModal 落地后的正确口径**(契约见 `stores/audit.ts` 头注):

| 值 | 正确来源 | 错误来源(下推后失效) |
|---|---|---|
| 总数 chip | `totalAll`(服务端) | `events.length` |
| 空态二分 | `totalAll === 0 ? 暂无 : 无匹配` | `events.length === 0` |
| critical 计数 | `totalCritical`(不受 kind 过滤) | 全集 `filter().length` |
| hasMore | `events.length < matched` | `filteredEvents === 全集` 假设 |

**Related**:`.trellis/spec/backend/database-guidelines.md`「审计事件 keyset
分页读」(服务端计数契约:matched / totalAll / totalCritical 随页返回)。

### Convention: 项目添加全模式统一走 DirBrowserModal(2026-09-03 沉淀)

**What**:projects store 的「添加项目」入口是 `openDirBrowser()`——只翻
`dirBrowserOpen = true`,由 AppShell 挂载的 `DirBrowserModal` 提供点选/路径直达
交互,「选择此目录」出口 `addProjectByPath(path)` 走共享注册尾巴
`registerPickedPath`(visible 去重 → hidden unhide 恢复 → create + focus,
RULE-FrontProj-001)。**没有 per-transport 分支**:桌面(Tauri)/浏览器/sidecar/
remote 全走同一条 `browse_dir` IPC(daemon route + Tauri command 双注册)。

**Why**:native `pick_project_dir`(tauri-plugin-dialog)已于 2026-09-03 整链
移除——WSLg 下 GTK 风格对话框观感不合预期(BACKLOG §5.3 用户偏好自渲染),
且它是该插件唯一消费方。**不要在新增交互时想当然加回 `tauri-plugin-dialog`**
(依赖/插件注册/capabilities 权限都已删);要原生对话框就等于重新引入整条链,
先确认真的放弃统一 UX 再动。

**Example**:

```ts
// ProjectTabs / EmptyProjectState 的「添加项目」——就是开模态框,没有 IPC
function onAdd() {
  store.openDirBrowser();
}
```

**Related**:`reka-ui-usage.md`「roving tabindex keyboard nav inside a Dialog」
(模态框键盘导航契约);`browser-regression.md`(e2e 全流程锁定用例)。

---

## State Categories

<!-- Local state, global state, server state, URL state -->

(To be filled by the team)

---

## When to Use Global State

<!-- Criteria for promoting state to global -->

(To be filled by the team)

---

## Server State

<!-- How server data is cached and synchronized -->

(To be filled by the team)

---

## Common Mistakes

<!-- State management mistakes your team has made -->

(To be filled by the team)
