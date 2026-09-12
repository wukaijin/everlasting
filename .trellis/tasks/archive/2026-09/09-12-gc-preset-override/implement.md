# Implement:群聊预设内置档覆盖层(GCE-P1b)

> 两个 dispatch:Phase A(后端)先行——前端覆盖行依赖 wire 的 `builtinKey` 字段
> 与 create 的 `builtin_key` 参数。Phase B(前端 + 文档)依赖 A。每阶段末尾跑
> 对应验证命令。命令名零新增 → routes-sync / CMD_TO_DOMAIN / invoke_handler 均
> 不动。

## Phase A:后端(Rust)

- [ ] A1 `db/migrations/columns.rs`:`add_group_chat_presets_column_if_missing`
  (mirror add_scheduled_tasks_column_if_missing);`db/migrations/schema.rs`:
  CREATE TABLE 段补 `builtin_key TEXT` 列 + run_migrations 建表后调用幂等加列 +
  `CREATE UNIQUE INDEX IF NOT EXISTS idx_group_chat_presets_builtin_key`(design §1.1)。
- [ ] A2 `db/group_chat_presets.rs`:`GcPresetRow` 加 `builtin_key: Option<String>`
  (serde default + skip_serializing_if none);map_row / list / get 的 SELECT 列清单
  加列;`create_group_chat_preset` 加 `builtin_key: Option<&str>` 参数并写列;
  update 不触碰该列(design §1.1 不可变)。
- [ ] A3 `db/group_chat_presets_tests.rs`:补三件——覆盖行往返 / 两条 NULL 行共存
  (UNIQUE NULL 互不相撞)/ 同 key 第二条覆盖被 UNIQUE 拒。
- [ ] A4 `commands/group_chat_presets.rs`:`create_*_inner` + `#[tauri::command]`
  加 `builtin_key: Option<String>` 参数;create 校验追加两臂(∈ BUILTIN_PRESET_KEYS
  / 同 key 无既有覆盖行,design §3);注释块同步覆盖行语义。
- [ ] A5 `commands/tests_group_chat_presets.rs`:补校验矩阵(design §6 commands 行)。
- [ ] A6 `daemon/routes/group_chat_presets.rs`:`CreateGroupChatPresetRequest` 加
  `#[serde(default)] builtin_key: Option<String>` 透传;oneshot 测试尾追加覆盖流
  (snake `builtin_key` → camelCase `builtinKey`;同 key 二次 400)。
- [ ] A7 验证:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test -p everlasting --lib`(全绿;含既有 GCE-P1 测试零回归)。

## Phase B:前端 + 文档

- [ ] B1 `stores/groupChatPresets.ts`:`GcPresetRow.builtinKey?` /
  `GcPresetInput.builtinKey?` / `create()` 透传 / `mergedPresets` 原位顶替 +
  `MergedGcPreset.overriddenBy?`(design §4.1;未知 builtinKey 跳过)。
- [ ] B2 `components/settings/GroupChatPresetsTab.vue`:overrideByKey /
  sortedRows 过滤覆盖行 / 内置行三态(覆盖编辑 → 编辑覆盖 + 恢复内置)/ 表单
  `overridingKey` + 预填解析(resolveModelRef 全目录,缺失留空)/ ConfirmDialog
  恢复文案(design §4.2)。
- [ ] B3 测试:`stores/groupChatPresets.test.ts` 顶替单测;`GroupChatPresetsTab.test.ts`
  覆盖编辑 / 恢复内置 / 列表过滤;`ScheduledTasksTab.test.ts` 覆盖档预填 + AC3
  stale;`GroupChatConfigModal.test.ts` 覆盖档 roster。
- [ ] B4 验证:`cd app && pnpm test`(全量 vitest,含 routes-sync)。
- [ ] B5 文档:docs/DAEMON-API.md §6.4(create 加 builtin_key 参数 + 覆盖语义);
  docs/DEBUG_DB.md group_chat_presets 行补 builtin_key;AGENTS.md GCE-P1 段补
  override 一句;`.trellis/spec/backend/group-chat-presets.md` 增补 override 契约
  (Phase 3.3 spec 更新时做,文件清单见 check)。

## 验证命令速查

```bash
# 后端
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test -p everlasting --lib
# 前端
cd app && pnpm test
# scripts 零改动确认(应天然绿,不必跑;git diff scripts/ 为空即证)
```

## 回滚点

- Phase A 纯 additive:加列(可空)+ 索引 + create 可选参数,旧调用行为不变。
- Phase B 依赖 A;B2 是仅有的既有组件行为改动点(内置区从只读变可覆盖),git 层面
  可单独 revert。
