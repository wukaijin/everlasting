# Design: tests_agent_loop.rs 目录化拆分

> 本文是技术设计。需求与验收见 `prd.md`。执行清单见 `implement.md`。

## 1. 形态:目录化(R1 首选方案)

```
agent/
├── mod.rs                     # L63 `pub mod tests_agent_loop;` 零改动(Rust 自动解析到目录)
└── tests_agent_loop/          # 新目录(hub + 9 簇文件)
    ├── mod.rs                 # hub:#![cfg(test)] + 9 个 mod 声明 + messages_to_text
    ├── basic.rs               # 基础 loop:8 测
    ├── mock_provider.rs       # mock provider 契约:2 测
    ├── error_path.rs          # error / cancel / c3 / persist-failure 路径:6 测
    ├── error_persist.rs       # persist 落库路径:5 测 + load_assistant_rows(私有)
    ├── checklist.rs           # update_checklist:3 测
    ├── parallel_dispatch.rs   # is_parallel_eligible 单测 + parallel/serial 集成:6 测
    ├── notifications.rs       # pending notifications + loop detection:4 测
    ├── resilience.rs          # p5 soft_block + a5plus retry:5 测 + p5_seed(私有)
    └── recall.rs              # recall emit:2 测
```

`tests_agent_loop.rs`(5674 行)整体删除,内容按簇平移到 9 个簇文件。`agent/mod.rs:63`
`pub mod tests_agent_loop;` 一字不改 —— Rust 2018 module 路径解析器对 `tests_agent_loop/`
目录与 `tests_agent_loop.rs` 文件等价对待,因此调用方(`daemon/server.rs:499`、
`daemon-server.md:247` 的符号引用 `tests_agent_loop::agent_loop_cancel_in_turn_2_kills_loop`
等)语义不变。

### 为什么不用平级多文件(备选)

平级方案要在 `agent/mod.rs` 把 `pub mod tests_agent_loop;` 改成 `pub mod tests_agent_loop_basic;`
等 10 行声明,**违反 R1 "声明零改动"优先项**且改动调用方模块路径。目录化零改动更优。

## 2. 簇映射(9 文件 / 41 测 + 5 helper)—— 权威表

> 测试数与行号经实测核对(原 PRD 表写"32 测"是错误的,实际 41;`task.json` 描述正确)。
> 行号是 `tests_agent_loop.rs` 的实测起止(含 `#[test]`/`#[tokio::test]` 行与文件头 use 块)。

| 文件 | 测数 | 源行号 | 行数 | 含 helper | 簇内测试 |
|---|---|---|---|---|---|
| `basic.rs` | 8 | L1–1101 | ~1101 | — | basic_text / tool_use / non_tool / use_skill_loads / use_skill_unknown / cancel_in_turn / max_turns / exhaustion |
| `mock_provider.rs` | 2 | L1409–1479 | ~95 | — | mock_protocol / call_count |
| `error_path.rs` | 6 | L1102–2131 | ~1050 | — | error_after_tool_use / c3_compaction / error_path_emits / c3_still_over / persist_failure / cancel_skips_audit |
| `error_persist.rs` | 5 | L2132–2793 | ~685 | `load_assistant_rows`(私有) | persists_partial / empty_text / persists_thinking / log_only / emits_turn_complete |
| `checklist.rs` | 3 | L2794–3330 | ~548 | — | update_checklist ×3 |
| `parallel_dispatch.rs` | 6 | L3332–4163 | ~850 | 嵌套 `batch`/`single`(本地) | is_parallel_eligible ×2 + readonly_batch / mixed_batch / web_fetch / parallel_cancel |
| `notifications.rs` | 4 | L4164–4747 | ~600 | — | drains_background_shell / no_pending / loop_detection ×2 |
| `resilience.rs` | 5 | L4748–5392 | ~660 | `p5_seed_verified_pitfall_full_match`(私有) | p5_soft_block ×2 + a5plus_retry ×3 |
| `recall.rs` | 2 | L5394–5674 | ~292 | — | recall_fts / recall_pitfall |
| **hub mod.rs** | — | — | ~30 | `messages_to_text`(`pub(super)`) | — |

**测数核验**:8+2+6+5+3+6+4+5+2 = **41** ✓
**最大文件** error_path.rs ~1050 < 1200 ✓;basic.rs ~1101 < 1200 ✓(AC1 满足)

### error_path.rs 边界说明

把 `error_after_tool_use`(L1102)+ `c3_compaction`(L1226)从 basic 簇并入 error_path 簇:
保留在 basic 会让 basic.rs 达 ~1390 行超 AC1(<1200)。这两测语义偏"错误/异常路径"
(error_after: 工具用后出错补合成结果;c3_compaction: c3 超限不崩),与 error_path 同域,
并入合理。basic 簇保留 8 个"正常路径"测。

### 三处簇合并(11 内聚簇 → 9 文件)

- `is_parallel_eligible` 单测×2 + parallel/serial 集成×4 → `parallel_dispatch.rs`(6 测)
- pending notifications + loop detection → `notifications.rs`(4 测)
- p5 soft_block + a5plus retry → `resilience.rs`(5 测,主题=韧性/恢复)

三合并都语义自然、各文件 <1200。

## 3. hub(mod.rs)内容

```rust
#![cfg(test)]

mod basic;
mod mock_provider;
mod error_path;
mod error_persist;
mod checklist;
mod parallel_dispatch;
mod notifications;
mod resilience;
mod recall;

/// 跨文件共享 helper:checklist.rs 与 notifications.rs 均用。
/// 仅本模块树内可见,不放宽到 crate(R3 修正:只有真跨文件 helper 才进 hub)。
pub(super) fn messages_to_text(msgs: &[crate::llm::types::ChatMessage]) -> String {
    // …原 L3301–3331 body 平移…
}
```

