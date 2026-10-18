# 合并供应商与模型配置 + 模型探测 + 在线能力自动匹配

## Goal

设置面原本把「供应商(provider)」与「模型(model)」拆成两个平行分类:配 key / base URL 在 Providers 页,配模型要切到 Models 页再回选供应商。用户要一条**单屏闭环**:供应商卡片下直接管理它的模型,并让「新增模型」这件事尽量不用手打参数。

三件事:

1. **合并为一处** —— 供应商卡片内嵌其模型列表(默认展开),模型表单的 provider 锁定为卡片供应商;独立 Models 分类从导航移除。
2. **模型探测** —— 按供应商协议调用它的模型列表端点,把可用 model name 列出来,勾选批量导入(替代手打 model name)。
3. **按名称自动匹配能力** —— 表单里一个按钮,按模型名查在线目录(models.dev),把上下文窗口 / 最大输出 / vision / thinking 自动填好。

用户已确认的三项取舍(2026-10-19 会话):

- 合并形态 = **供应商卡片内嵌模型列表**。
- 探测含义 = **拉取供应商模型列表**(不是单模型能力探针)。
- 自动匹配数据源 = **纯在线查询**(models.dev,**不内置静态表**)。

## Requirements

### R1 设置面合并(前端)

- Providers 页每个供应商卡片下渲染模型管理区:
  - 模型列表(原名 / model id / 能力标签 / 上下文窗口 / 每行测试结果)。
  - 每行操作:禁用开关、连通性测试、编辑、删除(含删除确认)。
  - 工具行:模型数 + 「探测模型」+ 「添加模型」。
  - 卡片 chevron 折叠/展开模型区(默认展开);折叠态卡片头部仍显示模型数徽标。
- 模型表单的 provider 字段在本形态下锁定为卡片供应商(只读标签),避免跨供应商误存。
- 独立 `Models` 分类从 `registry.ts` 移除,`SettingsModal` 不再挂该组件。
- **兼容**:历史 `models` 分类 id(localStorage 记忆 / 引导卡 openSettings / 书签)经别名表落到 `providers`,不得被扔回「通用」页。
- 聊天空状态引导卡 CTA:卡 A/B 落 `providers`,卡 C(未设默认模型)落 `default`。
- 搜索词:原 Models 分类的关键词(模型 / 测试 / 连通 / connectivity)迁入 Providers,并新增 探测 / probe / models.dev / 自动匹配 / context window / 上下文。

### R2 模型探测(后端 + 前端)

- 新 IPC `probe_provider_models(provider_id)`,返回:

  ```json
  {
    "success": true,
    "latencyMs": 123,
    "models": [{ "modelName": "...", "displayName": "...", "spec": { … } | null }],
    "truncated": false,
    "error": null,
    "metadataSource": "models.dev",
    "metadataFetchedAt": "...",
    "metadataCached": true,
    "metadataStale": false,
    "metadataWarning": null
  }
  ```

- 端点按协议:anthropic → `GET {base}/v1/models?limit=1000`(`x-api-key` + `anthropic-version`);openai / openai_responses → `GET {base}/models`(`Authorization: Bearer`)。base_url 的 `/v1` 约定与 `test_model` 一致(openai 系 base 含 `/v1`,anthropic 不含)。
- 响应形状容错:`{data:[…]}` 或裸数组;取 `id`(回退 `name`);`display_name` / `name` 作显示名;**按 id 去重**、排序、上限 300 条(`truncated` 标记)。
- 失败面与 `test_model` 同族:provider 不存在 / 协议不支持 / 请求失败 / 非 2xx / JSON 非法 → `{success:false, error}` 且 HTTP 200,前端内联展示。
- 每条模型尽力附带在线目录能力(`spec`);目录不可用时列表照常返回,`metadataWarning` 带原因。
- 前端导入:过滤框 + 全选未添加 + 勾选计数;已存在的 model name 标「已添加」且不可勾选;导入 = 逐条 `add_model`(失败不中断,末尾报告「已添加 N · 失败 M」);能力字段来自目录,无目录记录用默认(200k / 无 vision / 不覆盖 max_tokens)。

