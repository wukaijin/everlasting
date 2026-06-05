# Proposal — 项目基础结构 + 顶部 Tabs UI（步骤 3b-1）

> **状态**:评审反馈已消化,准备转 `implement.jsonl`。
> **背景**:[IMPLEMENTATION §2.4](./IMPLEMENTATION.md#24-步骤-3b--多项目--ui-三栏--rig-迁移-mvp) 原步骤 3b "多项目 + UI 三栏 + Rig 迁移" 拆成:
> - **3b-1(本提案)**:项目数据模型 + 顶部 Tab 入口 + cwd 漂移机制 — 为步骤 4 (Git worktree) 解锁前置数据结构
> - **3b-2(暂缓)**:完整三栏 UI + rig-core 迁移 — 独立做
> **关键依赖**:步骤 4 worktree 路径 `~/.local/share/everlasting/worktrees/<project_uuid>/<session_id>` 必须先有 project 概念。
> **任务**:`.trellis/tasks/06-05-tabs-ui-3b-1/`
> **评审**:
> - [`docs/REVIEW-deepseek-v4-pro-3b-1.md`](./REVIEW-deepseek-v4-pro-3b-1.md) — 工程细节导向
> - [`docs/REVIEW-glm-5.1-3b-1.md`](./REVIEW-glm-5.1-3b-1.md) — 设计完备性 + UX 导向

---

## 1. 目标

让 Everlasting 支持"多项目工作环境":
- 用户可添加多个本地目录作为项目(不强制 git 仓库)
- 顶部 Tabs 在项目间切换
- 每个项目独立的 sessions 集合
- agent 工具调用边界 = project root,cwd 可在内部漂移
- 为步骤 4 worktree 路径解锁 `<project_uuid>` 字段

---

## 2. 设计决策摘要(Q1-Q9 + 评审后定论)

| # | 决策 | 评审后定论 | 理由 |
|---|---|---|---|
| Q1 | 项目 ≠ 强制 git 仓库;`projects.is_git_repo` 字段缓存探测结果 | ✓ 不变 | 允许 `/etc/`、`~/dotfiles`、`/tmp/hack` 当项目用 |
| Q2 | UUID identity + path 是属性(`projects.id` = UUID v4,`projects.path` 可改) | **保持 UUID**(评审建议 path-as-PK,未采纳) | grill 时明确"uuid 肯定要";v2 跨设备(BACKLOG §9)预留接口;评审的"over-engineer"在单机关联不大 |
| Q3 | 弃旧 sessions(DROP TABLE 干净起步) | **改 Auto-default 兜底**(非破坏性 migration) | git history 不留"会删数据"的 migration 代码;用户可后续 reassign |
| Q4 | 非 git 项目接受无 session 隔离,前端 Tab 标 `⚠️`(不弹警告) | ✓ 不变 | 真正防线在步骤 5 权限审计;治本不治标 |
| Q5 | cwd 可在 project root 内自由漂移,不可越出 root | ✓ 不变 + **持久化时机改为 turn 结束一次性写** | 评审深 seek 论据更深:turn 是 agent 状态一致性边界,turn 内中间态 cwd 跨 turn 看见没意义 |
| Q6 | path 可变,约束:新 path 存在 + 无 active session | ✓ 不变 | 守住 typo / 跑一半切边界两条致命底线 |
| Q7 | 无项目时空状态:session 侧栏不渲染;中央居中显示"添加项目" | ✓ 不变 | 防止无项目时建出孤儿 session |
| Q8 | Tab UI 完整设计稿:见 §5.2 | **增"最近隐藏项目"列表** + pick_project_dir dialog 错误处理 + `📁` → `⚠️` | 评审一致建议(空状态 UX 提升) |
| Q9 | 每项目独立 session 集,切 Tab 换 sessions 列表 | ✓ 不变 | 避免"全局池 + 过滤" UI 复杂度 |

**评审消化**(完整记录见 §11):

- **采纳(11 条)**:ToolContext 注入式传递 / 受影响 commands 改造表 / PR 拆 2 个 / worktree 路径术语统一 `<project_uuid>` / pick_project_dir dialog 错误处理(toast 不重弹) / "最近隐藏项目"列表 / `⚠️` 替代 `📁` / PRD 补验收标准 / `PRAGMA foreign_keys = ON` / `send()` project 空值守卫(Q2 drop 后只剩 UI v-if 隔离) / LLM 指定的 `working_directory` 过 boundary 校验
- **未采纳(1 条)**:Q2 改 path-as-PK — 你 grill 时决策,评审建议不成立
- **评审冲突我替你拍(1 处)**:cwd 写 DB 时机 — **turn 结束一次性写**(理由见 Q5)
- **新增关注点**:SQLite `schema_version` 表(评审 §2.2 提) — 实际不需要,本期 migration 是一次性非破坏性 ALTER + INSERT,if-not-exists 模式继续用;真要版本号,PR1 第一步引入

---

## 3. 数据模型

### 3.1 `projects` 表

```sql
CREATE TABLE IF NOT EXISTS projects (
    id           TEXT PRIMARY KEY,        -- UUID v4
    name         TEXT NOT NULL,           -- 显示名,默认 = basename(path),用户可改
    path         TEXT NOT NULL,           -- 本机绝对路径,可改;启动时校验存在
    is_git_repo  INTEGER NOT NULL DEFAULT 0,  -- 缓存探测结果(bool)
    is_legacy    INTEGER NOT NULL DEFAULT 0,  -- Auto-default 兜底项目 = 1
    created_at   TEXT NOT NULL,           -- ISO 8601
    updated_at   TEXT NOT NULL,           -- ISO 8601 (path / name 变更时更新)
    hidden       INTEGER NOT NULL DEFAULT 0,  -- × 关闭 Tab 时置 1(数据保留)
    metadata     TEXT                     -- 预留 JSON
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_projects_path ON projects(path);
CREATE INDEX IF NOT EXISTS idx_projects_updated_at ON projects(updated_at DESC);
```

**字段约束**:
- `path` 全表唯一(同一目录不能加两次,UI 层 + DB 层双重防护)
- `is_git_repo` 应用层维护(添加项目时探测一次,改 path 时重探)
- `is_legacy` 标识 Auto-default 兜底项目 — UI 上可显示 "📦 旧 sessions 自动归入" 标记,用户能 reassign 或删除
- 索引:`updated_at` 倒序用于空状态"最近隐藏项目"列表

### 3.2 `sessions` 表改造

> **SQLite 限制**:`ALTER TABLE ADD COLUMN` 不支持同时加 NOT NULL + FOREIGN KEY。FK ON DELETE CASCADE 由应用层 `delete_session` 显式维护(已有)。

```sql
-- 一次性 DEFAULT 兜底,migration 后所有老 sessions 都有 project_id 指向 __default__
ALTER TABLE sessions ADD COLUMN project_id   TEXT NOT NULL DEFAULT '__default__';
ALTER TABLE sessions ADD COLUMN current_cwd  TEXT NOT NULL DEFAULT '';   -- 立刻 UPDATE 填实际值
CREATE INDEX IF NOT EXISTS idx_sessions_project_id ON sessions(project_id);
```

**`current_cwd` 后续 UPDATE** 必须在 §7 migration 步骤里显式 UPDATE 老行 — 不能靠 DEFAULT,因为 DEFAULT 只能用字面量。

### 3.3 `messages` 表

无需改(messages FK 到 sessions,CASCADE 跟随)。但**本期要让 CASCADE 真正生效** — `PRAGMA foreign_keys = ON` 必须设。

### 3.4 迁移策略(Auto-default 兜底 — 评审 Q4 修正)

```sql
-- 步骤 1:建 projects 表(包含一个 Default 兜底项目)
INSERT OR IGNORE INTO projects (id, name, path, is_git_repo, is_legacy, created_at, updated_at, hidden, metadata)
VALUES (
    '__default__',         -- 固定 UUID,方便 history 关联
    'Legacy / 未分类',     -- 名字提示用户 reassign
    '$HOME',               -- path = $HOME 实际意义,但 sessions 历史 cwd 在此
    0,                     -- 假定非 git(实际探测后可改)
    1,                     -- is_legacy = 1
    datetime('now'), datetime('now'), 0, NULL
);

-- 步骤 2:ALTER TABLE sessions / messages 加 project_id 字段
-- 步骤 3:UPDATE sessions SET project_id = '__default__' WHERE project_id IS NULL;
--         (此步要插在 ALTER ADD COLUMN NOT NULL 之前;实务上:分两步 ALTER 走)
```

**用户视角**:
- 首次启动新版本 → 看到 `Legacy / 未分类` 项目(标 `⚠️ 旧数据`)+ 顶层"+ 添加项目"按钮
- 旧 sessions 自动归入 Legacy 项目,用户**可以**:
  - 用 chat(store 重构后能看到内容,但会标"来自 Legacy")
  - 把 path 改成真实当时的工作目录(如果还记得)— 用 sqlite 手改或以后做"管理项目"面板
  - 把 Legacy 项目 × 关闭 → 连带 sessions 一同 hidden(数据保留)
- **永远不删用户数据**;git history 也不留 DROP 代码

**`is_legacy` 字段意义**:
- UI 上 Tab 标 `📦 旧数据`(或纯文字 `[Legacy]`)
- "管理项目"面板(未来)可"重新分配"孤儿 sessions 到其他项目
- 跑完用户的"reassign 仪式"后可手工 `UPDATE projects SET is_legacy=0`

---

## 4. 后端 — Rust / Tauri

### 4.1 新增模块

```
src-tauri/src/
├── projects/
│   ├── mod.rs           # CRUD + 公共 API
│   ├── types.rs         # Project struct, ProjectId, etc.
│   ├── store.rs         # sqlx queries (list/get/create/update_path/update_name/hide/unhide)
│   ├── detector.rs      # is_git_repo 探测(spawn `git -C <path> rev-parse --show-toplevel`)
│   └── boundary.rs      # cwd 边界校验工具:`assert_within_root(project, requested_cwd)` -> Result
│                          # spec 详见 .trellis/spec/backend/project-cwd-boundary.md
└── db.rs                # 加 projects 表 migration + sessions 改造 + schema 改
```

### 4.2 Tauri commands — 新增 7 个

| Command | 入参 | 出参 | 用途 |
|---|---|---|---|
| `list_projects` | `filter: { hidden: bool? }` | `Vec<ProjectInfo>` | 列项目,默认 hidden=false;空状态页用 `hidden=true` 拿"最近隐藏" |
| `create_project` | `path: String` | `Result<ProjectInfo>` | 添加项目:校验存在 → 探测 is_git_repo → 重复检测 → 写表 |
| `update_project_path` | `id, new_path` | `Result<ProjectInfo>` | 改 path:校验存在 + 无 active session + 重探 is_git_repo |
| `update_project_name` | `id, new_name` | `Result<ProjectInfo>` | 改 name(自由,无约束) |
| `hide_project` | `id` | `Result<()>` | × 关闭 Tab(hidden=1) |
| `unhide_project` | `id` | `Result<()>` | 空状态"最近隐藏项目"列表的"重新打开"按钮 |
| `pick_project_dir` | `()` | `Result<Option<String>>` | 调 Tauri dialog(tree-walk 选目录);`Ok(None)` = 用户取消,`Err(_)` = 弹 dialog 失败 / backend 校验目录不存在。前端 toast 提示,不重弹 dialog |

### 4.3 现有 commands 改造(评审 GLM §1.1 提)

| 现有 command | 现状签名 | 改为 | 备注 |
|---|---|---|---|
| `create_session` | `async fn create_session(model: String) -> Result<Session>` | `async fn create_session(project_id: String, initial_cwd: String, model: String) -> Result<Session>` | 改前端 `invoke("create_session", { projectId, initialCwd, model })`;`initial_cwd` 默认 = `project.path` |
| `list_sessions` | 无 project 过滤 | `async fn list_sessions(project_id: String) -> Result<Vec<Session>>` | WHERE project_id = ?;前端 `chat.ts:loadSessions` 传当前 projectId |
| `load_session` | 返回 `Session` | 返回 `Session` (含 `project_id` 字段) | 前端切换 session 时校验 `session.project_id === currentProjectId` |
| `delete_session` | `delete_session(id: String)` | 不变 | CASCADE 由 PRAGMA foreign_keys = ON 接管;显式 `DELETE FROM messages WHERE session_id = ?` + `DELETE FROM sessions WHERE id = ?` |
| `chat` | `chat(request_id, session_id, messages)` | 不变签名;内部增 project 反查 + ToolContext 构造 | 详见 §4.4 |

### 4.4 `chat` command + ToolContext 改造(评审一致提)

```rust
// 新增 src-tauri/src/tools/mod.rs
#[derive(Clone, Debug)]
pub struct ToolContext {
    pub project_root: PathBuf,   // 来自 project.path(canonicalize)
    pub cwd: PathBuf,            // 来自 session.current_cwd
}

#[derive(Clone, Debug)]
pub struct ToolContextUpdate {
    pub new_cwd: PathBuf,        // turn 内累计的最新 cwd
}

pub async fn execute_tool(
    name: &str,
    input: &serde_json::Value,
    ctx: &ToolContext,
) -> (String, bool)              // (output, is_error)
```

**ToolContext 传递机制**(评审 GLM §1.2 提的):
- `tools/mod.rs::execute_tool` 改签名,加 `ctx: &ToolContext` 参数
- `lib.rs:chat` 命令内构造 `ToolContext`:`let ctx = ToolContext { project_root: project.path, cwd: session.current_cwd }`
- **不**给 tools 持 state 引用;保持纯函数,可测试
- 调用点:`tools::execute_tool(name, input, &ctx)`

**Tools 改造面**(评审 GLM §1.2 提):
- `shell.rs`:增 `working_directory: Option<String>` 参数;**LLM 指定的 working_directory 必须过 boundary 校验**(评审深 seek §4.1 提);`effective_cwd = input.working_directory.unwrap_or(ctx.cwd)` → `assert_within_root(ctx.project_root, effective_cwd)?`
- `read_file.rs`:相对路径按 `ctx.cwd` 解析;绝对路径必须 `canonicalize` 后校验在 `ctx.project_root` 内
- `write_file.rs`:同 read_file

**cwd 持久化时机**(评审深 seek Q5 + 拍板):
- **turn 结束一次性写**,不是每次 shell tool 执行后写
- `lib.rs:chat` 命令的 agent loop 末尾:`if new_cwd != session.current_cwd { db::update_session_cwd(session_id, &new_cwd).await }`
- turn 内多次 shell 调用 → 把新 cwd 累积到一个 `ctx.clone()` 风格的 mutable state(每次 shell 返回携带 `ToolContextUpdate { new_cwd }`),turn 结束取最后一次

### 4.5 LlmContext / AppState

```rust
struct AppState {
    db: DbPool,
    // 现有字段...
}
// 不在 AppState 缓存"当前 project_id" — 每次 chat 用 session_id 反查 project_id
// 前端选 Tab 是 UI 层状态,不污染 backend
```

### 4.6 `db.rs` 改造

```rust
// 现有:create_session(pool: &SqlitePool, model: &str) -> Result<Session>
// 改为:create_session(pool, project_id: &str, initial_cwd: &str, model: &str) -> Result<Session>

// 现有:list_sessions(pool) -> Result<Vec<Session>>
// 改为:list_sessions(pool, project_id: &str) -> Result<Vec<Session>>

// 新增:update_session_cwd(pool, session_id, new_cwd) -> Result<()>
//      list_hidden_projects(pool) -> Result<Vec<ProjectInfo>>
//      create_project(pool, path) -> Result<ProjectInfo>
//      hide_project(pool, project_id) -> Result<()>
//      unhide_project(pool, project_id) -> Result<()>
//      update_project_path(pool, project_id, new_path) -> Result<ProjectInfo>
//      update_project_name(pool, project_id, new_name) -> Result<ProjectInfo>
```

---

## 5. 前端 — Vue 3 / Pinia

### 5.1 新增 store

```ts
// src/stores/projects.ts
export const useProjectsStore = defineStore('projects', () => {
  const projects = ref<ProjectInfo[]>([])
  const currentProjectId = ref<string | null>(null)
  
  async function loadProjects() { /* invoke('list_projects', { filter: { hidden: false } }) */ }
  async function loadHiddenProjects() { /* invoke('list_projects', { filter: { hidden: true } }) */ }
  async function addProject() { /* pick_project_dir → create_project → load → 切到新 Tab */ }
  async function switchProject(id) { /* 设 currentProjectId,触发 sessions 重新 load */ }
  async function hideProject(id) { /* invoke('hide_project') → reload → 若刚 hide 的是当前,切到剩下第一个 Tab(或空状态) */ }
  async function unhideProject(id) { /* invoke('unhide_project') → reload → switch */ }
  async function renameProject(id, name) { /* invoke('update_project_name') → reload */ }
  
  return { projects, currentProjectId, loadProjects, loadHiddenProjects, addProject, switchProject, hideProject, unhideProject, renameProject }
})
```

### 5.2 修改 `chat` store(Q9 决策 + 评审 GLM §1.1)

```ts
const projectsStore = useProjectsStore()

watch(() => projectsStore.currentProjectId, async (newProjectId) => {
  if (newProjectId) {
    await loadSessions({ projectId: newProjectId })
  } else {
    sessions.value = []
    currentSessionId.value = null
  }
})

async function createNewSession() {
  if (!projectsStore.currentProjectId) {
    // 评审 GLM §1.1 + 深 seek §4.2:守卫
    showToast('请先添加项目', 'warn')
    return
  }
  const project = projectsStore.projects.find(p => p.id === projectsStore.currentProjectId)
  const session = await invoke('create_session', {
    projectId: projectsStore.currentProjectId,
    initialCwd: project.path,
    model: config.model,
  })
  // ...
}

async function send(text: string) {
  if (!projectsStore.currentProjectId) {
    showToast('请先添加项目', 'warn')
    return
  }
  // ... 现有 send 逻辑
}
```

### 5.3 ChatWindow.vue 顶部 Tab 栏

**空状态**(评审 GLM §3.2 提"最近隐藏项目"列表):

```
┌──────────────────────────────────────────────────────────┐
│  Everlasting                                             │
├──────────────────────────────────────────────────────────┤
│  [+ 添加项目]                                            │  ← Tab 栏只有 + 按钮
├──────────────────────────────────────────────────────────┤
│                                                          │
│              还没有项目                                  │
│                                                          │
│         点上方「+ 添加项目」,从文件系统选个目录开始      │
│                                                          │
│              [+ 添加项目]                                │  ← 大居中按钮
│                                                          │
│  ─────────────────────────────────────                   │
│                                                          │
│  最近隐藏的项目:                                          │  ← 空状态时才出现
│  📦 Legacy / 未分类  [重新打开]  [彻底删除] (sqlite only)│
│  📁 dotfiles         [重新打开]                          │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

**正常态**(评审 GLM §4.1 改标记):

```
┌──────────────────────────────────────────────────────────┐
│  Everlasting                                             │
├────────────────────────────────────────────┬─────────────┤
│ [everlasting]  [dotfiles ⚠️]  [tmp ⚠️]  [+] │             │  ← 非 git 标 ⚠️
│════════════                                │             │     底部蓝线
├──────────────────────────────────────┬─────┤  statusbar  │
│ Sessions          │  [chat area]      │     │             │
│ + 新对话          │                    │     │             │
│ ─ session a ▸     │                    │     │             │
└───────────────────┴────────────────────┴─────┴─────────────┘
```

**Tab 规格**:

| 项 | 设计 |
|---|---|
| Tab 标题 | `project.name`(默认 = `basename(path)`,用户可改) |
| Tab tooltip | 完整 `path`(hover 显示) |
| Tab 选中标识 | 底部 2px 蓝色高亮线 + 浅色背景 |
| 关闭按钮 (×) | hover 显示(selected 始终显示);点击 = `hide_project`(数据保留) |
| **非 git 标记** | `⚠️` 12px 图标右上角(评审深 seek §3.4 / GLM §4.1 改,`📁` 含义模糊) |
| Legacy 标记 | `📦` 12px 图标(用于 Auto-default 兜底项目) |
| Tab 高度 | 36px;Tab 宽度 min 100px / max 240px / 溢出 ellipsis |
| 排序 | 按 `created_at` 升序追加;未来要拖动再加 |
| 数量上限 | 无;`overflow-x: auto` 横向滚动 |
| "+ 添加项目" | 紧贴最右 Tab,固定不随滚动消失 |

### 5.4 `pick_project_dir` 错误处理(Q8v2 决议)

Tauri `pick_folder` dialog 本身是从根目录一级级展开的 tree-walk,**没有**"手动输入 path"这条 UX 路径。dialog 的三种结局对应三种 UX:

```ts
// projects.ts addProject():
async function addProject() {
  let result: { path: string | null; err: string | null } = { path: null, err: null }
  try {
    const path = await invoke<string | null>('pick_project_dir')   // null = 用户取消
    result = { path, err: null }
  } catch (e) {
    result = { path: null, err: String(e) }  // dialog 弹失败 / 目录不存在
  }

  if (result.path) {
    // 选好了 → create_project + 切到新 Tab
  } else if (result.err) {
    // 失败 → toast 错误,不重弹 dialog(让用户主动重选)
    showToast(`添加项目失败: ${result.err}`, 'error')
  }
  // result.path == null && result.err == null → 用户取消,静默啥也不做
}
```

**核心**:
- **取消 ≠ 失败**:取消是用户主动意图(`Ok(None)`),静默
- **失败要明确告知**:`Err(_)`(dialog 弹不出 / 目录不存在)→ toast 错误,让用户知道发生了什么
- **不重弹 dialog**:失败后强加重弹是"反取消"语义 + 死循环风险(dialog 弹不出时)

跟 Q8 决议结合:Q8 选 (a) 取消静默 / dialog 弹失败 toast / 目录不存在 toast。

### 5.5 启动流程

```
app onMounted:
  1. config.load()                          // 现有
  2. projects.loadProjects()                // 新增(list_projects, hidden=false)
  3. if (projects.length === 0):
       currentProjectId = null → 渲染空状态(含"最近隐藏项目"列表,如适用)
     else if (lastActiveProjectId in projects):
       currentProjectId = lastActiveProjectId   // 从 localStorage 恢复
     else:
       currentProjectId = projects[0].id        // 默认第一个
  4. sessions.loadSessions({ projectId: currentProjectId })  // 现有(改造)
```

---

## 6. 衔接步骤 4(Git 集成)

本提案为步骤 4 提供:
- ✅ `project_uuid` 字段(worktree 路径 `<project_uuid>/<session_id>` 可算)
- ✅ `is_git_repo` 探测结果(步骤 4 跳过非 git 项目的 worktree 创建)
- ✅ `session.current_cwd` 持久化(步骤 4 worktree 创建后,session.current_cwd 初始 = worktree 路径)

**术语统一**(评审 GLM §1.3 提):ARCHITECTURE §3 当前的 `project_hash` 改 `project_uuid`,PR1 一并改。

步骤 4 实施时,只需:
- 在 `create_session` 命令里:if (`project.is_git_repo`) → 调 `git worktree add ~/.local/share/everlasting/worktrees/<project_uuid>/<session_id> -b session/<session_id>`,设 `session.current_cwd = worktree_path`
- if (`!project.is_git_repo`) → `session.current_cwd = project.path`(无隔离)

---

## 7. 升级路径(非破坏性 migration)

```
版本 N(现在):
  sessions(id, title, created_at, updated_at, model, metadata)
  messages(id, session_id, role, content, ...)

升级到 N+1(本提案) — 启动时一次事务:
  1. PRAGMA foreign_keys = ON;
  2. CREATE TABLE IF NOT EXISTS projects(...);
  3. INSERT OR IGNORE INTO projects VALUES('__default__', 'Legacy / 未分类', $HOME, 0, 1, ...);
  4. ALTER TABLE sessions ADD COLUMN current_cwd  TEXT NOT NULL DEFAULT '';          -- 先加,默认空
  5. UPDATE sessions SET current_cwd = (SELECT path FROM projects WHERE id = '__default__') WHERE current_cwd = '';   -- 填上 __default__ 的 path
  6. ALTER TABLE sessions ADD COLUMN project_id   TEXT NOT NULL DEFAULT '__default__';   -- 再加
  7. CREATE INDEX idx_sessions_project_id ON sessions(project_id);

  注:SQLite 3.35+ 支持 ALTER ADD COLUMN NOT NULL DEFAULT,旧版(3.34-)需拆两步:
    - ALTER ... ADD COLUMN project_id TEXT;
    - UPDATE sessions SET project_id = '__default__' WHERE project_id IS NULL;
    - 应用层后续保证 NOT NULL。
  PR1 实施时探测 sqlite version,选合适的 migration 写法。
```

**关键**:这是**非破坏性 migration**,git history 不留 "DROP TABLE sessions"。所有老 sessions 自动归入 `__default__` 项目,标 `is_legacy=1`,UI 上 Tab 标 `📦`,用户可见可重分配可删除。

---

## 8. 范围 / Out of Scope

**本期做**(PR1 + PR2):
- projects 表 + sessions 改造 + migration
- 顶部 Tab 栏 + 空状态 + "最近隐藏项目"列表
- cwd 漂移机制 + tools 边界校验
- Tauri commands(7 新增 + 4 现有改造)
- 前端 projects store + chat store 改造
- `pick_project_dir` 错误处理(toast 不重弹)
- PRD 验收标准补全 + PR 映射

**明确不做**(留给后续):
- ❌ 项目"真删除" UI(sqlite 手改)
- ❌ "管理项目"完整面板(本期只做"最近隐藏"列表)
- ❌ Tab 拖动排序
- ❌ 项目级 memory (CLAUDE.md per-project)
- ❌ trellis ↔ projects 关联
- ❌ rig-core 迁移(步骤 3b-2)
- ❌ 完整三栏 UI(步骤 3b-2)
- ❌ update_project_path UI 暴露

---

## 9. PR 拆分(评审 GLM §3.1 建议)

### PR1: 数据模型 + 后端(无 UI 变更)

- `db.rs` migration + projects CRUD + sessions 改造
- `projects/` 新模块(types / store / detector / boundary)
- `tools/mod.rs` ToolContext 引入 + execute_tool 签名改
- `tools/shell.rs` `read_file.rs` `write_file.rs` cwd 边界校验
- `lib.rs` `chat` 命令改造 + create_session / list_sessions / load_session / delete_session 改造
- ARCHITECTURE §3 worktree 路径术语 `project_hash` → `project_uuid`
- Unit tests:`projects/store.rs` CRUD / `boundary.rs` 7 个 edge case(评审深 seek Q11)
- **PR1 完后应用功能不变(前端没改),后端可独立 review + 测**

### PR2: 前端 Tab 栏 + 空状态 + store 改造

- `src/stores/projects.ts` 新增
- `src/stores/chat.ts` 改造(watcher + createNewSession / send 守卫)
- `src/components/ChatWindow.vue` 顶部 Tab 栏 + 空状态 + "最近隐藏项目"列表
- `pick_project_dir` + 手动输入 fallback
- 端到端测试:加项目 → 建 session → 漂移 cwd → 越界拒绝 → 切 Tab → 关闭 Tab → 重新打开

**为何不拆 3 个或更多**:评审深 seek §3.5 提的"流程开销大于认知节省",GLM §3.1 也建议 2 个。2 个是平衡点。

---

## 10. 工作量评估(评审深 seek §3.5 / GLM §3.1 估)

| 模块 | 估行 | 备注 |
|---|---|---|
| db migration + projects CRUD | ~150 | Auto-default 兜底 migration ~30 行 |
| `projects/` 模块(types + store + detector + boundary) | ~250 | boundary spec 7 edge case 单元测试 |
| `db.rs` sessions 改造(create_session 签名改 / list_sessions 过滤) | ~80 | |
| `lib.rs` `chat` + ToolContext 注入 | ~60 | |
| `tools/` 改造(3 tool + mod.rs 签名改 + 校验) | ~100 | boundary spec 见 spec 文档 |
| 前端 projects store | ~80 | |
| `chat.ts` 改造 | ~50 | |
| `ChatWindow.vue` Tab 栏 + 空状态 + 隐藏项目列表 | ~250 | UI 重头 |
| `pick_project_dir` 错误处理(toast 不重弹) | ~10 | 取消静默 / 失败 toast |
| ARCHITECTURE 文档 + spec 文档 | ~150 | 评审要求 |
| **合计** | **~1220** | 含测试 / spec / 文档 |

---

## 11. 评审消化(完整记录)

| 评审来源 | 反馈 | 采纳 | 备注 |
|---|---|---|---|
| 深 seek §2.1 | ToolContext 注入式 | ✅ | §4.4 |
| 深 seek §2.2 | schema_version 表 | ❌ | 本期非破坏性 migration 不需要;若未来加 DROP 重写,引入 |
| 深 seek §2.3 | create_session 改造 | ✅ | §4.3 |
| 深 seek §3.1 Q1 | Container 模型 | ✅ | 不变 |
| 深 seek §3.1 Q2 | path-as-PK | ❌ | 保持 UUID |
| 深 seek §3.1 Q3 | non-git 标记 | ✅ | §5.3 改 ⚠️ |
| 深 seek §3.2 Q4 | Auto-default 兜底 | ✅ | §3.4 |
| 深 seek §3.3 Q5 | turn 结束写 cwd | ✅ | §4.4 |
| 深 seek §3.3 Q6 | canonicalize + symlink | ✅ | 详见 spec |
| 深 seek §3.4 Q7 | 隐藏项目列表 | ✅ | §5.3 |
| 深 seek §3.4 Q8 | 独立 session 集 | ✅ | 不变 |
| 深 seek §3.4 Q9 | ⚠️ 替代 📁 | ✅ | §5.3 |
| 深 seek §3.5 Q10 | 不拆 3 个 PR | ✅ | §9 拆 2 个 |
| 深 seek §3.5 Q11 | boundary spec | ✅ | `.trellis/spec/backend/project-cwd-boundary.md` |
| 深 seek §3.6 Q12 | v2 path_per_device 不用拆 | ✅ | §3.4 |
| 深 seek §4.1 | LLM working_directory 过 boundary | ✅ | §4.4 |
| 深 seek §4.2 | send() 守卫 | ✅ | §5.2 |
| 深 seek §4.3 | PRAGMA foreign_keys | ✅ | §3.2 |
| 深 seek §5 | §9 提问风格改 | ❌ | 本期不需要 — 直接走两份外部评审消化 |
| GLM §1.1 | 受影响 commands 改造表 | ✅ | §4.3 |
| GLM §1.2 | ToolContext 传递机制明确 | ✅ | §4.4 |
| GLM §1.3 | worktree 路径术语统一 | ✅ | §6 |
| GLM §2.1 | cwd 每次 sync | ❌ | 改 turn 结束一次性 |
| GLM §2.2 | symlink 边界 | ✅ | spec |
| GLM §2.3 | Auto-default 兜底 | ✅ | §3.4 |
| GLM §3.1 | 拆 2 PR | ✅ | §9 |
| GLM §3.2 | 隐藏项目列表 | ✅ | §5.3 |
| GLM §4.1 | ⚠️ 替代 📁 | ✅ | §5.3 |
| GLM §4.2 | pick_project_dir 跨平台 fallback(手动输入) | ❌ 撤回 | Tauri `pick_folder` dialog **本身就是** tree-walk,无"手动输入"路径。改为"错误处理 + toast 不重弹"。评审当时基于错前提提,撤回 |
| GLM §5 | PRD 补验收标准 | ✅ | PRD 改 |

**冲突拍板**:
- cwd 写 DB 时机:深 seek vs GLM → **turn 结束一次性**(深 seek 论据更深)
- migration 策略:两份一致 → **Auto-default 兜底**
- Q2 UUID:深 seek 反对,GLM 不表态,你 grill 原意 → **保持 UUID**

---

## 12. 关联文档

- 同类工具:Claude Code workspace 模型 / Cursor "Open Folder" / Aider repo map
- 已有设计:[DESIGN §3.1 MVP](./DESIGN.md#31-mvp核心必做) / [ARCHITECTURE §3 worktree](./ARCHITECTURE.md#3-决策每个-session-一个-git-worktree) / [BACKLOG §9 跨设备](./BACKLOG.md#9-跨设备v2-候选)
- 实施时引用的 spec:`.trellis/spec/backend/project-cwd-boundary.md`(本任务创建,边界校验 edge case)
- 任务 prd:`.trellis/tasks/06-05-tabs-ui-3b-1/prd.md`
