# Design — subagent 多模型 C:per-agent 默认模型 UI + builtin DB override + 写回 frontmatter

> 配套 `prd.md`。本文件只讲技术设计;执行清单见 `implement.md`。

## 1. 已敲定决策(来自 brainstorm)

| 维度 | 取值 | 理由 |
|---|---|---|
| **优先级** | `DB override > frontmatter > parent` | UI 全局偏好优先,覆盖文件声明。一处配置即生效,直觉。 |
| **DB 作用域** | **全局** `agent_name → model_id` | builtin / user 级 agent 本就全局;schema 最简;无需 project 归属特判。 |
| **范围** | **完整 C** | builtin DB override + user/project 写回 frontmatter + UI 展示所有 subagent。 |

边界推论(由优先级导出,非新决策):
- builtin(researcher / general-purpose)无 frontmatter 文件 → **唯一**可配置入口是 DB override。
- user / project agent 有文件 → UI 改它时**写回 frontmatter**(不写 DB)。
- 因此 DB 表实际只承载 builtin name 的记录;但 resolve 仍按全局 name 查 DB,若用户用同名 user agent 覆盖 builtin 且 DB 有同名 override → DB 盖 frontmatter(符合"DB > frontmatter")。

## 2. 优先级链与接入点(核心)

**不动** `resolve_worker_provider`(`dispatch.rs:1189`,纯函数,catalog hit/miss → provider/ctx/display)。只在它的 caller `run_subagent` 之前**新增一个前置解析步骤** `resolve_final_model`,把"DB override / frontmatter"两路收敛成单个 `Option<model_id>`,再原样喂给 `resolve_worker_provider`:

```
final_model =
    get_subagent_model_override(db, def.name)   // ① DB override(全局)
    .or(def.model.clone())                       // ② frontmatter(覆盖层声明)
                                                // ③ None → resolve_worker_provider 内部 inherit parent
```

`run_subagent` 现有调用点(`dispatch.rs:549-562`,catalog read lock 段)改一行:`resolve_worker_provider(def.model.as_deref(), …)` → `resolve_worker_provider(final_model.as_deref(), …)`。

收益:`resolve_worker_provider` 的 6 个现有单测零改动(A 任务 AC1-5 不回归);新优先级逻辑独立可测。

## 3. DB 层

### 3.1 新表(全局作用域)

```sql
CREATE TABLE IF NOT EXISTS subagent_model_overrides (
  agent_name TEXT NOT NULL PRIMARY KEY,   -- 稳定 key:researcher / general-purpose / 用户 agent name
  model_id   TEXT NOT NULL,               -- → models.id(catalog key;逻辑 FK,不强制)
  updated_at TEXT NOT NULL
);
```

走 `migrations.rs` 既有 `CREATE TABLE IF NOT EXISTS` 幂等模式(同 `models` / `subagent_runs` 首建)。

### 3.2 db 函数(新模块 `db/subagent_overrides.rs`,沿用 CRUD 分散到子模块惯例)

- `get_subagent_model_override(db, name) -> Result<Option<String>, sqlx::Error>`
- `set_subagent_model_override(db, name, model_id) -> Result<(), sqlx::Error>`(`INSERT … ON CONFLICT(agent_name) DO UPDATE` UPSERT)
- `clear_subagent_model_override(db, name) -> Result<(), sqlx::Error>`(`DELETE`,UI 设回 inherit 时)
- `list_subagent_model_overrides(db) -> Result<Vec<(String, String)>, sqlx::Error>`(UI 一次性加载)

### 3.3 model 删除时的残留

`models` 行被删 → `subagent_model_overrides.model_id` 残留指向失效 id。**不级联清理**:
- resolve 时 catalog miss → `resolve_worker_provider` 已有 `warn!` + parent fallback 路径天然兜底。
- UI 加载时把失效 override 标红/提示(可选,MVP 至少不让下拉崩)。
- 记为 follow-up:删 model 时连带 clear 同名 override(低优)。

## 4. frontmatter 写回(user / project agent)

