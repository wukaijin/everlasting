# ROADMAP — 技术路线图

> **维护承诺(2026-06-10 锁定)**:本文档是 living document,随功能完善 / 需求更改及时更新。**实施 / git log 是终极归档**(完整 commit 列表见 `git log --oneline -20`),本文档只列宏观计划。
>
> 文档职责:
> - ✅ 做什么 + 什么时候做(V2 4 档分类 + 已实施粗粒度归类)
> - ❌ **不**讲具体实现细节(实现走 [IMPLEMENTATION.md §1](./IMPLEMENTATION.md) / [ARCHITECTURE.md](./ARCHITECTURE.md))
> - ❌ **不**讲历史决策(决策走 [IMPLEMENTATION/decisions.md 决策日志](./IMPLEMENTATION/decisions.md))
>
> 需求见 [DESIGN.md](./DESIGN.md),架构见 [ARCHITECTURE.md](./ARCHITECTURE.md),技术选型见 [TECH.md](./TECH.md),实现讲解 + 决策日志见 [IMPLEMENTATION.md](./IMPLEMENTATION.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

---

## 1. 已实施(MVP 主体 + 路线图外完成)

> 粗粒度归类,不逐 commit 罗列。具体 commit 走 `git log --oneline -20`。

### 1.1 MVP 主体(原 7 步路线图)

<details>
<summary>8 项里程碑(全部 ✅)— 点击展开</summary>

| 阶段    | 里程碑 | 状态 |
|---------|--------|------|
| MVP     | 步骤 1:Tauri 2 + Vue 3 + Rust 骨架,LLM 流式对话 | ✅ |
| MVP     | 步骤 2:Tool Calling(`read_file` / `write_file` / `shell`)+ Agent Loop | ✅ |
| MVP     | 步骤 3a:SQLite 持久化 + Session 管理 | ✅ |
| MVP     | 步骤 3b-1:Project 数据模型 + 顶部 Tabs UI | ✅ |
| MVP     | 步骤 4:Git 集成(worktree + opt-in attach / detach / delete) | ✅ |
| MVP     | 步骤 5:WSL 体验(spike-001 验证) | ✅ |
| v1      | 步骤 6a:多 Provider(Anthropic / OpenAI,自研 Provider trait) | ✅ |
| 跨阶段  | 步骤 8:代码重构(5 PR — lib.rs / db.rs / 前端 sub-components / 文档 / STRUCTURE.md) | ✅ |

> 步骤 3b-2(完整三栏 UI + rig-core 迁移)已废弃,详见 [IMPLEMENTATION §4 决策日志 2026-06-09](./IMPLEMENTATION/decisions.md)。
</details>

### 1.2 路线图外完成

| 功能 | 日期 | 一句话 + 链接 |
|------|------|--------------|
| Anthropic extended thinking 块展示 + 持久化 | — | thinking 块落库 + 前端交错渲染(设计见 [INTERLEAVED-THINKING](./_history/2026-08-28-interleaved-thinking-design.md)) |
| spike-005 follow-up 7 PR / 字体栈 / 6 UI bug 修复 / 工具集扩展批次 | 06 | 早期打磨批次:UI header / git_branch / backfill / pwd 简化 / write tracing / cancel / markdown 渲染 + HarmonyOS Sans 字体 + streamController 架构修复 + edit_file/grep/glob/list_dir/ReadGuard |
| provider catalog hot-reload + session model_id binding | 06-10 | [决策 06-10](./IMPLEMENTATION/decisions-2026-06.md) |
| 体验优化批次 F1-F4 | 06-11 | per-project session 记忆 / 跟底滚动 / 删除确认 / session 切换 loading |
| **A4** Token 用量统计 | 06-10 | per-session 4 列累计 + ChatInput 阈值色条 |
| **B5** Memory/指令文件系统 | 06-10/11 | 4 文件加载 + mtime fence + `cache_control: ephemeral`(spec [backend/memory.md](../.trellis/spec/backend/memory.md)) |
| **C1** 取消机制完整化 | 06-11 | tool 执行中途可取消 |
| **D1** session 重命名 + 8 色标记 | 06-11 | |
| **P0 工具打磨** / **P1 web_fetch** | 06-12 | read_file offset/limit + shell timeout;web_fetch(SSRF 拦截 + 5MiB cap + attribution,PRD [06-12-feat-tools-web-fetch](../.trellis/tasks/archive/2026-06/06-12-feat-tools-web-fetch-agent-api-p1/)) |
| **C3** Context 压缩 + token 硬卡 | 06-12 | 已被 C3+ 替代(见下) |
| **A2 + B7** 权限系统 + 多模式 | 06-12/13 | ⑨ 5-tier path 决策 + 3 档 Mode + 审计(spec [permission-layer.md](../.trellis/spec/backend/permission-layer.md),[决策 06-12/13](./IMPLEMENTATION/decisions-2026-06.md)) |
| **Mode 3 档化** | 06-13 | Chat→Edit 改名 + Review 移除 |
| **A7** RDP 双屏 position 修复 | 06-14 | Wayland 禁 setPosition → 全平台 `toggleMaximize()` |
| **C4** 审计日志查询 UI | 06-14 | `list_session_audit_events` + AuditLogModal |
| **RULE-E-006** worktree 路径对齐 app_data_dir | 06-15 | worktree 与 SQLite 同根 |
| **B3** /command 命令面板 | 06-16/17 | 内置 + 用户自定义(`.everlasting/commands/*.md`) |
| **B2** @文件补全 | 06-17 | @token 注入文件内容(调研 [at-file-injection](./_history/research/at-file-injection-coding-agents-survey.md)) |
| **D3** session 内消息编辑/重发 | 06-17 | edit_user_message 单事务 + Resend + `(edited)` 标签 |
| **B4** Skill 系统 | 06-18 | `use_skill` 三层渐进披露(调研 [skill-system](./_history/research/skill-system-survey.md),PRD [06-18-skill-system](../.trellis/tasks/archive/2026-06/06-18-skill-system/)) |
| **B12** Checklist | 06-19 | `update_checklist` tool + 前端卡片([决策 06-18](./IMPLEMENTATION/decisions-2026-06.md)) |
| **L2** 单 turn 多 tool 并发(只读 batch) | 06-19 | `is_parallel_eligible` + FuturesUnordered([ARCHITECTURE §2.5.9](./ARCHITECTURE.md#259-⑩-并行-tool-执行l2-mvp2026-06-19-落地已实施),PRD [06-19-l2](../.trellis/tasks/archive/2026-06/06-19-l2-parallel-readonly-tool-batch/)) |
| **L1** 后台 shell + 完成通知 | 06-19 | 3 tool + session-scoped + APPEND 通知(spec [tool-contract L1a](../.trellis/spec/backend/tool-contract.md),PRD [06-19-l1-shell-pty](../.trellis/tasks/archive/2026-06/06-19-l1-shell-pty/)) |
| **B6** Subagent + Drawer redesign | 06-20/21 | dispatch_subagent + subagent_runs 持久化 + SubagentDrawer(spec [frontend/chat.md](../.trellis/spec/frontend/chat.md)) |
| **RULE-D-001** api_key 加密 | 06-24 | AES-256-GCM + HKDF(machine-id),`api_key_enc` 列([决策 06-24](./IMPLEMENTATION/decisions-2026-06.md)) |
| **C2** 循环检测 / **C2+** 主动干预 | 06-24 / 07-06 | 分级触发(L1 硬 N=3 + L2 软 0.85);主动干预 per-run 询问(spec [agent-loop C2](../.trellis/spec/backend/agent-loop-architecture.md)) |
| **L3a** subagent 并发 / **L3c** worker 联网 | 06-24/25 | 并发只读 dispatch + worker web_fetch(spec [tool-contract concurrent](../.trellis/spec/backend/tool-contract.md)) |
| **L3d** subagent frontmatter loader | 06-25/26 | 用户/项目 agents/*.md + mtime fence |
| **L3b PR1-4** worker worktree 隔离 | 06-27/28 | branch 隔离 + merge/discard tool + sweep + 前端 UI(spec [worktree-contract](../.trellis/spec/backend/worktree-contract.md),PRD [07-04-a2-shell-p1p2-classify 同批](../.trellis/tasks/archive/)) |
| **V2 2期** 自主记忆系统 | 06-29 | autonomous_memories 表 + 状态机 + 两层召回 + 异步卫生(spec [backend/memory.md](../.trellis/spec/backend/memory.md) Scenario 2) |
| **B9** 生成式 UI / **B9+** 收尾 | 07-02 / 07-13 | use_ui non-blocking + UiCard + button/diff 应用 + UiDiffApplied 审计 |
| **B6+ A/B/C** subagent 多模型 | 07-03/06 | frontmatter model: + DB override + dispatch 动态选(spec [subagent-runs-schema](../.trellis/spec/backend/subagent-runs-schema.md)) |
| **A2+** shell 精细判定(P1+P2) | 07-04 | 复合命令拆分 + 写重定向检测(方案 [a2-shell-classification](./_history/2026-08-28-a2-shell-classification.md)) |
| **A5+** LLM 网络健壮性 | 07-05 | retry_open + Full Jitter + 首字节前重试(spec [llm-contract A5+](../.trellis/spec/backend/llm-contract.md),调研 [llm-network-resilience](./_history/research/llm-network-resilience-survey.md)) |
| **E1** CI 测试自动化 | 07-05 | GitHub Actions 双 job(prd [07-05-ci](../.trellis/tasks/archive/2026-07/07-05-ci-test-automation/)) |
| **V2-2+** 记忆可观测性 + 管理面板 | 07-06 | RuntimeMemoryModal + Recall 事件 + 人工编辑 provenance |
| `request_mode_change` tool | 07-07 | LLM 申请切 mode 用户 inline card 授权 |
| **B8** workflow 编排层 | 07-08~10 | workflow.json + 状态机 + breadcrumb + delegation + task.json(PRD [07-08-workflow-integration](../.trellis/tasks/archive/2026-07/07-08-workflow-integration/)) |
| pending-indicator 跨 session 三档提醒 | 07-08 | 角标 + 徽章 + toast |
| **E2** turn-level trace viewer | 07-14 | turn_trace 表 + TracePanel(spec [database-guidelines turn_trace](../.trellis/spec/backend/database-guidelines.md)) |
| **daemon 化**(5 Phase) | 07-20~23 | agent core 拆独立进程 + transport 抽象 + SSE([ARCHITECTURE §1/§4](./ARCHITECTURE.md),编排 [REMOTE-ACCESS-ROADMAP](./REMOTE-ACCESS-ROADMAP.md)) |
| **B11** 远程遥控通道(remote-control epic) | 08-11~13 | crates/everlasting-remote 云中继 + WSS 隧道 + PWA + workspace 翻转(部署 [REMOTE-DEPLOY](./REMOTE-DEPLOY.md),E2E [REMOTE-ACCESS-E2E](./REMOTE-ACCESS-E2E.md)) |
| daemon graceful shutdown 加固 | 07-25 | cancel drain + SSE 关闭 + 孤儿清理 |
| 交错思考渲染 | 07-23/24 | 真实流序落库 + 前端 run 分组(设计 [INTERLEAVED-THINKING](./_history/2026-08-28-interleaved-thinking-design.md)) |
| **C2** review-state 矩阵视图 | 07-26 | ReviewMatrix + commands/review.rs |
| lefthook pre-commit / ask_user_question 自由输入 / subagent resume(C1)+ plugin state(C0) | 07-26~28 | 工具链 + review epic 前置基建 |
| **群聊 group chat** | 07-29~08-07 | group_chat_loop 编排 + speaker 列 + role_history 隔离([决策 07-29~08-07](./IMPLEMENTATION/decisions-2026-08.md)) |
| 前端 vendor 分包 manualChunks | 08-11 | 主 chunk 1.08MB → 344kB |
| **C7** tools[] token 治理 | 08-14 | tools_token 度量 + 静态裁剪(spec [token-usage-tracking §C7](../.trellis/spec/backend/token-usage-tracking.md)) |
| **C7D** tools stub 注册 | 08-14 | STUB_CANDIDATES 原地 stub + load_tool_schemas 元工具 |
| **memory-gov** 指令块治理 | 08-15 | memory_token 度量 + digest 切节注入(spec [memory/decisions](../.trellis/spec/backend/memory/decisions.md)) |
| **B1** 图片支持(multimodal) | 08-16/17 | ContentBlock Image/ImageRef + supports_images + attachments 路由(spec [llm-contract Image](../.trellis/spec/backend/llm-contract.md)) |
| **D2①** 跨 session 全文搜索 / **D2②** search_history tool | 08-17 | messages_fts FTS5 + SearchModal;agent 侧 search_history(spec [database-guidelines messages_fts](../.trellis/spec/backend/database-guidelines.md) + [tool-contract 15](../.trellis/spec/backend/tool-contract/15-search-history.md)) |
| **C3+** 摘要式上下文压缩 | 08-18 | LLM 9 段摘要 + cutoff_seq 水位折叠(spec [pattern-llm-compaction](../.trellis/spec/backend/agent-loop-architecture/pattern-llm-compaction.md)) |
| **unified-context-budget** | 08-19 | at_files/system/context_window 三列 + 0.95 硬卡(spec [pattern-budget-gate](../.trellis/spec/backend/agent-loop-architecture/pattern-budget-gate.md)) |
| **MAX_TURNS 软卡** / **手动 /compact** / **handoff** | 08-19 | 撞线询问替代硬停 + 空闲期压缩入口 + 跨 session 接力(spec [pattern-turn-limit-softcap](../.trellis/spec/backend/agent-loop-architecture/pattern-turn-limit-softcap.md)) |
| **worker per-turn 度量** | 08-20 | turn_trace 并入 run 维度(表重建 UNIQUE 加 run_id) |
| **B1 收尾** 压缩 + 拖拽 + 工具读图 | 08-21 | canvas 压缩 + read_file 读图 + ToolResult.images |
| **F4** web_search 工具 | 08-25 | Tavily/DDG 双后端 + 无 SSRF 面(spec [tool-contract 16](../.trellis/spec/backend/tool-contract/16-web-search.md)) |
| **F1** 消息队列·用户连发档 | 08-25 | 输入排队 + 续轮批量注入 + TurnContinuation(spec [pattern-message-queue-driver](../.trellis/spec/backend/agent-loop-architecture/pattern-message-queue-driver.md)) |
| **F5** PDF/docx/xlsx 原生提取 | 08-26 | doc_extract 纯函数 + 指令式自助兜底;xlsx CSV 块(spec [pattern-doc-extraction](../.trellis/spec/backend/agent-loop-architecture/pattern-doc-extraction.md)) |
| **F6** 异步 agent 任务 + **F3** 全局并发闸 | 08-27 | SessionSummary.busy + 跨 session toast + max_concurrent_loops(spec [pattern-global-loop-semaphore](../.trellis/spec/backend/agent-loop-architecture/pattern-global-loop-semaphore.md)) |
| **F2/F2b** 定时任务 | 08-28 | scheduler 30s tick + due 落账 + origin 链;6 档 + 结束条件(spec [backend/scheduled-tasks.md](../.trellis/spec/backend/scheduled-tasks.md)) |
| **LLM schedule_task 家族**(F2 detached dispatch 收口) | 08-29 | 三件套(create/status/cancel,作者面分离 created_by='agent',tool 侧双 gate;spec [tool-contract 17](../.trellis/spec/backend/tool-contract/17-schedule-task-family.md)) |
| **C6** 大输出截断统一 | 08-30 | tool_output 契约模块(三恢复模式 + 统一标记 + RULE-E-009 唯一实现);spill 迁 app_data_dir/outputs/<session>(权限 carve-out + 双路径 sweep);web_fetch 落盘恢复;修 >64KB 管道死锁(spec [pattern-output-truncation](../.trellis/spec/backend/agent-loop-architecture/pattern-output-truncation.md)) |

---

## 2. V2 路线图分类(2026-06-10 重排)

> 2026-06-13 收尾更新。

### 🟢 第一档 — ✅ 已全部完成(2026-06-10/11,本档收口)

> A4 / B5 / C1 / D1 四项均已落地，详见 §1.2 已实施列表。

### 🟡 第二档 — ✅ 已全部完成(2026-06-12/13/14/17,6 项进 §1)

| 编号 | 功能 | 备注 |
|------|------|------|
| ~~A2 + B7~~ | ~~权限系统 + 多模式(合并工作组)~~ | ✅ 06-12/13 落地,见 §1.2 |
| ~~C3~~ | ~~Context 压缩 + token 硬卡~~ | ✅ 06-12 落地,见 §1.2 |
| ~~B3~~ | ~/command 命令面板~ | ✅ 06-16/17 落地,见 §1.2 |
| ~~C4~~ | ~~审计日志~~ | ✅ 06-13/14 落地,见 §1.2(⑨ ⑩ 写入 + 查询 UI)|
| ~~B2~~ | ~@文件补全~ | ✅ 06-17 落地(PR1+PR1.5+PR2),见 §1.2 |
| ~~D3~~ | ~session 内消息编辑 / 重发~ | ✅ 06-17 落地(PR1+PR2+PR3+follow-up),见 §1.2 |

### 🟠 第三档 — 缓做(active 项)

| 编号 | 功能 | 备注 |
|------|------|------|
| ~~B9~~ | ~~生成式 UI~~ | ✅ 07-02 部分落地(selector/diff/code_block,button 推后期),见 §1.2 |
| ~~B9+~~ | ~~生成式 UI 收尾(button + action / diff 应用)~~ | ✅ 07-13 落地(D3 通用 button + D4 diff 应用 + UiDiffApplied 审计),见 §1.2 |
| ~~C6~~ | ~~大输出截断统一~~ | ✅ **2026-08-30 落地**(08-30-c6-output-truncation:tool_output 契约模块 + 五工具迁移 + spill 迁 app_data_dir + web_fetch 模式 A 恢复 + grep 行级指引;顺带修 >64KB 管道死锁与 shell/in_memory 裸切片 panic 两处存量缺陷,见 §1.2) |
| ~~B1~~ | ~~图片支持(multimodal)~~ | ✅ 08-16/17 落地(粘贴+@双入口/wire 双形态/占位降级/images_token 切片/DOMPurify 收紧),见 §1.2 |
| ~~D2~~ | ~~跨 session 全文搜索(双驱动)~~ | ✅ 双驱动 08-17 全部落地:① 用户驱动(全局 SearchModal + messages_fts + 只读预览定位)+ ② Agent 驱动 `search_history` tool(复用 `db::search` 查询层,不走 IPC),见 §1.2 |
| A5/A6 | 错误处理完善 + README + demo | 打磨 |
| ~~E1~~ | ~~CI 测试自动化管线(GitHub Actions)~~ | ✅ 07-05 落地(双 job 并行 + CI 首跑暴露的 drain race / mtime fence 2 个 flaky 修复),见 §1.2 |
| ~~A5+~~ | ~~LLM 网络健壮性(重试 / 指数退避 / SSE 断点续传)~~ | ✅ 07-05 落地(retry_open + Full Jitter + 首字节前重试;SSE 无 resumption 故不走 message ID 续传),见 §1.2 |
| ~~V2-2+~~ | ~~自主记忆可观测性 + 管理面板~~ | ✅ 07-06 落地(`update_memory` + `validate_memory_text` helper 提取 + `ChatEvent::Recall` 只读 event + worker sink 隔离 AC7 + `RuntimeMemoryModal` 状态机/编辑/删除 + ChatPanel 实时召回 chip;2 commits Phase A 后端 / Phase B 前端),见 §1.2 |
| ~~E2~~ | ~~turn-level harness trace viewer~~ | ✅ 07-14 落地(后端 trace 管道 + 前端独立面板 live+回看,4 维缺口全补),见 §1.2 |
| ~~C2+~~ | ~~循环检测升级为主动干预~~ | ✅ 07-06 落地(per-run-local `loop_hit_count` N=3 + QuestionStore 复用 + 三分支 + worker `effective_is_worker` 直接 break + `AuditKind::LoopIntervention` 无 migration),见 §1.2 |
| ~~A2+~~ | ~~shell 命令只读/副作用精细判定~~ | ✅ 07-04 落地(P1+P2 同 PR),见 §1.2 |
| ~~B6+~~ | ~~subagent 多模型支持（A frontmatter / B 动态选模型 / C UI+DB override）~~ | ✅ A 07-03 / C 07-03 / **B 07-06 全落地**，优先级链 `dispatch > DB > frontmatter > parent`，见 §1.2 |
| ~~L3b PR1~~ | ~~worker worktree 隔离核心(PR1 落地,见 §1.2)~~ | 06-27 PR1 已落地,见 §1.2;PR2-4 拆为 follow-up tasks |
| ~~C7~~ | ~~工具上下文渐进式披露(tools[] token 治理)~~ | ✅ 08-14 落地(R1 度量 + R3 静态裁剪;live 实测 tools 占首轮 context 38.5% → D(Stub)Phase 2 触发线 >15% 已过),见 §1.2 |
| ~~F1~~ | ~~消息队列(输入排队 / 优先级 / 批量注入)~~ | ✅ **A 档(用户连发)2026-08-25 落地**(排队+续轮批量注入+撤销/退回+Stop 清队;`TurnContinuation` 续轮边界事件),见 §1.2。B 档(优先级分档/抢占)仍开放 —— **C 档 cron 消费者已由 F2 交付(2026-08-28,统一入口 = chat_inner「闲也入队」路由);**LLM detached dispatch 已由 schedule_task 家族交付(2026-08-29)** |
| **F3** | 资源治理(系统级限损框架) | context/token 治理已落地(unified-context-budget / C7 / memory-gov / B1,见 §1.2);**agent loop 并发上限已落地**(F6 2026-08-27:全局信号量 `max_concurrent_loops` 缺省 4,见 §1.2);余下:进程 / 内存、磁盘(worktree / attachments / 日志),与 F1 反压联动。**边界**:不含 Provider API 限流(C5 已移除,见 §3) |
| ~~F4~~ | ~~Web 搜索工具~~ | ✅ 08-25 落地(`web_search` snippet-only 搜索 + `web_fetch` 全文两段式;Tavily/DDG 双后端;固定端点无 SSRF 面,非原设想"复用 web_fetch 安全模型"),见 §1.2 |
| ~~F5~~ | ~~PDF/Office 文档阅读~~ | ✅ **第一档(PDF + docx)2026-08-26 落地**;**follow-up xlsx/xlsm 提取同日落地**(每 sheet CSV 块,calamine 纯 Rust);pptx 用户裁定不做;pdfium 渲染扫描件、正式 document skill 仍留 follow-up,见 §1.2 |

> **已实施的 22 项**(B6 / B6+ / B8 / B12 / B4 / C2 / C2+ / A7 / L2 / L1 / L3a / L3b PR1 / L3b PR2 / L3b PR3 / L3c / L3d / A2+ / A5+ / E1 / V2-2+ / E2 / C7)已从第三档或第四档移到 §1.2 已实施列表。

### 🔴 第四档 — 最远远期(app 主体完善之后)(开放 3 项:B10 / A2+ P3 / A4+;B8、B11、F2、F6 已完成迁 §1.2)

| 编号 | 功能 | 备注 |
|------|------|------|
| B10  | 飞书 IM | daemon 化已于 2026-07 作为独立基础设施落地(见 §1.2 "daemon 化" epic);B10 现可基于既有 daemon + transport 抽象推进,不再是"重大架构变更"阻塞。本档只评估飞书 channel 接入 |
| A4+ | 成本聚合视图(token → $) | **可做可不做**(2026-08-30 用户裁定,由第三档移入)。A4 per-session token 累计已有;若做:补跨 session / provider / day 汇总换算 + 每模型 $/M 价格表(provider 层现无 pricing 字段,原"纯前端聚合"估计偏乐观) |
| ~~B11~~ | ~~远程遥控通道(原"云端同步 Cloudflare Workers + D1")~~ | ✅ **08-11~13 已实施**(remote-control epic S1~S6b,08-13 合入 main),见 §1.2。中继方案:国内 2C2G 服务器 + 自研 Rust remote daemon;不做主动推送、不做多用户、不做跨节点同步 |
| A2+ P3 | shell 执行期沙盒兜底(bubblewrap/overlayfs/firejail) | A2+ P1+P2 **判定层** 07-04 落地(见 §1.2);P3 是判定层之下的独立**限损层** — 判定错了也限损(盲区 `VAR=val cmd` / `$var` 展开 / 拆分器引号极端误判靠它兜底)。**P3a 前置 spike 2026-08-31 通过**:主路线定 Landlock+seccomp(纯 Rust 零外部依赖,Codex 同款,微软内核全机群自带;bwrap 降为可选增强档;microVM 经 CubeSandbox/Zeroboot 评估不做),WSL interop(`.exe` 借 binfmt)逃逸及收口配方实测闭环 — 结论与 P3b 设计要点见 [spike PRD](../.trellis/tasks/08-31-a2-p3a-sandbox-spike/prd.md)。拆自 parent `07-04-a2-shell-classification`(已 archive,P1+P2 收口)。源方案 [docs/_history/2026-08-28-a2-shell-classification.md](./_history/2026-08-28-a2-shell-classification.md) §4 远期候选 |
| ~~F2~~  | ~~定时任务(本地 cron 式)~~ | ✅ **2026-08-28 落地**(daemon 常驻调度器 + preset 档位 + origin 标记链 + Settings 管理面;触发源 MVP 只做系统时间;catch-up 补跑一次;经两道外部评审,见 §1.2)。**F2b 调度模型扩展同日落地**(6 档 + 次数/日期结束条件,见 §1.2 F2b 行)。**LLM `schedule_task` 家族(detached dispatch)2026-08-29 落地**(create/status/cancel、`created_by='agent'` 作者面分离、tool 侧 kill-switch/上限双 gate,见 §1.2 与 spec tool-contract 17)。余下:fs 事件 / 本地 webhook 触发源 |
| ~~F6~~ | ~~异步 agent 任务(detach 后台跑)~~ | ✅ **编排面 2026-08-27 落地**(detach 运行时语义本就成立;补跨端 busy 可见性 + 完成 toast + F3 信号量 + 关闭确认,见 §1.2)。余下增强(系统级通知 / unread 持久化 / 等待态心跳)按需另立 |

---

## 3. 移除项 / 已废弃(V2 重排,2026-06-10 决定)

> **不再做**的项目归这里,避免认知噪音。决策日志已覆盖"为什么不做"。

### 3.1 移除(明确不做)

| 编号 | 项目 | 一句话原因 |
|------|------|------------|
| A1   | xterm.js 嵌入式终端 | v1 `shell` tool + 30K 落盘已覆盖"看 agent 在跑啥"的需求 |
| A3   | MCP 暴露 | 个人工具,工具集对外开放是 Claude Code 生态已经解决的问题,本项目杠杆不足 |
| C5   | Provider 限流(令牌桶) | 个人使用场景未撞到限流;v1 之后看实际用量再评估 |

### 3.2 已废弃(历史决策,保留归档)

- **3b-2 完整三栏 UI + rig-core 迁移** — rig-core 0.38.1 弃用(2026-06-09 决策,自研 `Provider` trait 已完整支持多 Provider),3b-2 同步废弃
- 决策依据见 [IMPLEMENTATION §4 决策日志](./IMPLEMENTATION/decisions.md)对应日期条目

---

## 4. 关键理解纠正(必须留笔,2026-06-10)

### 4.1 B6 = Subagent(**不是**用户切角色)

- **正确语义**:main agent 在 ⑥ LLM 决策后,派出一个 **worker agent** 跑独立 context(独立 messages / 独立 token 预算),完成后由 worker 把 **summary** 回填给 main agent
- **类比**:Claude Code 的 Task tool / OpenHands 的 subagent
- **harness engineering 学习价值高**:消息流隔离、context 预算管理、summary 注入位置,都是 harness 设计的核心命题
- **依赖**:B5 Memory 落地后(worker 需要 user/project memory 上下文)再做,效果最佳

### 4.2 B7 = Mode 是 A2 权限系统的 UX 层

- **正确语义**:B7(mode = `edit` / `plan` / `yolo`)**不是**独立功能,是 A2 权限系统的**前端 UX 层**;`Background` enum 留位但 UI 不暴露
- **历史演进**:2026-06-12 落地 4 档(`Chat` / `Plan` / `Review` / `Yolo`),2026-06-13 grill-with-docs session 3 档化(`Chat → Edit` 改名 + `Review` 移除,行为跟 `Plan` 重复);详见 [IMPLEMENTATION §4 决策日志 2026-06-13 "Mode 3 档化"](./IMPLEMENTATION.md)
- **联动链**:前端 mode 切换 → 后端 ARCHITECTURE §2.2 **⑧a Mode 检查**(plan 模式拒 tool_use / yolo 跳过 ⑨ Tier 4 弹窗但 Tier 2 硬墙仍生效) + ⑨ 权限检查 联动
- **工作组划分**:A2 + B7 合并做(基础设施 + UX 一组),已进 §1.2 已实施

### 4.3 A2 + B7 合并工作组(2026-06-12/13 完成,已进 §1.2)

- A2(后端 ⑨ 权限基础架构) + B7(前端 mode 切换 UI)是一组工作,不能拆
- 实施顺序:先 A2 后 B7(B7 依赖 A2 暴露的 mode 配置),3 档化(Q4 P2 后续)单列 ADR

---

## 5. 后续维护承诺

- **本文件改动时机**:
  - 完成 V2 任何一档任何一项 → 移到 §1 已实施(列"做了什么 + 时间",细节走链接,不写 commit hash)
  - 重新审视 V2 档位(升档 / 降档 / 移除) → 直接编辑 §2 / §3 + 在 [IMPLEMENTATION §4 决策日志](./IMPLEMENTATION/decisions.md) 追加 ADR 条目
  - V2 → V3 重排 → 整体替换本文件或归档到 `docs/_history/`
- **不做的边界(新增 / 编辑本文件时的自查点,与头部维护承诺一致)**:
  - ❌ 不列具体 commit / PR 编号(具体 commit 走 `git log`,日期即索引)
  - ❌ 不写测试数 / token 数字 / 实现机制等技术细节(具体设计走 BACKLOG.md / 各 spec 文件 / 对应 PRD 归档)
  - ❌ 不做决策追溯(走 IMPLEMENTATION §4 决策日志)
  - ✅ 新行 = 一句"做了什么 + 时间 + 链接(到 spec / PRD / 决策日志)"
- **其他文件引用本文件的统一形式**:`[docs/ROADMAP.md §X](./ROADMAP.md#X)`,不复制路线图内容到其他文件
