# 代码证据汇总(2026-09-15,两轮 Explore + spec 复核)

> 规划期调研落盘。所有 file:line 以 2026-09-15 工作区为准。

## A. skill 系统(B4)

- 四层来源,优先级 `Plugin > BuiltinPlugin > Project > User`(`app/src-tauri/src/skill/loader.rs:63-78`);merge 插入顺序 user → project → builtin-plugin → plugin(`loader.rs:490-523`,后插覆盖);查找 plugin → builtin-plugin → project → user(`loader.rs:563-599`)。
- **agent 链已有「全局 Builtin 垫底」先例**:`Project-plugin > BuiltinPlugin > Project > User > Builtin`(spec `tool-contract/12-builtin-plugin-source-layer.md` 开头)——skill 侧加全局层照抄此形,用户/项目可覆盖。
- BuiltinPlugin 层编译期 `include_str!`(源 `app/src-tauri/resources/builtin-workflow/{dev,review}/skills/`,清单 `agent/workflow/builtin.rs:22-43, 69-86`),仅 workflow 会话可见;`parse_skill_content` 纯函数复用同一 frontmatter parser。
- L0 渐进披露:`build_skill_listing_block()`(`loader.rs:623-649`)单 Text block `<available-skills>`(名字+description+tools),`cache_control: Ephemeral`;注入点 `agent/chat_loop/init.rs:641-711`。L1 = `use_skill`(`tools/use_skill.rs:64-101`)。L2 = read_file。
- `/` 面板:`commands/panel.rs:121-196` 只走 `list_skill_infos`(user+project),内置层不可见;跨类型去重 builtin > skill > custom command(`panel.rs:10-24, 269-315`)。
- frontmatter 仅 name/description/allowed-tools(`skill/loader/frontmatter.rs:15-20`);SKILL.md ≤64KiB(`loader.rs:56`)。
- 新内置资源镜像步骤清单:spec `workflow-plugin-builtin.md`。

## B. 工具注册 / 权限 / token 治理

- 新工具最小集:`tools/mod.rs` 模块声明(:19-52)+ `builtin_tools()` 追加 definition(:145-271,追加到末尾,喂 provider 前缀缓存)+ `execute_tool_inner` match arm(:530-736)。
- `ToolContext` 有 `db: SqlitePool`(`tools/mod.rs:392`),无 AppState;DB 访问先例 `tools/search_history.rs:134-154`(`search_messages(&ctx.db,…)`)。
- `test_model_inner` 现签名 `(state: &Arc<AppState>, model_id)`(`commands/providers.rs:405-550`)但**只用了 `state.db`**(:409 get_model、:427 get_provider)+ 自建 reqwest client——收敛成 `(db: &SqlitePool, model_id)` 即可从工具路径复用;两调用方:Tauri command(:552-558)、daemon 路由(`daemon/routes/providers.rs:253-259`,daemon 复用 `*_inner` 是既定模式)。
- 权限:未列名工具落 `ToolKind::Other` → Tier 5 静默 Allow(`agent/permissions/check/permission.rs:590-596, 624-646`);先例 search_history / web_search(`tools/mod.rs:235-254` 注释明确)。`risk_for_tool` 默认 Low。
- L2 并行:`NAME_ELIGIBLE = ["read_file","grep","glob","list_dir","use_skill"]`(`agent/chat_loop.rs:1532`)硬编码名单,不在名单 = 串行,零声明。
- stub/预算:STUB_CANDIDATES 手工维护(`tools/stub.rs:34-52`),新工具不自动进;小 schema(1-3 参)不进候选是先例决策(search_history 校准注 3);**加全量 schema 须同步平移静态预算线** `static_token_budget_classic_chat_first_turn`(≤4200 @ `stub.rs:348-389`,schedule 家族平移先例 :336-340)。
- 工具 schema 约定:snake_case 英文名;**description 英文散文含 when-to-use**(search_history.rs:52-88 先例);返回面向 LLM 紧凑文本,错误 `(String, is_error=true)`。
- worker 可见性:`STRUCTURALLY_DISABLED`(`subagent/tools_filter.rs:24`)/ `READONLY_TOOL_ALLOWLIST`(:135)可选,不加 = 串行 worker 可见。群聊白名单(`group_chat_prompts.rs:458-473`)外自动排除。
- **脱敏警示**:`db::ProviderRow` 含明文 `api_key`(解密后),llm_diagnostics 绝不能直接 serde 序列化该行。

