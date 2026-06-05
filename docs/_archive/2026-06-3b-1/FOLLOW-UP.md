# FOLLOW-UP — 已知 issues / TBDs / 候选修复

> **范围**:本项目实施过程中发现的"暂不修" / "下次做" / "踩坑后想沉淀"的事项集中记录。新发现的 follow-up 往里加;跟现有 `[IMPLEMENTATION §3](./IMPLEMENTATION.md#3-待办与下一步)` 不重复(那是路线图),跟 `[BACKLOG §1-§9](./BACKLOG.md)` 不重复(那是新功能)。
> **建立**:2026-06-05(步骤 3b-1 收尾时建)。

---

## 实施 follow-up(代码层面,影响 PR1/PR2)

### FU-1 · cwd 简化为 `~/`

- **现状**:chat header 显示 cwd 用完整绝对路径(`/home/carlos/code/foo/backend`)。PROPOSAL §5.4 / Q5 决议是简化为 `~/foo/backend`,但 PR1 backend 没暴露 `home_dir` 给前端,前端拿不到。
- **修法**:PR1 backend 加 `pub fn get_home_dir() -> String` Tauri command(读 `dirs::home_dir()`),PR2 frontend 调 `invoke('get_home_dir')` 缓存,在 `toPayloadContent` / chat header 显式替换前缀。
- **关联**:PR2 commit `93a0753` 注释 "简化为 `~/` 留给 follow-up" + post-fixes commit `18354a0` 末尾 follow-up 列表。
- **优先级**:低(可读性增强,不是 bug)
- **工作量**:~30 行(backend 10 + frontend 20)

### FU-2 · TS interface 字段 snake_case → camelCase

- **现状**:`SessionSummary.project_id` / `current_cwd` / `created_at` 等字段是 snake_case,跟 Rust struct 序列化一致。TS 端 `interface` 也是 snake_case,**非常规**。
- **修法**:PR1 backend 在所有 IPC 序列化 struct 上加 `#[serde(rename_all = "camelCase")]`,PR2 frontend interface 字段全改 camelCase。
- **决策点**:保留 snake_case 也行(Rust 风格 + 跟 backend 一致,少一层 rename)。**没定论**,follow-up 留着。
- **关联**:post-fixes commit `18354a0` 末尾 follow-up 列表。
- **优先级**:低(纯风格)
- **工作量**:~50 行(backend 改 8 个 struct 注解 + frontend 改所有 field 名)

### FU-3 · `pick_project_dir` 改成前端 reka-ui 渲染 dialog

- **现状**:用 Tauri native `pick_folder` dialog,WSLg 下走 GTK / xdg-desktop-portal,渲染是 linux GTK 风格(Q8v2)。
- **用户反馈**:"本来期望 dialog 是由前端渲染的"(2026-06-05 session)。希望自渲染:HTML 树形目录 + 搜索框 + 文件图标,更可控,更跨平台一致。
- **修法**:PR2 frontend 写一个 `<ProjectDirPicker>` 组件,从 Tauri 加一个 `list_dir(path: String) -> Vec<DirEntry>` Tauri command 读子目录,前端自渲染树形 + 搜索/键盘导航。`pick_project_dir` Tauri command 废弃。
- **关联**:PROPOSAL §5.4 (Q8v2 修正) + 用户偏好记录。
- **优先级**:中(UX 改善,不阻塞功能)
- **工作量**:~150 行(frontend 组件 ~120 + backend `list_dir` command ~30)

---

## 经验沉淀(已发生 bug,文档化避免重复)

### FU-4 · Tauri 2 IPC arg 默认 `rename_all = "camelCase"`

- **现象**:Rust 函数 `async fn create_session(state: ..., project_id: String, ...)` 在 Tauri 2 IPC 边界默认 `rename_all = "camelCase"`,JS 端 `invoke("create_session", { project_id })` 报 `invalid args 'projectId' for command 'create_session'`: 缺失。
- **已发生**:3b-1 PR2 第一次发消息时阻塞。
- **修复**:JS 端用 camelCase 调:`invoke("create_session", { projectId, initialCwd })`。
- **影响命令**:`list_sessions` / `create_session` / `update_project_name` / `update_project_path` 等所有 multi-word 参数。
- **特例**:单字参数(`path` / `id` / `fallback`)不受 camelCase 转换影响,两种写法都接受。
- **沉淀位置**:`docs/HACKING-wsl.md` 坑 11
- **关联**:post-fixes commit `18354a0` 修法 #1

