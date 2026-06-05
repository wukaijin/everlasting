# 步骤 3b-1 提案审查报告

> 审查日期：2026-06-05
> 审查模型：deepseek-v4-pro
> 审查范围：`docs/PROPOSAL-project-binding-and-top-tabs.md`（405 行）+ `.trellis/tasks/06-05-tabs-ui-3b-1/prd.md`（33 行）
> 审查基线：实际源码（`db.rs` 508 行、`lib.rs` 561 行、`tools/mod.rs` + `shell.rs` + `read_file.rs` + `write_file.rs`、`chat.ts` 569 行、`config.ts` 32 行）
> 审查角度：与现有代码的兼容性、数据模型正确性、§9 提问逐一回答、缺失关注点

---

## 一、总体评价

这是一份**工程上可直接执行**的提案。Q1-Q9 的决策树在 grill 阶段收敛得干净，范围克制（明确列出 8 项不做），ASCII layout 让 UI 预期可测试。PROPOSAL 的 §2 决策摘要表 + §10 实施清单让 reviewer 能快速理解全貌，PRD 清晰地指向 PROPOSAL 作为权威来源。

**核心优势**：边界感强（项目 ≠ git 强制）、向前兼容（为步骤 4 worktree 解锁 `<project_uuid>`）、CWD 漂移设计参考了工业界同类工具的成熟模式。

**核心不足**：与现有代码有 3 处摩擦需要设计层面解决（见第二章），cwd boundary 校验的 edge case 未充分展开，空状态交互有空白。

---

## 二、与现有代码的摩擦（最关键的 3 个阻塞项）

### 2.1 `tools::execute_tool` 签名不兼容

**现状**（`tools/mod.rs:23`）：

```rust
pub async fn execute_tool(name: &str, input: &serde_json::Value) -> (String, bool)
```

没有任何 project/cwd 上下文。提案 §4.4 要求 shell tool 接收 `working_directory`、read_file/write_file 按 `session.current_cwd` 解析相对路径。

**需要的签名**：

```rust
pub struct ToolContext {
    pub project_root: PathBuf,
    pub cwd: PathBuf,
}

pub async fn execute_tool(
    name: &str,
    input: &serde_json::Value,
    ctx: &ToolContext,
) -> (String, bool)
```

影响面：`tools/mod.rs`、`shell.rs`、`read_file.rs`、`write_file.rs` 四个文件全部要改。`lib.rs:465` 的调用点 `tools::execute_tool(name, input)` 需要传入 ToolContext。

**建议**：在 PROPOSAL §4.4 或 §10 实施清单中明确写出"execute_tool 签名变更 + ToolContext 结构体定义"。

### 2.2 Migration 策略与现有架构不兼容

**现状**：`run_migrations`（`db.rs:90`）是纯 `CREATE TABLE IF NOT EXISTS`，没有版本号追踪，没有 DROP 逻辑，没有破坏性迁移能力。

PROPOSAL §7 说"DROP TABLE sessions; DROP TABLE messages"然后重建——这在现有代码里无处落脚。你需要：

1. 新增 `schema_version` 表（`CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)`）
2. 改写 `run_migrations` 为版本号驱动：`if current_version < 2 { DROP + recreate + INSERT version=2 }`
3. 如果采纳备选方案 Auto-default 兜底（见 §3.2），则为版本 1→2 的 migration 而非破坏性 DROP

**建议**：在 PROPOSAL §7 中补充 `schema_version` 表设计，在 §10 实施清单第 2 项明确写"新增 schema_version 表 + 版本号驱动的 migration 函数"。

### 2.3 `create_session` 调用链需要全线改造

**现状**：
- Rust 端：`create_session(pool, model)` — 只有 model 参数（`db.rs:151`）
- 前端：`invoke("create_session", { model: null })` — （`chat.ts:405`）
- `list_sessions` 无 `project_id` 过滤参数（`db.rs:179`）

