# stores/chat.ts 拆分

## Goal

把 `app/src/stores/chat.ts`(2156 行,前端唯一 >1200 行源码文件)按职责簇
拆为 hub + action 簇子模块,降到 **~700 行**;拆分**纯搬迁不改逻辑**,
`pnpm test` 全绿是硬性验收。

## Background / 已确认事实(统计日期 2026-08-10)

### 在总纲中的定位

`08-07-large-file-splitting/prd.md`(全仓 >1200 行文件拆分总纲)把
`chat.ts` 标为 **Out of Scope**——理由是"薄门面,拆无收益"。本任务推翻
该判断:`chat.ts` 经 PR3(facade 化)+ 后续 mode/workflow/edit/resend/retry
五轮功能叠加,**已从"薄门面"长回 2156 行**,action 簇结构清晰(见下表),
拆分收益明确(hub 可导航性 + action 簇独立回滚)。本任务是总纲划出范围
的按批落地——Out of Scope 仅批次区分,非永久排除(总纲守则原文)。

### 现状结构测绘

`chat.ts` 是 Pinia **setup store**(`defineStore("chat", () => {...})`),
所有 action 是闭包内 `function` 声明、共享同一份 state(refs / computeds /
controller)。这与 `streamController.ts` 拆出 `streamEvents.ts` 那轮**完全
同构**——沿用其沉淀的 **factory + ctx 注入**契约
(`.trellis/spec/frontend/state-management.md` §Stream Controller Pattern)。

| 区段 | 行范围 | 行数 | 内容 |
|---|---|---|---|
| 模块级 export(必须留 hub) | 59–192 | 134 | `resolveModelInput` / `parseForcedDispatchPrefix` / `ForcedDispatchPayload` / `thinkingBlocksToText`(被 `chat.test.ts` 直接 import) |
| setup 头:state + computed + watch | 194–527 | 334 | refs / computeds / project-change watcher(共享 state,留 hub) |
| `onProjectChange` | 528–575 | 48 | 项目切换级联(被 watch 调,留 hub) |
| **会话 CRUD** | 577–888 | 312 | `loadSessions`/`createNewSession`/`updateGroupChatConfig`/`switchSession`/`deleteSession`/`clearSessionMessages`/`renameSession`/`setSessionColor`/`attach/detach/delete/publishWorktree` |
| diff 簇 | 890–957 | 68 | `getDiff`/`fetchDiff`/`invalidateDiff`/`getFileDiff`(短小,留 hub) |
| `toPayloadContent` | 959–1052 | 94 | wire 序列化(send/resend/retry 共用,留 hub) |
| **send + cancel** | 1054–1251 | 198 | 核心发送路径 |
| **edit + resend + retry** | 1253–1635 | 383 | 消息编辑/重发/重试 |
| **mode + yolo + workflow** | 1637–2027 | 391 | `requestSetMode`/`confirmYolo`/`cancelYolo`/`requestSetWorkflowEnabled`/`requestSetPluginName`/`listWorkflowPlugins` |
| return 块 | 2029–2156 | 128 | store proxy 导出(必须完整保留) |

### 拆分契约(必须遵守)——复用 streamEvents.ts 沉淀

- **factory + ctx 注入**:每个 action 簇拆出为 `createXxxActions(ctx)`
  工厂,顶部一次解构 ctx,函数体原样保留(**零逐函数改动**)。
- **ctx 在 hub 的 state/helpers 之后一次填充**:action 全是提升的
  `function` 声明,可在定义前引用;state/helpers 是 const,须先定义。
- **return 块导出约束**:`send`/`cancel`/`editMessage`/`resendMessage`/
  `retryChat`/`requestSetMode`/`confirmYolo`/`cancelYolo`/
  `requestSetWorkflowEnabled`/`requestSetPluginName`/`listWorkflowPlugins`/
  会话 CRUD 全套是 store proxy 入口(测试 + 组件经 `useChatStore().X`
  调用),拆模块后必须经 return 块 re-export,**不允许**调用方改从簇
  模块直接 import 内部符号。
- **模块级 export 留 hub**:`resolveModelInput`/`parseForcedDispatchPrefix`/
  `ForcedDispatchPayload`/`thinkingBlocksToText` 被 `chat.test.ts` 直接
  import(不经 store proxy),**不搬迁**。
- **循环依赖消除**:
  - `cancel` 被 sessions 簇(`clearSessionMessages`)、edit 簇、retry 簇
    调用 → `cancel` 留 hub,经 ctx 注入给簇。
  - `createNewSession` 被 `send` 调用 → `createNewSession` 随 sessions 簇
    拆出,经 ctx 注入给 send 簇。
  - `toPayloadContent` 被 send / edit 簇共用 → 留 hub,经 ctx 注入。
  - `currentSession`/`isCurrentSessionStreaming`/`currentRequestId` 等
    computed → 经 ctx 注入(只读引用,无环)。

### 测试覆盖现状

- `chat.test.ts`(182 行):测模块级 export(`resolveModelInput`/
  `parseForcedDispatchPrefix`)——拆分后 import 路径不变,**零改动**。
- `chatMode.test.ts`(223 行):测 `requestSetMode`/`confirmYolo`/
  `cancelYolo`/`pendingYoloConfirm`——经 store proxy,**零改动**。
