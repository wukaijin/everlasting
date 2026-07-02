# doc sync 2026-07-02: 删除 HANDOFF.md + 8 份文档对齐当前实现

## Goal

把 `docs/` 下 9 处过期内容对齐到 2026-07-02 当前实现状态,删除被 `git log` + `ROADMAP.md` 替代的 `HANDOFF.md`,保证 onboarding 路径(`CLAUDE.md → ROADMAP → DESIGN/ARCHITECTURE/TECH`)不丢关键实现。

## Background / Motivation

2026-07-02 audit(本任务前置 `git log` 校准 + 文档对照)发现:
- `HANDOFF.md` Jun 25 后再没更新,漏 ~30 个落地项(B12 / L1 / L2 / L3a-d / C2 / RULE-D-001 / V2 2 期自主记忆 / B9)。其"下一步候选"段已被 `ROADMAP.md §1.2` 完全替代。
- `CLAUDE.md` "当前状态(2026-06-13)" 段已过期 19 天,V2 第二档 7/7 已落地、第二档 2/7 字样过时、`MAX_TURNS=50` 已改为 200、`agent/` 模块路径未含 `loop_detection` / `auto_reflect` / `memory_*` / `question_store` 等 06-23 之后新增文件。
- `DESIGN.md §3.1` "已具备"工具集只列 8 个,实际 19 个 builtin tools;§5.1 风险表"Rig 0.x breaking change" 已不适用。
- `CONTEXT.md` 仍把 Checklist 标"规划中",实际 B12 2026-06-19 落地;术语表缺 Subagent / BackgroundShell / worker worktree / L1-L3 命名 / 当前 MAX_TURNS 等。
- `ARCHITECTURE.md` §1.1 Tool Registry 工具列表过期,§2 16 关卡跟 `IMPLEMENTATION §4` 多处落地记录对账滞后。
- `TECH.md` §1.1 terminal 字段标 `xterm.js + portable-pty`(L1a 不带 PTY);§1.4 模糊搜索字段未标实际选择 `fuzzysort`;§1.4 生成式 UI 三件套(ECharts/vue-table/vee-validate) 跟 B9 当前范围(selector/code_block/diff) 不一致。
- `DEBUG_DB.md §2` "9 张表" 实际 10 张(`autonomous_memories` 已加)。
- `BACKLOG.md §5.3` / `§3.3 B9 行` 有小过期点。
- `HACKING-llm.md` Jun 15 之后再没动,只覆盖单 provider(GLM-4.7),缺 OpenAI Provider / `stream_options.include_usage` / `cache_control: ephemeral` / `cached_tokens` 归一化。

## Requirements

### R1 — 删除 `docs/HANDOFF.md`

- 删除文件本体。
- 移除指向 `HANDOFF.md` 的反向引用:`STRUCTURE.md`(若有)、`CLAUDE.md` 读顺序表、`DESIGN.md`/`ARCHITECTURE.md`/`TECH.md`/`ROADMAP.md`/`IMPLEMENTATION.md`/`BACKLOG.md`/`CONTEXT.md`/`DEBUG_DB.md`/`HACKING-*.md`/`README.md` 头部的"关联文档"列表。
- 移除 `CLAUDE.md` 里 "5 分钟上手" 表 + 头部"最近 commit hash 用 `git log -1 --oneline` 查" 类提示(已无 HANDOFF 文件)。

### R2 — 更新 `CLAUDE.md`(项目根)

