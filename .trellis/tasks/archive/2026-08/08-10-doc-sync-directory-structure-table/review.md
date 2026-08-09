# Review — 大文件拆分收官表文档订正(directory-structure.md 漂移修复)

> 评审日期:2026-08-10。评审对象:`prd.md`(status=planning,实施前评审)。
> 方法:对 PRD 关键事实(`agent/subagent/loader.rs` 319 / `skill/loader.rs` 649 / `db/subagent_runs_tests.rs` 1219、前端 4 文件行数、总纲 PRD line 61 OOS 条目、追溯提交 `35e631c`/`703ab7d`、`subagent_runs_tests.rs` 在历次拆分 PRD 中的零命中、`skill/loader/frontmatter.rs` 存在性)逐条 `wc -l` / `grep` / `git log` 核验,并交叉比对 08-07 总纲 PRD(批 1 目标表 line 16、OOS line 61)与批 2 拆分提交 `dfcb9ba` 的 commit message。

## 总体评价

PRD 事实基础扎实——**全部行数声明、行号引用、追溯线索均独立核验命中**(9/9 项);范围界定干净(R4 单文件改动 + Out of Scope 零歧义),轻量 PRD-only 设计与任务规模匹配。

**结论:可批准进入实施**,但建议先处理 2 个 P1 再开工——它们不影响核验结论,但影响订正结果的准确性:P1-1 批次归因缺口(收官表"批 2"列也属 skill/loader.rs,PRD 未要求订正),P1-2 偏差 1 叙事与 git 历史不符。P2 为文档级小修,顺手改清。

## ✅ 核验通过(证据确凿)

| 声明 | 核验结果 |
|---|---|
| `agent/subagent/loader.rs` = 319 行 | **精确**(`wc -l` = 319) |
| `skill/loader.rs` = 649 行(收官表"✅ 649"的归属文件) | **精确**(`wc -l` = 649;`skill/loader/` 下确有 `frontmatter.rs` 子模块) |
| `db/subagent_runs_tests.rs` = 1219 行 | **精确**(`wc -l` = 1219) |
| 前端 4 文件 >1200:chat.ts 2156 / streamController.test.ts 2137 / ChatPanel.vue 1495 / SubagentDrawer.test.ts 1249 | **精确**(chat.ts 实际位于 `app/src/stores/chat.ts`,路径全命中) |
| 总纲 PRD line 61 为前端 OOS 条目 | **精确**(`grep -n` 命中 line 61:`ChatPanel.vue`(1,495 行)、`streamController.test.ts`(2,136 行)等 >1200 行前端文件;line 59 为 `chat.ts` 的 OOS 条目) |
| 追溯提交 `35e631c`(group-chat Phase 4)/ `703ab7d`(subagent resume C1)存在 | **精确**(message 与 PRD 描述吻合) |
| `subagent_runs_tests.rs` 从未进入任何拆分任务范围 | **确证**(08-09 四个 tests_* PRD + 08-07 总纲 PRD `grep` 零命中) |
| 偏差 2 定性"范围外漏网,非执行缺陷" | **成立**(不在 08-07 OOS 点名的 tests_agent_loop/tests_subagent 之列,亦非 4 个 08-09 任务目标) |
| PRD 行号引用(line 72 / line 93)与 directory-structure.md 现状一致 | **精确**(line 72 = loader 行,line 93 = 措辞行) |

## ⚠️ 需修正的问题(按严重度排序)

### 🔴 P1-1 — 批次归因缺口:收官表"批 2"也是 skill/loader.rs 的,agent/subagent/loader.rs 实为批 1 拆分

**位置**:`prd.md` §偏差 1 / R1 / AC1。

**问题**:收官表 line 72 `agent/subagent/loader.rs` 2290 | 批 2 | ✅ 649 实为**三列混杂两个文件**:

- **2290** = `agent/subagent/loader.rs` 原行数(总纲 PRD line 16,批 1 目标表)
- **批 2** = `skill/loader.rs` 的拆分批次 —— 批 2 提交 `dfcb9ba` commit message 明言"纠正前提:此前从未拆过(08-07 拆的是 agent/subagent/loader.rs,同名不同模块)"
- **✅ 649** = `skill/loader.rs` 现况

即 `agent/subagent/loader.rs` 的拆分发生在**批 1(08-07, commit `7b60b55`)**,非批 2。PRD 偏差 1 只纠正了 649 指错 loader,未指出批次列同样指错;按 R1/AC 字面实施,订正后仍会残留"agent/subagent/loader.rs → 批 2"的错误归因。

**建议**:R1 补明两文件各自批次与原始行数——`agent/subagent/loader.rs` 2290 → 批 1 → 319(子模块 frontmatter.rs/cache.rs);`skill/loader.rs` 1660 → 批 2 → 649(子模块 frontmatter.rs;总纲正文名单列,首批表格未列,可加注)。

### 🔴 P1-2 — 偏差 1 叙事"拆分后又被进一步精简"与 git 历史不符

**位置**:`prd.md` §偏差 1。

**问题**:`git log --follow` 显示 `7b60b55`(批 1)之后 `agent/subagent/loader.rs` **无后续拆分 commit**(后续均为 workflow/feature 提交)。319 更可能是批 1 拆分的直接结果——2290 行中 frontmatter/cache 簇 + 内嵌测试 1,114 行迁出后余 319 完全自洽,无需"进一步精简"假设。且"frontmatter 簇提到 `skill/loader/frontmatter.rs`"一句易误读为 agent/subagent 域产物(实为批 2 拆 skill/loader.rs 时产出)。

**建议**:表述改为"批 1 拆分后现状 319";如需举例 frontmatter 簇,注明其属 `skill/loader/` 域。

### 🟡 P2-1 — 新增遗留条目插入位置未指明

**位置**:`prd.md` R2。

**问题**:line 93 是"4 个测试任务进度"总结行;`subagent_runs_tests.rs` 条目插在 line 92 与 line 93 之间更合"遗留清单 → 总结行"体例,PRD 未指明位置。

**建议**:Notes 注明插入位置,或由 implementer 按体例自行落位。

### 🟡 P2-2 — 收官表头日期未列入改动

**位置**:`directory-structure.md` line 67 表头"现状(2026-08-09)"。

**问题**:订正的是 08-10 审计数据,表头日期宜同步为 2026-08-10;PRD 未提。属同表内增量,不违反 R5。

**建议**:R4 表述可顺带涵盖(仍为单文件改动)。

### 🟡 P2-3 — jsonl 占位行未清理

**位置**:`implement.jsonl` / `check.jsonl`。

**问题**:两文件 `_example` 占位行仍在。planning 阶段可留,进入 implement 前删除。
