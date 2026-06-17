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

### 🟠 第三档 — 缓做(9 项)

| 编号 | 功能 | 备注 |
|------|------|------|
| B6   | Subagent(main agent 派 worker agent,独立 context,summary 回填) | **harness 学习价值高**,依赖 B5 Memory |
| B4   | Skill 系统 | 指令层扩展 |
| B9   | 生成式 UI(4 primitives — button / selector / diff / code_block) | 输出层扩展 |
| C2   | 循环检测 | ⑬ 关卡实现 |
| C6   | 大输出截断统一 | ⑩ ⑫ 边界处统一处理 |
| B1   | 图片支持(multimodal) | 输入层扩展 |
| D2   | 跨 session 全文搜索(**双驱动** 路径) | ① **用户驱动**(MVP,1 PR):UI Modal + `Cmd/Ctrl+K` 触发 + 跳到原 message ② **Agent 驱动**(增量,1 PR):`search_history` tool,LLM 决策时调;**共享 `search_messages` Tauri command**;**实施顺序:先①后②**,可只 ship ①;**降档理由(2026-06-17)**:session 积累尚浅 + B5/C3 已覆盖"当次 memory"层,价值随 session 基数增长。详见 [IMPLEMENTATION §4 2026-06-17](./IMPLEMENTATION.md#4-决策日志) |
| A5/A6 | 错误处理完善 + README + demo | 打磨 |
| ~~A7~~ | ~~RDP 双屏 position bug 修复~~ | ✅ 06-14 落地(根因 Wayland setPosition 限制),见 §1.2 |

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