**需要改为**：
- Rust：`create_session(pool, project_id, initial_cwd, model)`
- 前端：`invoke("create_session", { projectId, initialCwd, model })`
- `list_sessions` 加 `project_id` WHERE 条件

前端 `send()` 函数（`chat.ts:492`）需要加 project 空值守卫（见 §4.2）。

---

## 三、对 PROPOSAL §9 评审问题的逐一回答

### 3.1 模型选择

**Q1 — Container vs Grouping 模型**：当前设计（session 挂 project，cwd 在 project 内漂移）对"个人 vibe coding"是正确的默认选择。你提的"一个 session 横跨多 repo"场景——发生频率低，且开两个 session 各挂一个 project 互相切的工作量不比在一个 session 里来回 cd 大多少。如果未来这个需求真的高频出现，可以在 session 上加 `linked_sessions` 字段做"session 组"，不破坏当前模型。

**Q2 — UUID + 可变 path 是否 over-engineer**：**是，对当前阶段 over-engineer。** Path-as-PK 在个人单机使用场景下更简单——没有跨设备需求、没有多机同步，`mv` 后失联的概率对个人工具极低。建议：**这个阶段用 path 做 PK，保留 `id` 字段（UUID），先不把它当 PK 用**。等 v2 真要做跨设备时再 swap PK。成本极低（加 unique index）。

**Q3 — non-git 项目无隔离的安全性**：担忧合理但不致命。强制"只允许 1 个 active session"治标不治本——用户开两个 project 分别指向 `/etc` 和 `/` 就绕过了。真正的防线在步骤 5 的权限审计（tool 执行前的确认弹窗）。本期 `⚠️` 标记 + tooltip 提醒就够了（`📁` 图标含义模糊，见 §5.2）。

### 3.2 数据安全

**Q4 — 弃旧 sessions（DROP TABLE）**：**建议改为 Auto-default 兜底方案。** 理由不是"用户可能想保留数据"（spike 阶段确实没价值），而是：DROP TABLE 的 migration 代码一旦写了就会留在 git history 里，未来任何人在任何环境跑开发版都可能误触发。用"Auto-default 项目（path = `$HOME`）兜底迁移"更安全——非破坏性的 INSERT + ALTER TABLE，不会在代码库里留下"会删用户数据"的 migration。

### 3.3 CWD 漂移机制

**Q5 — crash 时的 cwd 写回**：提案说"执行完更新 `session.current_cwd` 并写回 DB"。如果一个 turn 里 LLM 调了 3 个 tool call（`cd frontend` → `read package.json` → `cd ..`），每次 shell tool 执行完都写 DB 是 3 次 UPDATE。

**建议**：只在 turn 结束时写一次（取最后一个成功执行的 shell tool 的 cwd）。中间 crash 了就丢 cwd 更新，下次从 session 记录的 cwd 开始。减少 DB 写入 + 避免"crash 写到一半"。

**Q6 — canonicalize 与 symlink**：**必须用 physical path（canonicalize）。** 否则用户在 project root 内放 symlink 指向 `/etc`，agent `cd symlink` 后就能越界。

但 canonicalize 有一个坑：如果 symlink 目标不存在，canonicalize 返回错误。`assert_within_root` 需要处理这个 case——不存在的路径应该拒绝（因为无法判定它是否在 root 内），返回 `"path does not exist or is a broken symlink"`。

另外有一个**前缀匹配陷阱**：`/home/user/foobar` starts_with `/home/user/foo`？YES——但 `foobar` 不在 `foo` 内。必须 canonicalize 后在路径末尾加 `/` 或做 component-wise 比较：

```rust
fn assert_within_root(root: &Path, target: &Path) -> Result<()> {
    let root_real = root.canonicalize()?;
    let target_real = target.canonicalize()?;
    // 加 '/' 防止前缀匹配陷阱
    let root_str = root_real.to_string_lossy();
    let target_str = target_real.to_string_lossy();
    if target_str == root_str || target_str.starts_with(&format!("{}/", root_str)) {
        Ok(())
    } else {
        Err(anyhow!("'{}' is outside project root", target_real.display()))
    }
}
```

