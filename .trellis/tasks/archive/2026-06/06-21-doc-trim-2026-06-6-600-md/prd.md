# doc-trim-2026-06: 拆分/瘦身 6 篇超 600 行 md

## Goal

把项目里 6 篇超 600 行 markdown 文件瘦身,目标是只保留"当前 source of truth",把已解决债项 / 已落地特性 / 历史 ADR / 错放主题的内容迁出或删除,降低新人上手 / 日常查阅 / LLM 上下文加载成本。

不涉及代码修改、不改 spec 内容语义(只挪位置)。

## Final Decisions

| # | 决策点 | 选择 |
|---|---|---|
| D1 | 范围 | 全部 6 篇一次性做 |
| D2 | DEBT.md 已关闭项 | 硬删除(不留 archive) |
| D3 | BACKLOG.md 已落地章节 | 硬删除 + 一行 cross-ref |
| D4 | HACKING-wsl.md 坑 12 | 硬删除(实施前 grep 验证 tool-contract.md 是否已有 shell spillover 文档;没有则 flag) |
| D5 | agent-loop-architecture.md RULE-A-015/A-007 详细解释 | 仅 cross-ref,不搬内容 |
| D6 | database-guidelines.md subagent_runs | 拆分为独立 `.trellis/spec/backend/subagent-runs-schema.md` |
| D7 | IMPLEMENTATION.md 2026-06-04/05 早期条目 | 归档到 `.trellis/spec/archive/implementation-inception-2026-06-04-to-05.md` |
| D8 | Commit 粒度 | 单 commit 含全部 7 文件改动 + 新文件创建 |
| D9 | llm-contract.md 拆分粒度 | (待收敛) |

## What I already know

### 待瘦身的 6 篇文件

### 7 篇文件 (R7 待定)

| # | 文件 | 当前行数 | 瘦身后估计 | 类型 |
|---|---|---:|---:|---|
| 1 | `.trellis/reviews/DEBT.md` | 1055 | ~300 | trim-resolved (硬删除) |
| 2 | `docs/BACKLOG.md` | 732 | ~400 | trim-resolved (硬删除) |
| 3 | `docs/HACKING-wsl.md` | 613 | ~570 | trim-misplaced (坑 12 硬删除) |
| 4 | `.trellis/spec/backend/agent-loop-architecture.md` | 829 | ~655 | trim-historical (cross-ref) |
| 5 | `.trellis/spec/backend/database-guidelines.md` | 1073 | ~770 | split (subagent_runs 拆出) |
| 6 | `docs/IMPLEMENTATION.md` | 786 | ~740 | trim-historical (归档) |
| 7 | `.trellis/spec/backend/llm-contract.md` | 2290 | TBD | split (R7 待定) |

### DEBT.md 当前 open 条目(共 9 条,保留)

- P1: RULE-D-001 (API key 明文)
- P2: RULE-B-003 (sqlite_glob_match dead code), RULE-A-009 (死代码抑制噪音), RULE-A-005 (head_sha spawn 前查一次)
- P3: RULE-B-006 (AuditKind docstring), RULE-B-007 (Background Mode 空壳), RULE-C-008 (AGENTS.md 物理顺序), RULE-D-007 (OpenAI 多 tool_call index), RULE-D-008 (parse_anthropic_usage 全零判 None)

### 6 篇都保持现状的不动(10 个文件)

ARCHITECTURE / popover-pattern / reka-ui-usage / state-management / worktree-contract / memory / workflow / tool-contract(主体) / llm-contract / multi-provider-contract

## Requirements

### R1: DEBT.md 硬删除关闭项

- 删 §P0 (60-136) 全部 5 条 closed
- §P1 中删 12 项 closed,留 RULE-D-001 一条
- §P2 中删 18 项 closed,留 RULE-B-003 / RULE-A-009 / RULE-A-005 三条
- §P3 中删 6 项 closed,留 RULE-B-006 / RULE-B-007 / RULE-C-008 / RULE-D-007 / RULE-D-008 五条
- 删 FT 全 6 项 closed
- 删 §历史 review 债务合并追踪 (925-983) 整段
- §Re-evaluation Log (984-1054) 保留作为"已关闭条目 commit 索引"
- 头部加注:`> 已关闭条目不在此文档保留;通过 git log 或 Re-evaluation Log 追溯`

### R2: BACKLOG.md 硬删除 + cross-ref