### R3 按名称自动匹配能力(前端 + 后端)

- 新 IPC `lookup_model_metadata(model_name, provider_id?)`,返回:

  ```json
  {
    "success": true,
    "source": "models.dev",
    "fetchedAt": "...",
    "cached": true,
    "stale": false,
    "matches": [ModelSpec, …],
    "error": null
  }
  ```

- 数据源 `https://models.dev/api.json`(实测 232 供应商 / 9000+ 模型;字段 `limit.context`、`limit.output`、`modalities.input` 含 image、`reasoning`、`reasoning_options[type=effort].values`)。
- 落盘缓存 `{app_data_dir}/cache/models-dev.json`,TTL 24h(临时文件 + rename 写入);网络失败降级过期缓存并标 `stale: true`;两者都无 → `success:false`。
- 匹配是纯函数(可单测):名称归一化(小写 / 去供应商前缀 / 去括号注解 / `_ : . @ 空格` → `-` / 去尾 `-latest`)+ 分级打分(精确 id 100、可读名 96、日期型后缀 86、前缀 72、包含 55)+ 供应商线索加成 + 同 id 跨网关去重按一方厂商优先级裁决。
- 前端:表单「按名称自动匹配能力」按钮(模型名为空时禁用),写回上下文窗口、Max Tokens(= 目录 `limit.output`)、支持图片、支持思考、思考挡位(按协议词表挑 high → medium → low;目录无 effort 词表则不写挡位);Display Name 仅在用户留空时补;候选多于 1 个时给「其它候选」小按钮即时切换;结果/失败/离线缓存全部内联提示,不弹 toast。

### R4 双形态与契约同步

- 两条 IPC 走「`_inner` 单源 + daemon route」:命令名 `probe_provider_models` / `lookup_model_metadata`,域 `providers`。
- 同步点:`commands/model_metadata.rs`(业务)+ `daemon/routes/providers.rs`(路由)+ `daemon/routes/mod.rs` 注释 + `commands/mod.rs::all_command_names` + `transport/http.ts::CMD_TO_DOMAIN` + `tests/e2e.rs` 路由 smoke 清单。
- JSON wire 一律 camelCase(结构体 `serde(rename_all = "camelCase")` / `json!` 手写键)。

## Acceptance Criteria

- [ ] Providers 页每个供应商卡片内直接看到/管理其模型;独立 Models 分类消失;`models` 历史 id 落到 Providers。
- [ ] 卡片 chevron 可折叠模型区,折叠后模型数徽标仍可见;模型表单 provider 不可改。
- [ ] 「探测模型」对 httpmock OpenAI 兼容端点返回去重后的模型列表(后端 e2e 用例锁定);失败时错误内联、面板不崩。
- [ ] 探测列表可过滤、可全选未添加、可批量导入;已存在的模型不可重复导入。
- [ ] 表单「按名称自动匹配能力」在真实目录下能把 `claude-sonnet-4-5` 填成 200k ctx / 64000 输出 / vision ✓ / thinking ✓(官方条目优先于聚合网关变体)。
- [ ] 目录不可达时按钮给出可读错误,表单其余字段可手动填写并保存。
- [ ] `cargo test -p everlasting --lib` 全绿;`cargo test -p everlasting --test e2e all_api_routes_are_mounted` + 新增 e1f 三用例全绿;`app` 侧 `vue-tsc --noEmit` 与 `pnpm test` 全绿。

## Out of Scope

- 不内置静态模型能力表(用户明确选「纯在线查询」);离线只能用手填。
- 不做「单个模型的真实能力探针」(发请求猜上下文窗口)—— 那是另一条探测语义,本期只拉列表。
- 不做供应商侧模型删除同步(探测是只读;删模型仍只删本地 catalog 行)。
- 不改 `test_model` / `models.dev` 之外的目录源。
- 不改 DB schema。

## Notes

- 合并后 `app/src/components/settings/ModelsTab.vue` 删除,职责迁入 `ProviderModelsPanel.vue`。
- 在线目录的解析与匹配逻辑必须保持纯函数,便于离线单测;网络面只是薄壳。
