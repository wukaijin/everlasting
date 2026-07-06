# Design — B6+ B subagent dispatch 动态选模型

> 配套 `prd.md`。本文聚焦技术设计、契约变更、关键决策与回滚。执行清单见 `implement.md`。

## 1. 现状回顾（已验证事实）

| 事实 | 位置 |
|---|---|
| 优先级链当前 = `DB override > frontmatter`，由 `resolve_final_model(db, agent_name, frontmatter_model)` 收敛成 `Option<model_id>` | `agent/subagent/dispatch.rs:1267-1278`，调用点 `dispatch.rs:554` |
| `resolve_final_model` 之后 → `resolve_worker_provider(final_model, parent, ctx, catalog, db)`（hit/miss 纯函数，6 测试零回归） | `dispatch.rs:1308-1339` |
| dispatch input 解析：`input.get("subagent")` / `input.get("task")` / `input.get("isolation")`，`model` 同类插入 | `dispatch.rs:336-337, 402` |
| `dispatch_subagent` tool schema（生产 `definition_with_cache` + 测试 `definition()`）只有 `subagent` / `task` / `isolation` | `agent/subagent/mod.rs:299-329`（生产）+ `:188-219`（测试） |
| ForcedDispatch wire `{ subagent, task }`（snake_case），Tauri arg `forcedDispatch`（camelCase），turn-1 short-circuit 合成 `input = { subagent, task }` 调 `run_subagent` | `agent/subagent/mod.rs:137-148` + `chat_loop.rs:932-989` |
| 前端 `@@` 解析：`/^@@([A-Za-z0-9_-]+)[ \t]+([\s\S]+)$/` → `{ subagent, task }` | `app/src/stores/chat.ts:871-879` |
| `subagent_runs.model_display` 列已存在（C 落地），`run_subagent` 用 `worker_display`（catalog hit=Some / 其他=None）写入 | `dispatch.rs:624-644` |
| `format_dispatch_result_with_model(content, status, model_display, ...)` 已存在（A 落地），`model_display=None` 省略 `[model:]` 行 | `agent/subagent/truncate_summary.rs` |
| catalog = `HashMap<model_id, Arc<dyn Provider>>`，key 是 UUID | `state.rs:60` |
| `db::models::get_model(pool, id)` 返回 `ModelRow { display_name, context_window, ... }` | `db/models.rs:109` |
| `db::models::list_models(pool)` 返回 `Vec<ModelWithProvider>`（含 `display_name` + `id`） | `db/models.rs:68` |
| 前端 `useModelsStore` 已加载 `list_models`，`models: ModelWithProvider[]` 含 `id` + `display_name` | `app/src/stores/models.ts:28,75`；ChatInput.vue 已 import |

**关键洞察**：A/C 已把 model 解析链（`resolve_final_model` → `resolve_worker_provider`）建好，B 只需在**链的最前面**加一层 dispatch override（最高优先级），不动既有任何函数签名。两条入口（LLM / `@@`）汇合到同一个 `input.model` 字段，`run_subagent` 统一解析。

## 2. 方案总览

```
LLM dispatch_subagent({ subagent, task, model? })      user @@agent --model=<X> <task>
        │                                                       │ (前端解析 flag + 反查 display_name→id)
        │                                                       ▼
        │                                          ForcedDispatch { subagent, task, model_id? }
        │                                                       │ chat_loop turn-1 合成 input
        ▼                                                       ▼
   input = { subagent, task, model?: <id> }  ◄─────────────────┘
        │ run_subagent 解析
        ▼
┌──────────────────────────────────────────────────────────┐
│ dispatch_model = input.get("model").as_str().filter(!empty)│
│                                                          │
│ final_model =                                            │
│   dispatch_model              ◄── B 新增（最高优先级）     │
│     .or_else(resolve_final_model(db, name, def.model))   │
│         └─ DB override > frontmatter（C 落地）            │
│                                                          │
│ (worker_provider, worker_ctx, worker_display) =          │
│   resolve_worker_provider(final_model, ...)  ◄── 不动    │
└──────────────────────────────────────────────────────────┘
```

