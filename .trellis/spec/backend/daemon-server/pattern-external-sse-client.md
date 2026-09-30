# Pattern: 外部进程消费 daemon SSE(客户端侧契约)

> 2026-09-30 沉淀(N20 ACP shim,task 09-30-n20-acp-shim-mvp)。daemon 的 SSE 服务端契约见本目录 `pattern-sse-resync-and-tests.md`;本文是**消费端**(外部瘦客户端进程:evl CLI[Node] / everlasting-acp shim[Rust] 双先例)必须遵守的模式。两个客户端各自独立实现踩过同一组坑,新客户端(或重写)照此清单走。

## 契约清单(按生命周期)

### 1. 启动即挂,全程保持(结构性,非优化)

- `GET /api/v1/stream` 是全局单流(非 per-session);**零订阅者时权限 ask 被快拒**(`daemon/sse.rs` unattended 路径)。
- 消费任务必须是进程级单例,daemon 连通后、任何 prompt 之前启动,永不主动断开。evl 已固化为 RULE-SMOKE-001;acp shim 同款。
- 在途 permission ask 无恢复面(`pending_interaction` 无 Permission 变体)——「常驻订阅」是当前唯一规避;daemon 侧恢复面是 follow-up。

### 2. 重连:Last-Event-ID + 退避

- 逐帧推进记录 SSE `id`(全局递增);断线重连把它作为 `Last-Event-Id` 请求头回传,daemon replay buffer(512 帧)回放 `id > last` 的帧——**事件跨重连至多投递一次,不丢**。
- sentinel `stream-resync`(id=0)不推进 last_event_id;`reason=buffer_overrun` 之外的 resync(如 restart)= **在途 turn 已随 daemon 死亡,终态永不再来**,消费方必须立即对在途请求 respond error,不能继续等(实证坑:PR2 曾只记日志继续等 → prompt 永久悬挂)。
- 退避:错误路径 1s→30s 指数;干净 EOF 短退避(防 hot-spin);重连失败不退进程(编辑器 session 长存)。

### 3. 健康判据:窗口化,不是连接态 flag

- daemon 用 30s `:ping` 保活;**「此刻在活连接内」的瞬时 flag 在空闲连接上占空比趋零不可用**(PR2 实证 bug:只在建连时刷新时间戳,空闲 >10s 后所有请求被误拒)。
- 正确形状:每个收到 chunk(含 ping)刷新时间戳,判据 = 最近 N 秒收到过数据,**N 必须大于 ping 间隔**(shim 取 45s,测试钉死该约束)。
- mid-turn 失联:健康窗判死 + 自愈等待窗(shim 30s)后对在途请求 respond error;约 75s 检测延迟期间 UI 显示进行中,可接受(cancel 落地后用户可主动打断)。

### 4. 事件过滤与 casing(实现坑)

- 全局流按 `request_id` 过滤 chat-event、按 `session_id` 过滤 tool/permission 事件(tool payload 双 id 都带)。
- **payload 命名不对称**:SSE `permission:ask` 事件 data 是 **camelCase**(`{rid, sessionId, toolUseId, ...}`),`chat-event`/tool 事件是 **snake_case**——两个 casing 都要测试锚死(shim 用独立 DTO + `#[serde(other)]` 未知 kind 兜底不炸)。

### 5. prompt 驱动顺序(缺一即坑)

health → SSE 挂 → `POST /api/v1/agent/chat`(受理非 `started` 即拒)→ 按 rid 过滤消费 → `done{stop_reason}` 即终态;`error` 事件是独立终态(daemon **不补 done**),必须走 respond 路径。

## 环境坑:本机回环被 http_proxy 劫持

> **Warning**: 开发机 shell 常设 `http_proxy=127.0.0.1:<port>`(clash 系);reqwest 的 NO_PROXY 匹配**不认 `127.*` 通配写法**,回环请求被送进代理 → 502 空响应,症状是「daemon 明明在跑却连不上」。
>
> 修法:连本机 daemon 的客户端一律构建时 `.no_proxy()`(daemon 是本机零鉴权服务,走代理本身就是错的)。evl(Node fetch 系)天然不受影响;任何新 Rust 客户端(reqwest)都会踩。

## 测试模式

- 静态消费循环(解析/过滤/状态机):wiremock 静态 body 即可(解析层按 `\n\n` 切帧,一次性下发无差)。
- 需要时序因果的(权限环「反向请求落点后再推 done」/ cancel「先受理再取消」):手写 axum 状态化假 daemon(broadcast → BroadcastStream → Sse 逐帧推 + 捕获请求体取动态 rid),wiremock 做不到。
