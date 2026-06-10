# V2 路线图草案 + 技术线路愿景收敛

## Goal

把 2026-06-10 重新审视后确认的 V2 路线图分类沉淀到单一 source of truth,同时清理项目中其他文件里散落的、过时的、重复的"路线图 / 愿景 / 步骤"描述,确保未来读者只有一个权威入口。

## What I already know

### V2 分类(已与用户确认,2026-06-10)

**🟢 第一档 — 立刻做(4 项)**:
- A4 Token 用量统计
- B5 Memory(user + project,2 层先做)
- C1 取消机制完整化
- D1 session 重命名 / 标记

**🟡 第二档 — 接着做(6 项)**:
- **A2 + B7 权限系统 + 多模式(合并工作组)**
- B3 /command 命令面板
- C3 Context 压缩 + token 硬卡
- C4 审计日志
- B2 @文件补全
- D2 SQLite FTS5 全局搜索
- D3 session 内消息编辑 / 重发

**🟠 第三档 — 缓做(8 项)**:
- B6 Subagent(harness 学习价值高,依赖 Memory)
- B4 Skill 系统
- B9 生成式 UI(4 primitives)
- C2 循环检测
- C6 大输出截断统一
- B1 图片支持(multimodal)
- A5/A6 错误处理 + README + demo
- A7 RDP 双屏 bug

**🔴 第四档 — 最远远期(3 项,app 主体完善之后)**:
- B8 可编排(DAG workflow)
- B10 飞书 IM(触发 daemon 化)
- B11 云端同步(Cloudflare Workers + D1)

**🗑️ 移除(3 项,不做)**:
- A1 xterm.js 嵌入式终端
- A3 MCP 暴露
- C5 Provider 限流(令牌桶)

### 关键理解纠正(必须在草案里留笔)

- **B6 = subagent**(不是用户切角色):main agent 派出 worker agent,独立 context,完成后 summary 回填(类似 Claude Code 的 Task tool)。harness engineering 学习价值高。
- **B7 = mode 是 A2 权限系统的 UX 层**:前端 mode 切换 → 后端 ⑧a Mode 检查 + ⑨ 权限检查 联动。跟 A2 作为一组工作。

### 现状:愿景 / 路线图散落在哪里(已 inventory,2026-06-10 grep 校准)

| 文件 | 包含的愿景段落 | 处理方式 |
|---|---|---|
| `docs/IMPLEMENTATION.md` | §2 路线图 7 步 + §2.9 Step 8 + §3 待办与下一步 + §4 决策日志 | D3 简化方案 b |
| `docs/DESIGN.md` | §3.1 MVP / §3.2 v1 / §3.3 v2 / §3.4 远期 / §3.5 不做 + §5 Step 8 进度补丁 | D5 重构为项目能力边界 |
| `docs/HANDOFF.md` | §1 项目是什么 + §2 当前进度 + §4 接续 | 重构/删除过时段落 |
| `docs/ARCHITECTURE.md` | §2.4 实施映射表(步骤 N → 关卡) | 过时项移除 |
| `docs/BACKLOG.md` | §0.5 已落地 + §1-§7 候选 + 附录 A 远期 | 删/改顶层优先级标记 |
| `docs/TECH.md` | §1.4 扩展功能新增依赖(随候选功能引入) | 去掉路线图引用 |
| `docs/README.md` | 第 17 行 IMPLEMENTATION 描述 + 第 32 行第 3 步引导 + 第 37 行当前进度引导 | 替换为指向 ROADMAP.md |
| `CLAUDE.md`(项目根) | 第 9 行 Project Overview 段(整段含 MVP/v1/路线图外/Step 8 进度) + 第 138 行索引描述 | **顶层导航**:重写为"指向 ROADMAP" + 当前状态 1-2 句 |
| `README.md`(项目根) | 第 18-19 行当前状态 + 第 43/46 行索引 + 第 51-68 行"路线图(8 步)"完整表格 | **顶层导航**:删完整路线图表;改为指向 ROADMAP |

### 已落地状态(以 `git log` 为准,2026-06-10)

文档里说"待办"但实际已完成:
- Step 8-PR5 STRUCTURE.md(commit `b707e68` 已落地,`0133b89` 已 merge)
- 06-10 provider catalog hot-reload(commit `bb7abe5`)

## Decisions (locked)

### D1 (2026-06-10) — Source of truth = 新建 `docs/ROADMAP.md`