核心：dispatch override 接在 `resolve_final_model` **之前**，作为一个 `Option` 叠加层。`resolve_final_model` / `resolve_worker_provider` 签名与测试**零改动**。

## 3. 契约 / 接口变更

### 3.1 `dispatch_subagent` tool schema（`agent/subagent/mod.rs`）

两处 schema（生产 `definition_with_cache:299` + 测试 `definition():188`）都加 `model` 属性：

```jsonc
"model": {
    "type": "string",
    "enum": [<display_name_1>, <display_name_2>, ...],
    "description": "Override the worker's model for THIS dispatch only (does not \
                    persist). Pick a value from the enum (a model display name). \
                    When omitted, the worker uses its configured default (Settings \
                    per-agent override > frontmatter `model:` > parent's model). \
                    Use this for cross-model adversarial review (e.g. dispatch \
                    reviewer with a stronger / different-family model)."
}
```

- **不**加进 `required`（可选）。
- **enum 值 = display_name**（决策 4）：人可读，LLM 不必猜 UUID。后端经共享反查（§3.8）display_name→id。
- **enum 动态构建**：`definition_with_cache` 新增 `models: &[ModelBrief]` 参数，enum = `models.iter().map(|m| m.display_name).collect()`。chat_loop turn-0 查一次 `list_models` 投影成 `Vec<ModelBrief>` 缓存到 local，每轮 turn 复用（§3.9）。
- **测试用 `definition()`**（静态，无 cache/models 参数）：enum 用空 `&[]` 或省略（静态版本无 models 上下文，enum 为空数组 `[]`；既有 `definition_schema_*` 测试只断言 required + subagent enum，不涉及 model enum，零回归）。
- description 末尾保留 model 来源说明（enum 已是来源，description 补语义）。

### 3.2 dispatch input 解析（`dispatch.rs:336` 附近）

在 `subagent_name` / `task` 解析后加。LLM path 传 display_name（§3.1 enum），需经 §3.8 共享反查转 id：

```rust
// LLM path 传 display_name（schema enum 值），反查 → id；未命中 → None（走 agent 默认）。
let dispatch_model_raw: Option<&str> = input
    .get("model")
    .and_then(|v| v.as_str())
    .map(str::trim)
    .filter(|s| !s.is_empty());
let dispatch_model: Option<String> = match dispatch_model_raw {
    Some(raw) => match resolve_model_by_name_or_id(db, raw).await {
        Ok(Some(id)) => Some(id),
        Ok(None) => {
            tracing::warn!(input = raw, "dispatch model not found; ignoring (using agent default)");
            None
        }
        Err(e) => {
            tracing::warn!(input = raw, error = %e, "dispatch model lookup failed; ignoring");
            None
        }
    },
    None => None,
};
```

- `resolve_model_by_name_or_id` 见 §3.8（共享反查，先精确 id 再 display_name 取首）。
- 未命中不报错（返 None）→ dispatch_model=None → 走 `resolve_final_model`（AC7）。

### 3.3 优先级叠加（`dispatch.rs:554` 附近，**唯一核心改动**）

当前：
```rust
let final_model = match resolve_final_model(db, def.name.as_str(), def.model.as_deref()).await {
    Ok(m) => m,
    Err(e) => { warn!(...); def.model.clone() }
};
```

改为：
```rust
let resolved_lower = match resolve_final_model(db, def.name.as_str(), def.model.as_deref()).await {
    Ok(m) => m,
    Err(e) => { warn!(...); def.model.clone() }
};
let final_model = dispatch_model.clone().or(resolved_lower);
```

- `dispatch_model=Some` → 用之（最高优先级，跳过 DB/frontmatter）。
- `dispatch_model=None` → `final_model = resolved_lower`（= 现状，A/C 零回归）。
- 失效兜底由下游 `resolve_worker_provider` 统一处理（catalog miss → warn + parent），无需新代码。

