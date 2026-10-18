# Design — 合并供应商/模型配置 + 模型探测 + 在线能力匹配

## 1. 决策与边界

| 决策 | 取值 | 理由 |
|---|---|---|
| 合并形态 | 供应商卡片**内嵌**模型面板 | 用户点选;KEY / base URL 与它的模型同屏,消除跨页回选 |
| 探测语义 | 拉供应商**模型列表**(只读) | 用户点选;解决「手打 model name」这个真实痛点 |
| 能力数据源 | **纯在线** models.dev | 用户点选;避免发行版内置表陈旧 |
| 失败面 | 业务失败 = `{success:false,error}` + HTTP 200 | 与 `test_model` 同族;前端内联渲染,不升级为传输错误 |
| 目录缓存 | `{app_data_dir}/cache/models-dev.json`,TTL 24h,失败降级过期缓存 | 5.8MB JSON,每次点击都拉不可接受;daemon 是唯一 writer,写到 app_data_dir 与既有落盘纪律一致 |

非目标(明确不做):内置静态表、单模型真实能力探针、供应商侧删除同步、DB schema 变更。

## 2. 后端结构

```
app/src-tauri/src/commands/model_metadata.rs     ← 新增(业务 + 纯函数 + 单测)
app/src-tauri/src/commands/mod.rs                ← pub mod + all_command_names 两条
app/src-tauri/src/daemon/routes/providers.rs     ← 两个 handler + 两条 route
app/src-tauri/src/daemon/routes/mod.rs           ← 域注释计数更新
app/src/transport/http.ts                        ← CMD_TO_DOMAIN 两条
app/src-tauri/tests/e2e.rs                       ← 路由 smoke 清单 + e1f 三用例
```

### 2.1 `probe_provider_models_inner(state, provider_id)`

1. `db::get_provider` → 不存在/读失败直接返回失败载荷。
2. 按 `provider.protocol` 构造 GET:
   - anthropic: `{base}/v1/models?limit=1000`,头 `x-api-key` / `anthropic-version: 2023-06-01`
   - openai | openai_responses: `{base}/models`,头 `Authorization: Bearer …`
   - 其它: `unsupported protocol`
   - `base` 先 `trim_end_matches('/')`;`/v1` 归属沿用 `test_model` 的既有约定(anthropic base 不含 /v1,openai 含)。
3. 解析:裸数组或 `{data:[…]}`;`id` 优先、回退 `name`;去重(按 id)+ 排序 + 截断 300。
4. 富化:加载目录(见 2.3),逐条 `catalog.best(model_name, provider_hint)`;目录失败则 `metadataWarning` 携带原因,`models[].spec = null`。
5. 计时:请求发出前后 `Instant`,失败分支也回报 `latencyMs`。

### 2.2 `lookup_model_metadata_inner(state, model_name, provider_id?)`

1. `model_name.trim()` 为空 → `{success:false, matches:[], error:"model name is empty"}`(不触网络)。
2. `provider_hint`:provider 存在时用 `guess_provider_key(base_url, protocol)`;base_url 主机名命中表优先,其次协议默认(`anthropic`/`openai`),否则 None。
3. `catalog.matches(query, hint, 8)`;返回目录元信息(`fetchedAt` / `cached` / `stale`)。

### 2.3 目录加载 `load_catalog`

顺序:新鲜缓存(≤24h)→ 网络拉 `api.json` → 失败时读**过期**缓存(`stale: true` + `tracing::warn`)→ 都没有则 `Err(错误串)`。

- 写入用 `xxx.json.tmp` + `rename`,避免并发/崩溃留下半截 JSON。
- 读失败(损坏)按缺失处理(下次重拉)。
- 缓存目录不存在时 `create_dir_all` 后写入。

### 2.4 纯函数与不变量(单测锁定的部分)

| 函数 | 不变量 |
|---|---|
| `normalize_model_name` | `Anthropic/claude-sonnet-4-5` = `Claude Sonnet 4.5 (latest)` = `claude-sonnet-4-5`;`_ : . @ 空格`→`-`;去尾 `-latest` |
| `score_entry` | 精确 100 > 名 96 > 日期型后缀 86 > 前缀 72 > 包含 55;**非日期型后缀不给 86**(`gpt-4o` 不得吞 `gpt-4o-mini`) |
| `matches` | 同归一化 id 跨网关**只留一条**,按 `(score, priority)` 裁决;JSON 对象遍历顺序不影响结果 |
| `guess_provider_key` | 主机名命中优先于协议默认;未知主机回落协议默认 |
| `ModelsCatalog::from_json` | 结构不符(根非对象 / 供应商缺 models)不 panic,返回可用子集或空目录 |

`priority` 顺序(`provider_priority`):一方厂商(anthropic / openai / google / deepseek / zai / zhipuai / moonshot / xai / mistral / alibaba / minimax / meta / cohere / groq / opencode)在前,其余聚合网关并列在后。**这条是线上实证驱动的**:实测不带 hint 查 `claude-sonnet-4-5` 时,聚合网关 `aihubmix` 的同名条目(context=1M)会顶掉官方条目(context=200k),修法就是去重按 priority 而非「先到先得」。

