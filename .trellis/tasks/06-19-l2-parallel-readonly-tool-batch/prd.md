# L2 — 单 turn 多 tool 并发执行(只读 batch)

> brainstorm 进行中。两份调研已采纳(`docs/spikes/2026-06-19-async-parallel-tool-{research,independent-research}.md`),本 PRD 不重复调研内容,只沉淀本项目落地决策。

## Goal

将 agent loop 的 tool 执行从串行改为并发,把单 turn 内多个独立 tool 的耗时从 `Σt` 降到 `max t`(典型场景:LLM 一次发 3 个 `read_file`)。MVP 仅限**纯只读 batch** 并发,写类/shell 保持串行,行为同现状。

## What I already know(代码现状实证)

- **改造点**:`agent/chat_loop.rs:994-1089` 串行 `for (id,name,input) in &tool_calls { permissions::check → execute_tool → audit → push result }`。
- **协议已就绪**:Anthropic 默认返回多 tool_use;`result_blocks` 已单消息打包(`chat_loop.rs:1133-1136`),符合 parallel-tool-use 约束(多 tool_result 必须同一 user message,否则 Claude "学会"避免并行)。
- **cancel 天然支持并发**:`execute_tool` 每个 tool 拿 `token.clone()`(`tools/mod.rs:121-144`,`tokio::select! { biased; cancel.cancelled() ... }`),并行时每 task 独立响应,**不改 cancel 传递机制**。
- **只读 batch 无共享写冲突**:纯只读 tool 不改 `current_ctx.cwd`(只有 shell 改,line 1069-1072)、不写 `read_guard`(只有 edit_file 写)。→ 只读 batch 并发时无共享状态冲突。
- **安全前置已清**:DEBT E-001(shell env 窃密)/E-003(web_fetch SSRF)closed。
- **审计范式基础**:A-004(cancel 后跳过 audit)closed,建立了串行 audit 范式;并行下需扩展为 per-tool 完成时序。

## Requirements (evolving)

- 单 turn 内,若 LLM 返回的所有 tool_use ∈ {read_file, grep, glob, list_dir, use_skill} → 并发执行;否则(含任意 write/edit/shell/update_checklist/web_fetch)整批串行(行为同现状)。
- 并发时 check+execute 整体并发(每 task 内 `permissions::check` → `execute_tool` → audit → emit),无拆阶段(并发集合全静默 Allow,无 ask)。
- 并发结果**按 tool_use 原始顺序**回填 `result_blocks`(保证 LLM 上下文稳定,不依赖完成时序)。
- cancel 广播:cancel token 触发 → 所有 in-flight task 被取消 → 标记整轮 cancelled → 走现有 cancel path(`chat_loop.rs:1091-1131`,persist partial + Done(cancelled))。
- 审计:per-tool 独立 audit 行,RULE-A-004(cancelled 跳过)语义保留;并行下按各 tool 完成顺序写入,不阻塞。
- `emit_tool_result` 流式发送(完成即发,不等全部),与现状一致。

## Decisions

- **[Q1] 并发边界 = 整批全"本地静默只读"才并发**:batch 全部 ∈ {read_file, grep, glob, list_dir, use_skill} 才并发;含任意 {write_file, edit_file, shell, update_checklist, web_fetch} 则整批退回串行。零依赖分析、最保守。
- **[Q2] web_fetch 排除出并发集合**:web_fetch 虽只读但 Tier4 默认 ask,纳入会引入并发 modal 问题。MVP 让它走串行(行为同现状),保留逐个 ask UX。→ 并发集合里**全部静默 Allow**,permission check 无 ask 风险,**check+execute 可整体并发**(无需拆两阶段)。
- **[Q3] 并发实现 = FuturesUnordered**:完成即 `emit_tool_result`(流式,匹配现状 line 1075);`result_blocks` 按 tool_use **原始 index 回填**(预分配 Vec 槽位),不依赖完成时序 → LLM 上下文稳定。技术最优,非 preference。

(Open Questions 已全部收敛)

## Acceptance Criteria (evolving)

