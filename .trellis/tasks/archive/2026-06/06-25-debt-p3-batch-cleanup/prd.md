# 清理 DEBT P3 批次（docstring + OpenAI tool_call index + test unhandled rejection）

> **状态**：范围已通过 AskUserQuestion 对齐（2026-06-25），finding 全部经代码验证，待 Phase 2 实施。

## Goal

收口 DEBT.md 里 3 条**带真实正确性 / 稳定性风险**的 P3 finding，并把 1 条已被注释充分辩护的 finding（RULE-D-008）从 DEBT 移除。**DEBT 8 → 4**。

## 触发（2026-06-25）

- session-fallback 任务名 `06-24-debt-remove-3-closed-rules`（stale，目录已被清空，忽略）
- 用户记忆「推荐任务前先查 DEBT.md（open 债优先）」→ 查 DEBT：0 个 P0/P1/P2，8 个 P3
- AskUserQuestion 选「清 DEBT P3 批次」——推新功能前的清债窗口

## 范围

### ✅ In scope —— 3 条代码修复

| RULE | 文件 | 风险 |
|------|------|------|
| RULE-B-006 | `audit.rs:21` | 文档自相矛盾（`:4` 写 17，`:21` 写 10）|
| RULE-D-007 | `openai.rs:750-754` | 第三方 OpenAI 兼容层 tool_call 丢失（正确性）|
| RULE-FrontTest-001 | `streamController.test.ts` | 未来 Vitest 升级 unhandled rejection 变硬 fail（稳定性）|

### 🗑 DEBT 移除（保留代码现状）—— 1 条

- **RULE-D-008** `anthropic.rs:762`：全零判 None。**注释已为该设计充分辩护**（`anthropic.rs:763-771`："all-zero as no payload so agent loop sees `usage: None` and skips the SQL write"，属 D4 决策）。DEBT 的 "Fix: 改 None if not_present" 反而会改变语义、可能让 agent loop 对合法的空 usage 误写 SQL。**结论：保留现状，从 DEBT 移除**（注释即 rationale）。

### ⏸ Out of scope —— 决策类 / 拆独立 task

- **RULE-B-007**（`mode.rs:26-28` Background 空壳）：决策类（移除 enum or 保留预留），不在代码清理批次
- **RULE-C-008**（`loader.rs:321` AGENTS 物理顺序）：决策类
- **RULE-FrontSubagent-001**（抽 `ToolCallHeader.vue`）：中等工作量，需同时改 `ToolCallCard.vue` 本体，**拆独立 task**
- **RULE-FrontSubagent-002**（`transcriptPairing.ts` 抽 composable）：同上，**拆独立 task**

## 每条详细方案

### 1. RULE-B-006 — audit.rs docstring "10" → "17"

- **现状**：`audit.rs:4` 模块头 docstring 写 "17 variants"（2026-06-23 拆分时写，较新且准确）；`audit.rs:21` enum 上方 docstring 写 "10 variants — see PRD ..."（A2 时期遗留，未随 C4 `ToolExecuted` / D3 `MessageEdited`/`ResendMessage` / Worker 域新增更新）。两处自相矛盾。
- **修复**：`audit.rs:21` 改为 "17 variants"（与模块头一致）。顺便核对变体数 = 17（Tool 域 5 + Permission 域 3 + Mode 域 3 + Message 域 + Worker 域）。
- **行数**：1 行。

### 2. RULE-D-007 — OpenAI tool_call index 缺失不再 silent 默认 0

- **现状**（`openai.rs:749-757`）：
  ```rust
  for tc in tcs {
      let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
      let entry = tool_call_state.entry(idx).or_insert_with(ToolCallBuf::default);
      // ... 无条件 entry.id = / entry.args.push_str(...)
  }
  ```
- **风险**：官方 OpenAI API 每个 tool_call delta 必带 `index`（0/1/2…），安全。但第三方 OpenAI 兼容层（ DeepSeek / 某些 proxy）可能省略 `index` → 两个无 index 的 tool_call 都落 `idx=0`，第二个的 `id` / `args` **覆盖**第一个（HashMap 同 key，`or_insert_with` 不重建，后续赋值覆盖）→ 第一个 tool_call 参数丢失 / 错乱。
- **修复方案**：index 缺失时 `tracing::warn!` 一次 + `continue` 跳过该 delta（用 `let … else` / `let-else`）。比"错乱覆盖导致 tool_use 拼错参数"安全 —— 官方 API 零影响，无 index proxy 会被显式跳过而非静默错乱。
  ```rust
  let Some(idx_u64) = tc.get("index").and_then(|i| i.as_u64()) else {
      tracing::warn!("openai: tool_call delta missing `index`, skipping (third-party proxy?)");
      continue;
  };
  let idx = idx_u64 as u32;
  ```
- **权衡**：跳过 delta 意味着无 index 的 tool_call 永不 emit —— 但这优于错乱。官方 API 不受影响。
- **测试**：补一个单测——构造两个无 index 的 tool_call delta，断言不会把两者 args 拼到同一 idx（即触发 warn + 跳过，不产生错乱 tool_use）。看 openai.rs 现有 test 结构（`#[cfg(test)]` mod）确定 fixture 形态。

### 3. RULE-FrontTest-001 — streamController.test.ts 补 __TAURI_INTERNALS__ mock

- **现状**：`streamController.test.ts`（1393 行，4 个 `beforeEach` @ 297/652/814/1368）**未** stub `__TAURI_INTERNALS__`。`reloadAfterFinalize`（`streamController.ts:1256` 附近）内 `invoke("list_sessions")` 走真实 `window.__TAURI_INTERNALS__.invoke` → 4 个 unhandled promise rejection。tests 本身全 pass（rejection 异步、测试结束才浮出），但 `Errors: 4` 给全量 vitest run 噪音，且未来 Vitest 升级可能把 unhandled rejection 变硬 fail。
- **修复**：定位走真实 invoke 的具体 test case（grep `reloadAfterFinalize` 在 test 里的调用点），在其 `beforeEach` 加 `vi.stubGlobal("__TAURI_INTERNALS__", { invoke: vi.fn().mockResolvedValue([]) })`，`afterEach` `vi.unstubAllGlobals()`。
- **验证**：全量 vitest run 的 `Errors: 4` → `0`。

### 4. RULE-D-008 — DEBT 移除（不改代码）

- 见上「DEBT 移除」段。`anthropic.rs:762` 全零判 None 保留，注释即 rationale。从 DEBT.md 删除该条 + 更新优先级分布表（P3 8 → 实施后 4）。

## 验证

- `cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`（RULE-B-006 不影响测试；RULE-D-007 新增单测 pass）
- `cd app && pnpm vitest run`（RULE-FrontTest-001：`Errors: 4` → `0`，全绿）
- `cd app && pnpm exec vue-tsc --noEmit`（前端类型不回归）

## DEBT 收尾（四段式 commit）

按记忆「Trellis 任务收尾四段式 commit」+ DEBT.md 闭合即删约定：

1. `fix(debt): RULE-B-006/D-007/FrontTest-001 P3 cleanup` —— 代码改动
2. `docs(debt): close RULE-B-006/D-007/D-008/FrontTest-001` —— 从 DEBT.md 删 4 条 + 更新分布表
3. `chore(task): archive 06-25-debt-p3-batch-cleanup` —— `task.py archive`（auto-commit）
4. journal 记录（可选 IMPLEMENTATION §4 ADR 若有决策）
