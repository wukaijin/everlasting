# 修复 worker system_prompt dead code (B6 defect A)

## Goal

修复 `app/src-tauri/src/agent/chat_loop.rs:2052` 的 `_worker_system_prompt` dead code bug——目前 `run_subagent` 调用 `assemble_subagent_prompt(def, task)` 但结果赋给 `_` 直接丢弃；嵌套的 `run_chat_loop` 仍使用 parent 的 `assemble_system_prompt(mode_prefix, base_prompt)` 输出，导致 worker 不知道自己是谁、prompt 与权限行为矛盾。

## Background

B6 review `docs/review/b6-subagent-assessment.md` §2 标记为"关键缺陷"：
- `SubagentDef.system_prompt`（`subagent.rs:163-175` researcher、`:185-196` general-purpose）精心编写但**完全是 dead code**
- Worker 实际得到的是 parent 的 `assemble_system_prompt` 输出（包含 parent 的 mode_prefix 和 behavior_prompt）
- 在 Edit/Plan mode 下 prompt 写"可写"但 Tier 4 auto-deny 拒绝写入——**prompt 与现实矛盾**
- `chat_loop.rs:2053-2059` 已标记为 "PR1b Deviation"

## What I already know

### 现状（来自代码）

- `_worker_system_prompt = assemble_subagent_prompt(def, task)` 在 `chat_loop.rs:2052` 被丢弃
- 嵌套 `run_chat_loop` 调用点：`chat_loop.rs:2188-2227`，22 个参数
- `run_chat_loop` 自身签名：`chat_loop.rs:129`，22+ 参数（已有 `#[allow(clippy::too_many_arguments)]`）
- `run_chat_loop` 内部调用 `assemble_system_prompt(mode_prefix, base_prompt)`（`system/prompt.rs:103`）
- `assemble_subagent_prompt(def, task)` 已经在 `subagent.rs:222` 实现，按 doc comment 是"完全替换 parent 的 behavior_prompt + mode_prefix + base_prompt"
- 所有其他 callers of `run_chat_loop`：chat 命令（parent 路径）+ tests
- 现有 test 在 `app/src-tauri/src/agent/tests.rs`，需要检查 `run_chat_loop` 测试是否需要更新

### 设计意图（已有 doc comment 确认）

- **Worker 完全替换 parent 的 system prompt**（`subagent.rs:217-228` doc comment）——"the worker does NOT inherit the main system prompt (Claude Code convention)"
- mode-specific permission 边界在 ⑨ 关层 enforce（不在 prompt）
- `assemble_subagent_prompt` 已经返回 `def.system_prompt.clone()`（task 走 user message，不在 system）

### 关键约束

- 已有 `#[allow(clippy::too_many_arguments)]`，再加一个 param 不增加 lint debt
- `assemble_system_prompt` 签名不动（2-param 保留）
- `run_subagent` 已有 `app_handle: Option<AppHandle>` 模式——加一个 `Option` 类型 param 跟既有风格一致

## Open Questions

### 已决策 ✅

- **Q1 = A**：`run_chat_loop` 加 `system_prompt_override: Option<String>` 参数（最小改动）
- **Q2 = 全替换**：worker 完全替换 parent system prompt（按 `assemble_subagent_prompt` 现有 doc comment）
- **Q3**：`assemble_system_prompt(mode_prefix, base_prompt)` 签名不变
- **Q4**：`assemble_subagent_prompt` 在 `run_subagent` 内调用，结果以 `String` 形式传给 `run_chat_loop`（保持 chat_loop 对 SubagentDef 不可知）

### 实现期需要处理的默认决定

- `clippy::too_many_arguments`：`run_chat_loop` 加 1 个 param 后可能触发；预先在 `run_chat_loop` 签名加 `#[allow(clippy::too_many_arguments)]`（与 `run_subagent` 一致）
- Test caller 更新：`app/src-tauri/src/agent/tests.rs` 所有 `run_chat_loop` 测试调用加 `None` 作为末位参数；如有 helper，统一加 None
- 缓存影响：system prompt 不在 cache_control 路径上（4 指令文件独立 breakpoint），override 不影响 prompt caching
- `assemble_subagent_prompt(_task: &str)`：保持当前签名（task 走 user message），不动
- Dead code 注释：`chat_loop.rs:2053-2059` "PR1b Deviation" 注释替换为正常实现说明

## Requirements (evolving)

- `run_chat_loop` 加 `system_prompt_override: Option<String>` 参数
- worker 调用路径传入 `Some(assemble_subagent_prompt(def, task))`
- parent 调用路径传入 `None`（保持现有行为）
- `run_chat_loop` 内部判断：override is Some → 直接使用；None → 走原有 `assemble_system_prompt(mode_prefix, base_prompt)`
- 移除 `chat_loop.rs:2052-2059` 的 `_worker_system_prompt` dead code + 注释
- 所有 `run_chat_loop` tests 同步更新（添加 None param 或调整 helper）
- 新增单测：worker 路径使用 override、parent 路径不受影响

## Acceptance Criteria (evolving)

