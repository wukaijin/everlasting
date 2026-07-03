# Design — subagent 多模型支持(A): frontmatter model 声明

> 配套 `prd.md`。本文聚焦技术设计、契约变更、关键决策与回滚。B/C 不在本文范围。

## 1. 现状回顾（已验证事实）

| 事实 | 位置 |
|---|---|
| `run_subagent` 接收 parent 的 `provider: &Arc<dyn Provider>`，`:716` 原样 `clone()` 喂嵌套 loop | `agent/subagent/dispatch.rs:238,716` |
| `run_subagent` 接收 `context_window: u32`（parent 的），`:717` 透传 | `agent/subagent/dispatch.rs:240,737` |
| `SubagentDef` 字段：`name / description / system_prompt / tools / isolation`（无 model） | `agent/subagent/mod.rs:340` |
| `ProviderCatalog = HashMap<String, Arc<dyn Provider>>`，key = `models.id` | `state.rs:60` |
| catalog build 时 `build_provider(provider_row, model_row)`，provider caps（含 `supports_thinking`）按 model 设置 | `state.rs:389`+、`llm/provider/mod.rs` |
| `db::models::get_model(pool, id) -> Option<ModelRow>` 一次返回 `context_window` + `supports_thinking` + `display_name` | `db/models.rs:109` |
| `models.id` = UUID（`Uuid::new_v4()`） | `db/models.rs:30`、`migrations.rs:251` |
| dispatch 调用点：`chat_loop.rs` serial 拦截 + `chat.rs` forced dispatch 短路 | `agent/chat_loop.rs:2374`、`agent/chat.rs:327-344` |
| loader 是 mtime-fenced 纯文件扫描，不持 DB | `agent/subagent/loader.rs` |

**关键洞察**：thinking capability 在 provider caps 里（catalog build 时按 `model.supports_thinking` 设），**不在 `run_chat_loop` 按 parent model 单独判断**。因此 worker 换 provider 后，capability 自动跟随 worker 的 provider——议题 3 风险比预想小，只需加测试验证。

## 2. 方案总览

```
frontmatter (model: <uuid>)
        │ loader.rs (纯文件扫描, 不查 DB)
        ▼
SubagentDef.model: Option<String>
        │ run_subagent 拿到 def 后
        ▼
┌───────────────────────────────────────────────┐
│ def.model?                                     │
│  ├ None  → worker_provider = parent provider   │
│  │        worker_ctx    = parent context_window│
│  └ Some(mid):                                  │
│      catalog.get(mid) ──┬─ Some(p)             │
│                         │   → worker_provider = p
│                         │   → worker_ctx = get_model(mid).context_window
│                         └─ None                │
│                             → warn! + 降级 parent (provider + ctx)
└───────────────────────────────────────────────┘
        │
        ▼  nested run_chat_loop(worker_provider, worker_ctx, …)
```

catalog 结构不动；context_window 走 `get_model(mid)` 单查 DB（隔离方案）。

## 3. 契约 / 接口变更

### 3.1 `SubagentDef`（`agent/subagent/mod.rs`）
```rust
pub struct SubagentDef {
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub tools: Vec<String>,
    pub isolation: Option<bool>,
    pub model: Option<String>,   // ← 新增：models.id (catalog key)
}
```
- builtin 两个 agent `model: None`（mod.rs:378 / :414 两处构造点）。
- 所有 `SubagentDef { ... }` 字面量构造点（含 `tests_*.rs` fixture）需补 `model: None`。

### 3.2 `subagent/loader.rs` frontmatter 解析
- 在现有 inline parser 的字段读取处加 `model:` 单字符串字段（与 `name` / `description` 同类，非 `tools`/数组）。
- frontmatter 示例：
  ```yaml
  ---
  name: reviewer
  description: 跨模型对抗性 code review
  tools: [read_file, grep, glob, list_dir]
  model: 550e8400-e29b-41d4-a716-446655440000
  ---
  ```
- 解析失败 / 字段缺失 → `model: None`（容错，沿用 loader 现有全容错策略）。

### 3.3 `run_subagent` 签名（`agent/subagent/dispatch.rs`）
新增一个参数（插在 `provider` 附近，语义聚合）：
```rust
pub(crate) async fn run_subagent(
    provider: &Arc<dyn Provider>,
    catalog: Arc<RwLock<ProviderCatalog>>,   // ← 新增
    context_window: u32,
    ...
)
```
- lookup 出 `def` 后立即解析 worker provider + context_window：
  ```rust
  let (worker_provider, worker_ctx, model_display): (Arc<dyn Provider>, u32, String) =
      match def.model.as_deref() {
          None => (provider.clone(), context_window, /* parent display */),
          Some(mid) => {
              let cat = catalog.read().await;
              match cat.get(mid).cloned() {
                  Some(p) => {
                      drop(cat);
                      let ctx = get_model(db, mid).await
                          .ok().flatten().map(|m| m.context_window)
                          .unwrap_or(context_window);
                      let disp = get_model(db, mid).await...display_name;  // 合并成一次查询
                      (p, ctx, disp)
                  }
                  None => { warn!(...); (provider.clone(), context_window, /* parent+降级标 */) }
              }
          }
      };
  ```
  （实现时把 `get_model` 合并成一次调用，取 `ModelRow` 同时拿 `context_window` + `display_name`，避免双查。）