- "当前状态(2026-06-13)" 段 → 2026-07-02 视角:列出本批已落地(简略指向 `ROADMAP §1.2`)。
- "V2 路线图第一档收口 + 第二档 2/7 进 §1 已实施,剩余 5 项在第二档" → 改成"V2 第二档 7/7 全部落地(2026-06-17),第三档多个子项已落地(B4/B6/B9 部分/B12/C2/L1-L3/RULE-D-001/V2 2 期);详见 `ROADMAP §1.2`"。
- `C3 context 压缩(token 硬卡 + MAX_TURNS 50)` → `MAX_TURNS = 200`(`agent/mod.rs:76`)。
- `Architecture` 段:
  - `agent/chat.rs / chat_loop.rs (06-23 抽 run_subagent 后)主循环 ~2064 行` → 补全 `agent/` 实际模块清单(`chat.rs` / `chat_loop.rs` / `context.rs` / `loop_detection.rs` / `system_prompt.rs` / `behavior_prompt.rs` / `thinking.rs` / `auto_reflect.rs` / `memory_recall.rs` / `memory_hygiene.rs` / `question_store.rs` / `helpers.rs` / `provider.rs` / `at_file.rs` / `subagent/` / `permissions/`),`loop_detection.rs` 是 C2 06-24 落地。
  - `tools/` 段补全 19 个 builtin tools(对照 `tools/mod.rs::builtin_tools()`)。
  - `app/src/components/chat/` 补全 B9 新增(`<UiCard>` / `<WorkerBranchBadge>` / `<WorkerMergeControls>`)。
- 删除指向 `HANDOFF.md` 的引用。
- 头部状态行改为"详见 `docs/ROADMAP.md`(单一 source of truth)"。

### R3 — 更新 `docs/DESIGN.md`

- §3.1 "已具备"工具集:从 8 个 → 19 个 builtin tools(`read_file` / `write_file` / `edit_file` / `shell` / `run_background_shell` / `shell_status` / `shell_kill` / `grep` / `glob` / `list_dir` / `web_fetch` / `use_skill` / `use_ui` / `update_checklist` / `remember` / `ask_user_question` / `dispatch_subagent` / `merge_worker` / `discard_worker`)。
- §3.1 列出 B12 / L1 / L2 / L3 / B9 / C2 / RULE-D-001 / V2 2 期(简略,详细指向 `ROADMAP §1.2`)。
- §5.1 风险表:"Rig 0.x breaking change" → 删或改为"已无 rig 风险(2026-06-09 弃用)";循环检测"跟踪相同 tool call N 次自动打断" → 改为 C2 分级触发(L1 精确签名 N=3 + L2 Jaccard N=5/0.85,见 `IMPLEMENTATION §4 2026-06-24`)。
- §3.2 "明确不做" 段加"V2 重排后新增"小节:与现状对账。
- 删 `HANDOFF.md` 引用。
- 顶部状态日期不需硬编码(原 doc 也无状态行,无需改)。

### R4 — 更新 `docs/CONTEXT.md`(glossary)

- "Checklist (agent 自跟踪清单) 实现状态:规划中术语,B12 落地后才有完整 schema" → "B12 2026-06-19 落地,TodoWrite 式 `update_checklist` tool(全量替换 + 三态 pending/in_progress/done + 至多一 in_progress coerce),loop-local Vec(per-request,不入持久化)"
- 新增术语:
  - **Subagent / dispatch_subagent**:父 session 派 worker agent,独立 context + token 预算 + worker worktree(B6 / L3)。
  - **SubagentRun**:`subagent_runs` 表一行,`status` ∈ {running / completed / cancelled / error / incomplete};`transcript_json` 持久化整段 transcript + `transcript_truncated` 哨兵。
  - **Worker Worktree**:B6 / L3b 隔离 — `branch` 前缀 `worker/<run_id>`、`git worktree lock` 跑期间、`merge_worker` / `discard_worker` LLM-driven 收口。
  - **BackgroundShell (L1a)**:`run_background_shell` 启动后台 shell(tokio Child,无 PTY),`shell_status` 拉 exit_code,`shell_kill` 终止,默认 `max_runtime_ms` 24h,session-scoped。
  - **MAX_TURNS**:当前 `200`(`agent/mod.rs:76`)。
  - **Context Compression Thresholds**:触发 `context_window * 0.80`,降到 `0.50`,B5 memory 永远保护(C3)。
  - **Loop Detection (C2)**:分级触发(L1 精确签名 N=3 + L2 Jaccard N=5/0.85),命中软提示注入 `ContentBlock::Text` 不打断 loop;`MAX_TURNS=200` 仍是硬兜底。
  - **AuditKind**:`session_audit_events.kind` 字符串,共 10 类(`tool_executed` 等,详见 `ARCHITECTURE §2.5.8`)。
  - **L1 / L2 / L3 命名约定**:L1=后台 shell + 通知;L2=单 turn 多 tool 并发;L3=subagent 三层(L3a 并发只读 / L3b worker worktree 隔离 / L3c worker 联网 / L3d frontmatter loader)。