- [ ] `run_chat_loop` 签名增加 `system_prompt_override: Option<String>` 参数
- [ ] worker 路径 `run_subagent` 调用 `run_chat_loop` 时传入 `Some(assemble_subagent_prompt(def, task))`
- [ ] parent 路径调用 `run_chat_loop` 时传入 `None`
- [ ] `_worker_system_prompt` dead code 移除，doc 注释改写为正常注释
- [ ] `run_chat_loop` 内部：`override.is_some()` 时跳过 `assemble_system_prompt`，直接用 override
- [ ] 新增测试：worker 路径 system prompt 等于 `def.system_prompt`
- [ ] 新增测试：parent 路径 system prompt 仍走 `assemble_system_prompt`（不变）
- [ ] `cargo test --lib` 全部通过
- [ ] `cargo check` 通过
- [ ] 不破坏现有任何 caller（chat 命令 + 其他测试）

## Definition of Done

- 代码 + cargo test + cargo check 全绿
- 新测试覆盖 worker override 路径
- doc 注释更新（移除 "PR1b Deviation" 标记，加 "Fixed in PR" 备注）
- 在 PR 描述或 commit message 链接本 prd.md
- 在 `docs/review/b6-subagent-assessment.md` 追加 §2 的"已修复"小节

## Decision (ADR-lite)

**Context**: B6 review `docs/review/b6-subagent-assessment.md` §2 标记 worker system_prompt 为"关键缺陷"——`_worker_system_prompt` dead code (`chat_loop.rs:2052`) 让 worker 实际用 parent 的 system prompt，导致 prompt 与权限行为矛盾、SubagentDef.system_prompt 是 dead code。

**Decision**:
1. `run_chat_loop` 加 `system_prompt_override: Option<String>` 参数（最小签名改动，与既有 Option 风格一致）
2. 内部加守卫：`override.is_some()` → 直接使用；`None` → 走原有 `assemble_system_prompt(mode_prefix, base_prompt)`
3. worker 路径（`run_subagent` → `run_chat_loop`）传入 `Some(assemble_subagent_prompt(def, task))`；parent 路径传 `None`
4. 移除 dead code 注释 + 在 `run_chat_loop` 签名加 `#[allow(clippy::too_many_arguments)]`

**Consequences**:
- ✅ Worker 真正使用 SubagentDef.system_prompt（自治完成）
- ✅ Prompt 与权限行为一致（worker prompt 写"可写"时即在 yolo mode，prompt 写"只读"时即 read-only）
- ✅ 签名只 +1 param，波及面：chat command caller + 所有 run_chat_loop tests
- ⚠️ run_chat_loop 已 22+ args，再 +1 触发 clippy::too_many_arguments；预先加 allow（与 run_subagent 一致）
- ⚠️ 现有 test caller 需同步加 None param

## Implementation Plan (1 PR)

### PR1 — 修复 worker system_prompt override

1. 改 `app/src-tauri/src/agent/chat_loop.rs:129`：
   - `run_chat_loop` 签名末尾追加 `system_prompt_override: Option<String>`
   - 加 `#[allow(clippy::too_many_arguments)]`
   - 内部调用 `assemble_system_prompt` 前加守卫
2. 改 `app/src-tauri/src/agent/chat_loop.rs:2050-2059`：
   - 删除 `_worker_system_prompt` dead code
   - 删除 "PR1b Deviation" 注释
3. 改 `app/src-tauri/src/agent/chat_loop.rs:2188` 嵌套调用：
   - 末尾加 `Some(assemble_subagent_prompt(def, &task))`（task 已在 line 2016 解析为 `task: &str`）
4. 改 `app/src-tauri/src/agent/chat_loop.rs:run_subagent` 顶部的 `run_chat_loop` 调用（**parent 路径**）：
   - 末尾加 `None`
5. 更新 `app/src-tauri/src/agent/tests.rs` 所有 `run_chat_loop` 测试调用：末尾加 `None`
6. 新增单测（在 `tests.rs`）：worker path system prompt 等于 `def.system_prompt`
7. 跑 `cargo test --lib` + `cargo check` 验证
8. 更新 `app/src-tauri/src/agent/subagent.rs:217-228` doc comment：移除 "is used for documentation only"，改 "active since 2026-06-21"

**不依赖任何前置 PR**，可独立 merge。

## Out of Scope (本期明确不做)

- 修其他 review 缺陷（defect B 等，独立任务）
- Drawer 改造（独立任务 `06-21-redesign-subagent-drawer-entry-as-toolcard-style`）
- SubagentDef system_prompt 内容优化
- 新增 subagent 类型
- worker 模型覆盖 / context_window 覆盖（v2 OOS）
- `run_chat_loop` 整体参数重构（独立任务，本次只 +1 param）
- prompt caching 影响分析（4 指令文件独立，system 不在 cache_control 路径上）

## Technical Notes

- 涉及文件：
  - `app/src-tauri/src/agent/chat_loop.rs`（主改动：函数签名 + 调用点 + dead code 清理）
  - `app/src-tauri/src/agent/subagent.rs`（仅 doc 更新：移除 "is used for documentation only"，改 "active in PR X"）
  - `app/src-tauri/src/agent/tests.rs`（如有 `run_chat_loop` 测试 helper，加 None param）
- 关键约束：
  - `#[allow(clippy::too_many_arguments)]` 已在 `run_subagent` 上；`run_chat_loop` 上**无此 allow**，加 1 个 param 后可能触发 lint warning——需要 verify
  - `run_chat_loop` 调 `assemble_system_prompt` 的具体行号待 Phase 2 implement 时定位
  - Worker 的 4 指令文件加载走 `build_worker_messages`（独立 cache_control breakpoint），与 system_prompt override 正交