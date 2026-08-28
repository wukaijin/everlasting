# BACKLOG 附录 A — 远期候选(已归档 2026-06-25)

> **本文件已从 `docs/BACKLOG.md` 附录 A 归档到 `docs/_archive/`**。
>
> **归档原因**:这些是远期功能(7 项远期候选,共 357 行)的技术评估,排期归 `docs/ROADMAP.md §2 V2 路线图 4 档分类`。BACKLOG.md 当前版本只保留已落地功能的引用 + 跨功能共同关注点 + 跨设备 / implementation follow-up。
>
> **维护承诺**:本文件**只读不改**。远期候选如有进展,新评估应直接在 ROADMAP.md §2 中更新,不再回写此处。

---

## 附录 A: 远期候选

> 本节集中放远期项(已评估但不计划近期实施),原 BACKLOG §3-§7 候选功能按"已落地 / 远期"重新分组。每节内容保留原样,只调整标题层级。**整体排期 / 优先级归 [ROADMAP.md §2 V2 路线图分类](../ROADMAP.md#2-v2-路线图分类2026-06-10-重排)**,本附录不再标注时间。
>
> **重要**:本节编号"3-7"是历史原编号,内容跟 BACKLOG §3"跨 7 个功能的共同关注点"**完全无关**——后者是横向关注点,本附录是按功能归档的远期候选。

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
| **Edit**      | 正常对话 + 实时流式 + 全 tool list | 是             | 危险动作(⑨ 5-tier) |
| **Plan**      | 思考但**不执行**           | 否(只读 tool)  | 计划确认   |
| **Yolo**      | 无任何确认(危险,默认关)   | 是             | 无(整段 bypass Tier 4 弹窗,Tier 2 硬墙仍生效) |

> **历史演进**(详见 [IMPLEMENTATION §4 决策日志 2026-06-13 "Mode 3 档化"](../IMPLEMENTATION.md)):
> - 2026-06-12 落地 4 档:`Chat` / `Plan` / `Review` / `Yolo`
> - 2026-06-13 grill-with-docs session 重新审视语义,3 档化:`Chat → Edit` 改名 + `Review` 移除(行为跟 `Plan` 重复)
> - `Background` enum 留位置,UI 不暴露(留作远期接 channel router 的扩展位)
> - **Breaking wire rename**:不保留 `'chat'` / `'review'` 字符串 alias;v6 migration 启动时跑两次幂等 UPDATE 落库

**实现**:
- `enum Mode { Edit, Plan, Yolo, Background }`(Background 留位不暴露 UI)
- 在 ARCHITECTURE §2.2 第 ⑨ 关(权限检查)统一处理 + ⑧a Mode 检查(Tier 3 提前拦截 Plan 的 tool_use)
- 状态机:Mode 切换写审计日志(`AuditKind::mode_changed` / `yolo_entered` / `yolo_exited`,详见 [ARCHITECTURE §2.5.8](../ARCHITECTURE.md#258-⑯-审计日志a2--b7-pr1-落地2026-06-13已实施))
- 前端:`<ModeSelect>` 放 input row 左侧,3 档颜色区分(Edit 默认 / Plan 琥珀 / Yolo 红)
- 完整 PRD 走 `.trellis/tasks/archive/2026-06/06-12-a2-b7-permission-and-mode/`(已被 `06-13-a2-b7-regrill-path-based` re-grill 取代,顶部加 Superseded 标记)

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
- **首批不做**,首批只做单 agent + role/mode 切换

**库/选型**:
- 编排引擎:**自研**,DAG 调度 200-500 行 Rust 够用
- 可视化:`@vue-flow/core`(后期再加)
- 备选:`dagrs` 存在但不够主流

**风险**:
- 复杂度爆炸 → 提供"模拟运行"(dry-run)模式
- 跨 session 状态:崩溃恢复要细做
- token 成本:多 agent 串行 = 多倍成本 → 预算上限硬卡

**范围划分**:
- mode 切换(无 role 编排)— ✅ A2+B7 已落地(2026-06-12/13),见 [ROADMAP §1.2](../ROADMAP.md#12-路线图外完成) + [IMPLEMENTATION §4 决策日志 2026-06-13](../IMPLEMENTATION.md#4-决策日志)
- role 切换 — 远期(未排期,排期归 [ROADMAP §2 V2 路线图分类](../ROADMAP.md#2-v2-路线图分类2026-06-10-重排))
- 可视化 DAG 编辑器 + workflow 执行 — 排期归 [ROADMAP §2 第四档 B8](../ROADMAP.md#2-v2-路线图分类2026-06-10-重排)


### 5. 生成式 UI 开关

**目标**:让 agent 的回复不只文本,可以是可交互的 UI。

**两种范式**:
- **约束式**(推荐):LLM 通过 tool use 输出结构化 JSON,前端按 type 渲染
- **自由式**(远期考虑):LLM 生成 HTML,前端沙箱渲染

**约束式 UI primitives**(总览,首批做前 4 种):

| Type           | 渲染                  | Action 机制             | 范围       |
|----------------|----------------------|-------------------------|------------|
| `button`       | 按钮                 | 触发 Tauri command      | 首批(必做) |
| `form`         | 表单                 | 提交收集输入            | 后期       |
| `selector`     | 单/多选              | 选完返回                | 首批(必做) |
| `chart`        | 图表(折/柱/饼)      | 只读                    | 后期       |
| `table`        | 表格                 | 可排序                  | 后期       |
| `diff`         | 代码 diff            | 可应用/拒绝             | 首批(必做) |
| `code_block`   | 语法高亮             | 可复制                  | 首批(必做) |
| `markdown`     | 富文本               | —                       | 首批(基础,默认开) |

**首批范围**:
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


### 6. IM 通道(飞书)

**目标**:在飞书里直接跟 agent 对话,等于"在 IM 里跑 everlasting"。

**架构:Channel Adapter 模式**(核心抽象)

Channel trait 的定义与设计动机见 [ARCHITECTURE.md §5 决策:Channel Adapter 抽象](../ARCHITECTURE.md#5-决策channel-adapter-抽象为多入口铺路)。本节只讲飞书场景的实施。

**实现**:
- `TauriGuiChannel` — 走 Tauri event
- `FeishuChannel` — 走飞书 WebSocket
- `CliChannel` — 走 stdin/stdout
- **共享同一个 agent core**,只是输入输出接到不同 channel

**核心架构变更**:**Agent Daemon 化**

详细动机与协议选型见 [ARCHITECTURE.md §4 决策:Agent Daemon 化](../ARCHITECTURE.md#4-决策agent-daemon-化为多-channel-接入铺路)。本节展示 daemon 化后的拓扑:

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

**实施范围**(克制;排期归 [ROADMAP §2 第四档 B11](../ROADMAP.md#2-v2-路线图分类2026-06-10-重排)):
- 只读:session 列表 + 最新 1 条消息
- 简单写:发一条文本(限 500 字符)
- **不做**:文件 diff 推送、tool 调用跟踪(数据量太大)

**风险**:
- 数据过第三方(Cloudflare):可自托管(更麻烦)
- 离线一致性:本地网络挂了,消息会丢(下次同步重试)
- Worker 冷启动:首次访问慢(50-200ms)

**远期候选**:完全自托管(Go / Rust 写个小 server,跑在自己 VPS)