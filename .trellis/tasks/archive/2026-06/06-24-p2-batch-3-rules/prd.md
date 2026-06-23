# P2 batch: 清 3 条 open 债(A-005 / A-009 / B-003)

> **Status**: planning
> **Created**: 2026-06-24
> **Branch**: `main`
> **Estimated**: 4-6h(1 task batch)

---

## 1. Goal

把 DEBT.md 里 3 条 P2 open 债**一次性批量收口**:
- `RULE-A-005` head_sha 50 轮不刷新 → 每 turn 入口 refresh(semantically 等价于"每次 tool 后 refresh",LLM 在 provider.send 前 consume system_prompt)
- `RULE-A-009` 死代码抑制噪音 → 删 3 处未用变量 + 1 个永不构造 enum arm
- `RULE-B-003` sqlite_glob_match ? 分支 dead code → 删冗余块

**同步修**:DEBT.md 3 条 RULE 的 `File:` 引用因 06-23/24 文件 split 全部漂移(`chat.rs:362/528` → `chat_loop.rs:492/...` 等),本任务**顺手回填**真实路径(参 Session 68 同步模式)。

**不修**:路线图第三档功能 + 其他 P3 8 条债。

---

## 2. Background

### 2.1 DEBT.md 引用漂移(2026-06-23/24 split 后)

DEBT.md 三条 P2 RULE 引用是 `a4fb302` 审计基线(2026-06-14)的 file:line,06-23/24 连续 10 个文件 split(尤其 `chat.rs` → `chat.rs` + `chat_loop.rs`、`permissions/mod.rs` → `permissions/` 8 模块)后,所有 path 和行号都漂了。Session 68 同步任务**跳过了 P 等级债**(只动了 5 处源码注释),本任务补这一刀。

### 2.2 head_sha 风险面(LLM cache 验证)

`lookup_head_sha` → `build_system_prompt` → 拼出 `"ACTIVE on branch '{}' (HEAD {})"` 字符串段,最终通过 `assemble_system_prompt` 拼成 system_prompt 段。**关键问题**:这个段是否进 memory cache 的 `cache_control: Ephemeral` 块?

**验证结论**:`memory/loader.rs::build_instructions_blocks` 注入的 4 个指令文件块带 cache_control,**system_prompt 段是独立的 user message**(每轮都重新组装,不在 cache breakpoint 内)。所以 head_sha 字段每轮变化**不会破 memory cache**——cache key 只看 memory 那 4 块字节相同,system_prompt 在 cache block 之后,改它不影响 cache 命中。

> 这点 Session 68 漏验(只看了 head_sha 在 system_prompt.rs:68 拼字符串,没追溯到 cache_control 路径)。本任务 prd 已验,见 §6.1。

### 2.3 历史决策(参 ROADMAP §1.2 已实施)

- 06-10 B5 memory 重构:4 文件指令 + cache_control 注入
- 06-12 A2+B7 权限系统:5-tier path-based 决策层
- 06-23 split: `agent/chat.rs` 部分逻辑抽到 `agent/chat_loop.rs`(`run_subagent` 外移 + `assemble_system_prompt` 等);`permissions/mod.rs` → 8 模块

---

## 3. Scope

### 3.1 In Scope(3 处代码修改 + 1 处 DEBT.md 同步)

#### 修 1 — RULE-A-005: head_sha 每 turn 入口 refresh