- 嵌套 `run_chat_loop` 调用：`provider.clone()` → `worker_provider.clone()`；`context_window` → `worker_ctx`。

### 3.4 tool_result 信号行（`agent/subagent/truncate_summary.rs`）
- `format_dispatch_result` 增 `model_display: &str` 参数，在 `[status: ...]` 前缀后追加 `\n[model: <display_name>]`。
- `run_subagent` 调用处传入解析出的 `model_display`。
- `model=None` 时 display 取 parent 的 model display_name（由 chat 命令入口透传，或标 `[model: <parent> (inherited)]`）。

### 3.5 调用点改造
- `chat_loop.rs` serial dispatch 拦截分支：`run_subagent(...)` 实参增 `state.catalog.clone()`（已是 `Arc<RwLock<...>>`，clone 廉价）。
- `chat.rs` forced dispatch 短路分支：同上。
- parent display_name 透传：chat 命令入口已有 `model_display_name`（`ResolvedChatProvider`），需一路 thread 到 `run_subagent`（若当前未传，新增一个 `parent_model_display: &str` 参数）。

## 4. 关键决策（3 议题拍板）

### 决策 1：frontmatter `model:` 认 `models.id`（UUID），不做 display_name 解析
- **理由**：loader 是 mtime-fenced 纯函数，引 DB 查询破坏其性质；id 不友好的真正解药是 C（UI 下拉）。
- **代价**：MVP 用户需从 Settings → Models 复制 model id。文档（`docs/` 下 subagent 相关 + frontmatter 示例）写明。
- **回退**：若 MVP 期间发现 UUID 写法严重影响可用性，提前做 C 的"id↔name 映射"子集（仍不放在 loader，放 run_subagent 的 catalog 查表后回退）。

### 决策 2：catalog 结构不动，`context_window` 隔离单查 DB
- **理由**：catalog 升级成 `CatalogEntry { provider, context_window }` 会改 chat 入口（parent 路径），传染大；dispatch 是低频操作，一次 `get_model` roundtrip 可接受。
- **实现**：`def.model` 命中时 `get_model(db, mid)` 取 `ModelRow`，同时拿 `context_window` + `display_name` + `supports_thinking`（一次查询，三用）。

### 决策 3：thinking capability 跟随 provider caps，不标 TODO，加测试验证
- **依据**：`supports_thinking` 在 provider caps（`llm/provider/mod.rs`），catalog build 时按 model 设。换 provider 自动切换，`run_chat_loop` 不按 parent 重新判断。
- **MVP 动作**：加测试 AC6——构造 parent=`supports_thinking:true` model + worker=`supports_thinking:false` model，验证 worker 请求 body 不含 thinking 块。
- **风险残留**：若测试发现 parent 的 thinking 配置经 `run_chat_loop` 其他路径泄漏给 worker，则就地修复（不入 follow-up）。

## 5. 失败模式与 fallback

| 场景 | 行为 |
|---|---|
| `def.model` 指向的 model_id 不在 catalog（被删 / provider 缺 key 被 build 跳过） | `warn!` + 降级 parent provider + parent context_window，worker 正常跑（AC3） |
| `def.model` 命中 catalog 但 `get_model` DB 查询失败 | `warn!` + 用 parent context_window，provider 仍用 worker 的（catalog 命中说明 provider 有效） |
| loader 解析 `model:` 字段格式异常 | `model: None`（容错），等效未声明 |
| catalog RwLock 读锁竞争（CRUD 中） | 标准读锁等待，无特殊处理 |

## 6. 兼容性

- **builtin agent 零回归**：`researcher` / `general-purpose` `model=None`，走继承 parent 分支，行为 = 现状（AC2）。
- **frontmatter 向后兼容**：现有 user/project agent `.md` 不写 `model:` → `model=None` → 继承 parent，无破坏。
- **RULE-A-007（tool_use/tool_result 配对）**：tool_result 仅追加一行 `[model:]`，不改配对结构。
- **取消传播 / worktree 隔离**：不受 provider 替换影响（`worker_token` / isolation 决策与 provider 无关）。

## 7. 测试策略

新增用例（`agent/subagent/dispatch.rs` 内联 `#[cfg(test)]` 或 `tests_subagent.rs`）：
1. `worker_model_hit_swaps_provider` — `def.model` 命中 → worker provider ≠ parent（AC1）。
2. `worker_model_none_inherits_parent` — `model=None` → worker provider = parent（AC2）。
3. `worker_model_missing_falls_back_with_warn` — 不在 catalog → 降级 parent（AC3）。
4. `worker_context_window_follows_model` — 命中时 worker_ctx = model.context_window，≠ parent（AC4）。
5. `dispatch_result_includes_model_line` — tool_result 含 `[model: <name>]`（AC5）。
6. `worker_model_no_thinking_no_leak` — worker supports_thinking=false 时不发 thinking 块（AC6）。

用 `MockProvider`（已有，`llm/provider/mock.rs`）构造可区分实例；catalog 用临时 HashMap。

## 8. 回滚

- 全部改动集中在 `SubagentDef` + `run_subagent` + 两调用点 + loader 一个字段 + tool_result 一行。
- 回滚 = revert 该 task 的 commit 序列；builtin `model=None` 保证回滚前后行为一致，无数据迁移负担。
- 不涉及 DB schema 变更（`models` 表已有所需列），无需 migration 回滚。