**建议**：把这个 edge case 写入 spec（`.trellis/spec/backend/project-cwd-boundary.md`），而不是在代码里默默处理。

### 3.4 UI 决策

**Q7 — Tab × 关闭后无重新打开 UI**：**强烈建议在空状态页面加"最近隐藏的项目"列表。** 不需要完整的管理面板——就是一个 `list_projects({ hidden: true })` 查询 + 几条模板行，每条带"重新打开"按钮。实现成本 < 30 行，但用户体验差距巨大（手贱关 Tab 后不用开 sqlite）。

**Q8 — 每项目独立 session 集 vs 全局池**：当前方案正确。全局池 + 过滤的 UI 复杂度远高于独立集合，"在所有项目里搜索 session"在 3-5 个项目的场景几乎没有需求。如果未来要做，在 SQL 层加一个跨项目查询就行，store 模型不用改。

**Q9 — `📁` 图标**：`📁` 确实模糊——它的直觉含义是"文件夹/目录"，而非"非 git 项目/无隔离"。建议：
- git 项目 Tab：正常样式，无标记
- 非 git 项目 Tab：文字变灰色/斜体 + hover tooltip "未启用 git 隔离（步骤 4 worktree 不生效）"
- 或用文字标记 `[无隔离]` 比图标更直观

### 3.5 工程开销

**Q10 — 工作量评估**：PROPOSAL 估计 backend ~600 行 + frontend ~400 行。以我对实际代码的理解，更接近：

| 模块 | 估计行数 | 备注 |
|---|---|---|
| db migration + schema_version | ~80 | 比纯 CREATE TABLE 多版本管理逻辑 |
| projects/ 模块（types + store + detector + boundary） | ~250 | CRUD 简单，boundary 需处理 symlink/不存在/前缀陷阱 |
| 改造 db.rs（session CRUD 加 project_id） | ~100 | 主要是给查询加 WHERE + JOIN |
| 改造 lib.rs（chat 命令 + ToolContext 注入） | ~60 | |
| 改造 tools/（3 tool + mod.rs 签名改 + 校验） | ~80 | |
| projects store（Pinia） | ~80 | |
| chat store 改造 | ~40 | |
| ChatWindow.vue Tab 栏 + 空状态 | ~200 | UI 重头 |
| **合计** | **~890** | |

不需要再拆成 3b-1a + 3b-1b。拆分增加的流程开销（两次 review、两次 test）大于它节省的认知负担。

**Q11 — 测试策略**：cwd boundary 的测试确实值得写成 spec。关键 case：
- `cwd == project_root` → ✅
- `cwd == project_root/subdir` → ✅
- `cwd == project_root/../sibling` → canonicalize 后判定（可能在 root 内也可能不在）
- `cwd == project_root + "extra"` → ❌（前缀匹配陷阱，必须加 `/` 防护）
- `cwd == project_root/symlink_to_etc` → ❌（canonicalize 后越界）
- `cwd` 不存在 → ❌（无法判定）
- broken symlink → ❌

### 3.6 远期一致性

**Q12 — v2 path_per_device**：当前抽象够了。`projects.path` 是"本机路径"，v2 同步时加一个 `project_devices(project_id, device_id, path)` 表就行。现有 `projects.path` 变成"当前设备的 path"（从 devices 表 JOIN 或冗余缓存）。不需要现在拆字段。

---

## 四、提案未覆盖的问题

### 4.1 Shell tool 的 working_directory 参数需要过 boundary

PROPOSAL §4.4 说 shell tool 增 `working_directory: Option<String>` 参数。但这是 **LLM 可选的参数**——意味着 LLM 可以在一次 tool call 里指定任意 cwd。必须决定：LLM 指定的 `working_directory` 要不要过 boundary 校验？

**答案是要**——LLM 不应该能通过指定 `working_directory: "/etc"` 来越界。

```
effective_cwd = input.working_directory.unwrap_or(session.current_cwd)
assert_within_root(project.path, effective_cwd)?
// execute with effective_cwd
// session.current_cwd = effective_cwd (persist at turn end, per §3.3 Q5)
```

