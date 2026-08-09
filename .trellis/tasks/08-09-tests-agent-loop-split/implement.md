# Implement: tests_agent_loop.rs 目录化拆分

> 执行清单。需求见 `prd.md`,技术设计见 `design.md`。纯搬迁铁律适用全程(R2)。

## 前置(基线快照,做一次)

```bash
cd app/src-tauri
export PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
cargo test --lib --no-run  # 确认能编译
# 记录基线测数:tests_agent_loop 模块 = 41
cargo test --lib "agent::tests_agent_loop::" 2>&1 | tail -3   # 应 41 passed
```

## 簇 → 源行号速查(提取依据,见 design.md §2)

| 文件 | 源行号(含头 use) | 测数 | 关键边界 |
|---|---|---|---|
| basic.rs | L1–1101 | 8 | 止于 error_after_tool_use 之前 |
| error_path.rs | L1102–2131 | 6 | 含 error_after_tool_use / c3_compaction / error_path / c3_still_over / persist_failure / cancel_skips_audit;**不含** load_assistant_rows(L2132) |
| error_persist.rs | L2132–2793 | 5 | 含 load_assistant_rows(私有 helper) |
| mock_provider.rs | L1409–1479 | 2 | 注意:源文件中 mock_provider 在 error_path 区间之前(L1409 < L1102?否——L1102–2131 是 error_path,L1409 在其**内部**) |

> ⚠️ **行号交叉订正**:mock_provider 测(L1409–1479)实际落在 error_path 区间(L1102–2131)
> 内部。源文件真实顺序是:L1102 error_after → L1226 c3_compaction → **L1409 mock_protocol**
> → **L1428 call_count** → L1480 error_path → ...。故 mock_provider 簇必须**从 error_path
> 区间内抽走**,error_path.rs 行号应是 L1102–1408 + L1480–2131(跳过 mock 两测)。
>
> 这意味着两个簇在源文件中**交错**。提取策略调整:按测试逐个 `sed` 提取,非连续区间切片。
> 详见下方 Step 2 的逐簇提取说明。

## 执行步骤

### Step 1:建目录 + hub 骨架(主迁移 commit 内,原子)

1. `mkdir -p app/src-tauri/src/agent/tests_agent_loop`
2. 写 `tests_agent_loop/mod.rs`(hub):
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

   /// 跨文件共享:checklist.rs + notifications.rs 用。
   pub(super) fn messages_to_text(msgs: &[crate::llm::types::ChatMessage]) -> String {
       // ← 原 L3301–3331 body 平移
   }
   ```
   `messages_to_text` body 用 `sed -n '3301,3331p'` 提取原文件,`&[ChatMessage]` 改
   `&[crate::llm::types::ChatMessage]`(全路径)。

### Step 2:逐簇提取(每簇一个文件,均加 `#![cfg(test)]` + 实际 use)

因 mock_provider 簇与 error_path 簇在源文件中交错(见上 ⚠️),**按测试函数逐个提取**,
不要用连续区间切片。每簇:

- 从原文件按测试函数 `sed -n '<fn_start>,<fn_end>p'` 逐个提取(含 `#[tokio::test]`/`#[test]` 行)
- 簇文件头加 `#![cfg(test)]`
- 簇文件头加该簇用到的 `use`(先放原 L3–16 完整 use 块,clippy 后删 unused)
- 簇内若有 helper(error_persist 的 load_assistant_rows / resilience 的 p5_seed / parallel 的 batch·single),
  随簇迁,保持私有 + 原嵌套层级

**各簇包含的测试函数(按提取顺序,行号为源文件实测)**:

