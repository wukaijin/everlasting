# 大文件拆分收官表文档订正(directory-structure.md 漂移修复)

## Goal

`.trellis/spec/backend/directory-structure.md` 的"收官对照表"(§Large-File Splitting — 收官对照)是 08-07~08-09 一连串拆分任务的总纲收官记录。核对发现三处与代码现状偏移——一处张冠李戴、一处漏网超限、一处措辞过宽。本任务把这三处订正到与现状一致,**纯文档订正,零代码逻辑变更**。

## Background / 已确认事实(审计日期 2026-08-10)

源码侧逐一核对收官表声明 vs `wc -l` 实测,9/10 个核对点精确命中,三处偏差如下:

### 偏差 1:`loader.rs` 三列错配两个文件(收官表 line 72)

收官表 line 72 现文:

> `agent/subagent/loader.rs` 2290 | 批 2 | ✅ 649

这一行**三列分属两个同名不同域的 loader**,被错配到一行。逐列核实(`git show` + `wc -l`):

| 列 | 实际归属 | 证据 |
|---|---|---|
| `agent/subagent/loader.rs` 2290(目标原行数) | ✅ 属 `agent/subagent/loader.rs` | 总纲 PRD line 16 批 1 目标表 |
| 批 2(消化批次) | ❌ 实为 `skill/loader.rs` 的批次 | 批 2 提交 `dfcb9ba` commit message 明言"08-07 拆的是 agent/subagent/loader.rs,同名不同模块";批 1 提交 `7b60b55` 才是 `agent/subagent/loader.rs` 的拆分 |
| ✅ 649(现状) | ❌ 实为 `skill/loader.rs` 现状 | `skill/loader.rs` 现为 649 行;`agent/subagent/loader.rs` 现为 319 行 |

**两个 loader 的完整拆分史**(本次审计核实):

| 文件 | 原行数 | 批次/commit | 拆后即时行数 | 现状 | 子模块产出 |
|---|---|---|---|---|---|
| `agent/subagent/loader.rs` | 2290 | **批 1** `7b60b55` | **319**(`git show 7b60b55:...loader.rs \| wc -l`) | 319(批 1 后无后续拆分 commit) | 扁平:`frontmatter.rs` 240 + `cache.rs` 616 + `tests_loader.rs` 1127(**无 loader/ 子目录**) |
| `skill/loader.rs` | 1660 | **批 2** `dfcb9ba` | 646 | 649 | `loader/frontmatter.rs` 167 + `tests_loader.rs` 856(有 loader/ 子目录) |

关键纠偏:① 319 是**批 1 拆分的直接结果**(`7b60b55` 拆后即时即 319,`git log --follow` 显示此后无任何拆分 commit),**不存在"进一步精简"**;② `skill/loader/frontmatter.rs` 是**批 2 拆 `skill/loader.rs`** 的产物,与 `agent/subagent/loader.rs` 无关,勿混读。

**按 line 72 字面订正只会换对 649 的归属,仍会残留"agent/subagent/loader.rs → 批 2"的批次错配**——需把一行拆为两行(两文件各自批次 + 原行数 + 现状)。

### 偏差 2:`subagent_runs_tests.rs` 漏网超限(收官表 line 93 的反例)

收官表 line 93 声称:

> 全仓源码/测试文件现已全部 <1200 行(除上述遗留)

实际:`db/subagent_runs_tests.rs` **1219 行**,超 19 行。

追溯:`subagent_runs_tests.rs` 经 group-chat Phase 4(`35e631c`)+ subagent resume(`703ab7d`)等多轮迭代长到 1219,**从未出现在任何拆分任务的范围内**(总纲 PRD line 62 Out of Scope 明确只排除 `tests_agent_loop.rs` / `tests_subagent.rs`,没有点名 `subagent_runs_tests.rs`,但实际各批都没碰它)。是总纲点名范围之外的漏网。

注意:这是 **范围外** 漏网,不是任务执行缺陷——08-09 的四个 tests_* 拆分任务(`tests_agent_loop` / `tests_subagent` / `memories_tests` / `sessions_tests`)都精确达标,`subagent_runs_tests` 不在它们的 PRD 范围里。

### 偏差 3:收官表 line 93 措辞过宽

line 93 现文:

> 4 个测试任务进度:agent_loop ✅ / memories ✅ / sessions ✅ / subagent ✅(4/4 完成);全仓源码/测试文件现已全部 <1200 行(除上述遗留)。