### 3.4 ForcedDispatch 扩展（`agent/subagent/mod.rs:137`）

```rust
pub struct ForcedDispatch {
    pub subagent: String,
    pub task: String,
    /// B6+ B: optional per-dispatch model override. Parsed from
    /// `@@agent --model=<X> <task>` by the frontend; `<X>` resolved
    /// to model **id** via `useModelsStore` (display_name or id both
    /// accepted). `None` = no override (use agent default).
    pub model_id: Option<String>,
}
```

- wire 仍 snake_case（`model_id`），Tauri arg `forcedDispatch` 整体 camelCase 不变（serde-converts 整个 struct）。
- **wire 只认 id**（前端反查 display_name→id 后传 id；与后端 LLM path 在 §3.2 反查后汇合，`run_subagent` 内部 `dispatch_model` 一律是 id）。

### 3.5 chat_loop turn-1 short-circuit（`chat_loop.rs:937`）

当前合成 `input = { subagent, task }`，改为：

```rust
let mut input = serde_json::json!({
    "subagent": fd.subagent,
    "task": fd.task,
});
if let Some(mid) = &fd.model_id {
    input["model"] = serde_json::Value::String(mid.clone());
}
```

`run_subagent` 通过 `input.get("model")` 统一解析，与 LLM path 汇合。

### 3.6 前端 `@@` 解析（`app/src/stores/chat.ts:871-879`）

当前：
```ts
let forcedDispatch: { subagent: string; task: string } | undefined;
const atAt = trimmed.match(/^@@([A-Za-z0-9_-]+)[ \t]+([\s\S]+)$/);
if (atAt) {
    const task = atAt[2].trim();
    if (!task) return;
    forcedDispatch = { subagent: atAt[1], task };
    body = task;
}
```

改为（flag 位置：紧跟 agent 名、task 之前）：
```ts
type ForcedDispatch = { subagent: string; task: string; model_id?: string };
let forcedDispatch: ForcedDispatch | undefined;
// flag 必须紧跟 @@agent 之后、task 之前（git/cargo flag 语义）
const atAt = trimmed.match(
    /^@@([A-Za-z0-9_-]+)[ \t]+(?:--model=(\S+)[ \t]+)?([\s\S]+)$/
);
if (atAt) {
    const task = atAt[3].trim();
    if (!task) return;
    const rawModel = atAt[2]; // undefined | string
    const model_id = rawModel ? resolveModelInput(rawModel) : undefined;
    forcedDispatch = { subagent: atAt[1], task, ...(model_id ? { model_id } : {}) };
    body = task;
}
```

`resolveModelInput(raw): string | undefined` helper（新，`chat.ts` 内或 utils）：
- `useModelsStore().models` 里 `find(m => m.id === raw || m.display_name === raw)`。
- 命中 → 返 `m.id`（display_name 输入自动转 id；id 输入直返）。
- 多 display_name 同名 → `find` 取首（Array.find 语义）+ `console.warn`（前端可见，不入后端日志）。
- 未命中 → 返 `undefined`（即不附加 `model_id`，dispatch 走 agent 默认；**不报错**，与 AC9 "解析失败不静默丢弃但也不误解析" 对齐 — 用户在输入框看到 `--model=xxx` 文本仍在，可改正重发；MVP 不弹 toast，避免过度设计）。

> **决策**：`@@` path 的 display_name→id 反查放前端（`useModelsStore` 已加载 list_models，零 IPC），wire 只认 id；LLM path 的 display_name→id 反查放后端（LLM 输入在后端，§3.2/§3.8）。两路径反查逻辑极简（各 ~10 行），见决策 4b。

### 3.7 streamController 类型（`app/src/stores/streamController.ts:1851`）

```ts
forcedDispatch?: { subagent: string; task: string; model_id?: string };
```

加 `model_id?`。

### 3.8 后端共享反查 `resolve_model_by_name_or_id`（新，`dispatch.rs`）

