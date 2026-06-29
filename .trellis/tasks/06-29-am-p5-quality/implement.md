# P5 implement.md — 执行计划

> child `06-29-am-p5-quality` · 设计依据 [`design.md`](./design.md) · 前置 P1（接口底座）/P3（注脚档 + Tier1 落点）/P4（emit_tool_result seam + FailureTracker 范式）已合入。

## 实施顺序（按依赖 + 风险从低到高）

每步附带最小单测；逐步 `cargo test --lib` 保持绿，避免大爆炸。

### Step 1 — char-trigram Jaccard util（纯函数，零风险）
- 新文件 `app/src-tauri/src/agent/memory_hygiene.rs`（或 `memory/dedup.rs`），实现 `fn char_trigrams(s: &str) -> HashSet<String>`（滑动 3 char，不足 3 的整串当 1 个）+ `fn jaccard(a, b) -> f32`。
- 单测：完全相同=1.0；无关=0.0；中文短句重叠（"设置 PKG_CONFIG_PATH" vs "PKG_CONFIG_PATH 环境变量"）>0.7；大小写/trim 归一。
- `cargo test --lib memory_hygiene`

### Step 2 — 状态机晋升 `promote_if_eligible`（D2）
- `db/memories.rs`：新增 `pub async fn promote_if_eligible(pool, memory_id) -> Result<(), sqlx::Error>`。在 `bump_hit_count` 的 UPDATE 后**同连接**读回 `(hit_count, status, created_at)`，按 D2 阈值（candidate→active @ hit≥2；active→verified @ hit≥5 且 age≥3 天）调 `update_status`。原子化（避免 bump↔promote 竞态）；非法转换交给 `update_status` 矩阵。
- `bump_hit_count` 末尾调 `promote_if_eligible`（best-effort，`warn!` 吞，不影响 bump 返回）。
- 单测：hit 跨 2 升 active；跨 5+age 够升 verified、age 不够停留 active；demoted 不被 bump 晋升（矩阵拒绝）。
- `cargo test --lib memories`

### Step 3 — pre-tool recall 分档 `PitfallRecall`（D1 + §3 纠正）
- `agent/permissions/check.rs`：新增 `pub enum PitfallRecall { None, Footnote(String), SoftBlock { hint: String, memory_id: String } }`。新增 `pub async fn recall_pitfall(db, tool_name, tool_input, already_blocked: &HashSet<String>) -> PitfallRecall`：
  - `find_pitfalls_by_trigger` 返回行 → **先确认其 SQL 不限 status**（若内部 filter 了 candidate/verified，改 SQL 放宽，或新增 `find_pitfalls_by_trigger_all_status`）。
  - 分档：`verified` + 完全命中（tool+command_pattern+path_globs 皆中）+ `memory_id` 不在 `already_blocked` → `SoftBlock`；其余 active/candidate/二次命中 → `Footnote`；无 → `None`。
  - 保留旧 `recall_pitfall_footnote` 给 Footnote 档（或重构为 `recall_pitfall(...).into_footnote()`），P3 现有调用点行为不变。
- 单测：verified 完全命中→SoftBlock；active→Footnote；二次命中（id 在 set）→Footnote；candidate→Footnote。
- `cargo test --lib check`

### Step 4 — chat_loop 两 path 软拦截短路（本 task 最重）
- `agent/chat_loop.rs`：loop 顶部（`FailureTracker::new()` 旁，`:514`）加 `let soft_blocked: Arc<Mutex<HashSet<String>>> = Default::default();`（session 级记账，D1）。
- **parallel path**（`:1822`）/ **serial path**（`:2415`）：把 `recall_pitfall_footnote` 调用换成 `recall_pitfall(..., &soft_blocked)`：
  - `SoftBlock { hint, memory_id }` → **不调** `execute_tool`；插入 `memory_id` 到 set；构造 `ToolResult { content: hint, is_error: false }`；`emit_tool_result`；`bump_hit_count`；把该 block 写入 `result_slots[i]`（parallel）/ `result_blocks`（serial），跳过后续 execute/audit。注意**不写** `tool_executed` audit（tool 未真跑）。
  - `Footnote(text)` → 现状（execute 后前置注脚）。
  - `None` → 现状。
