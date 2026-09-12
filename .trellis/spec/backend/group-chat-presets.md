<!-- Schema + IPC spec for group_chat_presets (GCE-P1, 2026-09-12; GCE-P1b override, 同日; GCE-P2 引擎合流, 同日) -->

# Group Chat Presets(GCE-P1 + P1b + P2, 2026-09-12)

> **Source**: `.trellis/tasks/archive/2026-09/09-12-gc-preset-settings`(P1 全套)+
> `.trellis/tasks/archive/2026-09/09-12-gc-preset-override`(P1b 覆盖层)+
> `09-12-gc-preset-engine-visibility`(P2 引擎侧合流);
> migration 在 `app/src-tauri/src/db/migrations/schema.rs` run_migrations 尾部,
> CRUD 在 `app/src-tauri/src/db/group_chat_presets.rs`,校验单源在
> `app/src-tauri/src/commands/group_chat_presets.rs::validate_preset_input`,
> 路由在 `app/src-tauri/src/daemon/routes/group_chat_presets.rs`;
> 引擎侧合流在 `scripts/group-chat-run.mjs`(mergePresets / loadEffectivePresets /
> lookupPreset,scripts 单测 `group-chat-run.test.mjs` + `group-chat-mcp.test.mjs`)。
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
  updated_at         TEXT NOT NULL,
  builtin_key        TEXT                         -- P1b:NULL = 普通行;值 ∈ 四内置 key = 覆盖行
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_group_chat_presets_builtin_key
  ON group_chat_presets(builtin_key);             -- SQLite UNIQUE 对 NULL 互不相撞
