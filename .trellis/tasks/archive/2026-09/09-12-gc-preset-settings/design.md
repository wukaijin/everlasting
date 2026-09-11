# Design:群聊预设 Settings 可编辑/可新增(GCE-P1)

> 先例骨架:DB CRUD 抄 `db/models.rs`(UUID 主键全 CRUD),薄壳路由抄 `routes/subagents.rs`,Settings tab 抄 `SubagentsTab.vue`。完整触点清单见 `research/crud-precedent-chain.md` §9。

## 1. 数据模型

### 1.1 新表 `group_chat_presets`

```sql
CREATE TABLE IF NOT EXISTS group_chat_presets (
  id                 TEXT NOT NULL PRIMARY KEY,   -- Uuid::new_v4()(models.rs 同款)
  name               TEXT NOT NULL UNIQUE,        -- 显示名,用户可改
  description        TEXT NOT NULL DEFAULT '',
  moderator_model_id TEXT NOT NULL,               -- soft FK → models.id(不建约束,沿 subagent_model_overrides 惯例)
  participants       TEXT NOT NULL,               -- JSON 数组:[{"name","model_id","persona"}](scheduled_tasks.config 存 JSON 数组同款)
  created_at         TEXT NOT NULL,
  updated_at         TEXT NOT NULL
)
```

- 建表加在 `db/migrations/schema.rs::run_migrations` 序列尾部(幂等 CREATE TABLE IF NOT EXISTS,无数据迁移)。
- `moderator_model_id` / `participants[].model_id` 存 **UUID**(本机稳定;改名不断链)。模型被删不级联——使用处已有 catalog 预检/反诊兜底,保存时校验存在即可。
- `participants` 用 JSON 列而非子表:`scheduled_tasks.config`、session metadata 都是这个形态,查询面没有"按参与者查预设"的需求。

### 1.2 `GroupChatTaskConfig` 增量(db/scheduled_tasks.rs:65)

- 新增 `preset_key: Option<String>`,`#[serde(default)]` 兼容旧行(缺键 = None)。
- fire 路径(scheduler/mod.rs)**零改动零读取**——纯展示/未来留门字段。编辑任务回显时前端可读它显示"来自预设 X"。

## 2. wire 契约(camelCase,沿 AppCommandError 错误形状)

Row(`GcPresetRow`):

```jsonc
{
  "id": "uuid",
  "name": "我的评审团",
  "description": "",
  "moderatorModelId": "uuid",
  "participants": [{ "name": "架构", "modelId": "uuid", "persona": "arch" }],
  "createdAt": "RFC3339", "updatedAt": "RFC3339"
}
```

命令四条(`POST /api/v1/group_chat_presets/<cmd>`,Tauri 命令 1:1 同名):

| cmd | 请求 | 响应 |
|---|---|---|
| `list_group_chat_presets` | `{}` | `Vec<GcPresetRow>`(ORDER BY name,稳定序) |
| `create_group_chat_preset` | `{name, description, moderatorModelId, participants}` | `GcPresetRow` |
| `update_group_chat_preset` | `{id, ...同上}` | `GcPresetRow`(不存在 → InvalidRequest) |
| `delete_group_chat_preset` | `{id}` | `{ok: true}`(不存在也 ok,幂等删除沿 clear 先例) |

## 3. 校验(单一事实源在 commands 层 `*_inner`)

1. `name`:trim 非空、≤40 字符;与其它用户行 **大小写不敏感** 唯一(update 时排除自身);与内置 key `{review, fe_review, arch, retro}` **大小写不敏感** 不撞(Rust 侧硬编码四 key + 注释指向 scripts/group-chat-presets.json 同步义务)。
2. `description` ≤200 字符(可空串)。
3. `moderatorModelId` 及全部 `participants[].modelId`:`db::get_model` 存在(**允许 disabled**——禁用是使用处问题,保存不拦)。
4. `participants`:2..=3 条;每条 `name` trim 非空、≤20 字符、预设内唯一;`persona ∈ {arch, product, backend, frontend, outsider}`(硬编码五 kind,同 1 的同步注释)。
5. 错误走 `AppCommandError` InvalidRequest(→ HTTP 400),message 中文可读(前端直接 toast)。

## 4. 前端架构

### 4.1 关键设计:merged 形状复用 `GcPresetDef`,UUID 借道 `resolveModelRef` 零改动解析

`utils/groupChatPresets.ts` 的 `resolveModelRef` 第一趟就是 UUID 精确匹配(`byId`)。因此用户预设的 model 引用(UUID)塞进 `GcPresetDef.model` 字段后,**既有解析/预填/警告链路逐字复用**,无需为用户预设写新解析分支:

```ts
// stores/groupChatPresets.ts —— 合并视图
// Record<key, GcPresetDef & { key: string; builtin: boolean }>
//   内置:JSON 声明序,model 字段 = 名字(现状不动)
//   用户:name 字典序追加,model 字段 = UUID
// key:内置 = JSON key;用户 = 行 id(UUID,与内置 key 不可能撞)
```

- 展示名:内置用 key(现状);用户用 `name` + 标记(如「自定义」徽标或后缀),下拉/卡片区可区分。
- `composePersonaMd(kind)` 两边通用(persona kind 同源)。

### 4.2 pinia store `stores/groupChatPresets.ts`

- state:`rows`、`loaded`、行级 spinner;actions:`load()` / `create(payload)` / `update(id, payload)` / `remove(id)`,全走 `transport.invoke`,错误 `extractErrorMessage` 重抛。
- getter:`mergedPresets`(§4.1 形状)。
- 加载时机:Settings tab onMounted、ScheduledTasksTab refresh()、GroupChatConfigModal open 时(未 loaded 才拉,幂等)。

### 4.3 Settings tab `GroupChatPresetsTab.vue`

- 布局:内置四档只读压缩列表(名称+描述+「内置」徽标)→ 用户预设列表(编辑/删除)→ 新增/编辑表单。
- 表单:名称、描述、主持人 Select(选项 = `models.enabledModels` ∪ 当前值,沿 SubagentsTab rowModelOptions 模式——禁用模型本行回显不泄漏到别行)、参与者 2-3 行(名字 input + 模型 Select + persona Select 五档)、加减按钮(2 下限禁删 / 3 上限隐藏加,沿 GroupChatConfigModal D5 边界)。
- 前端预校验镜像 §3 规则(即时反馈);服务端仍是事实源。
- reka-ui Select(键盘打开/teleport 断言模式见 SubagentsTabModelOptions.test.ts)。

### 4.4 消费方改造(最小侵入)

- `ScheduledTasksTab.vue`:`GC_PRESET_OPTIONS` 改从 store.mergedPresets 构建;`k in GC_PRESETS.presets` 判定改 merged;**提交 config 追加 `presetKey: form.gcpreset`(选了预设时)**,其余提交链路不动。
- `GroupChatConfigModal.vue`:`gcPresetEntries` 改 merged;`PRESET_CUSTOM` 哨兵与既有交互不动。
- 两处 snapshot 语义文案不动(「重新展开并覆盖存档配置」)。

### 4.5 transport

`transport/http.ts` `CMD_TO_DOMAIN` 加 4 行(domain `group_chat_presets`)——`http.routes-sync.test.ts` 守卫强制双向一致。

## 5. 兼容与回滚

- 旧 DB:CREATE TABLE IF NOT EXISTS + serde default,零迁移风险;回滚 = 删表(用户数据可弃)。
- 旧任务行:无 `preset_key` → None,行为不变。
- scripts/ 零改动:内置 JSON 原样,run.test/mcp.test 镜像断言不受影响。
- MCP/M1(P2)看不到用户预设——已知边界,PRD 非目标。

## 6. 测试设计

| 层 | 文件 | 内容 |
|---|---|---|
| db | `db/group_chat_presets_tests.rs` | 内存池 CRUD 冒烟(subagent_overrides_tests 模式):create 往返 / list 有序 / update 改字段 / delete 幂等 / name UNIQUE 冲突报错 |
| commands | commands 层测试 | 校验矩阵:撞内置 key(CI)、用户重名(CI)、模型不存在、参与者 1/4 条、名字重名/空、persona 非法、update 不存在 |
| routes | `routes/group_chat_presets.rs` `#[cfg(test)]` | oneshot wiring:4 条路由 POST 通(models.rs projects.rs 模式,mock 或最小 state) |
| scheduler | 既有测试跑绿 | `preset_key` serde default 兼容旧 JSON + fire 忽略 |
| 前端单测 | `stores/groupChatPresets.test.ts` 或 utils 合并单测 | merged:内置在前用户按 name 序、UUID 引用进 model 字段、key 不撞 |
| 前端组件 | `GroupChatPresetsTab.test.ts` | mock transport 按 cmd 分发;建/存/删流;校验提示;内置只读 |
| 前端既有 | ScheduledTasksTab.test.ts / GroupChatConfigModal.test.ts | 补用户预设出现在选项 + presetKey 提交(按现有 mock 模式扩展) |
| 守卫 | http.routes-sync.test.ts | 自动覆盖,勿漏 CMD_TO_DOMAIN |

## 7. 明确不做(对齐 PRD 非目标)

overwrite 语义 / MCP+CLI 可见性(P2)/ 自定义 persona 文本(P3)/ stale 提示(可砍尾巴)。
