# Design — 07-10-docs-sync-sweep

> **本轮范围:仅 P0 三份文档**(3 个 commit)。P1 + P2(共 6 份文档 6 commit)推迟到下一轮任务 `07-XX-docs-sync-round-2`,届时本文 §3 + §4 复用。
>
> 本轮 3 commit 顺序:IMPLEMENTATION §4 ADR → ROADMAP B8 迁移 → STRUCTURE 树补全,按依赖顺序串行,每份独立 commit,可独立 revert。

## 2. P0 — 三份高优先级文档

### 2.1 `docs/IMPLEMENTATION.md §4` 决策日志补 07-08~07-10 ADR

**改动位置**:§4 顶部(2026-07-07 之前)插入 3 组日期的 ADR。

**格式沿用既有 ADR**(参照 `IMPLEMENTATION.md §4 2026-07-07` 条目):
```markdown
### 2026-07-XX — <标题>

**状态**:Accepted / Superseded(by ...) / Deprecated

**触发**:<1-2 句问题陈述>

**决策**:<做了什么>

**后果**:<影响范围 / 取舍 / 后续 follow-up>
```

**5 条 ADR 草拟**:

| 日期 | 标题 | 状态 |
|---|---|---|
| 2026-07-08 | workflow 系统总览(workflow.json 外置 + builtin dev plugin + Step 0.1~3.3) | Accepted |
| 2026-07-08 | pending-indicator(跨 session 待处理交互 UI 三档提醒) | Accepted |
| 2026-07-09 | workflow-chip-merge(WorkflowToggle + PluginSelect 合并为单 chip + popover) | Accepted |
| 2026-07-09 | workflow-transition-card(前端 task_state_transition 交互卡片) | Accepted |
| 2026-07-10 | task.json hardening(R1-R5:read_task lenient + create_task tool + 即时 resolve + 软推荐 hint) | Accepted |

**关键细节**(从 `git log --oneline` + `07-10-workflow-task-json-hardening/prd.md` 提取):
- workflow 系统触发了"workflow.json 外置 + 内置 dev plugin 解决开箱即用"两个关键决策
- pending-indicator 触发是"跨 session 消息携带 pending 标记时,UI 三档提醒(角标/徽章/toast)"
- chip-merge 触发是"两个 toggle + 一个 select 在标题栏并列占用过多空间,合并为单 chip + popover 节省视觉"
- transition-card 触发是"task_state_transition 在 IPC 层无前端反馈,加前端卡片让用户看到"
- task-json hardening 触发了 5 个 sub-decision(R1-R5),详见 `07-10-workflow-task-json-hardening/design.md`

**回归点**:不删除任何已有 ADR(包括 2026-07-07 之前的);新 ADR 插在 2026-07-07 之前,按时间倒序保持。

### 2.2 `docs/ROADMAP.md` B8 迁移 + §1.2 补全

**两处改动**:

1. **§1.2 已实施列表**(参照既有条目格式)新增:
   - `B8 可编排(DAG workflow) 编排层 — 2026-07-10 完整落地:workflow.json 外置 + builtin dev workflow plugin + plugin skill loader + Step 0.1~3.3(task 状态机 / breadcrumb 注入 / delegation 模板 / archive IPC / plugin agents/ 落点)`
   - `pending-indicator 跨 session 待处理交互 UI — 2026-07-08 落地(角标 + 徽章 + toast 三档)`

2. **§2 第四档**:B8 条目**整条删除**(若 §2 第四档只剩 B8 一条,整节可折叠为"无 active item"或保留标题但内容空)。

**风险**:删除 B8 后,§2 第四档可能只剩其他项目,需检查是否还有其他"最远远期"项;若有,§2 保留;若无,§2 改为"本档无活跃 item,见 §1.2 历史已实施"。

### 2.3 `STRUCTURE.md` 树补全 + 基线更新

**两处改动**:

1. **基线注释**(第 3 行):`基线:2026-06-24 commit 7f2553b` → `基线:2026-07-10 commit f08d61e (workflow 大集成收官)`

