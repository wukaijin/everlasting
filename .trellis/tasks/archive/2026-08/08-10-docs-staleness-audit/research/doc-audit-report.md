# doc-audit-report — docs 活文档过时审查报告

> 任务:`08-10-docs-staleness-audit`(prd + design + implement)
> 审查日期:2026-08-10
> 判定五档:保持 / 更新 / 重写 / 移除 / 归档。归档 = 移入 `docs/_archive/`(历史保留、git 可查);移除 = 删除(本次无)。
> 证据以 design.md 总表为准,动手前逐条 grep/读文件核验;凡与 design.md 不符处以实测为准并记录于「§3 与 design.md 总表的差异」。

## 0. 范围修正

design.md 称「活文档 48 个 = 根目录 5 + docs/ 顶层 26 + docs/IMPLEMENTATION/ 4 + docs/WORKFLOW-INTEGRATION/ 13」。
**实测 docs/ 顶层为 21 个 md 文件**(含 5 个待归档),实际活文档 = 5 + 21 + 4 + 13 = **43 个**。
design.md 总表逐行 13+24+5 = 42,亦与 48 不符(IMPLEMENTATION.md hub 未入表)。
本报告覆盖实际 **43 个**文档。历史目录(_archive/_deprecated/_reviews/research/spikes)不在审查范围。

---

## 1. 判定总表(43 个)

### 1.1 归档 5 个(→ docs/_archive/,日期前缀命名,已 git mv)

| 文件 | 理由 | 证据(处置前) | 归档名 |
|---|---|---|---|
| docs/MANUAL-TEST-P2.md | 一次性验收文档,daemon 化已收官(07-20~23) | 头部"Phase 2 手动测试指南"(P2.1–P2.5 commit e6b7a2f) | `2026-07-23-manual-test-p2.md` |
| docs/REMOTE-ACCESS-RESEARCH.md | 调研稿结论已消费(P1/P2 落地),仅 Phase 3 远期参考 | 头部"调研评估稿(2026-07-20)" | `2026-07-20-remote-access-research.md` |
| docs/WORKFLOW-INTEGRATION-REVIEW.md | 评审对象(07-07 版)已被 08-07 拆分版取代,结论已消费 | 头部"评审对象: docs/WORKFLOW-INTEGRATION.md(2026-07-07 版本)" | `2026-07-07-workflow-integration-review.md`(日期取文档自身) |
| docs/WORKFLOW-INTEGRATION-REVIEW-2.md | 同上(07-08 二轮) | 头部"评审日期:2026-07-08" | `2026-07-08-workflow-integration-review-2.md` |
| docs/SESSION-FIRST-MESSAGE-INTERFACE.md | 07-01 session 快照,MAX_TURNS=50 过时(现 200)、行号漂移 | 头部"首次发起 message 的接口内容 — 实测拼装" | `2026-07-01-session-first-message-interface.md` |

### 1.2 更新 31 行(处置见 §2,evidence file:line 为处置前位置;§1.3 中 9 个"(其余)"行为同一文档的保持延续行)

