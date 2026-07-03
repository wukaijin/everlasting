# subagent 多模型支持(A): frontmatter model 声明

## Goal

让 subagent（worker）能脱离 parent 当前选用的模型，按自身定义独立指定一个模型——典型场景是**跨模型对抗性 code review**（用一个更强 / 不同家族的模型审 parent agent 的产出，抗同模型自确认偏差）。

## 背景与动机

当前 `run_subagent` 强制复用 parent 的 `Arc<dyn Provider>`（`agent/subagent/dispatch.rs:238` 接收、`:716` 原样 `clone()` 喂嵌套 `run_chat_loop`）。`SubagentDef`（`agent/subagent/mod.rs:340`）字段只有 `name / description / system_prompt / tools / isolation`，**无 model 字段**；`dispatch_subagent` tool schema 也只有 `subagent / task / isolation`。整个链路没有任何"worker 换模型"的岔路，对抗检查无法实现。

但基础设施已就绪：`ProviderCatalog = HashMap<model_id, Arc<dyn Provider>>`（`state.rs:60`）把所有 model→provider 预 build 好；CRUD 后 `rebuild_catalog` 自动刷新。本 task 只需把 catalog 穿透进 `run_subagent`。

## 范围

### In-scope（A MVP）

1. `SubagentDef` 增 `model: Option<String>` 字段（值为 `models.id`，即 catalog key）。
2. `subagent/loader.rs` frontmatter 解析 `model:` 字段（沿用现有 inline-array 手写 parser，不引 YAML 库）。
3. `run_subagent` 签名增 `catalog` 参数；lookup 出 def 后按 `def.model` 查 catalog 替换 provider（None → 继承 parent）。
4. worker `context_window` 跟随 worker model：`def.model` 命中时查 `db::models::get_model(mid)` 取 `ModelRow.context_window`（catalog 结构不动，隔离方案）。
5. fallback：`def.model` 不在 catalog（model 被删 / provider 缺 key 被 build 跳过）→ `warn!` + 降级回 parent provider + parent context_window。
6. `dispatch_subagent` tool_result 增一行 `[model: <display_name>]` 信号，让 parent LLM 知道 worker 实际用了哪个模型（对抗场景的关键可见性）。
7. 调用点改造：`chat_loop.rs` serial dispatch 拦截 + `@@` forced dispatch 短路两处，传入 `state.catalog`。
8. 单测：model 命中换 provider / model 不在 catalog 降级 / model=None 继承 parent / context_window 跟换 / tool_result 含 `[model:]` 行。

### Out-of-scope（已进 ROADMAP `B6+` 或 follow-up）

- **B**：dispatch 时动态选模型（`dispatch_subagent` 加 `model` 参数 + `@@agent --model=`，优先级 dispatch>frontmatter>parent）→ ROADMAP `B6+` B。
- **C**：UI 配置 per-agent 默认模型（Settings → Subagents 下拉，写回 frontmatter / builtin DB override，顺带解决 `model:` 写 UUID 不友好的问题）→ ROADMAP `B6+` C。
- **frontmatter display_name 解析**：随 C 一起做（UI 下拉把 id↔display_name 映射掉），MVP 只认 `models.id`。
- **builtin agent 的 model 默认**：`researcher` / `general-purpose` 保持 `model=None`（继承 parent），零回归。

## 约束

- 不改 `ProviderCatalog` 结构（保持 `HashMap<model_id, Arc<dyn Provider>>`），避免传染 chat 入口（parent 路径）。
- 不在 loader 引 DB 查询，保持其 mtime-fenced 纯文件扫描性质；display_name 解析归 C。
- 不破坏现有 dispatch_subagent 的 tool_use/tool_result 配对不变量（RULE-A-007）、worker 取消传播、worktree 隔离决策。
- 全中文 user-facing 文案（与项目惯例一致）。

## Acceptance Criteria

- [ ] **AC1**：在 `<project>/.everlasting/agents/reviewer.md` 写 `model: <某非 default 的 model_id>`，dispatch `reviewer` 后，worker 的 LLM 请求实际打到该 model（用 MockProvider 或 tracing 验证 provider 实例与 parent 不同）。
- [ ] **AC2**：frontmatter 不写 `model:`（或 `model: null`）时，worker 行为与现状完全一致（继承 parent provider + parent context_window）——builtin `researcher` / `general-purpose` 零回归。
- [ ] **AC3**：frontmatter `model:` 指向一个已删除的 model_id（不在 catalog）时，dispatch 不失败，降级到 parent provider，`tracing` 有 `warn!` 记录，worker 正常完成。
- [ ] **AC4**：worker model 的 `context_window` 与 parent 不同时，C3 压缩阈值按 worker model 的 context_window 计算（非 parent 的）。
- [ ] **AC5**：`dispatch_subagent` tool_result 内容包含 `[model: <display_name>]` 行；`model=None` 时该行显示 parent 的 model display_name（或明确标注 `inherited`）。
- [ ] **AC6**：worker model `supports_thinking=false` 而 parent `=true` 时，worker 不发送 thinking 块（capability 跟随 provider caps，加测试验证无泄漏）。
- [ ] **AC7**：现有 `tests_subagent.rs` / `tests_agent_loop.rs` 相关用例全绿，新增 5+ 用例覆盖上述 AC。
- [ ] **AC8**：`cargo test --lib` + `vue-tsc --noEmit`（若有前端改动）全绿；`PKG_CONFIG_PATH` 按项目 HACKING 设。

## Notes

- B/C 已进 `docs/ROADMAP.md` 第三档 `B6+`；本 task 仅 A MVP。
- 技术设计与执行计划见 `design.md` / `implement.md`。