### 4.2 空状态时用户发消息的交互空白

当前 `chat.ts:498`：没 session 时点发送 → `createNewSession()` 自动创建。新 UI 下没 project 时不能创建 session，但 `send()` 没有处理这个 case。需加：

```ts
if (!projectsStore.currentProjectId) {
  // toast "请先添加项目" 或静默不响应
  return
}
```

### 4.3 SQLite 外键约束需要 PRAGMA

PROPOSAL §3.2 说 messages FK 到 sessions `ON DELETE CASCADE`。当前 schema 有这个约束（`db.rs:119`），但 SQLite 默认不开启外键——需要 `PRAGMA foreign_keys = ON`。当前 `run_migrations` 没设这个 PRAGMA。虽然 `delete_session` 是手动 DELETE 的（cascade 从未实际触发过），但重建表后如果依赖 cascade 行为，必须在连接时开启。

---

## 五、PRD 文档审查

PRD（33 行）结构清晰：Goal → 方案文档位置 → 状态 → 后续动作 → 风险点。一个小问题：

**外部 LLM 评审的提问设计**：PRD 第 16 行写"⏳ 等外部 LLM 评审反馈"，PROPOSAL §9 的 12 个问题发给外部 LLM。但 §9 的提问风格太偏"内部设计争议"——"UUID+可变 path 是否 over-engineer"、"弃旧 sessions 是否过激"——这些问题要求评审者了解 IMPLEMENTATION 路线图、ARCHITECTURE 的 worktree 设计、DESIGN 的 MVP 范围约束。外部 LLM 只能读 PROPOSAL 本身，无法有效回答这些需要跨文档上下文的判断。

**建议**：如果真的要发给外部 LLM，把问题改成只需要读 PROPOSAL 就能回答的形式。例如 Q2 不应问"是否 over-engineer"，而应问"在当前单机场景下，UUID+可变path 相比 path-as-PK 各有什么利弊？你能想到什么场景下必须用 UUID？"

---

## 六、改进建议（按优先级）

### 高优先级（实施前应解决）

1. **确定 migration 策略** — 在 Auto-default 兜底和 DROP TABLE 之间做最终选择。建议 Auto-default，在 PROPOSAL §7 更新方案
2. **设计 ToolContext 结构体** — `tools/mod.rs` 签名改设计 + 三个 tool 实现的改动范围，写入 PROPOSAL §4.4
3. **CWD boundary spec** — 把 canonicalize + symlink + 前缀陷阱 + 不存在路径的 edge case 写进 `.trellis/spec/backend/project-cwd-boundary.md`
4. **空状态交互补全** — 没 project 时点发送 = toast "请先添加项目"

### 中优先级（可边实施边调）

5. **加"最近隐藏的项目"列表** — 空状态页面显示 `hidden=true` 的项目 + 重新打开按钮
6. **每 turn 结束时写 cwd** — 而非每次 shell tool 执行后写
7. **`📁` 图标改为 `⚠️` 或灰色文字** — 提升信息传达准确度

### 低优先级（可延后）

8. **外部 LLM 评审问题重写** — 改为独立可回答的形式
9. **SQLite PRAGMA foreign_keys** — 在新 migration 中显式设置

---

## 七、总结

这份提案的**工程可执行性很高**。Q1-Q9 的设计决策经得起推敲，范围克制，对步骤 4 的前置解锁清晰。PRD 正确地指向 PROPOSAL 为权威来源。

三个阻塞项（ToolContext 签名、migration 策略、create_session 调用链）都可以在 PROPOSAL 层面解决——不需要重新设计，只需要在文档中补全。

**一句话**：提案足以指导实施，但需要把与现有代码的 3 处摩擦在 PROPOSAL §4.4 / §7 / §10 中显式解决，并补上 cwd boundary 的 edge case spec。

---

> 本报告随 PROPOSAL 演进更新。设计决策变更后应重新审查。
