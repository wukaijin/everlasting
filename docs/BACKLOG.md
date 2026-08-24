# BACKLOG — 候选功能与技术选型

> 7 个新功能方向(图片 / @ / command、Skill、Memory、角色/模式/编排、生成式 UI、飞书 IM、云端同步)的完整技术评估。
> **注**:飞书(§6 IM 通道)/ 云端同步(§7 云端状态同步)两节已于 2026-06-25 随附录 A 归档,见 [`docs/_archive/backlog-appendix-A.md`](./_archive/backlog-appendix-A.md);本文档正文不再包含这两节,下文相关引用均指向归档。
> **优先级 / 排期归 [ROADMAP.md](./ROADMAP.md),本文档只做技术评估**。
>
> 需求见 [DESIGN.md](./DESIGN.md),架构见 [ARCHITECTURE.md](./ARCHITECTURE.md),技术选型见 [TECH.md](./TECH.md),决策档案见 [IMPLEMENTATION.md](./IMPLEMENTATION.md),技术路线图见 [ROADMAP.md](./ROADMAP.md)。

---

## 0. 全局视角:这 7 个功能落在 5 个不同的层

> 💡 **关于版本号**:本文出现的 Phase 1 / Phase 2 指各**功能自身**的阶段(例:UI primitives Phase 1 必做 4 种、角色 Phase 1 不做编排)。**整体排期 / 优先级归 [ROADMAP.md §2 V2 路线图分类](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排)**,本文档不再维护排期。两套不重叠,按上下文区分。

```
┌─────────────────────────────────────────────────────┐
│ 触达层  §6 飞书 / §7 云端同步                        │  ← agent 在哪里被使用
├─────────────────────────────────────────────────────┤
│ 拓扑层  §4 多角色 / 多模式 / 可编排                  │  ← agent 怎么组织协作
├─────────────────────────────────────────────────────┤
│ 输出层  §5 生成式 UI                                 │  ← agent 怎么呈现结果
├─────────────────────────────────────────────────────┤
│ 指令层  §2 Skill / §3 多层 Memory                    │  ← agent 怎么被告知该做什么
├─────────────────────────────────────────────────────┤
│ 输入层  §1 图片 / @文件 / /command                   │  ← 用户怎么表达意图
└─────────────────────────────────────────────────────┘
```

> **注**:图中 §6 飞书 / §7 云端同步两节已于 2026-06-25 随附录 A 归档(见 [`docs/_archive/backlog-appendix-A.md`](./_archive/backlog-appendix-A.md) §6/§7),本文档正文不再展开。

**建议实施顺序(从下到上)**:下层先做、上层后做,后者依赖前者的稳定。**跨层都需要关注:token 预算、安全边界、状态管理**(见 §8)。

---


## 1. 输入层扩展 → 已落地 (B2 @file 2026-06-17, B3 /command 2026-06-17),详见 ROADMAP §1.2;§1.1 多模态缓做 (ROADMAP §3)

---

## 2. Agent Skill 系统