- `mod` 声明不带 `#[cfg(test)]`(跟随 split-agent-tests 先例:依赖文件级 `#![cfg(test)]`)。
- `messages_to_text` 参数类型改全路径 `&[crate::llm::types::ChatMessage]`(hub 不 `use`
  `ChatMessage`,避免为单 helper 引入 use)。**这是纯搬迁允许的可见性/路径调整,非逻辑改动**。
- 簇文件引用处 `messages_to_text(&sent[0])` 调用点零改(参数推断)。

### 可见性决策(R3 修正)

原 PRD R3:两个 helper(`load_assistant_rows`/`messages_to_text`)提 hub `pub(crate)`。
实测后修正:

- `messages_to_text`:真跨文件(checklist.rs L2941/2949/3114 + notifications.rs L4284/4294/4463)
  → 提 hub,`pub(super)`(限定模块树,不放宽到 crate)。
- `load_assistant_rows`:4 使用者全在 error_persist.rs(L2256/2369/2484/2772)
  → 留 error_persist.rs 私有,不进 hub。
- `p5_seed_verified_pitfall_full_match`:2 使用者全在 resilience.rs(L4787/4904)
  → 留 resilience.rs 私有,不进 hub。
- 嵌套 `batch`(L3340)/`single`(L3464):仅 is_parallel_eligible 两测内部用
  → 随测迁 parallel_dispatch.rs,留作测内嵌套,原样。

R3 的"簇间复用"前提对 load_assistant_rows/p5_seed 不成立(拆后无簇间复用),故偏离字面
`pub(crate)` 取"留使用处",更紧封装,且与 split-agent-tests 先例一致(该任务里
`load_assistant_rows`/`messages_to_text` 当时就留 agent_loop.rs)。

## 4. 每簇 import 策略

每个簇文件自含 `#![cfg(test)]` + 该簇**实际用到**的 `use`。不集中到 hub re-export
(避免 hub 膨胀 + 违反"unused-import 精确收敛"惯例)。原文件单一 use 块(L3–16)按簇拆。

**收敛办法**:每簇先放完整 use 块,跑 `cargo test --lib <簇过滤>` + `cargo clippy --lib --tests`,
按 `unused import` 警告逐簇删到零警告。这是 split-agent-tests 先例的"依赖编译器精确收敛"法。

**`Row` 注记**:原 L7 `use sqlx::{Row, SqlitePool}` 中 `Row` 实测在 `load_assistant_rows`
body(L2132–2152)中**未使用**(body 只 `filter`/`collect`,不调 `Row::get`)。但铁律禁止
"顺手改逻辑"——若 clippy 报 `unused import: Row`,删它属于"修死代码"非"改测试逻辑",允许;
若不报(可能 sqlx 派生 trait 隐式用),原样保留。**执行时以 clippy 输出为准,不主动判断**。

## 5. 纯搬迁铁律边界(R2)

允许 | 禁止
---|---
文件物理搬迁(`sed -n 'start,end p'` 提取簇 body) | 改测试函数 body / 断言 / 顺序
helper `pub`/`pub(super)` 可见性调整(R3) | 重命名测试函数或 helper
`messages_to_text` 参数类型改全路径(hub 无该 use) | 合并 / 拆分测试函数
每簇删 `unused import`(clippy 驱动) | 改 `#[tokio::test]`/`#[test]` 属性
module 接线(`mod xxx;`) | 改执行顺序 / 并行度

## 6. AC6 文档 sweep

实测 `grep -rn "tests_agent_loop\.rs:" --include="*.md" --include="*.rs" . | grep -v archive`:

- **行号引用(`:LINE`)0 处** —— AC6 已天然满足,无需改。
- **符号路径引用 2 处**(目录化后语义不变,保留不改):
  - `app/src-tauri/src/daemon/server.rs:499` —— `tests_agent_loop.rs::agent_loop_cancel_in_turn_2_kills_loop`
  - `.trellis/spec/backend/daemon-server.md:247` —— `tests_agent_loop.rs::persist_turn...`

目录化后这两处仍指向同一测试(模块路径 `tests_agent_loop::...` 不变),**保留不改**
(用的是 `::fn` 符号引用,不是 `:LINE` 行号)。R6 措辞"行号引用"已精确,本任务无
行号引用需 sweep。

## 7. 风险与回滚

- **风险**:簇边界切错(漏搬一行 / 多搬一行)→ `cargo test --lib` 立即报缺测或编译错,
  编译器是硬闸。逐簇过滤跑测可定位。
- **回滚**:`git revert <commit>`(主迁移 commit 整体回滚,回退到单文件 5674 行状态)。
- **编译中间态**:目录化模式下一个 commit 内 `tests_agent_loop.rs` 删除 + 目录建立必须
  原子完成(否则源文件与 `mod` 声明冲突)。故不逐簇 commit,主迁移单 commit(见 implement.md)。

## 8. 验证矩阵(终验)

| 检查 | 命令 | 期望 |
|---|---|---|
| 调用方零影响(AC3) | `git diff app/src-tauri/src/agent/mod.rs` | 空 diff(L63 一字未改) |
| 全量绿(AC2) | `PKG_CONFIG_PATH=... cargo test --lib` | 1662 基线,41 个 tests_agent_loop 测全过 |
| 行数合规(AC1) | 各簇 + hub `< 1200`(`wc -l`) | error_path ~1050 / basic ~1101,均达标 |
| clippy(AC4) | `cargo clippy --lib --tests` | 0 warning |
| fmt(AC4) | `cargo fmt --check` | 0 diff |
| 测数守恒(AC2/R5) | 跑前 41 / 跑后 41 | 0 增减 |
