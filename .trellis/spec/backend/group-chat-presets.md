<!-- Schema + IPC spec for group_chat_presets (GCE-P1, 2026-09-12) -->

# Group Chat Presets(GCE-P1, 2026-09-12)

> **Source**: `.trellis/tasks/09-12-gc-preset-settings`(PRD/design/implement 全套);
> migration 在 `app/src-tauri/src/db/migrations/schema.rs` run_migrations 尾部,
> CRUD 在 `app/src-tauri/src/db/group_chat_presets.rs`,校验单源在
> `app/src-tauri/src/commands/group_chat_presets.rs::validate_preset_input`,
> 路由在 `app/src-tauri/src/daemon/routes/group_chat_presets.rs`。
>
> **Cross-references**: [database-guidelines.md](./database-guidelines.md)(soft-FK / CRUD 约定)、
> [scheduled-tasks.md](./scheduled-tasks.md)(GroupChatTaskConfig / 快照语义)、
> [daemon-server.md](./daemon-server.md)(薄壳路由 / 顶层 snake_case)。

## 1. Scope / Trigger

GCE-P1(2026-09-12)落地「用户群聊预设」:内置四档(review / fe_review / arch / retro)仍是
`scripts/group-chat-presets.json` 单源(scripts 侧消费,只读),用户预设存 DB、Settings
「群聊预设」页 CRUD、前端两消费方(ScheduledTasksTab + GroupChatConfigModal)合并展示。
触发 code-spec 深度的原因:新表 + 新 IPC 命令 + 跨层 wire 契约。

**动机(7773b927 教训)**:预设按模型**名**引用目录,目录改名即断链;且那次修复漏扫了
前端测试 mock,主线 8 个用例红了两轮才发现。用户侧新实体改存 **UUID 引用**,从构造上
消灭改名断链。

## 2. Signatures

```sql
CREATE TABLE IF NOT EXISTS group_chat_presets (
  id                 TEXT NOT NULL PRIMARY KEY,   -- Uuid::new_v4()
  name               TEXT NOT NULL UNIQUE,
  description        TEXT NOT NULL DEFAULT '',
  moderator_model_id TEXT NOT NULL,               -- soft FK → models.id(无约束)
  participants       TEXT NOT NULL,               -- JSON:[{"name","modelId","persona"}] camelCase 键
  created_at         TEXT NOT NULL,
  updated_at         TEXT NOT NULL
)
```

- DB:`list_group_chat_presets(pool) -> Vec<GcPresetRow>`(ORDER BY name)/ `get_…` /
  `create_…` / `update_…`(re-read 带回 created_at)/ `delete_… -> bool`(幂等)。骨架抄
  `db/models.rs`(map_row 共用防列漂移);participants JSON↔Vec 封装在 db 层。
- IPC 四命令(Tauri 命令 = daemon 路由 1:1,domain `group_chat_presets`):
  `list_group_chat_presets` / `create_group_chat_presets` /
  `update_group_chat_presets` / `delete_group_chat_presets`。
- `GroupChatTaskConfig` 增量字段:`preset_key: Option<String>` +
  `#[serde(default, skip_serializing_if = "Option::is_none")]`(additive,旧行缺键 = None)。

## 3. Contracts

- **请求**:前端 `transport.invoke` 顶层 camelCase(`moderatorModelId` 等,httpTransport
  `transformArgsTopLevel` 只扳**顶层**为 snake);**嵌套** participants 元素 camelCase
  `{name, modelId, persona}`(嵌套不转换,Rust `GcPresetParticipant` 带 rename_all)。
- **响应** `GcPresetRow`:camelCase `{id, name, description, moderatorModelId,
  participants:[{name, modelId, persona}], createdAt, updatedAt}`(routes oneshot 测试锁形状)。
- **快照语义(定案)**:预设编辑/删除**不回溯**已建定时任务;任务 config 永远是提交时
  展开的 UUID 阵容;`preset_key` 纯记录出处(fire 路径零读取),「应用最新预设」只经
  用户显式重选。
- **前端 mergedPresets**(`stores/groupChatPresets.ts`):`Record<key, GcPresetDef &
  {key, name, builtin}>`;内置 JSON 声明序在前,用户行按 name 字典序;**用户行的
  moderator_model / model 字段直接放 UUID** —— `utils/groupChatPresets.ts::
  resolveModelRef` 首趟即 byId 精确匹配,UUID 借道既有解析/预填/警告链路,**不写新分支**。