### FU-5 · `Option<T>` Tauri 2 IPC null 行为

- **现象**:Rust `model: Option<String>`,JS 端 `invoke("create_session", { ..., model: null })` 报 `command create_session missing required key ` (key 名字段打印为空)。Tauri 2 IPC 把 JS `null` 当 missing required 处理,且错误打印的 key 名字段为空字符串。
- **已发生**:3b-1 PR2 第一次发消息时阻塞(接 hotfix 1 之后立刻撞到 hotfix 2)。
- **修复**:JS 端省略 `model` 字段不传(`{ projectId, initialCwd }`),Rust 端 `Option::None` 走 `unwrap_or_else(|| state.config.model.clone())` 兜底。
- **影响命令**:所有 `Option<T>` 参数 + 用户在 JS 端尝试 `null` 显式置空。
- **沉淀位置**:`docs/HACKING-llm.md` 客户端陷阱
- **关联**:post-fixes commit `18354a0` 修法 #2

### FU-6 · Anthropic tool_result 块只能出现在 user role

- **现象**:Anthropic Messages API 严格规定 `tool_result` 块只能出现在 user role message 里,assistant role 含 `tool_result` 块 → 2013 (`tool result's tool id ... not found`)。
- **已发生**:3b-1 PR2 第二次发消息时崩溃(第一次 OK,第二次 fetch 历史 messages 重新构造时撞)。
- **根因链**:`rehydrateMessages` 把 user message 2(tool_result-only "ghost")的 `toolResults` push 到上一个 assistant message 1 做 UI grouping("done / running" 状态查询),但**没清空** user message 2 自己的 `toolResults`。`toPayloadContent` 之前对 assistant / user 走同一条代码路径,把 assistant message 1 上的 `toolResults` 也喂给 LLM,违反协议。
- **修复**:`toPayloadContent` 按 role 分发,assistant role 跳过 `m.toolResults`(UI grouping 用,不上 wire),user role 才生成 `tool_result` 块。
- **影响**:所有 rehydrate tool_result 跨 role 边界的 UI 框架
- **沉淀位置**:`docs/HACKING-llm.md` 客户端陷阱
- **关联**:post-fixes commit `18354a0` 修法 #3

---

## 流程 follow-up(本项目工作方式)

### FU-7 · 外部 LLM 评审问题重写

- **现状**:PROPOSAL §9 给外部 LLM(GLM / DeepSeek)的 12 个提问有些偏"内部设计争议"(如 "UUID+可变 path 是否 over-engineer"),需要评审者读懂 IMPLEMENTATION 路线图 + DESIGN MVP 约束才能答。
- **修法**:发外部评审前,把所有问题重写成"只需要读 PROPOSAL 就能答"的形式。例如 "在当前单机场景下,UUID+可变 path 相比 path-as-PK 各有什么利弊?你能想到什么场景下必须用 UUID?"
- **关联**:深 seek 评审 §5 提及。
- **优先级**:中(下次发外部评审前做)

### FU-8 · Verifier 报告里"snake_case / camelCase"约定写进 PR 模板

- **现状**:PR2 verifier 报告里 3 个"PR1 / PR2 wire format 偏差"被列为 quality issues,但没变成 PR 检查项。
- **修法**:trellis 任务的 `check.jsonl` 增加必查项"Tauri command arg 是否 camelCase" + "TS interface 字段是否 camelCase",作为 PR 验收硬约束。
- **关联**:PR2 verifier 报告 quality #1 / #2。
- **优先级**:中

---

## 索引

| FU | 优先级 | 关联 commit / 文档 | 工作量 |
|---|---|---|---|
| FU-1 cwd `~/` | 低 | `18354a0` | ~30 |
| FU-2 TS snake_case | 低 | `18354a0` | ~50 |
| FU-3 前端 dialog | 中 | Q8v2 修正 / 用户偏好 | ~150 |
| FU-4 Tauri camelCase | 经验 | `18354a0` 修法 #1 | — |
| FU-5 Option<T> null | 经验 | `18354a0` 修法 #2 | — |
| FU-6 tool_result role | 经验 | `18354a0` 修法 #3 | — |
| FU-7 外部评审问题 | 中 | 深 seek 评审 §5 | ~30 (一次性) |
| FU-8 check.jsonl 模板 | 中 | PR2 verifier 报告 | ~10 |

**总计**(估):~270 行新代码 + 4 个文档章节增量。**不紧急**,按需实施。
