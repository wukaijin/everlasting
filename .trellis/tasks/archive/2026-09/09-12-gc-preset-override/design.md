# Design:群聊预设内置档覆盖层(GCE-P1b)

> 先例:列迁移抄 `add_scheduled_tasks_column_if_missing`(columns.rs:212,
> scheduled_tasks F2b 双路径:CREATE TABLE 补列给新库 + 幂等加列给存量库);
> 校验沿 `commands/group_chat_presets.rs::validate_preset_input` 单源;UI 沿
> GroupChatPresetsTab 既有三区结构。GCE-P1 全契约见
> `.trellis/spec/backend/group-chat-presets.md`。

## 1. 数据模型

### 1.1 `group_chat_presets` 加列 + 唯一索引

```sql
-- CREATE TABLE 段(schema.rs,新库)补列:
builtin_key TEXT                -- NULL = 普通用户行;值 ∈ 内置四 key = 覆盖行

-- run_migrations 在建表后追加(存量库补列 + 双库建索引):
add_group_chat_presets_column_if_missing(pool, "builtin_key", "TEXT")
CREATE UNIQUE INDEX IF NOT EXISTS idx_group_chat_presets_builtin_key
  ON group_chat_presets(builtin_key)
```

- SQLite UNIQUE 索引对 NULL 互不相撞 → 每个内置 key 至多一条覆盖行(DB 层铁律),
  普通用户行(全 NULL)不受影响。commands 层 create 前置查重给出可读 400,UNIQUE
  只作并发兜底(name UNIQUE 同款分工,竞态穿透 → 500 可接受)。
- 覆盖行 = 普通行的超集:同样有 name(管理面 display 名)/ description /
  moderator_model_id(UUID)/ participants / 时间戳。`builtin_key` 是唯一的顶替
  链接键;**update 不触碰该列**(创建时定死,不可把覆盖行转普通行)。

## 2. wire 契约(additive)

`GcPresetRow` 加 `builtin_key: Option<String>`,`#[serde(default,
skip_serializing_if = "Option::is_none")]`(GroupChatTaskConfig.preset_key 同款
additive 惯例,None 时 wire 字节不变,旧测试零改动)。

`create_group_chat_preset` 请求加可选 `builtin_key`(daemon 路由 struct
`#[serde(default)]`;Tauri 命令 `Option<String>` 参数,subagents.rs model_id 同款
先例)。update/delete 请求不变。

| cmd | 增量 |
|---|---|
| `create_group_chat_preset` | 请求 + `builtin_key?: string`(snake 顶层);响应 `GcPresetRow` 带回 `builtinKey` |
| `update_group_chat_preset` | 无(builtin_key 不可变) |
| `list` / `delete` | 无 |

命令名不变 → lib.rs / commands/mod.rs / transport CMD_TO_DOMAIN / routes-sync
守卫全部零改动。

## 3. 校验(commands 层单源,create 侧新增两臂)

`validate_preset_input` 形状不动(名称/描述/模型存在性/参与者/persona 全复用);
create 路径在调用它**之后**追加 builtin_key 专属校验:

1. `builtin_key` 非 None 时必须 ∈ `BUILTIN_PRESET_KEYS`(防脏值把覆盖行悬空)。
2. 同 key 已有覆盖行 → 400「内置预设「k」已有覆盖,请编辑现有覆盖行」
   (`SELECT id FROM group_chat_presets WHERE builtin_key = ?`)。
3. name 与内置 key 不撞的既有校验**对覆盖行照常生效**(display 名 ≠ 链接键,所以
   覆盖「arch」的行不能叫「arch」,必须起个管理面名字)。

## 4. 前端

### 4.1 store `mergedPresets` 原位顶替(核心机制)

```ts
// MergedGcPreset 加 overriddenBy?: string(覆盖行的行 id;UI 标记用,消费方逻辑零依赖)
for (const row of rows) {
  if (row.builtinKey) {
    // 覆盖行:原位顶替内置槽 —— key/name 保持内置 key(消费方 UI 形态不变),
    // 阵容字段换成行内容(UUID 借道 resolveModelRef byId 首趟,链路零新分支)。
    // key 不在内置集合(未来 JSON 删 key 的存量行)→ 跳过,不当用户档追加。
    if (out[row.builtinKey]) out[row.builtinKey] = { ...row 展开同用户档, key: row.builtinKey, builtin: true, name: row.builtinKey, overriddenBy: row.id };
    continue;
  }
  out[row.id] = { ...现状... };  // 普通用户行照旧追加
}
```

- **消费方(ScheduledTasksTab / GroupChatConfigModal)零改动**:`mergedPresets`
  的 key 集合不变(四个内置 key + 用户行 id),选中内置槽展开的就是覆盖后阵容;
  `preset_key: "arch"` 归位到覆盖后定义(B9 stale 比对自动吃覆盖版,AC3 天然成立)。
