# Research: 单节点配对问题的代码级定位(2026-08-26 会话排查)

结论先行:**服务端天然支持多 PC 接入,截断点全在手机端"单 token 存储"+ `/nodes` 单 token 单节点返回**。修复不需要动 `crates/everlasting-remote`。

## 关键洞察:token 自解析节点

remote 的 `require_device_token` 中间件(`crates/everlasting-remote/src/auth.rs`)从 Authorization header 取 token → 查 `devices` 表 → 得到 `node_id`,proxy 全按它路由。**transport 层从来不需要知道 nodeId,只需要"当前该用哪个 token"**。nodeId→token 映射只服务于:① `/nodes` 聚合列表(按 token 逐查);② 用户选择节点后取对应 token;③ 401 时知道该删哪条。

## 单节点链条(file:line)

| 环节 | 位置 | 现状 |
|---|---|---|
| redeem 签发新 token | `crates/everlasting-remote/src/db/crud.rs:228` `redeem_pairing_code` | 每次 redeem INSERT 新 devices 行(token PK → 单 node_id),旧 token 不回收 |
| 手机只存一个 token | `app/src/stores/pairing.ts:73` `setDeviceToken(deviceToken)` | localStorage `everlasting_device_token` 单值,新配对覆盖 |
| token 存储 API | `app/src/transport/auth.ts` | 单 token get/set/clear/has,全 app 共用 |
| /nodes 只回一个节点 | `crates/everlasting-remote/src/routes/nodes.rs:49` `list_nodes` | 中间件解析 token → 查单个 node → `vec![NodeInfo]`。**返回数组形态本来就是为多节点留的** |
| transport 注入 token | `app/src/transport/http.ts:353` invoke / `:283` ensureEventSource | `getDeviceToken()` 单值 → Bearer header + `/api/v1/proxy` 前缀 + SSE `access_token` query |
| 401 处理 | `app/src/transport/http.ts:370` + `app/src/App.vue:25` | 清单 token → 无条件 `router.push("/pairing")` |
| 节点选择 | `app/src/stores/nodes.ts:78` `selectNode` | 只写 localStorage `everlasting_selected_node`,不重置任何状态(单节点时代无需) |
| 登出 | `app/src/stores/nodes.ts:87` | 清单 token + 选择 |

## redeem 响应已含 nodeId(重要)

`POST /api/v1/pairing/redeem` 响应(`pairing.rs:63` `RedeemedResponse`,`rename_all="camelCase"`):
`{ deviceToken, nodeId, nodeDisplayName }` —— **手机端 redeem 时就能拿到 nodeId**,存 map `nodeId → token` 不需要额外查询。

## /nodes wire 形状(按 token 逐查的合并依据)

`GET /api/v1/nodes`(带 `Authorization: Bearer <token>`)→ `[{ nodeId, displayName, status: "online"|"offline", lastSeenAt }]`(恒为单元素数组,camelCase,`nodes.rs:27`)。客户端对每个 token 各调一次、按 nodeId 合并即得完整列表。401 = 该 token 已被吊销(devices 行被删)→ 客户端应把该 token 从存储中移除。

## 前端已为多节点准备的部分(不用动)

- `NodeListView.vue`:卡片列表渲染、在线/离线态、点击选择 —— 天然支持多卡片。
- `nodes.ts` store:`nodes: NodeInfo[]` + `selectNode`/`selectedNodeId`(localStorage 持久化)。
- `router/index.ts` guard:`/chat` 需要 `selectedNodeId`;`/` redirect 按是否有 token 分流 —— 逻辑对多节点天然成立,只是 `hasDeviceToken()` 的语义从"单 token 存在"变为"任一配对存在"。

## localStorage 键盘点(迁移相关)

| 键 | 写入方 | 说明 |
|---|---|---|
| `everlasting_device_token` | auth.ts(旧,单值) | 迁移源;升级后不再写 |
| `everlasting_selected_node` | nodes.ts | 选中节点 id,保留 |
| `everlasting_node_tokens` | 本任务新增 | JSON `Record<nodeId, token>` |

迁移路径:旧单 token 无法本地反解 nodeId(token 是 64-hex 随机),但 selected_node 键或一次 `GET /api/v1/nodes`(响应含 nodeId)都能补上 —— 在 `loadNodes` 里做惰性迁移最自然(它本来就要拿 token 查 /nodes)。

## 节点切换 = 换了一台机器的数据

各 PC 独立 SQLite、独立会话列表。pinia 全局 store 缓存的是"当前节点"的数据,切换节点若只 `router.push`,stores 不重置 → 串数据。SPA 没有全局 reset 机制;`window.location.assign("/chat")` 全量重启最稳(深链刷新本来就工作:ServeDir SPA fallback 既有,用户刷新 /chat 是既有场景)。SSE EventSource 也绑 token(`ensureEventSource`),重载一并重建。

## 参考文档

- `docs/REMOTE-ACCESS-E2E.md` §5.4(第二台 PC 验收)、§8 多 PC / 撤销设备 —— 行为承诺与本任务验收对齐。
- `.trellis/spec/frontend/transport-and-pwa-modes.md` —— 两信号模型(hasDeviceToken / isRemoteContext)、401 choke point、wire casing 约定,本任务延续。
