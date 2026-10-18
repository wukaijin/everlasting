# Model Catalog Contract — 模型探测 + 在线能力匹配

> **基线**:2026-10-19(task `10-19-merge-provider-model-settings`)
> **同源文档**:
> - [test-model-contract.md](./test-model-contract.md) — `test_model` 单模型连通性探测(本文的姊妹契约,失败面与 URL 约定同族)
> - [multi-provider-contract.md](./multi-provider-contract.md) — Provider trait + catalog + 三协议分发
> - [daemon-server.md](./daemon-server.md) — `_inner` 单源 + daemon route 约定
>
> **何时读本文**:改 `probe_provider_models` / `lookup_model_metadata` / models.dev 缓存逻辑 / 设置面「探测模型」「按名称自动匹配能力」/ 模型名归一化与候选排序时。

---

## Scenario: 两条模型目录 IPC(2026-10-19)

设置面把供应商与模型合并到一处(供应商卡片内嵌模型面板),并让「新增模型」尽量不用手打参数。后端提供两条命令,**都是「业务失败 200 + `{success:false, error}`」** —— 与 `test_model` 同族,前端统一内联渲染,不升级成传输错误。

实现单点:`app/src-tauri/src/commands/model_metadata.rs`(业务 + 纯函数 + 单测);
路由镜像:`app/src-tauri/src/daemon/routes/providers.rs`(域 `providers`)。

---

## 1. `probe_provider_models` — 供应商模型列表探测

### 1.1 请求 / 响应

```jsonc
// 请求(POST /api/v1/providers/probe_provider_models,body snake_case)
{ "provider_id": "<providers.id>" }

// 成功
{
  "success": true,
  "latencyMs": 123,
  "models": [
    { "modelName": "claude-sonnet-4-5", "displayName": "claude-sonnet-4-5",
      "spec": { /* ModelSpec,见 §3;未命中目录时为 null */ } }
  ],
  "truncated": false,
  "error": null,
  "metadataSource": "models.dev",
  "metadataFetchedAt": "2026-10-19T…Z",
  "metadataCached": true,
  "metadataStale": false,
  "metadataWarning": null
}
```

### 1.2 端点与鉴权(按 `provider.protocol`)

| protocol | URL | Headers |
|---|---|---|
| `anthropic` | `GET {base}/v1/models?limit=1000` | `x-api-key: {key}`、`anthropic-version: 2023-06-01` |
| `openai` / `openai_responses` | `GET {base}/models` | `Authorization: Bearer {key}` |
| 其它 | 不请求,`{success:false, error:"unsupported protocol: …"}` | — |

`base` 先 `trim_end_matches('/')`。**`/v1` 归属沿用 `test_model` 的既有约定**:anthropic 的 base_url 不含 `/v1`,openai 系的 base_url 含 `/v1`(见 [test-model-contract §3](./test-model-contract.md))。`limit=1000` 是为了绕开 Anthropic 默认 20 条分页。

### 1.3 解析与去重

- 响应形状两种都接受:`{ "data": [...] }` 与裸数组(部分兼容网关)。
- 取 `id`,回退 `name`;显示名取 `display_name` → `name` → id。
- **按 id 去重**、按 id 排序,截断 300 条(`truncated: true`)。
- 唯一的非 2xx / JSON 非法 / 请求失败都返回 `success:false`(错误串含 `HTTP <status>: <body[:200]>` 或 `request failed: <e>`)。

### 1.4 目录富化(尽力而为)

每条模型用 `ModelsCatalog::best(model_name, provider_hint)` 补 `spec`;`provider_hint` 由 `guess_provider_key(base_url, protocol)` 推导。目录不可用时:`spec` 全为 `null`、`metadataWarning` 带原因、**列表照常返回**。

---

## 2. `lookup_model_metadata` — 按名称匹配在线能力

### 2.1 请求 / 响应

```jsonc
// 请求
{ "model_name": "claude-sonnet-4-5", "provider_id": "<uuid> | absent" }

// 成功
{
  "success": true,
  "source": "models.dev",
  "fetchedAt": "2026-10-19T…Z",
  "cached": true,
  "stale": false,
  "matches": [ /* ModelSpec[],最多 8 条,第 1 条 = 最佳匹配 */ ],
  "error": null
}
```

- `model_name` trim 后为空 → `{success:false, matches:[], error:"model name is empty"}`,**不触网络**。
- `provider_id` 只作匹配排序线索,不参与过滤;缺省时按目录自身的供应商优先级裁决。

### 2.2 数据源与缓存

- 源:`https://models.dev/api.json`(公开,实测 232 供应商 / 9000+ 模型 / 5.8MB)。
- 落盘:`{app_data_dir}/cache/models-dev.json`,**TTL 24h**;写入用 `.json.tmp` + `rename`。
- 加载顺序:新鲜缓存 → 网络 → 失败降级**过期缓存**(`stale:true` + `tracing::warn`)→ 都没有则 `success:false`。
- 目录损坏(JSON 解析失败)按缺失处理,下次重拉。

### 2.3 目录字段映射(models.dev → `ModelSpec`)

| models.dev | ModelSpec | 缺省 |
|---|---|---|
| `id` | `modelId` | map key |
| `name` | `name` | `modelId` |
| `limit.context` | `contextWindow` | `null` |
| `limit.output` | `maxOutputTokens` | `null` |
| `modalities.input` 含 `"image"` | `supportsVision` | 旧 schema 回退 `attachment` |
| `reasoning` | `supportsThinking` | `false` |
| `reasoning_options[type=effort].values` | `thinkingLevels` | `[]`(budget_tokens 型不进此列) |
| `release_date` | `releaseDate` | `null` |

---

## 3. 匹配算法(纯函数,单测锁定)

