# Design: GCE-P2 引擎侧用户预设可见性(M1 + MCP 运行时拉取)

> 前置:daemon HTTP 面 P1/P1b 已就绪(`POST /api/v1/group_chat_presets/
> list_group_chat_presets`,body `{}`,响应 `Vec<GcPresetRow>` camelCase);引擎侧
> `api()` 助手 / MCP `realDeps` 注入 / 三趟模型解析全部现成(见
> `research/engine-visibility.md`,行号快照)。本任务 scripts/ 纯 JS。

## 1. 分层:纯函数(fail-loud)× 拉取封装(fail-open)× 消费点

```
group-chat-presets.json ──composePresets──▶ PRESETS(内置,模块期求值,烤进 bin)
                                               │
daemon HTTP list_group_chat_presets ──listUserPresets(base)──▶ rows(GcPresetRow[])
                                               │                    │
                        loadEffectivePresets(rowsProvider)◀─────────┘
                        catch → rows=[] + degraded(fail-open,stderr 警告)
                                               │
                        mergePresets(PRESETS, rows, presetsFile)   ← 纯函数,fail-loud
                                               │
                        {presets, degraded, detail} ─▶ resolveParticipants / moderator / 展示
```

两层分工(research §7c):**网络失败降级不抛错**(参照 ensureTranscript 先例),
**数据结构损坏照 throw**(参照 composePresets 防御)——拉取层吞掉的是
「daemon 不可达/HTTP 错」,吞不掉「行形状坏了」。

## 2. run.mjs 新增导出(引擎核心,MCP 经 import 复用)

### 2.1 `composePersonaMd(file, kind)`(从 composePresets 抽出)

```js
const persona = (kind) => { ... `${base}\n\n${common}` }   // 现有 :86-90 内联函数
// 抽为模块级导出,composePresets 与 mergePresets 共用——kind→persona_md
// 展开单源(脏 kind throw 同「缺 persona "${kind}"」文案)。
```

### 2.2 `mergePresets(builtin, rows, file)`(纯函数,合流核心)

输入:builtin = PRESETS(或其子集),rows = GcPresetRow[] wire 形状,
file = presetsFile(personas / persona_common 数据源)。
输出:合并 map,值形状 = PRESETS 条目超集:

```js
{
  description,                       // 行 description / 内置 def
  moderator_model,                   // 行 = UUID;内置 = JSON 名字(消费端 normalizeModelRef 统一解析)
  participants: [{name, model, persona_md}],   // 行的 persona kind 经 composePersonaMd 展开
  source: 'builtin' | 'user' | 'override',     // 展示标记(消费方逻辑零依赖)
  display_name,                      // 内置 = key;用户行 = row.name(管理面名)
  overridden_by,                     // 仅 override 行:行 id(UI/展示标记)
}
```

规则(镜像前端 stores/groupChatPresets.ts:128-150):

```js
const out = { ...builtin 深拷贝(source:'builtin', display_name:key) };
for (const row of rows) {
  if (row.builtinKey) {
    if (!out[row.builtinKey]) continue;            // 未知 builtinKey 脏行跳过(前端同款)
    out[row.builtinKey] = { ...行内容展开, source:'override',
      display_name: row.builtinKey, overridden_by: row.id };   // key/name 保持内置 key
    continue;
  }
  out[row.id] = { ...行内容展开, source:'user', display_name: row.name };  // key = 行 id
}
```

防御 throw(结构损坏,fail-loud):行缺 moderatorModelId / participants 非数组
或空 / participant 缺 name·modelId·persona / persona kind 不在 file.personas /
participants 重名(与 resolveParticipants 重名校验同文案风格)。

### 2.3 `listUserPresets(base)`(拉取封装)

```js
export async function listUserPresets(base) {
  return api(base, 'group_chat_presets/list_group_chat_presets');
}   // 同 listModels :461-463 形态;api() 未导出但同模块直用
```