### 4.1 策略:行级编辑,保留原字节

不引 YAML 序列化库,不 round-trip 整个 frontmatter(会丢注释 / 打乱顺序 / 改引号风格)。改为**行级编辑**:读原文件文本 → 定位 frontmatter 块(`---` fence 之间)→ 仅改 `model:` 那一行 → 写回。body 与其余 frontmatter 行原样保留。

### 4.2 `write_frontmatter_model(path, model_id: Option<String>) -> io::Result<()>`

逻辑:
1. 读原文件。**user/project agent 经 loader 加载必有 frontmatter fence**(loader 拒绝无 `name` 的文件,见 `parse_frontmatter`),故正常写回路径文件必有 fence。若读到无 fence 文件(竞态:外部刚改坏)→ **返错**(不隐式补 fence,避免魔法式改写用户文件结构);`model_id=None` 且无 `model:` 行 → noop 直接返回。
2. fence 内逐行扫:
   - 命中 `model:` 行(已声明)→ `model_id=Some` 替换值;`model_id=None` 删该行。
   - 未命中 → `model_id=Some` 在 frontmatter **首行(fence 之后)**插入 `model: <id>`;`model_id=None` noop。
3. 原子写:写 `path.tmp` → rename(防中途崩导致 agent 文件损坏;与 `files.rs` 既有原子写惯例一致)。

### 4.3 触发 cache 失效

写文件改 mtime → `SubagentCache` 的 mtime-fenced 扫描在**下一 chat turn** 自动重读(无需显式 reload 命令)。DB override 不经文件 cache,resolve 实时查 DB,立即生效。

## 5. 新 IPC(`commands/subagent_runs.rs` 旁或新 `commands/subagents.rs`)

### 5.1 `list_subagents(project_path) -> Vec<SubagentListModel>`

UI 数据源。组装:
1. `SubagentCache::list(project_path)`(已存在,builtin + user + project 合并 + source tag)
2. 叠加 DB override 解析:每个 agent 的最终 model = `db_override(name) or def.model`
3. 查 `models.display_name` 把 model_id 映射成 display(UI 友好)

```rust
struct SubagentListModel {
    name: String,
    source: String,                 // builtin / user / project
    description: String,
    resolved_model_id: Option<String>,   // DB>frontmatter 后的最终值(None=inherit parent)
    resolved_model_display: Option<String>,
    declared_model_id: Option<String>,   // frontmatter 原值(debug/可观测)
    has_db_override: bool,              // DB 表有该 name 的记录
    writable: bool,                     // source!=builtin(能否写回 frontmatter)
}
```

### 5.2 `set_subagent_model(name, source, project_path, model_id: Option<String>)`

UI 改一个 agent 的 model:
- `source == builtin` → `model_id=Some` 走 `set_subagent_model_override`;`None` 走 `clear_subagent_model_override`。
- `source ∈ {user, project}` → 调 `write_frontmatter_model(<推导路径>, model_id)`。路径由 `source` + `name` + `project_path` 经 **loader 既有的路径规则常量**(`AGENTS_SUBDIR` / `PROJECT_NAMESPACE`;user 层 = `<user_dir>/<AGENTS_SUBDIR>/<name>.md`,project 层 = `<project_path>/<PROJECT_NAMESPACE>/<AGENTS_SUBDIR>/<name>.md`)推导 —— **无需给 `LoadedSubagent` 加 `file_path` 字段**(其当前只有 `{def, source}`),抽 `locate_agent_file(source, name, project_path) -> PathBuf` helper 让 loader 扫描与 IPC 写回共用同一套定位逻辑。
- 写完返回最新 `SubagentListModel`(前端免二次 round-trip)。

### 5.3 `list_models` / `get_default_model` 已有

前端 `useModelsStore` 直接复用为下拉数据源。

## 6. 前端 — Settings 新 Subagents tab

### 6.1 结构

`SettingsModal.vue` 加第 5 个 `TabsTrigger value="subagents"` + `SubagentsTab.vue`(与 `MemoryTab.vue` 同级,复用其布局/滚动壳)。

