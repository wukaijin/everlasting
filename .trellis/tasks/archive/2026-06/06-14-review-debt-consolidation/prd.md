# Review 债务整合 + 跟进机制建设

## Goal

把 2026-06-14 全盘审计 (`docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md`) 的所有 P0/P1/P2/P3 发现 + 历史 review 长期悬挂的债务 (`REVIEW-sse-agent-loop` §8 4 条 0 落地) **合并到一个统一的 review backlog**,并建立 **review 跟进机制** (防止下次 audit 再次独立复述同一条 finding)。

审计基线: commit `a4fb302`(2026-06-14),Rust 28324 行 / 5 子系统

## Decisions (ADR-lite)

**Context**: 5 路并行深挖发现**历史 review 建议 0 条完全落地,被两路独立复述**(§3.4)。`REVIEW-sse-agent-loop` §8 的 4 条改进(data_buf cap / persist 失败 emit / GLM max_tokens 误分类 / mock 集成测试)在 2026-06-12 提出,2026-06-14 审计又把它们各自重新发现。这是**方法学问题**(没有 review 跟进机制),不是单条 finding 的问题。

**Decisions**:

1. **元任务定位**: 本 task 不实现任何具体修复,只做 **backlog 整合 + 机制建设**;具体修复 PR 由后续子 task 承担
2. **Backlog 文件**: 新建 `.trellis/reviews/DEBT.md`,集中追踪所有 review 发现的债务项
3. **每条 finding 必备字段**: `file:line` + 优先级 + owner + 关联 task + 状态 + commit hash(闭合时)
4. **跟踪节奏**: 每次新 audit 前**先 diff DEBT.md**,已记录的 finding 不再重复展开(只标"see DEBT.md §X.Y")
5. **优先级重审**: 接受我 meta-review 中提出的 5 处评级调整:
   - **A-P2-1 (messages.clone Arc<Vec>) 删掉,不做**(过度优化,损害借用边界)
   - **C-P1-3 降 P3**(过度压缩 ≠ 数据丢失)
   - **E-P3-1 升 P1**(`/tmp` fallback 是工作数据丢失路径)
   - **§3.5 集成测试缺口 升 P1**(P0 修复无回归保护等于盲修)
   - **§6 memory 1s debounce race 升 P1**(对应 C-P1-1,产品诚信问题)
6. **修复顺序**(meta-review 调整版):
   - **PR1-PR3**: P0 安全面 3 项独立 PR 并行(shell env / 进程组 / web_fetch SSRF)
   - **PR4**: C3 tail pair orphan + 超窗降级返回(同文件 context.rs,合并 1 PR)
   - **PR5**: MockProvider 集成测试框架(**前移**,为 P0/P1 修复提供回归保护)
   - **PR6+**: P1 正确性(persist_turn emit Error / audit 时序 / API key 加密 / OpenAI max_completion_tokens)
   - **PR7+**: P1 资源(glob spawn_blocking / worktree destroy await / worktree data_dir)
   - **PR-N**: P2 债务清理 + spec drift tracker
7. **明确不做**:
   - LLM summarization(C3-v2)
   - 前端 "context compressed" UI(C3 PR2 已 split,见 `06-12-c3-context-token` prd)
   - compressed_out DB 列 + 完整历史回看(C4 审计日志覆盖)
   - 二次取消语义(spec §2.5.1 当前未实现,留待 V3 评估)

**Consequences**:
- 后续 audit 不再独立复述已记录 finding,review 文档会显著缩短
- 修复 PR 编排有明确顺序,P0 安全面闭合可发布
- Meta-task 本身不写代码,只维护 DEBT.md 索引(developer responsibility)

---

## Requirements

### R1 — 创建 review backlog 跟踪文件

* `.trellis/reviews/DEBT.md` 存在,结构化记录所有 review finding
* 每条 finding 包含字段:`id` (RULE-{subsystem}-{seq}) / `level` (P0/P1/P2/P3) / `subsystem` / `file:line` / `description` / `fix_estimate` / `owner` / `related_task` / `status` (open/in_progress/closed) / `closed_at_commit`
* 初版至少包含 35 条 finding(本次审计 + 历史 review 合并)

