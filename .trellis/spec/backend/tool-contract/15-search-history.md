## Scenario: `search_history` — Agent 驱动跨 session 全文搜索(D2②,2026-08-17)

> 配套 task `08-17-agent-search-history-tool`。ROADMAP D2 双驱动的 ②:①
> 用户驱动 SearchModal(`08-17-cross-session-search`)与本文共享
> `db::search::search_messages` SQL 层;两侧只差 presentation(SQL 层 /
> presentation 层各自的边界在 ① design 已锁定)。

### 1. Scope / Trigger

- 用户问「上次怎么解的 X / 之前讨论过 Y 吗 / 那个决策是什么」——自主记忆
  (提炼后经验)与文件系统工具都答不了,需要原始对话全文检索。
- **纯后端增量**:新 `tools/search_history.rs` + `builtin_tools()` 末尾注册 +
  `execute_tool_inner` 普通 arm。IPC / 前端 / DB migration 零触碰。

### 2. 契约

```rust
// tools/search_history.rs
pub fn definition() -> ToolDef;   // name = "search_history"
pub async fn execute(input: &Value, ctx: &ToolContext, session_id: Option<&str>)
    -> (String, bool);
```

- **入参**:`query`(必填,trim 非空)/ `scope: "all" | "current_project"`
  (默认 all;`current_project` 由 `ctx.project_id` 解析 —— agent 永远看不到
  project UUID,用语义 enum)/ `limit`(默认 20,clamp 1..=50)。
  **scope 严格、limit 宽松**:scope 错值改变搜索方向 → `is_error: true`;
  limit 错值不改语义 → 静默用默认。
- **出参**:紧凑一行一 hit 的文本(非 `MessageSearchHit` JSON):
  `N. [日期] project / session 标题 · #seq role: snippet`;
  title 命中 `[title match]` 行;当前 session 命中追加 `(this session)`
  (模型上下文已有,软引导跳过)。零命中 = `is_error: false` + 建议文案。
- **limit 上界 50 ≠ modal 200**:每行 hit 落进 LLM 上下文,20 行 ≈ 3k tok
  是单次合理预算;两侧常量**有意独立**(modal 给人分页,tool 给模型预算)。

### 3. 权限与过滤链(全部零改动,靠既有裁决)

| 环节 | 裁决 | 依据 |
|---|---|---|
| Tier | 5 silent Allow | `classify_tool` 未列 → `ToolKind::Other` 默认(同 `remember`;只读 DB 查询无副作用) |
| Risk | Low | `risk_for_tool` `_` 默认 |
| Plan 模式 | 保留 | `filter_tools_for_mode` 只剥 write 类 |
| worker(serial) | 可用 | 不在 `STRUCTURALLY_DISABLED` |
| worker(并发 readonly) | 可用 | `READONLY_TOOL_ALLOWLIST` 含它(2026-08-17 第 6 员;builtin `researcher` 的硬编码 5 项**未**跟进 —— 其 system_prompt 枚举工具集,frontmatter agent 可自行声明) |
| 群聊 | 不可用 | `GROUP_CHAT_RESEARCH_TOOLS` 白名单不含(08-07 收敛决策,扩员留 follow-up) |
| C7D stub | 非候选 | 3 参数 schema;recall 类首次直用优于先 `load_tool_schemas` 再重试 |

注册序:**append 到 `builtin_tools()` 末尾** —— tools[] 顺序喂 provider 前缀
缓存,append 不移动既有前缀(与 C7 R3.2 稳定性前提一致)。

### 4. Token 预算涟漪(重要)

注册即 +178 tok(实测,classic-chat stubified 后 3677 → 3855),顶破 C7D
AC1 线 3700 → **二次校准 3900**(校准史见
[14-stub-registration](./14-stub-registration.md) §2;stub 化该工具只省 ~140
仍超线,故平移线而非 stub)。**后续每注册一个新 tool 都会撞这条线** —— 先
评估扩 `STUB_CANDIDATES`,平移线是最后手段。

### 5. Tests

`tools/search_history.rs` 内联 `#[cfg(test)]`(10 例):definition 形状 ×2 /
`parse_args`(默认、clamp、trim、非法 scope、空 query)×3 / execute 集成
(格式化输出含 project/标题/#seq/role/snippet/`(this session)`、scope 过滤、
title hit + 2 字中文 LIKE 兜底、零命中 vs 空 query)×4 / allowlist 成员
(`readonly_allowlist_keeps_search_history`)×1。种子走生产路径
(`create_project` / `create_session` / `persist_turn`),FTS 由 trigger 真实
索引(照 `db/search_tests.rs` 惯例)。

涟漪测试同步:`l3a_unit.rs` 守卫 5→6(改名 `…keeps_only_read_tools`,
防止未来新工具**无意识**漏进并发 readonly 集 —— 本次扩员是有意的,守卫
测试必须显式跟着改);`stub.rs` 静态预算线校准(§4)。