- `chatSend.test.ts`(232 行):测 `send`/`cancel`/`isCurrentSessionStreaming`/
  `sessions`——经 store proxy,**零改动**。

三个测试文件均经 store proxy 调用,return 块完整保留即可,**测试文件
本身不改**。

## Requirements

- R1 **拆 4 个 action 簇**(按下列顺序,每簇独立 commit + 独立回滚):
  1. **会话 CRUD** → `chatSessionActions.ts`(`createSessionActions(ctx)`,
     含 `onProjectChange` 外的 13 个会话/worktree action;~312 行)
  2. **mode + yolo + workflow** → `chatModeActions.ts`
     (`createModeActions(ctx)`,含 6 个 mode/yolo/workflow action +
     `pendingYoloConfirm`/`pendingResolveRequest` 两个 ref;~391 行)
  3. **edit + resend + retry** → `chatMessageActions.ts`
     (`createMessageActions(ctx)`,含 3 个 action;~383 行)
  4. **send + cancel** → `chatSendActions.ts`(`createSendActions(ctx)`,
     含 `send`/`cancel`;~198 行)
  - 每簇:新建文件 → 复制函数体(逐字) → 删源 → hub 顶部 `const X = createXActions(ctx)` → return 块用 `send: sendActions.send` 形式(或同名解构)保留原导出名。
  - R1.1 **ctx 类型**:每簇定义 `export interface XxxActionsContext { ... }`,
    字段 = 该簇用到的 state/computed/helper/跨簇 action。hub 在所有 const
    定义后一次构造 ctx 对象传入。
  - R1.2 **可见性**:簇内 helper(如 send 簇无,mode 簇无)随簇搬迁;
    跨簇共用 helper(`toPayloadContent`/`cancel`/`createNewSession`)留
    hub,经 ctx 注入——**不为拆分改任何可见性或 API 签名**。
- R2 **hub 收敛**:`chat.ts` 最终保留:模块级 export(L59-192)+ setup 头
  state/computed/watch(L194-527)+ `onProjectChange`+ diff 簇 +
  `toPayloadContent` + 4 个 `createXxx(ctx)` 调用 + return 块 → 目标
  **~700 行**(<1200 硬达标)。
- R3 **纯搬迁铁律**:逐 commit `git show <commit> --stat` 核对"新增簇文件
  行数 + hub 删除行数 ≈ 原 chat.ts 对应区段行数";`git diff -w` 逐函数
  对比函数体无语义改动(允许:删 trailing blank、import 重排、`function`
  前缀从缩进 2 空格→顶格)。**禁止顺手改逻辑、改命名、改注释措辞**。
- R4 **功能不变(硬约束)**:`pnpm test` 全绿(三个 chat 测试 + 全量);
  `pnpm typecheck`(若有)/`pnpm build` 无新错误。
- R5 **spec 同步**:拆分落地后更新 `.trellis/spec/frontend/state-management.md`
  (新增 §Chat Store Action Clusters,记录 4 簇 + ctx 契约,对标
  §Stream Controller Pattern);`.trellis/spec/frontend/directory-structure.md`
  补 stores/ 目录的簇文件登记(后端 directory-structure 已有"收官对照表"
  格式可参照)。

## Acceptance Criteria

- [ ] AC1:`chat.ts` <1200 行(目标 ~700);4 个簇文件各 <450 行;`pnpm test`
  全绿
- [ ] AC2:拆分是可量化的纯搬迁——逐 commit `git show --stat` +
  `git diff -w` 逐函数核对无语义改动
- [ ] AC3:三个 chat 测试文件(`chat.test.ts`/`chatMode.test.ts`/
  `chatSend.test.ts`)**零改动**(经 store proxy + 模块级 export 路径不变)
- [ ] AC4:全仓 `grep` 确认无组件/测试改从簇模块直接 import 内部符号
  (所有调用仍经 `useChatStore().X`)
- [ ] AC5:`state-management.md` + `directory-structure.md` 同步更新

## Out of Scope

- `streamEvents.ts`(1200 行,压线但 08-07 刚拆、边界干净,不动)
- `utils/chatInputCodeMirror.ts`(830 行,<1200,不拆)
- `chat.ts` 内 action 的**逻辑重构**(如 `send` 190 行单体的拆分留后续;
  本任务纯结构搬迁)
- `MessageItem.vue` / `ChatPanel.vue` 等组件拆分(总纲已列,后续批次)
- 后端任何文件(本任务纯前端)

## 风险与对策

| 风险 | 对策 |
|---|---|
| ctx 字段遗漏导致簇内 ReferenceError | 每簇搬迁后立即跑 `pnpm test chat` 局部验证,再进下一簇 |
| 循环 import(`chatSessionActions` ↔ `chatSendActions` 经 createNewSession) | createNewSession 随 sessions 簇拆出,send 簇经 ctx 拿——ctx 是运行时对象,无模块级环 |
| Pinia setup store 闭包语义破坏(ctx 传值 vs 传引用) | ctx 一律传 reactive ref/computed 的**引用**(不 `.value`),簇内读走 `ctx.x.value`——与 streamEvents 同构,已验证 |