| 文件 | 测试函数(源行号) |
|---|---|
| basic.rs | basic_text(29) / tool_use(149) / non_tool(287) / use_skill_loads(381) / use_skill_unknown(521) / cancel_in_turn(672) / max_turns(820) / exhaustion(997) |
| mock_provider.rs | mock_protocol(1409) / call_count(1427) |
| error_path.rs | error_after_tool_use(1102) / c3_compaction(1225) / error_path_emits(1480) / c3_still_over(1614) / persist_failure(1821) / cancel_skips_audit(1990) |
| error_persist.rs | helper load_assistant_rows(2132) + persists_partial(2153) / empty_text(2278) / persists_thinking(2383) / log_only(2518) / emits_turn_complete(2655) |
| checklist.rs | update_checklist_replaces(2794) / coerces_two(2990) / cancelled_update(3152) |
| parallel_dispatch.rs | is_parallel_classifies(3332,含嵌套 batch) / is_parallel_boundary(3443,含嵌套 single) / readonly_batch(3553) / mixed_batch(3758) / web_fetch(3909) / parallel_cancel(3975) |
| notifications.rs | drains_background(4164) / no_pending(4366) / loop_detection_injects(4518) / loop_detection_silent(4630) |
| resilience.rs | helper p5_seed(4748) + p5_soft_block_short(4784) / p5_soft_block_second(4901) / a5plus_double_count(5055) / a5plus_emits_retrying(5179) / a5plus_terminal(5291) |
| recall.rs | recall_fts(5393) / recall_pitfall(5517) |

### Step 3:删源(主迁移 commit 内,原子)

- `rm app/src-tauri/src/agent/tests_agent_loop.rs`
- **`agent/mod.rs` 不动**(L63 `pub mod tests_agent_loop;` 原样)

### Step 4:编译收敛(主迁移 commit 内)

```bash
cargo test --lib --no-run 2>&1 | grep -E "error|warning: unused" | head -40
```

- 编译错(missing/未声明的项)→ 检查簇间漏搬 / helper 可见性
- `unused import` → 逐簇删 use(允许:clippy 驱动的死 import 清理,非逻辑改动)
- `cannot find` → 该簇漏了某 use,补回

### Step 5:测试收敛(主迁移 commit 内)

```bash
cargo test --lib "agent::tests_agent_loop::" 2>&1 | tail -5   # 应 41 passed
PKG_CONFIG_PATH="..." cargo test --lib 2>&1 | tail -3          # 全量 1662 基线
```

41 缺一不可(漏搬 → 该测消失 → 计数 <41 → 回查 Step 2 提取)。

### Step 6:clippy + fmt(主迁移 commit 内)

```bash
cargo clippy --lib --tests 2>&1 | grep -E "warning|error" | head
cargo fmt --check
```
零警告零 diff。`Row`(若 clippy 报 unused)在此删。

### Step 7:行数核验(AC1)

```bash
wc -l app/src-tauri/src/agent/tests_agent_loop/*.rs
```
最大 error_path.rs ~1050 / basic.rs ~1101,均 < 1200。

### Step 8:主迁移 commit

```bash
git add app/src-tauri/src/agent/tests_agent_loop/ \
        app/src-tauri/src/agent/tests_agent_loop.rs   # 删除
git commit -m "refactor: tests_agent_loop 子模块化(tests_agent_loop.rs → hub + 9 簇文件)"
```

### Step 9:文档 sweep commit(AC6 —— 实测 0 行号引用,本步可能为空)

```bash
# 核验无 tests_agent_loop.rs:LINE 行号引用(应 0 输出)
grep -rn "tests_agent_loop\.rs:[0-9]" --include="*.md" --include="*.rs" . | grep -v "/archive/"
```
若有输出 → 改符号引用(本任务预判 0);若无,跳过本 commit。

## 回滚点

- `git revert <Step 8 commit>` → 回到单文件 5674 行状态
- Step 9 若做了 sweep,独立 `git revert <Step 9 commit>`

## 风险清单

| 风险 | 触发 | 缓解 |
|---|---|---|
| mock/error_path 行号交错导致区间切片错 | Step 2 用连续区间 | 改逐测试函数提取(已在上说明) |
| 漏搬一个测试 | Step 5 计数 <41 | 计数硬闸,逐簇过滤定位 |
| helper 可见性不够 | 编译报 private | messages_to_text 必须 pub(super);load_assistant_rows/p5_seed 留私有(簇内) |
| Row 死 import | clippy warning | clippy 报才删,不主动判 |
| mod.rs 误改 | AC3 失败 | git diff 空是硬闸 |
