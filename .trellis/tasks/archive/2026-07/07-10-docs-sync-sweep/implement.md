# Implement — 07-10-docs-sync-sweep

> **本轮范围:仅 P0**(Step 1-3)。P1(Step 4-6)+ P2(Step 7-9)推迟到下一轮任务 `07-XX-docs-sync-round-2`,届时 prd/design 引用本文对应段落。
>
> 执行顺序:P0 三份文档一个一个 commit,共 3 个 commit。每步独立可验证、可 revert。**不需要 PKG_CONFIG_PATH**(纯文档改动,无 Rust 编译),但末步做 sanity `cargo check`。

---

## Step 0 — 准备(无 commit)

- [ ] 读 `git log --oneline -50` 确认基线 commit `f08d61e` 是顶部
- [ ] 读 `app/src-tauri/src/config.rs` 找 `LLM_MODEL` 默认值(供 Step 4 用)
- [ ] `ls app/src-tauri/src/agent/workflow/` 确认 6 文件存在;`ls app/src-tauri/src/background_shell/` 确认 2 文件存在;`ls app/src-tauri/src/llm/retry.rs` 确认存在
- [ ] `grep -c "tool" app/src-tauri/src/tools/mod.rs` 或类似方式确认 tool 数 = 21

---

## Step 4-9 — P1 + P2(⏸ 推迟到下一轮)

> **本轮不执行**。下一轮任务 `07-XX-docs-sync-round-2` 启动时,从 Step 4 开始:
> - **Step 4 — P1.a: `CLAUDE.md`** — Architecture 段(workflow / background_shell / wire+retry / tools 21 个 / LLM_MODEL 默认值核对)
> - **Step 5 — P1.b: `docs/ARCHITECTURE.md`** — Tool Registry 21 + Workflow 子节 + merge/discard worker 标注
> - **Step 6 — P1.c: `docs/DESIGN.md`** — §3.1 B8 迁移 + 工具数 21 + workflow 模块描述
> - **Step 7 — P2.a: `docs/TECH.md`** — workflow 工具 + retry 层依赖归属
> - **Step 8 — P2.b: `docs/HACKING-llm.md`** — A5+ retry 策略条目
> - **Step 9 — P2.c: `docs/HACKING-wsl.md`** — 版本注脚 + PKG_CONFIG_PATH + 坑 1~11 校对
>
> Step 10-11(cross-doc consistency review + sanity)合并到 round-2 末步执行,届时把 P0+P1+P2 一起 grep 验证。

---

## Step 1 — P0.a: `docs/IMPLEMENTATION.md §4` 决策日志补 5 条 ADR

- [ ] 定位 `§4 2026-07-07` 条目位置(锚点:行号或小节标题)
- [ ] 在 2026-07-07 **之前**插入 5 条新 ADR(按时间倒序):
  - `2026-07-10` task-json hardening(R1-R5)
  - `2026-07-09` workflow-chip-merge
  - `2026-07-09` workflow-transition-card
  - `2026-07-08` pending-indicator
  - `2026-07-08` workflow 系统总览
- [ ] 每条 ADR 沿用既有格式(`### YYYY-MM-DD — <标题>` + **状态** / **触发** / **决策** / **后果** 四字段)
- [ ] 内容来源:`07-10-workflow-task-json-hardening/prd.md` + `design.md` + `git log --oneline | grep "workflow"` 提取关键 commit 信息
- [ ] 验证:`grep -n "2026-07-0[89]\|2026-07-10" docs/IMPLEMENTATION.md` 命中 ≥ 5 条新 ADR
- [ ] 验证:`grep -c "### 2026-07" docs/IMPLEMENTATION.md` = 旧值 + 5
- [ ] **commit**:`docs(impl): §4 决策日志补 2026-07-08~07-10 五条 ADR (workflow 系统 / pending-indicator / chip-merge / transition-card / task-json hardening)`

## Step 2 — P0.b: `docs/ROADMAP.md` B8 迁移 + §1.2 补全

- [ ] 定位 §1.2 已实施列表锚点
- [ ] 在 §1.2 末尾追加两条:
  - `B8 可编排(DAG workflow) 编排层 — 2026-07-10 完整落地(workflow.json 外置 + builtin dev plugin + plugin skill loader + Step 0.1~3.3)`
  - `pending-indicator 跨 session 待处理交互 UI — 2026-07-08(角标 + 徽章 + toast 三档)`
- [ ] 定位 §2 第四档 B8 条目,**整条删除**
- [ ] 若 §2 第四档删除 B8 后为空,改小节标题为"本档无活跃 item,详见 §1.2 历史已实施"(或保留标题但内容说明)
- [ ] 验证:`grep -n "B8" docs/ROADMAP.md` 只命中 §1.2 新条目(无 §2)
- [ ] 验证:`grep -n "pending-indicator" docs/ROADMAP.md` ≥ 1
- [ ] **commit**:`docs(roadmap): §1.2 补 workflow 系统 + pending-indicator;§2 B8 迁移至已实施`

