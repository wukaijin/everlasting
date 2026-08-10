# design.md — docs 过时审查处置设计

## 1. 范围与判定体系

- 审查范围：**活文档 48 个** = 根目录 5 + docs/ 顶层 26 + docs/IMPLEMENTATION/ 4 + docs/WORKFLOW-INTEGRATION/ 13。
- 排除（33 个历史记录文件，性质为记录不追求新鲜）：`_archive/` `_deprecated/` `_reviews/` `research/` `spikes/`。
- 判定五档：保持 / 更新 / 重写 / 移除 / 归档。**归档** = 移入 `docs/_archive/`（历史保留、git 可查）；**移除** = 删除（本次无）。
- 判定依据四源：① 与当前代码事实对照（Cargo.toml / package.json / src 实测）；② 文档间一致性；③ 文档自带状态标记；④ git 修改历史。

## 2. 判定结果总表（48 个）

### 保持 13 个
| 文件 | 说明 |
|------|------|
| AGENTS.md / STRUCTURE.md / THIRD_PARTY_LICENSES.md | 无信号或内容稳定 |
| docs/ARCHITECTURE.md / DEBUG_DB.md / HACKING-llm.md / HACKING-markdown.md | 活跃维护 |
| docs/HACKING-wsl.md | 活跃；仅头部日期"截至 2026-07-10"可顺手刷新（可选） |
| docs/ROADMAP.md | 活跃；§2 第三档 B9+ 两行重复（:125-126 与 :138）冗余非矛盾，可选清理 |
| docs/REMOTE-ACCESS-ROADMAP.md | P1/P2 已落地、Phase 3 远期编排唯一文档；补"完成状态"头部（可选） |
| docs/BACKLOG.md | §5.3 两条"⏸ 未实施"标签两个月未刷（:160/:168），低优先刷新 |
| docs/IMPLEMENTATION/decisions-2026-08.md | 最新且健康 |
| docs/WORKFLOW-INTEGRATION.md | hub，13 part 列全无重复 |

### 需更新 30 个
| 文件 | 更新内容 |
|------|----------|
| CLAUDE.md | :189 notify 已移除（改 mtime fence）；:11/:13/:190 锚点 `IMPLEMENTATION.md#4-决策日志` → `IMPLEMENTATION/decisions.md` |
| README.md | :23/:118 锚点同上 |
| STRUCTURE.md | :428/:708 "IMPLEMENTATION.md §4" 文案 → decisions.md |
| docs/CONTEXT.md | :4/:150 锚点；:144 "79 个 handler" → 91 |
| docs/DESIGN.md | :192 旧 DB 路径 → `dev.everlasting.app/everlasting.db`；:73 notify；:59 "21 个 builtin" → 24（或删数）；:70 "17 类 AuditKind" → 25；:88 "B10 触发 daemon 化"措辞 → 已落地 |
| docs/TECH.md | :72 "serde+tokio::fs+notify" 残留 → 去 notify（与 :54 自洽） |
| docs/A2-SHELL-CLASSIFICATION.md | 头部"草案/未排期" → 已实施状态（P1+P2 07-04 落地，P3 远期） |
| docs/INTERLEAVED-THINKING-DESIGN.md | 补"已实施(2026-07-23/24)"状态头，正文"现状 vs 目标"叙事调为回顾 |
| docs/README.md | 索引补 A2-SHELL-CLASSIFICATION / DEBUG_DB / INTERLEAVED-THINKING-DESIGN / WORKFLOW-INTEGRATION 系；归档文档从索引移出 |
| docs/WORKFLOW-INTEGRATION/07-review-plugin-vision.md | "愿景,延迟讨论" → review 流已开工（review-viz 07-26、epic 07-28、commands/review.rs 存在） |
| docs/IMPLEMENTATION/decisions.md | :7-9 索引 3 链多 `IMPLEMENTATION/` 前缀 → 去前缀 |
| docs/IMPLEMENTATION/decisions-2026-06.md | ① 尾部 11 条 07-02~08-04 错卷条目移回 07/08 分卷；② 8 处缺 `../` 链接补前缀 |
| docs/IMPLEMENTATION/decisions-2026-07.md | :248 与 :265 同题重复（248 实为 CI 内容）→ 改题或合并；:13 `./DEBUG_DB.md` 缺 `../`；:115 dev-workflow.json 路径已迁 → `resources/builtin-workflow/`；多处 task 路径补 `archive/` 前缀 |
| docs/WORKFLOW-INTEGRATION/ 00,01,02,03,05,06,08,09,10,11,12（11 个 part） | `./DESIGN.md` 等缺一层 `../`；`../.trellis/…` 缺两层 `../../`；12 号文件 :52 死锚 `#问题-2…` |

