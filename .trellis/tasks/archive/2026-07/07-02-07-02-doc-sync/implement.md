# doc sync 2026-07-02: 执行计划

## 概览

按 `prd.md` 的 R1-R10 顺序执行。每步一个 PR-friendly commit,review gate 在每文件改完后做一次 `grep` / `Read` 自检。

## 顺序(依赖:无;按"先删后改"避免反向引用残留)

### Step 1: 删 `docs/HANDOFF.md`(R1)
```bash
git rm docs/HANDOFF.md
```
- 不立即 commit,先做完所有引用清理(Step 2 末尾再统一 commit,避免中间状态 `grep` 不通)。

### Step 2: 全局反向引用扫描
```bash
grep -rnE 'HANDOFF\.md|\bHANDOFF\b' --include='*.md' docs/ CLAUDE.md STRUCTURE.md .trellis/spec/ 2>/dev/null
```
- 输出 0 命中才算干净(允许 `.trellis/tasks/07-02-doc-sync/` 内部提到过去任务名)。
- 命中处逐个 Edit 替换。

### Step 3: 更新 `CLAUDE.md`(R2)
按 `prd.md R2` 清单逐行 Edit:
- L8 "当前状态(2026-06-13)" → 2026-07-02 视角,改段。
- "V2 路线图第一档收口 + 第二档 2/7 进 §1 已实施,剩余 5 项在第二档" → "V2 第二档 7/7 全部落地(2026-06-17)..." 整句替换。
- "C3 context 压缩(token 硬卡 + MAX_TURNS 50)" → "C3 context 压缩(token 硬卡 + MAX_TURNS 200)"。
- `Architecture` 段 `agent/` 子模块补全。
- `tools/` 段补全 19 个 builtin(对照 `app/src-tauri/src/tools/mod.rs::builtin_tools()`)。
- `app/src/components/chat/` 段补 `<UiCard>` / `<WorkerBranchBadge>` / `<WorkerMergeControls>`。
- 删顶部指向 HANDOFF 的 5 分钟上手表(整表删)。

### Step 4: 更新 `docs/DESIGN.md`(R3)
按 `prd.md R3`:
- §3.1 工具集 → 19 个。
- §3.1 加 B12 / L1 / L2 / L3 / B9 / C2 / RULE-D-001 / V2 2 期 列表。
- §5.1 风险表 rig-core 行删除或调整。
- §5.1 循环检测行改写 C2 分级触发。

### Step 5: 更新 `docs/CONTEXT.md`(R4)
按 `prd.md R4`:
- 改 "Checklist" 段实现状态。
- 新增 9 个术语(Subagent / SubagentRun / Worker Worktree / BackgroundShell / MAX_TURNS / Context Compression Thresholds / Loop Detection / AuditKind / L1-L3 命名约定)。

### Step 6: 更新 `docs/ARCHITECTURE.md`(R5)
按 `prd.md R5`:
- §1.1 Tool Registry 列表 → 19 个。
- §1.1 Resource Loaders 段对齐 B5 + V2 2 期。
- §2 16 关卡对账 `IMPLEMENTATION §4` 已实施日期。

### Step 7: 更新 `docs/TECH.md`(R6)
按 `prd.md R6`:
- §1.1 terminal 行改写。
- §1.4 模糊搜索行加 `fuzzysort` 实际选择。
- §1.4 生成式 UI 三件套加"B9 当前未引入"注。
- §1.4 L1a 依赖说明行加。

### Step 8: 更新 `docs/DEBUG_DB.md`(R7)
按 `prd.md R7`:
- §2 "9 张表" → "10 张表" + 加 `autonomous_memories` 行。
- §3.3 + §5 加 autonomous memory 调试入口。

### Step 9: 更新 `docs/BACKLOG.md`(R8)
按 `prd.md R8`:
- §5.3 加注 `fe91605`。
- §3.3 生成式 UI 行改写。

### Step 10: 更新 `docs/HACKING-llm.md`(R9)
按 `prd.md R9`:
- §现状一句话加 OPENAI Provider。
- 加 "OpenAI Chat Completions 兼容层差异"章节(3 bullet)。
- 加 "`cache_control: ephemeral` 注入"章节。

