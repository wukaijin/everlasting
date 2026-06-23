# 拆分 permissions/mod.rs — 抽 check.rs + AuditKind 按域分

## Goal

`app/src-tauri/src/agent/permissions/mod.rs` 已涨到 **2814 行**，单文件承担权限决策 + store + audit + payload + 测试 5 个关注点。按职责拆成多文件模块，降低单文件认知负荷，与刚完成的 `agent/tests.rs` / `db/tests.rs` 拆分批次保持一致风格。

## What I already know (真实结构 inventory)

### 生产代码 L1-1751（~1751 行）

| 切片 | 行范围 | ~行数 | 内容 |
|------|--------|------|------|
| imports | 56-64 | 9 | use 声明 |
| `Risk` | 84-150 | 67 | enum + impl + `risk_for_tool` |
| `AuditKind` | 152-267 | 115 | enum **17 variant** + impl |
| `Decision` | 268-302 | 35 | enum + impl |
| types | 304-380 | 77 | `PermissionContext` + `PermissionResponse` + `PendingAsk` |
| store | 382-453 | 72 | `new_permission_store` + `register_ask` + `resolve_ask` + `cancel_session_asks` |
| payload | 455-510 | 56 | `PermissionAskPayload` + `ASK_TIMEOUT` |
| `WorkerAskTerminal` | 512-550 | 39 | enum |
| **check core** | 552-791 | 240 | `check` 主函数（5-tier 决策） |
| classify helpers | 792-841 | 50 | `ToolKind` + `classify_tool` + `extract_path_arg` |
| grant checks | 843-1045 | 203 | `check_path_grant` + `check_tool_grant` + `check_prefix_grant` |
| **ask_path** | 1047-1472 | **426** | `ask_path`（巨石） |
| ask reason | 1473-1562 | 90 | `build_ask_reason` |
| audit | 1564-1697 | 134 | `record_audit` + `record_tool_executed_audit` + `record_message_resend_audit` |
| mode | 1699-1750 | 52 | `mode_system_prefix` + `filter_tools_for_mode` |

**已有 sibling 子模块**：`pub mod dangerous`（L53）、`pub mod shell_trust`（L54）—— 拆分时注意它们的依赖方向。

### 测试 L1752-2814（~1062 行，`mod tests`）

纯函数测试（mode / risk / classify / extract_path / glob / match_value / build_ask_reason / payload wire shape）+ async worker 集成测试（`worker_ask_*` 系列，含 `CaptureAskSink`、`worker_ctx_with_db`、`LocalSink` 等 fixture）。

## description 原方案 vs 真实结构的 gap

原方案（task.json description）：

1. `check.rs`（5-tier 主函数 297 行 + helpers）
2. `store.rs`（PermissionStore + register/resolve/cancel）
3. `audit_kinds.rs`（AuditKind 17 variant 按域拆）
4. `payload.rs`（PermissionAskPayload）
5. `mod.rs` 只留 re-exports

**三个 gap**：

- **测试 1062 行完全没提** —— 必须决定拆不拆、怎么拆（沿用 `agent/tests.rs` 的"按域拆 + `tests_common.rs`"模式？）。
- **`check.rs` 行数估算严重偏低**：原方案把 `ask_path`(426) + grant checks(203) + classify(50) + build_ask_reason(90) 都当 "helpers" 塞进 check.rs → check.rs ≈ **1009 行**，没真正瘦身。
- **小切片归属未定**：`Risk`(67) / `Decision`(35) / types(77) / mode(52) / `WorkerAskTerminal`(39) 这五块原方案没明确落到哪个文件。

## Assumptions (temporary)

- 拆分是**纯行为保持**重构（无逻辑改动），`cargo test --lib` 全绿即等价。
- 拆完沿用同批 `agent/tests.rs` 拆分风格：内联 `#[cfg(test)] mod tests` 或独立 `*_tests.rs` + `tests_common.rs`。
- 不引入新 crate / 新依赖，模块仍在 `agent::permissions` 命名空间下。

## Decision (ADR-lite)

**Context**: 2814 行单文件，生产 1751 + 测试 1062。需选拆分边界。

**Decision**: 方案 A — 细拆 8 文件 + 平铺测试。已 resolve 的点：