```rust
/// B6+ B: resolve a model id from either an id (passthrough) or a
/// display_name (reverse lookup). Serves the LLM path (schema enum
/// values are display_names). Returns None on miss (caller falls
/// back to agent default).
pub(crate) async fn resolve_model_by_name_or_id(
    db: &SqlitePool,
    input: &str,
) -> Result<Option<String>, sqlx::Error> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    // ① exact id match (passthrough).
    if let Some(row) = crate::db::models::get_model(db, trimmed).await? {
        return Ok(Some(row.id));
    }
    // ② display_name reverse lookup (first match wins; display_name
    //    should be unique but DB doesn't enforce it).
    let models = crate::db::list_models(db).await?;
    Ok(models
        .into_iter()
        .find(|m| m.model.display_name == trimmed)
        .map(|m| m.model.id))
}
```

- 先精确 id（`get_model`，O(1)），miss 再 `list_models` 找 display_name（取首）。
- 未命中返 `Ok(None)`（不报错）→ caller §3.2 记 warn + dispatch_model=None。
- 多 display_name 同名取首（`find` 语义）；不 warn（后端 list_models 无排序保证，取首是确定性的；歧义场景低频，follow-up 可加 warn）。

### 3.9 chat_loop turn-0 models 快照 + `definition_with_cache` 加参数

`definition_with_cache` 签名扩展（加 `models: &[ModelBrief]`）：

```rust
pub struct ModelBrief {
    pub id: String,
    pub display_name: String,
}

pub async fn definition_with_cache(
    cache: &SubagentCache,
    project_path: &str,
    models: &[ModelBrief],   // ← 新增
) -> ToolDef
```

- enum = `models.iter().map(|m| m.display_name.clone()).collect::<Vec<_>>()`；空 slice → enum `[]`（防御，正常 ≥1）。
- 测试用静态 `definition()`（无此参数）：model enum 为空数组或不出现（§3.1 测试用版本）。

chat_loop turn-0 快照（`chat_loop.rs` 循环外、turn 工具构造前）：

```rust
// B6+ B: snapshot models once per chat session (models change at
// low frequency; CRUD during a session is reflected next session
// + covered by catalog-miss fallback). Used to build the dynamic
// dispatch_subagent `model` enum (display_name values).
let model_briefs: Vec<crate::agent::subagent::ModelBrief> = match crate::db::list_models(&db).await {
    Ok(rows) => rows.into_iter()
        .map(|mwp| crate::agent::subagent::ModelBrief {
            id: mwp.model.id,
            display_name: mwp.model.display_name,
        })
        .collect(),
    Err(e) => { tracing::warn!(error=%e, "list_models failed; dispatch model enum will be empty"); vec![] }
};
```

然后调用点（`chat_loop.rs:1450`）：

```rust
let dispatch_def = crate::agent::subagent::definition_with_cache(
    &subagent_cache, &project_path, &model_briefs,
).await;
```

- 快照在 session 级（`run_chat_loop` 入口附近），不在每 turn 重查（省 DB roundtrip）。
- 会话内新加 / 删 model：MVP 不即时反映 enum（下一 chat 会话刷新）；catalog 仍实时（dispatch 时 `resolve_model_by_name_or_id` 查最新 DB），故 enum 滞后只影响 LLM 的"选项可见性"，不影响实际解析正确性。

## 4. 关键决策

### 决策 1：dispatch override 接在 `resolve_final_model` 之前，不动既有签名

- **理由**：`resolve_final_model` / `resolve_worker_provider` 的 6+5 个既有测试零回归；新优先级是 `Option` 叠加（`dispatch_model.or(resolved_lower)`），一行代码。
- **替代**：扩 `resolve_final_model` 签名加 `dispatch_model: Option<&str>` 参数 → 改所有 caller + 测试 fixture，传染大。

### 决策 2：display_name→id 反查放前端，wire 只认 UUID

