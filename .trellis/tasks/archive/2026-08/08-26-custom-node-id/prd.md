# PRD: 自定义 tunnel node_id 优先 + Settings 可编辑

## 背景

用户公司机与本机 hostname 同为 `carlos`,`derive_node_id` 每次连接从 hostname
实时派生 → 两台机器在 remote 侧共用同一 node_id,触发"新连接踢旧连接"的
互踢循环(退避连接成功即重置回 1s,永不停歇),手机配对后流量随机路由到
其中一台,配对表现异常。

现实现还有一个自相矛盾:`node_id.rs` 注释宣称"DB 文件即身份,不随
hostname 变更漂移",但 hostname 派生成功时 `tunnel_node_id` key 根本不被
读取,hostname 改名照样漂移。

## 需求

1. **`tunnel_node_id` key 设置即优先**:`derive_node_id` 改为三级——
   ① `app_config["tunnel_node_id"]` 有值(用户自定义或历史 fallback UUID)
   → 直接用;② hostname 净化派生(现行为);③ 随机 UUID 持久化(写同一
   key)。①统一覆盖自定义与 fallback,消除"漂移"矛盾。
2. **新 IPC `set_tunnel_node_id`**(三态扁平标量参数,照 `set_web_search_config`
   的 `tavily_api_key` 先例):
   - `Some(非空)` → 校验通过后落 `tunnel_node_id`,并触发隧道按新身份重连
     (走既有 `build_tunnel_config` + `tunnel_manager.set_config` 路径);
   - `Some("")` → 删/清 key,回到自动派生(hostname→UUID);
   - `None` → 不动(本次实现前端不传 None,保留三态形状与先例一致)。
   - 校验规则:trim 后须满足 `sanitize(x) == x` 且非空(即小写字母/数字/
     连字符,连字符不连续、不在首尾);失败返 `InvalidRequest` 人类可读
     中文消息,**不写库**。
3. **`get_remote_config` payload 增加字段 `nodeId: Option<String>`**:
   返回自定义值原文(None = 未设置/自动)。注意 payload 本身"未配置
   remote 时为 null"的语义不变。
4. **双形态三层落地**(铁律):`commands/config.rs` `_inner` + `#[tauri::command]`
   包装 + `daemon/routes/config.rs` HTTP route(`POST /api/v1/config/set_tunnel_node_id`,
   请求体 snake_case,`#[serde(default)]`);`http.ts` CMD_TO_DOMAIN 加行。
5. **前端 RemoteTab 节点信息区可编辑**:
   - 保留现有生效 node id 展示(status 轮询);
   - 新增输入框(预填 `config.nodeId ?? ""`,placeholder 说明"留空 = 自动
     (hostname 派生)")+ 保存按钮 + inline 错误提示(照 config 表单先例);
   - 保存成功后刷新 config + status;
   - 更新 RemoteTab.vue 头部 P3-1 注释与 `remoteConfig.ts` 的 P3-1 注释
     (约束已解除:本任务就是那次 Rust 改动)。
   - 提示文案注明:修改 node_id 后已配对设备绑定的是旧 id,需重新配对。

## 需求(2026-08-26 晚增补:显示名编辑,原非目标转正)

6. **新 IPC `set_tunnel_display_name`**(三态,镜像 `set_tunnel_node_id`):
   - `Some(非空)` → trim 后非空且长度 ≤ 64 → 落 `tunnel_display_name`
     (**无字符集限制**——显示名给人看,允许中文;传输已有 percent-encode);
     触发隧道重连(remote 侧经 upsert 刷新 nodes.display_name);
   - `Some("")` → 删 key,回默认(hostname → node_id,现读取逻辑不动);
   - `None` → 不动;
   - 非法(trim 空 / 超长)→ `InvalidRequest` 中文消息,不写库。
7. **`get_remote_config` payload 增 `displayName: Option<String>`**;
   RemoteTab 节点信息区加显示名编辑框(placeholder "留空 = 自动(hostname)",
   提示这是手机/远程列表看到的名字);更新 P3-1 相关注释(S4 当时"PC 端
   够不着 displayName"的约束由本任务解除)。

## 非目标(Out of Scope)

- remote 侧自动消歧(同 node_id 不同实例自动改名)。
- 默认 node_id 改为 UUID(保持 hostname 派生默认,可读性优先)。

## 验收标准

1. 单元测试(Rust):key 有值时优先于 hostname;key 空时 hostname 派生;
   hostname 不可用时生成并持久化 UUID(现行为回归);`set_tunnel_node_id`
   校验拒绝非法值(大写/下划线/连续连字符/首尾连字符/纯中文)且不写库。
2. `set_tunnel_node_id` 成功后 `TunnelConfig.node_id` 变化经 supervisor
   触发隧道重启(manager 既有 `cfg != current` 路径,不新增机制)。
3. `cargo test -p everlasting --lib` 全绿(含新增用例);
   `cd app && pnpm test` 全绿;`pnpm typecheck`(或项目等价命令)绿。
4. 前端:RemoteTab 可设置/清空自定义 id;非法值 inline 报错;保存后
   2s 轮询的 nodeId 反映新值(需要 daemon 在跑才连上,单元层面验证
   store 调用形状即可)。
5. 存量兼容:未设置自定义 id 的机器行为不变(hostname 派生;曾触发
   fallback 的机器继续用已持久化 UUID——语义从"hostname 可用时忽略"
   变为"始终优先",对这类机器是收紧为稳定,符合注释宣称的意图)。
6. 显示名(增补):`set_tunnel_display_name` 合法值落库后
   `build_tunnel_config` 取新值;清除后回 hostname 默认;trim 空/超 64
   字符拒绝且不写库;`get_remote_config` 回显 `displayName`。