### R2 — 初始 backlog 内容(从本次审计合并)

**P0 — 必须修复(安全 + 数据完整性,5 条)**:

| ID | Subsystem | Finding | File:Line | Fix | 关联 |
|---|---|---|---|---|---|
| RULE-A-001 | Agent Loop | C3 `group_droppable_turns` tail pair orphan | `context.rs:334-381` | 把 tail-adjacent assistant(tool_use) 纳入隐式保护,补单测 | 本 task 子 |
| RULE-A-002 | Agent Loop | `compact_messages` 超窗静默不丢 | `context.rs:160-260` | 全丢完仍超 target 时返回 Result/emit Error | 本 task 子 |
| RULE-E-001 | Tools | `shell` 子进程继承父进程全部环境变量(ANTHROPIC_API_KEY 泄漏) | `shell.rs:237` | `Command::new("sh").env_clear()` + 白名单注入(PATH/HOME/LANG/TERM) | 本 task 子 |
| RULE-E-002 | Tools | `shell` 不 kill 进程组 → 孤儿进程 | `shell.rs:79-99` | `process_group(0)` + kill PGID(-PID) | 本 task 子 |
| RULE-E-003 | Tools | `web_fetch` redirect 不重做 IP 校验 → SSRF 绕过 | `web_fetch.rs:385` | 自定义 `Policy::custom` 每 3xx 重新 `lookup_host + is_blocked`;同步修 spec 内部矛盾 | 本 task 子 |

**P1 — 重要(正确性 + 资源,12 条;含评级调整)**:

| ID | Subsystem | Finding | File:Line | Fix | 关联 |
|---|---|---|---|---|---|
| RULE-A-003 | Agent Loop | `persist_turn` 失败静默,DB 与内存永久分叉 | `chat.rs:439-447/875-886/1205-1216` | 失败时 emit Error(兑现 REVIEW-sse P3) | 本 task 子 |
| RULE-A-004 | Agent Loop | `record_tool_executed_audit` 在 cancel 检查之前 → audit 撒谎 | `chat.rs:1094-1116` | audit 提到 cancel 检查之后 | 本 task 子 |
| RULE-A-005 | Agent Loop | `head_sha` spawn 前查一次 50 轮不刷新 → LLM 认知漂移 | `chat.rs:362` | 每 N 轮或每次 tool 后刷新 system_prompt(原 P1,暂降 P2) | 本 task 子 |
| RULE-A-006 | Agent Loop | 集成测试缺口(turn 边界 cancel / max_turns / C3 触发无回归保护) | (新建) | `MockProvider` 实现 trait 返回预设 `Stream<ChatEvent>`,跑完整 chat 命令 | 本 task 子(**前移**到 PR5) |
| RULE-B-001 | Permission | `delete_session` 不直接清理 `permission_asks` | `commands/sessions.rs:126` | 先改 `cancel_session_asks` 按 session_id 过滤,再接入 delete_session | 本 task 子 |
| RULE-B-002 | Permission | `cancel_session_asks` 是 `map.clear()` 全清,session_id 被忽略(latent bug) | `mod.rs:330-341` | 同上 | 本 task 子 |
| RULE-C-001 | Memory | watcher debounce 1s 窗口内 race | `watcher.rs:179-219` + `loader.rs:206-214` | 加 read-through fence 或在 invalidate 时标记,read 检测 fence | 本 task 子 |
| RULE-C-002 | Memory | 新建 project / memory 文件不自动 watch | `state.rs:178-197` | `MemoryWatcher` 提升到 AppState,加 `add_watch(project_id)` | 本 task 子 |
| RULE-D-001 | Provider | API key 明文存储 | `db/migrations.rs:240` + `commands/providers.rs:38-42` | keyring crate 或应用层对称加密(`app_data_dir` 0700 非绝对边界) | 本 task 子 |
| RULE-D-002 | Provider | OpenAI `max_tokens` 对 o1+ 协议错误 | `openai.rs:243-248` | `is_o1_family` 分支改用 `max_completion_tokens` | 本 task 子 |
| RULE-E-004 | Tools | `glob` 用 sync `std::fs::read_dir` 阻塞 tokio runtime | `glob.rs:115/205-226` | `spawn_blocking` 包裹 walk_dir | 本 task 子 |
| RULE-E-005 | Tools | worktree destroy 不等 cancel 生效就删目录 | `commands/worktree.rs:237-260` | `cancel_inflight_for_session` 返回退出信号,destroy await | 本 task 子 |
| RULE-E-006 | Tools | `git::worktree::data_dir` 走 env 而非 Tauri path(`/tmp` fallback 是数据丢失路径) | `worktree.rs:40-56` | **(P3 升级 P1)**改用 Tauri `app_data_dir()`,跨平台一致 | 本 task 子 |