## C. test_model 契约(spec `backend/test-model-contract.md`)

- per-model 1-token round-trip(anthropic 打 `/v1/messages`、openai 打 `/chat/completions`,`body.model` = catalog 的 model_name,禁硬编码);15s 超时;响应恒 `{success, latencyMs, error}` 四失败路径不抛 Rust Err。
- 错误矩阵:model not found / provider missing / request failed / HTTP non-2xx(body 截 200 字符)/ unsupported protocol。
- 无自动化 HTTP 测试是**有意的**(spec §6),手动冒烟即契约——本任务加工具包装时不改语义,只收敛签名。

## D. 前端改动点

- 数据:pinia `stores/providers.ts:25`(providers+loaded+load)、`stores/models.ts:45`(models/defaultModelId/enabledModels);**app 启动即加载**:`stores/config.ts:161-169` `Promise.all([providers.load(), models.load()])`,调用点 `ChatWindow.vue:25-26` onMounted。防 flash gate 先例:`ModelSelect.vue:149` `config.loaded`。
- 空状态:`ChatPanel.vue:1096-1119`(三态链 sessionLoading 骨架 :1087 → !hasMessages 空态 :1096 → MessageList :1120;`chat-panel__empty-warn` 是分态渲染先例);空 session 判断 `hasMessages`(:107)。引导卡样式先例 `EmptyProjectState.vue`(BEM + design token)。
- Settings 打开:纯本地 ref(`Sidebar.vue:74-78, 238`),**无全局入口**;初始分类只从 localStorage 恢复(`SettingsModal.vue:128-154`),无「指定初始 tab」入参——需新增 prop `initialCategory`(分类 id `"providers"`,registry `settings/registry.ts`)+ 一个跨层打开通道(新 pinia store 最顺)。
- 错误行:`chat/MessageItemFooter.vue:206-224`(`error?: {message, category?}`,`data-testid="msg-error-row"`,retry 按钮 :214-223,`canRetry` = categoryRetryable && !streaming :117-122);组件刻意无 store import(文件头注释 :19-28),新按钮走 emit → 父编排。**消息行无 model 字段**(chat.types.ts:306 起),model_id 在 session 级(chat.types.ts:486);群聊解析先例 `MessageItem.vue:233-242`。
- test_model 前端先例:`ModelsTab.vue:178-197` runTest(TestState running/ok(latencyMs)/fail(error));三传输模式全通(daemon 路由 `daemon/routes/providers.rs:276` + CMD_TO_DOMAIN 映射 `transport/http.ts:175`,守卫测试 `http.routes-sync.test.ts`)。
- 前端测试模式:纯展示组件 mount+props 不启 pinia(MessageItemFooter.test.ts:41-59);需 store 的组件 createPinia+vi.mock transport(MessageItem.test.ts:34-49)。

## E. 错误分类链(修复建议文案的依据)

- `LlmError` 5 类 → `user_message()` 中文文案(`llm/error.rs:21-82`);配置类 `PreFlightError`(NoModel/ProviderMissing/EmptyApiKey/DecryptFailed/BuildFailed,`agent/provider.rs:36-91`);`ChatEvent::Error {message, category}` → 气泡错误行。
- 前端 `utils/error.ts` categoryRetryable / categoryToastKey;ErrorCategory = auth|rate_limit|invalid_request|server|network(chat.types.ts:31-36)。