2. **后端树**(§3 节点)新增/补充:
   - `app/src-tauri/src/agent/workflow/` 子树:`mod.rs / builtin.rs / def.rs / inject.rs / state.rs / task.rs`
   - `app/src-tauri/src/background_shell/` 子树:`mod.rs / in_memory.rs`
   - `app/src-tauri/src/commands/` 补充:`task.rs` + `subagents.rs`
   - `app/src-tauri/src/tools/` 补充:`create_task.rs` + `request_task_state_transition.rs`(注:`request_task_state_transition` 已在 STRUCTURE 内但若缺则补)
   - `app/src-tauri/src/llm/provider/wire.rs`(06-08/09 落地,1109 行)
   - `app/src-tauri/src/llm/retry.rs`(A5+ 网络健壮性,07-05 落地)

**前端树**(§4 节点)无需改(本任务期间前端无新拆分,仅 workflow toggle/chip UI 改动小,不在目录树级别)。

## 3. P1 — 三份中等优先级文档

### 3.1 `CLAUDE.md` Architecture 段

**改动**:同步 STRUCTURE.md 的 5 项增补(workflow / background_shell / commands/task / commands/subagents / llm retry + wire);tools 列表 19→21;`llm/` 树补 `retry.rs`。

**核对 `LLM_MODEL` 默认值**:必须读 `app/src-tauri/src/config.rs`(或 `db/config.rs`)的 `from_env` 实际逻辑,以代码为准。若代码 default = `GLM-4.7`,CLAUDE.md 不改;若代码 default 改了,CLAUDE.md 同步。

### 3.2 `docs/ARCHITECTURE.md` Tool Registry + Workflow 子节

**两处改动**:

1. **§1.1 Tool Registry**:工具数 19→21(同 CLAUDE.md),`merge_worker` / `discard_worker` 标注 `ToolKind::GitMutation` + "L3b PR3 已落地"。

2. **新增 "Workflow 系统" 子节**(位置在 §1.1 Tool Registry 之后或 §1.3 之前):
   - builtin dev workflow plugin(开箱即用)
   - workflow.json 外置(`load_workflow` / `validate` / `fallback`)
   - plugin skill loader(`SkillSource::Plugin`)
   - task 状态机 + breadcrumb 注入 + delegation 模板
   - 与 LLM 的交互边界(`create_task` + `request_task_state_transition` tools 仅 workflow_enabled session 可见)

**风格**:沿用 ARCHITECTURE.md 既有的 §1.X 子节标题风格,不加新层级。

### 3.3 `docs/DESIGN.md §3.1`

**三处改动**:

1. **"未做"段**:删除 B8 条目。
2. **"已具备"段**(或对应章节)补充:`agent/workflow/` 模块一句话描述 + 工具数 19→21。
3. 若 §3.1 的工具清单按编号列示,补 21 号(`create_task`)和 22 号(`request_task_state_transition`)?需要核对实际编号 —— **DESIGN.md 通常不给工具编号**,而是按类别分;按既有风格分类补充即可。

## 4. P2 — 三份低优先级文档

### 4.1 `docs/TECH.md`

**改动**:在 §1.4 "扩展功能新增依赖"或类似段落补 workflow 工具相关的依赖归属(若 TECH.md 已列 `reqwest` / `tokio` / `serde` 等,无需新增 crate;若列了工具函数归属,补充 `llm/retry.rs` 是网络健壮性层)。

### 4.2 `docs/HACKING-llm.md`

**改动**:在"差异 N"段落(末段)后补 A5+ retry 策略条目:
- retry_open:连接级重试
- Full Jitter 退避
- 首字节前重试(special-case:响应头未到不重试,避免半 stream)
- 归属模块:`llm/retry.rs`(2026-07-05 落地)

### 4.3 `docs/HACKING-wsl.md`

**三处改动**:

1. **顶部 WSL 版本号**:核对当前 WSL 版本(`Linux 6.6.114.1-microsoft-standard-WSL2` 是否仍准),更新或加 "截至 2026-07-10" 注脚。