**P2 — 中等(健壮性 + 债务,15 条)**:

| ID | Subsystem | Finding | File:Line | Fix |
|---|---|---|---|---|
| RULE-A-007 | Agent Loop | error 路径 partial text 丢失 | `chat.rs:741-756` | Error arm persist 已累积 text(与 cancel 路径 `:796-805` 对齐) |
| RULE-A-008 | Agent Loop | `estimate_messages_tokens` 与 `_iter` 版大段重复 | `context.rs:85-133` vs `:275-317` | 抽公共 helper |
| RULE-A-009 | Agent Loop | 死代码抑制噪音 | `chat.rs:432/512` + `types.rs:357` | 删除未用变量 |
| RULE-B-003 | Permission | `sqlite_glob_match` 的 `?` 分支 dead code | `mod.rs:766-783` | 删除冗余分支 |
| RULE-B-004 | Permission | 危险命令检测有真实绕过路径(大小写敏感 + `find -delete` 走白名单) | `dangerous.rs:81-108` + `shell_trust.rs:108` | 加 `(?i)` + `find -delete` 黑名单 + 长选项 / 子 shell / env 展开检测 |
| RULE-B-005 | Permission | shell trust 结构降级 false positive(`grep "a|b"`) | `shell_trust.rs:365` | `cmd.contains('|')` 加引号 / 转义上下文检测 |
| RULE-C-003 | Memory | token 估算不反映 cache 折扣(P1 降 P3) | `context.rs:85-133` | 文档化或加 head pair cache 折扣 |
| RULE-C-004 | Memory | `MemoryWatcher` 不在 AppState 持有 | `state.rs:192-197` | 提升到 AppState,便于后续 add_watch |
| RULE-C-005 | Memory | user_dir 路径与 Claude Code 不一致 | `file.rs:58-66` | 决策:`~/.config/everlasting/`(当前)vs `~/.claude/`(Claude Code);产品层决策 |
| RULE-C-006 | Memory | 4 文件总大小无 cap | `mod.rs:54-60` | 加 ~400KB 总 cap |
| RULE-C-007 | Memory | watcher 路径表 fallback 按 `file_name()` 可能误触发 | `watcher.rs:331-339` | 加 parent_dir 校验 |
| RULE-D-003 | Provider | SSE parser 不容忍 `data:` 无空格 + `data_buf` 无上限(REVIEW-sse P2 复述) | `sse.rs:43-45/13` | 加 1 MiB cap + 容忍无空格版本 |
| RULE-D-004 | Provider | `WireRequest.reasoning_effort` dead field | `wire.rs:133` | 接通 `from_model_row` 或删字段 |
| RULE-D-005 | Provider | OpenAI `supports_reasoning_effort` caps hardcode true | `openai.rs:370-374` | 调 `WireCapabilities::from_model_row`(已实现) |
| RULE-E-007 | Tools | ReadGuard 进程内不持久,重启失效 | `read_guard.rs:17-21` | SQLite 持久化 fingerprint(可选,UX 退化可接受) |
| RULE-E-008 | Tools | `edit_file` `find_similar_lines` 大文件单行爆(minified bundle) | `edit_file.rs:277-306` | 行级 cap + 提前返回 |
| RULE-E-009 | Tools | `read_file` UTF-8 切片 panic 风险(中文/emoji ≥50KB) | `read_file.rs:222-225` | 同步 diff.rs `:298-302` 的 `floor_char_boundary` |
| RULE-E-010 | Tools | shell spillover 文件不日常清理 | `shell.rs:391-404` | LRU 清理或定期 background task |
| RULE-E-011 | Tools | worktree create self-heal 强制 `remove_dir_all` orphan 目录 | `worktree.rs:216-227` | 非空目录拒创建,返回错误 |
| RULE-D-006 | Provider | GLM max_tokens 500/400 误分类加 keyword(REVIEW-sse P4 复述) | `error.rs:129-136` | keyword 列表加 `max_tokens` |