| 文件 | 更新内容 | 证据 file:line |
|---|---|---|
| CLAUDE.md | 死锚 `IMPLEMENTATION.md#4-决策日志` ×3(:11/:13/:190)→ decisions.md;:189 notify → mtime fence | :11,:13,:189,:190 |
| README.md(根) | 死锚 ×2(:23/:118)→ decisions.md | :23,:118 |
| STRUCTURE.md | :428/:708 "IMPLEMENTATION.md §4" 文案;TOC 10 处死锚(`##1.顶层结构` 等标题无空格导致 slug 不匹配) | :17-29,:428,:708 |
| docs/ARCHITECTURE.md | §4/⑨ 标题含状态后缀致 7 处死锚;:10/:461/:726 IMPLEMENTATION.md §4 链接;:296/:359/:363/:479/:480/:559/:585/:760 指向 BACKLOG 已重构章节的锚点 | :10,:72,:95,:144,:258,:296,:359,:363,:461,:479,:480,:559,:575,:585,:726,:746,:760 |
| docs/BACKLOG.md | §2 标题含"→ 已落地"破坏锚点(2 处引用);:141/:175/:181 `../_archive/` 路径错误;§5.3 两条"⏸ 未实施"标签 2 个月未刷 | :37,:141,:160,:168,:175,:181 |
| docs/CONTEXT.md | :144 "79 个 handler"→91;:4/:150 §4 文案;:53/:115/:130 `../` 路径错误(解析到根目录) | :4,:53,:115,:130,:144,:150 |
| docs/DEBUG_DB.md | :19/:51 `../../app/…` 路径错(应为 `../app/…`);:177/:192/:200 `../IMPLEMENTATION/…`、:199 `../ARCHITECTURE.md`、:201 `../HACKING-llm.md` 路径错(应为 `./…`) | :19,:51,:177,:192,:199,:200,:201 |
| docs/DESIGN.md | :59 21→24 builtin;:70 17→25 AuditKind;:73 notify → mtime fence;:88 B10 措辞;:192 DB 路径 `~/.local/share/everlasting/db.sqlite` → `~/.local/share/dev.everlasting.app/everlasting.db`;:84 `resources/workflows/` 路径;:98/:105/:114/:155 死锚 | :59,:70,:73,:84,:88,:98,:105,:114,:155,:192 |
| docs/HACKING-llm.md | :197/:198 死锚(IMPLEMENTATION §2.1 已拆出);:319/:347 `../_archive/` 路径错 | :197,:198,:319,:347 |
| docs/HACKING-wsl.md | :329/:333 `../_archive/` 路径错(其余保持) | :329,:333 |
| docs/IMPLEMENTATION.md | :28 死锚 `#4-决策日志` → 指向 decisions-2026-07.md | :28 |
| docs/INTERLEAVED-THINKING-DESIGN.md | 补"已实施(2026-07-23/24)"状态头;§0 "现状 vs 目标" → "实施前 vs 实施后" | :1-26(处置前) |
| docs/README.md | 索引补 A2-SHELL-CLASSIFICATION / DEBUG_DB / INTERLEAVED-THINKING-DESIGN / WORKFLOW-INTEGRATION;REMOTE-ACCESS-RESEARCH / MANUAL-TEST-P2 行改指 `_archive/`;:19/:35 §4 文案 | :12-29(处置前) |
| docs/ROADMAP.md | §2 标题含"2026-06-13 收尾更新"后缀破坏 8 处锚点(规一化标题);:54/:65 锚点截断;:61/:63 `../research/` 路径错;:126/:138 B9+ 重复行清理;:90 task 路径补 archive 前缀;:49 notify 残留 | :49,:54,:61,:63,:65,:90,:104,:126,:138 |
| docs/TECH.md | :72 `serde + tokio::fs + notify` 去 notify;:45/:147 死锚;:77 候选库列表去 notify | :45,:72,:77,:147 |
| docs/WORKFLOW-INTEGRATION.md(hub) | 仅 :14 review part 标题与 07 号 part 状态同步(其余保持) | :14 |
| docs/WORKFLOW-INTEGRATION/00-intro.md | `./DESIGN.md`→`../DESIGN.md` 等 4 链 + §4 文案 | :15 |
| docs/WORKFLOW-INTEGRATION/01-doc-purpose.md | `./DESIGN.md`/`./IMPLEMENTATION/decisions.md` → `../…` | :3,:9 |
| docs/WORKFLOW-INTEGRATION/02-background.md | `./ROADMAP.md`×4 → `../…`;`../.trellis/`×2 → `../../…` | :5,:10-13 |
| docs/WORKFLOW-INTEGRATION/03-capability-boundary.md | `../.trellis/` → `../../…` | :25 |
| docs/WORKFLOW-INTEGRATION/05-architecture-engine-vs-plugin.md | `../.trellis/`×2 → `../../…` | :16,:73 |
| docs/WORKFLOW-INTEGRATION/06-default-dev-plugin.md | `../.trellis/`×2 → `../../…` | :84,:159 |
| docs/WORKFLOW-INTEGRATION/07-review-plugin-vision.md | "愿景,延迟讨论" → review 流已开工(review-viz 07-26、epic 07-28、`commands/review.rs` 存在、builtin `resources/builtin-workflow/review/`) | :1-7(处置前) |
| docs/WORKFLOW-INTEGRATION/08-hooks.md | `./DESIGN.md#22-关键约束` → `../DESIGN.md#22-关键约束` | :11 |
| docs/WORKFLOW-INTEGRATION/09-phased-plan.md | `./ROADMAP.md` → `../ROADMAP.md` | :3 |
| docs/WORKFLOW-INTEGRATION/10-key-decisions.md | `./DESIGN.md` + `../.trellis/`×2 → 加层 | :21,:35 |
| docs/WORKFLOW-INTEGRATION/11-risks.md | `../.trellis/` → `../../…` | :22 |
| docs/WORKFLOW-INTEGRATION/12-integration-points.md | `../.trellis/`×3、`./IMPLEMENTATION/decisions.md`、`./ROADMAP.md` 加层;:52 死锚 `#问题-2dev-workflow-有没有-templates-示例-skill-说明` → `#14-待对齐汇总` | :6,:12,:26,:30,:52 |
| docs/IMPLEMENTATION/decisions.md | :7-9 索引 3 链多一层 `IMPLEMENTATION/` 前缀 | :7-9 |
| docs/IMPLEMENTATION/decisions-2026-06.md | 尾部 12 条 07/08 错卷条目归位;9 处链接缺 `../`(:390 亦缺,design 记 8 处);6 处 task 路径补 archive 前缀 | :390,:577,:592,:666,:670,:671,:723,:839,:969;:841-999;:18,:37,:473,:477,:493,:523,:734 |
| docs/IMPLEMENTATION/decisions-2026-07.md | :13 缺 `../`;:248 与 :265 同题重复(248 实为 CI 内容)→ 改题"CI 测试自动化管线";:115 dev-workflow.json 路径 → `resources/builtin-workflow/dev/workflow.json`;14 处 task 路径补 archive 前缀;:23 归档文档引用 | :3,:13,:23,:27,:51,:70,:109,:115,:139,:143,:212,:246,:248,:291,:316,:341,:364 |

