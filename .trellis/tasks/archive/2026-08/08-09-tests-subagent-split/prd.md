# 测试文件拆分:tests_subagent.rs(4270 行)

## Goal

把 `app/src-tauri/src/agent/tests_subagent.rs`(4270 行)按被测功能簇拆分为目录化测试模块(`agent/tests_subagent/` hub + 簇文件),改善可导航性。延续拆分专项惯例:纯搬迁铁律——复制 + 删源 + module 接线,**禁止顺手改逻辑**。

> 批次衔接:批 1(`08-07-large-file-splitting`)PRD 是全仓 >1200 行文件拆分总纲,其 Out of Scope 把"既有独立测试文件的拆分"划出**当批**范围(仅批次区分,非永久排除);本任务落地该行——4 个测试大文件之一,亦是收官表(`directory-structure.md` §收官状态表)最后一行 ⏳ 项。4 个测试任务全部完成后,全仓源码/测试文件应全部 < 1200 行(除已知遗留)。

## Background / 已确认事实

- 声明处:`agent/mod.rs:75` `pub mod tests_subagent;`(文件级 `#![cfg(test)]`)。**目录化后此声明零改动**(Rust 自动解析到 `tests_subagent/mod.rs`)。
- 实测结构:**30 个测试函数**(原 PRD/task.json 写"13/16"是只数了 B6 dispatch 段、漏掉 L2376–4270 整整 1900 行的 l3a/l3b 并发 + merge/discard 段——以实测 30 + 本 PRD §簇表为准)。无 cluster-private helper;唯一跨簇共享 helper `run_loop`@L2311(封装 30+ 参的 `run_chat_loop` 调用,被 l3a/l3b 区 10 个测试调用)。mock provider / harness 设施复用既有 `tests_common`。
- 测试注解有**三种形态**,搬迁时原样复制(详见 R2):13 × `#[tokio::test]`(L47/171/405/553/1138/1286/1436/1568/2090/2201/2692/3148/3237)+ 15 × `#[tokio::test(flavor = "multi_thread")]`(L724/903/1769/2492/2790/2899/3005/3343/3469/3622/3787/3908/4080/4136/4225,并发/partial-transcript/merge/discard 测试用)+ **2 × 纯 `#[test]`**(L2375/2413,l3a unit 两个同步单测,不依赖 tokio runtime)。
- 功能簇(切分**按簇归属(函数名前缀),不按物理行号连续区间**——见 l3a/l3b concurrent 注;`run_loop` 提取到 hub 不计入任何簇):
  | 簇 | 测试数 | 代表函数 |
  |---|---|---|
  | forced dispatch 直通(无 LLM) | 1 | `agent_loop_forced_dispatch_runs_worker_without_llm`(L47) |
  | dispatch_subagent 主路径(completes / cancel / error / partial transcript / guard) | 5 | `agent_loop_dispatch_subagent_completes_and_returns_summary`(L172)/ `..._cancel_propagates_to_worker`(L406)/ `..._error_returns_status_error`(L554)/ `..._error_includes_partial_transcript_summary`(L725)/ `..._guard_does_not_evict_parent_session_active`(L904) |
  | persist / audit / token 记账 | 4 | `agent_loop_dispatch_subagent_persists_subagent_run`(L1139)/ `..._cancelled_persists_status_cancelled`(L1287)/ `..._audit_not_polluted_by_worker`(L1437)/ `..._token_usage_does_not_fold_into_parent`(L1569) |
  | plan mode 权限(写拒绝) | 1 | `agent_loop_dispatch_subagent_general_purpose_plan_mode_write_denied`(L1770) |
  | system_prompt_override | 2 | `system_prompt_override_worker_path_sends_override`(L2091)/ `system_prompt_override_none_path_uses_parent_assembly`(L2202) |
  | l3a unit guard(filter / classify) | 2 | `l3a_filter_tools_readonly_keeps_only_five_read_tools`(L2376)/ `l3a_classify_dispatch_batch_branches_correctly`(L2414);纯 `#[test]` 同步单测,不走 run_loop |
  | l3a concurrent dispatch(纯批并发 / 超限 / cancel / token / 混合批 / 单发串行) | 6 | `l3a_pure_batch_of_three_dispatches_runs_concurrently`(L2493)/ `l3a_pure_batch_over_limit_hard_rejects_all`(L2693)/ `l3a_concurrent_cancel_propagates_to_all_workers`(L2900)/ `l3a_concurrent_token_usage_does_not_fold_into_parent`(L3006)/ `l3a_mixed_batch_falls_through_to_serial_path`(L3149)/ `l3a_single_dispatch_runs_serial_path_unchanged`(L3238) |
  | l3b concurrent worker(共享完成 / 带写 / worktree 隔离 / force_readonly 废除) | 4 | `l3b_concurrent_general_purpose_workers_complete_shared`(L2791)/ `..._complete_with_writes`(L3344)/ `..._have_isolated_worktrees`(L3470)/ `..._force_readonly_param_no_longer_set`(L3623) |
  | l3b merge worker(ff / 冲突 / 无 parent worktree) | 3 | `l3b_merge_worker_happy_path_fast_forward`(L3788)/ `l3b_merge_worker_conflict_returns_error`(L3909)/ `l3b_merge_worker_no_parent_worktree_errors`(L4081) |
  | l3b discard worker(正常 / 已销毁) | 2 | `l3b_discard_worker_happy_path`(L4137)/ `l3b_discard_worker_already_destroyed_errors`(L4226) |
  > ⚠️ **l3a/l3b concurrent 物理穿插**:`l3b_concurrent_general_purpose_workers_complete_shared`(L2791)的物理位置**插在 l3a concurrent 区间内**(夹在 `l3a_pure_batch_over_limit`@L2693 与 `l3a_concurrent_cancel_propagates`@L2900 之间)。切分时**按簇归属归 l3b,不按物理行号连续区间切**——搬出该函数后,l3a concurrent 与 l3b concurrent 各自的剩余函数才形成连续块。