- 内置 key 集合 `{review, fe_review, arch, retro}` 与 persona 五 kind
  `{arch, product, backend, frontend, outsider}` 在 Rust 侧硬编码
  (`BUILTIN_PRESET_KEYS` / `PERSONA_KINDS`),**同步义务指向 scripts/group-chat-presets.json**
  ——改 JSON 阵容/加 persona 时必须同步两处。

## 4. Validation & Error Matrix

校验单源 `validate_preset_input`(commands 层;前端镜像只为即时反馈):

| 条件 | 错误 |
|---|---|
| name trim 空 / >40 chars | InvalidRequest(400) |
| name 与用户行或内置 key 大小写不敏感撞名(update 排除自身) | InvalidRequest |
| description >200 chars | InvalidRequest |
| moderator / 任一 participant 的 modelId 查 models 表不存在 | InvalidRequest(disabled **放行**——禁用是使用处反诊问题) |
| participants 不在 2..=3 | InvalidRequest |
| participant name trim 空 / >20 / 预设内重名 | InvalidRequest |
| persona ∉ 五 kind | InvalidRequest |
| update 目标 id 不存在 | InvalidRequest |
| delete 目标不存在 | Ok(幂等,沿 clear 先例) |

## 5. Good / Base / Bad Cases

- Good:Settings 建「我的评审团」(主持人 glm-5.3 UUID + 2 参与者 UUID + persona kind),
  定时表单下拉出现该档,选中按 UUID 预填,提交 config = UUID 快照 + `preset_key = 行 id`。
- Base:编辑该预设换模型 → 已建任务阵容**不变**(快照),任务编辑页 stale 提示亮起
  (B9:当前展开 ≠ 存档 config 时一行提示)。
- Bad:预设名取 `Arch`(撞内置 key,CI 拦);participants 只 1 人(拦);把用户预设的
  model 字段填模型名而非 UUID(绕过 byId 首趟,退化为名字匹配——改名断链回归)。

## 6. Tests Required

- db 冒烟(`db/group_chat_presets_tests.rs`):create 往返 / list 有序 / update 未知 id /
  delete 幂等 / UNIQUE 兜底。
- commands 校验矩阵(`commands/tests_group_chat_presets.rs`):§4 逐条 + create→list→delete 闭环。
- 路由 wiring(`routes/group_chat_presets.rs` 尾部 oneshot):四路由 CRUD 闭环 + 撞内置 key 400
  (同时锁顶层 snake_case 请求形状——写错收到 422 是第一嫌疑)。
- serde 兼容:`GroupChatTaskConfig` 旧 JSON(无 preset_key)反序列化 = None。
- 前端:store merged 单测(内置在前 / name 序 / UUID 借道 byId 直配——锁机制前提);
  tab 组件测试(mock transport 按 cmd 分发);两消费方"用户预设出现在选项 + 提交带
  preset_key";编辑态只改预算重交时 `preset_key` 保真(2026-09-12 check 抓过丢键缺陷)。

## 7. Wrong vs Correct

### Wrong:用户侧新实体按名字引用模型目录

```ts
// 预设 model 字段放目录名(内置 JSON 的跨机器惯例,别带到用户数据)
{ model: "deepseek-flash" }  // 目录改名 → 断链(7773b927 实证)
```

#### Correct:本机用户数据放 UUID,展示层才解析

```ts
{ model: row.moderatorModelId }  // UUID 经 resolveModelRef byId 首趟直配;改名不断链
```

### Wrong:为用户预设另写一套解析/预填分支

#### Correct:UUID 塞进 GcPresetDef.model 字段,复用 resolveModelRef +
选中展开 + 禁用/缺失警告整条既有链路(零新分支,store 单测锁此机制)。

### Wrong:改了模型目录名只扫 DB + 预设 JSON

#### Correct:同轮扫**前端测试 mock 目录**(ScheduledTasksTab.test.ts /
GroupChatConfigModal.test.ts 的 MODELS fixture)——7773b927 漏扫导致主线 8 用例
红了两天才被 GCE-P1 顺带修复(2026-09-12 worktree 实证)。

## 边界(P2 未做)

MCP server / M1 CLI **暂只认内置四档**(standalone bin 烤 JSON);用户预设可见性是
P2(引擎运行时依赖 daemon HTTP,加运行时拉取即可,bin 免重部署)。自定义 persona
文本(P3)、overwrite 语义(已评估否决)不在计划内。