- 单一职责(只讲路线图);易于其他文件链接;符合 OSS 惯例
- IMPLEMENTATION 内部 section 编号变化不影响外部引用
- 未来 V2 → V3 可整体替换或归档,不污染 IMPLEMENTATION 决策历史

### D2 (2026-06-10) — IMPLEMENTATION.md 要简化

用户原话:"IMPLEMENTATION 要简化,不需要过多的细节"。
具体简化范围在 Q2 进一步确认。

## Assumptions (to validate)

- 旧版 7 步路线图保留作 historical reference(归档到 `docs/_archive/2026-06-roadmap-v1/` 或在 ROADMAP.md 内嵌"历史路线图"折叠段)
- 决策日志条目**不动**(历史记录性质,只追加不删除)— 但归宿位置待 Q2 确认
- BACKLOG §1-§7 的"技术细节"段落保留,只删/改顶层引用(优先级 / 排期 / Phase 标记)
- DESIGN §3.1-3.5 大改:MVP/v1/v2/v3+ 4 档分类被 V2 的 4 档替代,产品版语义可能要清理

### D3 (2026-06-10) — IMPLEMENTATION.md 简化方案 = b 中等

- **保留**:§1 自研决策(15 行) + §4 决策日志(135 行,12 条 ADR)
- **移走**:§2 实施路线图 + §3 待办与下一步 → ROADMAP.md
- **结果**:IMPLEMENTATION.md 从 388 行降到 ~150 行,变成纯"决策档案"
- **不引入** `docs/DECISIONS.md` 单独文件(职责由瘦身后的 IMPLEMENTATION.md 承担)
- **依据**:决策日志是 ADR 性质的不可再生历史档案,塞进 ROADMAP 会污染"未来计划"主线视角

### D4 (2026-06-10) — 综合删除/重构策略(用户综合答复)

**ROADMAP.md 内容架构**:不只列未来计划,**也要列已实施项**(标注 ✅),作为"状态视图"。
- 顶部加 maintenance note:"本文档是 living document,随功能完善 / 需求更改及时更新"
- 已实施项粗粒度归类(原 7 步 + 路线图外 + Step 8 + 06-10 fix 等),不必逐 commit 罗列
- 4 档未来计划(V2 完整分类)

**顶层入口文件 加导航链接**:
- `CLAUDE.md`(项目根)
- `README.md`(项目根)
- `docs/README.md`(文档索引)
- 形式:简短导航句 + 链接到 `docs/ROADMAP.md`

**文档内部文件 不留链接,只重构或删除**:
- `docs/IMPLEMENTATION.md` — §2 §3 移走(已 D3 决定)
- `docs/DESIGN.md` — §3 scope 怎么重构待 Q5
- `docs/HANDOFF.md` — §2 当前进度等过时段落重构或删除
- `docs/ARCHITECTURE.md` — §2.4 实施映射表 — 过时项移除
- `docs/BACKLOG.md` — §0.5 已落地 + §1-§7 顶层优先级标记 — 移除(路线图归 ROADMAP)
- `docs/TECH.md` — §1.4 扩展功能段落 — 重构去掉路线图引用

**处理原则**:
- **过时的** → 移除
- **不适合新路线的** → 重构
- **顶层入口** → 加导航
- **维护承诺** → ROADMAP 顶部 banner

### D5 (2026-06-10) — DESIGN.md §3 重构方案 = a (项目能力边界)

- **删**:§3.1 MVP / §3.2 v1 / §3.3 v2 / §3.4 远期(产品版语义,过时)
- **保留并强化**:§3.5 明确不做(改名为"明确不做(硬约束)")
- **新增**:§3.1 项目能力(简略) + 链接到 ROADMAP
- **结果**:DESIGN §3 = "项目是什么 + 不是什么";ROADMAP = "做什么 + 什么时候做";职责互补不重叠
- **依据**:硬约束(Yolo 不默认开 / 不做云端触发 / 不做团队协作)是项目长期硬约束,不是排程相关,不应跟优先级一起放 ROADMAP

### D6 (2026-06-10) — 历史保留范围方案 = a (极简)

