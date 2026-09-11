# Implement:群聊预设 Settings 可编辑/可新增(GCE-P1)

> 两个 dispatch:Phase A(后端)先行——前端 routes-sync 守卫测试要解析 Rust 路由源码,后端不在则前端测试红。Phase B(前端 + 文档)依赖 A。每阶段末尾跑对应验证命令。

## Phase A:后端(Rust)

- [ ] A1 `db/migrations/schema.rs`:run_migrations 尾部加 `group_chat_presets` 建表段(design §1.1;幂等)。
- [ ] A2 `db/group_chat_presets.rs`:`GcPresetRow`(FromRow + camelCase serde,participants 存取 JSON 序列化在 db 层做,row 字段用 `Vec<GcPresetParticipant>` 内存形状)+ `list/get/create/update/delete`(models.rs 骨架,UUID v4 主键,list ORDER BY name);`db/mod.rs` 注册。
- [ ] A3 `db/group_chat_presets_tests.rs`:冒烟五件套(subagent_overrides_tests 模式:make_pool = 内存 + foreign_keys + run_migrations)。
- [ ] A4 `db/scheduled_tasks.rs`:`GroupChatTaskConfig` 加 `preset_key: Option<String>` + `#[serde(default)]`;补一条旧 JSON(无该键)反序列化 = None 的用例(放既有 config 测试旁)。
- [ ] A5 `commands/group_chat_presets.rs`:`*_inner` 四函数 + 校验(design §3,含硬编码内置 key/persona kind 常量 + 同步注释指向 scripts/group-chat-presets.json)+ `#[tauri::command]` 壳;`commands/mod.rs` 声明 + `all_command_names`;`lib.rs` invoke_handler 注册。
- [ ] A6 commands 校验测试:design §6 校验矩阵逐条。
- [ ] A7 `daemon/routes/group_chat_presets.rs`:四条薄壳 + `pub fn router(state)`;`routes/mod.rs` pub mod + `.nest("/api/v1/group_chat_presets", ...)`;文件尾 `#[cfg(test)]` oneshot wiring 测试(projects.rs:164 模式)。
- [ ] A8 验证:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test -p everlasting --lib`(全绿,含既有 scheduler 测试)。

## Phase B:前端 + 文档

- [ ] B1 `transport/http.ts`:CMD_TO_DOMAIN 加 4 行(domain `group_chat_presets`);跑 `pnpm test -- http.routes-sync` 确认守卫过。
- [ ] B2 `stores/groupChatPresets.ts`:pinia store(design §4.2;rows/loaded/spinner/load/create/update/remove + mergedPresets getter)。
- [ ] B3 `components/settings/GroupChatPresetsTab.vue`(design §4.3):内置只读区 + 用户列表 + 表单(主持人/参与者模型 Select 选项 = enabledModels ∪ 当前值;persona 五档;2-3 边界);registry.ts 条目(id `gc-presets`,scope global,group `智能体`)+ SettingsModal.vue import/map;registry.test.ts 如有分组断言则补。
- [ ] B4 `ScheduledTasksTab.vue`:选项构建改 merged;preset 判定改 merged;提交 config 加 `presetKey`(选中预设时);确认 snapshot 文案不变。
- [ ] B5 `GroupChatConfigModal.vue`:`gcPresetEntries` 改 merged;其余交互不动。
- [ ] B6 测试:GroupChatPresetsTab.test.ts(mock transport 按 cmd 分发,SubagentsTabModelOptions.test.ts 模式含 pointer capture stub);merged 单测;两消费方既有测试文件补用户预设用例(若现有 mock 直接读 JSON,需注入 store mock)。
- [ ] B7 验证:`cd app && pnpm test`(全量 vitest)。
- [ ] B8 文档:docs/DAEMON-API.md 新四条 endpoint;docs/DEBUG_DB.md schema 索引加表;AGENTS.md 群聊段落补一句"用户预设存 DB(Settings 可管理),内置四档仍是 scripts/group-chat-presets.json 单源"。
- [ ] B9(可砍尾巴)stale 提示:任务编辑态若 `preset_key` 指向的预设当前展开 ≠ 存档 config,显示一行提示。成本超预期即砍,不影响 AC。

## 验证命令速查

```bash
# 后端
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test -p everlasting --lib
# 前端
cd app && pnpm test
# scripts 回归确认零改动(应天然绿)
node --test scripts/group-chat-run.test.mjs && node --test scripts/group-chat-mcp.test.mjs
```

## 回滚点

- Phase A 独立可回滚(纯增量:新表/新命令/新路由 + GroupChatTaskConfig 加默认字段)。
- Phase B 依赖 A;B4/B5 是仅有的既有文件行为改动点,git 层面可单独 revert。