- **现状**:`chat_loop.rs:492` `let head_sha = lookup_head_sha(&worktree_path);` 在主循环 spawn 前一次性查,然后 497 注入 `build_system_prompt`。
- **问题**:50 turn 不刷新;turn 3 commit 后 turn 4 system_prompt 的 HEAD SHA 与 `git log` 不一致,LLM 认知漂移。
- **修法**(实装:每 turn 入口 refresh,等价于"每次 tool 后 refresh"):
  - 把 `head_sha` 改成 `let mut head_sha = String::from(lookup_head_sha(&worktree_path));`,**主循环外**初始化一次,每 turn 入口(50 轮 `for turn in 1..=turn_limit` loop 顶部)重新查并 reassign。
  - 每 turn 顶部同步重新走 `build_system_prompt(..., &head_sha)` + `assemble_system_prompt(mode_prefix, &base_prompt)` 重建 `system_prompt`;`system_prompt` 也改为 `let mut system_prompt = ...` 以支持 reassign。
  - `system_prompt_override = Some(p)` 的 B6 worker 路径(23 参)走 `Some(p) => p` 短路,**不重新查 head_sha**(worker 不读 main 的 system_prompt,沿用 override)。
  - **为什么不是"每次 tool execute 后"而是"每 turn 入口"**:LLM 在每 turn 的 `provider.send` 前 consume system_prompt,所以 refresh 时机只需在 `provider.send` 之前;turn 入口即覆盖每个 turn 的 `provider.send`,等价于"每次 tool 后"(每次 tool execute 都在某 turn 内,下一个 turn 入口会 catch up)。
  - 成本:1 次 `lookup_head_sha` per turn(子毫秒 libgit2 调用)+ 1 次 `build_system_prompt` per turn(纯字符串拼接,无 IO)。
- **测试**:`tests_prompts.rs::head_sha_refresh_after_commit_updates_system_prompt`(新增 T1,118 行,真 `git2::Repository::init` + 2 commits,断言 `build_system_prompt` 反映新 SHA)。

#### 修 2 — RULE-A-009: 死代码抑制噪音

3 处未用/永不构造:

| # | File:Line | 内容 | 修法 |
|---|---|---|---|
| a | `chat_loop.rs:519` | `let _ = &base_prompt;` | 删除整行(match arm 走完 `Some(p)` 后 base_prompt 还在 scope 但 rustc 给 "value assigned to `base_prompt` is never read"——已确认 `base_prompt` 在 493 声明后 514-517 `assemble_system_prompt` 真用,**应无警告**;`let _ = &base_prompt;` 是历史抑制器,可删) |
| b | `chat_loop.rs:772` | `let _ = turn_send_at;` | 删除整行(`turn_send_at` 在 911 写入、1177 传参,rustc 应该无 warning,这是 06-23 split 后残留) |
| c | `chat_loop.rs:1077` | `ChatEvent::ToolResult { .. } => {}` | **保留 arm 但用 `#[allow(dead_code)]` 标注 arm**;若 grep 确认 `ChatEvent::ToolResult` 在全 crate 0 构造点,可考虑**删整个 enum 变体**(待 §6.2 验证) |

`llm/types.rs:359`(DEBT 引用)可能是 `ChatEvent::ToolResult` 变体定义位置——**修 c 决策点**:
- 选项 X(**推荐,已验**):删 `ChatEvent::ToolResult` 变体 + 删 `match` arm。`grep` 全 crate 确认 0 构造点(只有 types.rs:359 定义 + chat_loop.rs:1077 arm;`ContentBlock::ToolResult` 名字相同但不同 enum,在 wire.rs/types.rs 大量使用,不受影响)。

#### 修 3 — RULE-B-003: sqlite_glob_match ? 分支 dead code

- **现状**:`permissions/check.rs:386-430` `sqlite_glob_match` 处理 `?` 字符的分支。
- **问题**:外层 409 `if tbytes[ti] == b'/'` 判到 `/` 时,内层 417 `if tbytes[ti] == b'/'` 永远 true,419 `return false` 必达;`if let Some(sp) = star_pi` 块(411-425)永远不会进 `if` 体内。
- **修法**:
  - 删 411-425 整段 `if let Some(sp) = star_pi` 块;
  - 直接 `return false;`(回到 426 路径);
  - 修后 407-430 简化成 8 行:判 `/` → `return false`,否则 `pi+=1; ti+=1; continue;`。
- **测试**:`tests_check.rs`(从 `permissions/tests_*.rs` 找)有 glob_match 测试,验证 `?` 不跨 `/` 行为;修后这些测试应全 pass(行为不变,只是死代码没了)。

