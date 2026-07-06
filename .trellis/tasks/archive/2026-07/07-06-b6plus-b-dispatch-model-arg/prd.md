# B6+ B subagent dispatch 动态选模型

## Goal

让 parent agent 在**单次 dispatch** 时动态指定 worker 用哪个模型，而不必依赖 worker 的静态声明（frontmatter `model:`）或全局 DB override。补齐 B6+ 多模型线的最后一档，使优先级链完整：**dispatch input > DB override > frontmatter > parent**。

典型场景：parent 用 Claude 写代码，dispatch reviewer 时临时指定用 GPT-4o 做跨模型对抗性 review，无需改 reviewer 的全局默认模型。

## 背景与动机

B6+ 三档：

- **A（2026-07-03 已落地）**：frontmatter `model:` 声明 + `resolve_worker_provider`（catalog 穿透）。worker 按**静态文件声明**换 provider。
- **C（2026-07-03 已落地）**：Settings UI per-agent 默认模型 + builtin DB override（`subagent_model_overrides` 表）+ 写回 frontmatter + `resolve_final_model`（DB > frontmatter）。card/drawer 显示 worker model。
- **B（本任务）**：dispatch 时动态选模型。两条入口：
  1. LLM-driven：`dispatch_subagent({ subagent, task, model })` 加 `model` 入参。
  2. user-driven：`@@agent --model=<id|display_name> <task>` 前缀。

A/C 都是**声明性**（写一次，所有 dispatch 该 agent 都用那个 model）；B 是**临时性**（单次 dispatch 覆盖），三者互补。

## 已确认事实（代码证据）

| 事实 | 位置 |
|---|---|
| `dispatch_subagent` tool schema 当前只有 `subagent` / `task` / `isolation`（无 `model`） | `agent/subagent/mod.rs:299-329`（`definition_with_cache`，生产路径）+ `:188-219`（静态 `definition()`，测试用） |
| 优先级链当前 = `DB override > frontmatter`，由 `resolve_final_model` 收敛成单个 `Option<model_id>` | `agent/subagent/dispatch.rs:1267-1278` + 调用点 `dispatch.rs:554` |
| `resolve_final_model(db, agent_name, frontmatter_model)` 签名（3 参，返回 `Result<Option<String>, sqlx::Error>`） | `dispatch.rs:1267` |
| `resolve_worker_provider(final_model, parent, ctx, catalog, db)` 不动（6 测试零回归）—— 仍是 hit/miss 的纯函数 | `dispatch.rs:1308-1339` |
| dispatch input 解析点：`input.get("subagent")` / `input.get("task")` / `input.get("isolation")` —— `model` 同类插入 | `dispatch.rs:336-337, 402` |
| ForcedDispatch struct 只有 `subagent` + `task`（无 `model`），serde camelCase wire `forcedDispatch` | `agent/subagent/mod.rs:137-148` |
| 前端 `@@` 解析：`/^@@([A-Za-z0-9_-]+)[ \t]+([\s\S]+)$/` —— 只解析 name + task，无 `--model` flag | `app/src/stores/chat.ts:873` |
| `subagent_runs.model_display` 列已存在（C 落地），`run_subagent` 用 `worker_display` 写入（catalog hit=Some / 其他=None） | `dispatch.rs:624-644` + `db/subagent_runs.rs` |
| tool_result `[model: <name>]` 行已存在（A 落地），`format_dispatch_result_with_model` —— dispatch 临时 model 也应反映 | `agent/subagent/truncate_summary.rs` |
| catalog = `HashMap<model_id, Arc<dyn Provider>>`，key 是 UUID | `state.rs:60` |
| `db::models::get_model` 一次返回 `ModelRow { id, display_name, context_window, supports_thinking, ... }` | `db/models.rs:109` |
| `list_subagents` IPC 已映射 id↔display_name（C 落地），`set_subagent_model` 写回 frontmatter / DB | `commands/subagents.rs`（C 新建） |
| 前端 `useSubagentsStore` + `SubagentsTab` 已存在，模型下拉按 provider group | `app/src/components/settings/SubagentsTab.vue` |
| C 的 Out-of-scope 明确写 B 优先级 = **dispatch > DB > frontmatter > parent** | archive task `07-03-subagent-per-agent-model-ui/prd.md:39` |

## 已敲定决策（brainstorm 结论）

| 维度 | 取值 | 理由 |
|---|---|---|
| **范围** | 一次做完整 B（LLM path + `@@` path） | 与 C 的 Out-of-scope 描述一致；两路径共享同一优先级接入点，拆 task 合并冲突 > 管理成本 |
| **`--model=<X>` 取值** | id（UUID）或 display_name 都接受；后端 `list_models` 反查 display_name→id，多同名取首 + `warn!` | A 的 UUID 痛点已被 C 的 UI 解决，B 的 user path 是 raw 文本，不友好；反查成本低（~20 行 + 1 测） |
| **`--model=` 位置约束** | flag 必须紧跟 `@@agent` 之后、task 之前（git/cargo flag 语义） | task 中间的 `--model=` 不误解析（task 常含 flag 文本）；正则简单，解析鲁棒 |

边界推论（由决策导出，非新决策）：
- LLM path（R1）的 `model` 入参只接受 `model_id`（UUID）—— LLM 从 tool schema 的 enum/描述拿 id，不像人需要 display_name（schema 描述写明）。display_name 反查只服务 user `@@` path。
- 临时 override 不持久化：只对本次 dispatch 生效，不写 DB / frontmatter（永久改走 Settings UI / C）。

## Requirements

