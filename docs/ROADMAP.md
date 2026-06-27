# ROADMAP — 技术路线图

> **维护承诺(2026-06-10 锁定)**:本文档是 living document,随功能完善 / 需求更改及时更新。**实施 / git log 是终极归档**(完整 commit 列表见 `git log --oneline -20`),本文档只列宏观计划。
>
> 文档职责:
> - ✅ 做什么 + 什么时候做(V2 4 档分类 + 已实施粗粒度归类)
> - ❌ **不**讲具体实现细节(实现走 [IMPLEMENTATION.md §1](./IMPLEMENTATION.md) / [ARCHITECTURE.md](./ARCHITECTURE.md))
> - ❌ **不**讲历史决策(决策走 [IMPLEMENTATION.md §4 决策日志](./IMPLEMENTATION.md#4-决策日志))
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

> 步骤 3b-2(完整三栏 UI + rig-core 迁移)已废弃,详见 [IMPLEMENTATION §4 决策日志 2026-06-09](./IMPLEMENTATION.md#4-决策日志)。
</details>

### 1.2 路线图外完成

| 功能 | 日期 | 备注 |
|------|------|------|
| Anthropic extended thinking 块展示 + 持久化 | — | |
| spike-005 follow-up 7 PR | — | UI 紧凑 header / git_branch 显示 / 启动 batch backfill / pwd `~/` 简化 / write_file tracing / LLM cancel 机制 / markdown 渲染 |
| 字体栈调整(HarmonyOS Sans SC 子集打包) | — | |
| 6 UI/状态 bug 修复 | — | streamController 架构 + 顶栏窗口控制 + Markdown 表格 + Tauri 2 权限 + minimize icon + 顶栏 padding |
| 工具集扩展批次 | — | `edit_file` / `grep` / `glob` / `list_dir` + ReadGuard + Bash 落盘 + cat -n 行号 |
| provider catalog hot-reload + display_name optional + session model_id binding | 06-10 | |
| 体验优化批次 F1-F4 | 06-11 | per-project session 记忆 / 全程跟底滚动 / ConfirmDialog + 删除确认 / session 切换 loading + 双 IPC 修 + reloadAfterFinalize 抖动修 |
| **A4** Token 用量统计 | 06-10 | per-session 累积 + ChatInput hint 区展示 |
| **B5** Memory/指令文件系统 | 06-10/11 | 4 文件加载 + notify 监听 + `cache_control: ephemeral` 注入 + 前端 MemoryPreview UI + Settings Tab |
| **C1** 取消机制完整化 | 06-11 | tool 执行中途可取消 |
| **D1** session 重命名 + 8 色标记 | 06-11 | |
| **P0 工具打磨** | 06-12 | `read_file` offset/limit + `shell` timeout |
| **P1 web_fetch 工具** | 06-12 | 新增 8 号 tool:agent 自主抓取外部文档/API 参考/错误信息,SSRF 拦截 (RFC 1918/loopback/link-local/CGNAT/multicast/reserved + 169.254.169.254 短路),5 MiB body cap,30s timeout,htmd 0.5 转 markdown,attribution prefix (T1a prompt-injection 缓解)。PRD + 3 份 research 落 `.trellis/tasks/06-12-feat-tools-web-fetch-agent-api-p1/` |
| **C3** Context 压缩 + token 硬卡 | 06-12 | 5a 加载层 token 预算 + 超限降级(参见 [ARCHITECTURE §2.5.5](./ARCHITECTURE.md#255-⑤-context-超限降级) + [ARCHITECTURE §2.2 ⑤](./ARCHITECTURE.md))。完整 PRD 走 `.trellis/tasks/archive/2026-06/06-12-c3-context-token/` |
| **A2 + B7** 权限系统 + 多模式(合并工作组) | 06-12/13 | ⑨ 关 5-tier 决策层(path-based,re-grill SOT)+ 3 档 Mode(`edit` / `plan` / `yolo`,`Background` enum 留位 UI 不暴露) + `match_kind` 3 种 wire 全连(`tool` / `prefix` / `path`)+ YoloConfirmModal + PermissionModal 路径范围行 + ⑯ 审计日志 10 类 AuditKind。`tools::web_fetch` 也接入 ⑨(Tier 4 走 `match_kind='tool'`) |
| **Mode 3 档化**(Q4 P2 后续) | 06-13 | `Mode::Chat → Edit` 改名 + `Mode::Review` 移除(行为跟 Plan 重复);v6 migration 启动时跑两次幂等 UPDATE;**breaking wire rename**,不保留 alias |
| **A7** RDP 双屏 position bug 修复 | 06-14 | 根因 = Wayland 禁止客户端 setPosition(WSLg/Weston 忽略,#14913 非可绕过),放弃手动铺满整屏,全平台改原生 `toggleMaximize()`;详见 [IMPLEMENTATION §4 2026-06-14](./IMPLEMENTATION.md#4-决策日志) |
| **C4** 审计日志查询 UI | 06-14 | ⑩ `tool_executed` 落表(`record_tool_executed_audit`,payload `{tool_name, tool_input, duration_ms, exit_code}`)+ Tauri command `list_session_audit_events` + `useAuditStore` + `<AuditLogModal>`(reka-ui Dialog,绑当前 session,kind 下拉 + "仅 critical" 复选 + 计数 + 刷新 + 按 kind 分发渲染)。⑬ ⑮ 仍只 tracing(收益低)。完整 PRD 走 `.trellis/tasks/06-14-audit-log-query-ui/`,架构描述见 [ARCHITECTURE §2.5.8](./ARCHITECTURE.md#258-⑯-审计日志a2--b7-pr1--c4-pr1pr2-落地2026-06-1314已实施) |
| **RULE-E-006** worktree 路径对齐 Tauri `app_data_dir` | 06-15 | 删 `git::data_dir()` env-based 函数 + re-export + 模块 docstring,`AppState` 加 `app_data_dir: PathBuf` 字段(落在 data-plane group 内,保留 Grill decision #2 catalog-after-db 不变式),`attach_worktree` 从 state 取,worktree 与 SQLite DB 同根 `~/.local/share/dev.everlasting.app/`,`/tmp` fallback 消失。`cargo check` 0 warning,`cargo test --lib` 484/484 pass。完整 PRD 走 `.trellis/tasks/06-15-p1-worktree-data-dir-tauri/` |
| **B3** /command 命令面板 | 06-16/17 | 输入框行首 `/` 触发命令自动补全面板;内置(`/help` 列全部命令 / `/clear` 清空消息保留 session / `/new` 新建 session)+ 用户自定义(`.everlasting/commands/*.md` 手写 frontmatter parser 解析 `name`/`description`/可选 `argument-hint` + Markdown body 展开后作 user message 走 `send()`)。`<TriggerMenu>` 组件为 B2 @文件 / B4 skill 预置触发器骨架(共享 trigger char + 数据源注入)。`serde_yml`/`serde_yaml` 均废弃 → 通用 `ResourceLoader` 内置手写 parser(零依赖,字段简单时够用)。源优先级 builtin > project > user(project 覆盖 user 同名)。PR1 `ac0592e`(后端 command palette + ResourceLoader + `clear_session_messages`)+ PR2 `d57788a`(前端 TriggerMenu + ChatInput `/` 触发 + 内置分发)+ PR3(用户命令 body 展开) |
| **B2** @文件补全 | 06-17 | 输入框 `@` 触发文件补全面板(fuzzysort 模糊匹配,复用 B3 `<TriggerMenu>` 第二 caller,与 `/command` palette 互斥)+ 后端 `files::walk_files`/`list_files`(gitignore + 默认排除 + 深度/数量上限)。CodeMirror 6 着色(@file `--color-tool-read` / /command `--color-accent`)。**后端 @token 注入文件内容**(对齐 CC/opencode/Aider/Cline,非路径提示):text 复用 `read_file` 截断(50KB head+tail + cat -n)注入,图片/PDF/Office/二进制**占位降级**(纯文本通道,multimodal 留 B1,文案引导 `pdftotext`/`pandoc`),无效路径(越界/不存在/不可读)保留原 token(email 不误伤)。二进制检测三层(NUL/非UTF-8/30% 控制字符)。PR1 `f3ac7a0`(前端 @面板 + walk_files)+ PR1.5 `1ed212c`/`8e7c975`(CodeMirror 迁移 + 着色)+ PR2 `a00adbc`(后端注入 + 降级)。6 家调研见 [docs/research/at-file-injection-coding-agents-survey.md](../research/at-file-injection-coding-agents-survey.md) |
| **D3** session 内消息编辑 / 重发 | 06-17 | PR1 后端 `edit_user_message` 单事务(in-place 改写 + cascade 截断后续 + AuditKind)+ PR2 前端 `MessageActionsMenu` + chat store `editMessage` + `MessageItem` edit mode + PR3 Resend 实质化(走 turn 边界 + agent loop 续编)+ `(edited)` 标签 + `AuditKind::ResendMessage` + RULE-A-010 spec 偏离声明 + follow-up `MessageActionsMenu` 点击无响应修复。PR1 `308d277` + PR2 `114b239` + PR3 `e747625` + follow-up `d6b6ad8` |
| **B4** Skill 系统 | 06-18 | `use_skill` 虚拟 tool + 三层渐进披露(L0 清单独立 synthetic message 常驻 / L1 `tool_result` 回填正文 / L2 `read_file` 拉 reference),对齐 Claude Code `Skill` / Hermes `skill_view` 业界模式。加载层独立 `SkillCache`(复制 B3 `resource_loader` 模式,B3 零改动,唯一差异:skill 是 `<name>/SKILL.md` **目录**非单文件 → scan 走子目录)。frontmatter 最小集 name+description(复用 B3 手写 parser,`serde_yml` 已废弃)。修正 BACKLOG §2 两处过时(注入消息流非 system prompt)。MVP 纯 LLM 自动触发(无用户 `/skill`、无 `allowed-tools`)。调研见 [docs/research/skill-system-survey.md](../research/skill-system-survey.md),完整 PRD 走 `.trellis/tasks/06-18-skill-system/` |
| **B12** Checklist(agent 自跟踪进度清单) | 06-19 | TodoWrite 式 `update_checklist` tool(全量替换 + 三态 `pending`/`in_progress`/`done` + 至多一 in_progress coerce)+ loop-local Vec(per-request,handle 走 `ToolContext` 不改 `run_chat_loop` 14 参签名)+ 每轮 ephemeral 注入(**append** 到请求副本,不入持久化 messages,不破坏 memory cache 断点 — trellis-check 修正原 prepend)+ 无新 DB 表(replay 从 DB history 还原,reload 按 `is_error` 过滤 cancel 合成 result)。前端 `<ChecklistCard>` ChatPanel 浮层(展开/最小化悬浮球 + 焦点动效)+ checklist store(客户端复刻 coerce)。先于 B6 subagent(注入机制小面 warm-up)。PR1 `994db84` + PR2 `1896470` + PR3 spec;决策见 [IMPLEMENTATION §4 2026-06-18](./IMPLEMENTATION.md#4-决策日志),术语见 [CONTEXT.md](./CONTEXT.md) |
| **L2** 单 turn 多 tool 并发执行(只读 batch) | 06-19 | `is_parallel_eligible` 纯谓词 + `FuturesUnordered`(每 task 内 check→execute→RULE-A-004 cancel 检查→audit→emit,`result_slots[i]` 按 tool_use 原始 index 回填 + `AtomicBool` 广播 cancelled)。并发集合 `{read_file,grep,glob,list_dir,use_skill}`(全静默 Allow,无 ask);排除 web_fetch(Tier4 默认 ask,Q2)+ 写类/shell/update_checklist。不变量:多 tool_result 单消息打包(parallel-tool-use 红线)+ RULE-A-004(cancelled 跳过 audit)+ execute_tool 签名未改 + 共享状态(`PermissionStore`/`SkillCache`/`ReadGuard` 均 Arc)并发安全。path-outside-root edge case 记 RULE-A-013(MVP 接受,follow-up 方案 a)。架构见 [ARCHITECTURE §2.5.9](./ARCHITECTURE.md#259--并行-tool-执行l2-mvp2026-06-19-落地已实施),调研见 [spikes](./spikes/2026-06-19-async-parallel-tool-research.md)。完整 PRD 走 `.trellis/tasks/06-19-l2-parallel-readonly-tool-batch/` |
| **L1** 后台 shell + 完成通知(L1a,不带 PTY) | 06-19 | `BackgroundShellRegistry` trait(Q1 决策 C,daemon 化换 impl 不动调用点)+ `InMemoryBackgroundShellRegistry` 进程内 impl(tokio 后台 task 拥有 Child,三触发 `select!`:`child.wait` / `kill_rx` / `sleep(max_runtime)`)。3 tool:`run_background_shell`/`shell_status`/`shell_kill`(Q2 Hermes terminal/process split);agent loop 每轮 `drain_notifications` + **APPEND** user message(Q3 opencode-pty 风格,APPEND 非 prepend 保 memory cache breakpoint,同 B12 checklist 不变量);通知仅 `exit_code`(Q4)LLM 主动 `shell_status` 拉;`max_runtime_ms` 默认 24h(Q6);session-scoped(Q7)+ `run_background_shell` Tier 4 Shell(同 shell,Plan 拦)/ `shell_status`·`shell_kill` Tier 5。复用 RULE-E-002 进程组 SIGKILL + RULE-E-001 safe_env(`apply_safe_env` 改 `pub(crate)`)。生命周期:`delete_session`→`kill_all_for_session`;`RunEvent::Exit`→`kill_all`。30KB spill + 1KB head/tail preview 复用 shell.rs。测试 651→680(+29)。Follow-up:`ShellEntry` 清理 sweeper([RULE-E-012](../.trellis/reviews/DEBT.md),P2)+ L1b 真 PTY(`portable-pty` + `pty_write` 交互式 dev server)+ L3 并行 subagent。完整 PRD 走 `.trellis/tasks/06-19-l1-shell-pty/`,spec 见 [tool-contract.md "Scenario: L1a Background Shell Tools"](../.trellis/spec/backend/tool-contract.md) |
| **B6** Subagent + SubagentDrawer redesign 收尾 | 06-20/21 | dispatch_subagent tool + worker agent(独立 context/token 预算)+ subagent_runs 持久化(RULE-A-014/015/016)+ transcript pairing + SubagentDrawer(B6 PR1-3 + FT-F-001~005)。**redesign 重构 PR1-6**(06-21):DB task/final_text 列(PR1 `86a81b2`)+ RunAccumulator 删 chat_event 暴露(PR2 `6e077b3`)+ MarkdownDetailModal/useTruncate(PR3 `a39ad00`)+ Drawer 视觉子组件 DrawerThinkingBlock/DrawerToolCallCard(PR4 `e66001e`)+ 5 段分组折叠重写(PR5 `3db2be2`)+ 边界态 cancelled/error/permission_ask(PR6 `d9f999f`)。详见 [.trellis/spec/frontend/chat.md](../.trellis/spec/frontend/chat.md)。降级债(permission_ask interactive / cancelled turn N)见 [DEBT.md](../.trellis/reviews/DEBT.md) RULE-FrontSubagent-003/004 |
| **RULE-D-001** provider api_key 加密存储 | 06-24 | P1 安全债收口。AES-256-GCM + HKDF(machine-id) 派生 master key,AAD=provider id;新列 `api_key_enc` + `key_migrated_at` 哨兵 + 启动幂等迁移抹除旧明文。`ProviderRow.api_key` `#[serde(skip)]` 切断 IPC,`list_providers` 返 `hasKey`;前端编辑留空覆盖(None=保持/Some=覆盖)+ 加密徽标。否决 keyring(WSL 不可用)+ stronghold。威胁模型防 DB 文件泄露(无 machine-id 解不开)。完整 PRD 走 `.trellis/tasks/archive/2026-06/06-24-p1-api-key-encryption/`,决策见 [IMPLEMENTATION §4 2026-06-24](./IMPLEMENTATION.md#4-决策日志) |
| **C2** 循环检测(agent loop ⑬ 关卡) | 06-24 | 架构预留、代码零实现的 ⑬ 关卡落地。**分级触发**取代原文单一 0.9 阈值(L1 精确签名硬触发 N=3 + L2 Jaccard 软提示 N=5/0.85),因单一阈值无法适配短/长 input。命中**软提示**:hint 作为 `ContentBlock::Text` 注入 result message,LLM 下一轮看到提示,**不跳过执行、不终止 loop**,MAX_TURNS=200 仍是硬兜底。token 切分纯 Rust `split_whitespace`(不复用 tiktoken)。edit_file 签名含 old_string(避免正当多块编辑误判)。无 AuditKind 落表(§2.5.8)。`agent/loop_detection.rs`(31 单测)+ `chat_loop` 接入 + 2 集成测试,855 测试全绿。完整 PRD 走 `.trellis/tasks/06-24-c2-loop-detection/`,架构见 [ARCHITECTURE §2.5.4](./ARCHITECTURE.md),决策见 [IMPLEMENTATION §4 2026-06-24](./IMPLEMENTATION.md#4-决策日志) |
| **L3a** subagent 并发(只读 worker fan-out) | 06-24/25 | serial path 加 `DispatchBatch` 分类(Serial/OverLimit/Concurrent)+ 纯 dispatch 批(≥2)`FuturesUnordered` 并发(复用 L2 只读 batch 模板 `result_slots[i]`+`Arc<AtomicBool>`)+ 运行时 `force_readonly` 剥写(`READONLY_TOOL_ALLOWLIST` 只留 read/grep/glob/list_dir,只读保证第2层;第1层 SubagentDef allowlist,第3层 is_worker Deny 兜底)+ env `DELEGATION_MAX_CONCURRENT_CHILDREN` 默认3硬拒超限(对齐 Hermes)。**3 竞态点只读范围消解**(permission:ask is_worker 塌缩 Deny / token `col=COALESCE(col,0)+?` 原子增量 / cancel `child_token` fan-out)→ 零并发控制代码。`run_subagent` 加 `force_readonly` 参(serial 传false保护B6)。前端 store 按 runId 天然支持 N concurrent(PR2 实质满足)。spec 见 [tool-contract "Concurrent dispatch"](../.trellis/spec/backend/tool-contract.md) + [agent-loop "Concurrent readonly dispatch"](../.trellis/spec/backend/agent-loop-architecture.md)。**worker 联网(web_fetch)拆独立 task** `06-25-subagent-web-access`(L3a 验证发现 worker 三层不能联网)。完整 PRD 走 `.trellis/tasks/06-24-l3a-readonly-concurrent/`,决策见 [IMPLEMENTATION §4 2026-06-25](./IMPLEMENTATION.md#4-决策日志) |
| **L3c** subagent 联网(worker web_fetch) | 06-25 | researcher `SubagentDef.tools` + 并发 worker `READONLY_TOOL_ALLOWLIST` 各加 `web_fetch`(第 1+2 层),第 3 层零改动。**基线验证推翻"is_worker Deny"前提**:worker ask 2026-06-22 已走 `WorkerAskBanner` round-trip(`ask.rs:124` biased select),且 worker `PermissionContext.session_id`=父 session → `check.rs:257` `check_tool_grant` 已查父 session grant,故"父 session 授权过 web_fetch → worker 自动 Allow;无 grant → 弹 banner"天然工作。L3a 验证时 worker 报"无 web_fetch"纯是第 1+2 层剥掉工具。`READONLY_TOOL_ALLOWLIST` 加 web_fetch 不波及 L2(独立谓词 `is_parallel_eligible`)。并发 N worker 无 grant 时弹 N banner 接受现状(AllowAlways 不持久化,防跨权限边界),silent allow/持久化/配额作 follow-up。顺手修 LLM-facing `dispatch_subagent` description 过时"worker no UI"+ `tool-contract.md`/`dispatch.rs` 同款过时注释。864 测试全绿。完整 PRD 走 `.trellis/tasks/06-25-subagent-web-access/`,决策见 [IMPLEMENTATION §4 2026-06-25](./IMPLEMENTATION.md#4-决策日志) |
| **L3d** subagent frontmatter loader(第三档收口) | 06-25/26 | 用户 `~/.config/everlasting/agents/*.md` + `<project>/.everlasting/agents/*.md` 定义 sub-agent;`SubagentCache` mtime fence 加载(复用 B3 CommandCache / B4 SkillCache 同款 inline-array parser),project > user > builtin last-write-wins,`tools` 字段可选(覆盖 builtin 同名且未声明 → 继承 builtin;全新 agent 未声明 → `vec![]` 全工具集 — deepseek review 修正原 PRD "必填")。`dispatch_subagent` 从 `builtin_tools()` 启动快照拆出,改每 turn `definition_with_cache(&SubagentCache, project_path)` 动态拼 enum + source tag(`builtin` / `user` / `project`)。**砍 PRD 的 `/reload-subagents` 命令**(B3/B4 同款 mtime fence 自动 reload)。`SubagentDef` 全 owned(PR1 纯重构铺路)。**防 worker 嵌套靠 `effective_is_worker` gate**(`chat_loop.rs` 跳过 dispatch_subagent 的 per-turn append),`STRUCTURALLY_DISABLED` filter 退为 defense-in-depth — filter 只过滤 seed list,不过滤共享 `run_chat_loop` body 的 per-turn append(PR3 check 发现的 BLOCKING 回归)。`MockProvider` 加 `sent_tools()` 可观测性。**修订 PRD**:R1 user 路径 `~/.config/everlasting/agents/`(非 `~/.everlasting`,跟 B3/B4/B5 一致)+ R2 复用 **Skill** loader(非 B3 — B3 scalar-only 不支持数组)+ R3 删"YAML fail-fast"伪命题(手写 parser 全容错)。909 测试全绿(含 PR1 owned 化 + PR2 loader 39 新测 + PR3 definition_with_cache 4 新 + no-nesting 回归),`vue-tsc --noEmit` 0 err。完整 PRD 走 `.trellis/tasks/06-25-l3d-subagent-loader/`,决策见 [IMPLEMENTATION §4 2026-06-26](./IMPLEMENTATION.md#4-决策日志),deepseek 审查见 [`docs/_reviews/REVIEW-l3d-subagent-loader-deepseek-v4-pro.md`](./_reviews/REVIEW-l3d-subagent-loader-deepseek-v4-pro.md) |
| **L3b PR1** worker worktree 隔离核心(serial 路径) | 06-27 | `git::worktree::create_worker` / `destroy_worker` 变体(branch 前缀 `worker/<run_id>` + base = parent session worktree HEAD + `git worktree lock` 跑期间 + self-heal 复用 session 变体的三态恢复);`SubagentDef.isolation: Option<bool>` 字段 + builtin `general-purpose: Some(true)` / `researcher: None`;`dispatch_subagent` tool `isolation` 入参 + `resolve_isolation` 双层合并(dispatch > frontmatter > 默认隔离);`run_chat_loop` 加 25 参 `worktree_override: Option<PathBuf>`(仿 23 参 `system_prompt_override` 模式,worker 隔离时切 `ToolContext.worktree_path`)+ 26 参 `app_data_dir` pass-through;`ReadGuard::new()` reset(worker 新 checkout 无继承已读集合);`subagent_runs.worktree_path TEXT NULL` 列 + `insert_run_with_id`(caller 预生成 id 让 worker worktree 路径可派生);worker 完成 `git::diff::diff_worker_worktree` 判 changes → 有保留 branch + diff summary 回填 / 无 destroy。941/942 测试绿(C3 pre-existing 见 [DEBT.md RULE-A-017](../.trellis/reviews/DEBT.md));spec 见 [tool-contract](../.trellis/spec/backend/tool-contract.md) + [agent-loop-architecture](../.trellis/spec/backend/agent-loop-architecture.md) + [worktree-contract](../.trellis/spec/backend/worktree-contract.md) + [subagent-runs-schema](../.trellis/spec/backend/subagent-runs-schema.md);决策见 [IMPLEMENTATION §4 2026-06-27](./IMPLEMENTATION.md#4-决策日志)。PR2-4 拆为 follow-up tasks(concurrent dispatch 解锁 `force_readonly` → 各 worker worktree / `merge_worker` + `discard_worker` tool + sweep / 前端 SubagentDrawer 合并/丢弃 UI) |
| **L3b PR2** concurrent dispatch 解锁 `force_readonly` → 各 worker worktree | 06-27 | `chat_loop.rs` concurrent 分支删 `force_readonly=true`(`run_subagent` 仍传 false);`force_readonly` 参保留 serial-only(L3a `single_dispatch_runs_serial_path_unchanged` 回归 + 未来「force read-only」opt-in feature 兼容);race-dissolution proof 重导(4 竞态点 — 新增 **worktree write race** = per-worker `worker/<run_id>` branch 消解;`permission:ask` 改 N banner 接受现状 + workaround pre-AllowAlways;token 用量 2026-06-26 reversal 不 fold 进父;cancel fan-out 不变);新增 3 个集成测试 + 1 个 L3a 测试改名(`l3a_concurrent_general_purpose_workers_complete_readonly` → `l3b_concurrent_general_purpose_workers_complete_shared`,加 `isolation: false` dispatch 入参复刻 L3a shared-cwd 行为);`make_harness_with_git_repo` 共享 helper(PR1 inline review 加,PR2 提升到 tests_common)。`run_chat_loop` 签名不变(concurrent 分支只调 `run_subagent` 的 `force_readonly=false`)。12 L3 测试全绿,944/945 全绿(C3 pre-existing)。spec 改写:agent-loop "Concurrent readonly dispatch" → "Concurrent isolated dispatch" + race-dissolution 表重导 + tool-contract "Concurrent dispatch warning" L3b PR2 段取代 PR1 partial-mitigation 段。决策见 [IMPLEMENTATION §4 2026-06-27](./IMPLEMENTATION.md#4-决策日志)。PR3-4 拆为 follow-up tasks(`merge_worker` + `discard_worker` tool + sweep / 前端 SubagentDrawer 合并/丢弃 UI) |
| **L3b PR3** `merge_worker` / `discard_worker` tool + sweep | 06-27 | 关闭 L3b PR1 留下「有 changes worker branch 没人合并/丢弃」环:两个新 builtin tool(LLM-driven)+ 启动 sweep(过期 mtime 自动清理)+ Tauri command `merge_worker_run` / `discard_worker_run`(PR4 SubagentDrawer 按钮用)。`merge_worker` libgit2 fast-forward / 3-way merge,冲突 → 返 conflict 文件列表 + `is_error: true` + 保留两边 branch(用户手动 resolve);`discard_worker` 销毁 worker worktree + branch + clear `subagent_runs.worktree_path` 列;**MVP 不做幂等**(PR3 PRD "幂等 follow-up" — fail-fast 返 `worker already destroyed`)。`ToolContext` 加 `db: SqlitePool` 字段(merge/discard 需读 `subagent_runs` + project row;Clone Arc-internal 不影响 per-turn clone 模式)。`commands/subagent_runs.rs` 加 2 IPC 共享 tool 层 helper(`do_merge_blocking` + `finalize_merge` / `do_discard`)。`git::worktree::sweep_stale_worker_worktrees(app_data_dir, project_uuid, project_path, cleanup_period_days)` libgit2 扫 worker dir + libgit2 `is_locked()` 跳过 active worker + mtime > N 天 destroy;`EVERLASTING_CLEANUP_PERIOD_DAYS` env 覆盖,默认 7 天对齐 Claude Code。`lib.rs::run` 启动时 `tauri::async_runtime::spawn` 一次性 sweep(非 await,不挡 Tauri 窗口首绘)。**MVP 简化**:并发 N `merge_worker` 同一 parent branch 走 LLM 单 turn 串行 + frontend drawer vs LLM 走 user permission UX 隔,不加 `Mutex<parent_session_id>`;`worktree_override` 语义不参与 PR3(worker 仍由 PR1 `worktree_override` 切 worktree,merge 回 parent session branch 走 `session/<id>`)。955/956 全绿(C3 pre-existing);新增 11 PR3 测试(5 l3b_merge/discard_* + 6 sweep_/resolve_cleanup)+ 12 tool test ctx `db` 字段接线。spec 增:`tool-contract.md` 新 "merge_worker / discard_worker" Scenario(签名 + Tier 4 + Conflict UX + 11 tests)+ `worktree-contract.md` 新 "Worker Worktree Sweep" Pattern(lock 跳过 + mtime + env + 6 tests)。决策见 [IMPLEMENTATION §4 2026-06-27](./IMPLEMENTATION.md#4-决策日志)。PR4 拆为 follow-up task(前端 SubagentDrawer 合并/丢弃 UI 按钮) |
| **L3b PR4** 前端 SubagentDrawer 合并/丢弃 UI | 06-27 | 闭合 L3b PR3 backend `merge_worker_run` / `discard_worker_run` IPC 在前端的可见/可控环。新增 `<WorkerBranchBadge>` (status + worktreePath 派生三态) + `<WorkerMergeControls>` (Merge/Discard 按钮 + ConfirmDialog 二次确认 + 冲突 inline 文件列表 + 0 i18n key 全中文);store 加 `mergeWorker` / `discardWorker` actions + per-run `mergeStateByRunId: reactive Map` spinner 隔离(多 drawer 互不阻塞);`parseConflictFiles` 正则提取 conflict 文件列表;`formatWorkerBranchLabel` util 把 `worker/<run_id>` → `Worker <8-char hash>`。**严格可见门**(不是单字段):`worktreePath != null && status === 'completed'` — cancelled/error/incomplete worker 不显按钮,即便 disk worktree_path 残留(L3b PR3 sweep 清后)用 worker exit-state 作权威信号。**Store cache 单源模式**:WorkerMergeControls 只接 `runId` prop,不接 `worktree-path`(`getRunCache` 是 SoT,`mergeWorker` 成功后 reactive `.set(runId, {...row, worktreePath: null})` → `v-if="visible"` 自动 false → 按钮消失,父无需 re-thread prop)。Icon.vue 加 `GitMerge`(lucide)。`vue-tsc --noEmit` 0 err + 597/597(初)→ 598/598(C5b 严格门 regression test 加后)vitest 全绿。spec 增:`frontend/chat.md` 加 "L3b PR4 SubagentDrawer merge/discard UI" 章节(新文件清单 + 严格门 + store 契约 + Conflict 跨层契约 + 单源模式决策) + `frontend/state-management.md` 加 "Per-run spinner isolation via reactive Map" Pattern + `backend/tool-contract.md` 加 "merge_worker conflict error string contract" 跨层契约(正则锁定)。决策见 [IMPLEMENTATION §4 2026-06-27](./IMPLEMENTATION.md#4-决策日志) |

---

## 2. V2 路线图分类(2026-06-10 重排,2026-06-13 收尾更新)

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
| B9   | 生成式 UI(4 primitives — button / selector / diff / code_block) | 输出层扩展 |
| C6   | 大输出截断统一 | ⑩ ⑫ 边界处统一处理 |
| B1   | 图片支持(multimodal) | 输入层扩展 |
| D2   | 跨 session 全文搜索(双驱动) | ① 用户驱动(MVP,1 PR)+ ② Agent 驱动(`search_history` tool);共享 `search_messages`;先①后②;降档理由(2026-06-17):session 积累尚浅 + B5/C3 已覆盖"当次 memory"层。详见 [IMPLEMENTATION §4 2026-06-17](./IMPLEMENTATION.md#4-决策日志) |
| A5/A6 | 错误处理完善 + README + demo | 打磨 |
| ~~L3b PR1~~ | ~~worker worktree 隔离核心(PR1 落地,见 §1.2)~~ | 06-27 PR1 已落地,见 §1.2;PR2-4 拆为 follow-up tasks |

> **已完成的 13 项**(B6 / B12 / B4 / C2 / A7 / L2 / L1 / L3a / L3b PR1 / L3b PR2 / L3b PR3 / L3c / L3d)已从第三档移到 §1.2 已实施列表。

### 🔴 第四档 — 最远远期(app 主体完善之后)(3 项)

| 编号 | 功能 | 备注 |
|------|------|------|
| B8   | 可编排(DAG workflow) | 编排层,多 agent 串行/并行 |
| B10  | 飞书 IM | **触发 daemon 化**,重大架构变更 |
| B11  | 云端同步(Cloudflare Workers + D1) | 个人远程遥控通道 |

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
- 决策依据见 [IMPLEMENTATION §4 决策日志](./IMPLEMENTATION.md#4-决策日志)对应日期条目

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
  - 完成 V2 任何一档任何一项 → 移到 §1 已实施 + 加 commit hash 引用
  - 重新审视 V2 档位(升档 / 降档 / 移除) → 直接编辑 §2 / §3 + 在 [IMPLEMENTATION §4 决策日志](./IMPLEMENTATION.md#4-决策日志) 追加 ADR 条目
  - V2 → V3 重排 → 整体替换本文件或归档到 `docs/_archive/`
- **不做的边界**:
  - 不在本文件列具体 commit / PR 编号
  - 不在本文件做技术细节(具体设计走 BACKLOG.md / 各 spec 文件)
  - 不在本文件做决策追溯(走 IMPLEMENTATION §4 决策日志)
- **其他文件引用本文件的统一形式**:`[docs/ROADMAP.md §X](./ROADMAP.md#X)`,不复制路线图内容到其他文件
