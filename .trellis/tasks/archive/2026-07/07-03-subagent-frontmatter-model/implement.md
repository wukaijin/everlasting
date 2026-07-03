# Implement — subagent 多模型支持(A): frontmatter model 声明

> 配套 `prd.md` / `design.md`。执行顺序按依赖关系排列；每步附验证命令。WSL 下所有 cargo 命令需带 `PKG_CONFIG_PATH`（见 CLAUDE.md HACKING-wsl）。

## 前置常量

```bash
PKG="PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
CD="cd /usr/local/code/github/everlasting/app/src-tauri"
```

## Step 1 — `SubagentDef` 加 `model` 字段 + 补全构造点

- [ ] `agent/subagent/mod.rs` `SubagentDef` 增 `pub model: Option<String>`（带 doc 注释：值为 `models.id`，None=继承 parent）。
- [ ] `builtin_subagents()` 两个 builtin 构造点补 `model: None`（mod.rs:378 / :414）。
- [ ] grep 全 crate `SubagentDef {` 字面量，逐个补 `model: None`（重点：`agent/subagent/dispatch.rs` 测试 fixture `writable_def`、`tests_subagent.rs`、`tests_agent_loop.rs`、`tests_common.rs`）。
- **验证**：`$CD && env $PKG cargo check 2>&1 | tail -20`（编译通过、无 `missing field` 错）。

## Step 2 — loader frontmatter `model:` 解析

- [ ] `agent/subagent/loader.rs`：在 inline parser 的标量字段读取段（`name` / `description` 同类）加 `model:` 解析，写入 `SubagentDef.model`。
- [ ] 容错：字段缺失 / 格式异常 → `None`（沿用 loader 全容错策略，不 panic、不 fail-fast）。
- [ ] 单测：`loader_parses_model_field` / `loader_missing_model_is_none` / `loader_project_overrides_builtin_model`（project 同名 agent 的 model 随 override 生效）。
- **验证**：`$CD && env $PKG cargo test --lib subagent::loader 2>&1 | tail -20`。

## Step 3 — `run_subagent` 签名 + worker provider/context_window 解析

- [ ] `agent/subagent/dispatch.rs` `run_subagent` 签名增 `catalog: Arc<RwLock<ProviderCatalog>>`（插在 `provider` 之后）。
- [ ] lookup 出 `def` 后、构造 `worker_messages` 前，解析三元组 `(worker_provider, worker_ctx, model_display)`：
  - `None` → `(provider.clone(), context_window, parent_model_display.into())`。
  - `Some(mid)` 命中 catalog → 一次 `get_model(db, mid)` 拿 `ModelRow`，取 `context_window` + `display_name`；`get_model` 失败 → `warn!` + fallback parent ctx。
  - `Some(mid)` 未命中 catalog → `warn!(model=mid, "subagent model not in catalog; falling back to parent")` + 降级 parent。
- [ ] 嵌套 `run_chat_loop` 实参：`provider.clone()` → `worker_provider.clone()`；`context_window` → `worker_ctx`。
- **验证**：`$CD && env $PKG cargo check 2>&1 | tail -20`（此时调用点未改会报参数缺失，先 Step 5 再全绿）。

## Step 4 — tool_result `[model:]` 信号行

- [ ] `agent/subagent/truncate_summary.rs` `format_dispatch_result` 增 `model_display: &str` 参数，在 `[status: ...]` 前缀后插 `\n[model: <display>]`（保留现有 partial-actions 段位置不变）。
- [ ] `run_subagent` 调用 `format_dispatch_result` 处传入 `model_display`。
- **验证**：`$CD && env $PKG cargo test --lib format_dispatch_result 2>&1 | tail -20`。

## Step 5 — 调用点改造 + parent display 透传

- [ ] `agent/chat_loop.rs` serial dispatch 拦截分支：`run_subagent(...)` 实参增 `state.catalog.clone()`；`parent_model_display` 从 chat 入口已解析的 display 拿（若未 thread 到此，沿调用链补一个参数或从 `current_ctx` 取）。
- [ ] `agent/chat.rs` forced dispatch（`@@`）短路分支：同上。
- [ ] 确认 parent `model_display_name` 一路可达 `run_subagent`（`ResolvedChatProvider.model_display_name` 已存在于 chat 命令入口）。
- **验证**：`$CD && env $PKG cargo check 2>&1 | tail -20`（全编译通过）。

## Step 6 — 单测覆盖 AC1–AC6

- [ ] `worker_model_hit_swaps_provider`（AC1，MockProvider 双实例）。
- [ ] `worker_model_none_inherits_parent`（AC2）。
- [ ] `worker_model_missing_falls_back_with_warn`（AC3）。
- [ ] `worker_context_window_follows_model`（AC4）。
- [ ] `dispatch_result_includes_model_line`（AC5）。
- [ ] `worker_model_no_thinking_no_leak`（AC6，需构造 supports_thinking 差异的两个 MockProvider / catalog 条目）。
- **验证**：`$CD && env $PKG cargo test --lib 2>&1 | tail -30`。

## Step 7 — 全量回归 + 前端

- [ ] `$CD && env $PKG cargo test --lib 2>&1 | tail -30`（含既有 `tests_subagent` / `tests_agent_loop` 全绿）。
- [ ] 若前端有改动（本 MVP 预计无，除非 SubagentDrawer 显示 model）：`cd app && pnpm exec vue-tsc --noEmit`。
- [ ] 手测（可选）：`.everlasting/agents/reviewer.md` 写 `model: <id>`，dispatch reviewer，观察 tool_result `[model:]` 行 + tracing warn（若造一个不存在的 id）。

## Review Gate（Step 7 后）

- 跑 `trellis-check`（spec 合规 + lint + 跨层数据流 + 一致性）。
- 全绿后 → Step 3.3 spec 更新（`docs/ROADMAP.md` §1.2 加本 task 落地条目，§2 `B6+` 备注里 A 从"待开 task"改"已落地见 §1.2"，B/C 仍列）。

## Rollback Point

- 每个 Step 是一个独立 commit（或 squash 时按 Step 切分）；任意 Step 失败可停在该 Step 修复，不污染前序。
- 最坏情况 revert 整个 task commit 序列；`models` 表无 schema 变更，builtin `model=None` 保证回滚 = 行为回到现状。

## Follow-up（不入本 task）

- B（dispatch 动态选模型）→ ROADMAP `B6+` B。
- C（UI 配置 + id↔display_name 映射）→ ROADMAP `B6+` C。
- 若 Step 6 AC6 测试发现 thinking 泄漏 → 就地修复（不单开 follow-up）。