### 6.2 `SubagentsTab.vue`

- `onMounted`:`list_subagents(currentProjectPath)` + `useModelsStore.fetch()`(下拉数据)。
- 列表:每行 = `name` + source tag chip + description(单行截断) + model 下拉。
- 下拉选项:`[{label: "继承父级 (inherit)", value: null}]` + 按 provider group 的所有 model(`modelsGroupedByProvider`,label = display_name,value = id)。
- 当前选中 = `resolved_model_id`(null → 显示 inherit)。
- source=builtin 行:下拉旁小字 "(DB override)" 标注 `has_db_override`。
- 改动 → `set_subagent_model(name, source, model_id)` → 用返回值本地刷新该行(spinner 隔离,仿 `WorkerMergeControls` 的 per-row reactive 模式)。
- 失效 model(override 指向已删 model)→ 下拉显示该 id + 红字"模型已删除,将降级",仍可改。

### 6.3 project_path 来源(canonical 保证)

前端用 `useChatStore.currentCwd` 传入 IPC,但**后端必须 canonicalize**(或前端传 `projectId`、后端经 `projects` 模块反查 canonical path),确保与 `SubagentCache` 内部 mtime-fenced 缓存的 key 一致——裸 `currentCwd` 可能非 canonical → cache miss → agent 列表为空。Settings 打开时锁定快照,切换 project 不热更(MVP)。

## 7. 边界与兼容

| 场景 | 行为 |
|---|---|
| DB override 指向已删 model | catalog miss → `resolve_worker_provider` warn + parent fallback;UI 标红 |
| 同名 user agent 覆盖 builtin,且 DB 有该 name override | DB 盖 frontmatter(`DB>frontmatter`);UI 显示 DB override 生效 |
| 写回 frontmatter 时文件无 fence(竞态:外部刚改坏) | 返错,不隐式补 fence(正常路径 agent 必有 fence,见 §4.2) |
| 写回 frontmatter 失败(权限/磁盘) | IPC 返错,UI 提示,不改 DB |
| DB override 与 frontmatter 同时存在 | DB 胜;UI 改该 agent 时按 source 决定写 DB 还是文件(不互写) |
| `resolve_worker_provider` 既有 6 测试 | 零改动(前置 resolve_final_model 不动它) |
| `run_chat_loop` 23 参签名 | 不动(resolve_final_model 在 `run_subagent` body 内现取 db,仿 A 任务 catalog 取法) |

## 8. 测试策略

- **DB 层**(`tests_subagent_overrides.rs`):get/set/clear/list + UPSERT 幂等 + clear 不存在不报错。
- **优先级**(`tests_subagent.rs` 增):DB>frontmatter(两者都有 → DB 胜)/ 仅 frontmatter / 仅 DB / 都无 → parent / DB 指向失效 model → frontmatter 兜底(再无则 parent)。
- **写回 frontmatter**(`loader.rs` 测):纯函数 `apply_model_line(text, mid) -> String`(输入必有 fence):已声明替换 / 未声明插入首行 / None 删除 / 保留 body 与注释;`write_frontmatter_model` IO 层:无 fence 返错 / 原子写(`.tmp` + rename)。
- **IPC**:`list_subagents` 组装(DB 叠加 + display 映射)/ `set_subagent_model` builtin→DB / user→文件。
- **前端**(`SubagentsTab` vitest,若易 mock):渲染列表 + 下拉 + 改动调 IPC + inherit 选项。
- 回归:`tests_subagent.rs` / `tests_agent_loop.rs` 既有用例全绿(A 任务 AC1-5 不回归)。

## 9. 回滚形状

- DB 表:`CREATE TABLE IF NOT EXISTS`,回滚 = DROP TABLE(无数据依赖,override 是纯偏好)。
- `resolve_final_model`:删函数 + caller 还原传 `def.model.as_deref()`(一行)。
- IPC:注销两个 command + 删 store action(前端编译即暴露)。
- 前端 tab:删 `SubagentsTab.vue` + Settings 一处 import。
- frontmatter writer:纯新增,删函数即可(已写入的 `model:` 行不影响 loader——本来就支持)。
- 可观测性:`subagent_runs.model_display` 列保留(null safe,前端 `v-if` 隐藏即回滚);真要清则 `UPDATE … SET NULL`。