- **理由**：前端 `useModelsStore` 已就绪（零额外 IPC）；后端单一解析点；wire shape 统一（ForcedDispatch + LLM path）。
- **代价**：后端日志看不到用户原始输入的 display_name（只记最终 id + catalog hit 后的 display）。可接受（用户面向错误在前端 console.warn 兜底）。

### 决策 3：`@@` flag 位置严格（紧跟 agent 名、task 之前），非通用 flag parser

- **理由**：task 中间 `--model=` 不误解析（task 常含 flag 文本，如"帮我看这个 --model= 的解析"）；正则简单（一个 optional 捕获组）；git/cargo flag 语义用户熟悉。
- **替代**：任意位置提取 → 误伤风险高，需复杂 parser。

### 决策 4：LLM path 的 `model` 入参**加 enum，值 = display_name**（人可读），后端反查 display_name→id

- **理由（推翻原"不做 enum"）**：system prompt 里**没有** model 列表（`build_system_prompt` 不含 catalog/models 信息，已核实 `agent/system_prompt.rs:74`）。LLM 唯一能"知道"有哪些 model 的途径是 tool schema 的 enum。不做 enum → LLM 瞎编 UUID，功能不可用。
- **enum 值选 display_name 而非 id**：display_name 人可读（"GPT-4o" / "Claude Sonnet 4.5"），id 是 UUID（`550e8400-...`）不可读且占 token。LLM 传 display_name，后端反查 → id（复用 §3.8 的共享反查函数）。
- **models 数据源**：`definition_with_cache` 新增 `models: &[ModelBrief]` 参数（`ModelBrief { id, display_name }`，从 `list_models` 投影）。chat_loop 在 turn-0 查一次 `list_models` 缓存到 local（models 在单 chat 会话内视为稳定快照——session 锁定 model，CRUD 低频；会话内新加 model 不即时反映，靠下一 chat 会话刷新 + 失效兜底覆盖）。
- **失效兜底**：LLM 传的 display_name 反查无果（被删 / 拼错）→ 共享反查返 None → dispatch_model=None → 走 `resolve_final_model`（AC7）。
- **token 成本**：enum 列 N 个 display_name，典型 N=3-8，每条 ~15 字符，总 ~100-200 token，可接受（dispatch_subagent description 已含 agent enum，同款模式）。

### 决策 4b：display_name→id 反查抽共享函数，前后端各一份（或后端单源）

- **后端** `resolve_model_by_name_or_id(db, input) -> Option<String>`（新，`dispatch.rs`）：先精确匹配 id（`get_model(input)`），miss 再 `list_models` 找 `display_name == input`（取首）。服务 LLM path（display_name enum 值）+ 可选服务 `@@` path 后端反查。
- **前端** `resolveModelInput(raw, models)`（§3.6）：纯内存查 `useModelsStore().models`，id 精确 / display_name 取首 + `console.warn`。服务 `@@` path（前端反查，wire 只认 id）。
- **为何不强制单源**：前端 `useModelsStore` 已加载 list_models（零 IPC），后端反查是 LLM path 的必然落点（LLM 输入在后端）；两份逻辑极简（各 ~10 行），强行合并（如前端反查后 wire display_name 给后端再查）反而多一层。接受轻度重复。

### 决策 5：前端反查未命中不报错、不弹 toast，只 `console.warn` + 不附加 model_id

- **理由**：用户输入框仍可见 `--model=xxx`，可改正重发；弹 toast 过度设计（forced dispatch 本身是快捷方式，不是主路径）。
- **代价**：用户不知道反查失败（除非开 devtools）。可接受；若高频抱怨，follow-up 加行内 hint。

## 5. 失败模式与 fallback