**状态**:已落地 (B4 2026-06-18),详见 [ROADMAP §1.2](./ROADMAP.md#12-路线图外完成)。

---

## 3. 跨 7 个功能的共同关注点

### 3.1 Token 预算管理
新功能都会吃 context window:
- 图片(每张 ~1000 tokens)
- @文件(大文件可能 5000+ tokens)
- 多层 memory(默认上限 2K)
- Role prompt(每个 role 1-2K)
- Skill(按需加载,但 LLM 选择可能不合理)

**缓解**:
- 统一 token 预算表
- 关卡 ⑤ (context 构造) 做硬卡
- 超限按优先级裁剪

> 📌 **待办(2026-08-14,源自 C7 brainstorm):memory 指令块(CLAUDE.md/AGENTS.md)窗口治理** — 与 `tools[]` 并列的窗口大头,但走不同路径。本仓库实测 CLAUDE.md 27KB≈6-7k token + AGENTS.md ~1k(`memory/loader.rs:204` 的 `load_for_session` 最多叠 4 层:用户/项目 × CLAUDE.md/AGENTS.md);挂了 `cache_control`(省钱)但**不省窗口**。等 C7(`.trellis/tasks/08-14-c7-tools-token-governance/`)的 tools 占比度量数据产出后,评估 memory 块治理优先级(手段是按相关性裁剪/分段加载,而非全量注入 — 与 tools 裁剪不同)。不纳入 C7(不同治理对象 + 不同手段)。
>
> **评估结论(2026-08-14,C7 live 烟测数据落地)**:首轮 context_input=17602 tok 中 tools=6773(38.5%,`turn_trace.tools_token`),memory 指令块估算 ~7-8k(≈42%;CLAUDE.md 27.8KB + AGENTS.md 3.2KB 复核)—— **tools + memory 合计约占首轮窗口八成,memory 块治理确认值得排期**,与 C7 Phase 2 的 D(Stub 注册,触发线 tools>15% 已过)同级候选。
>
> **进展(2026-08-15)**:D 已落地(C7D,`08-14-c7d-tools-stub-registration`,commit `bcf4187`:tools 首轮 6773→3677,占比 38.5%→26%),见 [ROADMAP §1.2](./ROADMAP.md)。memory 块(~7-8k)反超为首轮窗口最大单项,治理任务已建:`.trellis/tasks/08-15-memory-block-governance/`(WP1 度量先行 + WP2 手段 brainstorm)。
>
> **✅ 已完成(2026-08-15)**:memory 指令块治理落地(WP1 memory_token 度量 + WP2 分级注入 digest + `load_memory_sections` 按需拉取)。live 实测 memory **10124→2080(-79.5%)**,首轮 context **-47%**,双轮 cache 率不劣化,模型按目录主动拉节验证通过。见 [ROADMAP §1.2 memory-gov 行](./ROADMAP.md);剩余缓解手段(统一 token 预算表 / 关卡⑤硬卡)仍为候选,待多来源切片(tools/memory/图片/@文件)齐备后再评估。
>
> **🖼️ 图片切片就位(2026-08-16/17,B1)**:`turn_trace.images_token` 第三切片落地(口径 = 请求内全部图片块含历史重建;粘贴图前端 FileReader 估 / @图 imagesize 头探测,`(w×h)/750`)。三切片齐备,**统一 token 预算表 / 关卡⑤硬卡的评估前置已满足**,可排期。见 [ROADMAP §1.2 B1 行](./ROADMAP.md)。
>
> **✅ 已完成(2026-08-19,unified-context-budget)**:统一 token 预算表 + 关卡⑤硬卡落地(WP1 度量:`turn_trace` 加 `at_files_token` / `system_token` / `context_window` 三列 + 压缩口径统一为 system+tools+messages 发送部件加法;WP2 硬卡:`BUDGET_LINE_RATIO=0.95`×window 触发静默裁剪,裁尽仍超 fail-fast,落 `AuditKind::ContextBudgetTrim`;前端 TurnCard 预算构成条 + BudgetTrim chip)。本候选从"评估前置"转正式落地,见 [ROADMAP §1.2 unified-context-budget 行](./ROADMAP.md)。

### 3.2 状态管理复杂度
- 多 channel 共享 session 状态 → 集中到 agent daemon(daemon 化 2026-07 已落地,GUI + 浏览器共享同一 `everlasting-daemon` 进程的 session 池)
- 多 role/mode 切换 → 状态机
- 跨 session memory → SQLite 集中

### 3.3 安全边界

| 功能        | 风险点                          | 缓解                          |
|-------------|--------------------------------|-------------------------------|
| 图片        | 隐藏 prompt 注入                | 不渲染 LLM 之外的图           |
| @文件       | 路径遍历、敏感文件              | 工作目录校验、.env 黑名单     |
| /command    | 模板执行用户代码                | 模板只插值,不 exec            |
| Skill       | 第三方 skill 注入               | 文件位置隔离 + 显式 approve   |
| Memory      | 改文件不通知                    | banner 提示                   |
| 生成式 UI   | 按钮 action 越权            | B9 当前落地范围:selector(复用 ask_user_question)/ code_block(hljs + 复制)/ diff(复用 DiffView),**无独立 button + action surface**;独立 button + action 白名单推 D3 后期 |
| 飞书        | 消息内容外泄                    | 不在飞书存 session 历史       |
| 云端        | 请求流经中继(remote epic)      | 流经不落盘 + 配对码 60s 一次性 + device_token + shared_secret + per-IP 限速(10 次/分) |

### 3.4 实施顺序(供参考,排期归 [ROADMAP.md §2](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排))

> 实施顺序的**宏观视图**在 [ROADMAP §2 V2 路线图 4 档分类](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排);本节只讲**功能落地的依赖拓扑**(从下到上,下层先做):

```
下层先稳:
  §1 输入层(图片/@文件/command)→ §3 Memory → §2 Skill
                          ↓
中层:
  §4 多角色 / 多模式(无编排)
                          ↓
上层:
  §5 生成式 UI → §6 飞书 → §7 云端 → §4 可编排(§6/§7 已归档,见附录 A)
```

---

## 4. 跨设备

**目标**:在多台设备上访问同一个 agent 工作环境。

**定位**(重要):
- **不是**多端协作(明确不做)
- **是**个人多设备使用(家里电脑、公司电脑、手机)
- 跟 §6 飞书的关系:飞书 = 消息通道;跟 §7 云端的关系:云端 = 状态镜像(§6/§7 章节已归档到 [`docs/_archive/backlog-appendix-A.md`](./_archive/backlog-appendix-A.md) §6/§7,此处仅沿用其概念)
- 本节 = "在另一台机器接着干"

**形态**:
- **本地 daemon 化(✅ 已落地,2026-07,见 [ROADMAP §1.2 "daemon 化"](./ROADMAP.md#12-路线图外完成))**:agent core 已拆为独立 `everlasting-daemon` 进程(axum HTTP),Tauri GUI 作为瘦客户端 + 纯浏览器模式共享同一 agent core。这是跨设备的**基础**,但不等于跨设备 —— 本节未完成部分指**跨机器**接续。
- **VPS 中继 + 手机访问(✅ 已落地,remote epic S1~S6b,2026-08-11~08-13,merge `94828cb`)**:落地模型与早期计划(集中式"VPS daemon 唯一权威")不同 —— **PC daemon 是权威**(持全部 agent 数据/文件),云端 `everlasting-remote` **仅中继**(不持文件、不存 agent 数据,只存 nodes/devices/pairing_codes)。已交付:VPS 中继 + 配对 + PWA 手机访问(含移动端适配,08-13-mobile-chat-view / mobile-settings / mobile-polish),部署见 [REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md),E2E 验收见 [REMOTE-ACCESS-E2E.md](./REMOTE-ACCESS-E2E.md)
- **跨机器接续(❌ 未做,本节剩余主范围)**:worktree 迁移 / 多设备 session 同步仍未做 —— 数据仍只在 PC,手机经隧道访问的是 PC daemon

**daemon 化已提供的基础(本地)**:
- transport 抽象层(httpTransport 默认 / tauriTransport 逃生,`app/src/transport/`)—— 载体无关,跨设备时 VPS 远程也是 HTTP
- daemon 同源服务前端 SPA(ServeDir),浏览器已可访问本机 daemon
- `everlasting-daemon` bin 可独立部署(裸跑经 `scripts/daemon.sh`)
- worktree 路径用 XDG 标准 `~/.local/share/everlasting/worktrees/<project_hash>/<session_id>`(详见 [ARCHITECTURE §3](./ARCHITECTURE.md#3-决策每个-session-一个-git-worktree))

**跨设备待补(本节未做)**:
- 接续前置条件(早期原则):
  - 源机器必须 push 过(否则目标机器看不到最新)
  - 目标机器不能在跑 LLM(否则状态会变)
  - daemon 不自动 commit(避免过度设计),迁移时强制 commit + push

**实施范围**(技术细节,排期归 [ROADMAP §2 第四档](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排)):
- ✅ **VPS 中继部署文档(systemd + nginx,已交付)**:[REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md) + [`scripts/remote.sh`](../scripts/remote.sh) / [`scripts/deploy-remote.sh`](../scripts/deploy-remote.sh);E2E 验收见 [REMOTE-ACCESS-E2E.md](./REMOTE-ACCESS-E2E.md)
- ❌ 跨机器 session 列表同步(只读)
- ❌ "工作树迁移"流程(GUI 按钮)
- ❌ 多设备消息历史(只在源机器)
- ❌ 配置文件跨设备同步

**不做**:
- ❌ 多端同时编辑同一 session(冲突解决不做)
- ❌ VPS 持有 worktree 文件副本(隐私 + 存储)
- ❌ Cloudflare Tunnel / 第三方中转(国内 VPS 自建中继足够;此处排除的是**第三方**隧道服务,remote epic 中"VPS 即请求流中转"是自建中继,不在此列 —— 见上方形态)
- ❌ 实时同步(只在显式触发时同步)

**风险**(提前识别):
- 数据过 VPS(虽然不持文件,元数据仍过 VPS)— 接受这个权衡
- 跨机器 worktree 路径冲突(用 session_id 隔离)
- 源机器断网时目标机器不能接续 — 设计选择,不是 bug

> 💡 详见 [IMPLEMENTATION §4 决策日志"方案 C"](./IMPLEMENTATION/decisions.md)。本节功能在 [ROADMAP §2 第四档(最远远期)](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排),前期不展开实现细节。

---

## 5. 步骤 3b-1 实施后续(implementation follow-up)

> 这一节是步骤 3b-1(项目基础结构 + 顶部 Tabs UI)落地后留的"实施层面"小尾巴,不是新功能候选。技术债性质。完整列表 + 优先级见 [docs/_archive/2026-06-3b-1/FOLLOW-UP.md](./_archive/2026-06-3b-1/FOLLOW-UP.md),本节只记每条的工作量 + 触发时机 + 实际落地状态。

### ~~5.1 cwd 简化为 `~/` ✅ 已落地~~ (已落地 2026-06-06)(2026-06-06,commit `ef7cea8`)

- **原现状**:chat header 显示 cwd 用完整绝对路径(`/home/carlos/code/foo/backend`)。PROPOSAL §5.4 / Q5 决议是简化为 `~/foo/backend`,但 PR1 backend 没暴露 `home_dir` 给前端。
- **修法**:`configStore` 加 `homeDir` 字段(后端 `dirs::home_dir()` 经 Tauri command 暴露),frontend 写 `simplifyPath(cwd, homeDir)` 工具做前缀替换,`chatStore.simplifiedCwd` computed 派生给 ChatHeader 用。
- **落地状态**:`app/src/utils/path.ts` + `app/src/stores/config.ts` + `app/src/stores/chat.ts` `simplifiedCwd` computed 都已存在并使用。
- **关联**:PR3 commit `ef7cea8` "准备 pwd `~/` 简化数据通路" + FOLLOW-UP §FU-1(已 done,2026-06-06)。
- **状态**:✅ 已完成。

### 5.2 TS interface 字段 `snake_case` → `camelCase` ⏸ 保持现状(2026-06-07 决策)

- **现状**:`SessionSummary.project_id` / `current_cwd` / `created_at` 等字段是 snake_case 跟 Rust struct 序列化一致。TS interface 也跟着 snake_case,**非常规**。
- **决策(2026-06-07)**:**保持 snake_case,不引入 `#[serde(rename_all = "camelCase")]`**。
  - **理由**:(1) Rust 风格统一,少一层 rename;(2) 后端 8+ struct 都得加注解 + 前端 6+ interface 字段全改,工作量 ~50 行但**无功能收益**;(3) Tauri 2 IPC arg(不是返回值)有 camelCase 需求,这个**已修**(JS 端调 `invoke('create_session', { projectId })` 即可,FU-4 沉淀在 HACKING-wsl),跟 struct 字段命名是**两件事**。
  - **新写代码提醒**:Rust struct → TS interface 时直接复制字段名(snake_case);Tauri command 调用时,multi-word 参数用 camelCase。
- **关联**:FOLLOW-UP §FU-2(已决策,2026-06-07)。
- **状态**:⏸ 保持现状,显式决策已记录。

### 5.3 `pick_project_dir` 改成前端 reka-ui 渲染 dialog ⏸ 未实施(2026-06-07 状态;07-01 间接碰过;08-10 复核仍待做)

- **现状**:Tauri native `pick_folder` dialog,WSLg 下走 GTK / xdg-desktop-portal,渲染是 linux GTK 风格。
- **用户偏好**:"本来期望 dialog 是由前端渲染的"(2026-06-05 session)。希望自渲染:HTML 树形目录 + 搜索框 + 文件图标。
- **修法**:PR2 frontend 写一个 `<ProjectDirPicker>` 组件,新加 `list_dir(path)` Tauri command 读子目录,前端自渲染树形 + 键盘导航。`pick_project_dir` 废弃。
- **工作量**:~150 行(frontend ~120 + backend `list_dir` ~30)。**中等优先**(UX 改善,不阻塞功能)。
- **07-01 关联**:`fe91605 fix: 冷启动不再总是落到第一个项目` 间接碰过项目初始化路径(冷启动回退),但 dialog 仍未实施,留作下次碰项目创建流程时评估。
- **关联**:PROPOSAL §5.4 (Q8v2 修正) + 用户偏好;FOLLOW-UP §FU-3。
- **状态**:⏸ 未实施,下次碰 project 创建流程时评估。(2026-08-10 复核:daemon 化后 `pick_project_dir` 无 daemon route,浏览器模式项目添加走 fallback,状态不变)

### 5.4 trellis 流程 follow-up(非实施)

- **FU-7**:PROPOSAL §9 给外部 LLM 的提问重写,改成"只读 PROPOSAL 就能答"形式。~30 行(下次发评审前一次性做)。
- **FU-8**:`check.jsonl` 加 "Tauri command arg camelCase" + "TS interface 字段命名"作为 PR 验收硬约束。~10 行。

> 💡 本节"实现"层面的 follow-up 跟 §1-§9"候选功能"性质不同 —— 那些是新功能,本节是已实施步骤的技术债。完整 follow-up 列表(含经验沉淀类的 4-6 条)见 [docs/_archive/2026-06-3b-1/FOLLOW-UP.md](./_archive/2026-06-3b-1/FOLLOW-UP.md)。

---

## 附录 A: 远期候选

> 📦 **已归档**:本节内容(357 行,7 项远期候选技术评估)于 2026-06-25 归档到 [`docs/_archive/backlog-appendix-A.md`](./_archive/backlog-appendix-A.md)。**只读不改**。如远期候选进展,新评估直接在 [ROADMAP.md §2](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排) 中更新。
>
> 📌 **新候选(2026-08-24,08-24-btn-family-convergence 完工遗留)**:生成式 UI `ui-prim__btn` 家族(ButtonPrimitive/DiffPrimitive/CodeBlockPrimitive,LLM 渲染 per-action 变色语义)是否消费 `.btn` CSS 家族基类排版(仅吃 padding/字号/过渡,不吃变体色)。当时判定特例保留;若未来 ui-prim 按钮观感与主应用漂移成为问题,再评估。