- 删 §0.5 (32-55) transition marker 整段
- §1 (56-203) 替换为一行:`## 1. 输入层扩展 → 已落地 (B2 @file 2026-06-17, B3 /command 2026-06-17),详见 ROADMAP §1.2;§1.1 多模态缓做 (ROADMAP §3)`
- §2 (204-239) 替换为一行:`## 2. Agent Skill 系统 → 已落地 (B4 2026-06-18),详见 ROADMAP §1.2`
- §5.1 标记 `~~cwd→~/~~ (已落地 2026-06-06)` strikethrough
- 保留 §5.2 / §5.3 (仍 current)
- 保留附录 A (382-676 远期候选)
- §0 全局视角保留(是有用导览)

### R3: HACKING-wsl.md 坑 12 硬删除

- **先 grep 验证** `.trellis/spec/backend/tool-contract.md` 是否已有 shell spillover 文档
- 若有 → 直接删 HACKING-wsl.md 311-338 行
- 若无 → flag 给用户决定(可能改为"迁到 tool-contract.md")
- 删后检查坑编号连贯(原 1-12 → 现在 1-11,无空洞)

### R4: agent-loop-architecture.md cross-ref

- 不删除 509-683 行的内容
- 仅在该段顶部加一行:`> 历史 ADR 详见 [IMPLEMENTATION.md §4 2026-06-17 RULE-A-007 / 2026-06-20 RULE-A-015](IMPLEMENTATION.md)`
- 不修改原内容
- 净增 1 行(可视为 0 净变化)

### R5: database-guidelines.md subagent_runs 拆分

- 创建 `.trellis/spec/backend/subagent-runs-schema.md`
- 把 database-guidelines.md 809-1073 行 subagent_runs 整段迁过去
- 新文件头部加注:`<!-- Schema spec for subagent_runs table. Moved from database-guidelines.md 2026-06-21 (B6 PR2) -->`
- database-guidelines.md 末尾加 cross-ref:`## 参见: [subagent-runs-schema.md](subagent-runs-schema.md) (B6 PR2 2026-06-20)`

### R6: IMPLEMENTATION.md 早期条目归档

- 创建 `.trellis/spec/archive/implementation-inception-2026-06-04-to-05.md`
- 把 IMPLEMENTATION.md 735-782 行 (2026-06-04/05 项目启动期决策) 迁过去
- archive 文件头部加注:`<!-- ARCHIVED 2026-06-21: 只读历史,不再追写。源: docs/IMPLEMENTATION.md §4 (2026-06-04/05 段) -->`
- 主 IMPLEMENTATION.md 头部加注:`> 2026-06-04/05 项目启动期决策见 [archive/implementation-inception-2026-06-04-to-05.md](../.trellis/spec/archive/implementation-inception-2026-06-04-to-05.md)`

### R7: llm-contract.md 拆 3 个 scenario

- **Latency Tracking (F5)** 1393-2217 行 (~824 行) → 新文件 `.trellis/spec/backend/latency-tracking.md`
- **Token Usage Tracking (A4)** 671-1245 行 (~574 行) → 新文件 `.trellis/spec/backend/token-usage-tracking.md`
- **Per-Session Mode + ⑨ 关 Permission Layer (A2+B7)** 388-670 行 (~282 行) → 新文件 `.trellis/spec/backend/permission-layer.md`
- llm-contract.md 主文件保留: Overview + Extended Thinking (42-321) + 3 Decisions + Gotcha (354-387) + Common Mistakes + Anti-Patterns + DeepSeek fix (2218-2290)
- 主文件头部加注:`> 详细 scenario 见 [latency-tracking.md](latency-tracking.md) / [token-usage-tracking.md](token-usage-tracking.md) / [permission-layer.md](permission-layer.md)`
- 主文件行数预计 ~580 行
- 已知遗留: permission-layer.md 与 tool-contract.md 1269-1619 ⑨ 关段有重叠,后续单独任务处理

## Acceptance Criteria

