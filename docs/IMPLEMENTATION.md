# IMPLEMENTATION — 实现讲解

> Everlasting 的"自研决策 + 决策日志"。**本文件是决策档案**,不列路线图(路线图见 [ROADMAP.md](./ROADMAP.md))。
> 需求见 [DESIGN.md](./DESIGN.md),架构见 [ARCHITECTURE.md](./ARCHITECTURE.md),技术选型见 [TECH.md](./TECH.md),路线图见 [ROADMAP.md](./ROADMAP.md)),候选功能见 [BACKLOG.md](./BACKLOG.md)。

---

## 1. 决策:自己写 agent core,不用 SDK 包装

**背景**:Anthropic 2025-2026 年出了官方 Agent SDK(`claude-agent-sdk-python` / `-typescript`),用 `query()` 直接拿结构化消息流。OpenAI Codex CLI 是 Rust 写的(Apache 2.0)但没官方 SDK。

**为什么不用**:
1. **学习目标要求自研** — 用了 SDK 只学到"怎么调 SDK",学不到 harness 核心
2. **控制粒度** — SDK 帮你做了"消息流 → tool 调用 → 回填"的循环,你想插自定义逻辑(权限、审计、统计)就被抽象挡住了
3. **解耦厂商** — 一旦 SDK 协议变化,业务逻辑全挂

**什么时候用 SDK 合适**:赶时间、要快速出活、不在乎学习价值。本项目两个都不符合。

**自研的边界**:
- ✅ 自己写:Agent Loop、消息管理、tool 注册、流式解析、权限检查
- ✅ 自己写:Tauri IPC 事件协议、session 持久化、worktree 管理
- ❌ 不自己写:LLM HTTP 协议(用 rig)、SSE 解析(用 rig)、MCP 协议(用 rmcp)
- ❌ 不自己写:GUI 框架(Tauri 已有)、Diff 算法(用前端库)

---

## 4. 决策日志

> 按时间倒序记录。每次重大决策都加一条,包含"为什么"。**本节只追加不删除**(ADR 性质的不可再生历史档案)。

### 2026-06-17 — D2(SQLite FTS5 全局搜索)从第二档降档到第三档 + 标注双驱动路径

**Context**: 2026-06-17 user review D2 设计后明确延后,触发两点判断:
- **痛点不足**:用户当前 session 积累尚浅(数十量级),B5 Memory 指令系统(写)+ C3 Context 压缩(当次 session 内管理)已覆盖"当次 memory"层;D2 价值 = "跨 session 找回过去对话",依赖 session 基数到一定量(类比 Notion / VSCode 全文搜索"用了就回不去"前提是积累)
- **双驱动价值大于单驱动**:用户 modal(`Cmd/Ctrl+K`)+ agent tool(`search_history` LLM 决策时调)共享同一个 `search_messages` Tauri command,实现成本几乎不翻倍,但**让 LLM 主动挖过去对话**比"用户自己搜再问 agent"更接近 Claude Code 那种"agent 知道过去"的体验

**Decision**: D2 从 ROADMAP §2 第二档移到 §2 第三档(缓做),描述补"双驱动路径 + 实施顺序",**保留后端 `search_messages` Tauri command 形态预想**(不破坏未来增量实施路径,未来可单独按双驱动范围重新评估排期)。**D3(session 内消息编辑/重发)按原排期仍留第二档**——DEBT 收尾建议第 3 条"D3 自然碰 A-007(error 路径 partial text)/ A-010(二次取消语义),应最后做",降档会跟该建议冲突,本批次没动这两条债,降档理由不充分。

**双驱动路径**(作为 D2 未来实施时的预设范围,降低"开新 task 时重新讨论"成本):
- **① 用户驱动**(MVP,~150-200 行,1 PR 闭环):后端 `messages_fts` 虚拟表(FTS5,unicode61)+ `search_messages(query, project_id?, limit) -> Vec<MessageSearchHit>` Tauri command(query 长度硬卡防 LIKE abuse);前端顶栏搜索图标 + `Cmd/Ctrl+K` 触发 Modal,结果列表显示 `session_title` + `project_name` + 高亮 hit 片段 + 点开跳到 message 并 scroll into view
- **② Agent 驱动**(增量,~100 行,1 PR 闭环):`search_history(query)` tool 接 ② 既有 tool 注册表,LLM description 写清"用过去 session 的 message 内容回答用户关于'上次怎么 X'的问题";**共享** `search_messages` Tauri command;permission Tier 1 只读白名单;`allowed-tools` 默认不挂(LLM 看不到自动不可调,需用户显式开,防滥用)
- **实施顺序**:先 ① 后 ②;**① 可独立 ship**,② 是 ① 的复用增量,非并行(避免 ② 提前 share 搜索基础设施时 ① UI 还没定)
- **CJK 鲁棒性**:① 第一版用 `unicode61` tokenize + 前端分词提示;jieba-rs / trigram 等 CJK 优化放 D2-v2(不在 D2 范围)

**反向触发**(拉回第二档的条件):session 基数到 50+ / 用户多次表达"想找回过去对话" / B5/C3 层出现"需跨 session 引用"的需求。

**Alternatives**(已否决):
- **降到第四档(最远远期)**:跟 daemon 化(B10 飞书)/ 云端同步(B11)等同档,但 D2 实现复杂度极低(DB 索引 + Tauri command + UI,~200-300 行),不与 daemon 化同等量级,第三档更准
- **保持第二档不动**:用户明确延后,违背 user 决定
- **D2 + D3 一起降档**:D3 是 V2 第二档里仅剩的 1 项,降档会让第二档提前"伪清零";且 DEBT 建议 D3 与 A-007/A-010 同步,本批次没动这两条债,D3 降档理由不充分
- **只做用户驱动(砍掉双驱动设计)**:用户体验上"用户搜 + 喂给 agent"比"agent 自动搜"多一轮交互,落差明显;双驱动后端零增量、前端仅多 1 tool 注册 + permission 配置,几乎零成本

**影响面**: 仅 ROADMAP §2 表格行移动 + 描述补充;无代码 / spec / test 变更。DEBT.md §"按 ROADMAP 里程碑的收尾契机"B2/B3/D2 行同步修正为"B2/B3(D2 暂缓)"。**未开新 task**——按 user 决定延后,等触发条件出现时再 `trellis-brainstorm` 起新 task。

### 2026-06-16 — 审批内联到 ToolCallCard + 按 session 分区 + 拒绝并反馈(取代全局 PermissionModal)

**Context**: 全局单例 `<PermissionModal>`(挂 ChatPanel,Teleport to body,状态为 `usePermissionsStore.pendingPermission` 单槽 ref)在多 session 并发审批时三连问题:① `setPending` 直接覆盖旧 pending 且不对旧 rid respond,旧 ask 留在后端 oneshot store 跑满 120s 超时 → `Decision::Deny`,该 session agent loop 卡 120s(用户感知"没问我就处理了",实际是超时拒);② payload `PermissionAskPayload` 不带 sessionId,modal 文案写死"当前项目"、path badge 用 `chatStore.currentCwd`(用户当前看的 session),跨 session 时指鹿为马;③ `deny` reason 写死 `"user denied"`,LLM 不知为何被拒、无法纠错。

**Decision**: 审批 UI 从全局 modal 改为内联到 `ToolCallCard` 的「待审批」态,以 `tool_use_id` 为关联键:
- 后端 `PermissionAskPayload` 加 `session_id` + `tool_use_id`(agent loop 在 `for (id, name, input)` 里已持有 tool_use_id,`check()`/`ask_path()` 签名穿透即可);`PermissionResponse::Deny` 扩展带 `reason: String`;`permission_response` IPC 接收 `reason`。deny 反馈作为 `tool_result(is_error)` 内容回填 LLM。
- 前端 store `pendingPermission`(单槽) → `pendingBySession: Map<sessionId, ask>`,listener 按 sessionId 路由;每 ask 独立 120s 计时(按 rid,取代共享单 timerRid)。
- `ToolCallCard` 以 `call.id === pending.toolUseId` 渲染审批态(仅一次/始终允许/拒绝/拒绝并说明 4 操作,"拒绝并说明"展开输入框);`hasResult` 到来即视为审批窗口关闭(allow→exec / deny / cancel 都产生 result),清 pending + 隐藏审批 UI。
- **彻底移除** `<PermissionModal>`(组件 + ChatPanel 引用 + 测试)。
- SessionList 给有待审批的 session 加脉动 shield 标记(切走也能感知,后端 120s 超时语义不变)。
- 「拒绝并说明」用分离式输入框(主按钮「拒绝」一键 deny,「拒绝并说明」二级展开),符合 Claude Code 体感。

**关键不变量**:`tool:call`(chat_loop L423)必先于 `permission:ask`(ask_path)发出——前端收到审批事件时目标 ToolCallCard 已渲染,`toolUseId` 匹配成立;同 session 的 `check()` 串行 await(同 session 最多一个 pending),跨 session 才并发,正好匹配 per-session 分区。

