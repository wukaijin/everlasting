# Review — PROPOSAL-project-binding-and-top-tabs.md（步骤 3b-1）

> **评审者**: glm-5.1
> **评审日期**: 2026-06-05
> **评审对象**: `docs/PROPOSAL-project-binding-and-top-tabs.md` + `.trellis/tasks/06-05-tabs-ui-3b-1/prd.md`
> **评审范围**: 设计完备性、与现有代码衔接、可行性、风险

---

## 0. 总体评价

PROPOSAL 质量很高。Q1-Q9 决策表 + 明确的 out-of-scope 边界 + §9 给评审者的问题，体现了"自我说服后主动找漏洞"的好习惯。设计拆分 3b-1/3b-2 的决策合理——先解锁步骤 4 的前置数据结构，三栏 UI 和 rig 迁移后做。

PRD 过于单薄，基本只是指向 PROPOSAL 的指针，缺少可执行的验收标准和测试场景。

---

## 1. 与现有代码的衔接 gap（必须修复）

### 1.1 未列出受影响的现有 Tauri commands

PROPOSAL §4.2 列了新增的 7 个 commands，§4.3 展示了 `chat` 改造，但遗漏了以下现有 commands 的改造：

| 现有 command | 需要的变更 |
|---|---|
| `create_session` | 需加 `project_id` + `initial_cwd` 参数；现在签名是 `async fn create_session(model: String) -> Result<Session>` |
| `list_sessions` | 需加 `project_id: Option<String>` 过滤 |
| `delete_session` | 无变更（session 已 FK 到 project）但需确认 CASCADE 行为 |
| `load_session` | 无变更但需确认 `project_id` 返回值供前端校验 |

`create_session` 的改造尤其关键——它是前端新建对话的入口，不加 `project_id` 就建不出有 project 绑定的 session。建议 §4.2 补一个"受影响 commands 改造表"。

### 1.2 `shell` tool 当前无 cwd 概念——改造量比描述的大

现有 `shell.rs` 的 execute 签名是：

```rust
pub async fn execute(params: serde_json::Value) -> Result<ToolResult, ToolError>
```

它接收 `serde_json::Value`（从 LLM 输出解析），内部 `tokio::process::Command::new("sh")` 没有设 `.current_dir()`。PROPOSAL §4.4 说要加 `working_directory` 参数，但这涉及：

- tool input schema 变更（LLM 需要知道可以传这个参数）
- tool 执行上下文传递（怎么把 `session.current_cwd` 和 `project.path` 传进 tool？现有架构 tool 是纯函数，拿不到 AppState）
- `read_file` / `write_file` 同样需要相对路径解析逻辑

建议在 §4.4 明确 **ToolContext 传递机制**：是在 `execute_tool()` 分发时注入，还是 tools 持有一个 context 引用？对照现有代码，`tools/mod.rs` 的 `execute_tool()` 是 `pub fn execute_tool(name: &str, params: Value) -> Result<ToolResult>`，纯函数，没有 state。这里需要一个明确的接口设计决策。

### 1.3 worktree 路径术语不一致

ARCHITECTURE.md §3 用 `<project_hash>`，PROPOSAL §6 用 `<project_uuid>`。既然 Q5 选了 UUID v4，应该统一为 `project_uuid`，并回写 ARCHITECTURE.md。建议 PROPOSAL §6 加一句"注：ARCHITECTURE §3 的 `project_hash` 改为 `project_uuid`"。

---

## 2. 设计决策待确认（建议在实施前定论）

### 2.1 cwd 持久化时机（§9.5 Q5）——需要明确回答

PROPOSAL 提出了问题但没给结论。建议：**每次 shell tool 调用后立即 sync `current_cwd` 到 DB**。理由：

- Agent loop 单次 turn 内可能连续调多个 shell 命令（`cd backend && cargo test`），如果 batch 写，crash 会在中间态丢失 cwd
- 单次 DB write（sqlite WAL mode）延迟 <1ms，不值得为这点性能冒状态不一致
- 实现简单：`execute_tool("shell", ...)` 返回后，在 agent loop 里 `db::update_session_cwd(session_id, new_cwd).await`

建议在 §4.4 补一句明确的结论。

### 2.2 symlink 边界处理（§9.5 Q6）——需要明确回答

PROPOSAL 提了问题但没给结论。`canonicalize()` 解析 symlink 后做 `starts_with` 检查是正确的安全选择。但这意味着：

- project root 内的 symlink 如果指向 root 外，agent `cd` 进去会被拒绝
- 这是**有意为之**的安全行为，应在 §4.4 明确记录