### R5 — 更新 `docs/ARCHITECTURE.md`

- §1.1 Tool Registry 列表补全 19 个 tools(同 R3 工具集)。
- §1.1 Resource Loaders 段:Role loader 仍 backlog,但 `Command registry` 已落地(B3);`Memory loader` 描述对齐 `B5 指令文件加载 + V2 2 期 autonomous_memories 表` 双层。
- §2 "16 道关卡" 对账 `IMPLEMENTATION §4` 落地记录(尤其 §2.5.x 已实施小节),必要时补全缺失的 §2.5.9 / §2.5.10 / §2.5.11 引用,指向 `IMPLEMENTATION §4` 对应日期。
- 删 `HANDOFF.md` 引用。

### R6 — 更新 `docs/TECH.md`

- §1.1 "终端 `xterm.js + portable-pty`" → 改为"终端(todo)— L1a 用 tokio::process::Child 不带 PTY;L1b 后续接 `portable-pty`(暂不引入);`xterm.js` 同 backlog"。
- §1.4 模糊搜索 `~~nucleo~~(未采用)` → 加"`fuzzysort`(^3.1.0,B2 PR1 实际采用,前端 TS 库,极轻量)";删 `ignore` 行或加注"@文件补全实际改用更简实现,未引入 `ignore` crate"(与现有"gitignore 解析 `ignore`"措辞一致化)。
- §1.4 生成式 UI 三件套(ECharts/vue-table/vee-validate) → 加注"B9 当前落地范围仅 selector(复用 ask_user_question)/ code_block(hljs + 复制)/ diff(复用 DiffView);chart/table/form 推后期(D3 按钮白名单 + 自由式 UI 同档)"。
- §1.4 加 L1a 依赖说明:"`tokio::process::Child`(进程内)+ `BackgroundShellRegistry` trait(Q1 决策 C,daemon 化换 impl 不动调用点)"。
- 删 `HANDOFF.md` 引用。

### R7 — 更新 `docs/DEBUG_DB.md`

- §1 顶部"权威定义在 `migrations.rs`" 维持。
- §2 "Schema 索引(**9 张表**)" → **10 张表**(加 `autonomous_memories`,说明:2026-06-29 V2 2 期自主记忆 epic P1 存储落地,字段含 `memory_id` / `scope` / `kind` / `status` / `title` / `content` / `tags` / `tool_name` / `command_pattern` / `path_globs` / `source_session_id` / `source_ref` / `confidence` / `hit_count` / `last_used_at` / `demoted_reason` 等)。
- §2 表索引行补一行(`migrations.rs:707`)。
- §3.3 "5 个常用查询" 增加一条"看活跃的 autonomous memory(candidate/active 状态)",或保留 5 条但加一节"§3.4 autonomous memory 调试入口"。
- §5 故障排查表加一行"Memory 召回不命中" → `autonomous_memories.status NOT IN ('verified', 'active')` + `tool_name` / `command_pattern` 命中检查。
- 删 `HANDOFF.md` 引用。

### R8 — 更新 `docs/BACKLOG.md`

- §5.3 "`pick_project_dir` 改成前端 reka-ui 渲染 dialog ⏸ 未实施" → 加注"`fe91605 (07-01) fix: 冷启动不再总是落到第一个项目` 间接碰过项目初始化路径,但 dialog 仍未实施"。
- §3.3 安全边界表 "生成式 UI | 按钮 action 越权 | Tauri command 白名单" 行 → 改写对齐 B9 现状:"按钮 + action 走 D3 后期白名单;当前 B9 仅 selector/code_block/diff,无 action surface"。
- §1 / §2 标题已标 "已落地",无需改。
- 删 `HANDOFF.md` 引用(若有)。