**P3 — 轻微(文档/一致性,8 条)**:

| ID | Subsystem | Finding | File:Line |
|---|---|---|---|
| RULE-A-010 | Agent Loop | spec §2.5.1 二次取消语义未实现 | spec ARCHITECTURE.md §2.5.1 |
| RULE-B-006 | Permission | AuditKind docstring "10"→"11" | `mod.rs:140` vs `:152-179` |
| RULE-B-007 | Permission | Background Mode 仍空壳 | `types.rs:193` + `:1214` |
| RULE-C-008 | Memory | grill Q4 "AGENTS.md 物理顺序前置"未严格执行 | `loader.rs:321` |
| RULE-C-009 | Memory | WSL/9p/drvfs 下 inotify 可靠性未验证 | — |
| RULE-D-007 | Provider | OpenAI 多 tool_call `index` 缺失默认 0 | `openai.rs:593-597` |
| RULE-D-008 | Provider | `parse_anthropic_usage` 全零判 None 假设 | `anthropic.rs:617-627` |
| RULE-A-011 | Agent Loop | `A-P2-1 messages.clone Arc<Vec>` 过度优化,损害借用边界,**删除不做** | `chat.rs:461/529` |

**历史 review 债务合并(REVIEW-sse-agent-loop §8 4 条)**:
- RULE-D-003 (data_buf cap, P2) ✅ 已合并
- RULE-A-003 (persist 失败 emit Error, P1) ✅ 已合并(本任务 RULE-A-003)
- RULE-D-006 (GLM max_tokens keyword, P2) ✅ 已合并
- RULE-A-006 (mock 集成测试, P1) ✅ 已合并(本任务 RULE-A-006,**前移**)

### R3 — review 跟进机制

* `.trellis/reviews/DEBT.md` 顶部包含 **新增 finding 流程** 段落
* 流程要求: 任何新 audit 必须先 diff DEBT.md,已记录 finding 仅标 `// See DEBT.md §RULE-X-XXX` 一行引用,不重新展开
* 流程要求: 闭合 finding 时必须填 `closed_at_commit` + 关联 PR 链接
* 配套脚本(可选): `.trellis/scripts/review_debt.py` 统计 open/closed by priority

### R4 — spec drift tracker(可观测性问题)

* 单独建 `.trellis/reviews/SPEC-DRIFT.md`,集中记录 spec 与实现的**有意偏离**和**无意遗漏**
* 初版至少包含: 二次取消语义未实现(RULE-A-010)+ web_fetch redirect docstring 自相矛盾
* 后续 audit 维护

### R5 — 集成测试前移(P0 修复前置)

* MockProvider 实现 trait 返回预设 `Stream<ChatEvent>`,跑完整 chat 命令
* 覆盖场景: cancel 在 turn 2 tool 执行中 / max_turns 在 turn 50 / C3 在 turn 30 触发 / persist 失败 / audit 时序
* **必须在 RULE-E-001/002/003 + RULE-A-001/002 修复前完成**(否则修复无回归保护)