各阶段独立可回滚,无破坏性 schema 变更。

## 10. 可观测性:card / drawer 显示 worker model

> 用户补充需求(prd AC13-15)。sub-agent card 与 drawer 要显示 worker 实际用的 model name。

### 10.1 数据源统一:`subagent_runs.model_display`

**不**解析 `tool_result` 文本的 `[model:]` 行(A 任务加的,给 parent LLM 看;格式属实现细节,前端解析脆弱)。改为结构化持久化:

- `subagent_runs` 加 `model_display TEXT NULL` 列(`migrations.rs` 走 `add_subagent_runs_column_if_missing`,同 `task` / `final_text` / `worktree_path` 模式)。
- `run_subagent` 在 `resolve_final_model` + `resolve_worker_provider` 拿到 display 后写入(`insert_run_with_id` 时带上;display **直接取 `resolve_worker_provider` 返回的第三项 `Option<String>`**:仅 catalog hit 时为 `Some(name)`,parent 继承 / catalog miss 时为 `None` → 写 `NULL`)。语义与 tool_result 的 `[model:]` 行**完全一致**(`format_dispatch_result_with_model` 在 `None` 时省略该行)。**不改 `run_subagent` 签名**——parent display 不在其入参(只有 `provider: Arc<dyn Provider>` + `context_window`),本 task 不 thread。
- **前端**:`modelDisplay=null` → card/drawer 显示「继承父级」(或不显示)。
- **follow-up**(非本 task):若要 parent 继承时也显示具体模型,需把 chat 入口 `ResolvedChatProvider.model_display_name` thread 到 `run_subagent`(A 任务 design §113/118 设想但**未落地**——实际 `resolve_worker_provider` parent 继承返回 `None`)。
- `SubagentRunSummary`(后端 struct `#[serde(rename_all="camelCase")]`)+ 前端 `SubagentRunSummary` type 加 `modelDisplay: string | null`(legacy 旧 row = null,前端容忍)。

### 10.2 card 折叠预览(`ToolCallCard.vue` dispatch 分支)

card 已有 `workerSummary`(`getSummaryByToolUseId` → `SubagentRunSummary`)+ chip 模式(`workerTokenText` / `workerDisplayName`)。复用同一套:

- 加 `workerModelText` computed:`workerSummary.value?.modelDisplay ?? ""`。
- 模板:token chip 旁加 model chip(`<Icon name="cpu" :size="12"/> {{ workerModelText }}`),`v-if="workerModelText"`(空隐藏)。
- 进行中(row 未落地 / modelDisplay=null)→ 隐藏,不报错。
- **preview 文本去重(必须)**:card 折叠预览 `workerSummaryPreview` 在 fallback 用 `props.result.content` 时,该 content 含 `[model: X]` 行(A 任务 tool_result)。chip 已独立显示 model,**preview 文本必须 strip 掉 `[model: …]` 行**避免重复显示 —— 在 `workerSummaryPreview` 的 fallback 分支加一行 regex strip(也顺带 strip `[status: …]` 前缀,与 summary 路径对齐)。

### 10.3 `SubagentDrawerHeader.vue`

title-row 的 name 旁(或 meta 行)加 model 显示:`run?.modelDisplay` → mono 小字;`v-if` 守护 null。纯展示组件,直接读 `run.modelDisplay`(main drawer 已透传 `run`),无需新增 prop。

### 10.4 与 C 主体的关系

共享 `resolve_final_model`(C 改输入 = DB override;本节用输出 = display)。**独立于 DB override**——无 override 时,A 的 frontmatter model 也经此展示。并入 C 因同主题 + 共享 resolve 路径(避免拆 task 的合并冲突),但可单独验证:AC13-15 不依赖 AC1-8。