### R9 — 更新 `docs/HACKING-llm.md`

- §现状一句话:加"OPENAI 也接入(06-08/09),`OPENAI_API_KEY` + `OPENAI_BASE_URL` 可选;Provider trait 跨协议中间层(`provider::wire::WireMessage`)统一 Anthropic / OpenAI 差异"。
- 加新章节"OpenAI Chat Completions 兼容层差异":
  - token 用量只在流末尾,需 `stream_options: { include_usage: true }` 才返回。
  - `cached_tokens` 在 `prompt_tokens_details.cached_tokens`(不是顶层字段)。
  - 不支持 `cache_creation_input_tokens`(归一化为 0)。
- 加新章节"`cache_control: ephemeral` 注入"(`B5 指令文件 + V2 2 期 recall result`),记 Anthropic 协议侧 schema。
- 删 `HANDOFF.md` 引用。

### R10 — 删 `docs/HANDOFF.md` + 全局反向引用清理

- 删 `docs/HANDOFF.md`。
- grep `HANDOFF.md` 验证 0 命中。
- `grep -rE '\bHANDOFF\b' --include='*.md' docs/ CLAUDE.md STRUCTURE.md .trellis/spec/ 2>/dev/null` 必须 0 命中。

## Out of Scope (不做)

- 不动 `ROADMAP.md` / `IMPLEMENTATION.md` / `HACKING-wsl.md` / `HACKING-markdown.md` / `SESSION-FIRST-MESSAGE-INTERFACE.md` / `README.md` / `STRUCTURE.md`(已 fresh 或沉淀型,本次不动)。
- 不动 `_archive/` / `_deprecated/` / `spikes/`。
- 不改 `.trellis/spec/`(spec 是另一套活)。
- 不重写文档结构,只做内容对齐 + 反向引用清理。
- 不迁移 `HANDOFF.md` 内容到别处(R1 删就删,权威在 `ROADMAP.md` + `git log`)。

## Acceptance Criteria

### AC1 — 删除验证
- [ ] `docs/HANDOFF.md` 文件不存在。
- [ ] `grep -rE 'HANDOFF\.md|HANDOFF\b' docs/ CLAUDE.md STRUCTURE.md .trellis/spec/ 2>/dev/null` 全部 0 命中(允许 `.trellis/tasks/` 自身提到过去任务)。

### AC2 — CLAUDE.md 同步
- [ ] "当前状态" 段日期刷新为 2026-07-02 视角。
- [ ] "MAX_TURNS=200" 标在 C3 行(替换原 50)。
- [ ] V2 第二档 7/7 状态准确(替换原 "2/7")。
- [ ] `agent/` 模块清单覆盖 16+ 子模块(含 `loop_detection` / `auto_reflect` / `memory_recall` / `memory_hygiene` / `question_store`)。
- [ ] `tools/` 列覆盖 19 个 builtin。
- [ ] `app/src/components/chat/` 提到 `<UiCard>` / `<WorkerBranchBadge>` / `<WorkerMergeControls>`。

### AC3 — DESIGN.md 同步
- [ ] §3.1 "已具备"工具集覆盖 19 个。
- [ ] §3.1 列出 B12 / L1 / L2 / L3 / B9 / C2 / RULE-D-001 / V2 2 期(简略 + 指向 ROADMAP §1.2)。
- [ ] §5.1 风险表 rig-core 行已调整(无 rig 风险)。
- [ ] §5.1 循环检测行描述 C2 分级触发。

### AC4 — CONTEXT.md 同步
- [ ] "Checklist" 不再标 "规划中",标"B12 2026-06-19 落地"。
- [ ] 新增 Subagent / SubagentRun / Worker Worktree / BackgroundShell / MAX_TURNS / Context Compression Thresholds / Loop Detection / AuditKind / L1-L3 命名约定 共 9 个术语条目。
- [ ] 现存 A4 术语保持不变。

