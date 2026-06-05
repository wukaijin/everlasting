# brainstorm: docs 整理 — 归档 3b-1 产物 + 去重 + 合并

## Goal

把 `docs/` 目录从"散乱"状态整理为清晰的三层结构：核心设计（长期维护）/ 环境经验（长期维护）/ 任务归档（一次性）/ 评审快照（只读）。具体动 4 件事：

1. 归档 3b-1 任务产物（4 个文件：PROPOSAL + FOLLOW-UP + 2×REVIEW-3b-1）到 `docs/_archive/2026-06-3b-1/`
2. 迁移 2 个项目级设计评审（REVIEW-glm-5.1 + REVIEW-deepseek-v4-pro）到 `docs/_reviews/`
3. 把 3b-1 沉淀的**通用经验**（FU-1/2/3 + 客户端陷阱 1/2）**摘要吸收**进长期文档，原文归档
4. 修正 `HACKING-wsl.md` 的注释式大标题为标准 Markdown；`HANDOFF.md` 与 `IMPLEMENTATION.md` 走轻合并；`BACKLOG.md` 远期 v3+ 内容移到末尾

## What I already know

* `docs/` 共 15 个 .md，约 4740 行
* 4 个文件属于 3b-1 任务产物：PROPOSAL-project-binding-and-top-tabs.md (554) / FOLLOW-UP.md (102) / REVIEW-deepseek-v4-pro-3b-1.md (241) / REVIEW-glm-5.1-3b-1.md (144) → `docs/_archive/2026-06-3b-1/`
* 2 个文件是项目级设计评审：REVIEW-glm-5.1.md (354) / REVIEW-deepseek-v4-pro.md (326) → `docs/_reviews/`
* `.trellis/tasks/06-05-tabs-ui-3b-1/` 已归档（commit `092ff8c`），但 3b-1 任务产物还留在 `docs/` 根
* `HACKING-llm.md` 有重复段（两个"## 关联文档"、两个"## 客户端陷阱"）
* `HACKING-wsl.md` 用了 `# === 1. xxx ===` 注释式标题（grep 确认）
* `HANDOFF.md` §3 "5 分钟上手" 跟 `IMPLEMENTATION.md` §2 路线图有重复
* `BACKLOG.md` v3+ 段（多角色/IM/云同步）跟 MVP + v1 的优先级差异大
* `FU-1` cwd 简化为 `~/` 是项目决策 / `FU-2` camelCase 是 Tauri 2 习惯 / `FU-3` reka-ui 是 UI 选型
* `客户端陷阱 1/2`（FU-5/6）是 Tauri 2 + Anthropic 协议的硬约束
* `docs/README.md` 是文档索引，需同步更新
* `CLAUDE.md` 项目根文件也提到了部分文档结构（IMPLEMENTATION 路线图 + 7 步），也需检查
* grep 引用排查：`BACKLOG.md` 引用了 `docs/FOLLOW-UP.md` / `HACKING-llm.md` 引用了 `docs/FOLLOW-UP.md` / `HACKING-wsl.md` 引用了 `docs/PROPOSAL-*.md` / `IMPLEMENTATION.md` 引用了 `docs/PROPOSAL-*.md` → 都要修
* 没人引用 `docs/REVIEW-glm-5.1.md` 或 `docs/REVIEW-deepseek-v4-pro.md` → 移动安全

## Assumptions (validated by D1-D6)

* 通用经验"摘要吸收 + 原文归档"——长期文档只放摘要，详细仍归档（D1）
* 归档目录用 `docs/_archive/YYYY-MM-<task>/`，下划线前缀（D2）
* 评审目录用 `docs/_reviews/`，与 `_archive/` 平行的下划线前缀（D6）
* HANDOFF 与 IMPLEMENTATION 走"指向 + 链接"轻合并，IMPLEMENTATION 是单一事实源（D3）
* BACKLOG v3+ 整体移到末尾的"远期"段，前面只留 MVP/v1/v2 候选（D4）
* `_archive/` 和 `_reviews/` 各自带 README 索引（D5/D6）
* 4 个 3b-1 任务产物归 `_archive/2026-06-3b-1/`，2 个项目级设计评审归 `_reviews/`（D6）

## Decision (ADR-lite)

### D1: 3b-1 通用经验走"吸收"路径

**Context**: 3b-1 沉淀的 6 条 follow-up + 3 个 hotfix 经验里，FU-1/2/3 是项目级决策、FU-5/6 是平台/协议硬约束。如果纯归档，3 个月后写新代码时容易重复踩坑。
**Decision**: 采用"摘要进长期文档 + 原文归档"——FU-1/2/3 摘要进 `IMPLEMENTATION.md §4 决策日志`，FU-5/6 摘要进 `HACKING-llm.md` 的"客户端陷阱"段（与现有去重合并）。6 个原文件归档到 `docs/_archive/2026-06-3b-1/`，并在归档目录 README 索引。
**Consequences**: 写代码时第一时间看到经验，但需要写摘要的工作量（约 30 分钟）。未来若经验再增加，归档会成为唯一"完整历史"。

