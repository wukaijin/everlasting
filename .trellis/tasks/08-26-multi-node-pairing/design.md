# Design: 客户端多 token 模型(服务端零改动)

## 方案总览

**服务端零改动**。核心依据(research §关键洞察):remote 由 token 自解析目标节点(auth 中间件 token → node_id),transport 只需要"当前该用哪个 token"。修复 = 手机端把单 token 存储换成 `nodeId → token` map,`/nodes` 列表按 token 逐查合并。

```text
现状:  redeem → setDeviceToken(t2) 覆盖 t1 → /nodes 只见 node2
目标:  redeem → setNodeToken(node2, t2) 累积 {node1: t1, node2: t2}
        /nodes → 对每个 token GET /nodes → 合并 → 两张卡片
        invoke/SSE → currentDeviceToken() = 选中节点的 token
```

## 1. auth.ts —— 多 token 存储(唯一的"模型"改动)

localStorage:

- `everlasting_node_tokens`(新):JSON `Record<nodeId, token>`
- `everlasting_device_token`(旧,**只读迁移源**,不再写)
- `everlasting_selected_node`(已有,不动)

新 API(替换原 get/set/clearDeviceToken,保留 `hasDeviceToken` 语义但更名):

```ts
getNodeTokens(): Record<string, string>      // 读 map(损坏 JSON → {})
getTokenForNode(nodeId): string | null
setNodeToken(nodeId, token): void            // 同时删除旧单值 key(迁移完成)
removeNodeToken(nodeId): void
clearAllNodeTokens(): void                   // 登出用
hasPairedNode(): boolean                     // map 非空 或 legacy key 存在(router guard / PairingView 用)
currentDeviceToken(): string | null          // transport 用,同步、无网络:
    // 1. selected_node 的 map 条目 → 它
    // 2. map 仅一条 → 它(redeem 后未选/首配)
    // 3. legacy 单值存在 → 它(迁移前兜底)
    // 4. 否则 null
dropCurrentNodeToken(): void                 // 401 用:删 selected 的 map 条目;
    // map 无 selected 条目但有 legacy → 删 legacy(等于全清)
```

localStorage 访问沿用现有 try/catch 模式(私密模式)。

**迁移**(R5):`nodes.loadNodes()` 开头 —— 若 map 空且 legacy 存在:用 legacy token GET /nodes → 响应 `nodeId` → `setNodeToken(nodeId, legacy)`(内部删 legacy key)。迁移完成前 transport 走 `currentDeviceToken()` 的 legacy 兜底,不阻塞任何请求。

## 2. http.ts —— transport 切换 token 来源

- `invoke`(:353)与 `ensureEventSource`(:283):`getDeviceToken()` → `currentDeviceToken()`。其余(URL 前缀、Bearer、SSE `access_token` query)不变。
- 401 分支(:370):`clearDeviceToken()` → `dropCurrentNodeToken()`。仍是 transport choke point 拦截(spec 约定不变)。

## 3. App.vue —— 401 路由回调

```ts
setOnAuthFailed(() => {
  void router.push(hasPairedNode() ? "/nodes" : "/pairing");
});
```

(R3:部分失效回 /nodes 挑剩下的;全失效回 /pairing。)

## 4. stores

- **pairing.ts `redeem`**:响应已有 `nodeId` → `setNodeToken(nodeId, deviceToken)`;`resetEventSource()` 照旧。
- **nodes.ts `loadNodes`**:
  1. 惰性迁移 legacy(见 §1);
  2. 对 map 每个 `{nodeId, token}` GET /nodes(带各自 Bearer);
  3. 合并去重(按 nodeId;同 nodeId 以新结果覆盖);
  4. **401 的条目从 map 删除**(吊销自愈),其余节点照常显示;全部 401 → 空列表(用户走"配对新设备");
  5. 网络层失败(fetch throw / 5xx)→ 抛错,走现有 `loadError` + 重试路径。
- **nodes.ts `selectNode`**:保持只记录选择(切换重载由视图层负责,见 §5)。
- **nodes.ts `logout`**:`clearAllNodeTokens()` + 清选择 + 清列表(其余现状)。

## 5. 节点切换的状态重置 —— `location.assign` 全量重启

跨节点切换必须重置全部 pinia store + SSE(各 PC 独立 SQLite,`research` §节点切换)。方案:`NodeListView` 点在线卡片 → `selectNode(node)` + `window.location.assign("/chat")`。

理由:pinia 无全局 reset;逐 store `$reset` 清单会漏;深链 `/chat` 刷新是既有工作场景(SPA fallback 已部署);PWA 重载开销一次几百毫秒,统一"选卡即重启"行为最不易出边界情况。首次配对后的第一次选择同路径,统一。

## 6. 视图互跳(R4)

- **NodeListView**:卡片列表下方(空态文案下方同样)加"＋ 配对新设备"按钮 → `router.push("/pairing")`。用全局 `.btn .btn--outline` 家族;标题栏保持"登出"不动。
- **PairingView**:`hasPairedNode()` 为真时,表单下方渲染"已配对设备?前往选择 →"文字链 → `router.push("/nodes")`。

## 7. 路由

`router/index.ts` guard 逻辑不变;`hasDeviceToken` 引用改为 `hasPairedNode`(纯更名,语义"任一配对存在")。`/` redirect 与 `/chat` 门槛对多节点天然成立。

## 8. 服务端(零改动重申)

`GET /api/v1/nodes` 按 token 返回绑定节点 —— 客户端循环调用即得全列表。N 台 PC = N 请求,量级(个人 2-3 台)可接受;批量聚合端点列非目标。

## 测试设计(vitest)

- `transport/auth.test.ts`(新):map 读写 / `currentDeviceToken` 优先级(selected > 单条 > legacy)/ `dropCurrentNodeToken` 三形态 / `setNodeToken` 清 legacy / 损坏 JSON 容错。
- `stores/nodes.test.ts`(新):loadNodes 多 token 合并、401 修剪、legacy 迁移、全 401 空列表(mock fetch;沿用现有 store 测试的 pinia 初始化模式)。
- `stores/pairing.test.ts`(新):redeem 落 map(mock fetch)。
- `transport/http.test.ts`:回归 —— token 注入路径换 `currentDeviceToken` 后既有断言仍绿;如无单 token 断言则补最小覆盖。
- 视图按钮:按现有组件测试模式(参考 `SubagentDrawer.test.ts`)加轻量挂载测试(互跳按钮渲染 + 点击导航)。

## 风险与回滚

- 行为变化点集中在 auth.ts 一个模块;回滚 = revert 单 commit。
- localStorage map 损坏 → 当作空(回到未配对态),与旧版"token 丢失重新配对"的失败模式一致,无新增风险。
- `location.assign` 在 dev vite 下同样工作(history fallback);唯一可感知差异是选卡时整页刷新一次。
