# design.md — D2② search_history tool

## 1. 边界与契约

三层解耦(承接 ① design.md §「三层解耦点」):SQL 层 `db::search::search_messages`
**零改动**;本任务只加 tool 薄封装(输入解析 + 输出格式化);不碰 IPC /
SearchModal / 前端。

```
LLM → search_history({query, scope?, limit?})
    → tools/search_history.rs::execute     (新;输入解析 + 文本格式化)
    → db::search::search_messages          (复用,零改动)
    → messages_fts FTS5 / LIKE 兜底        (复用,零改动)
```

### 1.1 输入解析(纯函数,单测友好)

```rust
struct SearchHistoryArgs { query: String, scope: Scope, limit: u32 }
enum Scope { All, CurrentProject }
```

- `query` 缺失 / trim 空 → `(is_error: true)` 可行动文案。
- `scope` 非法值 → `is_error: true`(不静默兜底 —— 静默成功是 ① hotfix 的教训;
  但 schema enum 会让 provider 侧就拒绝,tool 层防御性校验兜底)。
- `limit` 非法(≤0 / 非整数)→ 用默认 20(参数性质宽松,与 scope 的严格不同:
  limit 错值不改变语义方向)。clamp 上界 50(tool 层常量 `MAX_AGENT_LIMIT`,
  不复用 db 层 MAX_LIMIT=200 —— 两侧上界是有意不同)。

### 1.2 输出格式化(纯函数)

```
Found 3 hits for "worktree" (scope: all projects, limit 20):
1. [2026-08-17] everlasting / D2 跨 session 搜索 · #4 assistant (this session): …前 48 + 后 96 字窗…
2. [2026-07-27] everlasting / review epic C3 · #142 assistant: …snippet…
3. [title] [2026-08-15] other-project / memory 治理 — session 标题命中,无消息体
```

- 日期取 `updated_at`(session 级,非消息级 —— `MessageSearchHit` 只有 session
  updated_at;消息级时间不在 hit 结构里,不为 agent 侧扩结构)。日期截 `T` 前
  10 位。
- `#seq` + role 帮模型判断「这是用户说的还是 assistant 说的」。
- `(this session)`:`execute` 的 `session_id` 参数与 hit.session_id 相等时追加
  (execute_tool_inner 已把 session_id 透传给部分工具;search_history 接收它)。
- `[title]` 行无 seq/role/snippet,标注 kind 防模型误读。
- 零命中:`is_error: false` + `No hits for "q". Try a longer distinctive phrase
  (FTS needs ≥3 chars; shorter queries use substring match), or raise limit.`

### 1.3 description(schema 给 LLM 看的)

克制(~40 词):说明搜什么(全部 project 的历史消息全文 + session 标题)、何时用
(找过往讨论/决策/解法)、返回形态(每 hit 一行摘要)。**不**写参数教程 —— schema
enum 自解释。

## 2. 接线点(全部既有模式,零新机制)

| 环节 | 位置 | 动作 |
|------|------|------|
| 注册 | `tools/mod.rs::builtin_tools()` | 末尾追加 + 注释(D2② / Tier 5 / 复用 db::search) |
| 分发 | `tools/mod.rs::execute_tool_inner` | 新 match arm(普通路径,非拦截) |
| mod 声明 | `tools/mod.rs` | `pub mod search_history;` |
| 并发 worker | `agent/subagent/tools_filter.rs::READONLY_TOOL_ALLOWLIST` | + `"search_history"` + 更新「恰等于 researcher」过时注释 |
| 权限 | 无 | `ToolKind::Other` → Tier 5 silent Allow;`risk_for_tool` `_` 默认 Low |
| plan 模式 | 无 | `filter_tools_for_mode` 黑名单不含 → 自动保留 |
| C7D stub | 无 | 非 `STUB_CANDIDATES`;stub gate 不受影响 |
| 群聊 | 无 | `GROUP_CHAT_RESEARCH_TOOLS` 不加(R6) |
| chat_loop | 无 | 非拦截 tool,无 `tool_name ==` 特判 |

`run_chat_loop` 签名 / `ToolContext` 字段零改动(db / project_id / session_id 全部
已到位:db 是 L3b PR3 加的,project_id 是 P2 remember 加的)。

## 3. 关键决策

- **D1 直查不走 IPC**:`execute` 持 `&ToolContext`(有 `db: SqlitePool`),与
  `remember`/`merge_worker` 同模式;IPC `search_messages` 6 处注册链零触碰。
- **D2 scope 用 enum 而非 project_id 透传**:agent 不知道(也不该知道)project UUID;
  `current_project` 由 `ctx.project_id` 解析,语义落在 tool 层。
- **D3 limit 上界 50 ≠ modal 200**:tool_result 进 LLM 上下文,20×~150tok ≈ 3k tok
  是合理单次预算;modal 是 UI 分页给人看的。两侧常量独立,不共享。
- **D4 不加 `include_current_session` 参数**:当前 session 命中以 `(this session)`
  标记软引导跳过,加参扩 schema 不值。
- **D5 title 命中保留**:对 agent 定位「哪个 session 聊过这事」有用(用户问的
  「之前那次讨论」常常指 session 粒度),且免费搭车。

## 4. 兼容 / 回滚

- 纯增量:新 tool 文件 + 3 行注册 + allowlist 1 项。回滚 = revert 单 commit。
- DB 零 migration(messages_fts 已随 ① 建立,存量已回填 1192=1192)。
- worker / 群聊 / plan 全部由既有过滤器天然裁决,无新组合态。
