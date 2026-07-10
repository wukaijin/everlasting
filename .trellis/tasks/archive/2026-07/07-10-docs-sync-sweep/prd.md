# 07-10-docs-sync-sweep — 正式文档同步

> **本轮范围:仅 P0**(3 份高优先级文档,3 个 commit)。P1(CLAUDE/ARCHITECTURE/DESIGN)+ P2(TECH/HACKING-llm/HACKING-wsl)推迟到下一轮任务 `07-XX-docs-sync-round-2`。完整 9 份文档的诊断和 plan 见本文 §"已确认事实" + §Acceptance Criteria 完整版,作为 round-2 的 reference。

## Goal

将项目 9 份正式文档同步至 2026-07-10 代码基线 (`f08d61e`)。当前 9 份文档相对代码有 **4 滞后 + 21 遗漏 + 3 错误**,最大遗漏是 `docs/IMPLEMENTATION.md §4 决策日志` 漏掉 07-08~07-10 全部 ADR,以及 `docs/ROADMAP.md` 把 B8 错归第四档。修复后这 9 份文档应能作为 onboarding / 决策追溯的可靠 reference,且与代码状态一一对应(无事实错误)。

## 背景:为什么需要这次同步

最近约 30 天密集落地了一波功能,文档侧没跟上:

- **V2 第二档(2026-06-21 收尾)** — B4 Skill / B6 Subagent / B12 Checklist / C2 循环检测 / L1a 后台 shell / L2 并发只读 / L3a-d subagent 全套 / RULE-D-001 api_key 加密
- **V2 第三档(2026-07-02 部分)** — B9 生成式 UI
- **V2 2 期 自主记忆(2026-06-29)** — auto_reflect / memory_recall / memory_hygiene / remember tool
- **2026-06-23 大拆分** — `agent/subagent/` 4 文件 + `agent/permissions/` 8 模块 + `db/tests.rs` 6 文件 + 前端 `chat/` 3 组件 1 composable 3 store 模块
- **2026-06-08/09 多 Provider 落地** — `llm/provider/wire.rs`(1109 行)+ `llm/retry.rs`(A5+ 网络健壮性)
- **2026-07-08~07-10 workflow 大集成(本 session 期间)** — workflow.json 外置 / Step 0.1~3.3 / builtin dev workflow plugin / plugin skill loader / breadcrumb 注入 / delegation 模板注入 / archive_task IPC / task_state_transition / 07-09 chip merge / 07-10 task.json hardening (R1-R5)
- **2026-07-08 pending-indicator** — 跨 session 待处理交互 UI 提醒

CLAUDE.md 顶部"当前状态"已经反映 V2 档完成,但其他 8 份文档(尤其 ROADMAP / IMPLEMENTATION / STRUCTURE / ARCHITECTURE / DESIGN)多处滞后,且多数**完全没提到 workflow 系统**。

## 已确认事实(诊断证据)

完整诊断由 Explore sub-agent 给出(报告内容作为本任务的 source of truth),按 9 份文档分类的核心问题:

| 文档 | 类型 | 关键问题 |
|---|---|---|
| `CLAUDE.md` | 遗漏 3 + 错误 1 | Architecture 树缺 `agent/workflow/` + `background_shell/` + `commands/task.rs`;tools 列表 19→21;`llm/` 缺 `retry.rs`;`LLM_MODEL` 默认值需核对 |
| `STRUCTURE.md` | 滞后 2 + 遗漏 4 | 基线注释停在 06-24;缺 `wire.rs` + `background_shell/` + `agent/workflow/` + `commands/task.rs` + `commands/subagents.rs` + 2 新 tool |
| `docs/ROADMAP.md` | 滞后 1 + 遗漏 2 | **B8 错归第四档**但 07-10 已完整落地;§1.2 已实施列表漏 workflow + pending-indicator |
| `docs/ARCHITECTURE.md` | 遗漏 3 + 错误 1 | tool registry 19→21;workflow 模块未提;L3b merge/discard 标注 |
| `docs/DESIGN.md` | 滞后 2 + 遗漏 1 | **B8 仍列"未做"**;工具数 19→21;workflow 模块未提 |
| `docs/TECH.md` | 遗漏 1 | workflow 工具依赖未列 |
| `docs/IMPLEMENTATION.md` | 滞后 1 + 遗漏 5 | **§4 决策日志最新只到 2026-07-07**,07-08~07-10 全部 ADR(workflow 系统 + pending-indicator + chip-merge + transition-card + task-json hardening)均缺失 |
| `docs/HACKING-llm.md` | 遗漏 1 | A5+ `llm/retry.rs` 网络健壮性坑点未记 |
| `docs/HACKING-wsl.md` | 错误 1 + 遗漏 1 | WSL 版本号过时;`cargo test --lib` 撞 pkgconfig 缺 PKG_CONFIG_PATH 完整说明缺失 |

