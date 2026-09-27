# DeepSeek Harness(dsh)对比调研

> 调研日期:2026-09-27
> 对象:[deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)(命令 `dsh`,dev preview,MIT,237k stars)
> 目标:对比 dsh 与 everlasting 的架构与能力面,判断哪些设计对本项目有帮助、哪些不应跟进。
> 方法:一手抓取仓库文档(architecture / agent-lifecycle / tool-execution-pipeline / capability-seams / session-format-status / README / BENCHMARK,中英对照版)+ packages 与 subsystems 目录清单,对照本仓库 `docs/ARCHITECTURE.md` / `ROADMAP.md` / `BACKLOG.md` 现状。
> 结论一句话:**帮助在「语义与纪律层」而非架构——四个 agent-loop 语义 + session 格式版本纪律可低成本吸收进 spec;LSP 工具与会话 fork 记 BACKLOG;单体 Rust daemon 路线不因它调整。**

---

## 0. TL;DR

1. **dsh 定位 = TypeScript/Node 的「一切皆插件」agent 框架**(基于 Cordis,arXiv:2608.25512),五种运行形态(Web UI :3080 / Electron 桌面 / headless / Python SDK / ACP),53 个 packages,连 agent loop、session log、LLM 适配器本身都是可运行时替换的插件。**与 everlasting 同构的部分**:daemon + 同源服务 Web SPA、远程浏览器共用同一 agent core、Web/桌面双形态。

2. **架构差异本质**:everlasting 是「本地优先的单体 Rust daemon + 瘦客户端」(编译期工具注册 + SQLite 行存);dsh 是「插件运行时平台」(append-only JSONL 事件溯源 + seam 插拔)。两者是不同生存策略,dsh 靠生态扩张,everlasting 靠单体的简单可测。

3. **最值得吸收的四件事(§3.1,低成本,落 spec 即可)**:① 步内重试复用装配(不重跑 pre-step/prompt);② 装配期取消原子性("commits neither system nor users");③ 压缩恢复防死循环(剪枝/摘要推进了「表面替换世代」才允许重试,否则维持原错误——正对本项目 C3+ 压缩 + 关卡⑤硬卡的组合场景);④ 工具 post-execute 钩子(可 accept/block/replace/addContext)。

4. **session 格式版本纪律(§3.2)可补进 `database-guidelines` spec**:代码常量单一真源(SESSION_FORMAT_VERSION)+ finalized/released 双记录分离 + 只允许相邻版本迁移链 + 无凭据 spec 测试校验记录结构。

5. **能力面上 dsh 有、本项目没有的(§3.5,记 BACKLOG 候选)**:LSP 工具(lsp-stdio,最大实 gap)、PTY 终端一等工具、browser-use/computer-use 一等工具、会话 fork(带种子从任意点分叉)、Claude Code/Codex hooks 桥(子 agent 宿主)。

6. **不建议跟进的(§4)**:整体插件化(Cordis)——对单体是过度工程;append-only JSONL 换 SQLite——丢 FTS5/turn_trace/事务优势,没有理由;BENCHMARK.md 本身很薄,对 N9 性能基准参考价值有限。dsh 处于 dev preview、预期破坏性变更,**只宜借鉴设计,不宜引入依赖**。

---

## 1. dsh 架构速览

来源:`docs/architecture.zh.md`、`docs/agent-lifecycle.zh.md`、`docs/tool-execution-pipeline.zh.md`、`docs/capability-seams.zh.md`。

### 1.1 Profile / Bundle 分层装配

- 运行实例 = 有序插件树:`dsh-base` 共享层(适配器/工具/持久化/沙盒/approval/settings/credentials/telemetry)+ 各 surface bundle(web/headless/sdk/sdk-minimal/acp)+ 用户 `cordis.patch.yml` 叠加。
- patch 按层序应用,任何一层 bundle 插入的东西都能被上层 patch 整体替换;`sdk-minimal` 是刻意全显式配置树的例外。

### 1.2 事件溯源 session log

- 版本化 JSONL(`session.vN.jsonl[.zstd]`),世代化写入:**已提交路径永不改名/替换**,迁移是编译期静态的相邻 `vN → vN+1` 链,写方独占发布下一代。
- 核心不变量「**模型可见即已记录**」:模型请求必须能从 log 完整重建;任何新的模型可见输入必须先有 session 事件。`deriveMessages()` 从 log 投影模型历史;fork / resume / transcript / telemetry 全部从持久事实派生,live UI 来自流事件。
- 失败/取消的尝试记 `assistant/attempt`(不加历史);成功调用记 `assistant/message`(内嵌精确 compact 时间戳流,含空内容与 max-tokens 终止)。

### 1.3 Capability Seams(接缝)

- 每个服务是 `ctx.*` 键:`ctx.llm`(llm-deepseek/llm-pi-ai/llm-replay)、`ctx.shell`(bash-local/bash-sandbox/pwsh-local)、`ctx.subprocess`(local/ssh)、`ctx.fs`(local/sandbox/ssh)、`ctx.sandbox`(local/ssh)、`ctx.storage`、`ctx.web`、`ctx.mcpResources` 等。
- consumer 只依赖 seam 不依赖具体后端:**把 `sandbox-local` 换成 `sandbox-ssh`,Bash/PTY/LSP 整体搬去远程,consumer 零改动**。