- 簇行数(按簇归属实际搬迁,均 < 1200):dispatch_main ≈967 / l3a_concurrent ≈741 / persist_audit_token ≈631 / l3b_concurrent ≈554 / plan_mode ≈321 / l3b_merge ≈349 / system_prompt_override ≈215 / forced_dispatch ≈124 / l3b_discard ≈135 / l3a_unit ≈116。最大簇 dispatch_main ≈967 在线内,无需二次拆。
- 基线:`cargo test --lib` = 1662 全绿(沿用 sibling 任务基线,拆分不增删测试,终验 N 不变);`clippy --lib --tests` + `fmt --check` 零警告。

## Requirements

- R1 拆分形态:**目录化**(首选,与 sibling 三单同款)——建 `agent/tests_subagent/` 目录,`mod.rs` 作 hub(`#![cfg(test)]` + `mod` 声明 + 共享 helper `run_loop` 提取 + `use super::tests_common` re-export),各簇独立文件;`agent/mod.rs:75` 声明零改动。备选:平级多文件(需改 mod.rs 加声明)——design 阶段拍板,以声明零改动为优先。
- R2 纯搬迁铁律:复制 + 删源 + module 接线;禁止改逻辑 / 重命名 / 合并 / 拆分测试函数;**三种注解形态原样复制、互不转换——`#[test]` / `#[tokio::test]` / `#[tokio::test(flavor = "multi_thread")]`**(改 flavor 会改变并发运行时,把同步单测改成 tokio 或反向是纯搬迁最易踩的漂移点)。簇文件需各自带全量 crate import(源文件 L1–15 的 `use` 块按需复制到用到该符号的簇文件)。
- R3 共享 helper `run_loop`(以及 `run_loop` 内对 `tests_common::TestHarness` 的引用)提取到 hub,`pub(super)` 可见性;**簇文件保留拆分前的显式命名 import 原样不动**(如 `use super::tests_common::{make_harness, test_messages, MockEmitter};` + 各自需要的 crate imports),hub 仅做 `#[allow(unused_imports)] use super::tests_common;` re-export 使原 import 继续解析(与 `tests_agent_loop/mod.rs` "keep imports unchanged ... pure relocation" 同款,**不引入 glob `use super::*`**)。
- R4 每簇独立 commit、独立回滚(回滚点 `git revert <commit>`);每 commit 后跑该簇过滤 + 全量 `cargo test --lib`(与 memories/sessions 两单同策略;agent-loop 首单因目录化中间态用单 commit,本任务目录化方案与后两单一致)。簇文件命名:sibling 约定为 snake_case 语义名(非行号),本任务沿用。
- R5 测试函数数量与名称零变化(30 个测试原样保留;`cargo test --lib "agent::tests_subagent::"` 跑前 30 / 跑后 30)。
- R6 非 archive 文档/代码注释若引用 `tests_subagent.rs:LINE` 行号,随拆改符号引用。**全仓实测仅一处**:`docs/IMPLEMENTATION/decisions-2026-06.md:711`(残留文档债注记,引 `tests_subagent.rs:1363/1669` 两处过时测试注释)——处理方式:改为符号引用指向具体测试函数名(`agent_loop_dispatch_subagent_audit_not_polluted_by_worker` / `..._token_usage_does_not_fold_into_parent`);其余引用(STRUCTURE.md 目录树、tool-contract 等)为纯路径无行号,R6 不涉及。
- R7 `directory-structure.md` §收官状态表本任务行(`agent/tests_subagent.rs` 4270 ⏳)拆完后改为 ✅ 并填实测(hub 行数 + 簇数 + 最大簇文件行数),与 sibling 三单收尾同款。

## Acceptance Criteria

- [ ] AC1 各子文件(含 hub)< 1200 行
- [ ] AC2 `cargo test --lib` 全绿(1662 基线无减少,30 个测试原样)
- [ ] AC3 `agent/mod.rs` 声明零改动(目录化方案);`cargo check --lib` 调用方零影响
- [ ] AC4 `clippy --lib --tests` + `fmt --check` 零警告
- [ ] AC5 每个拆分 commit 独立可回滚;diff 只含代码位移 + module 接线,无逻辑改动
- [ ] AC6 非 archive 无残留 `tests_subagent.rs:LINE` 行号引用
- [ ] AC7 `directory-structure.md` §收官状态表本任务行更新为 ✅(4 个测试任务全部完成)

## Out of Scope

- 不改被测业务代码(`agent/subagent/`、`agent/chat_loop/` 等)
- 不新增 / 不重命名 / 不合并测试(含 helper)
- 不改变测试语义、执行顺序与并行度(含 `flavor` 参数)
- 不调整 `run_chat_loop` 34 参签名债务(冻结,见收官表遗留)
- 4 个测试大文件其余 3 个已完成(`08-09-tests-agent-loop-split` / `08-09-tests-memories-split` / `08-09-tests-sessions-split`);本任务是收官最后一单