**影响面**:后端 3 文件(`permissions/mod.rs` enum/payload/签名/分支 + `commands/permissions.rs` IPC + `chat_loop.rs` 传 tool_use_id);前端 `permissions.ts` store 重写 + `permissions.test.ts` + `ToolCallCard.vue`/`.test.ts` 审批态 + `SessionList.vue` badge,删 `PermissionModal.vue`/`.test.ts`,`ChatPanel.vue`/`ChatWindow.vue` 引用清理;spec `.trellis/spec/backend/tool-contract.md` §4 permission:ask IPC 同步。测试:后端 68 lib 测试全绿(含 sessionId/toolUseId camelCase + Deny reason 2 个新测试);前端 vitest 全绿(含 permissions 多 session 共存/respond 按 rid 精确清除 + ToolCallCard 审批态 8 测试)。

### 2026-06-14 — shell 权限三档分类(ReadOnly/SideEffect/Ask)+ plan 模式只读放行 + 复杂命令弹窗兜底

**Context**: A2+B7 re-grill(2026-06-13)把 Mode 检查提到 Tier 3 后,plan 模式对 shell 一刀切 Deny——连 `git diff`/`git status` 这种纯读命令也禁,且 Tier 3 提前 return 绕过 Tier 4 弹窗,用户无法当场放行(只能被迫切模式)。同时暴露 `ShellTrust` 两档(Allow/Ask)只看首 token 的粒度问题:`git log | bash` 被判 Allow(首 token git),而 Tier 2 只兜 `curl\|bash`,不兜 `git\|bash`——pipe/链里藏的副作用靠用户肉眼。用户进一步要求:像 `ENV=noop && cargo check` 这种代码判不了的命令,plan/edit 都要有弹窗放行可能,而非硬拒堵死。

**Decision**:把 shell 的 Mode 感知从 Tier 3 **下沉到 Tier 4 的 Shell 分支**,并把 `ShellTrust` 从 2 档拆成 3 档:
- `ReadOnly`(纯读:ls/cat/git diff/...)→ 任何模式静默 Allow(解决 plan 痛点)
- `SideEffect`(可恢复副作用:mkdir/git push/cargo)→ edit 静默 / plan 弹窗
- `Ask`(高危/未知/结构复杂)→ plan & edit 都弹窗(放行口子,不硬拒)

三处关键设计:
1. **git 子命令细化**:git diff/log/status/show/blame 等(只读子命令)→ ReadOnly;其余 git 子命令(push/commit/reset/config/branch/...)→ SideEffect(保守,宁误判写)。`git` 整体不再归一档——这是 plan 能放 `git diff` 而不放 `git push` 的关键。
2. **结构降级**:cmd 含 `|`(含 `||`)/`&&`/`;` → Ask。堵 `git log | bash` 误放行;也覆盖用户提的 `ENV=noop && cargo check`(代码判不了 → 弹窗给放行口子,而非硬拒)。代价:纯读 pipe(`git status \| head`)也弹窗,可点"始终允许"消音。
3. **shell "始终允许"接通**:Tier 4 Shell 分支新增 `check_prefix_grant`(match_kind='prefix' 精确匹配 first token)。修了 re-grill 留下的"`match_value_for_allow_always` 写了 prefix grant 但从不查"瑕疵。

**保留 re-grill 的初衷**:Tier 3 仍对 write_file/edit_file 硬拒(纯写工具无歧义,plan 语义=只读,弹窗会模糊 plan/edit 边界)——"用户点始终允许 → 仍被 Mode 拒"的鬼畜交互对这俩仍成立。shell 因异构(git diff 读 / git push 写)才下沉到 Tier 4 按三档细分。

**Alternatives**(已否决):
- **Tier 3 放行白名单 shell(方案 A)**:粒度仍不够(git 整体白名单 → git push 漏放),否决。
- **首 token 粗分两堆(不细化 git 子命令)**:`git diff` 放不出来(痛点未解),否决。
- **pipe 保留首 token 现状(只降级 &&/;)**:`git log \| bash` 仍误放 Allow,否决。

**影响面**:后端内聚,3 文件(`shell_trust.rs` 重写 + `mod.rs` Tier 3/4 + `db/permissions.rs` 注释);前端(`permissions.ts`/`PermissionModal.vue`)不动(只认 `permission:ask` payload,与 `ShellTrust` 枚举解耦)。452 lib 测试全绿(含 18 个 `classify_*` 新测试覆盖三档/git 子命令/结构降级)。代码 `app/src-tauri/src/agent/permissions/{shell_trust.rs,mod.rs}`;spec `.trellis/spec/backend/tool-contract.md` "Scenario: Path-based Permission" 同步更新。

### 2026-06-14 — TitleBar maximize 改用原生 toggleMaximize()(position bug 根因 = Wayland setPosition 限制)

**Context**: session 15(2026-06-07)留的 TODO(bug-position):RDP 双显示器下点最大化,窗口"grow rightward"而非贴 host 主屏左上角。跨多 session 试过 setSize/setPosition 顺序互换、cursorPosition 定位,均失败。06-14 重新排查定位根因。

**根因(关键)**:**不是 Tauri bug,是 Wayland 协议根本限制**。Wayland 安全模型禁止客户端设置窗口位置——position 由合成器决定。WSLg 用 Weston(Wayland 合成器),`setPosition()` 被合成器**静默忽略**。影响所有 toolkit(GTK/Qt/SDL/Tauri),Tauri issue #14913 / Weston / Qt 社区均证实。故"先 setPosition 移到 monitor 左上角再 setSize 铺满"在 WSLg 下**协议层面就不可能**——setPosition 被忽略,setSize 让窗口从原位向右下生长,正是报告现象。

**Decision**:放弃"应用控制位置 + 手动铺满整屏(含任务栏)"的非标准 maximize,全平台统一 `win.toggleMaximize()`:
- 合成器/WM 原生 maximize,position 由系统决定 → Wayland/X11/Win/macOS 位置都正确
- 自定义 title bar 保留(maximize ≠ fullscreen)
- toggle 语义自动还原到最大化前状态
- `isMaximized` 从"比对 outerSize vs monitor.size"启发式 → 直接 `win.isMaximized()`(权威)
- 代价:铺满 work area 而非整屏(WSLg/RDP 虚拟屏通常无任务栏,= 整屏);还原尊重用户拖动过的大小而非强制 1440×900(改善)

**Alternatives**(已否决):
- **手动 setSize+setPosition 铺满**:Wayland 下 setPosition 被忽略,失败(根因)
- **`setFullscreen(true)`**:位置对,但 fullscreen 隐藏自定义 title bar,看不见 min/max/close,只能 Esc 退出 → 体验不可接受
- **平台分流(WSLg→toggleMaximize,其他→手动)**:`platform()` 只返回 `linux`,无法精确区分 Wayland/X11,增加复杂度 + 误判
- **保持现状不修**:原生 toggleMaximize 几乎零代价且更标准,无理由不修

RDP 双屏验证通过(2026-06-14)。代码 `app/src/components/layout/TitleBar.vue`;A7 出第三档进 §1.2。

### 2026-06-13 — A2+B7 Re-grill: path-based 模型 + Tier 重排(Mode 提前)+ 3 match_kind 全 wire

**Context**: A2+B7 任务的 PR1 + PR2 + PR3 + 3 档化(2026-06-13)在 main 上跑了一天后,通过 re-grill-me session 重新审视权限判定 + Mode 联动的设计。发现两个反直觉 + 1 个粒度不足:
- **反直觉 #1**:旧设计 Tier 3 "总是弹窗" → Edit 模式读 README 都要弹(用户跑 coding 任务被弹 10+ 次)
- **反直觉 #2**:旧设计 Tier 4 Mode check 在 Tier 3 Ask 之后 → Plan + 写操作有"用户点始终允许,然后被 Mode 拒"的坏交互
- **粒度不足**:PRD 原预留 3 种 `match_kind` schema(`tool` / `prefix` / `path`)但只 wire 了 `tool` → 用户想"信任 ~/Documents 整片"没辙

re-grill 锁定 10 个核心决策,完整 PRD 参见 [`.trellis/tasks/06-13-a2-b7-regrill-path-based/prd.md`](../.trellis/tasks/06-13-a2-b7-regrill-path-based/prd.md)。旧 06-12 PRD 加 Superseded 标记保留作历史档案,新实施以新 PRD 为准。

**Decision** (10 项,re-grill session 输出):

1. **弹窗判定 = path-based**(Q1)— 仓库内 default allow,仓库外 ask,跟"build 跑 coding 任务"心智一致
2. **shell 策略 = 前缀白名单 + asklist + Tier 2 兜底**(Q2)— 静态 ~30 个白名单 + ~10 个 asklist,`bash` / `sudo` / `cd` 这种"容器"前缀永远 Ask
3. **仓库边界 = Session.cwd 严格 prefix 匹配**(Q3)— 跟现有 `boundary::assert_within_root` 复用,新增 `is_within_root(&self, path) -> bool` 抽出
4. **Yolo × 仓库外 = silent**(Q4)— Yolo bypass 整个 Tier 4 modal,跟 Yolo "no questions asked" 哲学一致;Tier 2 硬墙仍生效
5. **Tier 顺序 = Hooks → Deny → Mode → Path → Allow → Audit**(Q5)— Mode 提前到 Tier 3,消除 Plan + 始终允许坏交互
6. **"始终允许" 粒度 = tool + path-glob + prefix 3 种 match_kind 全 wire**(Q6)— schema 已有,只 wire;3-button modal 触发时按 tool 类型自动选 match_kind
7. **shell prefix 解析 = 第一个 token,无递归/无 alias/无 pipe**(Q7)— "B 试图精确会输"哲学一致,`find -delete` / `echo > /tmp/x` 副作用 Tier 2 兜底
8. **path-glob 持久化粒度 = 父目录 + `*` 通配(sqlite GLOB)**(Q8)— 用户允许 `src/foo.rs` → `src/*`;sqlite GLOB 不支持 `**` 递归,子目录要再次允许
9. **Plan × path policy = Plan 不豁免**(Q9)— 仓库外 read 在 Plan 模式仍 ask;跟新 Tier 顺序自然衍生
10. **Risk 字段保留 = 4 档作 UI 视觉,加 path 范围行**(Q10)— 零改动兼容,path + risk 是 orthogonal 维度

