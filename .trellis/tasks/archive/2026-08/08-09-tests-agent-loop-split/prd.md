# 测试文件拆分:tests_agent_loop.rs(5674 行)

## Goal

把 `app/src-tauri/src/agent/tests_agent_loop.rs`(5674 行)按被测功能簇拆分为目录化测试模块(`tests_agent_loop/` hub + 簇文件),改善可导航性。延续拆分专项惯例:纯搬迁铁律——复制 + 删源 + module 接线,**禁止顺手改逻辑**(批 1/2/3 与 A 类专项同款)。

> 批次衔接:批 1(`08-07-large-file-splitting`)PRD 是全仓 >1200 行文件拆分总纲,其 Out of Scope 把"既有独立测试文件的拆分"划出**当批**范围(仅批次区分,非永久排除);本任务落地该行——4 个测试大文件之一。

## Background / 已确认事实

- 声明处:`agent/mod.rs:63` `pub mod tests_agent_loop;`(文件级 `#![cfg(test)]`)。**目录化后此声明零改动**(Rust 自动解析到 `tests_agent_loop/mod.rs`)。
- 实测结构:**41 个测试函数** + 5 个 helper(`load_assistant_rows`@L2132 / `messages_to_text`@L3301 / `p5_seed_verified_pitfall_full_match`@L4748 / 嵌套 `batch`@L3340 / `single`@L3464)。harness 与 mock_provider 设施在既有 `tests_common`,不在本文件,不搬。
- 功能簇(行号为实测起止,见 `design.md` §2 权威表;原表"32 测"为误,实为 41——漏算 error_after_tool_use / c3_compaction / c3_still_over / p5_soft_block×2 / a5plus_retry×3):
  | 簇(文件) | 行号区间 | 测试数 | 代表函数 |
  |---|---|---|---|
  | basic | L1–1101 | 8 | `agent_loop_basic_text_only_completes` / `agent_loop_use_skill_*` / `agent_loop_max_turns_emits_done_marker` |
  | mock_provider | L1409–1479 | 2 | `mock_provider_reports_mock_protocol` / `mock_provider_call_count_tracks_send_calls` |
  | error_path | L1102–2131 | 6 | `agent_loop_error_after_tool_use_appends_synthetic_result` / `agent_loop_c3_compaction_does_not_panic` / `agent_loop_error_path_emits_chat_event_error` / `agent_loop_c3_still_over_emits_error_and_skips_provider` / `agent_loop_persist_failure_emits_error` / `agent_loop_cancel_skips_audit_for_cancelled_tool` |
  | error_persist(+load_assistant_rows 私有) | L2132–2793 | 5 | `agent_loop_error_persists_partial_text` / `agent_loop_error_persists_thinking_and_tool_calls` / `agent_loop_error_emits_turn_complete` 等 |
  | checklist | L2794–3330 | 3 | `agent_loop_update_checklist_replaces_vec_and_injects_next_turn` 等 |
  | parallel_dispatch(is_parallel 单测 + parallel/serial 集成 + 嵌套 batch/single) | L3332–4163 | 6 | `is_parallel_eligible_classifies_correctly` / `agent_loop_parallel_readonly_batch_preserves_order` / `agent_loop_mixed_batch_with_edit_falls_back_to_serial` 等 |
  | notifications(pending notifications + loop detection) | L4164–4747 | 4 | `agent_loop_drains_background_shell_notification_into_turn_2` / `agent_loop_no_pending_notifications_skips_injection` / `agent_loop_loop_detection_*` |
  | resilience(p5 soft_block + a5plus retry + p5_seed 私有) | L4748–5392 | 5 | `agent_loop_p5_soft_block_*` / `a5plus_retry_*` |
  | recall | L5394–5674 | 2 | `agent_loop_emits_recall_on_fts_hit` / `agent_loop_emits_recall_on_pitfall_hit` |
- 基线:`cargo test --lib` = 1662 全绿;`clippy --lib --tests` + `fmt --check` 零警告。拆分不增删测试,终验 41 不变。

## Requirements

- R1 拆分形态:**目录化**(首选,已定)——建 `agent/tests_agent_loop/` 目录,`mod.rs` 作 hub(`mod` 声明 + `messages_to_text`),9 个簇独立文件;`agent/mod.rs:63` 声明零改动。备选平级多文件已否决(需改 mod.rs 声明)。
- R2 纯搬迁铁律:复制 + 删源 + module 接线;禁止改逻辑 / 重命名 / 合并 / 拆分测试函数。
- R3 共享 helper 提取:**只有真跨文件的 `messages_to_text` 提 hub** `pub(super)`(checklist.rs + notifications.rs 共用);`load_assistant_rows`(4 用者全在 error_persist.rs)留 error_persist.rs 私有;`p5_seed_verified_pitfall_full_match`(2 用者全在 resilience.rs)留 resilience.rs 私有。(修正自原 R3:实测后两 helper 拆后无簇间复用,留使用处更紧封装,与 split-agent-tests 先例一致。)
- R4 提交节奏:**主迁移(建目录 + 9 簇 + hub + 删源)一个 commit + 文档 sweep 一个 commit**;非逐簇 commit(目录化模式下中间态编译不过)。回滚点 `git revert <主迁移 commit>`。每 commit 后跑全量 `cargo test --lib`。
- R5 测试函数数量与名称零变化(41 个测试原样保留)。
- R6 非 archive 文档/代码注释若引用 `tests_agent_loop.rs:LINE` 行号,随拆改符号引用。

## Acceptance Criteria

- [ ] AC1 各子文件(含 hub)< 1200 行(最大 error_path.rs ~1050 / basic.rs ~1101)
- [ ] AC2 `cargo test --lib` 全绿(1662 基线无减少,41 个测试原样)
- [ ] AC3 `agent/mod.rs` 声明零改动(目录化方案);`git diff app/src-tauri/src/agent/mod.rs` 空
- [ ] AC4 `clippy --lib --tests` + `fmt --check` 零警告
- [ ] AC5 主迁移 commit 独立可回滚;diff 只含代码位移 + module 接线,无逻辑改动
- [ ] AC6 非 archive 无残留 `tests_agent_loop.rs:LINE` 行号引用(实测 0 处,天然满足;`::fn` 符号引用 2 处保留)

## Out of Scope

- 不改被测业务代码(`chat_loop/`、`agent/tests.rs` 等)
- 不新增 / 不重命名 / 不合并测试(含 helper)
- 不改变测试语义、执行顺序与并行度
- 其余 3 个测试大文件各自独立任务:`08-09-tests-subagent-split` / `08-09-tests-memories-split` / `08-09-tests-sessions-split`