- 呈现策略:覆盖槽在下拉/卡片仍显示内置 key(name = key),不把覆盖行 display 名
  带进消费方——「已覆盖」标记只做在 Settings(管理面);消费方验收只关心阵容
  (AC1 口径)。

### 4.2 Settings tab:内置区从只读变可覆盖管理

- `overrideByKey = computed(Map<key, GcPresetRow>)`(从 store.rows 过滤
  `builtinKey` 非空);用户列表 `sortedRows` 过滤掉覆盖行(内置区原位管理,无双
  管理面)。
- 内置行三态:
  - 未覆盖:description 来自 JSON def;动作 = 「覆盖编辑」。
  - 已覆盖:chip「已覆盖」+ description/roster 来自覆盖行;动作 = 「编辑覆盖」+
    「恢复内置」(danger,ConfirmDialog 文案:恢复内置预设「k」,丢弃覆盖行回落
    源码 JSON 定义,已建任务不受影响——快照)。
- 「覆盖编辑」预填(无覆盖行时,自 JSON def):`moderatorModelId` / 参与者
  `modelId` = `resolveModelRef(models.models ?? [], ref) ?? ""`(全目录解析,禁用
  也回显——表单选项 = 启用 ∪ 当前值,既有模式;解析不出留空,表单校验逼用户
  重选,这正是修复场景);participants name/persona 直取 def;description 预填
  def.description;name 留空 + placeholder 说明「仅管理面显示名,不与内置档重名」。
  已有覆盖行时 = 普通 `openEdit(row)`。
- 表单态加 `overridingKey: string | null`;submit:有值且非编辑 →
  `store.create({ ...input, builtinKey: overridingKey })`,编辑态照旧 update。
- 校验:`validateForm` 零改动(name 撞内置 key 的镜像规则已覆盖覆盖行)。

### 4.3 store `create` 载荷

`GcPresetInput` 加 `builtinKey?: string | null`;`create()` invoke 透传
(`builtinKey: input.builtinKey ?? null`;顶层 camelCase → snake 由 transport 扳,
null 反序列化为 None)。

## 5. 兼容与回滚

- 旧 DB:幂等加列(全 NULL)+ IF NOT EXISTS 索引,零数据迁移;新库 CREATE TABLE
  直建带列。回滚 = DROP INDEX + 删列(SQLite 删列 3.35+ 支持,或弃列无害)。
- 旧 wire:GcPresetRow None 不序列化;create 不传 builtin_key = None,行为不变。
- `scripts/` 零改动:JSON 原样,M1/MCP 的镜像断言不受影响(它们看不到覆盖——
  P2 边界,PRD 非目标)。
- 快照语义:覆盖行编辑/删除同样不回溯已建任务;B9 stale 是唯一联动(提示重选)。

## 6. 测试设计

| 层 | 文件 | 内容 |
|---|---|---|
| db | `db/group_chat_presets_tests.rs` | builtin_key 往返(create 带值 list 带回)/ 两条 NULL 行共库名不撞(UNIQUE NULL 语义)/ 同 key 第二条覆盖被 UNIQUE 拒绝 |
| commands | `commands/tests_group_chat_presets.rs` | builtin_key 非法值 400 / 同 key 重复覆盖 400 / 合法覆盖 create→list(带 builtinKey)/ 覆盖行 update 不改 builtin_key / 覆盖行 name 撞内置 key 仍 400 |
| routes | `routes/group_chat_presets.rs` oneshot | 既有测试尾追加:snake 顶层 `builtin_key` create → 响应 camelCase `builtinKey`;同 key 二次 create → 400 |
| 前端 store | `stores/groupChatPresets.test.ts` | 覆盖行原位顶替(merged[arch] 用行阵容、key/name 仍是 arch、overriddenBy=行 id、无追加键)/ 普通行照旧追加 / 未知 builtinKey 跳过 |
| 前端 tab | `GroupChatPresetsTab.test.ts` | 覆盖编辑预填(模型名解析 UUID;缺失留空)/ create 带 builtinKey / 已覆盖态按钮与「恢复内置」流 / 覆盖行不出现在用户列表 |
| 消费方 | ScheduledTasksTab.test.ts | 覆盖档选中预填覆盖后阵容;preset_key=内置 key 的旧任务在覆盖后 ≠ 存档时 stale 提示亮(AC3) |
| 消费方 | GroupChatConfigModal.test.ts | 覆盖档卡片选中展开覆盖后 roster |

## 7. 明确不做(对齐 PRD 非目标)

跨视图「覆盖修复」快捷入口(砍,PRD 已记录理由)/ 内置档入库可编辑(否决)/
MCP+M1 可见性(P2)/ 自定义 persona(P3)/ overwrite 回溯。