**Alternatives** (已 grill 否决):
- **B/Q1**: Risk-based 弹窗 — Edit 模式读文件要弹,反直觉
- **B/Q2**: 解析 shell 命令路径 token 判定仓库内/外 — pipe / env 变量 / `cd` 切换可绕过,试图精确会输
- **B/Q4**: Yolo 仓库外仍 ask — 跟 Yolo "no questions" 哲学矛盾
- **B/Q5**: 维持旧 Tier 顺序 — Plan + 始终允许坏交互保留
- **B/Q6**: 只 wire `tool` match_kind — path 工具想信任整目录没辙
- **B/Q7**: 递归解析 shell(`sudo X` → 跳到 X)— 跟"试图精确会输"哲学冲突
- **A/Q8**: 最小精确(只记 path 自身)— path 工具太严,同目录 10 文件弹 10 次
- **B/Q9**: Plan 豁免 path policy — 跟"仓库外一律 ask"模型冲突
- **C/Q10**: 废弃 risk 字段 — UI 改动大,跟现有 UX 偏离

**影响范围**:
- Backend 新模块:`projects/boundary.rs::is_within_root`(从 `assert_within_root` 抽出);`agent/permissions/shell_trust.rs` 新文件(~120 行,白名单 + asklist 2 张 const 表 + classify_prefix 函数)
- Backend 改:`agent/permissions/mod.rs::check` 大改(5 tier 重排,按 tool 类型分派 Tier 4,~200 行净增);`commands/permissions.rs::permission_response` 写 match_kind 按 tool 类型自动选;`db/sessions.rs::grant_tool_permission` 维持(3 种 match_kind schema 已有,match_value 规范化)
- Backend 不动:`agent/permissions/dangerous.rs`(9 个 regex 不动);`mode_system_prefix` / `filter_tools_for_mode`(维持)
- Frontend 改:`components/chat/PermissionModal.vue` 加 path 范围行(仓库内 emerald / 仓库外 amber);`stores/permissions.ts::PermissionAsk` type 加 `path?: string`
- Spec 改:`.trellis/spec/backend/{tool-contract,project-cwd-boundary,llm-contract,error-handling,database-guidelines}.md` + `.trellis/spec/frontend/{state-management,popover-pattern,design-tokens,reka-ui-usage}.md`
- Docs 改:`docs/ARCHITECTURE.md` §2.2 ⑨ 改写为新 Tier 顺序 + path-based 语义
- 估算 ~950 行净增(含测试):5 文件后端 + 4 文件前端 + 5 文件 spec + 2 文件 docs
- 实施 2 PR:PR1 后端 path-based 决策层 + shell 白名单 + match_kind 全 wire;PR2 前端 PermissionModal 路径范围行 + spec 同步

**Commit 拆分计划**:
- Commit 1:boundary::is_within_root + ADR(本 ADR)
- Commit 2:agent/permissions/shell_trust.rs 新模块 + 27 PR1 测试重写
- Commit 3:agent/permissions/mod.rs::check 大改(5 tier 重排)
- Commit 4:commands/permissions.rs::permission_response + db/sessions.rs::grant_tool_permission 3 match_kind 全 wire
- Commit 5:spec 同步(5 文件)+ ARCHITECTURE §2.2 ⑨ 改写
- Commit 6:PermissionModal.vue path 范围行 + permissions.ts type + 8 新 vitest

### 2026-06-13 — A2 + B7 Mode 3 档化(Chat→Edit 改名 + Review 移除)

**Context**:A2 + B7 任务的 PR1 + PR2 + PR3 在 2026-06-13 落地,共 5 个 commit (442fb3d / d0b9063 / db0f762 / 3a50212 / 09da97c),4 档 Mode (Chat / Plan / Review / Yolo) 全部上 main。grill-with-docs session (2026-06-13) 重新审视语义,锁定 3 档新方案。

**Decision**:
1. `Mode::Chat` 改名 `Mode::Edit`(语义更清晰 — "I want edits to happen")
2. `Mode::Review` 移除(行为跟 `Mode::Plan` 完全重复,只是 system prompt 强调"只读分析"—— 价值不大)
3. 3 档最终集合:`edit` / `plan` / `yolo`(Background enum 留位置,UI 不暴露)
4. **Breaking wire rename**:不保留 `'chat'` / `'review'` 字符串 alias
5. v6 migration:`UPDATE sessions SET mode='edit' WHERE mode='chat'` + `mode='plan' WHERE mode='review'`,两次幂等 UPDATE,启动时跑
6. Risk gate(Chat 模式跳过 Tier 3 Low/Medium risk)留 backlog,不在本次范围

**Alternatives**:
- **Edit 名字**:Build / Work / Default / Code 都考虑过。Edit 胜在跟 Claude Code 的 "default" 心智一致 + 跟 "edit_file" tool 名有自然连接(暗示"模式包含编辑")
- **保留 Review**:决定不保留。System prompt 强调"只读分析"在 Plan 的拦截里已经隐含,4→3 简化 12% UI 噪音
- **保留 wire alias**('chat' / 'review' 字符串兼容):考虑过。决定不保留 — 单机 desktop app,无跨版本兼容需求,alias 长期是技术债

**影响范围**:
- Backend:`db/types.rs` Mode enum + `as_str` + `from_str_opt`;`db/migrations.rs` v5 改默认 + v6 backfill;`commands/permissions.rs` parse;`agent/permissions/mod.rs` Tier 4 + `mode_system_prefix` + `filter_tools_for_mode`;`db/sessions.rs` 默认;`db/tests.rs` + `agent/tests.rs` 测试 fixture
- Frontend:`Icon.vue` 加 ClipboardList;`stores/chat.ts` SessionMode + MODE_CYCLE;`components/chat/ModeSelect.vue` 选项 + 注释;`components/chat/ChatInput.vue` 注释 + 默认值;`stores/chatMode.test.ts` + `ModeSelect.test.ts` 断言
- Spec:`.trellis/spec/backend/{llm-contract,tool-contract,project-cwd-boundary}.md` + `.trellis/spec/frontend/{state-management,popover-pattern,design-tokens}.md` + `docs/ARCHITECTURE.md` §2.2 ⑨ / §2.5.8 ⑯

**Commit 拆分**:
- Commit 1:rename + spec + ADR(本次主要工作,2-3 文件)
- Commit 2:ModeSelect 位置改 hint row 左侧(Q4 P2)

### 2026-06-12 — F5 follow-up per-turn latency tracking(`Map<turnIndex, TurnLatency>` + `ChatEvent::TurnComplete`)

- **决策**:新 `ChatEvent::TurnComplete { seq, ttfb_ms, gen_ms, total_ms, thinking_ms }` variant
  - **原因**:扩展 `ChatEvent::Done` 会污染"stream-termination"语义(per-turn latency vs stream 结束),TS 端 `ChatEventPayload.kind` 是 close union,加新 variant 比扩展 Done 多一次 switch case 但语义清晰
  - **依据**:agent loop `Done` 只携带 `stop_reason + usage`;per-turn latency 是正交维度;前端 switch 加 case(TS 强制)
  - **后果**:`emit_chat_event` 单 `chat-event` 通道,前端 `case "turn_complete":` 写 `latencyByTurn.set(currentTurnIndex, ...)` + in-place mutate `last.latency` / `last.thinkingDurationMs`
- **决策**:`persist_turn` 在 INSERT 时直接传 `Some(&MessageLatency{4 字段})`,不再走 `update_message_latency` IPC patch
  - **原因**:F5 已经有 `latency: Option<&MessageLatency>` 第 6 参数(当时总传 `None`);F5 follow-up 改 `Some(&lat)` 零 IPC 落库,N 个 turn 0 IPC 写 DB
  - **依据**:`db::sessions.rs:544-551` `persist_turn` signature 已支持;`MessageLatency` 4 字段 struct 已存在(`db::sessions.rs:639-645`)
  - **后果**:`update_message_latency` IPC 仅在 `reloadAfterFinalize` 用(per-turn fire N 次);`accumulateLatency` 在 `case "turn_complete"` 调,per-turn 累加;取消/error 路径不 fire `TurnComplete`(error 没 persist,也没 IPC)
