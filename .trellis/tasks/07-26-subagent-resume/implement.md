# Implement: subagent resume mechanism (C1)

> 父任务：`07-26-workflow-review-plugin`
> 配套：`prd.md` + `design.md`
> 本文件是 ordered execution checklist + 验证命令 + 风险点。每步可独立验证。

## 执行顺序

依赖关系决定顺序：DB schema → 写入侧 → 读取侧 → resume 逻辑 → schema 暴露 → 测试。每步标注可验证信号。

---

## Phase 1：DB schema（messages 持久化基础）

### 步骤 1.1 — 加 messages_json + messages_truncated 列

**文件**：`app/src-tauri/src/db/migrations.rs`

**改动**：在 `migrate` 函数的 subagent_runs 加列段（line ~740 `model_display` 之后）追加：
```rust
add_subagent_runs_column_if_missing(pool, "messages_json", "TEXT").await?;
add_subagent_runs_column_if_missing(pool, "messages_truncated", "INTEGER NOT NULL DEFAULT 0").await?;
```

加注释块说明（仿 line 613-638 风格）：C1 subagent resume，存 `Vec<ChatMessage>` 序列化，供 resume 续接；messages_truncated 同 transcript_truncated 模式（超阈值截断则 resume 走 fallback）。

**验证**：
```bash
cd app/src-tauri && cargo test --lib migrate 2>&1 | tail -5
# 期望：migration 测试通过；新列可空 + DEFAULT 0，旧库兼容
```

**风险**：低。复用现成 `add_subagent_runs_column_if_missing` 辅助函数（line 1201），模式与 final_text/turn_count 一致。

---

## Phase 2：写入侧（worker run 完成时持久化 messages）

### 步骤 2.1 — update_run_finished 加 messages 参数

**文件**：`app/src-tauri/src/db/subagent_runs.rs:451`

**改动**：`update_run_finished` 签名加两个参数：
```rust
pub async fn update_run_finished(
    pool: &SqlitePool,
    id: &str,
    status: SubagentStatusDb,
    finished_at: &str,
    summary: &str,
    final_text: &str,
    token_usage: &TokenUsage,
    transcript: &[crate::agent::subagent::TranscriptEntry],
    transcript_truncated: bool,
    turn_count: Option<i64>,
    messages: &[crate::llm::types::ChatMessage],        // 新增
    messages_truncated: bool,                            // 新增
) -> Result<(), sqlx::Error> {
```

UPDATE 语句加两列写入；`messages_json` 用 `serde_json::to_string(messages).unwrap_or_else(|_| "[]".to_string())`（同 transcript_json 模式）。

### 步骤 2.2 — 截断函数 + 调用点

**文件**：`app/src-tauri/src/agent/subagent/dispatch.rs`（update_run_finished 调用点 line ~1143）

**改动**：
1. 新增 `truncate_messages_for_persistence`（仿 `truncate_transcript_for_persistence`，阈值 `MESSAGES_MAX_BYTES = 8 * 1024 * 1024`，是 transcript 的 2 倍——见 design.md §1）。head+tail 策略，但 messages 截断后**不可续**（resume 走 fallback），与 transcript 截断仍可展示不同。
2. worker run 完成处，从 worker loop 累积的 messages 截断后传入 `update_run_finished`。

**关键**：worker loop 累积的 messages 需从 `run_chat_loop` 取出——确认 `run_chat_loop` 是否返回最终 messages，或需新增返回通道。**这是本步的不确定点**：若 run_chat_loop 不返回 messages，需让它把累积的 `Vec<ChatMessage>` 通过 sink 或返回值暴露（见风险点 R1）。

**验证**：
```bash
cd app/src-tauri && cargo test --lib subagent_runs 2>&1 | tail -10
# 期望：update_run_finished 单测通过；round-trip（写入 messages_json 后能读回）
```

---

## Phase 3：读取侧（load_persisted_messages）

### 步骤 3.1 — 新增 load_messages_by_run_id

**文件**：`app/src-tauri/src/db/subagent_runs.rs`

**改动**：新增函数：
```rust
/// 返回 (messages, truncated, parent_session_id)。
/// messages_json 为 NULL（旧 run）→ messages 空 Vec + truncated=false（caller 据此 fallback）。
pub async fn load_messages_by_run_id(
    pool: &SqlitePool,
    run_id: &str,
) -> Result<Option<(Vec<ChatMessage>, bool, String)>, sqlx::Error>
```

返回三元组供 caller 做 fallback 判定 + 跨 session 校验。`Option` 表示 run_id 不存在（None）。

**验证**：
```bash
cd app/src-tauri && cargo test --lib load_messages 2>&1 | tail -5
# 单测：存在 run + 有 messages → Some；旧 run（messages_json NULL）→ Some(空 Vec)；不存在 run_id → None
```

---

## Phase 4：resume 逻辑（run_subagent 分支）

### 步骤 4.1 — resume 分支 + fallback

**文件**：`app/src-tauri/src/agent/subagent/dispatch.rs`（run_subagent，line ~645 构造 worker_messages 处）