**总计**:滞后 4 条 / 遗漏 21 条 / 错误 3 条 = **28 条修复点**

## Requirements

### 范围(9 份正式文档)

用户已确认范围,只动这 9 份,**不动**:
- `docs/HANDOFF.md`(历来滞后,见 memory `handoff-lags-behind-commits`,单独 follow-up)
- `docs/CONTEXT.md` / `docs/BACKLOG.md` / `docs/DEBUG_DB.md` / `docs/README.md`
- `docs/WORKFLOW-INTEGRATION*.md` / `docs/A2-SHELL-CLASSIFICATION.md`(任务专属文档)
- `docs/_archive/` / `docs/_deprecated/` / `docs/_reviews/` / `docs/research/` / `docs/spikes/`
- `.trellis/spec/` / `.trellis/tasks/`(任务/规格系统内部文档)
- `app/...`(代码)

### 功能需求(按 P0/P1/P2 分组)

**P0(必须修,事实错误或最大遗漏量)**

1. **IMPLEMENTATION.md §4 决策日志补 07-08~07-10 全部 ADR**(5 条):workflow 系统总览 / pending-indicator / workflow-chip-merge / workflow-transition-card / task-json hardening。每条沿用既有 ADR 格式(标题 + 日期 + 状态 + 触发 + 决策 + 后果)。
2. **ROADMAP.md §1.2 + §2**:B8 从第四档移到 §1.2 已实施列表,标注"2026-07-10 完整落地(含 builtin dev plugin)"。补 pending-indicator 条目。补 workflow 系统总览条目。
3. **STRUCTURE.md**:基线注释从 `7f2553b (06-24)` 改为 `f08d61e (07-10)`。后端树补 `agent/workflow/`(6 文件)+ `background_shell/`(2 文件)+ `commands/task.rs` + `commands/subagents.rs` + 2 新 tool + `llm/provider/wire.rs` + `llm/retry.rs`。

**P1(应当修,主要遗漏)**

4. **CLAUDE.md**:Architecture 树补 `agent/workflow/` + `background_shell/` + `commands/task.rs` + `commands/subagents.rs`;tools 列表 19→21;`llm/` 补 `retry.rs`;核对 `LLM_MODEL` 默认值(代码查 `config::from_env`)。
5. **ARCHITECTURE.md**:tool registry 工具数 19→21;新增 "Workflow 系统" 一节(在 §1.1 Tool Registry 后,描述 builtin plugin / workflow.json / breadcrumb 注入);L3b merge/discard worker tool 标注 `ToolKind::GitMutation` 并注明已落地。
6. **DESIGN.md §3.1**:B8 从"未做"段移到"已具备"段;工具数 19→21;补充 `agent/workflow/` 模块描述。

**P2(可修,小修补)**

7. **TECH.md**:补充 workflow 工具的来源说明(指向 `llm/retry.rs` 等新增依赖的归属)。
8. **HACKING-llm.md**:在"差异 N"之后补 A5+ retry 策略(retry_open + Full Jitter + 首字节前重试)条目。
9. **HACKING-wsl.md**:核对 WSL 版本号(当前 6.6.114.1-microsoft-standard-WSL2 是否需更新);补充 PKG_CONFIG_PATH 完整说明(从 CLAUDE.md 引用到 HACKING-wsl 作为专项坑);验证坑 1~11 是否仍适用。

### 非功能需求