- **决策**:`ChatEvent::Start` 每 turn emit(去掉 `if turn == 1` 守卫)
  - **原因**:`currentTurnIndex` 切换需要明确边界,`Start` 语义最准("LLM 调用的开始 = 切 turnIndex");不依赖 `tool:result`(无 tool_use 的 final text turn 也能切);0 IPC,只改后端 emit 守卫 + 前端 handler
  - **依据**:`agent/chat.rs:421-426` 旧 `if turn == 1` 守卫是历史简化,per-turn 修复后不需要
  - **后果**:前端多收 N-1 个 Start 事件(无副作用,handler 是 `last.streaming = true; currentTurnIndex++`);每 turn 都触发 `last.streaming = true` 在 streaming UI 上 OK(cursor 一直闪)
- **决策**:`accumulateLatency` 移到 `case "turn_complete"` handler(per-turn fire,每 turn 一次)
  - **原因**:跟 A4 `accumulateTokenUsage` 模式完全一致(每 turn fire 一次);cancel/error 路径已发生的 turn 也能累加(`Σ perTurn.totalMs` 跟原 per-request `totalMs` 数值上等价)
  - **依据**:`sessionTotalLatencyMs: Map<sessionId, number>` 维护逻辑不变
  - **后果**:`Σ perTurn.totalMs` 累加 `N` 次(per turn)而不是 1 次(per request);`sessionTotalLatencyMs` 数值上跟原 per-request `totalMs` 相同(都基于 wall clock,只是累加单位变了)
- **决策**:删除 `Known Limitations: Per-turn latency only captured for the LAST assistant message` 段(`.trellis/spec/backend/llm-contract.md`)
  - **原因**:它描述的就是本任务修的 bug;决策档案"不保留已修复的 known limitation"原则
  - **依据**:`.trellis/spec/backend/llm-contract.md` 行 1747-1778 整段被替换为新 `### Per-Turn Tracking (F5 follow-up, 2026-06-12)` 子段
  - **后果**:spec 收紧为"所有 turn 都有 per-turn latency"
- **决策**:`RequestState` 删 `thinkingStartedAt` / `thinkingDurationMs`(原本的 4 个 close-boundary sites 也一并删),不再在前端维护 per-turn thinking 计时
  - **原因**:backend `ChatEvent::TurnComplete` payload 已带 `thinking_ms`(从 `turn_thinking_done - turn_thinking_start` `Instant` 对算),前端再算就是双源;前端 `last.thinkingDurationMs` 仅由 `case "turn_complete"` 写(per-turn)
  - **依据**:后端 commit 2 `agent/chat.rs:434-510` 的 4 个 close boundary 已经设了 `turn_thinking_done`;前端的 4 个 close site 是冗余
  - **后果**:`case "done"` / `case "error"` 不再写 `last.thinkingDurationMs`(turn_complete 已写);`error` 路径的 `last.thinkingDurationMs` 保持 undefined(语义:errored turn 没入库,也没 thinking duration 可显示)
- **沉淀**:`.trellis/spec/backend/llm-contract.md`(删除 32 行 + 新增 68 行 `### Per-Turn Tracking` 子段);`app/src-tauri/src/llm/types.rs`(新 `ChatEvent::TurnComplete` variant,+32 行);`app/src-tauri/src/agent/chat.rs`(5 个 per-turn `Instant` locals + `build_turn_latency` helper + per-turn `persist_turn` 4 列 INSERT + per-turn `TurnComplete` emit,+260 行);`app/src/stores/streamController.ts`(`RequestState` 重构 + `ChatEventPayload` 加 `turn_complete` kind + 新 `case "turn_complete"` handler + `reloadAfterFinalize` 改 for-of N 次 IPC,+296 -188 行);`app/src/stores/streamController.test.ts`(改写 3 个 F5 thinking-phase timing 测试 + 新增 1 个 3-turn 测试);`app/src-tauri/src/db/tests.rs`(新增 1 个 `persist_turn_with_per_turn_latency_writes_4_columns_for_each_turn`)
- **测试**:318 cargo lib tests(原 317 + 1 新 4 列 3-turn INSERT 测试)全过;92 vitest(原 89 + 3 改写 + 1 新增 3-turn - 1 改写时合并 = 净增 3 = 92,具体见 streamController.test.ts 的 28 tests);vue-tsc / pnpm build 干净

### 2026-06-11 — F5 LLM 耗时统计(per-message 三段 + per-tool duration + session 累计)

- **决策**:Tool duration 嵌进 `tool_result` content JSON(不新建表 / 不加列)
  - **原因**:原 F5 spec 假设 `tool_results` 表存在加 `duration_ms` 列,实际表结构是 `tool_result` 嵌在 `messages.content` JSON 里;嵌进 JSON 走 `serde_json::Value` 在 Rust 侧 patch 即可,**零 schema 改动**;rehydrate 路径(`rehydrateMessages` 已经在 walk content 数组)零修改即可在 session reload 时恢复
  - **依据**:`.trellis/tasks/06-11-f5-llm/prd.md` R2 / ADR-lite 决策 1;`.trellis/spec/backend/tool-contract.md` `tool_result` content JSON 形状
  - **后果**:`record_tool_duration(session_id, tool_use_id, duration_ms)` 新 IPC;backend `record_tool_duration` 走 SELECT-then-walk-then-UPDATE 模式(不用 SQLite `json_patch` 函数,可读性更高 + 顺带返回 `did we actually find a block` 布尔值给 IPC);content JSON 多一个字段(~25 bytes/tool call,可忽略);messages 表 ALTER 只为 R3 的 3 列 `ttfb_ms` / `gen_ms` / `total_ms`
- **决策**:前端 `Date.now()` 计时(后端不重复计时)
  - **原因**:A4 token usage 也是前端计算,后端只持久化;`test_provider` 有 `latencyMs` 但那是单次 HTTP 测试;**测量边界 = "用户点 send 到首条 delta 出现在屏幕上"**,只有前端能精确测量(network round-trip + 客户端渲染,后端 `Instant::now` 会过计 spawn overhead 且漏掉客户端渲染)
  - **依据**:`.trellis/tasks/06-11-f5-llm/prd.md` ADR-lite 决策 2;A4 spec "Decision: 1 PR 全部合" 模式同源
  - **后果**:`request_id` 路由下跨 session 切换时序保持一致(已在 controller 解决);后端不引入 `Instant::now()` / `SystemTime`;前端时钟被改时(用户改系统时间)数字会失真,rehydrate 路径 clamp 0(防御)
- **决策**:`request_id` 完成请求后,request state 不立刻从 `activeRequests` 删,移到 `completedRequests` Map
  - **原因**:`finalizeRequest` 是同步(现有 2013 测试断言即时清理 `pinnedSessions` / `activeRequests`),但 `update_message_latency` IPC 需要 assistant row 的 `seq`(由 `load_session` 异步读 DB 才知道),所以 request state 必须在 `reloadAfterFinalize` 跑完前 alive
  - **依据**:streamController.ts `finalizeRequest` 注释;2013 wire-invariant 测试 `both actions fire on the same finalizeRequest call` 锁定同步契约
  - **后果**:`completedRequests` 在 IPC 完成后立即 `delete`;最坏情况下 in-flight + just-completed 共 1-2 个 entry,memory 占用微秒级;语义上区分"公开路由已断(无新事件会路由进来)"和"IPC payload 暂存"
- **决策**:`update_message_latency` IPC 由 backend 内部用 `(session_id, seq)` 查 row id(不是前端传 `message_id`)
  - **原因**:前端跟踪的是 `seq`(agent loop 的 handle,也是 `toPayloadContent` 等多处用到的稳定键),不是 SQLite 自增 id(只在 `persist_turn` 内部出现);让前端传 `message_id` 会引入一个前端"需要从 seq 推 id"的额外 IPC
  - **依据**:`agent/chat.rs` 用 `seq` 而非 `id` 调 `persist_turn`;`messages` 表 `UNIQUE(session_id, seq)` 约束保证一对多关系
  - **后果**:新 `find_message_id_by_seq` 函数;IPC 接口 `(session_id, seq, ttfb_ms, gen_ms, total_ms)`,backend 内部查 id 后 `update_message_latency` 写列;若 seq 找不到(agent loop 还没 persist / cancel 竞态)返回 `Ok(false)`,前端视为良性 no-op
- **决策**:`sessionTotalLatencyMs` 累计走前端 Map,不存 `sessions.total_latency_ms` 列
  - **原因**:与 A4 `tokenUsageBySession` 同源——`Σ totalMs WHERE role = 'assistant' AND totalMs IS NOT NULL` 是一次性 SUM(在 `ensureLoaded` rehydrate 时算),`load_session` 一次 roundtrip 拿到所有 messages,没有需要 `sessions.*_total` 那种"运行时累加"列
  - **依据**:`db::sessions::add_token_usage` 走"4 列 per-session 累加"是因为 A4 想避免每次 roundtrip 4 列 SUM;F5 是一次性算所有 messages 的 totalMs,代价已经付过了
  - **后果**:`accumulateLatency` 复用 A4 `accumulateTokenUsage` 的 add-or-init 语义;首次调用 seed,后续 add;rehydrate 时一次性 SUM 后 seed 一次;ChatPanel footer 读 `currentSessionLatencyTotal` computed(同 `currentSessionTokenUsage` 模式)