**改动**：把 `build_worker_messages(...).await` 包进 resume 判定（见 design.md §2 伪代码）：
1. 读 `input.resume_from`（Option<&str>）。
2. resume 路径：`load_messages_by_run_id` → 校验（存在 / 未截断 / 同 session / 非 running）→ 拼 messages（上轮 + clarification + task）。
3. 任一校验失败 → fallback `build_worker_messages` + warn + tool_result 内嵌 `[resume: fallback, reason: <code>]`。
4. 无 resume_from → 原 `build_worker_messages` 路径（零回归）。

### 步骤 4.2 — build_clarification_message 辅助

**文件**：`app/src-tauri/src/agent/subagent/dispatch.rs`

**改动**：新增辅助函数，把 `resume_clarification`（current_state/changes_since_last/this_round_purpose）拼成一条 user ChatMessage（markdown 格式化）。resume 路径调用它追加到 messages。

**验证**（4.1 + 4.2）：
```bash
cd app/src-tauri && cargo test --lib resume 2>&1 | tail -10
# 单测（design.md §7 列的全部）：
# - resume 续接 messages 正确性
# - 空 messages_json → fallback
# - truncated → fallback
# - run_id 不存在 → fallback + resume_run_not_found
# - running → fallback + resume_run_still_running
# - 跨 session → fallback + resume_run_other_session
# - 不传 resume_from → 行为与现状一致（snapshot 对比）
```

---

## Phase 5：schema 暴露（dispatch_subagent 工具定义）

### 步骤 5.1 — definition_with_cache 加 resume properties

**文件**：`app/src-tauri/src/agent/subagent/mod.rs:381`（input_schema properties）

**改动**：在 properties 加 `resume_from`（string）+ `resume_clarification`（object，含 current_state/changes_since_last/this_round_purpose）。description 说明 resume 语义 + 「省略则全新派」。required 不变（仍 subagent + task）。

**验证**：
```bash
cd app/src-tauri && cargo test --lib definition_with_cache 2>&1 | tail -5
# 单测：schema 含 resume_from/resume_clarification properties；required 不含它们
```

---

## Phase 6：全量验证

### 步骤 6.1 — 回归 + lint

```bash
cd app/src-tauri
cargo test --lib 2>&1 | tail -20          # 全量单测
cargo clippy --lib --tests -- -D warnings 2>&1 | tail -10
```

**回归重点**：
- 现有 subagent 测试全绿（不传 resume_from 走原路径）。
- `update_run_finished` 所有调用点已更新签名（dispatch.rs + 测试构造点）。
- migration 向后兼容（旧库 messages_json NULL 不报错）。

### 步骤 6.2 — 集成 smoke（手动，C1 验收外但推荐）

mock provider 跑：dispatch worker（run-A）完成 → dispatch worker（resume_from=run-A）→ 验证 messages 续接。这步可在 C3 集成时补，C1 单测覆盖即可。

---

## 风险点

### R1（中）：worker loop messages 暴露
**问题**：步骤 2.2 需从 `run_chat_loop` 取出累积的 messages 持久化。若 run_chat_loop 当前不返回 messages（只通过 sink 流事件），需新增返回通道（返回值加 messages，或 sink 加 `final_messages()` 方法）。
**缓解**：步骤 2.2 开工先 read run_chat_loop 签名 + sink 结构确认。若需改 run_chat_loop 签名，影响面大（所有 caller），考虑用 sink 的 `final_messages()`（仿 `final_text()`，sink.rs:297）最小侵入。
**回滚点**：若 run_chat_loop 改动风险高，C1 可先只做 resume 的「读旧 run messages」+「fallback 全新派」，持久化留 follow-up——但这会让 resume 永远 fallback，需与产品确认是否接受（见 design.md follow-up）。

### R2（低）：messages_json 存储膨胀
**问题**：messages 含 tool_use/tool_result 全文，长 run（reviewer 探索 codebase）可能几 MB。8MB 阈值 + truncated 标记可缓解，但 DB 体积增长。
**缓解**：8MB 阈值已是 trade-off（小于此 resume 失效多，大于此 DB 胀）；可后续加清理策略（归档 task 时清 messages_json）。C1 不做。

### R3（低）：resume 模式 memory 不重注
**问题**：design.md §2 决策——resume 绕过 build_worker_messages，不重注 memory。用户中途改 memory 文件，resume 不生效。
**缓解**：文档化为「resume 继承上轮 memory 快照」（design.md follow-up 已记）。review 场景 memory 变更频率低，可接受。

---

## Follow-up（C1 范围外，已记 design.md §8）

- 写 agent（implementer）resume 复用保留的 worktree
- 跨 session resume（Phase 2 跨 session 评审）
- resume 时 memory 变更检测
- messages_json 清理策略（task 归档时）

---

## 验证命令汇总

```bash
cd app/src-tauri
cargo test --lib migrate                    # Phase 1
cargo test --lib subagent_runs              # Phase 2/3
cargo test --lib resume                     # Phase 4
cargo test --lib definition_with_cache      # Phase 5
cargo test --lib                            # Phase 6.1 全量
cargo clippy --lib --tests -- -D warnings   # Phase 6.1 lint
```