### 1.4 Agent turn/step 生命周期

- step = 一次模型请求 + 其 tool calls;turn = 零或多个 step。
- `agent/pre-step` waterfall 决定准入输入(可改写/拒绝)→ prompt 装配 → `agent/request` 解析路由(在提交 system prompt 与 user 消息**之前**)→ 从 log 派生冻结请求 → `llm/stream` 流式 → tool calls 走 `tools/pre-execute → execute → post-execute` → `step/end`。
- **重试语义**:步内重试复用同一渲染装配,不重复 pre-step 与 user 准入。
- **取消语义**:prepare 或 stream 阶段取消 → "commits neither system nor users"(部分准入不落盘)。
- **压缩**(`dsh-compaction-basic`):`agent/pre-step` 阶段处理压力;先剪 tool result,再摘要;恢复留在仍打开的 step 内,**只有剪枝/摘要确实推进了「表面替换世代」才重试**,否则维持原请求错误。
- 转向(steering)/注入上下文走同一 waterfall(排队输入在 turn 收尾时被 Driver 认领进新 step)。

### 1.5 工具执行管线

- 注册:`ToolDefinition` 带 `projectContent`(暂存"执行预备文本与图片")与 `finalizeContent`(与快照一起定稿的"末态 content 不变量")。
- 流程:`tools/pre-execute`(钩子/权限/沙盒,可 allow/deny/ask)→ ask 走 `ctx.approval` 单发询问(拒绝/取消/不可用/无人应答都 = deny)→ **单调 guard 串**(不可重排,owner 策略注册为 guard)→ `tools/execute` waterfall(超时/重试/指标包裹 dispatch)→ `tools/post-execute`(**可 accept/block/replace/加附加上下文**)→ 注册表做无损外层归一化(快照抛错也归一为 isError)→ `finalizeContent` → `tools/result` 同步通知冻结结果 → 单条模型可见 `tool/result` session 事件。
- 文件变更过 `fs/write-intent` / `fs/edit-intent` 门(read-before-edit 在其下);denial 跳过工具体但**仍流入 projectContent**(下游可见拒因)。
- 批内全部结果落定后,"Active-batch additionalContexts FIFO" 在已记录 tool results 之后注入 user/message 条目(循环检测类钩子的注入通道)。

### 1.6 子系统面(packages 53 个,节选)

MCP、LSP(lsp-stdio)、terminal(PTY)、browser-use、computer-use、schedule、webhook、compaction、plan、todo、skill、hooks-claude-code / hooks-codex(把 Claude Code/Codex 当子 agent 宿主)、experimental 的 Agent Teams(常驻花名册 + 任务板 + 邮箱)。

---

## 2. 逐维度对比

| 维度 | Everlasting | dsh |
|------|-------------|-----|
| 技术栈 | Rust(axum daemon + Tauri)+ Vue 3,单体编译 | TypeScript/Node + Cordis 插件运行时 |
| 运行形态 | Tauri GUI / 浏览器(daemon 同源 SPA)/ 远程 PWA(云中继 + WSS 隧道)/ `evl` CLI | Web(:3080)/ Electron / headless / Python SDK / ACP |
| Agent loop | 自研 Rust loop,16 关卡请求生命周期,28 builtin tool 编译期注册 | 插件化 `ctx.agentLoop`,waterfall 中间件 |
| 持久化 | SQLite(WAL + FTS5 + turn_trace),消息行存 | append-only JSONL 事件日志 + zstd + 世代化写入 + 相邻迁移链 |
| 扩展模型 | 编译进二进制 + JSON workflow + skills/commands 文件 + tools stub 注册 | 一切皆插件(工具/LLM 适配器/shell 后端/沙盒全部运行时可换) |
| 权限/沙盒 | 5-tier 路径决策 + 3 档 Mode + Landlock/seccomp + prefix-grant | approval 单发 + 单调 guard + `ctx.sandbox` 接缝(local/ssh) |
| 压缩 | C3+ LLM 摘要压缩 + unified-context-budget 关卡⑤硬卡 + MAX_TURNS 软卡 + 手动 /compact | compaction 插件:先剪 tool result 再摘要,恢复留在同一步内 |
| 多 agent | group_chat 跨模型审议 + dispatch_subagent(worktree 隔离)+ workflow 引擎 | subagents + 实验性 Agent Teams |
| 取消 | C1 中途可取消(tool 执行中途可停) | 装配/流阶段取消不提交任何准入 |
| dsh 有而本项目没有 | — | LSP、PTY 终端工具、browser-use/computer-use 一等工具、Claude Code/Codex hooks 桥、会话 fork |

同构点:两者都走 daemon + 同源服务 Web SPA 的路子,连「远程浏览器共用同一 agent core」的目标都一致——印证 everlasting daemon 化方向与业界一致。

---