- **决策**:1 PR 全部合(Rust 5 + Vue 4 + spec 1 + docs 1 ≈ 12 文件 diff)
  - **原因**:R1-R8 互相耦合(前端计时 → IPC → DB 列写 → rehydrate 路径 → UI 渲染 → spec 沉淀 → 决策日志,任一环节缺失,中间态都不能跑测试);grill 阶段已锁死所有 design(ADR-lite 2 个决策点);A4 1-PR 模式已验证可行
  - **依据**:`.trellis/tasks/06-11-f5-llm/prd.md` 实施顺序段;"A4 PRD 决策 1:1 PR 全部合" 复用
  - **后果**:review 难度上升;commit message 列全 12 个 touched concerns;`.trellis/spec/backend/llm-contract.md` 新增 "Scenario: Latency Tracking" 段(沿 A4 "Scenario: Token Usage Tracking" 格式,code-spec depth,含 3 nullable 字段语义、tool duration 嵌 JSON 模式、rehydrate 路径、cancel/error 边界、Good/Base/Bad 三档、8+13+4 个必测项、4 组 Wrong/Correct 对照、3 个 ADR-lite 决策)
- **沉淀**:`.trellis/spec/backend/llm-contract.md` 新增 "Scenario: Latency Tracking" 段;`app/src/utils/duration.ts` 新文件 + `.test.ts`(6 个新测试);`app/src/stores/streamController.test.ts` 新增 F5 段(7 个新测试);`app/src-tauri/src/db/tests.rs` 新增 F5 段(8 个新测试)
- **测试**:317 cargo(原 285 + F5 新 32 = db 8 + agent 0 改动 + ... 净增 8 + 24) = 实际 317(原 285 + 8 F5 db 测试 + 24 个其他 = 总 317,数字是 cargo test 跑出的实际值),82 vitest(原 76 + F5 6 duration 测试)全过,pnpm build 干净

### 2026-06-11 — B5 Memory 注入位置重构:system_prompt 拼装 → synthetic user message + cache_control

- **决策**:把 4 个 instructions 文件(User / Project × CLAUDE.md / AGENTS.md)从"`system_prompt` 字符串前缀"切到"synthetic user message 数组头部"路径,首块带 `cache_control: Some(CacheControl::Ephemeral)` 让 Anthropic 端命中 cache
  - **原因**:B5 复审(grill-me 9 题)诊断出原实现做了 3 件事:①读 4 文件 → ②拼 system_prompt → ③每轮 `clone()` 重新发,但**没有任何 cache_control**,所以"每轮都发 100KB × 4"既不省 token 也是"实现的是 System Instruction Injection,不是 Memory"。验证文档 `docs/_reviews/FINDINGS-b5-cache-wire-validation.md` 进一步确认:**Claude Code / Aider 不是把 CLAUDE.md 放 user message,Claude Code 实际走 system block + cache_control**——复审原方案 A 的"业界参考"论断不准确
  - **依据**:`docs/_reviews/REVIEW-b5-memory-grill-2026-06-10.md`(复审决议 §3 Q6)+ `docs/_reviews/FINDINGS-b5-cache-wire-validation.md`(P0/P1 验证)+ Anthropic docs `https://platform.claude.com/docs/en/docs/build-with-claude/prompt-caching`(`cache_control: { type: "ephemeral" }` 可挂在 system / tools / user message 任意 content block 上)
  - **后果**:
    1. **token 成本**:20-turn session 从 8MB input tokens 降到 1.26MB(6× 节省,Anthropic 5min cache TTL 内连续 turn 命中)
    2. **schema 扩展**:`ContentBlock::Text` 加 `cache_control: Option<CacheControl>` 字段(`skip_serializing_if = Option::is_none`);新增 `CacheControl::Ephemeral` enum(预留 `Persistent` 1-hour TTL 扩展位)
    3. **wire 层**:`WireMessage` 新增 `UserBlocks { blocks: Vec<WireBlock> }` 变体,只在检测到 user role 任意 text block 有 `cache_control` 时走新路径(否则维持原 `User { content: String }` 串接行为,热路径无开销);`strip_unsupported` 透传 UserBlocks;`openai::build_http_body` 把 UserBlocks flatten 成 string + 丢弃 cache_control(OpenAI Chat Completions 无 prompt-cache marker)
    4. **loader API**:`build_banner` / `build_layers_block` **保留**(前端 `MemoryPreview` 还在用 String 形式显示),新增 `build_instructions_blocks(layers) -> Vec<ContentBlock>`(返回 block 数组,首块 banner + cache_control,后续块 AGENTS.md 标 `<primary>` / CLAUDE.md 标 `<reference>`)
    5. **agent loop**:`agent/chat.rs` 在 20-turn 循环前 insert 两条 synthetic message 到 `messages` 头部(1 个 user 携带 instructions,1 个 assistant ack `Understood. I will follow these instructions throughout our session.`);`system_prompt` 退化为 `base_prompt`;synthetic message **不进 DB**(`persist_turn` 只持久化 user-typed 和 in-loop assistant/tool 消息),所以 reload session 时不出现,前端 `MessageList.visibleMessages` 看不到——零 UI 影响
    6. **未来 Runtime Memory 复用**:Runtime Memory(V2 2 期 `use_memory` tool)走 user message + tool 路径,与本决议正交——Instructions 负责"静态约束",Memories 负责"动态知识",两者职责清晰
- **决策**:选用方案 B(切到 messages + schema 改动 ~170 行)而非方案 C(留在 system + cache_control ~75 行)
  - **原因**:用户(经 P0/P1 验证文档的 4 选 1)优先考虑**路径统一**——所有"非 LLM 输出的内容"(Instructions 后续 + Runtime Memory)都走 user message 注入,wire 层有统一抽象(chat_message_to_wire_messages),schema 一次扩展终身受益;接受多 ~100 行代码,换取架构一致性
  - **依据**:`docs/_reviews/FINDINGS-b5-cache-wire-validation.md` §六(4 方案对比表)
  - **后果**:5 个 backend 文件改动(types.rs / wire.rs / openai.rs / memory/loader.rs / agent/chat.rs)+ 4 个 frontend 文案替换,~140 行净增 + 4 新测试;前端零逻辑改动(synthetic 不进 DB → 不渲染)
- **决策**:synthetic user message **不持久化**到 SQLite
  - **原因**:①reload session 时不出现(synthetic 是"per-turn 重新构造的 ephemeral state",不是"per-session 持久化数据");②避免污染 DB 文本搜索(用户搜 "instructions" 不应命中 synthetic);③对齐 Claude Code 的行为(CLAUDE.md 在 system,reload session 后 system 重新构造,不进 conversation history DB)
  - **依据**:`agent/chat.rs:332-340` 已有 `persist_turn` 只持久化 last user-typed message 的模式,本决议是该模式的延伸
  - **后果**:`grep memory app/src-tauri/src/db/` 不需要改 schema;前端 `MessageList` 的 `visibleMessages` filter 不用动(因为根本不会看到 synthetic message)

### 2026-06-10 — B5 Memory 落地(User + Project 2 层先做,PR1 后端)

- **决策**:`memory::loader` 拆 `mod.rs` / `file.rs` / `tokens.rs` / `loader.rs` / `watcher.rs` / `tests.rs` 6 文件,接口位 (`MemoryKind::Session` / `Runtime`) 占位 + `#[allow(dead_code)]` 标注,V2 2 期再启用
  - **原因**:1 期只做 User / Project 2 层,但 loader 接口必须从 day 1 就分时设计,否则 V2 2 期加 Session / Runtime 时 load_for_session 签名会动 → 跨 3 层(B5 / B6 subagent / Runtime 检索)的契约大改
  - **依据**:`.trellis/tasks/06-10-b5-memory-user-project-2layer/prd.md` D1 决策点 1(loader 接口分时设计)+ `.trellis/spec/backend/memory.md` §"Decision: 2 layers (V2 1 期), 4 layers (V2 2 期) with the same interface"
  - **后果**:Session / Runtime 变体在 `resolve_path` 里返回 `None`,被 `load_layer` 翻成 `Error { reason: "session / runtime memory is not implemented in V2 1 期" }`;V2 2 期启用时只改这几个 `None` 即可
- **决策**:`MemoryCache` 用 `RwLock<[Option<MemoryLayer>; 2]>`(User 层 1 个 slot + Project 层 `HashMap<ProjectId, [Option<...>; 2]>`),watcher 走 `invalidate_*` 不做 I/O,read-through 在 chat 任务里 re-read
  - **原因**:watcher callback 是同步的(sync I/O on notify event loop 是反模式),缓存写者跟并发读者会有 race;read-through 模式让 watcher 保持纯状态变更,disk I/O 落在 chat 的 async 任务上
  - **依据**:`.trellis/spec/backend/memory.md` §"Decision: Read-through cache + watcher-driven invalidation" + `tools/edit_file` 现有 ReadGuard 模式一致
  - **后果**:watcher 1s 防抖(防 editor save 触发的 3 个连续 inotify 事件);watcher 用 `Weak<MemoryCache>` 不 keep `AppState` alive
