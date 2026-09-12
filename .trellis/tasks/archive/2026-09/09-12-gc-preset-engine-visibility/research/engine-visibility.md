# GCE-P2 前期调研:M1/MCP 引擎侧预设运行时拉取(2026-09-12)

> 来源:Explore 子代理全量扫描(路径:行号为当时快照)+ 主会话复核。
> 结论先行:**daemon HTTP 面 P1/P1b 已全部就绪,本任务 = scripts/ 纯 JS 侧改造,
> Rust / 前端 / deploy 脚本零改动。**

## 1. 引擎侧现状:预设加载与消费点

- 加载点 `scripts/group-chat-run.mjs:38` **静态 JSON import**(非 fs 读取)。
  :31-37 注释写明动机:**bun --compile standalone bin 只内嵌模块图,readFileSync
  旁路文件不会被打包**——deploy 面(deploy.mjs)依赖此行为。**内置四档继续烤进
  bin,运行时拉取的只是用户行/覆盖行增量 → bin 免重部署天然成立。**
- `composePresets(file)` run.mjs:83-105(导出纯函数):JSON → 运行时形状,
  persona kind 展开为 `persona_md = base + "\n\n" + persona_common`(:86-90)。
  防御 throw:顶层非 object / 缺 persona / presets 空 / 缺 moderator_model
  或 participants(:84-96)。
- `export const PRESETS = composePresets(presetsFile)` :107,模块加载期求值。
- 消费点:
  - `resolveParticipants({preset, participantsJson, set})` :120-150,**同步读模块
    全局 PRESETS**;preset miss :126-128 报「未知预设 + 可用清单(查 presets 子命令)」。
  - M1 `run`:resolveParticipants 在 :562(先于 dry-run 分支与 checkDaemon),
    moderator 默认 :567 `opt.moderatorModel || PRESETS[opt.preset]?.moderator_model`。
  - `--dry-run` :576-589「纯静态模板,零网络」——但 resolveParticipants 已在
    前面跑了,内置 preset 校验本来就同步发生。
  - `presets` 子命令 `cmdPresets()` :750-765,同步、零 daemon(main :812 不传 base)。
  - MCP `coreStart` mcp.mjs:121-127:resolveParticipants + `PRESETS[preset]?.
    moderator_model`,miss :127 报未知预设。
  - **MCP tools/list 硬编码**:`buildToolShapes` mcp.mjs:361
    `preset: z.enum(['review','fe_review','arch','retro'])`(全链路唯一硬编码
    预设清单);start_discussion description :384 硬编码四档阵容摘要。

## 2. daemon HTTP 面(已就绪,零新增)

- 路由注册 `app/src-tauri/src/daemon/routes/group_chat_presets.rs:99-106`,
  挂载 routes/mod.rs:122-125 → **`POST /api/v1/group_chat_presets/
  list_group_chat_presets`,body `{}`,响应 `Vec<GcPresetRow>`**(GUI Thin 模式
  同一条 HTTP 面,transport http.ts:115-118 CMD_TO_DOMAIN 同域)。
- `GcPresetRow` wire(db/group_chat_presets.rs:54-72,serde camelCase):
  `{id, name, description, moderatorModelId, participants:[{name, modelId,
  persona}], createdAt, updatedAt, builtinKey?}`——普通行**缺 builtinKey 键**
  (skip_serializing_if None),覆盖行 `builtinKey ∈ {review,fe_review,arch,retro}`。
- **moderator 是 model UUID**(soft FK models.id);participants 的 persona 是
  **五种 kind**(arch/product/backend/frontend/outsider,create 侧白名单校验),
  **不是完整 persona 文本**——引擎合流需自行 kind→persona_md 展开,数据源
  personas + persona_common 仍在 JSON(前端同款组装 utils/groupChatPresets.ts
  :41-43 composePersonaMd)。
- 校验矩阵(commands 层单源):用户行 name UNIQUE + 不撞内置 key(大小写不敏感)
  → **内置 key 与用户 name 无撞名空间,三趟解析(id → name 精确 → name 忽略
  大小写)无歧义**;participants 2..=3;persona ∈ 五 kind。

## 3. 引擎侧 daemon HTTP 客户端(现成)