## 3. 对本项目有帮助的点(按价值排序)

### 3.1 工具循环的四个语义细节(低成本、直接进 `agent-loop-architecture` spec)

1. **重试复用装配**:步内重试不再重跑 pre-step/prompt 装配,只重新 prepare + 派生请求。省 token 且避免副作用重复。(对照 A5+ `send_with_retry`:A5+ 重试在 provider 层整轮重发,粒度不同——dsh 的启发是"装配产物在步内可冻结复用"。)
2. **取消原子性**:「prepare 或 stream 阶段取消,system 与 user 都不提交」——不留半装配状态。对照 C1:本项目中途可停,但「装配期取消不落任何事实」这个不变量值得对照检查。
3. **压缩恢复防死循环**:上下文溢出后,只有剪枝/摘要确实推进了「表面替换世代」才允许重试,否则维持原错误。**正对 C3+ 压缩 + 关卡⑤ 0.95 硬卡的组合场景**(压缩完仍超预算时怎么办),现成的收口语义。
4. **post-execute 钩子**:工具结果落地前的最后一道可观测/可改写横切面(可 accept/block/replace/addContext)——比"结果直接进消息"多一个注入点(泄漏脱敏、循环检测注入 C2+ 都可挂这里)。本项目的 16 关卡里没有等价物。

### 3.2 session 格式版本纪律(补进 `database-guidelines` spec)

- 单一真源 = 代码常量(`SESSION_FORMAT_VERSION` 在 `packages/core/session/src/types.ts`);包版本/导出名/fixture 名/投影缓存版本**明确非权威**。
- **finalized 与 released 双记录分离**:开发分支定稿(latestFinalizedVersion=4,不可变 checkpoint `finalized/v4.json`)≠ 已随版本发布给用户(latestReleasedVersion=3,证据 tag 记录写入路径)。alpha/beta/RC 也算建立发布义务。
- 只允许相邻(连续整数)迁移链;破坏性变更必须抬 writer 版本且**不能复用已接受的 3→4 转换**。
- 无凭据 spec 测试(`scripts/doc-standard.spec.ts`)校验记录结构/双语一致/证据 tag/文档 released ≤ workspace writer。

对照本项目:SQLite schema 迁移目前靠 spec 约束,"定稿/发布分离 + 代码常量单源 + 相邻链"的纪律可以补强。

### 3.3 「模型可见即已记录」不变量 + 投影

所有模型请求可从持久日志重建,UI 是投影。本项目 SQLite 落库虽全,但这个不变量表述干净——将来做 turn replay / 审计回放(N8 混沌冒烟、E2 TracePanel 扩展)时以此为验收标准。

### 3.4 沙盒 seam 化(远程执行形态的预留)

BACKLOG 余留是 bwrap + 网络白名单。dsh 把 sandbox/subprocess 做成可替换接缝(`sandbox-ssh` 一换,Bash/PTY/LSP 全走远程)的**切分方式**值得参考——即使现在不做,在 Rust 侧保持一个薄的 `SandboxExecutor` trait 边界(现有 `sandbox-executor` spec 已接近),未来远程执行形态就不用重构。

### 3.5 能力差距提示(记 BACKLOG 候选)

- **LSP 工具**(lsp-stdio):dsh 有而本项目没有的最大实 gap,对 coding agent 的代码导航/引用查找质量有实际影响。候选优先级较高。
- **会话 fork**(从任意点带种子分叉):对已有 edit_user_message(D3)+ handoff 是自然延伸。
- **hooks 桥**(Claude Code/Codex 作子 agent 宿主):生态位思路,与 dispatch_subagent 的自定义 worker 路线互补,优先级低。

---

## 4. 不建议跟进的

| 项 | 理由 |
|----|------|
| 整体插件化(Cordis) | 框架产品的生存策略(靠生态);对单体是过度工程。Rust 单体 + 116 REST 镜像 + 编译期工具注册更简单可测,现有路线不动摇 |
| append-only JSONL 换 SQLite | 丢 FTS5(D2 跨 session 搜索)/ turn_trace / 事务能力,没有理由迁移;只借鉴其版本纪律 |
| BENCHMARK.md 评测方法 | 本身很薄(只指向 jsonrpc-agent 跑法 + "独立 workspace/session"建议),对 N9(cargo criterion + vitest bench)参考价值有限 |

另:dev preview、预期破坏性变更——**只宜借鉴设计,不宜引入依赖**(它是 TS 生态,技术上也无法直接复用)。

---

## 5. 来源

- 仓库:<https://github.com/deepseek-ai/deepseek-harness>(默认分支 `master`;文档站 <https://deepseek-harness.github.io/deepseek-harness/> 为 JS 渲染,直接抓 raw md)
- 架构:<https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/architecture.zh.md>
- 生命周期:<https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/agent-lifecycle.zh.md>
- 工具管线:<https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/tool-execution-pipeline.zh.md>
- 接缝:<https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/capability-seams.zh.md>
- session 格式:<https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/session-format-status.zh.md>
- Cordis:<https://github.com/cordiverse/cordis>(论文 arXiv:2608.25512)