```

- DB:`list_group_chat_presets(pool) -> Vec<GcPresetRow>`(ORDER BY name)/ `get_…` /
  `create_…`(P1b 起多一个 `builtin_key: Option<&str>` 参数;列迁移走 columns.rs
  `add_group_chat_presets_column_if_missing` 幂等加列,CREATE TABLE 段带新列给新库,
  scheduled_tasks F2b 双路径先例)/ `update_…`(re-read 带回 created_at;**SET 清单
  不含 builtin_key——覆盖链接键创建时定死,update 不可改**)/ `delete_… -> bool`(幂等)。骨架抄
  `db/models.rs`(map_row 共用防列漂移);participants JSON↔Vec 封装在 db 层。
- IPC 四命令(Tauri 命令 = daemon 路由 1:1,domain `group_chat_presets`;命令名零新增,
  P1b 只给 create 加可选参数):
  `list_group_chat_presets` / `create_group_chat_presets`(P1b:+`builtinKey?: string`,
  顶层 camelCase → snake)/
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
- **覆盖层(P1b,2026-09-12)**:带 `builtin_key` 的行 = 内置档覆盖行,在
  mergedPresets 里**原位顶替**对应内置槽——key/name 保持内置 key、阵容字段换行内容
  (UUID 借道 byId 首趟,消费方零改动),附 `overriddenBy: 行id` 供 UI 标记;不追加
  新键。每 key 至多一条(commands create 前置查重 400 + UNIQUE 索引兜底)。覆盖行
  的 `name` 是纯管理面显示名,**仍不许撞内置 key**(display 名与链接键分离)。
  Settings 内置区「覆盖编辑」预填 JSON def(模型名经 resolveModelRef 全目录解析,
  缺失留空逼重选——正是修复场景);「恢复内置」= 删除覆盖行,回落 JSON 源码定义。
  快照语义照旧:覆盖编辑/删除不回溯已建任务;旧任务 `preset_key: "arch"` 归位到
  覆盖后定义(B9 stale 比对自动吃覆盖版)。
- **引擎合流(P2,2026-09-12,scripts/ 侧;Rust/前端零改动)**:M1 CLI 与 MCP
  运行时调 `list_group_chat_presets` 拉全部行,与内置 JSON 四档**客户端合并**:
  - `mergePresets(builtin, rows, file)` 纯函数**逐条镜像前端 mergedPresets 规则**
    (用户行追加 key=id / 覆盖行原位顶替 key·name=内置 key / 未知 builtinKey 脏行
    跳过);行的 persona **kind** 经 `composePersonaMd(file, kind)` 展开(数据源
    仍是 JSON 的 personas + persona_common——DB 不存 persona 文本);行 model 直接
    放 UUID(`normalizeModelRef` byId 首趟直收,零新分支)。值 = 内置条目超集,
    附 `source('builtin'|'user'|'override')` / `display_name` / `overridden_by`
    展示标记,消费方逻辑零依赖。
  - **降级两层分工(勿混)**:`loadEffectivePresets(rowsProvider)` 吞**拉取失败**
    (daemon 不可达 / 老版本路由 404·405)→ 返回内置 PRESETS + `degraded:true`
    + detail(fail-open;旧 daemon 实证:2026-09-12 对 pre-P1 daemon 冒烟
    degraded=true 四档兜底);`mergePresets` 对**行结构损坏**(缺字段/脏 persona
    kind/重名)照 throw(fail-loud,拉取层吞不掉数据脏)。
  - **preset 引用三趟解析** `lookupPreset(presets, ref)`(normalizeModelRef 同构):
    key 直配(内置 key / 用户行 id)→ `display_name` 精确 → 忽略大小写;DB 校验
    保证用户行 name UNIQUE 且不撞内置 key,歧义理论不可达(防御报错留)。miss 报
    可用清单(用户档带 key 前 8 位);**降级态 preset miss 的报错必须追加「用户
    预设不可用(daemon 拉取失败)」提示**——别让 daemon 不在伪装成预设不存在。
  - MCP:`start_discussion.preset` schema = `z.string()`(动态 enum 否决——
    buildToolShapes 是无 daemon 环境消费的同步纯函数;枚举发现义务移交
    `list_presets` 工具);`coreStart` / M1 `run` 都经 `loadEffectivePresets` +
    三趟解析取 roster 与 moderator;makeMockDeps 默认 `listPresets→[]`(既有
    单测零扰动)。wire 预算八工具实测 3678 → 锁 3800(四处同步:常量注释 /
    mcp.test / smoke BUDGET / deploy spec)。
  - **standalone bin 免重部署**:内置 JSON 仍静态 import 烤进 bin(内置档唯一
    来源不变),用户行/覆盖行运行时拉取——deploy 脚本与 entry 哨兵零改动。

## 4. Validation & Error Matrix

校验单源 `validate_preset_input`(commands 层;前端镜像只为即时反馈):

| 条件 | 错误 |
|---|---|
| name trim 空 / >40 chars | InvalidRequest(400) |
| name 与用户行或内置 key 大小写不敏感撞名(update 排除自身;**覆盖行照常生效**) | InvalidRequest |
| description >200 chars | InvalidRequest |
| moderator / 任一 participant 的 modelId 查 models 表不存在 | InvalidRequest(disabled **放行**——禁用是使用处反诊问题) |
| participants 不在 2..=3 | InvalidRequest |
| participant name trim 空 / >20 / 预设内重名 | InvalidRequest |
| persona ∉ 五 kind | InvalidRequest |
| create 的 builtin_key ∉ 四内置 key(P1b;空串同拒,不静默降级 None) | InvalidRequest |
| create 的 builtin_key 已有同 key 覆盖行(P1b) | InvalidRequest |
| update 目标 id 不存在 | InvalidRequest |
| delete 目标不存在 | Ok(幂等,沿 clear 先例) |

## 5. Good / Base / Bad Cases

- Good:Settings 建「我的评审团」(主持人 glm-5.3 UUID + 2 参与者 UUID + persona kind),
  定时表单下拉出现该档,选中按 UUID 预填,提交 config = UUID 快照 + `preset_key = 行 id`。
- Good(P1b):内置 review 的模型被删 → Settings 内置区「覆盖编辑」预填(缺失留空),
  重选现有模型存成 builtin_key="review" 的行;此后定时表单/建群弹窗选 review 展开的
  就是覆盖后阵容,旧任务 `preset_key: "review"` 的 stale 提示对覆盖后定义生效。
- Base:编辑该预设换模型 → 已建任务阵容**不变**(快照),任务编辑页 stale 提示亮起
  (B9:当前展开 ≠ 存档 config 时一行提示)。
- Bad:预设名取 `Arch`(撞内置 key,CI 拦);participants 只 1 人(拦);把用户预设的
  model 字段填模型名而非 UUID(绕过 byId 首趟,退化为名字匹配——改名断链回归);
  覆盖「arch」的行起名「arch」(display 名与链接键分离,仍拦)。

## 6. Tests Required

- db 冒烟(`db/group_chat_presets_tests.rs`):create 往返 / list 有序 / update 未知 id /
  delete 幂等 / UNIQUE 兜底;P1b 三件——覆盖行 builtin_key 往返、两条 NULL 行共存
  (UNIQUE NULL 互不相撞)、同 key 第二条覆盖被索引拒。
- commands 校验矩阵(`commands/tests_group_chat_presets.rs`):§4 逐条 + create→list→delete 闭环;
  P1b 五件——非法 key 400 / 同 key 重复覆盖 400 / 合法覆盖带回 / update 不改
  builtin_key / 覆盖行 name 撞内置 key 仍 400。
- 路由 wiring(`routes/group_chat_presets.rs` 尾部 oneshot):四路由 CRUD 闭环 + 撞内置 key 400
  (同时锁顶层 snake_case 请求形状——写错收到 422 是第一嫌疑);P1b 覆盖流
  (snake `builtin_key` → 响应 camelCase `builtinKey`;同 key 二次 400)。
- serde 兼容:`GroupChatTaskConfig` 旧 JSON(无 preset_key)反序列化 = None;
  `GcPresetRow` None 不序列化(wire additive,P1b)。
- 引擎合流(scripts/,node --test;P2):`group-chat-run.test.mjs` —— mergePresets
  六臂(追加/顶替/脏行跳过/persona 展开/fail-loud 四投掷/空行集)、lookupPreset
  三趟 + miss 清单、resolveParticipants(presets) 传参/缺省回落、
  loadEffectivePresets 正常/降级两臂;`group-chat-mcp.test.mjs` —— coreStart 用户档
  (by id/by name/覆盖档)、降级两臂(内置照常 / 用户档报错带提示)、corePresets
  合并视图与 degraded、八工具 + preset schema string + 预算实测;smoke 非 live
  八工具名 + list_presets 内置四 key 恒在(daemon 两态确定性)。
- 前端:store merged 单测(内置在前 / name 序 / UUID 借道 byId 直配——锁机制前提);
  P1b——覆盖行原位顶替(key/name = 内置 key、overriddenBy、无追加键)、普通行照旧、
  未知 builtinKey 跳过;tab 组件测试(mock transport 按 cmd 分发;P1b——覆盖编辑预填
  / create 带 builtinKey / 恢复内置 / 覆盖行不双列);两消费方"用户预设出现在选项 + 提交带
  preset_key";编辑态只改预算重交时 `preset_key` 保真(2026-09-12 check 抓过丢键缺陷);
  P1b——消费方(源码零改动)选中覆盖档预填覆盖阵容 + preset_key 指内置 key 的 stale
  提示对覆盖后定义生效。

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

### Wrong:MCP tools/list 动态拉 daemon 构建 preset enum

```js
const presets = await fetchPresets();          // listTools 时拉 daemon
preset: z.enum(Object.keys(presets))           // 动态 schema
```

破 buildToolShapes 同步纯函数性质(无 daemon 单测/非 live 冒烟依赖)、daemon 两态
结果漂移、宿主缓存 tools 快照会 stale。

#### Correct:schema 放宽 `z.string()` + describe 指向 list_presets,校验在
coreStart 运行时(未知预设报错本就存在);枚举发现义务移交 `list_presets` 工具
(GCE-P2 定案,scripts/group-chat-mcp.mjs buildToolShapes)。

### Wrong:把「拉取失败」和「数据损坏」塞进同一层降级

```js
catch (e) { return PRESETS; }   // 连行结构损坏也吞 → 脏数据静默变成「没有用户预设」
```

#### Correct:两层分工——`loadEffectivePresets` 只吞 fetch/HTTP 失败(fail-open
降级内置 + degraded 标记);`mergePresets` 对行结构损坏照 throw(fail-loud,
composePresets 防御同款)。降级态 preset miss 报错追加「用户预设不可用」提示。

## 边界(P2 已收,2026-09-12)

引擎侧(M1 CLI / MCP server / standalone bin)已运行时拉取消费用户预设与覆盖行
(降级 fail-open;bin 免重部署)。剩余不做:自定义 persona 文本(P3)、overwrite
回溯(已评估否决)不在计划内;daemon 侧把内置档入库仍不做(内置 JSON 单源)。