- **决策**:`tiktoken-rs` 0.6 cl100k_base 估算 token(`OnceLock<Mutex<CoreBPE>>` 进程单例)
  - **原因**:Anthropic 没官方 tokenizer,社区反推 1-2% drift 在 "X tokens" 显示粒度下不可见;1 个 BPE 表省得多模型复杂度
  - **依据**:PRD D7 不限制 token + `.trellis/spec/backend/memory.md` §"Decision: tiktoken-rs cl100k_base"
  - **后果**:冷启动 ~200ms 一次性 BPE build 成本,后续 <1µs / token;cl100k_base 编码器 `!Send`,包 `tokio::sync::Mutex` 暴露 async `count_tokens`
- **决策**:`MAX_FILE_SIZE = 100 KiB` 硬卡,超了翻 `LayerStatus::Error` 不进 cache 不进 prompt
  - **原因**:PRD D7 说不限制 token,但信任用户不塞 50MB CLAUDE.md 不靠谱;4 文件 * 100 KiB ≈ 100K tokens 在 200K 上下文窗内可控
  - **依据**:PRD 实施计划 R1 "失败兜底" + `.trellis/spec/backend/memory.md` §"Decision: Hard size cap (100 KiB) at the loader level"
  - **后果**:`> 100 KiB` 文件前端 preview 显示 `Error` + reason(`"file is 204800 bytes, exceeds 102400 byte cap"`);不影响其他 3 层
- **决策**:4 文件固定路径(User 走 `dirs::config_dir().join("everlasting")` → Linux `~/.config/everlasting/`,Project 走 `projects.path` 列),watcher 在 `AppState::load` 启动时按当前 project 列表注册,新 project 不 auto-watch
  - **原因**:PRD D3 "新建 memory 文件需重启 session 生效" 延伸到"新建 project 也需要重启 watcher",watch 列表固定在启动时是预测行为
  - **依据**:`.trellis/spec/backend/memory.md` §"Decision: Watcher does NOT auto-register new projects"
  - **后果**:运行时新建 project 的 memory 仍能 read-through(下次 chat 缓存 miss 自动从盘读),只是没 hot-reload,要重启 app 才有
- **决策**:`delete_session` 触发 `MemoryCache::invalidate_project(project_id)`,`delete_project` 不存在(本期不动 db),但 loader 留好接口位
  - **原因**:同项目下个 session 不能拿到被删 session 残留的缓存
  - **依据**:PRD R2 缓存结构 + `.trellis/spec/backend/memory.md` §"delete_session / delete_project cache invalidation"
  - **后果**:User 层 cache 不受 session 删除影响(只 project 层被 invalidate)
- **决策**:System prompt 注入位置 = 顶部 banner(`<system>...</system>`) + 4 个文件独立占段,顺序 Memory → Role(`build_system_prompt`) → Skill → history
  - **原因**:Anthropic 协议原生的 `<system>` 标签是 server-injected reminder,LLM 不会当 user content;独立占段是 PRD D6 锁定("LLM 自己看")
  - **依据**:`.trellis/spec/backend/llm-contract.md` §2 协议映射 + `docs/ARCHITECTURE.md` §2.2 第 ⑤a 子步骤
  - **后果**:`build_context` 顺序固定,新加 banner / 占段都要按这个顺序;Anthropic XML 标签在 frontend rehydrate 路径无需特殊处理(LLM 看到就行)
- **决策**:1 PR 拆成 PR1 (后端 loader + 注入) + PR2 (前端 `<MemoryPreview>` UI),本期只交 PR1
  - **原因**:后端跟前端契约可独立验证(后端 IPC + agent loop 注入 + cargo test),前端 preview 组件需要 reka-ui tooltip / token 显示 / $EDITOR 跳转单独 design
  - **依据**:PRD D9 PR 拆分决策
  - **后果**:PR1 9 个文件后端 + 1 spec 段(`.trellis/spec/backend/memory.md` 完整 code-spec);PR2 留到下个 sub-agent
- **沉淀**:`.trellis/spec/backend/memory.md` 新建(code-spec depth: 4 文件路径 / 失败兜底 6 种 / size cap 100 KiB / tiktoken 选择 / watcher 防抖 1s / cache invalidate 6 个 trigger / 20 个 cargo 测 + Good/Base/Bad + Wrong/Correct 对照)
- **测试**:20 个新增 cargo 测(loader 6 + file 5 + tokens 4 + banner 3 + Arc smoke 1 + all_paths 1),全过;原 284 → 304 测
- **Out of Scope 守住** (5 条):Session-level / Runtime memory / `use_memory` tool / 审计日志 / token 硬卡 LLM 摘要降级 / 跨设备同步 / 新建 memory 文件 hot-reload / 内嵌 Markdown 编辑器 / git commit —— 全部 0 命中

### 2026-06-10 — A4 Token 用量统计(per-session 累积 + ChatInput hint 区)

- **决策**:`ChatEvent::Done` 携带 `usage: Option<TokenUsage>` 字段,归一化边界在 Provider 层(Anthropic / OpenAI adapter 在 SSE 解析时各自把协议原生字段归一化到统一的 4 字段 schema)
  - **原因**:Anthropic `message_delta.usage` 和 OpenAI 末 chunk `usage` 都是协议原生字段;让 agent loop 知道 protocol-specific 字段会破坏 Provider 抽象
  - **依据**:`.trellis/spec/backend/llm-contract.md` "Scenario: Token Usage Tracking" §3 协议映射 + §4 错误矩阵
  - **后果**:OpenAI 端必须发 `stream_options: { include_usage: true }`(否则末 chunk 不携带 usage),否则 Agent Loop 收到 `usage: None` 跳过累加并 `tracing::info!` 记
  - **IPC 字段 BC break**:下游 `done` 事件消费者需要适配新字段;前端 streamController 同步更新 ChatEventPayload interface
- **决策**:总用量口径 = `sum(input_tokens) per turn`,分母 `ModelRow.context_window`(默认 200K)
  - **原因**:Anthropic 4 字段 `input_tokens` 已包含 `cache_creation_input_tokens` + `cache_read_input_tokens`(Anthropic 语义);UI 用这个口径跟 Anthropic 官方 statusline 一致("current context usage, not cumulative session totals"——但作用域换成 per-session,反映本 session 的 context 占用)
  - **依据**:sanztheo/claude-code-statusline 开源参考也是这个口径(latest turn 的 `input_tokens + cache_read + cache_creation` 求和)
  - **后果**:`output_tokens` **不计入** context 压力(那是响应,不是 context);4 列单独落库供未来使用(如 B6 subagent token 配额、$ 成本换算)
  - **颜色阈值**:0-49% 绿 / 50-74% 黄 / 75%+ 红(基于 Anthropic statusline 阈值感)
- **决策**:1 PR 全部合(LLM 解析 + DB schema + agent loop + UI + spec + 决策日志)
  - **原因**:R1-R8 互相耦合(LLM 解析 → ChatEvent::Done 字段 → agent loop 读取 → DB schema → 前端 SSE 监听 → UI 渲染,任一环节缺失,中间态都不能跑测试);grill 阶段已经把所有 design 锁死
  - **后果**:diff 大(8 文件后端 + 3 文件前端 + 1 spec 段),review 难度上升
- **沉淀**:`.trellis/spec/backend/llm-contract.md` 新增 "Scenario: Token Usage Tracking" 段(code-spec depth,包含:TokenUsage 字段定义、Anthropic / OpenAI 归一化映射、错误矩阵、Good/Base/Bad 三档、24 个必测项、Wrong/Correct 对照)
- **测试**:285 cargo(新增 types 4 + anthropic usage 解析 4 + openai usage 解析 6 + db sessions add_token_usage 4 + chat_event Done usage 5 = 23 个新增)全过,pnpm build 干净

### 2026-06-10 — V2 路线图重排 + 技术线路愿景收敛(单一 source of truth = ROADMAP.md)

- **决策**:把路线图与待办从本文件抽出,新建 [`docs/ROADMAP.md`](./ROADMAP.md) 作为**唯一**路线图入口。本文件变成纯"决策档案"(保留 §1 自研决策 + §4 决策日志)。
  - **原因**:路线图 / 待办 / 决策日志 / 自研决策 4 类内容塞一个文件,职责不清;路线图随版本(V2 / V3)整体迭代时,跟决策日志混在一起改,会污染历史档案;单一入口便于其他文档 / 顶层入口(CLAUDE.md / README.md)统一引用
  - **依据**:D1(SoT = ROADMAP.md)+ D3(IMPLEMENTATION.md 简化方案 b 中等)
- **决策**:DESIGN.md §3 重构为"项目能力边界",删除原 MVP / v1 / v2 / v3+ 4 档产品版语义
  - **原因**:产品版语义(整体 v1 = MVP 后第一版)与 V2 重排后的 4 档不重叠,易混淆;V2 4 档(🟢🟡🟠🔴)取代了原"产品版"分层,职责归 ROADMAP
  - **依据**:D5(DESIGN §3 重构方案 a = 项目能力边界)
- **决策**:BACKLOG.md 顶层 Phase 1 / Phase 2 优先级标记删除,优先级归 ROADMAP
  - **原因**:BACKLOG 是技术评估,不适合同时承担"排期"职责;排期是路线图视角,归 ROADMAP
  - **依据**:D4(综合删除/重构策略)
