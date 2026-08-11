# S3 e2e 隧道管线:Request/Response/Stream 帧协议 + SSE 桥接 + 心跳重连 + 双端联通

> 架构决策见 [parent PRD](../08-11-remote-control-epic/prd.md)。本任务只细化执行。

## Goal

S1(remote 侧骨架)+ S2(PC 侧骨架)各自跑通非流式后,本任务做**完整帧协议联调 + SSE 桥接 + 取消传播 + 心跳/重连的端到端验证**。打通后:手机经 remote 能看到 agent 的实时流式输出,断开能取消,断线能恢复。

**这是 epic 的技术核心难点**。前两个任务的"骨架"在这里被压测和补全。

## Scope

### 帧协议定稿(抽 `everlasting-remote-protocol` crate)

S1/S2 实施时若各自本地定义了帧类型,本任务统一抽到共享 crate(纯类型,零依赖,S1/S2 都 depend):

```rust
#[derive(Serialize, Deserialize)]
pub enum Frame {
    /// remote → PC:一次 HTTP 请求
    Request { id: u64, method: String, path: String, headers: HashMap<String, String>, body: Bytes },
    /// PC → remote:非流式响应
    Response { id: u64, status: u16, headers: HashMap<String, String>, body: Bytes },
    /// 双向:流式响应(SSE chunked)
    Stream { id: u64, event: StreamEvent },
}

pub enum StreamEvent {
    Chunk(Bytes),    // SSE 的一段
    End,
    Error(String),
}
```

序列化:JSON(MVP),bincode(优化期)。

### remote 侧(S1 补全)

- **request_id 映射表**:`DashMap<request_id, oneshot::Sender<Frame>>`,手机请求发出时插入,收到 PC 的 Response/Stream 时按 id 取出 send
- **SSE 桥接(remote → 手机)**:PC 发来的 `Stream::Chunk` 帧 → 转成 SSE chunk 写到手机的 HTTP response body → 持续直到 `Stream::End`
- **手机 SSE 连接的生命周期**:手机 `GET /api/v1/stream/:session_id`(带 token)→ remote 查 token → 找 node WSS → 发 `Request` 帧 → PC 把 SSE chunked 转成 `Stream::Chunk` 持续推 → remote 转发给手机的 SSE 连接
- **超时清理**:request_id 映射项加超时(如 5min 无响应)→ 清理 + 手机端收到 504

### PC 侧(S2 补全)

- **SSE 检测**:dispatcher 发出 reqwest 请求后,检测响应 `Content-Type: text/event-stream` → 切换到流式处理路径
- **流式转发**:SSE response body 用 `BytesStream` 逐 chunk 读 → 每段包成 `Stream::Chunk` 塞回 WSS → SSE 结束发 `Stream::End`
- **取消传播**:remote 侧手机 SSE 连接断开 → remote 发 `Stream::End`(或直接 drop oneshot)→ PC 侧 WSS 接收端关闭 → dispatcher 的 `BytesStream` 读返回 → reqwest 连接 drop → PC daemon 的 axum 检测到客户端断开 → 触发 agent loop 的 CancellationToken(复用现有机制)

### 心跳 / 重连 双端对齐

- **remote 侧**:30s 发 WebSocket ping,90s 无 pong → 标记 node 离线 + 清理该 node 的所有 in-flight request_id(对应手机请求返 502 `node_offline`)
- **PC 侧**:30s 等 ping,超时主动重连;重连后所有 in-flight 请求(本地 dispatcher 还在等 reqwest)的处理 —— **MVP 简化:重连后旧请求的 Response 帧丢弃(remote 那边已清理),手机侧已返 502 前端重试**
- 不做 in-flight 请求迁移(Q11 推后项)

### shared_secret 完整校验链

- PC daemon WSS 连接握手时带 secret → remote 校验 → 失败 401 关闭
- 错误 secret 的 fake daemon:连上即被踢

### 端到端测试 harness

- 集成测试:启 remote(测试端口)+ 起 PC daemon(连测试 remote)+ 模拟手机 HTTP client → 验证完整链路
- 关键场景:
  1. 非流式请求(GET sessions/list)→ 200 + body
  2. SSE 流式(POST chat → stream)→ 实时收到 chunk 序列
  3. 手机断开 SSE → PC agent loop cancel
  4. PC daemon 断线 → 手机请求返 502 → PC 重连后恢复
  5. shared_secret 错误 → 连接被拒

## 依赖

- **S1 + S2 必须先完成**(双端骨架就绪才能联调)

## 验收标准

- [ ] 手机 `POST /api/v1/chat`(经 remote)→ 实时收到 agent 的 SSE chat-event 流(thinking/text/tool_use/tool_result 完整序列)
- [ ] 手机 `GET /api/v1/stream/:session_id` → 持续收到该 session 的事件
- [ ] 手机触发 permission:ask → 卡片弹出 → 手机点允许 → agent 继续(完整 round-trip)
- [ ] 手机断开 SSE 连接 → PC agent loop CancellationToken 触发 → agent 停止(cancel 传播通)
- [ ] PC daemon 模拟断线(kill WSS)→ 手机 in-flight 请求返 502 `node_offline`;PC 重连后新请求恢复
- [ ] shared_secret 错误的 fake daemon 连不上,remote 日志记录拒绝
- [ ] 心跳:PC 90s 不回 pong → remote 标记离线 + `GET /api/v1/nodes` 反映离线
- [ ] 端到端集成测试 harness 5 个场景全过

## Notes

- **SSE chunked 转 Stream 帧的边界**:SSE 是 `event: x\ndata: y\n\n` 文本格式,按 chunk 原样转 `Stream::Chunk` 即可(remote 侧透传到手机 SSE body,手机 EventSource 解析)。不要在隧道层解析 SSE 语义。
- **取消传播是难点**:手机断开 → remote drop oneshot → PC WSS 接收端关闭 → reqwest 连接 drop → axum 检测客户端断开 → agent cancel。每一环都要验证,任一环漏了就 cancel 不了。
- 帧类型抽 crate 的决策:倾向 `everlasting-remote-protocol`(三方共用),S1/S2 实施时就按这个方向写,本任务正式落地。