- **旧版 7 步路线图**:整段删除(无保留价值;git log 是终极归档)
- **已废弃项**(3b-2 / rig-core / A1 / A3 / C5):不在 ROADMAP 重复列出;IMPLEMENTATION §4 决策日志已记录或会追加
- **路线图外完成项**:ROADMAP "已实施"段粗粒度归类(不逐 commit 罗列)
- **V2 重排决策**:作为新条目追加到 IMPLEMENTATION §4 决策日志,内容含删除/移除项 + 升档/重新归类 + 4 档简表 + 指向 ROADMAP 链接
- **依据**:决策日志已覆盖所有"为什么不做 X";旧 7 步是中间产物;ROADMAP 保持 clean view

## Open Questions

(全部已拍板)

1. ~~草案放哪~~ → D1
2. ~~IMPLEMENTATION.md 简化到什么程度~~ → D3
3. ~~删除策略~~ → D4
4. ~~DESIGN.md §3 scope 怎么处理~~ → D5
5. ~~历史保留范围~~ → D6

## Requirements (evolving)

- 单一 source of truth 文件存在,包含 V2 4 档完整分类 + 移除项说明 + B6/B7 理解纠正
- 其他文件不再重复路线图详情,只允许"一句话 + 链接"形式
- 已废弃项(A1/A3/C5 + 历史项)有显式归档,不在主路线图里造成认知噪音
- IMPLEMENTATION §4 决策日志保持完整(只追加 2026-06-10 V2 重排决策一条)

## Acceptance Criteria (evolving)

- [ ] 草案文件创建,包含 V2 4 档分类 + 移除项 + B6/B7 纠正 + 已落地状态校准
- [ ] `docs/IMPLEMENTATION.md` 路线图段落与草案对齐(重写或链接)
- [ ] `docs/DESIGN.md` §3 scope 段落处理(替换或重构)
- [ ] `docs/HANDOFF.md` §2 当前进度对齐 V2,移除 Step 8 滞后描述
- [ ] `docs/ARCHITECTURE.md` §2.4 实施映射表更新或加注释指向新草案
- [ ] `docs/BACKLOG.md` §0.5 + §1-§7 + 附录 A 顶层引用与草案对齐
- [ ] `docs/TECH.md` §1.4 扩展功能段落与草案对齐
- [ ] `CLAUDE.md` Project Overview 段对齐 V2 实际状态
- [ ] `docs/README.md` 索引更新
- [ ] `README.md`(根)inspect 后视情况处理
- [ ] IMPLEMENTATION §4 决策日志追加"2026-06-10 V2 路线图重排"一条
- [ ] grep "MVP 步骤" / "v1 / v2" 验证无散落引用

## Definition of Done

- 所有 acceptance criteria 勾选
- `grep -rE "步骤 [0-9]|MVP/v1|v1/v2" docs/ CLAUDE.md README.md` 结果仅指向单一 source of truth
- 文档构建 / 链接无 404(手动 spot check)

## Out of Scope

- 改 BACKLOG §1-§7 各 section 的**技术细节**(只动顶层优先级 / 排期 / Phase 标记)
- 改 `docs/_archive/` 已归档文档
- 改 `.trellis/spec/` 任何 spec 文件
- 改 `.trellis/tasks/archive/` 任何已归档任务
- 实施 V2 第一档任何具体功能(本任务只做文档收敛)
- 改 IMPLEMENTATION §4 决策日志已有条目(只追加 1 条新决策)
- 改 `docs/HACKING-*.md`(工程笔记,与路线图无关)
- 改 `docs/spikes/`(验证记录,与路线图无关)

## Technical Notes

### V2 分类完整原始记录(对话出处)

详见上方 "What I already know" 段。

### 涉及文件清单

参考 "现状:愿景 / 路线图散落在哪里" inventory 表。

### Source-of-truth 候选位置分析

| 选项 | 优势 | 劣势 |
|---|---|---|
| 重写 `docs/IMPLEMENTATION.md §2` | 已有读者习惯,索引位置已定 | 路线图 + 决策日志 + 待办全塞一个文件,体积膨胀 |
| 新建 `docs/ROADMAP.md` | 单一职责,易引用,符合 OSS 惯例 | 多一个文件,需更新 README 索引 |
| 拆 `docs/_roadmap/` 子目录 | 可分 active / archive / decisions | 过度设计,文件少时不必要 |

### 删除 vs 链接的取舍

- 完全删除:认知清爽,但读者点过来时缺过渡
- 保留一句话 + 链接:导航友好,但有"小型重复"风险
- 折中:每个文件顶部加 banner "技术路线图统一见 [X.md],本文档只描述本模块职责"
