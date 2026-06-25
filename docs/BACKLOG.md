# BACKLOG — 候选功能与技术选型

> 7 个新功能方向(图片 / @ / command、Skill、Memory、角色/模式/编排、生成式 UI、飞书 IM、云端同步)的完整技术评估。
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

**建议实施顺序(从下到上)**:下层先做、上层后做,后者依赖前者的稳定。**跨层都需要关注:token 预算、安全边界、状态管理**(见 §8)。

---


## 1. 输入层扩展 → 已落地 (B2 @file 2026-06-17, B3 /command 2026-06-17),详见 ROADMAP §1.2;§1.1 多模态缓做 (ROADMAP §3)

---

## 2. Agent Skill 系统 → 已落地 (B4 2026-06-18),详见 ROADMAP §1.2

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

### 3.2 状态管理复杂度
- 多 channel 共享 session 状态 → 集中到 agent daemon
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
| 生成式 UI   | 按钮 action 越权                | Tauri command 白名单          |
| 飞书        | 消息内容外泄                    | 不在飞书存 session 历史       |
| 云端        | 第三方数据托管                  | 只 push 摘要 + 端到端鉴权     |

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
  §5 生成式 UI → §6 飞书 → §7 云端 → §4 可编排
```

---

## 4. 跨设备

**目标**:在多台设备上访问同一个 agent 工作环境。

**定位**(重要):
- **不是**多端协作(明确不做)
- **是**个人多设备使用(家里电脑、公司电脑、手机)
- 跟 §6 飞书的关系:飞书 = 消息通道;跟 §7 云端的关系:云端 = 状态镜像
- 本节 = "在另一台机器接着干"

**形态(暂定方向,留接口)**:
- VPS 自托管 daemon(用户已有国内 VPS,直连,不走 Cloudflare Tunnel)
- 集中式:VPS daemon 是唯一权威,本机 GUI 是 client
- 跨机器接续:worktree 走 git push/pull(不依赖 VPS 中转)

**前期已留的接口**(本决策已落地):
- Channel Adapter 协议走明文 JSON,载体无关(详见 [ARCHITECTURE §5](./ARCHITECTURE.md#5-决策channel-adapter-抽象为多入口铺路))
- worktree 路径用 XDG 标准 `~/.local/share/everlasting/worktrees/<project_hash>/<session_id>`(详见 [ARCHITECTURE §3](./ARCHITECTURE.md#3-决策每个-session-一个-git-worktree))
- 接续前置条件(早期原则):
  - 源机器必须 push 过(否则目标机器看不到最新)
  - 目标机器不能在跑 LLM(否则状态会变)
  - daemon 不自动 commit(避免过度设计),迁移时强制 commit + push

**实施范围**(技术细节,排期归 [ROADMAP §2 第四档](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排)):
- VPS daemon 部署文档(系统级 systemd,配置文件)
- 跨机器 session 列表同步(只读)
- "工作树迁移"流程(GUI 按钮)
- 多设备消息历史(只在源机器)
- 配置文件跨设备同步

**不做**:
- ❌ 多端同时编辑同一 session(冲突解决不做)
- ❌ VPS 持有 worktree 文件副本(隐私 + 存储)
- ❌ Cloudflare Tunnel / 第三方中转(国内 VPS 直连足够)
- ❌ 实时同步(只在显式触发时同步)

**风险**(提前识别):
- 数据过 VPS(虽然不持文件,元数据仍过 VPS)— 接受这个权衡
- 跨机器 worktree 路径冲突(用 session_id 隔离)
- 源机器断网时目标机器不能接续 — 设计选择,不是 bug

> 💡 详见 [IMPLEMENTATION §4 决策日志"方案 C"](./IMPLEMENTATION.md#4-决策日志)。本节功能在 [ROADMAP §2 第四档(最远远期)](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排),前期不展开实现细节。

---

## 5. 步骤 3b-1 实施后续(implementation follow-up)

> 这一节是步骤 3b-1(项目基础结构 + 顶部 Tabs UI)落地后留的"实施层面"小尾巴,不是新功能候选。技术债性质。完整列表 + 优先级见 [docs/_archive/2026-06-3b-1/FOLLOW-UP.md](../_archive/2026-06-3b-1/FOLLOW-UP.md),本节只记每条的工作量 + 触发时机 + 实际落地状态。

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

### 5.3 `pick_project_dir` 改成前端 reka-ui 渲染 dialog ⏸ 未实施(2026-06-07 状态)

- **现状**:Tauri native `pick_folder` dialog,WSLg 下走 GTK / xdg-desktop-portal,渲染是 linux GTK 风格。
- **用户偏好**:"本来期望 dialog 是由前端渲染的"(2026-06-05 session)。希望自渲染:HTML 树形目录 + 搜索框 + 文件图标。
- **修法**:PR2 frontend 写一个 `<ProjectDirPicker>` 组件,新加 `list_dir(path)` Tauri command 读子目录,前端自渲染树形 + 键盘导航。`pick_project_dir` 废弃。
- **工作量**:~150 行(frontend ~120 + backend `list_dir` ~30)。**中等优先**(UX 改善,不阻塞功能)。
- **关联**:PROPOSAL §5.4 (Q8v2 修正) + 用户偏好;FOLLOW-UP §FU-3。
- **状态**:⏸ 未实施,下次碰 project 创建流程时评估。

### 5.4 trellis 流程 follow-up(非实施)

- **FU-7**:PROPOSAL §9 给外部 LLM 的提问重写,改成"只读 PROPOSAL 就能答"形式。~30 行(下次发评审前一次性做)。
- **FU-8**:`check.jsonl` 加 "Tauri command arg camelCase" + "TS interface 字段命名"作为 PR 验收硬约束。~10 行。

> 💡 本节"实现"层面的 follow-up 跟 §1-§9"候选功能"性质不同 —— 那些是新功能,本节是已实施步骤的技术债。完整 follow-up 列表(含经验沉淀类的 4-6 条)见 [docs/_archive/2026-06-3b-1/FOLLOW-UP.md](../_archive/2026-06-3b-1/FOLLOW-UP.md)。

---

## 附录 A: 远期候选

> 📦 **已归档**:本节内容(357 行,7 项远期候选技术评估)于 2026-06-25 归档到 [`docs/_archive/backlog-appendix-A.md`](../_archive/backlog-appendix-A.md)。**只读不改**。如远期候选进展,新评估直接在 [ROADMAP.md §2](./ROADMAP.md#2-v2-路线图分类2026-06-10-重排) 中更新。