## Step 3 — P0.c: `STRUCTURE.md` 树补全 + 基线更新

- [ ] 第 3 行基线注释:`基线:2026-06-24 commit 7f2553b` → `基线:2026-07-10 commit f08d61e (workflow 大集成收官)`
- [ ] 后端树补:
  - `app/src-tauri/src/agent/workflow/(mod.rs / builtin.rs / def.rs / inject.rs / state.rs / task.rs)`
  - `app/src-tauri/src/background_shell/(mod.rs / in_memory.rs)`
  - `app/src-tauri/src/commands/task.rs` + `app/src-tauri/src/commands/subagents.rs`
  - `app/src-tauri/src/tools/create_task.rs` + `app/src-tauri/src/tools/request_task_state_transition.rs`
  - `app/src-tauri/src/llm/provider/wire.rs`(注明"高内聚不拆,1109 行")
  - `app/src-tauri/src/llm/retry.rs`(注明 A5+ 网络健壮性)
- [ ] 验证:`grep -n "workflow\|background_shell\|wire.rs\|retry.rs" STRUCTURE.md` 命中新节点
- [ ] 验证:`wc -l STRUCTURE.md` 增量 < 50 行(避免膨胀)
- [ ] **commit**:`docs(structure): 基线 07-10 + 后端树补 workflow / background_shell / commands / tools / llm(wire+retry)`

---

## Step 4 — P1.a: `CLAUDE.md` Architecture 段

- [ ] 后端树补 5 项(同 Step 3 增补):`agent/workflow/` + `background_shell/` + `commands/task.rs` + `commands/subagents.rs` + `llm/retry.rs` + `llm/provider/wire.rs`
- [ ] tools 列表 19 → 21(在 tools 段加一句 "21 个 builtin tool(含 workflow 的 create_task / request_task_state_transition)")
- [ ] `LLM_MODEL` 默认值核对:读 `app/src-tauri/src/config.rs` 的 `from_env`(或 `db/config.rs`),核对实际 default;若与 `GLM-4.7` 不一致,改 CLAUDE.md
- [ ] 验证:`grep -n "workflow\|background_shell\|wire.rs\|retry.rs\|21 个 builtin" CLAUDE.md` 命中新增内容
- [ ] **commit**:`docs(claude): Architecture 补 workflow / background_shell / wire+retry;tools 19→21;核对 LLM_MODEL 默认值`

## Step 5 — P1.b: `docs/ARCHITECTURE.md` Tool Registry + Workflow 子节

- [ ] §1.1 Tool Registry:工具数 19 → 21(同 CLAUDE.md 同步)
- [ ] `merge_worker` / `discard_worker` 标注 `ToolKind::GitMutation` + "L3b PR3+ 已落地"
- [ ] 新增 "Workflow 系统" 子节(位置:§1.1 之后或 §1.3 之前):
  - builtin dev workflow plugin
  - workflow.json 外置 + `load_workflow` / `validate` / `fallback`
  - plugin skill loader(`SkillSource::Plugin`)
  - task 状态机 + breadcrumb 注入 + delegation 模板
  - LLM 交互边界:`create_task` + `request_task_state_transition` 仅 workflow_enabled session 可见
- [ ] 验证:`grep -n "Workflow 系统\|builtin dev workflow\|create_task" docs/ARCHITECTURE.md` 命中新增子节
- [ ] **commit**:`docs(architecture): Tool Registry 21 个 + 新增 Workflow 系统子节;merge/discard worker 标注已落地`

## Step 6 — P1.c: `docs/DESIGN.md §3.1`

- [ ] "未做"段删除 B8 条目
- [ ] "已具备"段(或对应章节)补充:
  - 工具数 19 → 21
  - `agent/workflow/` 模块描述(一句话)
- [ ] 验证:`grep -n "B8" docs/DESIGN.md` = 0
- [ ] 验证:`grep -n "21 个\|workflow" docs/DESIGN.md` 命中新增
- [ ] **commit**:`docs(design): §3.1 B8 迁移至已具备;工具数 21;补 agent/workflow 模块描述`

---

## Step 7 — P2.a: `docs/TECH.md`

- [ ] 在 §1.4 "扩展功能新增依赖"或类似段落补 workflow 工具的依赖归属
- [ ] 指向 `llm/retry.rs`(网络健壮性层)的归属说明
- [ ] 验证:`grep -n "retry\|workflow" docs/TECH.md` 命中新增
- [ ] **commit**:`docs(tech): §1.4 补 workflow 工具与 retry 层依赖归属`

## Step 8 — P2.b: `docs/HACKING-llm.md` A5+ retry 策略

- [ ] 在"差异 N"末段后补 A5+ retry 策略条目:
  - retry_open(连接级重试)
  - Full Jitter 退避
  - 首字节前重试(special-case:响应头未到不重试)
  - 归属:`llm/retry.rs`(2026-07-05 落地)
