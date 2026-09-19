<!-- Moved from daemon-server.md 2026-09-19 (doc-split) -->


## Scenario: tunnel node_id 派生与自定义(同 hostname 撞车,2026-08-26)

### 1. Scope / Trigger

改 node_id 派生、`set_tunnel_node_id` IPC 或 remote 节点注册逻辑时读本节。
背景 gotcha:两台机器 hostname 相同(如都叫 `carlos`)→ node_id 相同 →
remote `tunnel_registry` 同 key 互踢(新连接踢旧是设计行为,但两台活机器
+ 退避连接成功即重置回 1s)→ 永久互踢循环 + 手机流量随机路由到其中一台,
配对表现"时好时坏"。症状:remote 日志刷 `duplicate node_id, kicking
previous tunnel`。

### 2. Signatures

- `daemon/tunnel/node_id.rs::derive_node_id(pool: &SqlitePool) -> String`
- IPC `set_tunnel_node_id(node_id: Option<String>)`(四处齐全规则见上节;
  请求体 snake_case `node_id` + `#[serde(default)]`)

### 3. Contracts

- 派生三级优先:① `app_config["tunnel_node_id"]` trim 非空 → 直接用
  (统一覆盖用户自定义与历史 fallback UUID);② hostname 净化
  (`sanitize`:只留 `[a-z0-9-]`,非法字符折叠单连字符);③ 随机
  `node-<uuid>` 持久化写同一 key。
- 三态(照 `set_web_search_config` 的 `tavily_api_key` 先例):
  `Some(非空)` = 校验 + 落库 + 从 DB 重建 `TunnelConfig` 经
  `tunnel_manager.set_config` 触发重连(supervisor 既有 `cfg != current`
  路径);`Some("")` = 删 key 回自动派生;`None` = 不动。
- `get_remote_config` payload 含 `nodeId: Option<String>`(自定义值原文,
  null = 自动);payload 本身"未配置 remote 时为 null"语义不变。

### 4. Validation & Error Matrix

- `sanitize(trim(x)) != trim(x)` 或净化后为空(大写/下划线/连续或首尾
  连字符/纯中文/纯空格)→ `InvalidRequest` 中文消息,**不写库**
- remote_url 未配置 → 仅落 key,`set_config(None)`,无 panic

### 5. Good/Base/Bad Cases

- Good: `carlos-home` → 落库,隧道以新 id 注册,remote upsert 新行
- Base: key 无值 → hostname 派生(存量默认行为)
- Bad: 两台机器不设自定义且 hostname 相同 → 互踢(见 Scope 症状)

### 6. Tests Required

- 派生三臂各自单测:key 优先于 hostname;key 空走 hostname;fallback
  UUID 持久化后二次读回稳定
- HTTP route 全链 roundtrip:非法值逐个断言 4xx **且库中无写入**;
  清除后 cfg 回 hostname 派生(真实案例:`08-26-custom-node-id` 的
  `tunnel_node_id_set_validate_clear_roundtrip`)

### 7. Wrong vs Correct

#### Wrong — hostname 优先于持久化 key

2026-08-26 前的实现:注释宣称"DB 文件即身份,不随 hostname 漂移",
但 hostname 派生成功时根本不读 key → 改名漂移 + 同 hostname 撞车。

#### Correct — key 有值即优先

身份以 `app_config["tunnel_node_id"]` 锚定;写入仅经 `set_tunnel_node_id`
(校验后的自定义值)或 fallback 生成;hostname 只影响"key 无值时"的默认。

### 8. 增补(同日):`set_tunnel_display_name` 镜像契约

- 三态同 `set_tunnel_node_id`;校验差异:**无字符集限制**(显示名给人看,
  允许中文,传输已有 percent-encode)+ `trim` 非空 + `chars().count() <= 64`
  (按字符数,非 UTF-8 字节)。
- 读取链不动:`build_tunnel_config` 里 `tunnel_display_name`(滤空)→
  hostname 原文 → node_id。remote 侧 `nodes.display_name` 经每次连接
  upsert 刷新——手机配对看到的 `node_display_name` 即来自该表。
- 展示语义:status 快照不带 displayName;自定义值经 `get_remote_config`
  的 `displayName` 回显(None = 自动)。

