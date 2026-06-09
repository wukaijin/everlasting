# BACKLOG — 候选功能与技术选型

> 7 个新功能方向(图片 / @ / command、Skill、Memory、角色/模式/编排、生成式 UI、飞书 IM、云端同步)的完整技术评估。
> **优先级暂不排**,先沉淀方案,后续再决定取舍。所有交叉引用统一指向本文件。
>
> 需求见 [DESIGN.md](./DESIGN.md),架构见 [ARCHITECTURE.md](./ARCHITECTURE.md),技术选型见 [TECH.md](./TECH.md),实现路径见 [IMPLEMENTATION.md](./IMPLEMENTATION.md)。

---

## 0. 全局视角:这 7 个功能落在 5 个不同的层

> 💡 **关于版本号**:本文出现的 Phase 1 / Phase 2 指各**功能自身**的阶段(例:UI primitives Phase 1 必做 4 种、角色 Phase 1 不做编排)。整体产品版本号定义在 [DESIGN.md §3](./DESIGN.md#3-scope明确什么做什么不做)(MVP / v1 / v2 / v3+)。两套不重叠,按上下文区分。

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

## 1. 输入层扩展:图片 / @文件 / /command

**目标**:丰富用户向 agent 表达意图的方式,不止打字。

**共享 UI 模式**:三者都用**输入触发器**模式
- 监听输入框的关键字符(`@`、`/`)
- 弹出 autocomplete 面板
- 用户选择 → 插入或触发
- 复用同一个 `<TriggerMenu>` 组件

### 1.1 图片支持

**入口**:
- 粘贴板(`paste` 事件 + `clipboardData.files`)
- 拖拽到输入框(`dragover` + `drop`)
- 文件选择按钮(`<input type="file" accept="image/*">`)

**处理流程**:
```
上传
  ↓
客户端 resize(长边 ≤ 2048px,JPEG 质量 85)
  ↓
hash(SHA-256)→ 去重
  ↓
存到 ~/.local/share/everlasting/images/<hash>.jpg
  ↓
DB 存引用(message_id, path, hash, mime, size)
  ↓
构造 LLM content block(Anthropic multimodal)
```

**库选型**:
- 客户端 resize:`image` crate(纯 Rust,跨平台)
- HEIC 支持:`libheif-rs`(苹果 HEIC/HEIF 格式)
- 哈希:`blake3` 或 `sha2`

**发送到 LLM**(Anthropic 格式):
```json
{ "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": "..." } }
```

**风险**:
- 大图:客户端**必须** resize,1 张 4K 照片 = 10MB+ base64 直接炸 context
- HEIC:苹果设备照片默认格式
- 恶意图:LLM 之外的图片**不渲染**(防 prompt injection 到图像隐藏文本)

### 1.2 @文件支持

**入口**:输入 `@` 触发

**后端流程**:
```
用户输入 @
  ↓
触发 autocomplete
  ↓
后端:扫描项目目录(忽略 .git/、node_modules/、target/、>1MB 二进制)
  ↓
前端:列表展示,键盘上下选
  ↓
用户选中(回车)
  ↓
两种模式:
  ├─ 简单:把文件内容拼到 user message 前面
  └─ 高级:作为 tool_result 预填(模拟已读)
```

**库选型**:
- gitignore 解析:`ignore` crate
- 模糊匹配:`nucleo`(VSCode 同款,fzf 算法的 Rust 端口)

**索引策略**:
- 不预建索引(项目可能很大)
- 每次 @ 触发时扫一层,缓存 30 秒
- 超过 10000 文件时,提示用户用关键词

**风险**:
- 路径遍历:`@../../etc/passwd` → 必须验证在工作目录内
- 敏感文件:不默认屏蔽 `.env`、`.ssh/`、secrets.yaml → 提供"黑名单 + 二次确认"

### 1.3 /command 支持

**入口**:输入 `/` 触发

**命令分类**:
- **内置**:`/clear`、`/model <name>`、`/permissions <tool>`、`/mode plan|chat|review`、`/help`
- **用户定义**:`.everlasting/commands/*.md`,每个文件 = 一个命令

**用户命令文件格式**:
```yaml
---
name: commit
description: 用约定格式提交当前变更
argument-hint: [可选的 commit message 后缀]
---
看一下当前 git diff,生成一个符合 Conventional Commits 规范的 commit message,然后 commit。
如果有未暂存的改动,先 add。
```

**实现**:
- 命令注册表:启动时扫描 `.everlasting/commands/`,解析 frontmatter
- 触发:`/commit` → 展开模板内容 → 作为 user message 发送
- **与 skill 的区别**:command 是用户手动调,skill 是 LLM 可调
- **与 skill 的联系**:底层可以共用(都用 frontmatter + Markdown)

**库选型**:
- YAML:`serde_yml`(`serde_yaml` 已弃用,迁移到此分叉)
- 简单 parser 几十行,不用 `clap`

**风险**:
- 命令名冲突:内置 vs 用户 → 内置优先,用户覆盖必须用 `/custom:commit`
- 模板 token 膨胀:用户写超长命令 → 限制 4KB

### 1.4 架构影响

- **Tauri commands 新增**:
  - `upload_image(bytes) -> ImageRef`
  - `search_files_for_mention(prefix) -> Vec<FileCandidate>`
  - `list_commands() -> Vec<CommandInfo>`
  - `run_command(name, args) -> Result<()>`
- **数据模型新增**:
  - `images(id, hash, path, mime, size, created_at)`
  - `attachments(message_id, type[image|file|command], ref_id, meta_json)`
- **前端组件**:`<TriggerMenu>`、`<ImagePreview>`、`<FileChip>`
- **库选型(前端)**:reka-ui `command` 组件(无依赖 popover,几十行代码)

---

## 2. Agent Skill 系统

**目标**:把"做某件事的方法"打包成可复用单元,既能被用户显式调(`/skill`),也能被 LLM 按需调。

**Skill 的本质**:
- 一段 Markdown 指令(系统 prompt 片段)
- 可选:依赖的 tool 列表
- 可选:关联的资源文件
- 命名 + 描述(让 LLM 知道何时调)

**格式**(对齐 Anthropic skill 规范):
```
.everlasting/skills/<name>/SKILL.md
---
name: review-pr
description: |
  当用户要求 review PR / diff 时调用。会读 diff、思考、给出结构化反馈。
allowed-tools: [read_file, search_code, git_diff]
---
# Review PR 指南

(实际指令内容...)
```

**调用入口**:
- **用户**:`/review-pr` 触发 → skill 内容作为 user message 发送
- **LLM**:看到一个虚拟 tool `use_skill(skill_name)`,description 字段列出所有可用 skill
  - LLM 决策:读 description,觉得合适就调
  - 我们的 runtime:看到 use_skill 调用 → 展开 skill 内容 → 注入到 system prompt

**与 memory / role 的边界**(重要,见 §3 / §4):
- **Memory**:始终加载的指令(全局上下文)
- **Skill**:按需加载的指令 + 工具
- **Role**:一组 memory + 默认 skill 集合 + 工具白名单
- **实现可共用**:三者都是 frontmatter + Markdown,只是**加载时机**不同

**库/选型**:
- frontmatter 解析:`serde_yml`
- 加载器:几十行 Rust
- **不引入新框架**

**风险**:
- Skill 数量爆炸 → LLM 不知道用哪个
  - 缓解:description 要写好;限制单 session 可见 skill 数
- Skill 注入恶意指令
  - 缓解:文件位置隔离(`~/.everlasting/skills/` vs 项目内),用户必须显式 approve
- Skill 之间冲突
  - 缓解:优先级机制(具体 skill 覆盖通用)

**与 §1.3 /command 的关系**:
- /command 是 skill 的"用户调用入口"(强制触发)
- skill 是"LLM 调用入口"(按需触发)
- 一个 skill 可以暴露成 /command,但反之不必然

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

### 3.4 实施顺序建议(供参考,不是死规)
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

## 4. 跨设备(v2 候选)

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

**Phase 1(后期 v2 考虑)**:
- [ ] VPS daemon 部署文档(系统级 systemd,配置文件)
- [ ] 跨机器 session 列表同步(只读)
- [ ] "工作树迁移"流程(GUI 按钮)
- [ ] 多设备消息历史(只在源机器)
- [ ] 配置文件跨设备同步

**不做**:
- ❌ 多端同时编辑同一 session(冲突解决不做)
- ❌ VPS 持有 worktree 文件副本(隐私 + 存储)
- ❌ Cloudflare Tunnel / 第三方中转(国内 VPS 直连足够)
- ❌ 实时同步(只在显式触发时同步)

**风险**(提前识别):
- 数据过 VPS(虽然不持文件,元数据仍过 VPS)— 接受这个权衡
- 跨机器 worktree 路径冲突(用 session_id 隔离)
- 源机器断网时目标机器不能接续 — 设计选择,不是 bug

> 💡 详见 [IMPLEMENTATION §4 决策日志"方案 C"](./IMPLEMENTATION.md#4-决策日志)。本节是 v2 候选,前期不展开实现细节。

---

## 5. 步骤 3b-1 实施后续(implementation follow-up)

> 这一节是步骤 3b-1(项目基础结构 + 顶部 Tabs UI)落地后留的"实施层面"小尾巴,不是新功能候选。技术债性质。完整列表 + 优先级见 [docs/_archive/2026-06-3b-1/FOLLOW-UP.md](../_archive/2026-06-3b-1/FOLLOW-UP.md),本节只记每条的工作量 + 触发时机 + 实际落地状态。

### 5.1 cwd 简化为 `~/` ✅ 已落地(2026-06-06,commit `ef7cea8`)

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

## 远期（v3+，暂不评估）

> 本节集中放 v3+ 远期项(已评估但不计划近期实施),原 §3-§7 已迁移至此。每节内容保留原样,只调整标题层级 + 加 [v3+] 标识。

> [v3+ 远期 — 暂不评估]

### 3. 多层 Memory 与约束

**目标**:不同范围的指令/记忆,让 agent 行为可控且一致。

**层级**(从外到内,优先级递增):
1. **User-level**:跨项目、跨 session 的全局设置
   - 例:"总是用中文回答"、"代码风格:函数式优先"
   - 存储:`~/.config/everlasting/CLAUDE.md`
2. **Project-level**:本项目规则
   - 例:"用 pnpm"、"提交前必须跑测试"
   - 存储`<project>/.everlasting/CLAUDE.md` 或 `AGENTS.md`(对齐 Claude Code 生态)
3. **Session-level**:本次对话特有
   - 例:"接下来专注重构 user 模块"
   - 存储 SQLite `session_instructions` 表
4. **Runtime memory**:agent 跨 session 长期记忆(可被 LLM 主动写)
   - 例:"用户在 Rust 中偏好使用 anyhow 而不是 thiserror"
   - 存储 SQLite `memories` 表 + FTS5 检索

**加载与覆盖规则**:
- 加载优先级:User → Project → Session
- 同级覆盖:文件最新修改覆盖 DB 历史
- 超 token 限制:按优先级裁剪,先砍低级
- UI 实时反映:用户编辑 user memory 文件,下一个 user message 立即生效

**库/选型**:
- 文件监听:`notify`(跨平台 fsnotify 绑定)
- FTS5:SQLite 内置
- 格式:Markdown + YAML frontmatter
- **对齐行业标准**:用 `AGENTS.md` 或 `CLAUDE.md` 文件名(Anthropic 官方推荐)

**架构影响**:
- **Tauri commands 新增**:
  - `read_user_memory()`、`write_user_memory(content)`
  - `read_project_memory(project_id)`、`write_project_memory(...)`
  - `search_memories(query) -> Vec<Memory>`(FTS5)
- **Context 构造阶段**(ARCHITECTURE.md §2.2 第 ⑤ 关)扩展:
  - 加载 4 层 memory
  - 按 token 预算裁剪
  - 拼到 system prompt 头部
- **UI**:
  - Settings 页:编辑 user memory
  - Project 页:编辑 project memory
  - Session 页:编辑 session-level instructions

**与 skill / role 的协同**:
- Skill + Memory + Role **都走同一个 loader**,只是触发时机不同

| 类型       | 加载时机           | 触发方式              |
|------------|--------------------|-----------------------|
| Memory     | 每次 LLM 调用前    | 自动                  |
| Skill      | LLM 显式调         | `use_skill` tool      |
| /command   | 用户显式调         | 键盘 `/`              |
| Role       | session 启动时     | UI 选                 |

**风险**:
- Memory 越长,token 越贵 → 强约束(总 memory ≤ 2K tokens)
- 跨项目 memory 泄漏 → 严格 user/project 边界
- 用户改了 memory 不知道 → 启动 banner 提示"加载了 N 条 memory"

> [v3+ 远期 — 暂不评估]

### 4. 多角色 · 多模式 · 可编排

**目标**:让 agent 不止"一个 agent",而是一个可定制的协作系统。

#### 4.1 多角色(Role)

**预定义**(起步):
- **架构师** — 重设计、重权衡
- **开发者** — 重实现、重测试
- **Reviewer** — 重代码质量、重边界
- **Tester** — 重覆盖率、重边界 case
- **文档作者** — 重清晰、重示例

**每个 role 定义**:
```toml
[role]
name = "developer"
description = "负责写实现,偏好函数式,先写测试"

[role.system_prompt]
base = "你是一个有 10 年经验的 Rust 开发者..."
suffix = "每次写完代码,自动跑 cargo test"

[role.tools]
whitelist = ["read_file", "write_file", "shell", "edit_file"]
blacklist = ["git_push"]  # 强制不允许直接 push

[role.model]
preferred = "claude-sonnet-4"
fallback = "claude-haiku-3.5"
```

> 💡 **`role.model.preferred` / `fallback` 实施时引用 `.trellis/tasks/archive/2026-06/06-08-multi-model-llm-provider-planning/` 落地的 `providers` / `models` / `app_config.default_model_id` catalog**(PR1 `f9c5648` + PR2 `0a787ef` + PR3 即将 commit)。`role.model.preferred` 解析为 `ModelRow.model_name` 字符串,`fallback` 同理;若 model 行被删,fallback 走 `app_config.default_model_id` 兜底(catalog-first 跟 PR2 决议一致)。**本节不重复定义 catalog schema,详细 wire shape 见 `.trellis/spec/backend/llm-contract.md` "Scenario: Multi-Provider Abstraction (PR1)" section**。

**存储**:
- 预定义:`.everlasting/roles/*.toml`(随 app 装)
- 用户自定义:`~/.config/everlasting/roles/*.toml`

**切换方式**:
- session 启动时选
- session 中途可切换(切换会带新的 system prompt,但历史消息保留)

**库选型**:
- 解析:`toml` crate(标准)
- 几十行 Rust

#### 4.2 多模式(Mode)

| 模式          | 描述                       | Tool 调用?     | 用户确认?  |
|---------------|----------------------------|----------------|------------|
| **Chat**      | 正常对话,实时流式          | 是             | 危险动作   |
| **Plan**      | 思考但**不执行**           | 否(只看)      | 计划确认   |
| **Review**    | 只读不写                   | 否(只读 tool) | —          |
| **Background** | 后台跑,完成时通知         | 是             | 危险动作   |
| **Yolo**      | 无任何确认(危险,默认关)   | 是             | 无         |

**实现**:
- `enum Mode { Chat, Plan, Review, Background, Yolo }`
- 在 ARCHITECTURE §2.2 第 ⑨ 关(权限检查)统一处理
- 状态机:Mode 切换写审计日志

#### 4.3 可编排(Orchestration)

**节点定义**:
```rust
struct WorkflowNode {
    id: NodeId,
    role: RoleRef,             // 用哪个 role
    mode: Mode,                // 在哪个 mode 下跑
    prompt_template: String,   // 输入 prompt(支持 {{prev.output}} 插值)
    depends_on: Vec<NodeId>,   // 依赖哪些节点
}
```

**执行模型**:
- tokio tasks + `tokio::sync::mpsc` channels
- DAG 拓扑排序
- 节点并行(无依赖关系)
- 失败策略:全部停止 / 继续 / 重试 N 次

**持久化**:
- workflow 定义:`.everlasting/workflows/<name>.json`
- workflow 状态:SQLite,崩溃可恢复

**可视化**:
- `@vue-flow/core`(原 React Flow,Vue 版同名)
- 节点拖拽、连线、配置
- **不做到 Phase 1**,Phase 1 只做单 agent + role/mode 切换

**库/选型**:
- 编排引擎:**自研**,DAG 调度 200-500 行 Rust 够用
- 可视化:`@vue-flow/core`(Phase 2 再加)
- 备选:`dagrs` 存在但不够主流

**风险**:
- 复杂度爆炸 → 提供"模拟运行"(dry-run)模式
- 跨 session 状态:崩溃恢复要细做
- token 成本:多 agent 串行 = 多倍成本 → 预算上限硬卡

**Phase 1 / Phase 2 范围划分**:
- Phase 1:role + mode 切换,**无编排**
- Phase 2:可视化 DAG 编辑器 + workflow 执行

> [v3+ 远期 — 暂不评估]

### 5. 生成式 UI 开关

**目标**:让 agent 的回复不只文本,可以是可交互的 UI。

**两种范式**:
- **约束式**(推荐,Phase 1):LLM 通过 tool use 输出结构化 JSON,前端按 type 渲染
- **自由式**(v3+ 考虑):LLM 生成 HTML,前端沙箱渲染

**约束式 UI primitives**(总览,Phase 1 只做前 4 种):

| Type           | 渲染                  | Action 机制             | 范围       |
|----------------|----------------------|-------------------------|------------|
| `button`       | 按钮                 | 触发 Tauri command      | **Phase 1** |
| `form`         | 表单                 | 提交收集输入            | Phase 2+    |
| `selector`     | 单/多选              | 选完返回                | **Phase 1** |
| `chart`        | 图表(折/柱/饼)      | 只读                    | Phase 2+    |
| `table`        | 表格                 | 可排序                  | Phase 2+    |
| `diff`         | 代码 diff            | 可应用/拒绝             | **Phase 1** |
| `code_block`   | 语法高亮             | 可复制                  | **Phase 1** |
| `markdown`     | 富文本               | —                       | Phase 1(基础,默认开) |

**Phase 1 范围**:
- 必做:`button` / `selector` / `diff` / `code_block` 4 种
- 够覆盖 80% 用例(agent 询问 / 申请确认 / 展示结果)
- 4 种之外的需求降级为 text 描述

**实现路径**:
```
LLM 调 use_ui(primitives: [...])
  ↓
harness 收到
  ↓
emit("ui:render", { primitives }) → 前端
  ↓
前端 component registry: type → Vue 组件
  ↓
渲染
```

**开关**(防止滥用):
- session-level:`allow_generative_ui: bool`(默认 false)
- tool 白名单:`use_ui` 必须在 enabled tools 中

**库选型**:
- 图表:`ECharts` + `vue-echarts`(跨框架、中文文档全,替代 recharts)
- 表格:`@tanstack/vue-table`
- diff:框架无关的 `diff` (jsdiff) + 自渲染 Vue 组件
- 表单:`vee-validate`
- **不引入 UI 框架**(MUI / Ant Design 太重,自己攒)

**风险**:
- 按钮回调的 action:必须白名单,前端能调的 Tauri command 是受控的
- 跨 session 持久化:UI 事件不存 DB(刷新即丢),除非显式标记
- LLM 幻觉:输出的 JSON 不合法 → 兜底渲染为错误提示,不崩 UI

> [v3+ 远期 — 暂不评估]

### 6. IM 通道(飞书)

**目标**:在飞书里直接跟 agent 对话,等于"在 IM 里跑 everlasting"。

**架构:Channel Adapter 模式**(核心抽象)

Channel trait 的定义与设计动机见 [ARCHITECTURE.md §5 决策:Channel Adapter 抽象](./ARCHITECTURE.md#5-决策channel-adapter-抽象为多入口铺路)。本节只讲飞书场景的实施。

**实现**:
- `TauriGuiChannel` — 走 Tauri event
- `FeishuChannel` — 走飞书 WebSocket
- `CliChannel` — 走 stdin/stdout
- **共享同一个 agent core**,只是输入输出接到不同 channel

**核心架构变更**:**Agent Daemon 化**

详细动机与协议选型见 [ARCHITECTURE.md §4 决策:Agent Daemon 化](./ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路)。本节展示 daemon 化后的拓扑:

```
[之前]
Tauri 进程 = GUI + Agent + Tools(全在一起)

[之后]
┌──────────────────┐
│ Tauri GUI 进程   │ ← 只是个 client
└──────────────────┘
        ↕ IPC / HTTP
┌──────────────────┐
│ Agent Daemon     │ ← agent core 跑在这
│  - Session 管理  │
│  - Channel 路由  │
│  - LLM/Tool 执行 │
└──────────────────┘
        ↑         ↑
        │         │
    Feishu      CLI / 别的 client
```

**飞书侧实现**:
- 用现有 `feishu-integration` skill 的能力
- WebSocket 长连接(飞书 SDK v2)
- 收消息 → 转 `IncomingMessage` → 喂给 agent
- 发消息:文本 + interactive card

**流式响应**:
- 飞书消息可以 patch:发一条占位 "..." 消息,然后 PATCH 内容
- 或者每 N 个 token 整条更新
- 卡片 markdown 字段可更新(用 message_id)

**身份映射**:
- 飞书 `user_open_id` ↔ 本地 user_id
- 简单方案:1 个飞书 bot = 1 个本地用户(个人用够)
- 复杂方案:多账号(不做)

**架构影响**:
- 新模块:`src-tauri/src/channels/{feishu,cli,gui}.rs`
- Tauri 进程从"主进程"降级为"GUI client"
- 新增:`src-tauri/src/daemon.rs` 跑 agent core
- 通信:本地用 Unix socket / Named pipe,远程用 WebSocket(为 §7 留接口)

**风险**:
- daemon 进程管理:写个简单 supervisor 或用 systemd
- 消息顺序:飞书消息无序到达 → 用 client_msg_id 去重
- 速率限制:飞书有 QPS 限制 → 批处理
- 卡片长度:markdown 字段有限制 → 超长分页

> [v3+ 远期 — 暂不评估]

### 7. 云端状态同步

**目标**:在外网环境下,能用手机 / IM 简单操作(看 session、发简单指令)。

**定位**(重要):
- **不是**完整的多端协作(说过不做团队协作)
- **是**个人远程遥控
- 跟 §6 飞书的关系:飞书 = 消息通道,云端 = 状态层

**最小方案**:
- **Cloudflare Workers + D1**(SQLite)
- 暴露 REST API:
  - `GET /sessions` — 列出 session
  - `GET /sessions/:id/messages?limit=20` — 最近消息
  - `POST /sessions/:id/messages` — 发文本(限长 + 限频)
- 鉴权:bearer token

**数据流**:
```
[Local Daemon]
  ├─ 状态变更 → push 到 Worker (HTTPS POST,只 push 摘要)
  └─ 定时增量同步

[Cloudflare Worker]
  ├─ 存 D1
  └─ 暴露 REST

[IM 端(飞书)或 Web 端]
  └─ 通过 Worker 读 / 写
```

**为什么选 Cloudflare Workers**:
- 免费额度够个人用(10 万请求/天)
- D1 是 SQLite,跟本地数据模型**完全一致**
- 部署简单(`wrangler deploy`)
- 不用维护服务器
- 全球边缘

**隐私设计**:
- **只 push 摘要,不 push 完整消息**
  - push:session id、标题、状态、最后 1 条消息预览
  - 不 push:完整消息历史、tool 调用、文件内容
- 用户主动"展开历史"时才拉详情
- **任何写操作(发消息)在本地确认弹窗**(可选关)
- token 存 OS keychain,不进 DB

**Phase 1 范围**(克制):
- 只读:session 列表 + 最新 1 条消息
- 简单写:发一条文本(限 500 字符)
- **不做**:文件 diff 推送、tool 调用跟踪(数据量太大)

**风险**:
- 数据过第三方(Cloudflare):可自托管(更麻烦)
- 离线一致性:本地网络挂了,消息会丢(下次同步重试)
- Worker 冷启动:首次访问慢(50-200ms)

**v3+ 候选**:完全自托管(Go / Rust 写个小 server,跑在自己 VPS)