- **[Q1] `ask_path`(426) 独立成 `ask.rs`** ✅ —— 不留 check.rs，避免 check.rs ≈1009 行没真正瘦身。
- **[Q2] 测试拆分方式** ✅ —— 对齐 `agent/` 既有惯例：**平铺 `tests_*.rs` + `tests_common.rs`**（`pub mod tests_*` 声明 + 文件头 `#![cfg(test)]`），**非** `tests/` 子目录。按域拆：`tests_check.rs` / `tests_ask.rs` / `tests_audit.rs` / `tests_store.rs` / `tests_payload.rs` / `tests_types.rs`。
- **[Q3] 小切片归属** ✅ —— `Risk`/`Decision`/`PermissionContext`/`PermissionResponse`/`PendingAsk`/`ToolKind`/`WorkerAskTerminal` → `types.rs`；`mode_system_prefix`+`filter_tools_for_mode` → `mode.rs`。
- **[Q3b] `PermissionStore` 是 `type` alias（L379）** ✅ —— 归 `store.rs`，保持 `pub` 可见性。
- **[Q3c] `AuditKind` 保持单 enum** ✅ —— 17 variant 在 `audit.rs` 内按 Tool/Permission/Mode/Message/Worker 域**排列 + `// === Tool 域 ===` 注释分组**，**不**拆多 enum（保 `record_audit` 签名 + serde tag 落 DB + 前端 C4 UI 解析）。`build_ask_reason` 归 `ask.rs`（与 `ask_path` 同源），不放 payload.rs。
- **[Q4] PR/commit 切分** ✅ —— 对齐同批惯例（`099a7b3` tests.rs 拆 / `6db0938` chat_loop 抽 dispatch 均**单 commit 一次拆完**）：本任务单 commit 一次拆完。

**Consequences**: 改动面最大（8 文件 + 6 测试文件 move），但纯行为保持，`cargo test --lib` 全绿即等价；对外 `pub` API 路径全部不变（`permissions::check` / `permissions::record_*` / `new_permission_store` 等），`agent/tests_common.rs` 的 `use crate::agent::permissions::new_permission_store` 不受影响。

## Implementation Plan (单 commit，按依赖顺序 move)

1. 建 `types.rs`（所有 enum/struct/type 搬入）→ `store.rs`（PermissionStore alias + 4 函数）→ `payload.rs`（PermissionAskPayload + ASK_TIMEOUT）→ `mode.rs`
2. 建 `audit.rs`（AuditKind + 17 variant 重排注释 + 3 record 函数）
3. 建 `check.rs`（check 主函数 + classify/grant helpers）→ `ask.rs`（ask_path + build_ask_reason）
4. `mod.rs` 收敛为 `pub mod`/`pub use` re-exports
5. 测试拆 `tests_common.rs`（fixtures：CaptureAskSink/worker_ctx_with_db/LocalSink）+ 6 个 `tests_*.rs`
6. `cargo test --lib`（PKG_CONFIG_PATH）全绿验证

## Requirements (evolving)

- 拆分后单文件 ≤ ~600 行目标（除测试）。
- `mod.rs` 收敛为 re-exports + 子模块声明。
- 外部调用方（`agent/` 其他模块对 `permissions::check` / `permissions::record_*` 等的引用）路径不变。

## Acceptance Criteria (evolving)

- [ ] `cargo test --lib`（含 PKG_CONFIG_PATH）全绿，测试数量不减。
- [ ] 拆分前后 `permissions` 模块对外 public API（`cargo doc` 或 grep `pub` 导出）集合不变。
- [ ] 单文件行数符合目标。
- [ ] DEBT.md 如有相关 RULE 更新引用。

## Definition of Done

- Rust 单元/集成测试全绿
- `cargo check` 绿（PKG_CONFIG_PATH）
- 无对外 API 破坏（纯内部重组）
- prd.md 收口 + jsonl curated → `task.py start` → 实现 → check → archive（四段式 commit）

## Out of Scope (explicit)

- 不改权限决策逻辑（5-tier 语义、Risk 分级、glob 匹配规则保持不变）。
- 不重构 `dangerous.rs` / `shell_trust.rs`（仅调整它们对 mod.rs 的 import 路径若必要）。
- 不做 API key 加密（那是 DEBT `RULE-D-001`，独立任务）。

## Technical Notes

- 编译/测试命令：`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`（见 CLAUDE.md WSL 段）。
- 同批参考：`agent/tests.rs` 已拆成 5 域文件 + `tests_common.rs`（commit `099a7b3`）。
- AuditKind 17 variant 按 description 建议按 Tool / Mode / Message / Worker 域分组（但仍是同一 enum，分组是注释/排列层面）。