- [ ] 3 个只读 tool(read_file ×3)并发,总耗时 ≈ max(单 tool),非 3×
- [ ] 含 1 个写类的 batch 仍串行(行为不变)
- [ ] 含 web_fetch 的只读 batch 仍串行(web_fetch 不并发,Q2)
- [ ] 含 update_checklist 的 batch 仍串行(归写类,Q1)
- [ ] 并发 batch 中途 cancel:所有 in-flight tool 取消,cancelled tool 不落 audit(A-004)
- [ ] `result_blocks` 按 tool_use 原始顺序,LLM 上下文正确
- [ ] `cargo test --lib` 全绿(现有 + 新增并发/cancel 用例)

## Definition of Done

- 新增/更新单元 + 集成测试(MockProvider 多 tool_use batch + 并发 cancel)
- `cargo check` 0 warning,`cargo test --lib` 全绿
- `ARCHITECTURE.md` §2.5 补"并行 tool 执行"小节
- `ROADMAP.md` L2 移到 §1.2 已实施(若完成)

## Out of Scope (explicit)

> **Follow-up(RULE-A-013,2026-06-19 trellis-check 发现)**:Q2 并行集合 `{read_file,grep,glob,list_dir,use_skill}` 在 **path-outside-root**(无 grant)时仍会触发并发 `permission:ask`(多 modal)。MVP 接受(概率极低:LLM 并行 batch 通常仓库内 read;无数据损坏:前端 `pendingBySession` 按 rid + `PermissionStore` 内 Mutex 串行化;仅 UX 乱),`chat_loop.rs:1000-1009` 注释已记录。收紧方案 (a):`is_parallel_eligible` 加 `projects::boundary::is_within_root` 检测,任一 out-of-root read tool 拉回串行(推荐,低成本,保留"并发集合绝对 silent"不变量);方案 (b):两阶段 check-then-execute。见 [DEBT.md](../../../.trellis/reviews/DEBT.md) RULE-A-013。

- L1 后台 shell / L3 并行 subagent(独立任务,L1 结论见 spike §5.1)
- 写 tool 并发(write_file/edit_file/shell/update_checklist 永远串行)
- tool 分类器(同文件 read+write 隐式依赖分析)— MVP 用整批判定
- 用户可配 `parallel_tools: auto|never|always`(Cline 模式)— 后续
- 前端改动(ToolCallCard 已支持流式 result,MVP 不改 UI)

## Technical Notes

- 循环主体:`chat_loop.rs:994-1156`(execute → cancel path → result 打包 → 下一轮 LLM)
- cancel wrapper:`tools/mod.rs:121-144`(`token.clone()` 已支持并发)
- 共享状态:`current_ctx.cwd`(shell 写)、`read_guard`(edit_file 写)、`permission_asks`(交互)— 只读 batch 全不涉及写
- 相关 DEBT:A-004(audit cancel 时序,closed = 范式基础)
- 调研出处:`docs/spikes/2026-06-19-async-parallel-tool-{research,independent-research}.md` §2/§5/§6

### Permission tier 实证(`agent/permissions/mod.rs`,2026-06-19)

`classify_tool`(line 711-720)+ Tier 4 内部分档(line 33-37 ReadOnly→静默 Allow):

| tool | ToolKind | Risk | 默认行为 | 并发安全? |
|---|---|---|---|---|
| read_file/grep/glob/list_dir | Path | Low | Tier4 ReadOnly → **静默 Allow** | ✅ 无 ask |
| use_skill | Other | Low | "Default Allow"(line 708) | ✅ 无 ask |
| update_checklist | Other | Low | Tier5 default-allow(tests.rs:2427) | ✅ 无 ask,但 Q1 归"写类"串行 |
| **web_fetch** | WebFetch | Low | **默认 emit ask**(line 663-664,除非 `check_tool_grant` 命中) | ⚠️ **唯一会 ask 的只读 tool** |
| write_file/edit_file | Path | Medium | Tier4 SideEffect | 串行(Q1) |
| shell | Shell | High | Tier4 SideEffect | 串行(Q1) |

→ **只读集合中,web_fetch 是并发 ask 风险的唯一来源**。Q2 聚焦它。