### Step 11: 验证(AC10)
```bash
test ! -f docs/HANDOFF.md && echo OK
grep -rE 'HANDOFF\.md' --include='*.md' docs/ CLAUDE.md STRUCTURE.md 2>/dev/null | wc -l  # = 0
grep -cE 'MAX_TURNS.*200' CLAUDE.md  # ≥ 1
grep -cE '"read_file"|"write_file"|"edit_file"|"shell"|"grep"|"glob"|"list_dir"|"web_fetch"|"use_skill"|"use_ui"|"update_checklist"|"remember"|"ask_user_question"|"dispatch_subagent"|"merge_worker"|"discard_worker"|"run_background_shell"|"shell_status"|"shell_kill"' docs/DESIGN.md docs/ARCHITECTURE.md  # 19 × 2 = 38(每文件命中)
grep -E 'autonomous_memories' docs/DEBUG_DB.md  # ≥ 1
```
全绿则下一步,否则回 Step 2-10 修。

### Step 12: 一次 commit
```bash
git add docs/ CLAUDE.md
git commit -m "docs: 2026-07-02 doc sync — 删 HANDOFF + 8 份对齐当前实现

- R1: 删 docs/HANDOFF.md(权威走 ROADMAP.md + git log)
- R2: CLAUDE.md 状态刷 2026-07-02;MAX_TURNS=200;第二档 7/7;
      agent/ + tools/ + chat/ 模块路径补全(loop_detection / auto_reflect /
      memory_* / question_store / <UiCard> / <WorkerBranchBadge> /
      <WorkerMergeControls>)
- R3: DESIGN.md §3.1 工具集 8 → 19;§3.1 加 B12/L1-L3/B9/C2/RULE-D-001/V2 2 期;
      §5.1 rig-core 行去,循环检测改 C2 分级触发
- R4: CONTEXT.md Checklist '规划中' → 'B12 已落地';新增 9 个术语
- R5: ARCHITECTURE.md §1.1 Tool Registry → 19;§1.1 Resource Loaders 对齐;
      §2 16 关卡对账 IMPLEMENTATION §4
- R6: TECH.md terminal 改 L1a 无 PTY;fuzzysort 标实际选择;
      B9 三件套加'当前未引入';L1a 依赖说明
- R7: DEBUG_DB.md 9 张表 → 10;加 autonomous_memories 行 + 调试入口
- R8: BACKLOG.md §5.3 加 fe91605 注;§3.3 生成式 UI 行改写
- R9: HACKING-llm.md 加 OPENAI 兼容层 + cache_control 注入
- R10: 全部 HANDOFF.md 反向引用清零

不动 ROADMAP / IMPLEMENTATION / HACKING-wsl/markdown / README /
STRUCTURE / SESSION-FIRST-MESSAGE-INTERFACE(已 fresh 或沉淀型)。"
```

## 验证 Gate

每 Step 完成后做局部检查:
- Step 3 后:`grep -E 'MAX_TURNS.*200' CLAUDE.md` ≥ 1
- Step 4 后:`grep -cE 'tool' docs/DESIGN.md` 增加(粗略)
- Step 5 后:`grep -E 'Subagent|MAX_TURNS|BackgroundShell' docs/CONTEXT.md` ≥ 3
- Step 7 后:`grep -E 'fuzzysort|portable-pty' docs/TECH.md` ≥ 2
- Step 8 后:`grep -E 'autonomous_memories' docs/DEBUG_DB.md` ≥ 1
- Step 10 后:`grep -E 'OPENAI|cache_control' docs/HACKING-llm.md` ≥ 2

总验证(Step 11)全绿后 Step 12 commit。

## Review Gate

Step 11 验证通过后,Step 12 commit 前用 `git diff --stat` 列出所有改动文件,核对 9 文件清单:
- CLAUDE.md(改)
- docs/DESIGN.md(改)
- docs/CONTEXT.md(改)
- docs/ARCHITECTURE.md(改)
- docs/TECH.md(改)
- docs/DEBUG_DB.md(改)
- docs/BACKLOG.md(改)
- docs/HACKING-llm.md(改)
- docs/HANDOFF.md(删)

## Rollback

`git revert HEAD` 回滚(单 commit),无副作用。

## 不动文件(Out of Scope,显式声明)

- `docs/ROADMAP.md`(Jul 2 刚更新)
- `docs/IMPLEMENTATION.md`(Jul 2 刚更新)
- `docs/HACKING-wsl.md`(沉淀型)
- `docs/HACKING-markdown.md`(沉淀型)
- `docs/SESSION-FIRST-MESSAGE-INTERFACE.md`(Jul 1 新)
- `docs/README.md`(索引型,本次不动)
- `STRUCTURE.md`(CLAUDE.md 引用层,本次不动除非反向引用必然导致断链;实际反向引用主要是 HANDOFF)
- `docs/_archive/` / `docs/_deprecated/` / `docs/spikes/`
- `.trellis/spec/`(spec 是另一套活)