- 两 path 对称改；复用 `Decision::Deny`（`:1790`/`:2270`）的"不执行+回填"结构作模板。`RULE-A-004`（cancel 跳 audit）不变量保持。
- 单测：chat_loop 集成测试 stub 一条 verified pitfall → 首次 tool_use 返回 SoftBlock 提示、未执行；同 pitfall 二次 → 执行+注脚。
- `cargo test --lib chat_loop`

### Step 5 — 注释纠正 + filter 确认（§3）
- `memory_recall.rs:96` 注释改为"P5 决定保持 `IncludeCandidate`（candidate 靠召回命中晋升，收紧会断路，见 design.md §3）"。filter 代码不动。
- `RecallStatusFilter::ActiveVerifiedOnly` 保留（P1 定义），P5 不切。

### Step 6 — 卫生 job（D4 + §6）
- `agent/memory_hygiene.rs`：`pub async fn run_hygiene_pass(pool)` —— dedup（pitfall 按 trigger_key；其余按 char-trigram Jaccard>0.7，合并 hit_count/保留高 confidence、`delete_memory` 删冗余）+ 降权（candidate/active 且 last_used>30 天 且 hit<2 → `update_status(Demoted, "aged_out")`）。
- `db/memories.rs::insert_memory` 末尾：按 `(scope, kind)` 计数 `& 10 == 0` → `tokio::spawn(run_hygiene_pass)`。fire-and-forget。
- `lib.rs` setup：app 启动 `tokio::spawn(run_hygiene_pass)` 一次（清理历史积累）。
- 单测：两条高 Jaccard → 合并 hit_count；老低命中 → demoted；trigger_key 相同 pitfall → 合并。
- `cargo test --lib memory_hygiene`

### Step 7 — 全量回归 + 手动验证
- `PKG_CONFIG_PATH=... cargo test --lib` 全绿（memories / check / auto_reflect / memory_recall / chat_loop / memory_hygiene）。
- 手动（真实 LLM，可选但推荐）：手写一条 `verified` pitfall（trigger_key=shell/cargo test）→ 新 session 跑 `cargo test` → 看软拦截提示 + tool 未执行 → LLM 重判 → 重发同命令 → 看降级注脚 + 执行。确认前端 `ToolCallCard` 正常渲染提示型 result。

## 验证命令

```bash
# 单元测试（WSL 需 PKG_CONFIG_PATH，见 CLAUDE.md HACKING-wsl 坑 1）
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib

# 编译检查
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
```

## 风险点 / 回滚

- **chat_loop 两 path 对称改动**：parallel（`FuturesUnordered`）+ serial（`for` loop）结构不同但逻辑对称；小心 `RULE-A-004`（cancel 跳 audit）、`emit_tool_result` 时序、`result_slots[i]` 索引写入。回滚：`PITFALL_SOFT_BLOCK_ENABLED=false` → `recall_pitfall` 永不返回 SoftBlock，退回 P3 注脚。
- **`bump_hit_count` 内嵌 promotion**：SQLite 单写者串行，但 bump 的 UPDATE 与 promote 的读回须**同连接/同事务**，否则读不到刚写的 hit_count。用 `promote_if_eligible` 接 `&SqlitePool` 紧跟 bump 同任务 await（sqlx 连接池同 query 序列在同一连接的概率高；若 flaky，改为单事务版本）。
- **`find_pitfalls_by_trigger` status filter**：若其 SQL 内部 `WHERE status='active'`，Step 3 的 candidate/verified 分档拿不到数据 —— 必须先读它的 SQL（`db/memories.rs:1046`），必要时放宽。
- **前端 ToolCallCard**：is_error=false 的提示型 result 不能显示成 error 态；实现后手动看一眼。

## review gate（`task.py start` 前）

- [ ] design.md 4 决策 + §3 纠正经用户确认
- [ ] implement.md 顺序无环依赖
- [ ] `cargo test --lib` 现状基线绿（动手前先跑一次，确认 P1-P4 测试无回归起点）