- 核心助手 `api(base, route, ...)` run.mjs:428-442(**模块私有未导出**),
  daemon 不可达翻译成「先确认 daemon 在跑(scripts/daemon.sh)」;域封装全 POST。
  新增拉取 = 模块内加 `export async function listUserPresets(base)` 一行级封装
  (同 listModels :461-463 形态)。
- MCP `realDeps(overrides)` mcp.mjs:90-105 把函数绑 base 构成可注入 deps;
  测试注入点 `makeMockDeps` mcp.test.mjs:28-43。**加 listPresets 键即可**。
- 模型解析 `normalizeModelRef` run.mjs:255-269 三趟(UUID byId → modelName/
  displayName 精确 → 大小写不敏感),`validateModelRefs` :272-277 全员归一 UUID。
  用户预设存 UUID → byId 首趟直收,零新分支。

## 4. 测试布局与预算锁

- run.test.mjs::214-245 「PRESETS 单一事实源」锁(JSON 唯一事实源 + 四 key 顺序
  + 阵容 + persona_md 组装确定性)——内置档形态锚,继续保留。:21-33
  resolveParticipants 用例(含 preset miss 断言 :32)。
- mcp.test.mjs::66-68 enum 与 Object.keys(PRESETS) 对齐(改造点);:106-154
  coreStart 全链 + 错误路径;**wire 预算锁 `TOOLS_BUDGET_CHARS = 3500`**
  (mcp.mjs:352,注释含历次实测:09-11 加 list_models 后 3403;单测 :434-441
  用真 SDK InMemoryTransport 实测 ≤ 锁值且 ≥1200;smoke `BUDGET` 常量
  smoke.mjs:23 需手动同步)。**升锁纪律:常量注释 + AC4 断言 + smoke BUDGET +
  spec 四处同步。**
- 非 live 冒烟 smoke.mjs:spawn → tools/list 断言 **7 工具名**(:46-52)→
  discussion_status 错误链(daemon 两态皆过)。加 list_presets 后改 8;
  断言「内置四 key 恒在」是 daemon 两态皆确定性的口径。

## 5. standalone bin / deploy 面

- deploy.mjs `buildBin` :155-163 bun compile;presets JSON 经静态 import 进
  模块图已烤进 bin;`EVERLASTING_BASE` bin 同生效(entry :24)→ 运行时拉取
  base 语义免费继承,**deploy 脚本零改动**。
- entry 哨兵:standalone-entry.mjs:15 argv[1] 赋值必须在动态 import 之前
  ——本任务不动 entry,新逻辑全在引擎模块内,天然合规。

## 6. 合流语义基准(前端 mergedPresets 镜像)

前端 stores/groupChatPresets.ts:128-150:用户行追加(`out[row.id]`,UUID 直塞
moderator_model/model 字段借道 resolveModelRef byId 首趟);**覆盖行原位顶替
内置槽**(key/name 保持内置 key,阵容换行内容,`overriddenBy` 仅 UI 标记);
key 不在内置集合的脏行**跳过不当用户档**(:133-134);每 key 至多一条覆盖行
(UNIQUE 索引)。引擎侧照此镜像;store 头注 :27「M1/MCP 仍只认 JSON(P2 边界)」
即本任务要收的句。

## 7. 风险与注意

- (a) MCP z.enum → z.string:buildToolShapes 是同步纯函数、被无 daemon 单测
  消费 → **不做动态 schema**(listTools 时拉 daemon 会破同步性与冒烟确定性),
  放宽为 string + 运行时校验(coreStart 未知预设报错已存在)。
- (b) M1 --dry-run 语义:「零网络」收窄为「不建 session、不发 LLM」,本地
  daemon 元数据拉取允许、失败降级内置四档(help 文案同步)。
- (c) fetch 失败降级 ≠ 数据损坏降级:拉取失败 → 空行集 + degraded 标记(fail-open
  参照 ensureTranscript mcp.mjs:170-198 先例);merge 纯函数对脏行结构错误
  **照 throw**(fail-loud 参照 composePresets),两层分工。
- (d) mock deps 默认 listPresets → []:既有 coreStart 单测零扰动。