#### 修 4 — DEBT.md 3 条 file ref 同步

- `RULE-A-005` File: `app/src-tauri/src/agent/chat.rs:362/528` → `app/src-tauri/src/agent/chat_loop.rs:492/(refresh 插入点)`
- `RULE-A-009` File: `app/src-tauri/src/agent/chat.rs:432/512 + types.rs:357` → `app/src-tauri/src/agent/chat_loop.rs:519/772/1077 + llm/types.rs:357(变体定义)`
- `RULE-B-003` File: `app/src-tauri/src/agent/permissions/check.rs:386` → **不变**(line 仍准)

### 3.2 Out of Scope

- 路线图第三档(B9/C2/C6/B1/D2)任何功能
- P1 RULE-D-001 API key 加密(独立 task)
- 其他 P3 8 条债(独立 batch)
- 任何性能优化(本次是清理债,不是优化)

---

## 4. Acceptance Criteria

| ID | 条件 | 验证方式 |
|---|---|---|
| AC-1 | `cargo check --all-targets` 0 error 0 warning | 终端命令 |
| AC-2 | `cargo test --lib` 全绿(基线 813 + 新加 1 个 head_sha refresh test) | 终端命令 |
| AC-3 | `pnpm exec vue-tsc --noEmit` 0 error(零前端改动) | 终端命令 |
| AC-4 | 3 处代码修改 + DEBT.md 3 条 file ref 更新 全部 landed | `git diff --stat` |
| AC-5 | DEBT.md 在 PR merge 后**删除 3 条 RULE**(本文件 = open 集合) | 文件检查 |
| AC-6 | 1 个 commit batch(1 work commit) + 1 doc commit(DEBT) + 1 archive commit + 1 journal commit = 4 段式 | `git log --oneline -5` |
| AC-7 | trellis-check sub-agent 5 spec 全 PASS(同前几个 session 验证) | sub-agent dispatch |

---

## 5. Testing Strategy

### 5.1 既有测试覆盖

- `agent/tests_subagent.rs:1779-1850`:用真实 `lookup_head_sha`,验证 head_sha 注入 system_prompt 路径。
- `permissions/tests_check.rs`(若有):覆盖 `sqlite_glob_match` `?` `*` 字面 3 种 case。
- `agent/tests_*.rs`:涵盖 `build_system_prompt` + `assemble_system_prompt` + system_prompt override 路径。

### 5.2 新增测试

- **T1 — head_sha refresh after commit**(`agent/tests_*.rs`):
  - 用 temp git repo,init + commit "A" → lookup → expect SHA-1
  - commit "B" → refresh → expect SHA-2
  - 模拟"turn 4 system_prompt 反映新 SHA":把 SHA-2 注入 `build_system_prompt` 输出,grep 验证 "HEAD SHA-2" 出现。

### 5.3 不需新测试的修改

- **修 2a/2b**(`let _ = ...` 删除):cargo check 0 warning 即证。
- **修 2c**(`ChatEvent::ToolResult` 变体):若有 `cargo check` 0 error 即证(无构造点)。
- **修 3**(`sqlite_glob_match` 简化):既有 `?` 测试 pass 即证。

---

## 6. Risks & Decisions

### 6.1 head_sha refresh 不会破 memory cache ✓ 已验

`memory/loader.rs::build_instructions_blocks` 的 cache_control 块在 user message 头部(banner 块 + 4 个指令文件块),system_prompt 在其后的独立 user message,改 head_sha 字段**不破 cache key**。验证:`grep -n "cache_control" /usr/local/code/github/everlasting/app/src-tauri/src/memory/loader.rs | head -10` 应显示 cache_control 只在 `build_instructions_blocks` 函数内出现。

### 6.2 ChatEvent::ToolResult 决策点(修 2c)

需 grep 全 crate 看 `ChatEvent::ToolResult` 是否有任何构造点:

```bash
grep -rn "ChatEvent::ToolResult\|ChatEvent::ToolResult {" /usr/local/code/github/everlasting/app/src-tauri/src/ | head -20
```

- 0 匹配 → 选项 X(删变体 + arm),彻底干净
- ≥1 匹配 → 选项 Y(保留变体 + arm 标 `#[allow(dead_code)]`)

实施时验证。

### 6.3 turn_send_at 抑制器(修 2b)是否真无 warning

`let _ = turn_send_at;` 在 772,`turn_send_at` 767 声明,911 写入,1177 传出。rustc 应无 "binding never used" 警告(因为 1177 用了)。但 06-23 split 后 `let _` 可能是为了抑制某条 split 阶段消失的 warning;删后 cargo check 0 warning 即可,无 warning 即正确删除。

### 6.4 head_sha refresh 实现选型

`Rc<RefCell<String>>` vs `&mut String` vs 函数 `fn refresh_and_build(head_sha: &mut String, ...)` 闭包?

**实装选型**:`let mut head_sha: String` + reassign(等价于"owned String 重新赋值"——prd §6.4 原倾向 `&mut String` 借用,实装走 owned reassign 更直接,不引入 Rc/RefCell/借用生命周期顾虑):主循环外 `let mut head_sha = String::from(lookup_head_sha(&worktree_path));`;refresh 时 `head_sha = lookup_head_sha(&worktree_path);`(`String` drop 后重新赋值);`build_system_prompt(..., &head_sha)` 接受 `&str` 不变(解引用后 `&str` 自动 coerce)。

---

## 7. Files Impact

| File | 改动 | 来源 RULE |
|---|---|---|
| `app/src-tauri/src/agent/chat_loop.rs` | +~15 行(refresh 接入 + 修 2a/2b) | A-005 + A-009 |
| `app/src-tauri/src/agent/llm/types.rs`(可能)| -1 变体 + 测试 patch | A-009 (修 2c 选 X) |
| `app/src-tauri/src/agent/permissions/check.rs` | -15 行(? 分支简化) | B-003 |
| `app/src-tauri/src/agent/tests_*.rs`(1 个文件) | +~30 行(refresh test) | A-005 |
| `.trellis/reviews/DEBT.md` | 3 条 File 字段更新 + 3 条 RULE merge 后删除 | 同步 |
| `.trellis/workspace/Carlos-home/journal-3.md` | +1 Session entry | journal |

**0 前端改动**(vue-tsc 不应跑,AC-3 仅作"零影响"验证)。

---

## 8. Commit Plan(4 段式,参 memory `trellis-task-finish-commit-pattern.md`)

1. **commit 1 `fix(agent): close 3 P2 open rules`**:
   - `chat_loop.rs`(A-005 refresh + A-009 修 2a/2b + 修 2c 决策)
   - `check.rs`(B-003 简化)
   - `tests_*.rs`(新加 T1)
2. **commit 2 `docs(debt): update RULE-A-005/A-009 file refs after chat_loop split`**:
   - `DEBT.md`(3 条 File 字段 + Related Task 字段填 `.trellis/tasks/06-24-p2-batch-3-rules`)
3. **commit 3 `chore(task): archive 06-24-p2-batch-3-rules`**(自动)
4. **commit 4 `chore: record journal`**(自动)

注:DEBT.md **不在 commit 1 删 RULE 字段**,merge 后才删(避免 PR 阶段 RULE 误删后 audit 复查不到)。

---

## 9. References

- DEBT.md 3 条 RULE 原文:`.trellis/reviews/DEBT.md` §RULE-A-005 / A-009 / B-003
- Session 68 同步模式:`.trellis/workspace/Carlos-home/journal-3.md` "Session 68: 同步代码地图 + 文档引用"
- Trellis 收尾四段式 commit:`memory/trellis-task-finish-commit-pattern.md`
- head_sha 注入路径:`app/src-tauri/src/agent/system_prompt.rs:56-72`
- memory cache_control 路径:`app/src-tauri/src/memory/loader.rs::build_instructions_blocks`
