# Implement — B6+ B subagent dispatch 动态选模型

> 配套 `prd.md` / `design.md`。执行顺序按依赖关系排列；每步附验证命令。WSL 下所有 cargo 命令需带 `PKG_CONFIG_PATH`（见 CLAUDE.md HACKING-wsl）。

## 前置常量

```bash
PKG="PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
CD="cd /usr/local/code/github/everlasting/app/src-tauri"
APP="cd /usr/local/code/github/everlasting/app"
```

## Step 0 — 核实 serde unknown-fields 行为（决策 6 兼容性）

- [ ] 确认 `ForcedDispatch` 当前是否有 `#[serde(deny_unknown_fields)]`（grep）。若无（默认），新前端发 `model_id` 到旧后端不会报错（serde 忽略未知字段）→ 无需 `#[serde(default)]`。若有，则加 `#[serde(default)]` 到 `model_id`。
- **验证**：`rg -n "deny_unknown_fields" app/src-tauri/src/agent/subagent/mod.rs`（预期：无匹配）。

## Step 1 — 后端：共享反查 + dispatch input 解析 + 优先级叠加（核心）

- [ ] `agent/subagent/dispatch.rs` 新增 `resolve_model_by_name_or_id(db, input) -> Result<Option<String>, sqlx::Error>`（design §3.8）：先 `get_model` 精确 id，miss 再 `list_models` 找 display_name 取首。
- [ ] `agent/subagent/dispatch.rs` §336 附近（`subagent_name` / `task` 解析后）加 `dispatch_model` 解析（design §3.2）：`input.get("model")` → 经 `resolve_model_by_name_or_id` 反查 display_name→id；未命中 → None + warn。
- [ ] `agent/subagent/dispatch.rs` §554 附近（`resolve_final_model` 调用点）改 `final_model = dispatch_model.clone().or(resolved_lower)`（design §3.3）。
- [ ] 命名清晰（`dispatch_model` 区别于 `def.model`）。
- **验证**：`$CD && env $PKG cargo check 2>&1 | tail -20`（编译通过）。

## Step 2 — 后端：`dispatch_subagent` tool schema 加 `model` 属性（enum=display_name）

- [ ] `agent/subagent/mod.rs` `definition()`（§188-219 测试用）schema `properties` 加 `model`（design §3.1）：enum=空 `[]`（静态版本无 models 上下文）。
- [ ] `definition_with_cache()`（§299-329 生产）schema `properties` 加 `model`，enum 动态构建。
- [ ] 新增 `ModelBrief { id: String, display_name: String }` struct（pub，`mod.rs`）。
- [ ] `definition_with_cache` 签名加 `models: &[ModelBrief]` 参数；enum = `models.iter().map(|m| m.display_name.clone()).collect()`。
- [ ] description 末尾追加 model 来源提示（design §3.1）。
- [ ] 既有 schema 测试（`definition_schema_*` / `definition_with_cache_*`）调用点补 `&[]`（测试用版本）；若断言 properties 数量则更新。预期 required + subagent enum 断言不受影响。
- **验证**：`$CD && env $PKG cargo test --lib definition 2>&1 | tail -20`。

## Step 3 — 后端：ForcedDispatch + chat_loop turn-1 注入

- [ ] `agent/subagent/mod.rs:137` `ForcedDispatch` 加 `pub model_id: Option<String>`（design §3.4）+ doc 注释。
- [ ] `agent/chat_loop.rs:937` turn-1 short-circuit 合成 input 时注入 `model`（design §3.5）。
- [ ] 核实 `ForcedDispatch` 所有构造点（grep `ForcedDispatch {`）—— 预期只有前端 wire + 测试，后端不构造（只反序列化）；若有后端测试构造点补 `model_id: None`。
- **验证**：`$CD && env $PKG cargo check 2>&1 | tail -20`。

## Step 3b — 后端：chat_loop turn-0 models 快照 + definition_with_cache 调用点

- [ ] `agent/chat_loop.rs` `run_chat_loop` 入口附近（turn 循环外）加 `model_briefs` 快照（design §3.9）：`list_models(&db)` 投影成 `Vec<ModelBrief>`，失败 warn + 空 vec。
- [ ] `agent/chat_loop.rs:1450` `definition_with_cache` 调用点加 `&model_briefs` 参数。
- [ ] 核实所有 `definition_with_cache` 调用点（grep）补 `&model_briefs`；worker path（`effective_is_worker` gate）不构造 dispatch_def，预期无 worker 调用点。
- **验证**：`$CD && env $PKG cargo check 2>&1 | tail -20`（编译通过）。

## Step 4 — 后端单测（design §7 后端 1-13）