### D2: 归档目录用下划线前缀

**Context**: 归档目录在 `docs/` 内需要明确"非主目录"信号，避免误以为是当期文档。
**Decision**: 采用 `docs/_archive/2026-06-3b-1/`——下划线前缀让目录在 `ls` 排序时**最前**，与"主目录文档"视觉区分明显。
**Consequences**: 未来所有一次性归档任务都用 `_archive/YYYY-MM-<task>/` 格式，归档会自然形成时间序列。

### D3: HANDOFF / IMPLEMENTATION 走轻合并

**Context**: `HANDOFF.md` 的"待办"和"决策摘要"段跟 `IMPLEMENTATION.md` 重叠。两份独立维护容易失同步。
**Decision**: 保留两个文件，但 `HANDOFF.md` 删掉重叠段（§4.2 选下一步、§6 决策摘要），改为指向 `IMPLEMENTATION.md` 的对应章节的链接。`IMPLEMENTATION.md` 是单一事实源。
**Consequences**: HANDOFF 专注于"5 分钟上手"和"接续步骤"的引导，详细待办/决策在 IMPLEMENTATION 查。HANDOFF 行数会减少约 1/3。

### D4: BACKLOG v3+ 移到末尾

**Context**: `BACKLOG.md` 的 7 个候选功能里，v3+ 远期项（多角色/IM 飞书/云同步/生成式 UI）跟 MVP/v1/v2 优先级差异大，混在一起读起来心智负担重。
**Decision**: 把 v3+ 远期段统一移到 `BACKLOG.md` 末尾的 `## 远期（v3+，暂不评估）` 段，前面只保留 MVP/v1/v2 候选。`§0 全局视角`的五层架构图保留。
**Consequences**: BACKLOG 顶部 5 分钟就能看清近期评估范围。v3+ 完整描述仍在文件里，需要时翻到末尾。

### D5: 归档目录写 README

**Context**: `docs/_archive/` 里的历史任务产物没有索引，新人第一次访问不知道这是什么。
**Decision**: 写 `docs/_archive/README.md`，解释归档约定（`_archive/YYYY-MM-<task>/`）+ 列出当前所有归档任务入口（带一句话摘要）。在 `docs/README.md` 主索引中加一行"历史归档见 [docs/_archive/](_archive/README.md)"。
**Consequences**: 归档目录可发现性提高，新人能找到历史决策。维护成本是每次新归档时更新 README 一行。

### D6: 6 个文件拆 4+2：归档 4 个 + 评审目录 2 个

**Context**: 原计划把 6 个 3b-1 任务产物（PROPOSAL/FOLLOW-UP/2×REVIEW-3b-1/2×REVIEW-design）全归档到 `_archive/2026-06-3b-1/`，但发现其中 2 个（`REVIEW-glm-5.1.md` / `REVIEW-deepseek-v4-pro.md`）评审的是**项目整体设计**（DESIGN/ARCHITECTURE/TECH/IMPLEMENTATION/BACKLOG），不是 3b-1 任务。归档到的 archived task prd (`06-05-handoff-claude-md`) 明确写过这 2 个是"外部评审快照，不应改"。grep 排查发现**没有文件引用这 2 个**。
**Decision**:
- 4 个 3b-1 任务产物（PROPOSAL + FOLLOW-UP + 2×REVIEW-3b-1）→ `docs/_archive/2026-06-3b-1/`
- 2 个项目级设计评审（REVIEW-glm-5.1 + REVIEW-deepseek-v4-pro）→ `docs/_reviews/`（下划线前缀与 `_archive/` 平行，`ls` 排序时归到非主目录组）
- `docs/_reviews/README.md` 说明"项目级设计评审快照（外部 LLM 评审，2026-06 一次性事件）"
- `docs/README.md` 加一行指向 `_reviews/README.md`
**Consequences**: `_reviews/` 是"评审类只读快照"的中长期容器，未来如果有新一期的项目级设计评审也放这里。`grep 引用`范围从 6 个文件缩到 4 个归档文件。

## Open Questions

*（无 — 所有偏好问题已回答）*

## Requirements (evolving)