### 3.1 归一化 `normalize_model_name`

小写 → 去供应商前缀(`anthropic/xxx` → `xxx`)→ 去括号注解(`(latest)`)→ `_ : . @ 空格` 统一成 `-` → 折叠重复 `-` → 去首尾 `-` 与尾 `-latest`。

两侧(目录 / 用户输入)共用同一函数。**`@` 必须归一**:Vertex 目录条目用 `claude-sonnet-4-5@20250929`,不归一就与 `-20250929` 拼法重复出现在候选里。

### 3.2 打分 `score_entry`

| 条件 | 分 |
|---|---|
| 目录 id 归一化 == 输入 | 100 |
| 目录可读名归一化 == 输入 | 96 |
| 输入 = 目录 id + **变体后缀** | 86 |
| 目录 id = 输入 + 任意后缀(输入 ≥4 字符) | 72 |
| 输入(≥5 字符)是目录 id 的子串 | 55 |
| 其它 | 0(不匹配) |

**变体后缀白名单**(`is_variant_suffix`):纯数字/连字符(`-20250929`),以及 `latest` / `preview` / `thinking` / `instruct` / `reasoning` / `high` / `chat`。**非白名单后缀不给 86** —— 否则 `gpt-4o` 会吞掉 `gpt-4o-mini`(单测 `variant_suffix_guard_blocks_family_collision` 守门)。

### 3.3 候选去重与排序(线上实证)

同一归一化 id 会跨多个供应商重复出现。去重必须按 **`(score, provider_priority)`** 裁决,不能「先到先得」—— models.dev 的 JSON **对象遍历顺序不可依赖**。

实证(2026-10-19 live):不带 hint 查 `claude-sonnet-4-5` 时,聚合网关 `aihubmix` 的同名条目(`context=1000000`)会顶掉官方 `anthropic` 条目(`context=200000`),自动匹配就会填错上下文窗口。修法 = `provider_priority`(一方厂商在前,聚合网关在后并列)参与去重裁决。

排序:分数降序 → priority 升序 → provider 名 → modelId。单测 `gateway_variants_do_not_beat_official_entry_on_priority` 锁死。

### 3.4 供应商线索 `guess_provider_key`

base_url 主机名命中表优先(`api.anthropic.com` → anthropic、`open.bigmodel.cn` → zhipuai、`api.z.ai` → zai …),未命中回落协议默认(`anthropic` / `openai`)。**只是排序加成(+6),不过滤**。

---

## 4. 前端折算(契约)

`app/src/utils/modelMetadata.ts` 的纯函数(单测 12 例):

| 目录字段 | 表单 / 落库字段 | 规则 |
|---|---|---|
| `limit.context` | `contextWindow` | 缺失沿用当前值或 200k |
| `limit.output` | `maxTokens` | 缺失留空 = 不覆盖全局默认 |
| `modalities.input` 含 image | `supportsImages` | — |
| `reasoning` | `supportsThinking` | — |
| `reasoning_options[effort]` | `thinkingEffort` | 按协议词表过滤,**无 effort 词表则不写挡位** |

**协议词表**:`openai_responses` → `minimal|low|medium|high`;其余 → `low|medium|high|xhigh|max`。偏好顺序 high → medium → low → 首个合法值。理由:Responses 对 `xhigh/max` 会 400(`responses.rs::normalize_responses_effort`),自动填进去等于埋雷。

**不覆盖用户输入**:`Display Name` 仅在用户留空时补目录名。

---

## 5. 跨层同步清单(新增同类 IPC 照抄)

1. `commands/model_metadata.rs` 的 `*_inner`
2. `daemon/routes/providers.rs` 的请求结构体(snake_case 字段)+ handler + `.route`
3. `commands/mod.rs::all_command_names`
4. `transport/http.ts::CMD_TO_DOMAIN`
5. `tests/e2e.rs` 的 `all_api_routes_are_mounted` 清单
6. `daemon/routes/mod.rs` 的域注释计数

漏第 4 步 → 浏览器 / remote 模式报 `unknown cmd`(Tauri IPC 不走该表,单测测不出);`http.routes-sync.test.ts` 守门。

---

## 6. 测试

| 层 | 用例 |
|---|---|
| Rust 单测(离线) | `model_metadata::tests::*` 11 例:归一化、精确优先、日期后缀、前缀、变体守卫、跨网关去重优先级、thinking/vision 投影、provider hint、空目录、`@` 归一 |
| Rust 单测(live,`#[ignore]`) | `live_catalog_parses_and_matches_common_models` — 真实 `api.json` 仍能解析且常见模型名仍命中;目录结构漂移时它先红 |
| Rust e2e | `e1f_model_probe` 三例:httpmock `/models` 探测去重 + 鉴权头、未知 provider 返回失败载荷、空 model name 短路 |
| 前端 | `utils/modelMetadata.test.ts`(12)+ `ProviderModelsPanel.test.ts`(6:探测列表/导入能力字段/失败内联/自动匹配填表/其它候选切换/失败提示)+ `ProvidersTab.test.ts`(内嵌面板 + chevron 折叠) |

---

## 7. 反模式

| 反模式 | 后果 |
|---|---|
| 依赖 JSON 对象遍历顺序去重 | 上下文窗口随机取到聚合网关口径(实测事故) |
| 目录 `limit.output` 写进 `context_window` | 窗口被砍到输出上限,长会话被压缩 |
| 把目录 `minimal` 无条件填进 anthropic 模型的 `thinking_effort` | 非 Responses 协议可能 400 / 语义漂移 |
| 目录不可达时让「探测」整体失败 | 用户明明能拉列表,只是缺能力字段 —— 应降级返回 + `metadataWarning` |
| 让 `display_name` 被自动匹配覆盖 | 用户手填的名字被冲掉 |