### 待归档 5 个（→ docs/_archive/，日期前缀命名）
| 文件 | 理由 |
|------|------|
| docs/MANUAL-TEST-P2.md | 一次性验收文档，daemon 化已收官（07-20~23） |
| docs/REMOTE-ACCESS-RESEARCH.md | 调研稿结论已消费（P1+P2 落地），仅 Phase 3 远期参考 |
| docs/WORKFLOW-INTEGRATION-REVIEW.md / -2.md | 评审对象（07-07 版）已被 08-07 拆分版取代，结论已消费 |
| docs/SESSION-FIRST-MESSAGE-INTERFACE.md | 07-01 session 快照，MAX_TURNS=50 过时（现 200）、行号漂移 |

### 重写 / 移除
- 重写：0 个（所有过时均为局部更新，无整体重写需求）
- 移除：0 个（没有无价值文档；归档保留历史）

## 3. 关键处置策略

### 3.1 全局锚点统一（最大系统性问题）
- 规则：凡指 `IMPLEMENTATION.md#4-决策日志` / `IMPLEMENTATION.md §4`（拆分后死锚）→ 统一为 `IMPLEMENTATION/decisions.md`（相对路径按所在文件调整）。
- 涉及文件：CLAUDE.md / README.md / STRUCTURE.md / CONTEXT.md / docs/README.md / ARCHITECTURE.md:10 / BACKLOG.md（需逐一 grep 确认）。
- 已正确指向 decisions.md 的（DESIGN.md:16 / DEBUG_DB.md:200 / WORKFLOW-INTEGRATION parts）只修缺 `../`，不改目标。

### 3.2 错卷条目归位（对只追加日志的编辑）
- decisions-2026-06.md 尾部 11 条（07-02~08-04）移回 decisions-2026-07.md / decisions-2026-08.md 按日期归位，正文原样保留。
- 风险：破坏"只追加"惯例 → 缓解：git 全程可追溯；按月分卷是本项目明确规则，归位恢复规则；归位时在 06 卷尾部留一行"X 条 07/08 月条目已归位至对应分卷（2026-08-10）"备注。

### 3.3 归档约定
- 目标命名：`docs/_archive/<YYYY-MM-DD>-<原文件名小写>`（沿用 `2026-06-04-roadmap-restructure.md` 惯例）。
- 归档后引用修复：docs/README.md 索引移除/改指；REMOTE-ACCESS-ROADMAP.md 对 RESEARCH 的链接改指 `_archive/` 路径；grep 全项目确认无活文档指向旧路径。

### 3.4 更新边界
- 只修"过时事实"：数字、路径、状态头、链接、措辞。**不重写叙事、不改结构**（无重写判定）。
- 每个更新保留 file:line 证据，落 audit report。

## 4. 验收校验

- 链接完整性：脚本扫描范围内全部 markdown 相对链接，目标存在且锚点可解析（无 `#` 死锚）。
- 锚点归零：grep `IMPLEMENTATION.md#4` 在活文档中 0 命中。
- 错卷归零：decisions-2026-06.md 无 07/08 月条目。
- 索引一致：docs/README.md 索引表与 docs/ 实际活文档一一对应。
- audit report 覆盖全部 48 个文档、每行含判定+理由+证据 file:line。
