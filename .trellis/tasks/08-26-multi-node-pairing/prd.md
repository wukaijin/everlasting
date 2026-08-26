# PRD: PWA 多节点配对修复 + 配对/节点页互跳

## 背景(2026-08-26 用户报告)

"远程配对似乎有问题,只能配置一个节点吗?" —— 排查结论:**是**,且与文档承诺矛盾。

单节点链条(详见 `research/code-findings.md`):

1. redeem 每次签发全新 `device_token`,`devices` 表 `token(PK) → 单个 node_id`(`crates/everlasting-remote/src/db/schema.rs:27`);
2. 手机 localStorage 只存一个 token(`everlasting_device_token`),新配对直接覆盖旧值(`app/src/stores/pairing.ts:73`);
3. `GET /api/v1/nodes` 只返回该 token 绑定的那一个节点(`crates/everlasting-remote/src/routes/nodes.rs:49`,注释自述"当前单 token → 单 node")。

而 `docs/REMOTE-ACCESS-E2E.md` §5.4 / §8"多 PC"承诺"手机 /nodes 看到多张卡片",前端 nodes store(`selectNode`/`selectedNodeId`)、`NodeListView` 列表选择器、路由 guard 也都按多节点设计 —— 实现在 token 模型这层截断。

次生问题:`/nodes` 与 `/pairing` 之间没有任何互通入口 —— 已配对状态下没有去配第二台的入口;进了 `/pairing` 只能重新配对或登出才能回节点列表。

## 需求

- **R1 多节点**:手机同时保留多台 PC 的配对(每台各一个 token),`/nodes` 显示全部已配对卡片,在线/离线状态独立。
- **R2 节点切换**:点选某台在线 PC 进入 `/chat`,所有 app 命令代理到该 PC;切换到另一台时数据隔离(不残留前一台的会话/项目缓存)。
- **R3 token 部分失效**:某台 PC 的 token 被吊销(remote 侧删 devices 行)只影响该节点 —— 移除该 token 并回 `/nodes`;全部 token 失效才回 `/pairing`。
- **R4 页面互跳**:`/nodes` 提供"配对新设备"入口 → `/pairing`;`/pairing` 在已有配对时提供"前往已配对设备"入口 → `/nodes`。
- **R5 平滑迁移**:旧版单 token localStorage 数据升级后无需重新配对,`/nodes` 仍显示原节点。
- **R6 登出**:清除全部 token + 节点选择,回 `/pairing`。

## 非目标

- 服务端批量 `/nodes` 聚合端点(客户端按 token 逐个查,N 台 PC = N 个请求,量级可接受;留作后续优化)。
- 设备管理 UI(列出/单独吊销某台设备)。
- 服务端孤儿 device 行的清理(redeem 累积的旧 token 行不影响功能)。
- PC 端多 remote 服务器配置(一台 PC 仍只连一个 remote)。

## 验收标准

- **A1**:配对 PC-A → `/nodes` 显示 A 卡片;经 `/nodes` 页新入口再配对 PC-B → `/nodes` 显示 A+B 两张卡片,状态独立。(REMOTE-ACCESS-E2E §5.4 验收从"必然失败"变为通过)
- **A2**:点 B 卡片 → `/chat` 走 B 的 token(代理到 B);回 `/nodes` 点 A → 应用全量重载后走 A 的 token,无 B 的会话数据残留。
- **A3**:remote 侧吊销 B 的 token(`DELETE FROM devices ...`)后:下次请求 401 → B 的配对被移除、A 不受影响、落在 `/nodes`;最后一个 token 失效则落 `/pairing`。
- **A4**:旧版单 token 用户升级:`/nodes` 仍显示原节点,无需重新配对。
- **A5**:`/nodes` 有"配对新设备"入口;`/pairing` 在已有配对时显示"前往已配对设备"入口。
- **A6**:登出清除全部配对,回 `/pairing`。
- **A7**:`cd app && pnpm test` 全绿;`vue-tsc --noEmit` 无错误。

## 约束

- 服务端(`crates/everlasting-remote`)零改动 —— 修复全部落在 app 前端层。
- 遵循 `.trellis/spec/frontend/transport-and-pwa-modes.md` 既有约定(token 是 transport 路由信号、401 在 transport choke point 拦截、remote-native 端点直连 fetch 不进 CMD_TO_DOMAIN)。