| 场景 | 行为 |
|---|---|
| LLM 传不存在的 model id（编造 / 已删） | `resolve_worker_provider` catalog miss → `warn!` + parent fallback（AC7） |
| `@@ --model=` display_name 拼错 | 前端反查未命中 → `console.warn` + 不附加 model_id → dispatch 走 agent 默认（AC7/AC9） |
| `@@ --model=` 后无值（`@@agent --model= task`） | 正则 `(\S+)` 要求 ≥1 非空白，`--model=` 后直接空白 → 该捕获组不匹配 → 整体退化为无 flag 匹配，`task` 含 `--model=` 文本 → 走普通 `@@agent <task>` 解析（task = `--model= task`，合法但奇怪）→ 用户可见可改 |
| dispatch_model 命中 catalog 但 `get_model` DB 失败 | `resolve_worker_provider` 已处理：catalog hit 说明 provider 有效，ctx 用 parent，display 用 model_row 或 None（既有逻辑） |
| `@@` 出现在 task 中间（非前缀） | 正则 `^@@` 锚定行首，中间 `@@` 不匹配 → 整条当普通消息（无 forced dispatch） |
| ForcedDispatch 反序列化 `model_id` 缺失（旧前端 + 新后端） | `Option<String>` 缺字段 → `None`（serde 容错），等效无 override |

## 6. 兼容性

- **A/C 零回归**：`dispatch_model=None` → `final_model = resolve_final_model(...)` = 现状（A 的 frontmatter + C 的 DB override 全保留）。
- **builtin agent 零回归**：`researcher` / `general-purpose` 无 frontmatter model，无 DB override（除非用户 C 配过），dispatch 不传 model → 继承 parent。
- **旧前端 + 新后端**：ForcedDispatch `model_id` 是 `Option`，serde 容错缺失字段。
- **新前端 + 旧后端**（降级场景，理论上不会发生）：前端发 `model_id`，旧后端 `ForcedDispatch` 无该字段 → serde 反序列化**报错**（serde 默认 deny unknown fields? 需核实；若 deny，加 `#[serde(default)]`）。**核实点**见 implement。
- **RULE-A-007（tool_use/tool_result 配对）**：input 加字段不改配对结构。
- **取消传播 / worktree 隔离**：不受 model override 影响。

## 7. 测试策略

### 后端（`agent/subagent/dispatch.rs` 内联 `#[cfg(test)]`）

1. `dispatch_model_overrides_db_override` — dispatch_model=X + DB override=Y → final=X（AC2）。
2. `dispatch_model_overrides_frontmatter` — dispatch_model=X + frontmatter=Y → final=X（AC3）。
3. `dispatch_model_none_falls_to_resolve_final_model` — dispatch_model=None + DB=Y → final=Y（AC4 零回归）。
4. `dispatch_model_none_no_db_no_frontmatter_inherits_parent` — 全无 → final=None → parent（AC4）。
5. `dispatch_model_missing_in_catalog_falls_back` — dispatch_model=不存在 id → catalog miss → warn + parent（AC7）。
6. `dispatch_input_model_display_name_resolved_to_id` — LLM 传 display_name（schema enum 值）→ 经 `resolve_model_by_name_or_id` 反查 → final=id（AC1，覆盖 §3.2 + §3.8）。
7. `dispatch_input_model_unknown_display_name_ignored` — display_name 反查无果 → dispatch_model=None → 走 agent 默认（AC7 LLM path）。
8. `resolve_model_by_name_or_id_id_passthrough` — 输入 = 已存在 id → 直返该 id。
9. `resolve_model_by_name_or_id_display_name_lookup` — 输入 = display_name → 返对应 id；多同名取首。
10. `resolve_model_by_name_or_id_miss_returns_none` — 输入不存在 → Ok(None)。
11. `dispatch_input_model_parsed_from_json` — `input.model` 字段被正确解析（§3.2 入口）。
12. `resolve_priority_chain_idempotent` — 多次 dispatch 同一 agent 不传 model → 行为一致（不持久化）。
13. `definition_with_cache_model_enum_from_briefs` — `definition_with_cache(.., &[ModelBrief{...}])` 的 schema enum = display_names（§3.1/§3.9）。

