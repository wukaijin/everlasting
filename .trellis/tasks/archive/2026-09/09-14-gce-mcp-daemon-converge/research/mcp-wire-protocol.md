# MCP wire 协议反向提取(@modelcontextprotocol/sdk 1.30.0)

> 方法论:不逆向宿主二进制,以 SDK 为契约源反推。依据:宿主 config.json 的
> `mcp.servers` 支持 `{"type":"http","url":...}` 挂载(siyuan-sisyphus 先例),
> ZCode 宿主的 MCP 客户端与我们的 stdio 壳同族(皆 @modelcontextprotocol/sdk),
> 客户端 wire 行为以 SDK 源码为准。服务端(daemon /mcp)按此契约反向实现。
>
> 源码基线:`scripts/node_modules/@modelcontextprotocol/sdk@1.30.0`
> (dist/esm,行号可点)。宿主实际 SDK 版本未知,但版本协商机制(下文)
> 天然吸收版本差——客户端请求的版本它自己必然支持。

## 1. 客户端行为(StreamableHTTPClientTransport)

源:`dist/esm/client/streamableHttp.js`

### 1.1 连接生命周期

| 阶段 | 行为 | 源码 |
|---|---|---|
| start() | 仅建 AbortController,不发任何请求 | :257-262 |
| initialize | `Client.connect()` 发 `initialize` 请求(POST),成功后发 `notifications/initialized` 通知 | client/index.js:274-311 |
| 通知收到 202 后 | 若是 initialized 通知,自动 GET 打开 SSE 流;**服务端 405 = 预期内,静默跳过** | streamableHttp.js:371-379, 101-105 |
| 断线重连 | 仅对 GET 流 / 未收到响应的 POST 流;指数退避,默认 maxRetries=2 | :7-12, 139-158 |
| terminateSession() | DELETE + session 头;405 亦接受 | :433-458 |

### 1.2 POST 请求形态(send,streamableHttp.js:289-418)

请求头:

```
content-type: application/json
accept: application/json, text/event-stream     ← 必须两者都列
mcp-session-id: <id>                            ← 仅当服务端曾在响应头给过
mcp-protocol-version: <version>                 ← 仅 initialize 协商后(setProtocolVersion)
```

body:单条 JSON-RPC 消息(或数组)。响应分派:

| 响应 | 客户端行为 |
|---|---|
| 202 Accepted | 通知已受理,body 丢弃;initialized 通知触发 GET SSE |
| 200 + `application/json` | **直接解析为 JSON-RPC 响应(单条或数组)——纯 JSON 服务端是一等公民** :394-403 |
| 200 + `text/event-stream` | SSE 流内 `event: message`(或无 event)帧携带响应 :388-393 |
| 401/403 | OAuth 流程(我们零鉴权,不适用) |
| 其他非 2xx | 抛 StreamableHTTPError |

**关键放宽点:客户端不要求服务端开 GET SSE 流(405 预期)、不要求 session id
(服务端从不返回该响应头则客户端永不发送)、不要求 SSE 响应(纯 JSON 即可)。**

## 2. 握手与版本协商(client/index.js:274-311)

initialize 请求 params:`{protocolVersion: LATEST, capabilities, clientInfo}`。
SDK 1.30.0 的常量(types.js:2-4):

```js
LATEST_PROTOCOL_VERSION = '2025-11-25'
DEFAULT_NEGOTIATED_PROTOCOL_VERSION = '2025-03-26'
SUPPORTED_PROTOCOL_VERSIONS = ['2025-11-25','2025-06-18','2025-03-26','2024-11-05','2024-10-07']
```

客户端对 initialize **结果**的两条硬校验(失败即断连):

1. `result.protocolVersion` 必须 ∈ 客户端 SUPPORTED 列表(:293-295);
2. 后续 `tools/list` 要求 `result.capabilities.tools` 已声明,否则抛
   "Server does not support tools"(:270-273)。

→ 服务端策略(决策 D5):请求版本 ∈ 我方支持集(同上五值)→ **echo 回请求版本**
(客户端请求的版本它自己必支持,跨宿主 SDK 版本最稳);不在集内 → 回
`2025-03-26`(SDK 缺省协商值)。协商后客户端每个请求都带
`mcp-protocol-version` 头,值 = 我们 echo 的版本。

## 3. 服务端校验规则(WebStandardStreamableHTTPServerTransport)

源:`dist/esm/server/webStandardStreamableHttp.js`(948 行;server/streamableHttp.js
只是 Node 兼容薄壳)

