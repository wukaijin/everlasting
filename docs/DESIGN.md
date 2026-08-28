# DESIGN — 需求设计

> Everlasting 的"是什么、为什么、边界在哪"。这是文档的入口。
> 架构设计见 [ARCHITECTURE.md](./ARCHITECTURE.md),技术选型见 [TECH.md](./TECH.md),决策档案见 [IMPLEMENTATION.md](./IMPLEMENTATION.md),技术路线图见 [ROADMAP.md](./ROADMAP.md),候选功能见 [BACKLOG.md](./BACKLOG.md)。

---

## 1. 文档目的

这不是产品规格说明,也不是产品愿景。这是**给自己看的工程决策备忘录**,用来:
- 在开始写代码前,把架构想清楚
- 记录"为什么这么选,而不是那么选"
- 之后再回来能快速回到上下文
- 讨论时有共同语言

讨论过程中产生的关键决策会沉淀到 [IMPLEMENTATION/decisions-2026-{06,07,08}.md 决策日志](./IMPLEMENTATION/)(按月分卷,入口索引在 `decisions.md`)。

---

## 2. 项目定位

### 2.1 一句话

**给个人用的 vibe coding 桌面工作台,WSL 是真正的家,agent core 自己写,顺带学 harness engineering。**

### 2.2 关键约束

| 维度     | 选择                                         | 反例                                  |
|----------|----------------------------------------------|---------------------------------------|
| 目标用户 | **仅本人**                                   | 不考虑他人付费、不考虑多租户          |
| 平台     | **WSL 优先**(Linux Ubuntu 22.04)             | Windows / macOS 都不主动支持          |
| 技术栈   | Tauri + Rust 后端 + Vue 3 前端             | 不考虑 Electron、PyQt 等              |
| Agent    | **自己写 runtime**                           | 不包装 Claude Code / Codex SDK        |
| 商业化   | 无                                           | 不会发出去、不考虑隐私多用户          |

### 2.3 核心差异点

普通 coding agent(Claude Code / Cursor / Copilot)都是通用工具。Everlasting 的存在理由是:

1. **WSL 体验做到极致** — 不是"凑合能用",是"Windows 用户在 WSL 里写代码的最佳伴侣"
2. **自研 agent core** — 学习价值 + 完全可控 + 不被厂商牵着走
3. **多项目 / 多 session / 工作流** — 不是一次性对话,是一个持久的工作环境

如果哪天这三点不再重要,这个项目就失去了存在理由。

---

## 3. 项目能力边界

> 本节讲"项目是什么 + 不是什么"。**做什么 + 什么时候做**见 [ROADMAP.md](./ROADMAP.md);**候选功能技术评估**见 [BACKLOG.md](./BACKLOG.md)。

### 3.1 项目能力(简略)

