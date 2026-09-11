# Settings 可管理实体 CRUD 先例全链路(调研 2026-09-12)

> 结论先行:仓库里**没有** `subagents` 表;subagent Settings 先例(`subagent_model_overrides`)是自然键 upsert(无 UUID、无 delete)。**"UUID 主键 + 全 CRUD" 的正确骨架先例是 `db/models.rs`(models 表)**。下述触点清单按 models.rs + subagent 先例合并整理。

## 1. DB 层

- 迁移 hub:`app/src-tauri/src/db/migrations.rs`(re-export schema/columns/pool/schema_helpers);建表序列在 `db/migrations/schema.rs::run_migrations`(schema.rs:29)。
- `subagent_model_overrides` 建表:schema.rs:735-764,`agent_name TEXT PRIMARY KEY / model_id / updated_at`,无 FK(soft-FK 惯例,见 `.trellis/spec/backend/database-guidelines.md`)。
- `models` 表建表:schema.rs:237;CRUD 在 `db/models.rs`:`create_model`(43,`Uuid::new_v4().to_string()` 生成 id)、`list_models`(97,JOIN provider 反范式)、`get_model`(130)、`update_model`(151)、`delete_model -> Result<bool>`(220);共享 `map_model_row`(20-40)防列清单漂移。
- subagent CRUD 在 `db/subagent_overrides.rs`:get/set(UPSERT)/clear/list,`Result<_, sqlx::Error>`,无事务;`list` 带 ORDER BY 保稳定序。
- 模块注册:`db/mod.rs`(subagent_overrides 在 82-83)。

## 2. daemon HTTP 路由

- 模式:routes/<domain>.rs 暴露 `POST /api/v1/<domain>/<cmd>`,handler 是薄 JSON 壳转发到 `commands::<domain>::*_inner`(单源);`routes/mod.rs::router` `.nest` 注册(routes/mod.rs:101-144;URL 约定注释在 15-20,与 Tauri 命令 1:1)。
- 本地 daemon 无 auth 中间件(server.rs:289 只挂 permissive CORS);错误统一 `AppCommandError` IntoResponse(daemon/error.rs:31-58,Auth→401 / InvalidRequest→400 / Server→500 / Network→502),错误 JSON 为 camelCase 序列化,前端读 kind/message/request_id。
- subagents 路由实例:routes/subagents.rs(两条 POST,51-59 router),共享逻辑在 commands/subagents.rs 的 `*_inner`,Tauri 壳在 157-163/333-342,lib.rs invoke_handler 注册(211 起,422-423),commands/mod.rs `all_command_names()`(88)。

## 3. 前端调用

- 统一走 `transport.invoke(cmd, args)`(`app/src/transport/index.ts:20-33` 默认 httpTransport);httpTransport 的 `CMD_TO_DOMAIN` 表在 `transport/http.ts:54-253`,**新增命令必须加一行**否则浏览器/sidecar 模式 `unknown cmd`;invoke 顶层 key camelCase→snake_case(http.ts:313-327);`!resp.ok` 抛 `TransportError`(265-282)。
- pinia store 先例:`app/src/stores/subagents.ts`(state rows/loaded/行级 spinner;action invoke + 错误经 extractErrorMessage 重抛)。
- SubagentsTab.vue:加载 `refresh()` 先 `models.load()`;保存行级错误 errorByName;下拉选项 = 启用模型 ∪ 本行已解析 id(SubagentsTab.vue:86-111,禁用模型需本行回显)。

## 4. Settings tab 注册

- `registry.ts`:`SettingsCategory`(20-33):id/scope/group/title/description/keywords;subagents 条目 86-93(group "智能体");分组联合类型在 18(`"模型" | "智能体" | "集成" | "存储" | "远程"`);`SETTINGS_GROUP_ORDER`(37-43)。
- `SettingsModal.vue`:id→组件映射 `CATEGORY_COMPONENTS`(73-87,subagents 在 79)+ import(55);渲染 `<component :is>`(355);导航 localStorage 记忆(91-117)。
- 新 tab 改动:registry 条目 + SettingsModal import/map 各一行;可补 registry.test.ts。

## 5. models store 前端形状

- `app/src/stores/models.ts`:`ModelWithProvider`(10-38:id/providerId/modelName/displayName/maxTokens/thinkingEffort/supportsThinking/supportsImages/contextWindow/disabled?/providerDisabled?/createdAt/updatedAt/providerDisplayName/providerProtocol)。
- `enabledModels` computed(93-95,过滤 disabled || providerDisabled);`modelsGroupedByProvider`(60-86);`load()`(125-133)。

## 6. 迁移/CRUD 测试模式

- `db/subagent_overrides_tests.rs` 可抄:`make_pool()` = 内存 SQLite + `PRAGMA foreign_keys = ON` + `run_migrations`(22-30);六个用例覆盖 get-不存在/set-往返/UPSERT-不重复/clear-不存在-Ok/clear-只删目标/list-有序,全 `#[tokio::test]`。

## 7. 路由测试模式

- Rust:routes/projects.rs:164 起 `#[cfg(test)]`,`tower::ServiceExt::oneshot` 打 `router(...)`,`post_json` helper 断言 StatusCode + JSON(注释明说"新 IPC 命令必须有一条 Router oneshot 测试锁 wiring");同模式 routes/permissions.rs:237、routes/agent.rs:115。
- 前端强制守卫:`app/src/transport/http.routes-sync.test.ts` 正则解析 routes/mod.rs 的 `.nest` 与各 domain 文件的 `.route("/{cmd}", post(`,断言与 CMD_TO_DOMAIN 双向一致——漏加 CI 必红。

## 8. 前端组件测试模式

- 先例 `SubagentsTabModelOptions.test.ts`:mock `transport.invoke`(按 cmd 分发)+ projects store;真 pinia mount;reka-ui Select 用键盘 Enter 打开,断言 teleport 到 body 的 `[role=option]`;beforeAll 补 pointer capture stub;afterEach 清 body。

## 9. 新 Settings 实体完整触点清单

1. `db/migrations/schema.rs` 建表段(幂等 CREATE TABLE IF NOT EXISTS)
2. `db/<entity>.rs` CRUD + `db/mod.rs` 注册 + `db/<entity>_tests.rs` 冒烟
3. `commands/<entity>.rs` `*_inner` + `#[tauri::command]` 壳 + `lib.rs` invoke_handler + `commands/mod.rs::all_command_names`
4. `daemon/routes/<entity>.rs` 薄壳 + `routes/mod.rs` pub mod + nest + oneshot wiring 测试
5. `transport/http.ts` CMD_TO_DOMAIN(routes-sync 守卫强制)
6. pinia store `stores/<entity>.ts`
7. `settings/registry.ts` 条目 + `SettingsModal.vue` 两行
8. 组件测试按 SubagentsTabModelOptions.test.ts 模式