"全仓…全部 <1200"措辞过宽,实际有 `subagent_runs_tests.rs` 1219 这条反例(偏差 2),且前端有 4 个 >1200 文件(`chat.ts` 2156 / `streamController.test.ts` 2137 / `ChatPanel.vue` 1495 / `SubagentDrawer.test.ts` 1249)——后四者在总纲 PRD line 61 属 **显式 Out of Scope**,不算缺陷,但"全仓"措辞容易误导读者以为它们也不存在。

## Requirements

- **R1** 订正 line 72:把错配的一行**拆为两行**,分别归属两个同名 loader——
  - `agent/subagent/loader.rs` 2290 → **批 1**(`7b60b55`)→ 现状 **319**(扁平拆:`frontmatter.rs` + `cache.rs`,无 loader/ 子目录)
  - `skill/loader.rs` 1660 → **批 2**(`dfcb9ba`)→ 现状 **649**(子模块 `loader/frontmatter.rs`)
  - 三列(原行数/批次/现状)各自归属正确,不留"agent/subagent/loader.rs → 批 2"的错配。
- **R2** 在收官表登记 `db/subagent_runs_tests.rs` 1219 作为"范围外已知遗留"(类比 line 90-92 已知遗留段的体例),标明它不在四个 tests_* 拆分任务范围内,可选后续拆。**插入位置**:line 90-92"已知遗留"段内(在 chat_loop.rs hub 1376 条目之后、`chat_loop/drive.rs` 条目附近),保持"遗留清单 → line 93 总结行"的体例顺序——不要插到 line 93 总结行之后。
- **R3** 订正 line 93 措辞:从"全仓源码/测试文件现已全部 <1200 行(除上述遗留)"收敛为"**批范围内**源码/测试文件全部 <1200 行",并附范围外遗留清单(`subagent_runs_tests.rs` 1219 + 前端 4 文件)指向,避免"全仓"误导。
- **R4** 零代码逻辑变更:本任务**只**改 `directory-structure.md` 一个文件,不碰任何 `.rs` / `.ts` / `.vue`,不改 ADR、不改其他 spec。表头 line 67"现状(2026-08-09)"顺带同步为"现状(2026-08-10)"(订正的是 08-10 审计数据,同表内增量,不违反 R5)。
- **R5** 保持收官表既有体例(Markdown 表格 + "已知遗留"分段),改动以增量订正为主,不重写整个 §收官对照 段。

## Acceptance Criteria

- [x] line 72 拆为两行:`agent/subagent/loader.rs` 2290→批1(`7b60b55`)→319(扁平拆 frontmatter.rs 240+cache.rs 616);`skill/loader.rs` 1660→批2(`dfcb9ba`)→649(loader/frontmatter.rs 167)。行数与 `wc -l` 实测一致(10/10 命中),批次与 commit 归属正确
- [x] 收官表新增 `db/subagent_runs_tests.rs` 1219 行条目,插在 line 90-92 已知遗留段内(line 93 总结行之前),注明"范围外漏网,可选后续拆"
- [x] line 93 "全仓…全部 <1200"措辞改为"批范围内…全部 <1200",并指向范围外遗留(`subagent_runs_tests.rs` 1219 + 前端 4 文件:chat.ts 2156/streamController.test.ts 2137/ChatPanel.vue 1495/SubagentDrawer.test.ts 1249)
- [x] 改动仅限 `directory-structure.md` 一个文件(`git diff --stat`:1 file,6+ 4-)
- [x] `cargo check` / `vue-tsc` 未跑(纯文档,无代码变更);`wc -l` 复核改动引用的 10 个行数全部与当前代码一致

## Out of Scope

- **拆 `subagent_runs_tests.rs`**(本任务只登记,不拆;真要拆另立任务,沿用 `tests_*` 目录化模式)
- 拆前端 4 个 >1200 文件(总纲已显式 Out of Scope,本任务不重新决策)
- 改 `directory-structure.md` §收官对照 以外的段落
- 改任何 `.rs` / `.ts` / `.vue` / ADR / 其他 spec
- 改总纲 PRD(08-07-large-file-splitting/prd.md,历史归档不改)
- 改 `chat_loop.rs` hub 1376 / `chat_loop/drive.rs` 1653 / `tools.rs` 1648(已是 line 90-91 登记的已知遗留,不动)

## Notes

- 改动量预估:单文件 ~10 行增量(1 行订正 + 1 行新增遗留条目 + 1-2 行措辞改写)
- 参考 `06-24-sync-docs-after-10-splits` 的体例:文档订正任务以"旧行号/旧路径 + 现状标注"格式保留可追溯性
- 任务轻量,PRD-only 即可,无需 design.md / implement.md