### AC5 — ARCHITECTURE.md 同步
- [ ] §1.1 Tool Registry 覆盖 19 个 tools。
- [ ] §1.1 Resource Loaders 段描述对齐 B5 + V2 2 期。
- [ ] §2 "16 道关卡" 引用补全到当前(对照 `IMPLEMENTATION §4` 已实施日期)。

### AC6 — TECH.md 同步
- [ ] §1.1 terminal 行改写(L1a 不带 PTY)。
- [ ] §1.4 模糊搜索标 `fuzzysort`。
- [ ] §1.4 生成式 UI 三件套加注"B9 当前未引入"。
- [ ] §1.4 L1a 依赖说明行已加。

### AC7 — DEBUG_DB.md 同步
- [ ] §2 "10 张表"(替换 9)。
- [ ] §2 表索引行 `autonomous_memories` 字段列表已加。
- [ ] §3.3 + §5 故障排查有 autonomous memory 入口。

### AC8 — BACKLOG.md 同步
- [ ] §5.3 加注 `fe91605` 间接碰过。
- [ ] §3.3 生成式 UI 行改写对齐 B9 现状。

### AC9 — HACKING-llm.md 同步
- [ ] §现状一句话包含 OPENAI Provider。
- [ ] "OpenAI Chat Completions 兼容层差异"章节已加(至少 3 个 bullet:`stream_options.include_usage` / `cached_tokens` 归一化 / `cache_creation` = 0)。
- [ ] "`cache_control: ephemeral` 注入" 章节已加。

### AC10 — 验证命令

```bash
# 1. HANDOFF 已删 + 无反向引用
test ! -f docs/HANDOFF.md
grep -rE 'HANDOFF\.md' docs/ CLAUDE.md STRUCTURE.md .trellis/spec/ 2>/dev/null | wc -l  # = 0

# 2. MAX_TURNS 标 200 在 CLAUDE.md
grep -E 'MAX_TURNS.*200' CLAUDE.md

# 3. Tool 数 ≥ 19(grep builtin_tools 调用)
grep -c 'builtin_tools\|"read_file"\|"write_file"\|"edit_file"\|"shell"\|"grep"\|"glob"\|"list_dir"\|"web_fetch"\|"use_skill"\|"use_ui"\|"update_checklist"\|"remember"\|"ask_user_question"\|"dispatch_subagent"\|"merge_worker"\|"discard_worker"\|"run_background_shell"\|"shell_status"\|"shell_kill"' docs/DESIGN.md docs/ARCHITECTURE.md

# 4. DB 表数 10 张在 DEBUG_DB
grep -c 'CREATE TABLE\|autonomous_memories' docs/DEBUG_DB.md  # ≥ 10

# 5. Markdown lint(若有)
# 不强制,作为参考
```

### AC11 — git 工作流

- [ ] 全部改动一个 commit(`docs: 2026-07-02 doc sync — 删 HANDOFF + 8 份对齐当前实现`)。
- [ ] commit message 列出 8 个文件 + R1-R10 摘要。
- [ ] 不动 ROADMAP / IMPLEMENTATION / HACKING-wsl/markdown / README / STRUCTURE / SESSION-FIRST-MESSAGE-INTERFACE(已 fresh 或沉淀型)。

## Risks & Mitigations

| 风险 | 缓解 |
|---|---|
| 漏改反向引用导致断链 | AC1 grep 0 命中强校验 |
| 工具清单漂移(与 `tools/mod.rs` 不一致) | AC3 / AC5 校对 `app/src-tauri/src/tools/mod.rs::builtin_tools()` |
| DB schema 编号与实际漂移 | AC7 校对 `migrations.rs` CREATE TABLE 总数 |
| Module 路径漂移 | AC2 校对 `app/src-tauri/src/agent/mod.rs` 子模块 |
| 误改 ROADMAP/IMPLEMENTATION(已 fresh) | R10 + AC11 强约束不动 |
| 删除 HANDOFF 后 onboarding 断链 | R1 + R2 顶部"最近 commit hash 用 `git log -1 --oneline` 查"类提示已迁移或删除,CLAUDE.md 顶部明确指向 ROADMAP |

## Rollback

`git revert <commit>` 即可,文档改动无副作用。