- [ ] 7 个目标文件均按 R1-R7 完成
- [ ] 每个文件改动前后行数记录(供 commit message 引用)
- [ ] R3 中 grep 验证 step 已执行并记录结论
- [ ] 没有断链:所有新增 cross-ref 指向的 heading/section 存在
- [ ] 段落编号连续:BACKLOG 删的章节不留空洞编号;HACKING-wsl 坑编号连贯
- [ ] DEBT.md 头部明确指引"已关闭项不在此文档保留"
- [ ] llm-contract.md 头部指引 3 个拆分文件
- [ ] 单 commit 落库,commit message 格式:`chore(docs): trim 7 over-600-line markdown files (-XXXX lines)`
- [ ] 不修改任何 `.rs` / `.ts` / `.vue` / `.json` 业务代码
- [ ] 不修改其他 10 篇保持现状的 markdown 文件
- [ ] 不修改 `.trellis/tasks/archive/**`

## Definition of Done

- 7 篇目标文件均瘦身到位,行数记录在 commit message
- archive 目录结构合理(`.trellis/spec/archive/` 新建 1 个文件)
- 新文件 4 个:`.trellis/spec/backend/subagent-runs-schema.md` / `latency-tracking.md` / `token-usage-tracking.md` / `permission-layer.md`
- 所有 cross-ref 验证为有效链接
- git status 干净,单 commit 已落
- journal-2.md 末尾追加本任务记录(按项目惯例)

## Out of Scope

- `.trellis/tasks/archive/**` 任何文件
- `docs/CLAUDE.md` / `.trellis/spec/**/index.md` 等 meta 文件
- 业务代码 / 测试 / 配置
- 内容改写(只挪位置/删行,不重写文案)
- llm-contract.md 之外保持现状的 10 篇 markdown 文件
- database-guidelines.md 188-601 行的 3 个 feature pattern (update_message_metadata / edit_user_message / record_message_resend_audit) 保留
- tool-contract.md ⑨ 关段与 permission-layer.md 的重叠 (后续单独任务处理)
- llm-contract.md Future Work (Deferred from Step6) 1381-1392 行 (实施时再判断是否过期)

## Technical Notes

### 新建文件

- `.trellis/spec/backend/subagent-runs-schema.md` (~265 行,从 database-guidelines.md 809-1073 迁出)
- `.trellis/spec/archive/implementation-inception-2026-06-04-to-05.md` (~47 行,从 IMPLEMENTATION.md 735-782 迁出)
- `.trellis/spec/backend/latency-tracking.md` (~824 行,从 llm-contract.md 1393-2217 迁出)
- `.trellis/spec/backend/token-usage-tracking.md` (~574 行,从 llm-contract.md 671-1245 迁出)
- `.trellis/spec/backend/permission-layer.md` (~282 行,从 llm-contract.md 388-670 迁出)

### 实施顺序

1. **pre-flight grep**: 验证 shell spillover 在 tool-contract.md 是否已记裁
2. **新建 archive / subagent-runs-schema / 3 个 llm 子文件**: Write 5 个新文件
3. **修改 llm-contract.md**: 删 3 个 scenario 段 + 头部加 3 个 cross-ref
4. **修改 IMPLEMENTATION.md**: 删 735-782 + 加头部注
5. **修改 database-guidelines.md**: 删 809-1073 + 加末尾 cross-ref
6. **修改 agent-loop-architecture.md**: 仅在 509 行附近加一行 cross-ref
7. **修改 DEBT.md**: 删各优先级段已关闭条目 + 删 FT 段 + 删历史合并追踪段 + 头部加注
8. **修改 BACKLOG.md**: 删 §0.5 + 替换 §1/§2 + strikethrough §5.1
9. **修改 HACKING-wsl.md**: 删 311-338 行(坑 12)
10. **post-flight 验证**: 行数核对 / cross-ref 链接检查 / grep 悬空引用
11. **commit + journal**

### 风险点

- DEBT.md 删 closed 条目时需谨慎:open 条目行号会变,需逐一确认未误删
- BACKLOG §0 全局视角可能引用已删章节的编号,需检查是否需调整 §0 的表述
- HACKING-wsl.md 坑 12 硬删除的前提是 tool-contract.md 已有等价文档,否则丢信息
- IMPLEMENTATION.md 头部注的相对路径 `.trellis/spec/archive/...` 需在 IMPLEMENTATION.md 所在目录(doc-trim-2026-06-6-600-md 的相对位置)正确,实际应是从 `docs/` 到 `.trellis/spec/archive/`

### 关联

- 项目惯例: [trellis-task-finish-commit-pattern] — fix→docs(debt)→archive→journal,DEBT.md 回填 commit hash
- 任务完成后 commit hash 回填到 journal-2.md (按惯例)