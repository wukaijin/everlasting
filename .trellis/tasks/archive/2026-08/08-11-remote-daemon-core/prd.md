# S1 remote daemon core:独立 crate + WSS 服务端 + devices 表 + 反向代理骨架

> 架构决策见 [parent PRD](../08-11-remote-control-epic/prd.md)。本任务只细化执行。

## Goal

建独立 Rust crate `everlasting-remote`,跑在云服务器(国内 2C2G,ubuntu 原生二进制)。承担:WSS 服务端(收 PC daemon outbound 连接)+ devices 表(token ↔ node 绑定)+ 配对码生命周期 + 反向代理骨架(token → node WSS 转发)+ 节点状态 API + shared_secret 校验。

**本任务只做"骨架":WSS 连接管理 + devices 表 CRUD + 配对码生成/校验 + HTTP 反向代理的请求转发骨架(不含完整帧协议联调,那是 S3)。** S1 完成时:PC daemon 能连上、能注册、能生成配对码、手机能用 token 触发一次"hello world"级的转发(非流式),SSE 桥接留 S3。

## Scope

### 新建 crate `everlasting-remote`

位置:`app/src-tauri/` 同级或 `crates/everlasting-remote/`(与 daemon 同 workspace)。**零系统库依赖** —— 只依赖 axum / tower-http / tokio-tungstenite / sqlx / serde,确保 ubuntu `cargo build --release` 产出可执行二进制。

二进制 target:`everlasting-remote`,CLI:`--port <N>`(默认 7457)+ `--db-path <P>`(默认 `~/.local/share/dev.everlasting.remote/remote.db`)+ `--shared-secret <S>`(或环境变量 `EVERLASTING_REMOTE_SECRET`)。

### WSS 服务端

- `GET /ws`(upgrade to WebSocket):PC daemon 连接入口
- 连接握手时校验 `shared_secret`(header 或 query param),失败拒绝
- 连接成功 → 生成/识别 `node_id`(PC daemon 第一次连用 MAC/hostname 派生稳定 id)→ 写入 `node_connections: Arc<DashMap<node_id, ConnHandle>>`
- 心跳:30s ping,90s 无 pong → 判离线 → 移除连接 + 标记 `nodes.status=offline`

### 数据模型(remote 自己的 SQLite)

```sql
-- 节点(PC daemon)注册表
nodes(id TEXT PK, display_name TEXT, status TEXT, last_seen_at INTEGER, created_at INTEGER)
-- 设备(手机/浏览器)token 表
devices(token TEXT PK, node_id TEXT REFERENCES nodes(id), display_name TEXT,
        last_seen_at INTEGER, created_at INTEGER, revoked INTEGER DEFAULT 0)
-- 配对码(短生命周期)
pairing_codes(code TEXT PK, node_id TEXT REFERENCES nodes(id), expires_at INTEGER, used INTEGER DEFAULT 0)
```

不做 user 表(单用户模型,Q11)。

### 配对码生命周期

- `POST /api/v1/internal/pairing/generate`(PC daemon 经 WSS 调):生成 6 位随机码,写 `pairing_codes`(60s 过期),返回码
- `POST /api/v1/pairing/redeem`(手机 HTTP 调,body `{code, device_name}`):校验码(未过期 + 未用)→ 签发 `device_token`(随机 32 字节 hex)→ 写 devices 表 → 标记码 used → 返回 token + node_id

### 节点状态 API

- `GET /api/v1/nodes`(手机 HTTP 调,带 device_token):返回该 token 绑定的 node 信息 + 在线状态
- `GET /api/v1/health`:无认证,nginx 健康检查用

### 反向代理骨架

- 手机 HTTP 请求带 `Authorization: Bearer <device_token>` → 中间件查 devices 表 → 拿到 `node_id` → 查 `node_connections` 找到对应 WSS conn
- **本任务只实现非流式转发骨架**:把 HTTP 请求(method/path/headers/body)序列化成 `Request` 帧塞 WSS → 等 `Response` 帧(oneshot + request_id 映射)→ 返回手机。**Stream 帧 / SSE 桥接留 S3。**
- node 离线时返 502 + `{"error":"node_offline"}`

### shared_secret 校验

- 环境变量 / CLI 传入,启动时加载
- WSS 握手时校验(PC daemon 连接必带)
- 手机 HTTP 走 device_token,不走 shared_secret(分离)

## 依赖

- 无前置(可立即启动)
- 与 S2 并行(S2 做 PC 侧,本任务做 remote 侧)

## 验收标准

- [ ] `cargo build --release -p everlasting-remote` 产出 ubuntu 可执行二进制
- [ ] `./everlasting-remote --port 7457 --shared-secret test123` 启动,`GET /api/v1/health` 返 200
- [ ] PC daemon(用 S2 的 client,或临时 wscat 模拟)带正确 secret 连 WSS → 注册成 node → `nodes` 表有记录 + `GET /api/v1/nodes` 返在线
- [ ] 错误 secret 的连接被拒(401)
- [ ] 配对码生成(S2 调或 curl 模拟)→ 60s 内 redeem 成功 → 拿到 device_token;过期/重复用失败
- [ ] 用 device_token 发一个 `GET /api/v1/sessions/list`(透传到 PC daemon)→ 拿到响应(非流式转发骨架打通)
- [ ] node 离线(模拟断开 WSS)→ 同请求返 502 `node_offline`
- [ ] 心跳:30s ping / 90s 超时判离线

## Notes

- 帧序列化格式先用 JSON(S3 可换 bincode)。`Request`/`Response` struct 定义放本 crate(被 S2 PC daemon 侧引用,或抽到共享 crate)。
- **帧协议共享 crate 决策**:若 S2 需要,把帧类型抽到 `everlasting-remote-protocol`(纯类型 crate,零依赖),S1 和 S2 都 depend。实施时定。
- SSE 桥接(Stream 帧)明确留 S3,本任务 non-streaming 骨架够。
