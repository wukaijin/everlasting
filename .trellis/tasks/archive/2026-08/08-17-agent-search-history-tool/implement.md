# implement.md — D2② search_history tool

单 PR 体量(纯增量,预计 +350/-5 含测试)。按序执行,每步可独立验证。

## 步骤

1. **`tools/search_history.rs` 新建**(照 `remember.rs` 结构:模块 docstring 权限段
   + `definition()` + `execute()` + 纯函数 `parse_args` / `format_hits` +
   `#[cfg(test)]`)
   - `use crate::db::search::{search_messages, MessageSearchHit, SearchHitKind}`
   - `execute(input, ctx, session_id) -> (String, bool)`,内部 `parse_args` →
     `search_messages(&ctx.db, ...)` → `format_hits`
2. **`tools/mod.rs` 接线**
   - `pub mod search_history;`
   - `builtin_tools()` 末尾 `search_history::definition(),` + 注册注释(D2②、
     Tier 5 silent Allow 同 remember、复用 db::search 共享层、非 stub 候选)
   - `execute_tool_inner` 新 arm(照 grep/list_dir 形状,session_id 透传)
3. **`agent/subagent/tools_filter.rs`**:`READONLY_TOOL_ALLOWLIST` +
   `"search_history"`;更新 docstring 里「researcher allowlist 恰等于
   READONLY_TOOL_ALLOWLIST」表述(researcher 仍 5 项,不再相等 —— 注释说明
   search_history 是 tool 层新增的只读成员,researcher 未跟进是 R5 决策)
4. **测试**(search_history.rs 内 `#[cfg(test)]`,pool 用
   `crate::tools::test_default_pool` + 照 `db/search_tests.rs` 的 seed 方式
   INSERT projects/sessions/messages):
   - definition 形状(name / 3 参数 / 必填仅 query)
   - happy path:FTS 命中格式化输出(序号/日期/标题/#seq/role/snippet)
   - scope=current_project 过滤生效;默认 all 跨 project
   - limit:默认 20、clamp 50(用 limit>50 入参断言行数 ≤ 50,或直接断言 clamp
     纯函数;seed 足量行成本高 → clamp 单独纯函数测)
   - 空 query → is_error true;非法 scope → is_error true
   - 零命中文案 + is_error false
   - (this session) 标记(session_id 传入命中行)
   - 2 字中文 LIKE 兜底路径(继承验证一条,锁 agent 侧不破坏短查询)
5. **回归验证**
   - `cargo test -p everlasting --lib search_history`(scoped)
   - `cargo test -p everlasting --lib`(全量,PKG_CONFIG_PATH 见 AGENTS.md)
   - `cargo fmt --check` / `cargo clippy -p everlasting --lib`
   - 前端零改动 → `pnpm test` 不必跑(wire 无触碰;若 check 发现 tool list 快照
     类前端测试引用 builtin 集合则补跑)
6. **spec 更新**:`.trellis/spec/backend/tool-contract.md` 增 `search_history`
   Scenario(AC8 四要素)

## 风险 / 回滚点

- 唯一外溢面 = `builtin_tools()` 集合变化,可能波及枚举该集合的既有测试
  (tests_mode / tests_subagent 断言 tool 数量或成员)→ 步骤 5 全量兜底。
- 回滚:单 commit revert,无 DB / wire 兼容面。