- [ ] `dispatch_model_overrides_db_override`（AC2）。
- [ ] `dispatch_model_overrides_frontmatter`（AC3）。
- [ ] `dispatch_model_none_falls_to_resolve_final_model`（AC4 零回归 DB 路径）。
- [ ] `dispatch_model_none_no_db_no_frontmatter_inherits_parent`（AC4 parent）。
- [ ] `dispatch_model_missing_in_catalog_falls_back`（AC7）。
- [ ] `dispatch_input_model_display_name_resolved_to_id`（AC1，display_name 反查 → id，§3.2+§3.8）。
- [ ] `dispatch_input_model_unknown_display_name_ignored`（AC7 LLM path，反查无果 → None）。
- [ ] `resolve_model_by_name_or_id_id_passthrough`（§3.8，id 直返）。
- [ ] `resolve_model_by_name_or_id_display_name_lookup`（§3.8，display_name 取首）。
- [ ] `resolve_model_by_name_or_id_miss_returns_none`（§3.8，未命中 None）。
- [ ] `dispatch_input_model_parsed_from_json`（§3.2 入口）。
- [ ] `resolve_priority_chain_idempotent`（不持久化）。
- [ ] `definition_with_cache_model_enum_from_briefs`（§3.1/§3.9，enum=display_names）。
- **策略**：5-7 可针对 `final_model = dispatch_model.or(resolved_lower)` 纯叠加 + mock catalog/db；8-10 针对 `resolve_model_by_name_or_id` 纯函数（mock list_models/get_model）；6 直接构造 `serde_json::json!({"model": "..."})` 测解析；13 构造 `&[ModelBrief{...}]` 测 enum。
- **验证**：`$CD && env $PKG cargo test --lib dispatch_model 2>&1 | tail -30` + `cargo test --lib resolve_model_by_name 2>&1 | tail -10` + `cargo test --lib definition_with_cache 2>&1 | tail -10`。

## Step 5 — 前端：`@@` 解析 + resolveModelInput

- [ ] `app/src/stores/chat.ts:871-879` 正则改写为带 optional `--model=` 捕获组（design §3.6）。
- [ ] 新增 `resolveModelInput(raw, models): string | undefined` helper（design §3.6）：id 精确匹配优先 / display_name `find` 取首 + `console.warn`（多同名）/ 未命中返 undefined + warn。
- [ ] ForcedDispatch 局部类型加 `model_id?: string`。
- [ ] `app/src/stores/streamController.ts:1851` forcedDispatch 类型加 `model_id?: string`（design §3.7）。
- [ ] 核实 `useModelsStore` 在 `chat.ts send()` 作用域可见（若未 import，加 `useModelsStore()` 调用）。
- **验证**：`$APP && pnpm exec vue-tsc --noEmit 2>&1 | tail -20`。

## Step 6 — 前端单测（design §7 前端 8-14）

- [ ] 找现有 `chat.test.ts` / `chatForcedDispatch` 测试位置（若无可新建 `app/src/stores/__tests__/chatForcedDispatch.test.ts`）。
- [ ] `parses_model_flag_with_id`（AC5）。
- [ ] `parses_model_flag_with_display_name`（AC6）。
- [ ] `model_flag_in_task_middle_not_parsed`（AC9）。
- [ ] `model_flag_no_value_degrades`（AC9）。
- [ ] `resolveModelInput_unknown_returns_undefined`（AC7 前端）。
- [ ] `resolveModelInput_multiple_same_name_takes_first`（AC6）。
- [ ] `no_model_flag_omits_field`（AC4 零回归）。
- **验证**：`$APP && pnpm test --run 2>&1 | tail -30`。

## Step 7 — 集成测试 + 全量回归

- [ ] `tests_subagent.rs` 或 `tests_agent_loop.rs` 加 `forced_dispatch_with_model_swaps_provider`（design §7.15，端到端验证 ForcedDispatch model_id → run_subagent 实际换 provider）。
- [ ] `$CD && env $PKG cargo test --lib 2>&1 | tail -30`（含既有 `resolve_final_model` / `resolve_worker_provider` / `format_dispatch_result_with_model` 全绿）。
- [ ] `$APP && pnpm exec vue-tsc --noEmit && pnpm test --run 2>&1 | tail -30`。
- [ ] `cargo fmt --check`（`$CD && env $PKG cargo fmt --check`）。
- [ ] 手测（可选）：`@@reviewer --model=<某 id> 审查这段代码`，观察 worker 实际用该 model（card chip / drawer 显示）。

## Review Gate（Step 7 后）

- 跑 `trellis-check`（spec 合规 + lint + 跨层数据流 + 一致性）。
- spec 更新（Phase 3.3）：
  - `tool-contract.md` dispatch_subagent Scenario 加 `model` 属性 + 优先级说明。
  - `agent-loop-architecture.md` 优先级表（若有 dispatch row 则更新；无则加 dispatch > DB > frontmatter > parent）。
  - `frontend/chat.md` `@@` 解析段加 `--model=` flag 语义。
  - `subagent-runs-schema.md` 无改（model_display 已存在，dispatch path 共用）。

## Rollback Point

- 每个 Step 是独立 commit；任意 Step 失败可停在该 Step 修复。
- 核心改动 = `final_model = dispatch_model.or(resolved_lower)` 一行 + schema 一处 + ForcedDispatch 一字段 + 前端正则一段。
- 回滚 = revert 该 task commit 序列；无 DB schema 变更，无 migration，builtin 行为零回归。

## Follow-up（不入本 task）

- `@@` panel 的 model 下拉 UI（若用户反馈 `--model=` 文本 flag 不便）。
- per-dispatch model 持久化（当前刻意不持久化，永久改走 Settings C）。
- LLM path 的 `model` enum（若 LLM 频繁编造 id，可在 `definition_with_cache` 动态拼 model enum）。