| 检查 | 失败响应 | 源码 |
|---|---|---|
| POST Accept 须同时含 `application/json` 与 `text/event-stream` | 406 `-32000` | :463-470 |
| POST Content-Type 须 application/json | 415 `-32000` | :471-476 |
| body 非法 JSON / 非 JSON-RPC 形 | 400 `-32700` | :493-508 |
| 纯通知(无 id 的消息) | **202 空 body** | :561-566 |
| PUT/PATCH 等 | 405 | :439-451 |
| GET Accept 无 text/event-stream | 406 | :220-225 |
| `mcp-protocol-version` 头:缺失→接受(用协商值);未知→400 | 400 `-32000` | :755-770 |

**无状态模式**(`sessionIdGenerator: undefined`):跳过全部 session 校验
(validateSession 直接返回 undefined,:716-719),可重复 initialize,
无 404 `-32001` 风险;`enableJsonResponse: true` 时请求一律回纯 JSON
(:551-567)。SDK 自己就以无状态 + 纯 JSON 为合法组合——正是我们的目标形态。

## 4. 极简兼容服务端 profile(daemon /mcp 的实现契约)

- `POST /mcp`:校验 Accept/Content-Type → 解析 JSON-RPC(单条或数组)→ 分派:
  - `initialize` → `{protocolVersion: echo(D5), capabilities: {tools: {}}, serverInfo: {name: "everlasting-group-chat", version: ...}}`,**不设 mcp-session-id 响应头**
  - `notifications/*`(含 initialized)→ 202
  - `ping` → result `{}`
  - `tools/list` → `{tools: [8 个,含 name/description/inputSchema]}`(无分页,8 个一页)
  - `tools/call` → `{content: [{type:"text", text: JSON.stringify(result, null, 2)}], isError?}`(工具级错误 = 200 + isError:true,不用 JSON-RPC error——JS 壳同款 errorResult)
  - 未知方法 → 200 + JSON-RPC error `-32601`
- `GET /mcp` → **405**(无服务端主动流;客户端预期内)
- `DELETE /mcp` → 200 no-op(无 session 可终结)
- `mcp-protocol-version` 头:**lenient,不校验只记日志**(D6;严格校验反而在
  宿主升级到更新协议版本时自我打断)
- 批处理:接受数组,逐条处理;含请求的数组回数组(纯 JSON 模式天然支持,
  客户端 :397-399 解析数组)。2025-06-18+ 协议已移除 batching,支持仅为防御。

## 5. wire JSON shape(types.js 精确提取)

```jsonc
// initialize 结果(必填三键,InitializeResultSchema :539-552)
{ "protocolVersion": "2025-06-18", "capabilities": { "tools": {} },
  "serverInfo": { "name": "...", "version": "..." } }   // Implementation{name,version,title?}

// tools/list 结果(:1283-1285,ToolSchema :1229+)
{ "tools": [ { "name": "...", "description": "...",
    "inputSchema": { "type": "object", "properties": {...}, "required": [...] } } ] }
// inputSchema 根必须 type:"object"(JSON Schema 2020-12);additionalKeys 透传

// tools/call 结果(CallToolResultSchema :1289-1305)
{ "content": [ { "type": "text", "text": "..." } ],   // 无 outputSchema 时必填,可空数组
  "structuredContent": {...},                          // 可选,我们不用
  "isError": false }                                   // 缺省 false
```

JSON-RPC 骨架:`{jsonrpc:"2.0", id, method, params}` / `{jsonrpc:"2.0", id, result}`
/ `{jsonrpc:"2.0", id, error:{code, message, data?}}`;通知 = 无 id。
**id 必须原样回显(数字/字符串皆可能)**——serde_json::Value 透传即可。

## 6. 宿主兼容性证据与残余风险

- 证据:config schema 有 `type:"http"` + `url` + `headers`(siyuan 条目实证);
  本会话挂载的 stdio 工具名前缀 `mcp__everlasting-group-chat__*`,换 HTTP 挂载
  保留同名 server 条目即可保前缀稳定。
- 残余风险:宿主对 http 型 server 的连接管理(会话内懒连/重连)未实测;siyuan
  条目 `enabled:false` 原因不明(无日志佐证)。→ 迁移阶段 2 以**双挂载并行**
  (新名 `-http` 后缀)实测收口,不盲切。
- 兜底:echo 版本策略 + lenient 头校验 + 纯 JSON 响应 = 协议面取交集的最保守
  形态;任何版本的 SDK 客户端(2024-10-07 起)都能消费。