---

## Acceptance Criteria

### 必须满足(Definition of Done)

* [ ] `.trellis/reviews/DEBT.md` 创建并包含 ≥ 35 条 finding
* [ ] DEBT.md 每条 finding 字段完整(id/level/subsystem/file:line/fix/owner/status)
* [ ] `.trellis/reviews/SPEC-DRIFT.md` 创建并包含 ≥ 2 条 spec drift
* [ ] DEBT.md 顶部"新增 finding 流程"段落清晰可执行
* [ ] 本 prd.md 列出 5 个 P0 子 task(RULE-A-001, RULE-A-002, RULE-E-001, RULE-E-002, RULE-E-003)的 outline
* [ ] 本 prd.md 列出 MockProvider 集成测试子 task(RULE-A-006)outline,标注 **PR5 前移**
* [ ] ARCHITECTURE.md 加 §N "Review Backlog" 章节,指向 DEBT.md
* [ ] ROADMAP.md 第二档加"Review Debt Consolidation"分类,引用本 task

### 不必须满足(留给后续子 task)

* 各 P0/P1 具体修复(由子 task 实现)
* 集成测试代码本身(由 RULE-A-006 子 task 实现)
* DEBT.md 闭合(由各修复 PR 闭合)

---

## Definition of Done

* `.trellis/reviews/DEBT.md` 存在且内容齐全
* `.trellis/reviews/SPEC-DRIFT.md` 存在
* 本 prd.md 列出全部 P0 子 task 的 outline(每个有 goal / file:line / fix 方向 / 单测要求)
* ARCHITECTURE.md / ROADMAP.md 引用更新
* 后续 audit 时,新 finding 先查 DEBT.md 已记录的不重新展开

---

## Out of Scope

* 各 P0/P1 修复的代码实现(由本 task 衍生的子 task 各自实现)
* 集成测试代码(RULE-A-006 子 task)
* 二次取消语义实现(spec §2.5.1 偏离,V3 评估)
* LLM summarization(C3-v2)
* 前端 "context compressed" UI(C3 PR2)
* compressed_out DB 列(C4)
* review 跟进机制的 CI 自动化(后续评估)

---

## Technical Approach

### 本 task 实施步骤

**Step 1: 创建 review 目录结构**
- 新建 `.trellis/reviews/` 目录
- 创建 `DEBT.md`(见 R1/R2)
- 创建 `SPEC-DRIFT.md`(见 R4)
- 创建 `README.md`(索引,指向 DEBT.md 和 SPEC-DRIFT.md)

**Step 2: DEBT.md 编写**
- 按 R2 表填写全部 ≥ 35 条 finding
- 顶部"流程"段:
  ```
  ## 新增 finding 流程
  1. 先 diff 本文件,已记录的 finding 不重新展开(仅引用 §RULE-X-XXX)
  2. 新 finding 按模板添加:id / level / subsystem / file:line / fix / owner / status
  3. 闭合时填 closed_at_commit + 关联 PR 链接
  ```

**Step 3: 子 task outline 编写**
本 prd.md 已经包含 5 个 P0 子 task + 1 个集成测试子 task 的 outline。下一步每个子 task 单独建 task 目录(用 `task.py create` + `--parent 06-14-review-debt-consolidation`):

| 子 task 名 | 对应 finding | 建议 PR 顺序 |
|---|---|---|
| `06-14-p0-shell-env-clear` | RULE-E-001 | PR1 |
| `06-14-p0-shell-process-group` | RULE-E-002 | PR2 |
| `06-14-p0-web-fetch-redirect-ssrf` | RULE-E-003 | PR3 |
| `06-14-p0-c3-tail-pair-orphan` | RULE-A-001 + RULE-A-002 | PR4(同文件,合并) |
| `06-14-p1-agent-loop-integration-tests` | RULE-A-006 | **PR5(前移)** |

