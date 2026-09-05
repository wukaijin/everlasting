# BACKLOG — 候选功能与技术选型

> 7 个新功能方向(图片 / @ / command、Skill、Memory、角色/模式/编排、生成式 UI、飞书 IM、云端同步)的完整技术评估。
> **注**:飞书(§6 IM 通道)/ 云端同步(§7 云端状态同步)两节已于 2026-06-25 随附录 A 归档,见 [`docs/_history/backlog-appendix-A.md`](./_history/backlog-appendix-A.md);本文档正文不再包含这两节,下文相关引用均指向归档。
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

> 💡 详见 [IMPLEMENTATION §4 决策日志"方案 C"](./IMPLEMENTATION/decisions.md)。本节功能在 [ROADMAP §2 第四档(最远远期)](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排),前期不展开实现细节。

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
| N1 | 首次引导 3 步向导 + 报错分级提示 | **P0** | 产品+新用户 | 空状态仅一句「开始对话」,新手不知先配 provider;401/529 裸英文报错死胡同;形态:配 provider → 测试连接 → 开聊(per-row 测试按钮已存在可复用) |
| N2 | checkpoint / revert 闭环 | P1 上半(**非** P0) | 产品(后端/前端/安全/测试四方修正) | 真痛点但成本被证伪:diff 展示层现成(`git/diff.rs` session 分支),缺 per-turn 文件基线(新写入路径)+ 多 session 原子化;落地路径 = turn 边界 auto-commit + revert=reset;约束:revert 仅 UI 触发、不进 agent 工具、走 dangerous 通道 + audit 归因;前置 = N4 |
| N3 | 新项目冷启动 `/init` + 轻量 repo map | P1 | 产品 | 4 个指令文件手写、每 session 靠 grep 摸地形;B5 memory digest 只优化注入成本,不解决首印象 |
| N4 | 长会话渲染虚拟化 | P1(rewind 前置) | 前端 | `MessageList.vue` 裸 v-for 全量 DOM,无 IntersectionObserver/content-visibility;路线(content-visibility vs 真虚拟化)等 N9 基准后定 |
| N5 | sandbox fail-open 审计可区分 | P1 | 安全+测试 | 审计不区分 sandboxed / failed-open 执行,事后归因混淆 |
| N6 | 沙盒测试 CI 静默 SKIP 门禁 | P1 | 测试 | `sandbox/tests_sandbox.rs` 4 处 `eprintln!("SKIP")` 后照常通过——无 Landlock/seccomp 主机(macOS runner)沙盒覆盖率=0;修法:`#[cfg(target_os="linux")]` 强制门禁或 Linux docker-runner |
| N7 | DiffView 增强(行级高亮 / side-by-side / 按文件折叠) | P2 | 前端 | 变更信任闭环的审阅质量面;`DiffPrimitive` 已证明可被任意入口挂载 |
| N8 | 混沌 / 故障注入冒烟(`chaos-smoke.sh`) | P2 | 测试 | SIGKILL daemon / SSE 中断 / DB 锁 / disk full;RULE-PERSIST-001 是被动恢复,无主动注入 |
| N9 | 性能基准(cargo criterion + vitest bench) | P2(与 N4/N7 联动) | 测试 | agent loop 启动 P99 / SSE 首字节 / 10k message DB 查询均无基准 |
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
- **虚拟化路线**:content-visibility vs 真虚拟化(vue-virtual-scroller / 自渲染),等 N9 基准数据后定。