### 2.4 `loadEffectivePresets(rowsProvider)`(降级层,测试可注入)

```js
export async function loadEffectivePresets(rowsProvider) {
  try {
    const rows = await rowsProvider();
    return { presets: mergePresets(PRESETS, rows, presetsFile), degraded: false };
  } catch (e) {
    return { presets: PRESETS, degraded: true, detail: e.message };   // fail-open
  }
}
```

- 入参是 provider 而非 base:M1 传 `() => listUserPresets(opt.base)`,MCP 传
  `() => deps.listPresets()`(deps 注入,mock 友好);单测传 throwing provider。
- 降级只回内置 PRESETS(不带 source 标记的原始形状——展示层对缺标记按 builtin
  处理,消费逻辑只读 participants/moderator_model,两形状兼容)。

### 2.5 `resolveParticipants` 增可选 `presets` 参数 + 三趟 key 解析

```js
export function resolveParticipants({ preset, participantsJson, set, presets }) {
  const table = presets || PRESETS;
  // 三趟:table[preset] 直配(内置 key / 用户行 id)
  //   → 用户行 display_name 精确 → display_name 忽略大小写(唯一命中才收,
  //   多命中报歧义——DB UNIQUE 保证 display_name 唯一,理论不可达,防御留)
  // miss 文案:`未知预设 "${preset}";可用:${内置 keys + 用户 display_name}(查看:presets 子命令 / list_presets)`
}
```

- 既有调用(不传 presets)零改动,现有单测不动。
- coreStart / M1 run 改传 loadEffectivePresets 结果。

## 3. M1 CLI 消费点(group-chat-run.mjs)

- **run()**:resolveParticipants(:562)之前插
  `const eff = await loadEffectivePresets(() => listUserPresets(opt.base))`;
  `eff.degraded` → stderr 一行警告「用户预设拉取失败(<detail>),降级内置四档」;
  `--preset` 解析失败且 degraded → 报错追加「(用户预设不可用:daemon 不可达)」。
  resolveParticipants / moderator 默认(:567)改吃 `eff.presets`。
  dry-run 分支在拉取之后 → 用户预设 dry-run 天然可用;help 文案「零网络」改
  「不建 session、不发 LLM(预设目录拉取失败降级内置四档)」。
- **cmdPresets() → cmdPresets(base) 异步化**:main 传 DEFAULT_BASE;内部
  loadEffectivePresets(daemon 不在 → 内置 + stderr 警告,不炸);输出三段:
  内置档(description + moderator + roster;override 行加「已覆盖(行 <id8>)」
  标记 + 阵容为行内容)/ 用户档(key = 行 id + display_name + 阵容)/ 既有
  覆盖语法 footer + 新增一行「用户预设存 daemon DB(Settings 管理),引用传
  行 id(UUID)或名称」。
- 退出码 / --quiet / 转录链路零改动。

## 4. MCP 消费点(group-chat-mcp.mjs)

### 4.1 realDeps + coreStart

- `realDeps` 加 `listPresets: () => listUserPresets(base)`(import from run.mjs)。
- `coreStart`:roster 解析前 `const eff = await loadEffectivePresets(() =>
  deps.listPresets())`;resolveParticipants 传 `presets: eff.presets`;moderator
  改 `eff.presets[preset]?.moderator_model`(经同一三趟解析后的定义);degraded
  且 preset miss → 报错文案追加降级提示。created_via / token_budget / ledger
  链路零改动。
- **mock 默认**:makeMockDeps 加 `listPresets: async () => []` → 既有单测
  (不关心预设)零扰动。

### 4.2 tools 面

- `buildToolShapes`:`preset: z.string().optional().describe('Participant preset:
  builtin review/fe_review/arch/retro, user preset UUID/name — see list_presets')`;
  新增 `list_presets: {}` shape。
- `TOOLS` 数组加 list_presets 条目(description:「List participant presets:
  builtin four + user presets (key=UUID) + overrides. Cheap metadata call.」);
  **start_discussion description 里四档阵容摘要句改为「Presets: see list_presets」
  抵扣新增工具的预算增量**(D3 description 克制约束)。