`@` 归一化同理来自线上:Vertex 的 `claude-sonnet-4-5@20250929` 与 `-20250929` 若不归一,候选里会出现两条同模型。

## 3. 前端结构

```
app/src/utils/modelMetadata.ts                          ← 新增:两条 IPC 的类型 + 纯函数折算
app/src/utils/modelMetadata.test.ts                     ← 新增:12 例
app/src/components/settings/ProviderModelsPanel.vue     ← 新增:卡片内模型管理(原 ModelsTab 职责 + 探测)
app/src/components/settings/ProviderModelsPanel.test.ts ← 新增:5 例
app/src/components/settings/ModelForm.vue               ← 增 providerLabel / matching / matchHint + auto-match 按钮
app/src/components/settings/ProvidersTab.vue            ← 卡片头部 + chevron + 模型数徽标 + 内嵌面板
app/src/components/settings/ModelsTab.vue               ← 删除
app/src/components/settings/registry.ts                 ← 删 models 分类 + LEGACY_CATEGORY_ALIASES
app/src/components/settings/SettingsModal.vue           ← 摘除 ModelsTab 挂载
app/src/components/chat/ChatPanel.vue                   ← 引导卡 CTA 落点(providers / default)
```

### 3.1 组件职责

- `ProvidersTab`:供应商 CRUD(既有)+ 卡片折叠状态(`collapsed: Set<string>`,默认展开)+ 模型数徽标(读 `modelsStore`)+ 把 `ProviderRow` 下发给 `ProviderModelsPanel`。
- `ProviderModelsPanel`(:provider):模型 CRUD / 测试 / 禁用 / 删除确认 / 探测面板 / 表单编排 / 自动匹配。
- `ModelForm`:纯展示 + emit(`submit` / `cancel` / `auto-match`);`providerLabel` 非空 → provider select 换成只读标签。

### 3.2 自动匹配的字段折算(`capabilitiesFromSpec`)

| 目录字段 | 表单字段 | 缺省行为 |
|---|---|---|
| `limit.context` | `contextWindow` | 缺失沿用当前值(编辑)或 200k |
| `limit.output` | `maxTokens` | 缺失留空 = 不覆盖全局默认 |
| `modalities.input` 含 `image` | `supportsImages` | false |
| `reasoning` | `supportsThinking` | false |
| `reasoning_options[effort].values` | `thinkingEffort` | 无 effort 词表 → `"none"`(不显式发送) |

挡位按协议过滤:`openai_responses` 只认 `minimal|low|medium|high`,其余认 `low|medium|high|xhigh|max`;偏好顺序 high → medium → low → 首个合法值。**理由**:Responses 协议对 `xhigh/max` 会 400(`responses.rs::normalize_responses_effort` 的既有约束),自动填进去等于埋雷。

### 3.3 提醒而不是覆盖

- `Display Name` 仅在用户留空时填目录名 —— 自动匹配不该覆盖用户已输入的内容。
- 目录返回 `stale` 时提示「离线缓存,可能不新」,仍可用。

## 4. 兼容性与回滚

- **历史 id**:`registry.LEGACY_CATEGORY_ALIASES = { models: "providers" }`,`findCategory` 先过别名。覆盖三处入口:localStorage 记忆、`openSettings("models")`、外部书签。
- **回滚**:删 `ProviderModelsPanel` 还原 `ModelsTab` 即可恢复双页形态;后端两条 IPC 独立,不删也不影响既有命令。
- **无 schema 变更**:`models` / `providers` 表零改动。

## 5. 风险

| 风险 | 缓解 |
|---|---|
| models.dev 结构漂移 | 解析全部走 `get/pointer` + 默认值,缺字段只降级不崩;`#[ignore]` 的 live 测试锁「真实 JSON 仍可解析 + 常见模型名仍命中」 |
| 目录 5.8MB 每次点击 | 24h 落盘缓存;探测富化与匹配共用同一索引(一次加载) |
| 聚合网关变体污染匹配结果 | 去重按 priority;provider 主机名 hint 加成 |
| 批量导入慢 / 部分失败 | 逐条 add(每条成功即 `modelsStore.load()` 刷新),失败计数不中断;上限 300 条 |
| 供应商列表端点不存在(部分代理) | 失败内联展示原始 `HTTP 4xx + body[:200]`,用户可回退手填 |
| jsdom 无 `EventSource` / 网络 | 前端测试全 mock `transport`;后端 e2e 用 `httpmock`,不触真实目录 |

## 6. 验证

- 后端:`cargo test -p everlasting --lib model_metadata::`(11 例,含去重/优先级/协议词表)+ `cargo test -p everlasting --test e2e e1f_model_probe::`(3 例)+ `all_api_routes_are_mounted`。
- 前端:`pnpm vue-tsc --noEmit`;`pnpm test`(新增 17 例:12 utils + 5 panel + 2 providers tab)。
- 手动 live:临时 daemon(`--port 7599 --data-dir <tmp>`)→ `POST /api/v1/providers/lookup_model_metadata` 实测返回官方 `claude-sonnet-4-5` 200k / 64000;第二次请求命中落盘缓存(`cached:true`)。