- **写作风格与既有段落一致**:不引入新格式,沿用各文档现有的小节标题层级、表格风格、引用符号(中文文档)。
- **零代码改动**:本任务纯文档,不碰 `.rs` / `.vue` / `.ts`。
- **commit 粒度**:每份文档一个 commit,或 P0 一组 / P1 一组 / P2 一组(三 commit)。**不**一次大 commit 改 9 份。
- **可独立验证**:每份文档改完能 diff 看清楚增量;`git diff --stat <file>` 单文件改动行数可控(<200 行/份)。
- **不破坏交叉引用**:ROADMAP ↔ IMPLEMENTATION §4 ↔ ARCHITECTURE ↔ DESIGN 之间的链接和段落锚点必须保持有效。
- **git status 干净**:除 9 份文档外无其他文件被改动。

### 不做(明确排除)

- 不重新组织文档结构(不重排章节顺序)
- 不翻译现有内容
- 不引入新文档(只改 9 份)
- 不动 docs/_archive / _deprecated / _reviews 等历史归档
- 不为每个修复点单独建子任务(单一父任务 + implement.md checklist 即可)

## Acceptance Criteria

> **本轮验收口径**:仅 P0 的 3 份文档 + 跨文档一致性。P1/P2 的验收推迟到 round-2(见 §"下一轮验收")。

### P0 验收(本轮必过)

- [ ] `docs/IMPLEMENTATION.md §4 决策日志` 包含 2026-07-08、2026-07-09、2026-07-10 三组条目,覆盖 workflow 系统总览、pending-indicator、workflow-chip-merge、workflow-transition-card、task-json hardening(R1-R5)
- [ ] `docs/ROADMAP.md §1.2` 已实施列表包含 workflow 系统 + pending-indicator 两条新条目
- [ ] `docs/ROADMAP.md §2` 第四档 B8 条目**已删除**(B8 已迁移到 §1.2)
- [ ] `STRUCTURE.md` 第 3 行基线注释为 `2026-07-10 commit f08d61e` 或更新
- [ ] `STRUCTURE.md` 后端树包含 `agent/workflow/`、`background_shell/`、`commands/task.rs`、`commands/subagents.rs` 共 4 处新子树/文件
- [ ] `STRUCTURE.md` 提到 `llm/provider/wire.rs` + `llm/retry.rs`

### 跨文档一致性(本轮)

- [ ] `git diff --stat` 共 3 个文件被改动(IMPLEMENTATION / ROADMAP / STRUCTURE),无其他文件
- [ ] 3 个文件的总 diff 行数 < 600 行(避免过度膨胀)
- [ ] ROADMAP.md ↔ IMPLEMENTATION.md §4 之间的 ADR 引用编号一致(如 "B8 详见 IMPLEMENTATION §4 2026-07-10")
- [ ] 3 个 commit message 遵循 `docs(<scope>): <action>` 风格

### 下一轮验收(P1 + P2,推迟到 07-XX-docs-sync-round-2)

> 见 prd §Requirements "功能需求" 的 P1 + P2 段。下轮任务启动时复用本文完整 28 条诊断,聚焦剩余 6 份文档 + 跨文档一致性 review。

## Notes

- 这是 docs-sync 性质任务,**不是新功能开发**,优先级跟随文档可靠性。
- `git log --oneline -100` 顶部 30 条 commit 中,workflow 集成占了 ~20 条,是最近一个月最大一波落地;若时间紧,**先做 IMPLEMENTATION §4 + ROADMAP B8 + STRUCTURE 三件**(P0 全部),P1/P2 可拆成第二轮。
- 不为每份文档建独立 Trellis 子任务(单父任务 + implement.md checklist 已足够;9 个 commit 天然隔离)。
- 风险点:
  - **交叉引用断裂**:ROADMAP 改完要回头核 IMPLEMENTATION / ARCHITECTURE 是否仍引用旧段落。
  - **STRUCTURE.md ↔ CLAUDE.md 同步**:两份文档的目录树必须人工对齐(用同一份 grep 结果)。
  - **LLM_MODEL 默认值核对**:`config::from_env` 实际值需代码核对,不能凭 CLAUDE.md 旧值假设。

## 关联任务

- 无直接依赖(纯 docs 同步)
- 间接相关:`07-08-workflow-integration` / `07-09-workflow-chip-merge` / `07-09-workflow-builtin-plugin` / `07-10-workflow-task-json-hardening` —— 本任务同步它们的成果到正式文档
- 后续 follow-up:`docs/HANDOFF.md` 同步(不在本任务范围)