### 1.3 保持 16 个

| 文件 | 说明 | 证据 |
|---|---|---|
| AGENTS.md | Trellis 托管入口,无过时信号 | 全文复核 |
| THIRD_PARTY_LICENSES.md | 引用资产全部存在(HarmonyOS Sans SC 子集打包字体等) | :1-10 |
| docs/HACKING-markdown.md | 无过时信号(`javascript:` 链接为代码块内 XSS 示例,非正文链接) | :1-8 |
| docs/HACKING-llm.md | 活跃维护(仅链接修正,见 §1.2) | — |
| docs/REMOTE-ACCESS-ROADMAP.md | P1/P2 已落地状态文档自身已体现;仅对 RESEARCH 的引用改指 _archive + 1 处死锚修正 | :3,:5,:366 |
| docs/WORKFLOW-INTEGRATION/04-vision-two-plugins.md | 无相对链接;愿景记录仍准确(已开工事实在 part 07 头注明) | 全文复核 |
| docs/IMPLEMENTATION/decisions-2026-08.md | 最新且健康;07-29~08-04 群聊条目归位至本卷尾部 | 全文 22 行 |
| STRUCTURE.md(结构部分) | :222 "24 个 builtin" 与实测一致(builtin_tools() 23 + dispatch_subagent 动态追加) | :222 |
| docs/DESIGN.md(其余) | DB 路径/notify/工具数等事实修正后其余保持 | — |
| docs/ROADMAP.md(其余) | B9+ 重复行清理后其余保持 | — |
| docs/BACKLOG.md(其余) | §5.3 标签刷新后其余保持 | — |
| docs/ARCHITECTURE.md(其余) | 锚点修复后其余保持 | — |
| docs/IMPLEMENTATION.md(其余) | Part Index 结构保持 | :36 |
| docs/DEBUG_DB.md(其余) | DB 路径事实已正确(`~/.local/share/dev.everlasting.app/everlasting.db`) | :15-17 |
| docs/A2-SHELL-CLASSIFICATION.md(其余) | P1+P2 已实施头补上后其余为方案回顾 | — |
| docs/INTERLEAVED-THINKING-DESIGN.md(其余) | 状态头补上后其余为方案回顾 | — |

### 1.4 重写 / 移除

- 重写:0 个(所有过时均为局部更新,无整体重写需求)
- 移除:0 个

---

## 2. 处置汇总(按 implement.md 阶段)

### A 阶段
- A1 本报告(43 个逐行判定,证据见上)。
- A2 归档前引用扫描:指向 5 个待归档文档的引用点 = docs/README.md:21/:22(索引行)、REMOTE-ACCESS-ROADMAP.md:3/:366(链接)、decisions-2026-07.md:23(纯文本提及);WORKFLOW-INTEGRATION-REVIEW-2.md:4 引用 REVIEW(自身归档,无需修)。全部已修复。