* R1 — 4 个 3b-1 任务产物移出 `docs/` 根目录到 `docs/_archive/2026-06-3b-1/`
* R2 — 2 个项目级设计评审移出 `docs/` 根目录到 `docs/_reviews/`
* R3 — 通用经验不丢失（FU-1/2/3 + FU-5/6 沉淀到对的长期文档）
* R4 — `HACKING-llm.md` 内部去重，删掉重复的"## 关联文档"和"## 客户端陷阱"小节
* R5 — `HACKING-wsl.md` 的注释式大标题改为标准 Markdown `##` 标题
* R6 — `HANDOFF.md` 与 `IMPLEMENTATION.md` 的"待办"部分去重，HANDOFF 不再独立维护一份待办
* R7 — `BACKLOG.md` v3+ 远期段移到末尾，前面只留 MVP/v1/v2
* R8 — `docs/_archive/README.md` 和 `docs/_reviews/README.md` 写明各自约定 + 当前条目
* R9 — `docs/README.md` 索引更新，加入 `_archive/` 和 `_reviews/` 目录
* R10 — 项目根 `CLAUDE.md` 不出现失效引用（grep 排查归档文件的引用）

## Acceptance Criteria (evolving)

* [ ] 4 个 3b-1 文件（PROPOSAL/FOLLOW-UP/2×REVIEW-3b-1）物理上不在 `docs/` 根目录
* [ ] `docs/_archive/2026-06-3b-1/` 包含这 4 个文件原样
* [ ] 2 个项目级设计评审（REVIEW-glm-5.1/REVIEW-deepseek-v4-pro）物理上不在 `docs/` 根目录
* [ ] `docs/_reviews/` 包含这 2 个文件原样
* [ ] `docs/_archive/README.md` 写明归档约定 + 列出当前归档任务入口
* [ ] `docs/_reviews/README.md` 写明评审快照约定 + 列出当前条目
* [ ] `FU-1/2/3` 摘要出现在 `IMPLEMENTATION.md §4 决策日志`（每个 ≤ 5 行）
* [ ] `FU-5/6` 唯一一次出现在 `HACKING-llm.md` 的"客户端陷阱"段（去重完成）
* [ ] `HACKING-wsl.md` 无 `# === xxx ===` 注释式标题
* [ ] `HANDOFF.md §4.2/§6` 改为指向 `IMPLEMENTATION.md` 对应章节的链接
* [ ] `BACKLOG.md` v3+ 段在末尾的"远期"小节，前面只留 MVP/v1/v2 候选
* [ ] `docs/README.md` 索引表格反映新结构 + 加 `_archive/` 和 `_reviews/` 目录
* [ ] 项目根 `CLAUDE.md` 不出现失效引用
* [ ] `BACKLOG.md` / `HACKING-llm.md` / `HACKING-wsl.md` / `IMPLEMENTATION.md` 中对被归档文件的引用都修好

## Definition of Done (team quality bar)

* 改动后 `pnpm build` 不受 docs 整理影响（应该不受影响，但要确认）
* 所有 git 改动用 commit `docs(cleanup): ...` 单 commit
* 提交前在 `docs/README.md` 与 `CLAUDE.md` 跑通"点链接能跳到对的位置"的手工验证
* 不创建新文件除非必要（只移动 + 编辑）

## Out of Scope (explicit)

* 不动 `app/src/` 或 `app/src-tauri/` 任何代码
* 不动 `spikes/` 目录
* 不动 AGENTS.md / .trellis/ 内部文件
* 不重构文档内容（措辞/段落）— 只移动 + 去重 + 标灰
* 不重写 README.md（项目根）— 只检查引用是否失效

## Technical Notes

* 4 个 3b-1 文件归档总行数 ≈ 1041 行，2 个评审迁移总行数 ≈ 680 行
* `HACKING-llm.md` 重复段定位：grep "^## 关联文档\|^## 客户端陷阱" 找位置
* `HACKING-wsl.md` 注释式标题定位：grep "^# ===" 找位置
* `IMPLEMENTATION.md §4 决策日志` 已有 4 条记录，FU-1/2/3 可加为新条目
* `BACKLOG.md` v3+ 内容大致在 §3-7（多角色/Memory/IM/云同步）
* 引用 `docs/FOLLOW-UP.md` 的地方：`BACKLOG.md` 2 处 + `HACKING-llm.md` 2 处
* 引用 `docs/PROPOSAL-project-binding-and-top-tabs.md` 的地方：`HACKING-wsl.md` 1 处 + `IMPLEMENTATION.md` 1 处
* 没人引用 `docs/REVIEW-glm-5.1.md` / `docs/REVIEW-deepseek-v4-pro.md` — 移动安全
* 移动后链接形式：`./FOLLOW-UP.md` → `../_archive/2026-06-3b-1/FOLLOW-UP.md`，`./PROPOSAL-*.md` → `../_archive/2026-06-3b-1/PROPOSAL-*.md`

## Research References

(暂无 — 这次主要是文档搬运与去重，不涉及技术选型研究)
