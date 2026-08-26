# Implement: 执行清单

前置阅读:`prd.md` → `design.md` → `research/code-findings.md` → `.trellis/spec/frontend/transport-and-pwa-modes.md`。

依赖顺序:auth.ts 是根,先做;transport/store 依赖它;视图最后。

## Step 1 — auth.ts 多 token 模型

- [ ] 重写 `app/src/transport/auth.ts`:`everlasting_node_tokens` map + legacy 只读兜底,导出 design §1 的 8 个函数(删旧 get/set/clearDeviceToken,`hasDeviceToken` → `hasPairedNode`)
- [ ] 新增 `app/src/transport/auth.test.ts`(用例见 design §测试)
- 验证:`pnpm test -- transport/auth`(此时 http.ts 尚未切新 API,应仍编译 —— 若 auth.ts 删了旧导出导致 http.ts/pairing.ts/nodes.ts/router 编译错,Step 2-4 一并完成后再跑)

> 注意:auth.ts 旧导出被 http.ts / pairing.ts / nodes.ts / router 引用。为避免中间态编译错,Step 1-5 作为一个原子批次实施,跑测放在 Step 5 后。

## Step 2 — http.ts transport

- [ ] `invoke` + `ensureEventSource` 换 `currentDeviceToken()`
- [ ] 401 分支换 `dropCurrentNodeToken()`
- [ ] 回归/补 `transport/http.test.ts` token 注入断言

## Step 3 — stores

- [ ] `stores/pairing.ts` redeem → `setNodeToken(nodeId, deviceToken)`(响应解构已含 nodeId)
- [ ] `stores/nodes.ts` loadNodes:legacy 迁移 → 逐 token 查询 → 合并去重 → 401 修剪;logout → `clearAllNodeTokens()`
- [ ] 新增 `stores/nodes.test.ts`、`stores/pairing.test.ts`(mock fetch)

## Step 4 — 路由与视图

- [ ] `App.vue` 401 回调:`hasPairedNode() ? "/nodes" : "/pairing"`
- [ ] `router/index.ts`:`hasDeviceToken` → `hasPairedNode`(guard 逻辑不动)
- [ ] `NodeListView.vue`:① 点在线卡片 → `selectNode` + `window.location.assign("/chat")`;② 列表/空态下方"＋ 配对新设备"按钮 → `/pairing`;空态文案去掉"重新配对"改为引导新按钮
- [ ] `PairingView.vue`:`hasPairedNode()` 时表单下方"已配对设备?前往选择 →" → `/nodes`
- [ ] 视图轻量挂载测试(两个按钮的渲染与导航;按 `SubagentDrawer.test.ts` 模式)

## Step 5 — 全量验证

```bash
cd app && pnpm test                      # vitest run 全量
cd app && npx vue-tsc --noEmit           # typecheck(build 脚本同款)
```

- [ ] 全绿后进入 Step 6

## Step 6 — 文档

- [ ] `docs/REMOTE-ACCESS-E2E.md`:§5.4 标注已支持(两卡片验收生效);§8"多 PC"补"每台各配一次即累积;切换=点卡片(整页重载);吊销单台不影响其余"
- [ ] `.trellis/spec/frontend/transport-and-pwa-modes.md`:Signal 1 从单 token 更新为多 token map(`hasPairedNode` / `currentDeviceToken`)—— Phase 3.3 spec 更新时做

## 回滚点

- Step 1-5 原子批次失败 → `git checkout -- app/src` 回到干净态。
- Step 6 文档独立,可单独 revert。