2. **PKG_CONFIG_PATH 完整说明**:从 CLAUDE.md §Common Commands 引用,在 HACKING-wsl 坑 1 附近补一段:"`cargo test` 撞 gdk-pixbuf / webkit2gtk not found 时,需 PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"。

3. **坑 1~11 验证**:扫一遍现有 11 条坑,对照当前 WSL 环境,若有过期(如 Rust 版本、linuxbrew 路径),更新;若仍准确,不动。

## 5. 协调机制(跨文档)

**改完所有文档后做一次"cross-doc consistency review"**:

```bash
# 1. 工具数 grep(应一致 = 21)
grep -rn "19 个 builtin" CLAUDE.md docs/ARCHITECTURE.md docs/DESIGN.md STRUCTURE.md
grep -rn "tool" CLAUDE.md docs/ARCHITECTURE.md docs/DESIGN.md STRUCTURE.md | grep -E "[0-9]+\s*(builtin|tool)"

# 2. B8 出现位置(应只剩 ROADMAP §1.2 + IMPLEMENTATION §4 + ARCHITECTURE workflow 子节,不在 DESIGN §3.1 "未做"段)
grep -n "B8" docs/ROADMAP.md docs/DESIGN.md docs/ARCHITECTURE.md docs/IMPLEMENTATION.md

# 3. ADR 引用编号(若 ROADMAP 说"B8 详见 IMPLEMENTATION §4 2026-07-10",必须在 IMPLEMENTATION §4 找到)
grep -n "2026-07-10" docs/IMPLEMENTATION.md docs/ROADMAP.md

# 4. STRUCTURE.md ↔ CLAUDE.md 目录树结构一致性
diff <(grep "├──" STRUCTURE.md) <(grep "├──" CLAUDE.md)  # 仅看同名段
```

**review gate**:4 项 grep 全过才算 P0+P1 收官。失败则补漏。

## 6. 兼容性 / 回滚

- **每 commit 独立 revert**:`git revert <hash>` 不影响其他 8 个 commit。
- **P0 整体回滚**:`git revert <P0 commit1>..<P0 commit3>` —— 文档回滚零风险(代码不动)。
- **P0 内部回滚优先级**:若只做部分,**至少要做 §2.1 IMPLEMENTATION §4 补 5 条 ADR**(最大遗漏量)+ §2.2 ROADMAP B8 迁移(事实错误)。
- **不引入交叉引用循环**:本任务不创建新的"x 详见 y"链接,只在需要时引用既有链接。

## 7. 测试 / 验证策略

文档任务的"测试"是**人工 review + grep 一致性**:

- **每文档 commit 前**:自检该文档内提及的工具数 / 模块名 / 路径与代码一致(`grep` + `ls`)。
- **全部 commit 后**:§5 cross-doc consistency review 的 4 项 grep 全过。
- **不跑 `cargo test` / `pnpm test`**:纯文档改动不影响代码,但为保险可在最后跑一次 `cargo check`(零代码改动,只是 sanity)。

## 8. 已知风险

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| STRUCTURE.md ↔ CLAUDE.md 目录树描述不一致 | 中 | 文档可信度 | §5 cross-doc consistency review 的 grep #4 |
| ROADMAP §1.2 B8 迁移后,§2 第四档为空 | 低 | 段落失衡 | §2 改 "无活跃 item" 提示 |
| IMPLEMENTATION §4 ADR 格式与既有不一致 | 低 | 风格分裂 | 参照 `2026-07-07` 条目模板 |
| LLM_MODEL 默认值实际与 CLAUDE.md 不同 | 中 | 事实错误 | 先 grep `config.rs` 再改 CLAUDE.md |
| HACKING-wsl 坑 1~11 大面积过期 | 中 | 文档不可用 | P2 阶段允许只补 PKG_CONFIG_PATH,坑 1~11 标 "TBD 下一轮" |
| 交叉引用断裂 | 低 | 阅读中断 | §5 grep #3 验证 |