# BACKLOG — 候选功能与技术选型

> 7 个新功能方向(图片 / @ / command、Skill、Memory、角色/模式/编排、生成式 UI、飞书 IM、云端同步)的完整技术评估。
> **注**:飞书(§6 IM 通道)/ 云端同步(§7 云端状态同步)两节已于 2026-06-25 随附录 A 归档,见 [`docs/_history/backlog-appendix-A.md`](./_history/backlog-appendix-A.md);本文档正文不再包含这两节,下文相关引用均指向归档。
> **优先级 / 排期归 [ROADMAP.md](./ROADMAP.md),本文档只做技术评估**。
>
> 需求见 [DESIGN.md](./DESIGN.md),架构见 [ARCHITECTURE.md](./ARCHITECTURE.md),技术选型见 [TECH.md](./TECH.md),技术路线图见 [ROADMAP.md](./ROADMAP.md)。

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

> **注**:图中 §6 飞书 / §7 云端同步两节已于 2026-06-25 随附录 A 归档(见 [`docs/_history/backlog-appendix-A.md`](./_history/backlog-appendix-A.md) §6/§7),本文档正文不再展开。

**建议实施顺序(从下到上)**:下层先做、上层后做,后者依赖前者的稳定。**跨层都需要关注:token 预算、安全边界、状态管理**(见 §8)。

---


## 1. 输入层扩展