### B 阶段
- **B1 断链修复**:WI 11 个 part 补 `../`(23 处链接);decisions.md :7-9 去前缀;decisions-2026-06 9 处补 `../`;decisions-2026-07 :13 补 `../`;12-ip :52 死锚改指 §14。
- **B2 锚点统一**:`IMPLEMENTATION.md#4-决策日志` / `IMPLEMENTATION.md §4` 在 CLAUDE/README/STRUCTURE/CONTEXT/docs-README/ARCHITECTURE/IMPLEMENTATION.md 共 15 处 → `IMPLEMENTATION/decisions.md`(或分卷),`grep "IMPLEMENTATION.md#4"` 活文档 0 命中。
- **B3 归档 5 文档**:git mv 至 `docs/_archive/<YYYY-MM-DD>-<原名小写>`;引用同步(docs/README 索引行改指、REMOTE-ACCESS-ROADMAP 2 链、decisions-2026-07:23 文本)。
- **B4 内容更新**:DESIGN 5 处(+2 顺带)、CONTEXT :144、TECH :72(+:77)、CLAUDE :189、A2 状态头、INTERLEAVED 状态头 + §0 叙事、07-review-plugin-vision 已开工(+hub :14 同步)、BACKLOG §5.3 标签刷新、ROADMAP B9+ 重复行清理 + :49 notify。**只修事实,不动叙事结构**。
- **B5 错卷归位**:decisions-2026-06 尾部 12 条(07-03/07-02×2/07-07/07-13/07-14/07-23-24/07-25/07-26×2/07-28 → 07 卷;07-29~08-04 → 08 卷)按日期降序插入;06 卷尾部留归位备注;07 卷 :248 改题(CI)、:115 路径刷新、14 处 task 路径补 archive 前缀;06 卷 6 处 task 路径补 archive 前缀(顺带)。

### C 阶段
- **C1 索引更新**:docs/README.md 补 A2-SHELL-CLASSIFICATION / DEBUG_DB / INTERLEAVED-THINKING-DESIGN / WORKFLOW-INTEGRATION(hub)4 行;REMOTE-ACCESS-RESEARCH / MANUAL-TEST-P2 两行改指 `_archive/` 并标注"已消费";IMPLEMENTATION 行补分卷说明。索引表与 `ls docs/*.md` 一一对应。
- **C2 全量校验**:见 §4。
- **C3 git diff 审查**:已完成(未提交,Phase 3.4 由主会话执行)。

---

## 3. 与 design.md 总表的差异(实测修正)

1. **文档总数**:design"48 个"为误计,实际 43 个(docs/ 顶层 21 非 26;总表 13+30+5=48 与逐行 13+24+5=42 亦不自洽,IMPLEMENTATION.md hub 漏列)。
2. **错卷条目数**:design 记 11 条,实测 **12 条**;按"07-28 及以前→07 卷、07-29~08-04→08 卷"归位:07 卷 11 条 + 08 卷 1 条,备注行记 12 条。
3. **decisions-2026-06 缺 `../` 链接**:design 记 8 处,实测 **9 处**(含 :390 `research/skill-system-survey.md`)。
4. **DEBUG_DB.md:200**:design 称"已正确指向 decisions.md,只修缺 `../`" — 实测为 `../IMPLEMENTATION/decisions.md`,从 docs/ 解析到根目录不存在,**应去掉 `../`**;同文件 :177/:192/:199/:201、:19/:51 共 7 处路径错误一并修复(design 未列)。
5. **design 未列但实测存在的断链(全部已修)**:
   - CONTEXT.md:53/:115/:130 `../` 路径错误;
   - BACKLOG.md:141/:175/:181、HACKING-llm.md:319/:347、HACKING-wsl.md:329/:333 `../_archive/` 路径错误;
   - 死锚:ARCHITECTURE §4 标题状态后缀致 7 处、⑨ 标题后缀致 1 处、:296/:359/:363/:479/:480/:559/:585/:760 指向 BACKLOG 已重构章节(§2 Skill/§3 Memory/§4.1 Role/§4.2 Mode/§5 UI/§6 飞书/§7 云端 → 重定向至 memory spec / permission-layer spec / ROADMAP §1.2 / BACKLOG §4 跨设备 / ROADMAP §2);
   - ROADMAP §2 标题"2026-06-13 收尾更新"后缀致 8 处锚点失效(规一化标题,后缀移入正文首行);ROADMAP:54/:65 锚点截断;ROADMAP:61/:63 `../research/` 路径错误;ROADMAP:206 `#X` 为行内代码模板(检查器跳过代码 span);
   - DESIGN:98/:105/:114/:155;HACKING-llm:197/:198;IMPLEMENTATION.md:28;STRUCTURE TOC 10 处(`##1.顶层结构` 等标题无空格);
   - 修复方式:重定向到现存活章节 / 规一化标题锚点(状态后缀移入正文),未改叙事。