- `corePresets(deps)`:loadEffectivePresets → 数组视图
  `[{key, name, source, description, moderator, participants:[{name, model,
  persona}]}]`(persona 原样 kind;model 内置=名字/行=UUID,hint 注明)+
  `degraded` 标记 + hint(「引用传 key(UUID)或 name;start 时解析」)。
- handler 接线照 list_models(coreModels → textResult)。

### 4.3 wire 预算

实测(八工具 InMemoryTransport listTools)后定:目标保 3500(摘要句抵扣后
净增约 +100,实测 3403 基数上很可能贴线);超则升锁最小档(3700/3800),
四处同步:TOOLS_BUDGET_CHARS 注释(记实测值+mcp.test 断言自动跟常量)+
 smoke BUDGET + spec(DAEMON-API 或 scripts spec 提及处)。

### 4.4 动态 schema 否决记录

listTools 时拉 daemon 构建 enum:破 buildToolShapes 同步纯函数性质(无 daemon
单测/非 live 冒烟依赖)、daemon 两态结果漂移、宿主缓存 tools 快照会 stale。
改 string + 运行时校验(未知预设报错本就存在),枚举发现义务移交 list_presets。

## 5. 测试设计

| 文件 | 新增用例 |
|---|---|
| run.test.mjs | mergePresets 六臂:用户行追加(key=id、UUID 直塞、persona_md 展开)/ 覆盖行原位顶替(key/name=内置 key、overridden_by、roster=行内容)/ 未知 builtinKey 跳过 / 空行集内置不变 / 脏行 fail-loud(persona kind 缺 / participant 缺字段)/ 参与者重名 throw |
| run.test.mjs | resolveParticipants(presets=merged):内置 key / 用户 id / display_name 精确 / 忽略大小写 / miss 报可用清单;不传 presets 参数回归内置(既有用例即锁) |
| run.test.mjs | loadEffectivePresets:provider 正常 → merged;provider throw → {presets:PRESETS, degraded:true, detail} |
| mcp.test.mjs | coreStart 用户预设:key=id 建群 roster=行内容(UUID)、moderator=行 moderatorModelId、created_via=mcp 不变;listPresets throw + 内置 preset → 正常 + degraded 警告;listPresets throw + 用户 preset → 报错含降级提示;覆盖档吃到覆盖后阵容 |
| mcp.test.mjs | corePresets:mock rows → 合并视图形状(source 标记 / degraded=false);throw → 内置四档 + degraded:true |
| mcp.test.mjs | tools/list:八工具名;preset schema 为 string 型(JSON Schema type:'string',非 enum);wire 预算 ≤ 锁值(实测值断言进注释) |
| smoke(非 live) | tools/list 八工具名 + 预算;list_presets callTool:断言内置四 key 恒在 + 数组形状(daemon 两态皆确定性;不断言用户行) |

live 零 LLM 验证链(AC6,真 daemon、零 token):`presets` 子命令见用户行/覆盖标记 →
`run --dry-run --preset <用户档id>` 全链 → MCP `list_presets` callTool 返回用户行。

## 6. 兼容与回滚

- 旧调用面:resolveParticipants 不传 presets = 原行为;makeMockDeps 默认 []
  = 原行为;bin(烤 JSON)不重部署即获得运行时增量。
- daemon 老版本(无 group_chat_presets 路由,P1 之前):404 → loadEffectivePresets
  catch → 降级内置,与「daemon 不可达」同路径——无需版本探测。
- 回滚:revert scripts/ 三文件 + 测试即可,无 schema/wire 变更。

## 7. 明确不做(对齐 PRD Non-goals)

动态 tools/list schema(4.4 否决记录)/ daemon 内置档入库 / GUI 改动 /
定时 fire 路径 / 自定义 persona 文本(P3)。