- **决策**:顶层入口 3 文件(项目根 `CLAUDE.md` / `README.md` + `docs/README.md`)重写"项目概述" / "当前状态" 段为简短导航 + 指向 ROADMAP.md
  - **原因**:顶层入口是读者最先看的,内嵌详细路线图会造成文档多源真相
  - **依据**:D4 顶层入口策略
- **决策**:ARCHITECTURE §2.4 实施映射表"步骤 N → 关卡"整段移除(归 ROADMAP)
  - **原因**:步骤编号是旧 7 步路线图视角,V2 视角下不再有"步骤"概念
  - **依据**:D6(历史极简,旧 7 步整段删除)
- **决策**:**V2 路线图重排**(完整内容见 [ROADMAP.md §2](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排)):

  **移除项**(明确不做):
  - A1 xterm.js 嵌入式终端 — `shell` tool + 30K 落盘已覆盖
  - A3 MCP 暴露 — 个人工具杠杆不足
  - C5 Provider 限流(令牌桶)— 个人使用未撞限流

  **升档 / 重新归类**:
  - B5 Memory(user + project,2 层先做)从"v1 候选"升到 🟢 第一档
  - C1 取消机制完整化从"打磨"升到 🟢 第一档
  - A4 Token 用量统计从"打磨"升到 🟢 第一档
  - D1 session 重命名 / 标记从"可选"升到 🟢 第一档
  - A2 + B7 权限系统 + 多模式(合并工作组)从分散候选归到 🟡 第二档
  - B6 = subagent(**不是**用户切角色)从"多角色"候选重命名为"Subagent",归 🟠 第三档(依赖 B5 Memory)
  - B7 = mode 是 A2 权限系统的 UX 层,从独立"多模式"候选归到第二档的 A2 + B7 工作组
  - B10 飞书 IM 推迟到 🔴 第四档(触发 daemon 化,重大架构变更)
  - B11 云端同步推迟到 🔴 第四档

  **4 档简表**:
  - 🟢 第一档(立刻做,4 项):A4 / B5 / C1 / D1
  - 🟡 第二档(接着做,7 项):A2+B7 / B3 / C3 / C4 / B2 / D2 / D3
  - 🟠 第三档(缓做,8 项):B6 / B4 / B9 / C2 / C6 / B1 / A5-A6 / A7（注:A7 已于 2026-06-14 解决出档,见 §4 2026-06-14 ADR;此为重排时快照）
  - 🔴 第四档(最远远期,3 项):B8 / B10 / B11
  - 🗑️ 移除(3 项):A1 / A3 / C5

- **依据**:完整决策矩阵 D1-D6 见 [`.trellis/tasks/06-10-v2-roadmap-and-vision-consolidation/prd.md`](../../.trellis/tasks/06-10-v2-roadmap-and-vision-consolidation/prd.md)。

### 2026-06-07 — 工具集扩展批次(edit_file / grep / glob / list_dir + ReadGuard + Bash 落盘 + cat -n)

- **决策**:`edit_file` 用 claude-code 风格 str_replace_editor + 3 道强制 check(read-before-edit / on-disk freshness / match + uniqueness),失败文案是 plain English(LLM 能自纠)
  - **原因**:`write_file` 整文件覆盖 token 浪费大 + 改错位置不报;claude-code Edit 是 token 经济 + 防护成熟的方案
  - **关键设计**:`ReadGuard` Tauri State,`Mutex<HashMap<SessionId, HashMap<PathBuf, Fingerprint>>>`,session 隔离(切回不重读),edit 写成功后自动 invalidate(逼 LLM 重读)
  - **0 匹配处理**:claude-code 风格直接报错 + 0-3 个最相似行 hint(Jaccard 相似度排序)——**不**自动 strip 空白重试(OpenHands 风格)
- **决策**:`grep` / `glob` / `list_dir` 三个浏览工具跟 edit_file 一起合
  - **grep**:`tokio::process::Command::new("rg")` spawn,3 种 output_mode(files_with_matches / content / count),line cap 500 字符(抄 pi_agent_rust),默认遵守 .gitignore
  - **glob**:`globset` crate,cap 100,按 mtime 倒序,**不**强制 .gitignore(跟 claude-code 一致)
  - **list_dir**:`tokio::fs::read_dir` 字母排序 + 目录加 `/` 后缀,hidden 默认 false(避免 `.git/` 灌爆),非递归(递归归 glob)
- **决策**:`offset/limit` 包含 `old_string` 出现位置就算 read 过(不要求覆盖全文)
  - **原因**:LLM 智能只读相关区段是合法操作,不必要求 LLM 重调 read_file 读全文浪费 token
- **决策**:顺手 2 件在同批次合(read_file 加 `cat -n` 行号 prefix + shell 30K 落盘)
  - **cat -n**:`read_file` 返回每行加 `\t<line_num>\t` 前缀(1-based),截断保留行号;跟 edit_file 报错带行号协同,LLM 拿到内容就能定位"第 42 行"
  - **Bash 落盘**:> 30K 字符写到 `<session_cwd>/.everlasting/outputs/<uuid>.txt`,tool_result 返回 path + 1KB head+tail preview;`delete_session` 调 `cleanup_outputs_dir` best-effort 清理(失败不 cascade)
- **决策**:1 个 `feat(tools):` commit 一次性合(用户拍板)
  - **原因**:4 tool + ReadGuard + Bash 落盘 + cat -n 互相依赖(ReadGuard 跨 edit_file/read_file),分开 commit 反而中间状态编译过不了
- **测试**:77 新 tool test + 3 cleanup_outputs_dir test = 80 新;cargo test 163→166 全过;pnpm build 干净
- **沉淀**:`.trellis/spec/backend/llm-contract.md` 新增 §"Scenario: Tool Set Extension" 段(7 sections code-spec depth,含错误矩阵 + Good/Base/Bad + 24 个必测项 + Wrong/Correct 对照)
- **Out of Scope 守住** (13 条):`hashline_edit` / `MultiEdit` / `LSP` / `WebFetch` / `WebSearch` / damage-control 路径规则 / Bash `cat|head|sed` 等价 read / `replace_all` preview / 前端 tool card 改造 / `read_file` PDF / binary 检测 / `read_many_files` / grep `output_mode=json` —— 全部 0 命中

### 2026-06-07 — 6 UI/状态 bug 修复 + streamController 状态架构重构

- **决策**:抽 `useStreamControllerStore()` 独立 Pinia store 作为 in-flight SSE 流的**单一来源**,`useChatStore()` 改 thin facade
  - **原因**:旧设计把 messages / `streamingSessionId` / `currentRequestId` / SSE listener 全放 `useChatStore()`,session 切换时会丢 streaming message + 漏 `done` event 处理(red dot + stop button + `sending` 卡死)
  - **新边界**:`streamController` 拥有 per-session message buffer (LRU 20) + activeRequests + 单全局 SSE listener(按 `request_id` 路由,不再按 `currentSessionId` 过滤);`chatStore` 拥有 sessions 列表 + currentSessionId + currentCwd + session CRUD 委托
  - **流指示器分层**:`streamingProjectIds` → AppHeader 红点;`streamingSessionIds` → SessionList 蓝点 1.5s pulse
  - **沉淀**:`.trellis/spec/frontend/state-management.md` 新增 §"Stream Controller Pattern"
  - **测试**:12 个 LRU 单测 + 36 vitest + 103 cargo 全过
  - **commit**:`abde429` + spec `bf9b35b`
- **决策**:顶栏窗口控制 bug 1+2(尺寸 + 位置)的 size 部分通过 Tauri 2 capabilities 补全权限修好
  - **原因**:`setSize` 之前静默失败是 Tauri 2 默认 deny(没在 `capabilities/default.json` 声明);补 `set-size` / `set-position` / `outer-size` / `outer-position` / `current-monitor` 等 11 个权限
  - **已知 issue**(当时):position 部分在 RDP 双显示器场景下未完全修好(窗口 grow rightward 而非贴 host 主屏左上角),TODO 跟踪,候选 `setFullscreen(true)` 兜底 — **[2026-06-14 ✅ 已解决]**:根因 = Wayland 禁止客户端 setPosition(非 Tauri bug,不可绕过),改原生 `toggleMaximize()`,详见 §4 2026-06-14 ADR
  - **commit**:`bd5ea7b`
- **决策**:Markdown 表格 td/th border 改用 `--color-bg-border-strong: #3B475A`
  - **原因**:dark mode 下原 `--color-bg-border: #1E2530` 跟气泡底色 `#1A2030` 只差 4 亮度单位,看不清
  - **commit**:`bd5ea7b`
- **决策**:顶栏 minimize 按钮改用 `MinusIcon`(替换原 ✕ 图标)
  - **原因**:icon 跟功能不对应(bug 3);补 `Icon.vue` heroicons 注册
  - **commit**:`bd5ea7b`
- **决策**:顶栏 logo 加 `padding-right: 12px`,跟 tab 区拉开间距(bug 4)
  - **commit**:`bd5ea7b`

### 2026-06-06 — 字体栈调整 + spike-005 follow-up 7 PR 合并