6. **工具数**:DESIGN:59 "21 个 builtin" → 实测 `builtin_tools()` 注册 **23** 个 + `dispatch_subagent` 动态追加(chat_loop.rs:450)= **24** 个 LLM 可见,按 design 写 24(与 CLAUDE/README/STRUCTURE 一致)。
7. **归档日期**:WORKFLOW-INTEGRATION-REVIEW 采用文档自身"评审日期 2026-07-07"(dispatch 标注 07-08;以"日期用文档原始日期"为准);其余 4 个按 dispatch(07-23/07-20/07-08/07-01)。
8. **顺带修复(design 未列,同类事实)**:DESIGN:84、07 卷 :115 `resources/workflows/dev-workflow.json` → `resources/builtin-workflow/dev/workflow.json`(实测文件存在);ROADMAP:90 + decisions-2026-06 6 处 task 路径补 archive 前缀;TECH:77、ROADMAP:49 notify 残留;hub WORKFLOW-INTEGRATION.md:14 标题与 part 07 同步;docs/README IMPLEMENTATION 行补分卷说明。
9. **可选未做**:HACKING-wsl 头部日期("截至 2026-07-10 验证"与当前环境实测一致,内核 6.6.114.1 相同,未刷);REMOTE-ACCESS-ROADMAP 补"完成状态"头部(文档自身已体现 P1/P2 落地,未加)。
10. **保留的历史性语句**:`17 类 AuditKind` 等计数出现在日期锚定的 ADR/交付描述中(ARCHITECTURE:684、ROADMAP:55、decisions-2026-07:434、HACKING-llm:503、A2:77/:122、decisions-2026-06:247/:278 的 "IMPLEMENTATION.md §4" 历史表述),为实施时点的历史事实,保留不动。
11. **trellis-check 复核补修(2026-08-10)**:① design/本报告只列 ROADMAP:90 一处 task 路径,实测活文档共 **22 处**裸 `.trellis/tasks/<mm>-*` 死路径(ROADMAP §1.2 行 18 处、HACKING-markdown:165/:166、ARCHITECTURE:720、REMOTE-ACCESS-ROADMAP:348),已全部补 `archive/2026-0x/` 前缀;② ROADMAP:90 B8 行 `resources/workflows/dev-workflow.json` → `resources/builtin-workflow/dev/workflow.json`(与 DESIGN:84、07 卷 :115 同类,本报告 §3.8 漏列);③ 本报告 §1.2 头部"更新 22 个"与实际 31 行不符,已订正。

---

## 4. 验收校验结果(C2)

| 校验项 | 结果 |
|---|---|
| 链接完整性脚本(`.trellis/tasks/08-10-docs-staleness-audit/scripts/check-links.py`) | **PASS**:38 个活文档扫描,0 失败(目标存在 + 锚点可解析;跳过围栏代码块/行内代码 span 中的链接) |
| `grep -rn "IMPLEMENTATION.md#4"` 活文档 | **0 命中** |
| `grep -nE "^### 2026-0[78]-" docs/IMPLEMENTATION/decisions-2026-06.md` | **0 命中**(错卷归零) |
| docs/README.md 索引 vs `ls docs/*.md` | **一致**:16 个顶层活文档全部有索引行;已归档 5 个从索引移出(其中 2 个改指 `_archive/`) |
| 归档引用 | 无活文档指向旧路径(`grep MANUAL-TEST-P2/REMOTE-ACCESS-RESEARCH/WORKFLOW-INTEGRATION-REVIEW/SESSION-FIRST-MESSAGE-INTERFACE` 仅剩改指 `_archive/` 的 3 处) |
| 错卷归位完整性 | 07 卷 25 条(14 原有 + 11 归位)按日期降序;08 卷尾部追加群聊条目;06 卷尾部留归位备注 |

## 5. 遗留问题(不在本任务范围)

- 历史目录(_archive/_reviews/research/spikes/_deprecated)内容未审查(prd Out of Scope)。
- 各文档正文中的纯文本 §N 引用(BACKLOG 图例中残留 "§6 飞书/§7 云端同步/§5 生成式 UI" 等旧章节号)为历史叙事,未逐一追改(只修链接,不动纯文本章节号)。
- A2-SHELL-CLASSIFICATION.md 正文仍以方案口吻叙述(已加"已实施(P1+P2)"状态头 + §6 措辞修正);若需整体改写为实施记录,属重写范畴,单列报告交用户决策。