P1 各项作为独立子 task 按需开启:
- `06-14-p1-persist-turn-emit-error` (RULE-A-003)
- `06-14-p1-audit-cancel-order` (RULE-A-004)
- `06-14-p1-permission-asks-cleanup` (RULE-B-001 + B-002)
- `06-14-p1-memory-watcher-appstate` (RULE-C-001 + C-002)
- `06-14-p1-api-key-encryption` (RULE-D-001)
- `06-14-p1-openai-o1-max-completion-tokens` (RULE-D-002)
- `06-14-p1-glob-spawn-blocking` (RULE-E-004)
- `06-14-p1-worktree-destroy-await` (RULE-E-005)
- `06-14-p1-worktree-data-dir-tauri` (RULE-E-006, P3 升 P1)

P2 / P3 各自子 task 按需开启。

**Step 4: ARCHITECTURE.md + ROADMAP.md 更新**
- ARCHITECTURE.md 新增 §N "Review Backlog",指向 `.trellis/reviews/DEBT.md`
- ROADMAP.md 第二档加 "Review Debt Consolidation" 分类,引用本 task + DEBT.md
- 这两步保证 review 跟进机制可发现

---

## Technical Notes

### 关键参考文件

* `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` — 本次审计原文(基线)
* `docs/_reviews/REVIEW-sse-agent-loop-2026-06-12.md` — 历史 review 债务源
* `docs/_reviews/REVIEW-a2-b7-permission-mode-plan-2026-06-13.md` — 历史 review(已大部分解决,无新 debt)
* `docs/_reviews/REVIEW-b5-memory-grill-2026-06-10.md` — 历史 review(7 ✅ / 2 ⚠️)

### 元任务特殊性

* 本 task 不写任何 agent 代码,只维护 `.trellis/reviews/` 跟踪文件
* 真正的修复在子 task 中
* "集成测试前移"(RULE-A-006,PR5)是**关键路径**,必须在 P0 安全面修复前后都给回归保护
* review 跟进机制是**长期收益**,避免下次 audit 又独立复述已记录 finding(本次审计就复述了 REVIEW-sse §8 4 条)

### 评级调整依据(meta-review 产出)

* **A-P2-1 (Arc<Vec> clone) 删掉**: 8MB churn 在现代机器上不是性能问题,反而 `Arc<Vec>` 破坏借用边界、增加 atomic 开销、可变性表达不清晰
* **C-P1-3 降 P3**: 过度压缩 ≠ 数据丢失,对用户影响是"对话历史少保留几轮"而非"功能崩坏"
* **E-P3-1 升 P1**: `/tmp` fallback 重启清空 = 用户工作数据丢失,跨平台部署的硬阻塞
* **§3.5 升 P1**: Agent Loop 无集成测试意味着 P0 修复无回归保护,等于盲修,补 MockProvider 是最高 ROI 测试投入
* **§6 memory debounce race 升 P1**: PRD 文案承诺"立即生效"但实际是概率性,产品诚信问题

### SPEC-DRIFT.md 初版内容

1. **二次取消语义未实现** — spec ARCHITECTURE.md §2.5.1 要求"取消不立即终止,把'取消'作为 tool_result 回传给 LLM 一次自我收敛机会",当前实现单次 cancel 即终止。状态:有意识偏离(MVP 简化),记录待 V3 评估。
2. **web_fetch redirect docstring 自相矛盾** — `web_fetch.rs:17` 写"each redirect target",`web_fetch.rs §5 security notes` 又写"not implemented"。状态:无意识遗漏(spec drift,实施 RULE-E-003 时同步修)。

---

## Research References

* `docs/_reviews/REVIEW-agent-loop-full-audit-2026-06-14.md` — 本 task 的直接输入
* `.trellis/spec/backend/agent-loop/index.md` — agent loop spec(若已建)
* `.trellis/spec/backend/permissions/index.md` — permission spec(若已建)
* ARCHITECTURE.md §2.5.1 — 二次取消语义(spec 原文)