- [ ] 验证:`grep -n "retry\|Full Jitter" docs/HACKING-llm.md` 命中新增
- [ ] **commit**:`docs(hacking-llm): A5+ retry 策略(retry_open + Full Jitter + 首字节前重试)`

## Step 9 — P2.c: `docs/HACKING-wsl.md`

- [ ] 顶部 WSL 版本号核对:`Linux 6.6.114.1-microsoft-standard-WSL2` 是否仍准,更新或加 "截至 2026-07-10" 注脚
- [ ] PKG_CONFIG_PATH 完整说明:在坑 1 附近补一段("cargo test 撞 gdk-pixbuf / webkit2gtk not found 时,需 PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig")
- [ ] 坑 1~11 扫一遍:对照当前 WSL 环境,过期项更新;准确项不动;无法核对的项标 "TBD 下一轮"
- [ ] 验证:`grep -n "PKG_CONFIG_PATH\|6.6.114.1" docs/HACKING-wsl.md` 命中更新
- [ ] **commit**:`docs(hacking-wsl): 顶部 WSL 版本注脚 + 补 PKG_CONFIG_PATH 完整说明 + 坑 1~11 校对`

---

## Step 10 — Cross-doc consistency review(无 commit,收官)

跑 design.md §5 的 4 项 grep,全过才算收官:

- [ ] `grep -rn "19 个 builtin" CLAUDE.md docs/ARCHITECTURE.md docs/DESIGN.md STRUCTURE.md` = 0(应全改为 21)
- [ ] `grep -n "B8" docs/ROADMAP.md docs/DESIGN.md docs/ARCHITECTURE.md docs/IMPLEMENTATION.md` — DESIGN 应 0 命中,ROADMAP 应只在 §1.2 命中,ARCHITECTURE 应在 Workflow 子节命中,IMPLEMENTATION 应在 §4 命中
- [ ] `grep -n "2026-07-10" docs/IMPLEMENTATION.md docs/ROADMAP.md` — 双向引用编号一致
- [ ] `STRUCTURE.md` 与 `CLAUDE.md` 的目录树段结构一致(`grep "├──" <file>` 提取对比)

若有失败,**回到对应 Step 修补,补一个 `docs(fix): ...` commit**。

---

## Step 11 — Sanity check

- [ ] `cd app/src-tauri && cargo check`(零代码改动,纯 sanity,10s 内完)
- [ ] `git diff --stat HEAD~9..HEAD` 确认 9 个文件被改,无其他文件
- [ ] `git log --oneline HEAD~9..HEAD` 确认 9 个 commit message 风格一致(scope: action)

---

## 验证命令汇总

```bash
# 文档任务零代码改动,无需 PKG_CONFIG_PATH

# Step 0 准备
git log --oneline -1          # 确认 f08d61e 在顶
ls app/src-tauri/src/agent/workflow/
ls app/src-tauri/src/background_shell/
ls app/src-tauri/src/llm/retry.rs
grep -A2 "LLM_MODEL" app/src-tauri/src/config.rs  # 或 db/config.rs

# Step 1-9 各 Step 自带的 grep 命令见各 Step 末尾

# Step 10 cross-doc consistency
grep -rn "19 个 builtin" CLAUDE.md docs/ARCHITECTURE.md docs/DESIGN.md STRUCTURE.md
grep -n "B8" docs/ROADMAP.md docs/DESIGN.md docs/ARCHITECTURE.md docs/IMPLEMENTATION.md
grep -n "2026-07-10" docs/IMPLEMENTATION.md docs/ROADMAP.md
diff <(grep "├──" STRUCTURE.md) <(grep "├──" CLAUDE.md)

# Step 11 sanity
cd app/src-tauri && cargo check
git diff --stat HEAD~9..HEAD
git log --oneline HEAD~9..HEAD
```

---

## Rollback points

- **每个 Step 独立 commit,独立 revert**(`git revert <hash>`)。
- **P0 三 commit 整体回滚**:`git revert HEAD~9..HEAD~6`(具体 range 视实际 commit 数)
- **部分回滚优先级**(若时间紧):
  - 必做:Step 1(IMPLEMENTATION §4 补 5 ADR)+ Step 2(ROADMAP B8 迁移)+ Step 3(STRUCTURE 树补全)
  - 可选:Step 4-6(P1)与 Step 7-9(P2)可拆为第二轮
- **无代码改动,无兼容性风险**:文档回滚零副作用。

---

## 完成标准

- [ ] 9 个文档 commit 全部完成
- [ ] Step 10 cross-doc consistency review 4 项 grep 全过
- [ ] Step 11 sanity cargo check 通过(0 warning,0 error)
- [ ] prd.md 所有 Acceptance Criteria 复选框全部勾选
- [ ] journal 记录 session(`.trellis/workspace/Carlos/journal-1.md` 追加本 session)
- [ ] `task.py finish` 收官