> 这些测试针对叠加逻辑 + display_name 反查 + enum 构建，**不**重测 `resolve_final_model` / `resolve_worker_provider`（既有测试覆盖）。

### 前端（`app/src/stores/chat.test.ts` 或新 `chatForcedDispatch.test.ts`）

8. `parses_model_flag_with_id` — `@@reviewer --model=<id> task` → forcedDispatch.model_id = id（AC5）。
9. `parses_model_flag_with_display_name` — `@@reviewer --model=GPT-4o task` → model_id = GPT-4o 的 id（AC6）。
10. `model_flag_in_task_middle_not_parsed` — `@@reviewer task --model=x` → 无 model_id，task 含 `--model=x`（AC9）。
11. `model_flag_no_value_degrades` — `@@reviewer --model= task` → 退化为无 flag（AC9）。
12. `resolveModelInput_unknown_returns_undefined` — 反查未命中 → undefined + console.warn（AC7 前端侧）。
13. `resolveModelInput_multiple_same_name_takes_first` — 多同名取首（AC6）。
14. `no_model_flag_omits_field` — `@@reviewer task` → forcedDispatch 无 model_id 字段（AC4 零回归）。

（前端测试编号在最终 §7 序列里为 15-21，因后端反查新增了 5 个用例，见上。）

### 集成（`tests_agent_loop.rs` 或 `tests_subagent.rs`）

22. `forced_dispatch_with_model_swaps_provider` — ForcedDispatch 带 model_id → run_subagent 用该 model（端到端，AC5 后端侧）。
23. `llm_dispatch_with_display_name_swaps_provider` — LLM-style `input.model=<display_name>` → 后端反查 → worker 换 provider（AC1 端到端）。

## 8. 回滚形状

- **后端**：`final_model = dispatch_model.or(resolved_lower)` → 还原为 `final_model = resolved_lower`（一行）；删 `dispatch_model` 解析（§3.2）+ `resolve_model_by_name_or_id`（§3.8）；schema 删 `model` 属性 + enum；`definition_with_cache` 删 `models` 参数；`ModelBrief` 删。
- **ForcedDispatch**：删 `model_id` 字段（`Option` 缺失向后兼容，旧 wire 不报错）。
- **chat_loop**：删 turn-0 `model_briefs` 快照 + turn-1 input 注入 + `definition_with_cache` 调用点还原。
- **前端**：还原正则 + 删 `resolveModelInput`（一处）+ streamController 类型回退。
- 无 DB schema 变更，无 migration；builtin 行为零回归。

## 9. 影响面清单

| 文件 | 改动 |
|---|---|
| `agent/subagent/mod.rs` | `definition()` + `definition_with_cache()` schema 加 `model` 属性（enum=display_name）；`definition_with_cache` 加 `models: &[ModelBrief]` 参数；`ForcedDispatch` 加 `model_id: Option<String>`；新 `ModelBrief { id, display_name }` struct |
| `agent/subagent/dispatch.rs` | input 解析 `dispatch_model`（§3.2，含 display_name 反查）；`final_model` 叠加（§3.3，核心一行）；新 `resolve_model_by_name_or_id`（§3.8） |
| `agent/chat_loop.rs` | turn-0 `model_briefs` 快照（§3.9）；turn-1 short-circuit 注入 `input.model`（§3.5）；`definition_with_cache` 调用点传 `&model_briefs`（§3.9） |
| `app/src/stores/chat.ts` | `@@` 正则 + `resolveModelInput` + ForcedDispatch 类型（§3.6） |
| `app/src/stores/streamController.ts` | forcedDispatch 类型加 `model_id?`（§3.7） |
| spec（Phase 3.3） | `tool-contract.md` dispatch_subagent 段加 model 属性（enum=display_name + 反查语义）；`agent-loop-architecture.md` 优先级表加 dispatch row；`subagent-runs-schema.md` 无改（model_display 已存在）；`frontend/chat.md` `@@` 解析段加 flag |

无 DB / migration / IPC 新增 / 前端组件新增。