- **R1（LLM 入参 + 可发现性）**：`dispatch_subagent` tool schema 加 `model`（可选 string，**enum 值 = display_name**，人可读）。LLM 从 enum 选 display_name；后端反查 display_name→id。不传时走 `resolve_final_model`（DB > frontmatter > parent）。enum 动态构建（`definition_with_cache` 加 `models` 参数，chat_loop turn-0 快照 `list_models`）。
- **R2（user 前缀）**：`@@<agent> --model=<X> <task>` 前缀解析出 model，经 ForcedDispatch 透传到 `run_subagent`，语义同 R1。`<X>` 支持 id 或 display_name（反查 id）。flag 位置：紧跟 agent 名、task 之前；不在该位置的 `--model=` 按普通文本走。
- **R3（优先级）**：`dispatch_model > DB override > frontmatter > parent`。dispatch model 接在 `resolve_final_model` 之前（最优先）。
- **R4（失效兜底）**：dispatch 指定的 model 不在 catalog（被删 / display_name 拼错 / id 不存在 / 反查无果）→ 不失败，`warn!` + 降级到 `resolve_final_model` 的结果（DB/frontmatter），再无则 parent。
- **R5（可观测）**：dispatch 临时 model 命中时，`subagent_runs.model_display` 写该 model display；tool_result `[model:]` 行同样反映（与 A/C 一致，零特例）。
- **R6（前端解析容错）**：`@@` panel 不加 model 下拉（用户主要走 LLM path 或 Settings 默认）；`--model=` 文本解析工作；解析失败（无值 / 位置错）给清晰错误，不静默丢弃但也不误解析 task 文本。

## Acceptance Criteria

- [ ] **AC1（LLM dispatch model 命中）**：LLM 发 `dispatch_subagent({ subagent: "reviewer", task: "...", model: "<X-display-name>" })`（model 从 schema enum 选），后端反查 display_name→id，worker 实际打到 X 的 provider（≠ parent），`tool_result` 含 `[model: X-display]`。
- [ ] **AC2（优先级 dispatch > DB）**：reviewer 有 DB override=Y，LLM dispatch 传 model=X → worker 用 X（dispatch 胜）。
- [ ] **AC3（优先级 dispatch > frontmatter）**：reviewer frontmatter `model: Y`、无 DB，LLM dispatch 传 model=X → worker 用 X。
- [ ] **AC4（不传 model 零回归）**：LLM 不传 `model` → 行为 = 现状（`resolve_final_model`：DB > frontmatter > parent），A/C 全部 AC 不回归。
- [ ] **AC5（user `@@ --model` id）**：用户输入 `@@reviewer --model=<X-id> 审查这段代码`，worker 用 X。
- [ ] **AC6（user `@@ --model` display_name）**：用户输入 `@@reviewer --model=GPT-4o 审查这段代码`，前端反查 display_name → id，worker 用 GPT-4o；多同名取首 + `console.warn`。
- [ ] **AC7（失效 dispatch model 兜底）**：dispatch 传一个不存在的 display_name / id（反查无果）→ dispatch 不失败，降级到 `resolve_final_model` 结果，`warn!` 记录。
- [ ] **AC8（model_display 持久化 + 可见）**：dispatch 临时 model 命中时 `subagent_runs.model_display` 记该 model display；card chip / drawer 显示与 A/C 路径一致（无特例分支）。
- [ ] **AC9（前端解析容错）**：`--model=` 不在 flag 位置（task 中间）→ 不误解析，整段当 task（无 model override）；`@@agent --model=` 后无值 → 解析失败报清晰错误（不静默丢弃）。
- [ ] **AC10（回归）**：`resolve_final_model` / `resolve_worker_provider` / `format_dispatch_result_with_model` 既有测试全绿；`tests_subagent.rs` / `tests_agent_loop.rs` / `tests_chat.rs` 既有用例不回归。
- [ ] **AC11（全绿）**：`cargo test --lib`（带 `PKG_CONFIG_PATH`）+ `vue-tsc --noEmit` + `vitest run` 全绿。
- [ ] **AC12（LLM 可发现性）**：`dispatch_subagent` tool schema 的 `model` 属性含动态 enum（值 = 当前所有 model 的 display_name），LLM 不必猜 UUID；会话内 CRUD model 的 enum 滞后可接受（下会话刷新 + 失效兜底）。

## Out of Scope

- **per-dispatch model 不持久化**：临时 override 只对本次 dispatch 生效，不写 DB / 不写 frontmatter（R1/R2 语义）。要永久改走 Settings UI（C）。
- **`@@` panel 的 model 下拉 UI**：用户走 LLM path 或 Settings 默认；`--model=` 是文本 flag，MVP 不在 panel 加下拉。若后续发现高频，可加（follow-up）。
- **`--other-flag` 泛化**：只解析 `--model=`，不引入通用 flag parser。
- **worker 自身不能选自己的 model**：worker 的 `dispatch_subagent` 被 `STRUCTURALLY_DISABLED`（no nesting），故 model 选择权只在 parent（LLM 或 user）。
- **display_name 歧义报错 vs 取首**：决策 = 取首 + `warn!`（不报错）。display_name 应唯一但 DB 不强约束；取首保证可用性。

## Notes

- 前序任务：A（`archive/2026-07/07-03-subagent-frontmatter-model`）+ C（`archive/2026-07/07-03-subagent-per-agent-model-ui`）。本 task 复用 A 的 `resolve_worker_provider` + C 的 `resolve_final_model`，**两者签名不动**，只在 `resolve_final_model` 之前加一层 dispatch override。
- 核心 spec：`.trellis/spec/backend/agent-loop-architecture.md`（优先级表）+ `tool-contract.md`（dispatch_subagent schema）+ `subagent-runs-schema.md`（model_display）+ `frontend/chat.md`（`@@` 解析）。
- 技术设计见 `design.md`，执行清单见 `implement.md`。