**已具备**(完整 commit 走 `git log`,粗粒度状态见 [ROADMAP.md §1](./ROADMAP.md#1-已实施mvp-主体--路线图外完成)):

- Tauri 2 + Vue 3 桌面应用,WSL 优先
- 自研 agent core:Agent Loop + Tool Calling + 流式 SSE + 18+ 关卡请求生命周期(详见 [ARCHITECTURE.md §2](./ARCHITECTURE.md#2-harness-设计从用户输入到文件变更的-16-道关卡);2026-08-14~19 加 C7 / C7D / memory-gov / C3+ / budget 硬卡 / softcap 6 个新横切关注点)
- 多项目 / 多 session 管理(SQLite 持久化)
- 工具集(2026-08-25 实测 27 个注册名 = 25 builtin(含 F4 `web_search`)+ 1 stub 元工具 `load_tool_schemas` + 1 动态 dispatch `dispatch_subagent`,`app/src-tauri/src/tools/mod.rs::builtin_tools()` 注册;filter_tools_for_mode/subagent/workflow 三层过滤):
  - 读 / 写:`read_file` / `write_file` / `edit_file`(ReadGuard 三道 check 前置)/ `grep` / `glob` / `list_dir`
  - Shell:`shell`(Bash 落盘 + cat -n)/ `run_background_shell` / `shell_status` / `shell_kill`(L1a 后台 shell,tokio Child 不带 PTY)
  - 联网:`web_fetch`(SSRF 拦截 + 5 MiB body cap,attribution prefix)+ `web_search`(F4 08-25,snippet-only,Tavily/DDG 双后端;固定端点无用户可控 URL → 无 SSRF 面,Tier 5 silent Allow;与 web_fetch 构成 search → fetch 两段式)
  - Skill / Memory / UI:`use_skill`(B4 三层渐进披露,workflow-aware)/ `use_ui`(B9 生成式 UI,non-blocking)/ `update_checklist`(B12 loop-local + workflow 分支同步 task.json.items)/ `remember`(V2 2 期自主记忆写入)/ `search_history`(D2② 08-17,`READONLY_TOOL_ALLOWLIST` 第 6 员,薄封装 `db::search::search_messages`)/ `load_tool_schemas`(C7D 08-14 stub 元工具,LLM 显式取回罕见工具 schema)
  - 交互:`ask_user_question`(跨 turn,B9 selector 复用)/ `request_mode_change`(B6+ A,07-07,LLM 申请切 mode 用户 inline card 授权)
  - Workflow(07-08~10,workflow_enabled session 可见,filter_tools_for_workflow 白名单):`create_task` / `request_task_state_transition`
  - Subagent:`dispatch_subagent`(B6)/ `merge_worker` / `discard_worker`(L3b worker worktree 收口,`ToolKind::GitMutation`)
- Git 集成:worktree 解耦 + opt-in attach / detach / delete;**L3b PR1-PR4 worker worktree 隔离**(branch 前缀 `worker/<run_id>` + `git worktree lock` + libgit2 fast-forward / 3-way merge + 启动 sweep 清理过期 worker)
- 多 LLM Provider(自研 `Provider` trait,Anthropic / OpenAI 双 Provider;rig-core 已弃用 2026-06-09)
- 顶层 GUI:三栏(Vue sub-components)+ SessionList + 顶部 Tabs + 流式指示器 + B9 `<UiCard>` + L3b PR4 `<WorkerBranchBadge>` + `<WorkerMergeControls>`
- A2+B7 权限系统:⑨ 关 5-tier path-based 决策层 + 3 档 Mode(`edit`/`plan`/`yolo`)+ ⑯ 审计日志 **28 类 AuditKind**(2026-08-28 实测,`ScheduledTaskFired` 为第 28 个,见 `app/src-tauri/src/agent/permissions/audit.rs`;按 Tool/Permission/Mode/Message/Loop/Worker/TaskStateTransition/Budget/UI/Scheduler 域分组)+ web_fetch 接入 ⑨ + **`ToolKind::GitMutation`**(L3b PR3+,WebFetch 式 tool-level grant,避免 Shell 串扰)(详见 [ARCHITECTURE §2.2 ⑨ / §2.5.8](./ARCHITECTURE.md))
- **C3+ LLM 摘要式压缩**(2026-08-18 落地,**替代 C3 MVP 机械丢组 0.80→0.50**):`context_window * 0.85` 触发(2026-08-19 起触发口径统一切换为 system+tools+messages 三部件之和,见 [ARCHITECTURE §2.5.5](./ARCHITECTURE.md))→ LLM 9 段模板结构化摘要(`task/progress/facts/decisions/open/files/next`)+ `prior-summary` 增量合并 + 保留区存活(`clamp(15k, 10%窗, 25k)` 最近 turn 逐字不丢)+ `cutoff_seq` 水位精确折叠;连续 3 次 LLM 摘要失败熔断回退 C3 机械丢组;叠加关卡⑤统一预算硬卡(`BUDGET_LINE_RATIO=0.95`,unified-context-budget 2026-08-19);撞线兜底见下(2026-08-19 起 MAX_TURNS 软卡询问,非硬停)(详见 [ARCHITECTURE §2.5.5/§2.5.14](./ARCHITECTURE.md))
- C2 循环检测:分级触发 — L1 精确签名硬触发 N=3 + L2 Jaccard 软提示 N=5/0.85;软提示命中后注入 `ContentBlock::Text` hint,**不打断 loop**,撞线兜底见下(2026-08-19 起 MAX_TURNS 软卡询问,非硬停)
- **MAX_TURNS 软卡**(2026-08-19 落地,**替代硬终断**):单聊主 loop 撞线(缺省 200)改 QuestionStore 询问——继续(+200)/ 压缩后续跑 / 停止,10 分钟超时兜底;worker 与群聊 speaker 段保持硬卡直接 break(详见 [ARCHITECTURE §2.5.15](./ARCHITECTURE.md) + [pattern-turn-limit-softcap](../.trellis/spec/backend/agent-loop-architecture/pattern-turn-limit-softcap.md))
- B5 Memory/指令文件系统:4 文件(User / Project × CLAUDE.md / AGENTS.md)+ `cache_control: ephemeral` 注入 + 100 KiB 硬卡 + tiktoken cl100k_base 估算 + mtime fence 新鲜度校验(notify 已移除)
- **memory-gov 指令块窗口治理**(2026-08-15 落地):`memory/digest.rs` fence-aware 切节目录(纯机械,标题+首句;`AGENTS.md` primary 永不 digest / `CLAUDE.md` 且 tokens>600 才 digest)+ `load_memory_sections` 元工具(append,精确寻址 banner label)+ `turn_trace.memory_token INTEGER` 度量(实测指令块 -79.5%,context_window 72% → 28%);`MemoryDigestRegistry` OnceLock 单例 + `memory_digest_enabled` 缺省 on(fail-open,worker / 群聊豁免)
- **V2 2 期** 自主记忆系统(2026-06-29 落地,5 child epic):agent 自主产生 + 跨 session 召回的经验库 — `autonomous_memories` 表(状态机 candidate→active→verified)+ 两层召回(per-turn FTS5 + 工具前 trigger_key 精确匹配)+ verified 软拦截重判 + 异步卫生 job
- A4 Token 用量统计:per-session 累积(4 列)+ ChatInput hint 区 0-49% 绿 / 50-74% 黄 / 75%+ 红
- D1 session 重命名 + 8 色标记
- C1 取消机制完整化:tool 执行中途可取消(CancellationToken)
- **B12** Checklist(agent 自跟踪进度清单):TodoWrite 式 `update_checklist` tool(全量替换 + 三态 pending/in_progress/done + 至多一 in_progress coerce),loop-local Vec,前端 `<ChecklistCard>` 浮层
- **L1a** 后台 shell + 完成通知:3 tool(`run_background_shell` / `shell_status` / `shell_kill`),session-scoped,默认 `max_runtime_ms` 24h,APPEND user message 保 memory cache breakpoint
- **L2** 单 turn 多 tool 并发(只读 batch):`is_parallel_eligible` 纯谓词 + `FuturesUnordered`,并发集合 `{read_file, grep, glob, list_dir, use_skill}`,多 tool_result 单消息打包
- **L3a-d** Subagent 全套:并发只读 dispatch / worker worktree 隔离 / worker 联网 / frontmatter loader(`~/.config/everlasting/agents/*.md` + `<project>/.everlasting/agents/*.md`)
- **B9** 生成式 UI(部分落地:selector / diff / code_block):`use_ui` tool + `<UiCard>` + component registry + `WorkerBranchBadge` / `WorkerMergeControls` for L3b PR4
- **RULE-D-001** provider api_key 加密存储:AES-256-GCM + HKDF(machine-id),`api_key_enc` 列 + `key_migrated_at` 哨兵,IPC 切断明文
- **B8** Workflow 编排层(07-08~10 完整落地):`workflow.json` 外置(`.everlasting/workflow.json` + `load_workflow` + `validate` + `fallback`)+ builtin dev workflow plugin(`resources/builtin-workflow/dev/workflow.json` 开箱即用)+ 任务状态机(Planning → Implement → Check → Done 四态单向)+ per-turn breadcrumb 注入(synthetic user message + `cache_control: ephemeral`)+ delegation 模板(`run_subagent` 时注入 worker)+ Step 0.1~3.3 完整 9 阶段管线(`workflow_enabled` 列 / 顶栏 toggle / `WorkflowDef` struct / `task.json` 读写 / `create_task` IPC / plugin skill loader / `set_task_state` + `archive_task` IPC)+ plugin agents/ 落点(`SubagentSource::Plugin`)+ `B12 Checklist → task.json.items` 同步 + `TaskStatus → Done` 触发 `trigger_spec_distillation` 沉淀 spec 到 `.everlasting/spec/`
- **daemon 化**(07-20~23,remote-access epic):agent core 拆出独立 `everlasting-daemon` 进程 + 前端 transport 抽象(httpTransport 默认)+ 纯浏览器模式(详见 [REMOTE-ACCESS-ROADMAP.md](./REMOTE-ACCESS-ROADMAP.md))
- **远程遥控通道**(08-11~13,remote-control epic S1~S6b):`crates/everlasting-remote` 云中继(仅转发不存 agent 数据)+ PC tunnel client(WSS 长连接 + loopback 转发,agent core 零改动)+ 手机 PWA 配对/节点/远程操作 + 移动端适配;Cargo workspace 翻转(详见 [REMOTE-ACCESS-E2E.md](./REMOTE-ACCESS-E2E.md) + [REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md))
- **C7 tools token 治理**(2026-08-14):`STUB_CANDIDATES` 静态裁剪(`filter_tools_for_session_type` drive.rs 第 3 环,按 session_type 砍不适用的 builtin)+ `turn_trace.tools_token INTEGER` 度量;实测 session 起步 tools_token -38.5%
- **C7D tools stub 注册**(2026-08-14):`StubRegistry`(session 粘性 loaded-set)+ `load_tool_schemas` 元工具按需取回 + `tools_stub_enabled` gate(开关 && 非 worker && 非群聊时生效);C7 + C7D 联合实测 -62%
- **B1 image multimodal**(2026-08-16/17):`ContentBlock::Image` / `ImageRef` 双形态 + `models.supports_images` 配置(`INTEGER NOT NULL DEFAULT 0`,`db/migrations/schema.rs:1012`)+ `messages.metadata.attachments[]` 引用 attachments 表 + **首个二进制 GET 路由** `GET /api/v1/attachments/<id>`(daemon `daemon/routes/attachments.rs`,手机 PWA 看图路径)+ `turn_trace.images_token INTEGER` 度量(B1 PR4);不支持 vision 的模型走 ImageRef 占位降级
- **D2 跨 session 全文搜索**(2026-08-17):`messages_fts` FTS5 虚拟表(`db/migrations/schema.rs:1051`,external-content + trigram + `UPDATE OF text` 防写放大 + `messages_fts_docsize` 影子表守卫回填)+ `db/search.rs` 双路分派(FTS 命中走 rowid → `messages` 主表;0 命中回退 LIKE 兜底)+ `search_messages` POST IPC + 前端 `SearchModal` 两态(空态/命中态按 session 分组)+ Cmd/Ctrl+K 接管
- **F1 消息队列·用户连发档**(2026-08-25):流式期间编辑器解锁,发送统一入队(后端 per-session 内存队列 `agent/message_queue.rs`,FIFO/uuid 寻址/上限 20),turn 边界驱动器 drain 全队批量注入;`ChatEvent::TurnContinuation` 续轮渲染边界;单条撤销/退回/水合(详见 [ARCHITECTURE §1.6](./ARCHITECTURE.md))
- **F4 `web_search` 工具**(2026-08-25):与 `web_fetch` 两段式分工(Tavily keyed / DDG 兜底),固定端点无 SSRF 面,Tier 5 silent Allow;Settings 第 7 tab 配 key(AEAD 加密)
- **F5 PDF/docx/xlsx 原生文本提取**(2026-08-26):`agent/doc_extract.rs` 纯函数提取(pdf-extract / quick-xml / calamine,零 Node 运行时零 pdfium),@文件在 Degraded 兜底前分流,成功走 `<doc>` span 注入通道 + `at_files_token` 度量;占位文案升级为指令式自助兜底(长尾格式 agent 自行转换);pptx 用户裁定不做
- **F6 异步 agent 任务 + F3 全局并发闸**(2026-08-27):`SessionSummary.busy` 运行时 enrich(跨端侧栏红点)+ 轮次终结跨 session toast + `max_concurrent_loops` 全局信号量(缺省 4)+ Tauri 壳关闭确认;零新表零 migration(详见 [ARCHITECTURE §1.6](./ARCHITECTURE.md))
- **F2·F2b 定时任务**(2026-08-28):daemon 常驻调度器(`scheduler/` 30s tick,单一扫描算法 + `due` 落账防相位漂移 + catch-up)+ origin 载体链走 F1 队列入口;preset 6 档 + `max_runs`/`ends_at` 结束条件 + `completed` 审计;Settings 第 8 tab 管理面,PWA 可用(详见 [ARCHITECTURE §1.6](./ARCHITECTURE.md) + [backend/scheduled-tasks.md](../.trellis/spec/backend/scheduled-tasks.md))
- **stream 事件补 session_id**(2026-08-27):`chat-event` payload 回填 `session_id`,支持跨客户端(remote PWA)按 session 认领

**未做**(排期归 [ROADMAP.md §2](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排) 第四档,技术评估见 [BACKLOG.md](./BACKLOG.md)):

- 触达层:`B10` 飞书 IM(消息收发;B10 曾预期「触发 daemon 化」,实际 daemon 化由远程访问需求先行落地,2026-07 完成,见 [decisions-2026-07.md](./IMPLEMENTATION/decisions-2026-07.md))/ ~~`B11` 远程遥控通道~~ **✅ 2026-08-11~13 已实施**(remote-control epic S1~S6b,合并 `94828cb`):中继方案变更为国内 2C2G 服务器 + 自研 Rust remote daemon,PC daemon 权威 + 云端仅中继;详见 [ROADMAP §1.2](./ROADMAP.md))
- 安全:`A2+ P3` shell 执行期沙盒兜底(bubblewrap/overlayfs/firejail,前置 WSL userns spike;详见 [A2-SHELL-CLASSIFICATION.md](./_history/2026-08-28-a2-shell-classification.md) §4)

> **2026-07-10 同步**:本节此前列出 B2 / B3 / B4 / B5 / B6 / B9 / C2 等均已落地,迁移至"已具备"列表上方;`DAG workflow(B8)` 07-10 完整落地移至上文。剩余 2 项 + A2+ P3 归 ROADMAP §2 第四档。
> **2026-08-13 同步**:`B11` 远程遥控通道已由 remote-control epic(S1~S6b)实施,从"未做"移除。

### 3.2 明确不做(硬约束)

> 硬约束 ≠ 排期相关。**这些是项目长期原则,不会因为 V2 / V3 路线图调整而松动**。路线图只动"做什么 + 什么时候做",不动"什么不做"。

**核心不做**(项目根基):
- ❌ **不包装 Claude Code SDK / Codex SDK** — 违背学习目标(详见 [IMPLEMENTATION.md §1](./IMPLEMENTATION.md#1-决策自己写-agent-core不用-sdk-包装))
- ❌ **不做通用 agent 框架** — Cline / OpenHands 已经在做
- ❌ **不做 Windows 端优化** — WSL 跑得好就行(详见下文 §4 WSL 优先)
- ❌ **不做云端部署 agent core** — 本地优先,agent 进程不出本机。**例外(2026-08 起)**:云端只跑轻量中继 `everlasting-remote`(remote daemon,不持文件、不存 agent 数据,仅转发;见 [REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md))
- ❌ **不做原生移动 App** — 桌面为主;**移动端以 PWA 形态提供**(2026-08 起,配对 + 节点 + 远程查看/操作通道,见 [REMOTE-ACCESS-E2E.md](./REMOTE-ACCESS-E2E.md))

**范围守护**(避免后期蔓延):
- ❌ **不做 Yolo 模式默认开** — Yolo(无任何确认)必须显式开启,默认拒绝(详见 [权限层 spec](../.trellis/spec/backend/permission-layer.md))
- ❌ **不做云端触发器** — 定时/事件触发源必须在本地(系统时间、fs 事件、本地 webhook);Cloudflare Cron Trigger 之类不接(用户经 PWA **主动发起**的远程会话是例外,见下条)。**本地定时已实现(F2,2026-08-28)**:daemon 常驻调度器按本机时钟触发,云端不参与(用户经 PWA 主动发起的是例外,见下条)
- ❌ **不做 in-app 自动升级** — 新版本走包管理器或手动下二进制,降低供应链攻击面和复杂度
- ❌ **不做云端自动推送任务回写本机** — agent 不接受"云端自动/定时/事件推下来"的任务,主动权必须在本地用户;**排除的是自动推送**,用户经 PWA 主动发起的远程会话是已实施例外(remote-control epic,2026-08)

**关于"云端"语义**:
> - **云端部署**:把 agent 跑在云服务器上 ❌ 不做(agent core 仍在 PC daemon)
> - **云端同步**:用云服务做"状态镜像 / 远程遥控通道" — 远程遥控通道 **✅ 已实施(2026-08)**:`everlasting-remote` 中继 + WSS 隧道 + PWA 配对,实时转发不落盘;"状态镜像 / 数据同步"仍未做(各 PC 数据隔离,不做跨节点同步)
>
> 这两项**不矛盾**,前者是"agent 跑哪",后者是"数据镜像到哪"。已实施部分见 [REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md) + [REMOTE-ACCESS-E2E.md](./REMOTE-ACCESS-E2E.md),剩余跨设备同步候选见 [BACKLOG §4 跨设备](./BACKLOG.md#4-跨设备)。

**V2 重排后新增的"不做"**(2026-06-10 决策):
- ❌ **不做 xterm.js 嵌入式终端** — `shell` tool + 30K 落盘已覆盖"看 agent 在跑啥"的需求
- ❌ **不做 MCP 暴露** — 个人工具,工具集对外开放杠杆不足
- ❌ **不做 Provider 限流(令牌桶)** — 个人使用场景未撞到限流,后期按需再评估

> 完整"移除"决策矩阵见 [IMPLEMENTATION §4 决策日志 2026-06-10 条](./IMPLEMENTATION/decisions-2026-06.md) + [ROADMAP §3 移除项](./ROADMAP.md#3-移除项--已废弃v2-重排2026-06-10-决定)。

---

## 4. 决策:WSL 优先,Windows 次要

**WSL 优先的具体含义**:
- Tauri 编译运行在 WSL 内部,通过 WSLg / Wayland 显示到 Windows 桌面
- 项目文件存放在 WSL 内部(`~/projects`),不走 `/mnt/c`
- 所有 git / shell / fs 操作都在 WSL 内完成
- 不考虑纯 Windows 路径作为项目主路径

**为什么要这样**:
- WSL 内部的文件系统性能远高于 `/mnt/c`
- 几乎所有 dev 工具在 Linux 上行为最标准
- Git 在 WSL 里跑比 Windows 快很多
- 沙箱在 Linux 内核支持更全(landlock / namespace),虽然在 WSL2 内有用户态限制(详见 [§5.1 风险表](#51-技术风险))

**用户视角**:
- 在 Windows 桌面看到一个 Tauri 窗口
- 背后是 WSL 里的 Linux 进程
- 像用 WSL Terminal 一样,只是多了个 GUI

---

## 5. 风险与权衡

### 5.1 技术风险

| 风险                          | 严重度 | 缓解                                          |
|-------------------------------|--------|-----------------------------------------------|
| Rig 0.x breaking change       | ✅ **已无** | rig-core 已弃用(2026-06-09),改自研 Provider trait;rig 升级不再适用 |
| Tauri 2 在 WSLg 下的 bug       | 低(✅ spike-001 已验证可用) | 准备 fallback 到 WSL 内部启动 + VNC/X11 转发  |
| Git2-rs worktree API 不全      | 中     | 必要时 spawn `git worktree` 命令              |
| Linux sandbox (bwrap/landlock) | 高     | WSL2 默认禁 user namespace,bwrap 实际不可用;退路:landlock(内核 5.13+,需 WSL2 内核版本对齐)/ firejail / 应用层黑名单(rm -rf /、curl \| sh 之类)。这是 [⑨ Tool 权限](./ARCHITECTURE.md#⑨-tool-权限检查) 实施的前提 |
| LLM 流式 token 断连            | 低 (✅ A5+ 07-05 落地) | ✅ **首字节前重试**(Full Jitter + retry-after advisory + 双向熔断 max_retries×budget)。SSE 协议无 resumption(research §5.4),"断点续传用 message ID"退路不可行,改走整请求重发的安全边界 — tool 执行在 stream 完成后,首字节前重发 = 零 tool 副作用,不需幂等 key。spec 见 [llm-contract A5+](../.trellis/spec/backend/llm-contract.md),决策见 [IMPLEMENTATION §4 2026-07-05](./IMPLEMENTATION/decisions-2026-07.md) |
| 上下文爆炸                    | 高     | ✅ **C3+ LLM 摘要式压缩**(2026-08-18,替代 C3 MVP 0.80→0.50)+ 保留区存活(`clamp(15k, 10%窗, 25k)`)+ `cutoff_seq` 水位折叠 + 消息裁剪 + tool result 截断 |
| 循环检测(agent 死循环)        | 高     | ✅ C2 分级触发 — L1 精确签名硬触发 N=3 + L2 Jaccard 软提示 N=5/0.85;软提示注入 hint 不打断,撞线走 MAX_TURNS 软卡询问(2026-08-19,见 [ARCHITECTURE §2.5.15](./ARCHITECTURE.md)) |

### 5.2 工程权衡

**复杂度 vs 学习价值**(历史决策,2026-06-04 起 + 2026-06-09 rig-core 弃用):
- 选 rig:省掉 50% 样板代码,但少学 50% harness 细节
- 选 reqwest:多学 50%,但每个字节都懂
- **决策**:前两步手写学(步骤 1-2);rig-core 评估后于 2026-06-09 弃用(0.38.1 阶段),改自研 `Provider` trait 走 Anthropic / OpenAI 双 Provider(详见 [TECH §2](./TECH.md#2-决策rig-core-弃用2026-06-09改自研-provider-trait) + [IMPLEMENTATION §4 决策日志 2026-06-09](./IMPLEMENTATION/decisions-2026-06.md))

**功能范围 vs 完成度**:
- MVP 8 项都做,每项做到 70 分,胜过做 15 项每项 40 分
- 任何一项发现比预期难,砍掉,不要延期

**本地 vs 云端**:
- Agent 进程 100% 本地,数据不出本机(remote 中继流经不落盘,只存 token/节点/配对码)
- 不考虑自建 LLM 网关(多设备远程已落地但仍是单用户直连各 PC 的 LLM 配置,网关仍未做)
- 远程遥控 = **实时隧道通道**(2026-08 已实施),非"push 摘要"式同步(旧设想见 [BACKLOG.md §4](./BACKLOG.md#4-跨设备))

### 5.3 不可控因素

- **Anthropic / OpenAI 协议变化** — 只能靠 adapter 层隔离
- **Tauri 2 生态成熟度** — 还在快速演进,有些库还在追
- **WSL 跨版本差异** — WSL2 行为稳定,但偶尔有 kernel upgrade 引起的边角问题

---

## 附录 A:常用命令速查

```bash
# 开发
cd app && pnpm tauri dev         # 启动 Tauri 开发模式
cd app && pnpm tauri build       # 打包生产版本

# 数据库
sqlite3 ~/.local/share/dev.everlasting.app/everlasting.db ".tables"

# 调试
RUST_LOG=debug pnpm tauri dev    # 在 app/ 目录下

# WSL 内查看 Tauri 进程
ps aux | grep everlasting
```

---

> 本文档随项目演进更新。任何重大架构变更,先改这里,再改代码。