> 状态与排期归 [ROADMAP §1.2](./ROADMAP.md#12-路线图外完成)(B2 @文件 + B3 /command 已落地;多模态 **B1 已于 2026-08-16/17 落地**迁 §1.2,见 [ROADMAP §1.2 B1 行](./ROADMAP.md#12-路线图外完成))。

---

## 2. Agent Skill 系统

> 状态与排期归 [ROADMAP §1.2](./ROADMAP.md#12-路线图外完成)(B4 已落地)。

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

> 📌 **候选(2026-08-14 起,memory 指令块窗口治理)**:已评估并落地——`memory-gov`(分级注入 digest,指令块注入 -79.5%)与 `unified-context-budget`(统一 token 预算 + 关卡⑤硬卡)均已实现,进度见 [ROADMAP §1.2](./ROADMAP.md#12-路线图外完成) 对应行,此处不再重复。

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
| 生成式 UI   | 按钮 action 越权            | B9+ 已落地(2026-07-13):selector(复用 ask_user_question)/ code_block(hljs + 复制)/ diff(复用 DiffView)+ **D3 通用 button + D4 diff 应用 + `UiDiffApplied` 审计**;剩余 session 开关等细节见 [ROADMAP §1.2 B9+ 行](./ROADMAP.md#12-路线图外完成) |
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
- 跟 §6 飞书的关系:飞书 = 消息通道;跟 §7 云端的关系:云端 = 状态镜像(§6/§7 章节已归档到 [`docs/_history/backlog-appendix-A.md`](./_history/backlog-appendix-A.md) §6/§7,此处仅沿用其概念)
- 本节 = "在另一台机器接着干"

**形态**:
- **本地 daemon 化(✅ 已落地,见 [ROADMAP §1.2 "daemon 化"](./ROADMAP.md#12-路线图外完成))**:agent core 已拆为独立 `everlasting-daemon` 进程(axum HTTP),Tauri GUI 作为瘦客户端 + 纯浏览器模式共享同一 agent core。这是跨设备的**基础**,但不等于跨设备 —— 本节未完成部分指**跨机器**接续。
- **VPS 中继 + 手机访问(✅ 已落地,remote epic S1~S6b,见 [ROADMAP §1.2](./ROADMAP.md#12-路线图外完成))**:落地模型与早期计划(集中式"VPS daemon 唯一权威")不同 —— **PC daemon 是权威**(持全部 agent 数据/文件),云端 `everlasting-remote` **仅中继**(不持文件、不存 agent 数据,只存 nodes/devices/pairing_codes)。已交付:VPS 中继 + 配对 + PWA 手机访问(含移动端适配,08-13-mobile-chat-view / mobile-settings / mobile-polish),部署见 [REMOTE-DEPLOY.md](./REMOTE-DEPLOY.md),E2E 验收见 [REMOTE-ACCESS-E2E.md](./REMOTE-ACCESS-E2E.md)
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

> 💡 本节功能在 [ROADMAP §2 第四档(最远远期)](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排),前期不展开实现细节。

---

## 5. 步骤 3b-1 实施后续(implementation follow-up)

> 这一节是步骤 3b-1(项目基础结构 + 顶部 Tabs UI)落地后留的"实施层面"小尾巴,不是新功能候选。技术债性质。完整列表 + 优先级见 [docs/_history/2026-06-3b-1/FOLLOW-UP.md](./_history/2026-06-3b-1/FOLLOW-UP.md),本节只记每条的工作量 + 触发时机 + 实际落地状态。

### ~~5.1 cwd 简化为 `~/`(✅ 已落地 2026-06-06)~~

- **原现状**:chat header 显示 cwd 用完整绝对路径(`/home/carlos/code/foo/backend`)。PROPOSAL §5.4 / Q5 决议是简化为 `~/foo/backend`,但 PR1 backend 没暴露 `home_dir` 给前端。
- **修法**:`configStore` 加 `homeDir` 字段(后端 `dirs::home_dir()` 经 Tauri command 暴露),frontend 写 `simplifyPath(cwd, homeDir)` 工具做前缀替换,`chatStore.simplifiedCwd` computed 派生给 ChatHeader 用。
- **落地状态**:`app/src/utils/path.ts` + `app/src/stores/config.ts` + `app/src/stores/chat.ts` `simplifiedCwd` computed 都已存在并使用。
- **关联**:PR3 "准备 pwd `~/` 简化数据通路"(具体 commit 走 `git log`)+ FOLLOW-UP §FU-1(已 done,2026-06-06)。

### 5.2 TS interface 字段 `snake_case` → `camelCase` ⏸ 保持现状(2026-06-07 决策)

- **现状**:`SessionSummary.project_id` / `current_cwd` / `created_at` 等字段是 snake_case 跟 Rust struct 序列化一致。TS interface 也跟着 snake_case,**非常规**。
- **决策(2026-06-07)**:**保持 snake_case,不引入 `#[serde(rename_all = "camelCase")]`**。
  - **理由**:(1) Rust 风格统一,少一层 rename;(2) 后端 8+ struct 都得加注解 + 前端 6+ interface 字段全改,工作量 ~50 行但**无功能收益**;(3) Tauri 2 IPC arg(不是返回值)有 camelCase 需求,这个**已修**(JS 端调 `invoke('create_session', { projectId })` 即可,FU-4 沉淀在 HACKING-wsl),跟 struct 字段命名是**两件事**。
  - **新写代码提醒**:Rust struct → TS interface 时直接复制字段名(snake_case);Tauri command 调用时,multi-word 参数用 camelCase。
- **关联**:FOLLOW-UP §FU-2(已决策,2026-06-07)。
- **状态**:⏸ 保持现状,显式决策已记录。

### 5.3 `pick_project_dir` 改成前端 reka-ui 渲染 dialog ✅ 已落地(2026-09-03)

- **原始现状**:Tauri native `pick_folder` dialog,WSLg 下走 GTK / xdg-desktop-portal,渲染是 linux GTK 风格。
- **用户偏好**:"本来期望 dialog 是由前端渲染的"(2026-06-05 session)。希望自渲染:HTML 目录 + 搜索框 + 文件图标。
- **落地形态**(2026-09-02 `7a747379` + 2026-09-03 `09-03-dirbrowser-desktop-unify`):列表式 `DirBrowserModal`(单击进入 / `..` / 路径直达 / 隐藏目录开关,数据源 `browse_dir` IPC daemon+Tauri 双注册),并于 09-03 升级为**全模式统一入口**——桌面(Tauri)也走它,native `pick_project_dir` 命令 + `tauri-plugin-dialog` 依赖 + `dialog:default` 权限整链删除;同批补齐键盘导航(roving tabindex:方向键钳边移动 / Enter 原生进入 / 输入框不劫持 / 列表导航后焦点复位首行)。注册尾巴(去重 / unhide / create + focus,RULE-FrontProj-001)切换前后零变化。
- **未交付**:搜索框 / 目录名过滤(原始设想中唯一剩余项,后续按需另立)。
- **关联**:PROPOSAL §5.4 (Q8v2 修正) + 用户偏好;FOLLOW-UP §FU-3;task [09-03-dirbrowser-desktop-unify](../.trellis/tasks/09-03-dirbrowser-desktop-unify/)。

### 5.4 trellis 流程 follow-up(非实施)

- **FU-7**:PROPOSAL §9 给外部 LLM 的提问重写,改成"只读 PROPOSAL 就能答"形式。~30 行(下次发评审前一次性做)。
- **FU-8**:`check.jsonl` 加 "Tauri command arg camelCase" + "TS interface 字段命名"作为 PR 验收硬约束。~10 行。

> 💡 本节"实现"层面的 follow-up 跟 §1-§9"候选功能"性质不同 —— 那些是新功能,本节是已实施步骤的技术债。完整 follow-up 列表(含经验沉淀类的 4-6 条)见 [docs/_history/2026-06-3b-1/FOLLOW-UP.md](./_history/2026-06-3b-1/FOLLOW-UP.md)。

---

## 附录 A: 远期候选

> 📦 **已归档**:本节内容(357 行,7 项远期候选技术评估)于 2026-06-25 归档到 [`docs/_history/backlog-appendix-A.md`](./_history/backlog-appendix-A.md)。**只读不改**。如远期候选进展,新评估直接在 [ROADMAP.md §2](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排) 中更新。
>
> 📌 **新候选(2026-08-24,08-24-btn-family-convergence 完工遗留)**:生成式 UI `ui-prim__btn` 家族(ButtonPrimitive/DiffPrimitive/CodeBlockPrimitive,LLM 渲染 per-action 变色语义)是否消费 `.btn` CSS 家族基类排版(仅吃 padding/字号/过渡,不吃变体色)。当时判定特例保留;若未来 ui-prim 按钮观感与主应用漂移成为问题,再评估。

---

## 附录 B: 群聊共识候选(2026-09-05)

> 来源:headless 群聊实跑(session `082add5c-a98a-431a-936b-a764eea54ce5`,moderator MiniMax-M3,产品/前端/后端/测试/新用户/安全六视角混编 3 模型,全程读仓库求证)。参与者各自核码后的优先级建议是**参考**,排期归 [ROADMAP §2](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排)。流程缺陷(非功能候选)另见 [BUGLIST-group-chat.md](./BUGLIST-group-chat.md)(GC-x 编号);本附录 N-x 为候选临时编号,立项进 ROADMAP 时换正式编号。
> 收录规则:只收「现有 docs 无对应条目」的候选;群聊重申既有条目的(A5/A6、A4+、跨设备清单、移除项确认)不重复收录。
> **2026-09-06 增补**:第二场 live 群聊(session `eb14d2df`,议题即三处求证衍生修复)的共识——止损包(C1.1 ask-free / C1.2 token 预算)、证据链(C2 结构化 summary)、回归闸(C3)、「假注释毒数据」RULE——属群聊**内部改进线**,依赖矩阵与推进记账见 [GROUP-CHAT-API-ROADMAP.md §6](./GROUP-CHAT-API-ROADMAP.md),不在本附录重复立行。

### B.1 新增候选(grep 全 docs 无既有对应)

| 编号 | 候选 | 群聊建议 | 主张视角 | 一句话依据(参与者的核码结论) |
|------|------|---------|---------|-------------------------------|
| ~~N1~~ | ~~首次引导 3 步向导 + 报错分级提示~~ | **✅ 2026-09-15 交付**(方向调整:不做重向导——轻页面引导(空状态四分态检测卡 + 错误行测试连接)+ 内置诊断/引导/配置 LLM 三件套 skills + 只读诊断双工具;task `09-15-n1-onboarding-skills`,见 [ROADMAP §1.2](./ROADMAP.md)) | 产品+新用户 | 空状态仅一句「开始对话」,新手不知先配 provider;401/529 裸英文报错死胡同;形态:配 provider → 测试连接 → 开聊(per-row 测试按钮已存在可复用) |
| ~~N2~~ | ~~checkpoint / revert 闭环~~ | **✅ 2026-09-20 交付**(悬空快照路线落地,含群聊评审 15 结论回填:API 级致命伤修正/基线轮首/D3 级联/TOCTOU 双窗口/ref 泄漏全修;task `09-20-n2-checkpoint-revert`,见 [ROADMAP §1.2](./ROADMAP.md)) | 产品(后端/前端/安全/测试四方修正) | 真痛点但成本被证伪:diff 展示层现成(`git/diff.rs` session 分支),缺 per-turn 文件基线(新写入路径)+ 多 session 原子化;落地路径 = turn 边界 auto-commit + revert=reset;约束:revert 仅 UI 触发、不进 agent 工具、走 dangerous 通道 + audit 归因;前置 = N4 |
| ~~N3~~ | ~~新项目冷启动 `/init` + 轻量 repo map~~ | **✅ 2026-09-18 交付**(范围用户裁定收窄:唯一交付物 = `<project>/AGENTS.md` 内嵌 repo map 章节,不建目录骨架/项目级 EVERLASTING.md/.gitignore;形态 = GlobalBuiltin 第四件 skill `init`,`/` 面板即字面 `/init`,当前 session LLM 驱动零机制新增;幂等 = marker 区块增量更新【`edit_file` 结构保障】+ 纯手写保护,task `09-18-n3-project-init`,见 [ROADMAP §1.2](./ROADMAP.md)) | 产品 | 4 个指令文件手写、每 session 靠 grep 摸地形;B5 memory digest 只优化注入成本,不解决首印象 |
| ~~N4~~ | ~~长会话渲染虚拟化~~ | **✅ 2026-09-20 交付**(路线裁定 = 自渲染真虚拟化 `useVirtualizedMessages`(弃 content-visibility / vue-virtual-scroller);PR0 判据换尺中立测试基建先行保回归;锚定语义全迁移 + flatten 打平 + data-seq 命令化 + flash/run-enter 动效;群聊评审 12 结论回填;task `09-19-n4-render-virtualization`,见 [ROADMAP §1.2](./ROADMAP.md)) | 前端 | `MessageList.vue` 裸 v-for 全量 DOM,无 IntersectionObserver/content-visibility;路线(content-visibility vs 真虚拟化)等 N9 基准后定 |
| ~~N5~~ | ~~sandbox fail-open 审计可区分~~ | **✅ 2026-09-26 交付**(并入 N6 同任务;tool_executed 终态归因字段 + 词表双层类型化 + escalation/worker 两实锤缺陷收口 + capability 三维,task `09-26-sandbox-failopen-audit`,见 [ROADMAP §1.2](./ROADMAP.md)) | 安全+测试 | 审计不区分 sandboxed / failed-open 执行,事后归因混淆 |
| ~~N6~~ | ~~沙盒测试 CI 静默 SKIP 门禁~~ | **✅ 2026-09-26 交付**(`require_sandbox_cap()` 单源 + env `EVERLASTING_SANDBOX_TESTS_REQUIRED=1` ci.yml rust job 硬门禁,同任务) | 测试 | `sandbox/tests_sandbox.rs` 4 处 `eprintln!("SKIP")` 后照常通过——无 Landlock/seccomp 主机(macOS runner)沙盒覆盖率=0;修法:`#[cfg(target_os="linux")]` 强制门禁或 Linux docker-runner |
| ~~N7~~ | ~~DiffView 增强(行级高亮 / side-by-side / 按文件折叠)~~ | **✅ 2026-09-27 交付**(勘察修正:按文件折叠已存在;实做 = 行内 word-diff 高亮 + side-by-side 双栏 + 工具行切换/localStorage/窄屏降级 + `allowSplit` prop + EditFileCard 随批升级;群聊评审 14 结论回填;task `09-26-diffview-enhance`,见 [ROADMAP §1.2](./ROADMAP.md)) | 前端 | 变更信任闭环的审阅质量面;`DiffPrimitive` 已证明可被任意入口挂载 |
| N8 | 混沌 / 故障注入冒烟(`chaos-smoke.sh`) | P2 | 测试 | SIGKILL daemon / SSE 中断 / DB 锁 / disk full;RULE-PERSIST-001 是被动恢复,无主动注入 |
| ~~N9~~ | ~~性能基准(cargo criterion + vitest bench)~~ | **✅ 2026-09-19 交付**(bench feature 门 + criterion 三 bench【harness 开销 / DB 内存与 disk 分档 / SSE 降级双路径】+ Playwright 真浏览器 F1 前端基准 + 真实负载画像 profile JSON 单一出处 + CI 编译门(数字不进门禁,用户裁定);规划评审 12 结论采纳;基线与复跑口径落 spec [perf-baseline](../.trellis/spec/backend/perf-baseline.md),task `09-19-n9-perf-benchmark`,见 [ROADMAP §1.2](./ROADMAP.md)) | 测试 | agent loop 启动 P99 / SSE 首字节 / 10k message DB 查询均无基准 |
| N10 | 本地性一键导出 / 清除(DB + outputs spill + 日志) | P2 | 安全 | 本地优先是卖点但缺出口;跨设备同步(BACKLOG §4)上线前备好 |
| N11 | 可控灰度 / 回滚 + 崩溃收集(minidump / 符号化) | P2 | 测试 | `daemon.sh restart` 硬切、无版本门 / kill switch;崩溃现场全丢 |

### B.2 既有条目的增量修正(不新增行,仅记注记)

- **B10 飞书([ROADMAP §2 第四档](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排))**:群聊建议收窄首发形态为「任务完成通知」,不做完整 IM 对话——F2/F6 的完成通知目前仅 GUI 内 toast,而 PWA 使用场景恰恰人不在 PC 前。
- **F6 余留增强(同上 ROADMAP F6 行「系统级通知 / unread 持久化 / 等待态心跳按需另立」)**:群聊确认系统级通知价值被 F2/F6 使用场景放大,优先级信号向上。

### B.3 翻案候选(与既有用户决策冲突,需产品裁定后才可立项)

- **远程隧道来源降级**(群聊安全+测试双视角 P0):tunnel 请求无 scope/来源标记,经 loopback 转发后与本机请求完全等价——手机丢失或 device_token 泄露 = 整台 PC 的 daemon 权限;建议 tunnel 请求加来源标记 + dispatcher 侧默认降级(远程禁写类工具 / 敏感命令拒绝),事前降级 > 事后吊销(吊销机制已有)。
  ⚠️ **冲突**:[REMOTE-ACCESS-ROADMAP.md P3.3](./REMOTE-ACCESS-ROADMAP.md) 记录「远程读写不对称」已于 **2026-08-16 用户决策取消**——PWA 全权为最终形态,不做远程权限分层。群聊新论据(漏洞窗口不对称:checkpoint 失败最坏无损、隧道旁路静默外泄才暴露)是否构成翻案理由,由用户裁定;维持原决策则本条关闭。

### B.4 待决决策点(非功能项)

- **P0 资源排序**(若 N1 / B.3 / N2 立项撞期):群聊建议 首次引导 > 隧道降级 > checkpoint——留存漏斗 > 安全裸奔 > power feature。
- **「session 不 auto-commit」旧决策**:N2 的前置 ADR(跨设备 §4 亦有「迁移时强制 commit」关联语义),翻案与否单独决策。
- ~~**虚拟化路线**:content-visibility vs 真虚拟化(vue-virtual-scroller / 自渲染),等 N9 基准数据后定。~~ → ✅ 已决(2026-09-20,N4 交付):自渲染真虚拟化 `useVirtualizedMessages`,content-visibility / vue-virtual-scroller 两案弃。

---

## 附录 C: 调研衍生候选(2026-09-27)

> 来源:harness 对比调研两场——DeepSeek Harness(dsh)([2026-09-27](./_history/research/deepseek-harness-survey.md))+ MiniMax Code(mcode)([2026-09-29](./_history/research/minimax-code-survey.md))。与附录 B 同规则:N-x 为候选临时编号(续 B 编号),**立项进 ROADMAP §2 时换正式编号**;排期归 [ROADMAP](./ROADMAP.md),本文档只记评估。
> 收录立场:两者均只借鉴设计不引入依赖(TS 生态无法直接复用;mcode 开源部分是源码快照非开发仓库);**所有条目立项前须先做专项调研**(C.2 逐项列出),本附录只记一句话价值与调研范围,不预支结论。

### C.1 新增候选

| 编号 | 候选 | 建议优先级 | 视角 | 一句话依据 |
|------|------|-----------|------|-----------|
| ~~N12~~ | ~~agent-loop 语义吸收三小件(重试复用装配 / 装配期取消不落半准入 / 压缩恢复防死循环世代判定)~~ | **✅ 2026-09-27 交付**(按调研收窄范围单 PR 三件收口:件③无进展熔断主体——`CompactionRegistry` 第二维度,连续 2 次「摘要 Applied 但水位未推进 ‖ 总量未降」→ 粘性跳过摘要直达机械,水位推进即解除,与既有「连续 3 次失败」熔断正交;件②摘要落库前取消检查(`SummaryOutcome::Cancelled` 丢弃已生成摘要,不计任何熔断);件① `retry_open` 零重装配测试锚。两处口径修正经 check 复核成立:`tokens_after` 取折叠后持久值(机械丢组只裁内存 wire 不落库)、同形比较补 `request_overhead`;task `09-27-n12-compaction-noprogress-breaker`,见 [ROADMAP §1.2](./ROADMAP.md);调研工件 [09-27-n12-agent-loop-semantics-research](../.trellis/tasks/archive/2026-09/09-27-n12-agent-loop-semantics-research/)) | 后端 | dsh 验证过的收口语义,前两条对照 A5+/C1 补不变量,第三条正对 C3+ 压缩 + 关卡⑤硬卡「压缩后仍超预算」的组合场景 |
| N13 | 工具结果 post-execute 横切钩子(可观测 / 改写 / 附加上下文) | P2 | 后端 | 16 关卡里没有「结果落地前」的最后一道横切面;泄漏脱敏、C2+ 循环检测注入都可挂这里 |
| N14 | DB schema 版本纪律补强(定稿/发布分离 + 单一真源常量 + 相邻迁移链 + 无凭据 spec 测试) | P2 | 测试 | 现迁移纪律靠 `database-guidelines` 约束,缺 finalized ≠ released 的显式记录与机械校验 |
| N15 | LSP 工具(代码导航 / 引用 / 诊断) | P1-P2 | 后端 | dsh 有而本项目没有的最大能力实 gap,对 coding agent 质量影响直接;但依赖与进程模型复杂,调研后定档 |
| N16 | 会话 fork(带种子从任意点分叉) | P2-P3 | 产品 | 对 D3 edit/resend + handoff 是自然延伸;需先厘清与 N2 checkpoint 的重叠度 |
| N17 | Claude Code / Codex 作子 agent 宿主(hooks 桥) | P3 | 拓扑 | 与 dispatch_subagent 自定义 worker 路线互补的生态位思路,依赖外部 CLI,优先级最低 |
| N18 | 会话级 browser 自动化工具(Playwright 型) | P2 | 前端+后端 | 双 harness 印证(dsh browser-use / mcode browser 单工具 24-action 分发 + 快照分页 + 每回合可视化图片上限 + 不支持任意 JS 执行的安全面);everlasting 浏览器面为零(调研 [minimax-code-survey §3.8](./_history/research/minimax-code-survey.md)) |
| ~~N19~~ | ~~mcode 语义吸收小件包(edit_file fuzzy 匹配 CJK 保真自查 / diff 大小限界 / 后台任务 SIGTERM 宽限终止 / 父进程死亡守卫 / 并行批次取消配对补齐)~~ | **✅ 2026-09-29 交付**(按调研收窄范围单 PR 三件收口 + 测试锚:件④缺口 A 主体——daemon graceful shutdown 链补 `background_shells.kill_all()`(drain 后、Immediate 档、log-only,修 Thin/sidecar/`daemon.sh stop` 优雅退出孤儿化后台 shell,SIGTERM 集成测试共享 `SIGNAL_TEST_MUTEX`);件③——前台/后台两处 `kill_and_collect` 两段式 SIGTERM→宽限(默认 3s,env `EVERLASTING_SHELL_KILL_GRACE_MS` 覆盖)→SIGKILL,单杀路径(shell_kill tool/前台取消/前后台超时)宽限档、批量路径(`kill_all_for_session`/`kill_all`)恒 0,RULE-E-002「必死」不变(最坏 = 现状 + 3s);件⑤——`finalize_turn` 取消臂差集补齐未执行 tool_use 的 synthetic is_error tool_result(`FinalizeFrame.tool_calls` 承载,真×N→synthetic×M→hint Text 末尾,幂等零行为变化,worker 内存态同补齐 DB 不落,wire 层自愈回归本职「warn 出现即回归信号」);件①——edit_file CJK 全角字符往返保真测试锚;task `09-29-n19-mcode-semantics-impl`,见 [ROADMAP §1.2](./ROADMAP.md);调研工件 [09-29-n19-mcode-semantics-research](../.trellis/tasks/archive/2026-09/09-29-n19-mcode-semantics-research/)) | 后端 | pi-mono 补丁账本实锤级教训(fuzzy edit NFKC 归一化破坏全角字符、20k 行 diff 167s→0.2s;调研 [minimax-code-survey §3.2-3.5](./_history/research/minimax-code-survey.md));自查先行,按 N12「先核验现有行为再收口」同形推进 |
| ~~N20~~ | ~~ACP (Agent Client Protocol) 接入:`everlasting-acp` shim bin(everlasting 作为 Zed 等 ACP 客户端的 agent 后端)~~ | **✅ 2026-09-30 交付**(MVP 4 PR:PR1 生命周期[initialize 能力声明 / session new/load/list / cwd 词规整 resolve]→ PR2 翻译层[SSE 全程保持连接 + Last-Event-ID 重连 + ChatEvent→update 映射 + stop_reason 值域表 + prompt 全链 queued 拒绝]→ PR3 交互桥[request_permission 反向请求环 reject→deny rename / cancel_chat / set_mode+current_mode_update]→ PR4 收口[session/load 重放官方时序 / 11 例真子进程集成测试 / CI 两行 / 本文档];权限选项同名同义、capability 全按协议合法降级;text-only、群聊不达、daemon 不自动拉起为有意取舍;**follow-up**:在途 permission ask 恢复面(pending_interaction 增 Permission 变体,daemon 侧 ~20-40 行)/ 图片(image=true + ContentBlock 转换)/ MCP over ACP / daemon 自动拉起;契约与 Zed 注册指引见 [docs/ACP.md](./ACP.md),task `09-30-n20-acp-shim-mvp`) | 后端 | 调研结论全数兑现(2026-09-29 调研,架构零改道);实测坑:daemon 沙箱 shell 层对纯读/项目内写命令静默放行不弹 ask(landlock 边界即安全边界),live 冒烟需用需网络类命令触发 |

### C.2 立项前专项调研要求

> 每条 = 一个独立调研任务(可走 Trellis research),产出「现状缺口 + 改动面 + 取舍」三段,结论回填本附录后再立项。

- ~~**N12**:逐条对照 16 关卡与 `agent-loop-architecture` 系列 pattern spec,核验现有实现(A5+ `send_with_retry` 的整轮重发粒度、C1 取消在装配期的落库行为、C3+ 压缩失败后的重试路径)与三条语义的差距及改动面;明确「表面替换世代」在本仓库的等价物(cutoff_seq 水位?)。~~ → ✅ 调研完成(2026-09-27):结论见 task `09-27-n12-agent-loop-semantics-research` 的 `research/n12-semantics-gap-analysis.md`(三段式 + 代码行号证据;注:A5+ 实现名为 `retry_open` 非 `send_with_retry`)。
- **N13**:盘点现有横切点(权限闸 / C2 循环检测 / memory-gov / C6 输出截断)与钩子位置选型(落在关卡⑩ tool 执行后、消息落库前的哪一缝);评估对 `tool-contract` spec 的契约影响与拒绝语义(deny 是否仍流向下游,dsh 的 projectContent 可见拒因设计)。
- **N14**:盘点现有 migration 机制(sqlx migrate?手写?),设计「定稿/发布双记录」在 SQLite schema 语境的映射(版本常量放哪、发布证据 tag 怎么记)、无凭据 spec 测试的落点(对照 dsh `doc-standard.spec.ts`)。
- **N15**:Rust LSP client 生态调研(crate 选型 vs 起外部 binary:rust-analyzer / gopls / typescript-language-server 等)、工具面设计(暴露哪些能力:定义跳转 / 引用查找 / 诊断,与 read_file / grep / glob 的分工)、多 project 进程生命周期与资源上限、token 成本与 C7 tools[] 治理的接入。
- **N16**:fork 语义定义(种子 = 截止 seq + 记忆状态?)与 D3 / handoff / N2 checkpoint 的关系矩阵;存储面(新 session 行 + 事件前缀复制 vs 引用)。
- **N17**:外部 CLI 的授权 / 进程 / 计费模型,与现有 subagent 契约(`subagent-runs-schema`)的融合方式;仅在 N15 落地且 subagent 面稳定后再评。
- **N18**:架构决策(daemon Rust 侧驱动 vs 前端 Node 侧驱动 vs 独立辅助进程—— everlasting 前端 E2E 已有 Playwright 真浏览器经验但 daemon 侧无浏览器栈)、会话级生命周期与资源上限、安全面设计(禁任意 JS 执行 / 敏感操作确认 / mcode `safety.requiredNextTool` 强制下一步的等价物)、token 成本(快照分页 / 可视化观察图上限)与 C7 tools[] 治理接入。
- ~~**N19**:逐件核验 everlasting 现状——edit_file fuzzy 路径有无归一化破坏 CJK、edit/diff 有无大小限界(Myers 二次方卡死风险)、后台 shell 停止是否直 SIGKILL、宿主崩溃后后台进程组回收现状、并行只读批取消时 tool_use/result 配对完整性;按 N12 同形「件①测试锚 / 件②③收口」拆分单 PR 收口。~~ → ✅ 调研完成(2026-09-29):结论见 task `09-29-n19-mcode-semantics-research` 的 `research/n19-semantics-gap-analysis.md`。五件判定:①缺陷形态不存在(无 fuzzy 路径/零归一化,精确字节匹配)→测试锚;②三层结构天然规避(edit 无 diff 计算/libgit2 C+untracked 64KiB cap/前端 N7 双守卫)→记注不实施;③真实缺口(全终止路径直 SIGKILL,RULE-E-002)→两段式 SIGTERM→3s 宽限→SIGKILL;④真实缺口且比预想实——**daemon graceful shutdown 链漏 kill_all**(只挂 GUI Full 的 RunEvent::Exit,Thin/sidecar/daemon.sh 优雅退出也孤儿化后台 shell,崩溃面零守卫),缺口 A(shutdown 链补 kill_all ~10-20 行)为实施主体,缺口 B(崩溃面 lease/PDEATHSIG)记注 C.3 等 N11;⑤形态存在但有下游自愈(serial 执行中途取消部分落库+悬空,靠 wire 层每 turn 注入 synthetic 兜住;send 阶段取消已全量补齐)→finalize_turn 取消臂补齐差集对齐层次。实施范围建议:④A+③+⑤+①测试锚单 PR。
- ~~**N20**:ACP 协议面(v1 stable)方法/事件/枚举与 daemon 现有客户端 API/SSE 的全量映射;Rust crate 选型(官方 crate 成熟度 vs 手写);Zed 注册路径;shim 生命周期(daemon 拉起策略)与权限桥语义核对。~~ → ✅ 调研完成(2026-09-29):结论见 task `09-29-n20-acp-integration-research` 的 `research/acp-integration-analysis.md`(三段式:缺口 5 项 / 改动面 4 PR 拆解 / 取舍 8 项;判定 = 可立项,shim bin + daemon 零改动,官方 crate 1.x)。

### C.3 未立项记注(不单开候选行)

- **「模型可见即已记录」不变量**:dsh 的验收表述(模型请求可从持久日志完整重建)——不单独立项,作为 N8 混沌冒烟 / E2 TracePanel 回放扩展 / N14 纪律的验收标准引用。
- **沙盒 seam 化**(`SandboxExecutor` trait 边界):现有 spec 已接近薄 trait;不立项,仅在 bwrap / 网络白名单(BACKLOG 余留)动工时保持边界、不为远程形态预做抽象。
- **egress 代理沙箱参照**(mcode):bwrap `--unshare-net` 全断 + 宿主 socat 代理桥开洞 + seccomp 只拦 `socket(AF_UNIX)` 创建 + 凭据假文件替换(maskedFileBinds),域名过滤妥协在宿主代理层的取舍有明文记录——正是 sandbox-executor 余留「bwrap 增强档 / 网络白名单」的完整落地参照,动工时读 [minimax-code-survey §3.1](./_history/research/minimax-code-survey.md),不重新摸索;NetPolicy 三态与代理桥可拼。
- **B10 飞书架构蓝图**(mcode):feishu/telegram/wechat 三通道(channels:adapter-registry + access-control 独立策略/存储 + channel binding + permission-bridge IM 内权限审批 + questionnaire-bridge 问卷回流 + 媒体归一)——B10 收窄形态评估时的现成参照;权限审批回流 IM 是值得注意的形态。
- **F6 通知语义清单**(mcode):四事件(turn-complete/turn-failed/permission-required/question-required)+ when 三档(unfocused/always/never)+ method 多路回退(auto/osc9/osc777/bel)+ 前台焦点抑制与去重——F6 余留系统级通知实施时直接抄作业。
- **A2+ 判定深化参照**(mcode):bash 判定 AST 级文件族(wrapper-unwrap 剥命令包装 / slow-command-scan / windows-native-delete + 本地 dangerous-patterns 词表),比 P1+P2 深一层;云分类器不跟进,本地规则词表可扩充参照。
- **task_append 语义**(mcode):向运行中后台任务/subagent 注入后续工作(activated/steered/duplicate 三态)——everlasting L1 APPEND 仅用户通知面、dispatch_subagent 无中途注入,subagent/L1 线增量参照。
- **压缩「归档可回读」思路**(mcode):老 tool result 归档为引用 + agent 需要时 read 回读(ToolResultArchiver),比机械丢组多一个「瘦身不销毁」层次;C3+ 后续增强候选,与 N12 无进展熔断正交(注:mcode 无无进展熔断,everlasting 领先)。
- **N15 优先级修正注**(mcode):mcode 无 LSP 工具(dsh 有)——「行业标配」证据削弱,但真实能力 gap 不变,定档仍待 N15 专项调研裁决。
- **后台 shell 崩溃面孤儿(N19 件④缺口 B,2026-09-29 调研衍生)**:daemon SIGKILL/panic 下后台进程组永久孤儿(无 PDEATHSIG/lease 守卫,max_runtime 计时器随 daemon 进程死,registry 纯内存重启失忆)。优雅路径缺口 A 已并入 N19 实施;崩溃面守卫(PDEATHSIG 只护一层且有线程归属语义 / pipe-lease wrapper 完整但与 sandbox pre_exec·env_clear·进程组语义全交互)不单独立项——真实发生频率未知(无崩溃收集,N11 未做),等 N11 落地有数据或实际撞到再评估,方案两方向已记 N19 调研 `件④改动面`。
- **tracked diff patch 无 per-file 上限(N19 件②残余,2026-09-29)**:`git/diff.rs` tracked delta 的 `Patch::to_buf` 全量构建后才被下游裁剪(C6 截断管 LLM 输出面,前端另有守卫),构建期 CPU/内存无闸。libgit2 C 实现 + 非热路径,风险低;mcode 的 20k 行 167s 教训是 TS 归一化+diff 叠加,形态不同。撞到 TurnCard「本轮 diff」超大仓库卡顿再评估 per-file 早退。