如果用户确实需要访问 root 外的路径（比如 monorepo 里的 symlink），那是 v2 考虑的"workspace"概念，不在本期 scope。

### 2.3 migration 策略（Q3）——建议改为 auto-default 兜底

虽然 Q3 的决策是 DROP TABLE，但理由"spike 阶段数据保留价值低"有一个问题：**一旦对外发布（即使只是 GitHub release），用户的 sessions 就会在 minor 升级时丢失**。改成 auto-default 的额外工作量很小（migration 里插一条 `INSERT INTO projects(id, name, path, ...) VALUES('default', 'Default', $HOME, ...)`，然后 `ALTER TABLE sessions ADD COLUMN project_id TEXT NOT NULL DEFAULT 'default'`），但能避免未来骂声。

建议：如果确定本期不对外发布，保留 DROP 策略但在 PROPOSAL 加一个 `⚠️ RELEASE BLOCKER: 发布前必须改为 auto-default migration` 的标记。如果可能对外发布，现在就改。

---

## 3. 实施可行性（建议关注）

### 3.1 工作量评估偏乐观

§10 估 "backend ~600 行 + frontend ~400 行"。对照现有代码：

- `db.rs` 现有 ~350 行，要加 projects 表 CRUD + sessions 表改造 + migration，可能翻倍
- `tools/` 改造要引入 ToolContext 传递机制，影响 `mod.rs` + 三个 tool 文件
- `lib.rs` 的 `chat` command 改造涉及 project 校验 + cwd 边界 + tool context 注入
- 前端 `ChatWindow.vue` 要大改布局（加 Tab 栏 + 空状态 + sessions 按 project 过滤）

粗估 backend ~800-1000 行（含 tests）、frontend ~500-600 行。建议拆成两个 PR：
- **PR1**: 数据模型 + migration + 后端 commands + tool context 机制（无 UI 变更）
- **PR2**: 前端 Tab 栏 + 空状态 + store 改造

### 3.2 Tab 关闭后无重新打开 UI（§9.4 Q7）

空状态页面加一个"最近隐藏的项目"列表，实现成本极低（`list_projects({ hidden: true })` 渲染个 `<ul>` + 点一项调 `unhide_project`），但解决了"手贱关 Tab 就只能改 sqlite"的痛点。建议纳入本期 scope。

---

## 4. 小问题

### 4.1 Tab 的非 git 标记用 📁 不直觉

§5.4 说 📁 表示"非 git 项目"，但 📁 的通用含义是"文件夹"。用户看到 📁 可能以为"这是一个文件夹类型的项目"而不是"这个项目没有 git"。建议改为小圆点 `●`（灰色 = 非 git，绿色 = git）或直接不加标记（git 是默认能力，非 git 不需要特别标记，用户自己知道）。

### 4.2 `pick_project_dir` 的跨平台行为

Tauri dialog 在 Linux (WSLg) 下的文件选择器行为不稳定（CLAUDE.md 提到 WSL-first 设计）。建议加一个 fallback：如果 dialog 不可用，提供手动输入 path 的输入框。

---

## 5. PRD 具体问题

### 5.1 PRD 缺少验收标准

PRD 只有目标 + 指向 PROPOSAL 的链接。建议补上：

- 功能验收清单（能加项目、能切 Tab、能关 Tab、空状态正确、session 按 project 隔离、cwd 漂移 + 越界拒绝）
- 每条可测试（能手动验证或写集成测试）

### 5.2 PRD 的状态流程不完整

"后续动作"列了 4 步，但缺少：
- 从 PROPOSAL 到 implement.jsonl 的映射关系（哪几条 implement 对应 PROPOSAL §10 的哪几步）
- 外部 LLM 评审的反馈如何回写到 PROPOSAL 的流程

---

## 6. 总结：需要确认的 5 个决策点

| # | 问题 | 建议 |
|---|---|---|
| 1 | 现有 commands (`create_session` / `list_sessions`) 改造 | §4.2 补受影响 commands 表 |
| 2 | ToolContext 传递机制 | 在 §4.4 明确设计方案 |
| 3 | cwd 持久化时机 | 每次 shell 调用后立即 sync |
| 4 | migration 策略 | 如果不对外发布就保留 DROP + 加 release blocker 标记 |
| 5 | Tab 关闭后重新打开 | 空状态页加隐藏项目列表（~20 行前端代码） |

这些确认后，PROPOSAL 就可以定稿进 implement.jsonl 了。