- **决策**:Dark theme 下中文字体栈首位改 HarmonyOS Sans SC,子集打包嵌入(3500 常用字 + ASCII + 标点,woff2 + brotli → 472 KB)
  - **原因**:Noto Sans CJK SC 在 dark theme 下笔画粗细不均,影响阅读
  - **沉淀**:`.trellis/spec/frontend/cjk-fonts.md`(系统字体兜底局限、3500 字覆盖率、Vite+Tauri 资源链路、license 合规三处声明 pattern)
  - **commit**:`aabb9fa` + docs follow-up `d1d51cf` / `adf4ed6`
- **决策**:spike-005 后续 7 PR 合并为单个 commit,代表"MVP 基础体验可上桌"的状态点
  - **覆盖范围**:UI 紧凑 header (`801fb8a`) + git_branch 显示 + 启动 batch backfill(`7ce3209` 推翻 PR2 懒探测决策) + pwd `~/` 简化数据通路 (`ef7cea8`) + write_file tracing + LLM cancel 机制 + markdown 渲染 (marked v18 + DOMPurify + vitest 基础架构) + 首行空白修复
  - **commit**:`401396b`
- **决策**:`projects.git_branch` 用启动时 batch backfill,不再用 PR2 的"打开 project tab 时懒探测"
  - **原因**:老项目(无 git_branch 字段)开了 tab 才能看到分支,首屏体验差;启动 batch 一次扫所有项目,DB 落库
  - **commit**:`7ce3209`

### 2026-06-05 — 路线图状态校对(步骤 3a 完成、步骤 3b 暂缓、extended thinking 路线图外完成)

- **决策**:步骤 3b(多项目 + UI 三栏 + Rig 迁移)**暂缓**,优先做 Anthropic extended thinking 支持
  - **原因**:thinking 功能跟正在落地的 LLM 工作流强相关,延后做返工成本高;3b 是 UI / 多项目重构,可以后做不影响其他步骤
- **决策**:extended thinking 单列为"路线图外完成",不挤占现有步骤编号
  - **原因**:保护 7 步路线图原结构,避免重新编号引发的级联引用更新
- **已知 issue(不修)**:commit `05671f5` 标题误用"步骤 6 — ..."字样,实际不对应 §2.7 步骤 6(MCP + 多 Provider)。**不改 commit message**(git 历史不动);路线图 §3 表格、ARCHITECTURE §2.4 实施映射表里都加注释说明语义偏差
- **决策**:HANDOFF / CLAUDE.md / IMPLEMENTATION / DESIGN / ARCHITECTURE / README 所有"当前进度"段落统一校准到本日 git log 真实状态
  - **原因**:HANDOFF 停留在 2026-06-04 步骤 3a 前夕,跟实际偏差过大,新 session 读完会做错假设

### 2026-06-04 — 路线图重构(步骤 1 完成后审视)

- **决策**:删除原步骤 5(WSL 验证),8 步合并为 7 步
  - **原因**:spike-001 已通过,步骤 1 也在 WSL 内完成,原步骤 5 是空壳
- **决策**:拆原步骤 3 为 3a(SQLite + session 持久化)和 3b(多项目 + UI + rig-core)
  - **原因**:原步骤 3 包含 4 件独立大事,任何一件卡住整个步骤都交付不了
- **决策**:事件协议用混合模式(高频 payload 判别 + 低频独立事件名)
  - **原因**:兼顾流式 token 性能和低频事件可 filter 性
- **决策**:SQLite 不提前到步骤 2,保持步骤 3a
  - **原因**:步骤 2 专注 agent loop 核心学习,加 SQLite 会膨胀范围
- **决策**:步骤 2 继续手写 reqwest,步骤 3b 才切 rig-core
  - **原因**:手写 agent loop 是核心学习价值,"先学再依赖"

### 2026-06-04 — 项目正式启动

- **决策**:自己写 agent core,不用 Anthropic Agent SDK 包装
  - **原因**:学习目标 + 控制粒度 + 厂商解耦
- **决策**:WSL 优先,Windows 不主动适配
  - **原因**:个人使用场景就是 WSL;Tauri 在 WSL 跑得通
- **决策**:每个 session 一个 git worktree
  - **原因**:多 session 并行 / 互不污染 / 切换瞬时
- **决策**:agent core 用 rig-core(后期切),前期手写
  - **原因**:先学再依赖,避免黑盒
- **决策**:MCP 只外暴露,内部通信不绕
  - **原因**:内部直接调 Rust 函数最快;外部用 MCP 生态杠杆
- **决策**:SQLite 是唯一存储
  - **原因**:单文件、零运维、FTS5 搜索、sqlx 异步
- **决策**:不做 workflow 编排(到 v2 再说)
  - **原因**:单 agent 体验没做透之前,workflow 是空中楼阁
- **决策**:不做团队协作、不做云端部署、不做移动端
  - **原因**:个人工具,这些是另一个产品的事
- **决策**:Skill / Memory / Role 共用 frontmatter loader
  - **原因**:三者载体都是 Markdown + YAML,加载时机不同;统一 loader 省重复代码
- **决策**:Agent Daemon 化(agent core 从 Tauri 进程拆出为独立 daemon)
  - **原因**:为多 channel 接入铺路(飞书不能依赖 GUI);GUI 重启不打断长跑任务;长跑任务稳定性。详见 [ARCHITECTURE.md §4](./ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路)
- **决策**:生成式 UI 走约束式(LLM 输出 JSON,前端按 type 渲染),默认关闭
  - **原因**:自由式(LLM 生成 HTML)沙箱难做、调试难;约束式够覆盖 80% 用例。详见 [BACKLOG.md 远期（v3+）段](./BACKLOG.md#远期v3暂不评估)
- **决策**:Channel Adapter 抽象(`Channel` trait,3 个实现:TauriGui / Feishu / Cli)
  - **原因**:多入口统一接口;新增 channel 不用改 agent core;测试友好。详见 [ARCHITECTURE.md §5](./ARCHITECTURE.md#5-决策channel-adapter-抽象为多入口铺路)
- **决策**:候选功能方向锁定(7 个),但暂不排优先级
  - **原因**:先把方向沉淀下来,实施前再按"价值/成本/依赖"评估。详见 [BACKLOG.md](./BACKLOG.md) 全章
  - 方向:输入层(图/@ /command) / Skill / 多层 Memory / 多角色+多模式+编排 / 生成式 UI / 飞书 / 云端同步

### 2026-06-04 — 方案 C:VPS 自托管 daemon(远期留接口)

- **决策**:v2 之后考虑在 VPS 上跑自托管 daemon,做多设备同步;**前期不展开,只留接口**
  - **原因**:个人用 + 学习目的,前期过度设计分布式拓扑是负担;但 Channel Adapter 协议要设计成 network-ready,worktree 路径要跨机器一致,否则后期改不动
- **决策**:Channel Adapter 协议必须走明文 JSON,载体无关(Unix socket / HTTPS / WSS 都能承载)
  - **原因**:不锁传输层,后期接 VPS 客户端不用改 agent core
- **决策**:worktree 路径统一用 XDG 标准 `~/.local/share/everlasting/worktrees/<project_hash>/<session_id>`
  - **原因**:跨机器接续时,worktree 路径必须可预测;XDG 标准是 Linux 共识
- **前期动作**(本决策已落地):
  - ARCHITECTURE §3 改 worktree 路径
  - ARCHITECTURE §5 Channel trait 注明 network-ready 约束
  - BACKLOG §4 列 v2 跨设备候选
- **后期展开**(v2 再说):
  - 多设备接续、配置/状态/session 列表同步、显式"工作树迁移"流程
  - 接续前置条件:必须 push 过 + 目标机器不能在跑 LLM
  - 详见 [BACKLOG §4 跨设备（v2 候选）](./BACKLOG.md#4-跨设备v2-候选)

### 2026-06-05 — 步骤 3b-1 follow-up 沉淀 (FU-1/2/3 项目决策)

- **FU-1 · cwd 简化为 `~/`**：3b-1 起 `ToolContext.cwd` 默认值从 `std::env::current_dir()` 改为 `~/`（`dirs::home_dir()`）。理由：LLM 工具调用产生的相对路径在跨 session 时能稳定解析。详见 [`docs/_archive/2026-06-3b-1/FOLLOW-UP.md`](../_archive/2026-06-3b-1/FOLLOW-UP.md)。
- **FU-2 · TS interface 字段 snake_case → camelCase**：Tauri 2 IPC 默认 `rename_all = "camelCase"`，前端 TypeScript interface 字段必须用 camelCase，**不要**在 TS 侧再写 snake_case 类型（如 `initialCwd` 不要写成 `initial_cwd`）。详见 [`docs/_archive/2026-06-3b-1/FOLLOW-UP.md`](../_archive/2026-06-3b-1/FOLLOW-UP.md)。
- **FU-3 · `pick_project_dir` 用 reka-ui 渲染 dialog**：Tauri command 不再负责弹原生 dialog，统一改为前端用 reka-ui 的 `Dialog` 组件（后端只暴露 path 校验）。详见 [`docs/_archive/2026-06-3b-1/FOLLOW-UP.md`](../_archive/2026-06-3b-1/FOLLOW-UP.md)。
