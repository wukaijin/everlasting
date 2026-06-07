# step 4 follow-up: 解耦 session 与 worktree(手动 attach / detach / delete)

## Goal

把 worktree 从 session 生命周期的硬性绑定拆出来,改成按需 opt-in:
- session 创建时不再自动建 worktree、不再要求项目是 git 仓库
- git 项目的 session 可在前端手动 `attach_worktree` 创建隔离工作区
- 可 `detach_worktree` 解绑(session 回到 project root,worktree 留盘)
- 可 `delete_worktree` 销毁 worktree 文件 + branch(独立于 detach)
- 修掉"非 git 项目无法创建 session / 发消息"的实际问题
- 状态机要可恢复:worktree 创建失败不阻塞用户发消息
- destructive 行为有安全网(检查 uncommitted、in-flight cancel、confirm 弹窗)
- LLM 对 worktree 状态切换有显式感知,避免认知断裂

## Decision (ADR-lite)

| 决策 | 拍板 | 理由 |
|---|---|---|
| 三个独立操作 | attach / detach / delete 三个 Tauri command + 三个 UI 按钮 | detach 只解绑留盘,delete 物理销毁,可分步走避免误删 |
| 状态机三态 | none / active / detached | 区分"从未用过"和"用过又解绑",留给用户后悔药 |
| Schema 编码 | 加 `worktree_state TEXT` + `last_worktree_path TEXT NULL` | 类型安全、显式可读、前端不用组合判断 |
| 中途 attach 边界 | 拒绝 + 提示 | 跟 git CLI 的 `worktree add` 行为一致,语义清晰,不让 user 隐式丢失改动 |
| UI 入口 | ChatPanel 头部 chip + 下拉菜单 | 状态机多了下拉最自然,跟现有 diff chip 合并 |
| Attach 失败 UX | toast 提示 + 回到 none 状态 | 轻量、可恢复、少一步操作 |
| 数据迁移 | 启动时 `UPDATE ... SET worktree_state='active' WHERE worktree_path IS NOT NULL AND worktree_state IS NULL` | 无脑 startup 脚本完事,默认 'none' 正确 |
| 原子性 | disk first, then DB + best-effort 回滚(跟 PR1 一致) | 跨介质一致,失败容忍 |
| Detach 安全检查 | 拒绝 uncommitted,跟 attach 对称 | libgit2 `repo.status()` 检 worktree_path,有未 commit/stash → 拒绝 + 提示 |
| In-flight 处理 | hybrid:delete_session 自动 cancel;detach/delete worktree 前端 disabled(预防) + 后端兜底 cancel | 兼顾 UX(预防为主)跟安全(后端防御) |
| Delete confirm | 仅 active+有 diff 时弹 modal | 无 diff 一键,省操作;有 diff 弹 modal 保护 |
| **LLM 透明度** | **工具返回加 cwd 字段 + 切 worktree 时 system event** | 既能验证路径,又能感知状态变更,避免认知断裂 |
| **Merge 流程** | **本 task OOS,另开 task** | 控制 MVP 范围,merge 涉及冲突处理 / target 选择 / 清理策略,独立 task 谈 |

## Requirements (最终)

### 核心解耦
- **REQ-1** `create_session` 不再要求 project 是 git repo(去掉 `lib.rs:280-285` 的守卫)
- **REQ-2** `create_session` 不再自动建 worktree(去掉 `lib.rs:287-325` 的 worktree 编排)

### 三个 Tauri commands
- **REQ-3** 新 Tauri command `attach_worktree(sessionId)`,在 git 项目上为该 session 建 worktree + branch + 更新 session row;非 git 项目后端拒绝 + 前端 disable 按钮
- **REQ-4** 新 Tauri command `detach_worktree(sessionId)`:
  - 后端先 `Repository::open(worktree_path).status()?` 检 uncommitted,有 → 拒绝 + 错误 "worktree has uncommitted changes; commit/stash before detach"
  - 改 session row:worktree_path=NULL,worktree_state='detached',last_worktree_path=原值
- **REQ-5** 新 Tauri command `delete_worktree(sessionId)`,复用 `git::destroy` 清文件 + branch + metadata,session row 同步清(纯 DB 操作)

### Schema + 迁移
- **REQ-6** `db.rs` 加 `worktree_state TEXT NOT NULL DEFAULT 'none'` + `last_worktree_path TEXT` 列(走 `add_session_column_if_missing` 模式);`SessionRow` / `SessionSummary` 携带
- **REQ-7** 启动时执行一次性 migration:存量 step 4 session 回填 `worktree_state = 'active'`

### 边界检查
- **REQ-8** Attach 中途边界:后端 `Repository::open(project_path).status()` 检 uncommitted changes → 返回明确错误,前端 toast 提示
- **REQ-9** Detach 时同样检 worktree 自身 uncommitted,见 REQ-4

### Frontend UI
- **REQ-10** Frontend ChatPanel 头部 chip 重构:状态动态 (none=🔲 attach / active=⫶ diff(N) ▼ / detached=⫶ 上次 worktree ▼)
- **REQ-11** Frontend `chat.ts` 加 actions: attachWorktree / detachWorktree / deleteWorktree + 缓存失效
- **REQ-12** Frontend `chat.ts` 加流式状态 computed: `isStreaming` 来自 store(已有)
- **REQ-13** Frontend ChatPanel 头部下拉里的 `detach` / `delete worktree` 按钮: `disabled = chatStore.isStreaming`;`attach` 不 disable(允许 streaming 中 attach)
- **REQ-14** Frontend delete worktree 按钮:`worktree_state === 'active' && diffCache.files.length > 0` 时弹 confirmation modal;"<N> files 会被销毁,确定吗?" 确认/取消两按钮
- **REQ-22** Frontend ChatPanel 头部下拉菜单加 2 个 menu item:"复制 worktree path" / "复制 branch name"(**NEW**:为 LLM 手动 merge 提供信息)
- **REQ-23** 显示条件:`worktree_state in ('active', 'detached')`,即 `none` 时不显示
- **REQ-24** 复制值:
  - active: `session.worktree_path` + `session/<session_id>` (branch name)
  - detached: `session.last_worktree_path` + `session/<session_id>`(branch 永久存在,merge 时仍可用)
- **REQ-25** 复制实现:`navigator.clipboard.writeText` + 成功 toast "已复制 <label>"
- **REQ-26** 这两个复制按钮在 `isStreaming` 时 **不 disable**(只读操作,无副作用)

### Destructive 安全网
- **REQ-15** Backend 三 destructive 路径 (`delete_session` / `detach_worktree` / `delete_worktree`) 入口统一加 in-flight cancel:
  - 从 `cancellations` map 查 `request_id`(如有),`token.cancel()`
  - 不存在则跳过(in-flight 已结束)
  - 然后继续 destructive

### LLM 透明度(**NEW**)
- **REQ-16** 7 个 read 类工具(read_file / shell / list_dir / glob / grep)返回 JSON 加 `cwd: String` 字段,值是 `ctx.worktree_path.to_string_lossy()`。write_file / edit_file 也加(对 LLM 写后能看到路径)。让 LLM 看到工具结果时知道"这文件是哪个路径下的"。
- **REQ-17** 切 worktree 状态(attach / detach / delete)时,后端在 session messages 表里 insert 一条 system event:
  - `role = 'user'`, `content = '[worktree event] <event description>'`(或更结构化的 role='tool' / name='worktree_event' 形式,实施时定)
  - attach: `worktree attached: <path> on branch session/<id>`
  - detach: `worktree detached from <path> (changes preserved on branch session/<id>)`
  - delete: `worktree deleted: branch session/<id> and dir <path> removed`
  - LLM 下一次 chat 时 messages 数组里包含这条,LLM 明确感知状态变更
- **REQ-18** 切 worktree 前先 cancel in-flight chat(REQ-15 的副作用自然保证);system event 注入在 cancel 之后、新一轮 chat 之前;LLM 的"下一轮"看到事件

### 文档 + 回归
- **REQ-19** 文档更新:prd.md 标 step 4 改为 opt-in,ARCHITECTURE.md §3 加手动 attach/detach/delete 段 + LLM 透明度机制
- **REQ-20** 回归:PR2 的 NULL fallback 仍生效,7 工具 worktree_path 缺失时回退 project.path
- **REQ-21** 8 个 db::tests 调用点更新到新 schema;新增 attach/detach/delete 单元测试;新增 system event 注入测试

## Acceptance Criteria (最终)

### 基础 + 解耦
- [ ] 非 git 项目可正常创建 session 并发消息(原 bug 修复)
- [ ] git 项目创建 session 不自动建 worktree,`sessions.worktree_path` 默认 NULL,`worktree_state` 默认 'none'
- [ ] 前端在 session 头部看到 attach worktree 按钮,点击 → worktree 创建成功 + 按钮变成"diff (N) ▼"下拉
- [ ] Attach 时若 project root 有 uncommitted changes,后端返回明确错误,前端 toast + 回到 none
- [ ] Attach 失败(libgit2 错误)toast 提示,chip 回到 none

### Detach / Delete
- [ ] Detach 时若 worktree 自身有 uncommitted changes,后端返回错误,前端 toast,worktree 跟 session 都不动
- [ ] Detach 成功后 session 仍可发消息,工具 worktree_path 回退 project.path,chip 变"上次 worktree (N files) ▼"
- [ ] Delete 后 worktree 文件 + branch 物理消失,session 仍存在但 worktree_path / worktree_state 置空
- [ ] Delete_worktree 在 active+有 diff 时弹 confirmation modal,确认后执行
- [ ] Delete_worktree 在 active+无 diff / detached 状态时一键执行(不弹 modal)
- [ ] Session.delete 仍 best-effort 清理 worktree(基于 worktree_path IS NOT NULL 判断)
- [ ] **复制按钮(active + detached)**:头部下拉菜单在 active 时显示"复制 worktree path" + "复制 branch name",值来自 `worktree_path` / `session/<id>`(**NEW**)
- [ ] **复制按钮(detached)**:detached 时也显示,值来自 `last_worktree_path` / `session/<id>`,给 LLM 手动 merge 用
- [ ] **复制按钮(none)**:none 时菜单里不出现这两项
- [ ] **复制行为**:点击 → `navigator.clipboard.writeText` + toast 提示"已复制 <label>",`isStreaming` 时不 disable

### In-flight cancel
- [ ] Delete session 时若 LLM 正在 stream,后端先调 cancel_chat 中断,然后删 session
- [ ] Detach worktree / delete worktree 按钮在 `chatStore.isStreaming` 时前端 disabled(灰显)
- [ ] 异常路径(前端 disabled 已设但 IPC 在途中):后端仍调 cancel_chat

### LLM 透明度(**NEW**)
- [ ] 7 个 read 类工具 + write_file + edit_file 的返回 JSON 都带 `cwd` 字段,值 = `ctx.worktree_path.to_string_lossy()`
- [ ] Attach worktree 后,LLM 下一次 chat 的 messages 数组里能看到 system event "worktree attached: <path> on branch session/<id>"
- [ ] Detach 后 messages 里有 "worktree detached from <path> (changes preserved on branch session/<id>)"
- [ ] Delete 后 messages 里有 "worktree deleted: branch session/<id> and dir <path> removed"
- [ ] LLM 在切 worktree 后的工具调用里能拿到正确的 cwd,不会跨视图困惑
- [ ] 切 worktree 跟 in-flight chat 冲突时:先 cancel(防御)+ insert event(在 cancel 后)+ LLM 下一轮看到

### 迁移 + 回归 + 质量
- [ ] 存量 step 4 session 启动后 worktree_state = 'active',UI 行为跟新 session 一致
- [ ] 8 个 db::tests + 新增 10+ 单元测试全过(166 → 估计 180+)
- [ ] `vue-tsc --noEmit` 0 errors,`cargo check` 0 warnings,`pnpm build` 成功
- [ ] 文档(prd.md / ARCHITECTURE.md / HANDOFF.md)同步更新

## Definition of Done

- cargo test 全过,新增 attach/detach/delete 单元测试 + migration 测试 + in-flight cancel 路径测试 + system event 注入测试 + 7 工具 cwd 字段测试
- vue-tsc / cargo check / pnpm build 全绿
- 手动 smoke test 矩阵:
  - 4 种 session 状态(none / active / detached / legacy-active) × 3 个动作(attach / detach / delete) 全跑
  - 3 种 streaming 场景(stop 后 / streaming 中 disable / 异常穿透到后端) 全跑
  - 3 种 confirm 场景(active+diff / active+clean / detached) 全跑
  - LLM 透明度:attach/detach/delete 后发一条 chat,确认 LLM 能看到 system event;切 worktree 前后的工具调用 cwd 正确
- prd.md 标 step 4 决策变更,ARCHITECTURE.md §3 更新 + 加 LLM 透明度段,HANDOFF.md 更新 on-session-create 步骤
- 跟 PR2 的 NULL fallback / 7 工具兼容,边界检查仍生效

## Out of Scope (explicit)

- **merge worktree 流程**(冲突处理、target branch 选择、merge 后清理)— 另开 task
- worktree 共享/合并(目前每个 session 独立,不变)
- 自动 commit(PR1 显式 OOS,继续 OOS)
- worktree 内容预览(只保留 diff 视图,文件树浏览另开 task)
- 老 PR 的 2 个孤儿 worktree metadata 目录清理(沿用 58d7852 commit message 的 `git worktree prune` 一次性处理建议)
- RDP 双显示器 bug 1+2 修复(沿用现有 TODO)
- 跨平台:Windows / macOS data_dir 适配(沿用 worktree.rs::data_dir() 的 WSL/Linux first,后期单独 task)
- "切 session" 时的 in-flight cancel(切 session 不算 destructive,旧 session 的 LLM 跑就让它跑完)

## Technical Approach

### 关键改动
- 后端 lib.rs 简化 `create_session`(去掉 ~30 行 git 守卫 + worktree 编排),新增 3 个 Tauri command,3 destructive 路径加 in-flight cancel hook,system event 注入
- 后端 db.rs 加 2 列 + 启动 migration + 调整 8 tests
- 后端 git/worktree.rs 加 `check_clean` 工具函数(给 attach / detach 共用)
- 后端 7 工具返回加 cwd 字段(需改 ToolResult 类型)
- 前端 ChatPanel.vue 重构头部 chip(从单一 diff chip 改为三态下拉)+ isStreaming disabled 逻辑
- 前端 chat.ts 加 4 actions (attach/detach/delete + 缓存失效)
- 前端 delete confirm modal(简单组件)
- 文档三连更 + LLM 透明度文档

### 实施拆分(小 PR 思路)
- **PR1(本 task 核心)**:核心闭环 + LLM 透明度
  - 后端:db.rs 列 + migration + helpers;lib.rs::create_session 简化 + 3 commands + destructive cancel + system event 注入;git module 复用 + check_clean;7 工具 cwd 字段
  - 前端:ChatPanel 头部三态 chip + 下拉(含 2 个复制菜单项);chat.ts 4 actions + isStreaming disabled;DeleteWorktreeConfirm modal
- **PR2(本 task 收尾)**:工具 + UI 收尾
  - ToolCallCard edit_file diff popover 条件适配
  - 错误处理统一 toast
  - 文档更新(prd.md / ARCHITECTURE.md / HANDOFF.md + LLM 透明度段)
  - 手动 smoke test 走完所有状态机路径

(两 PR 都是同一个 task,但代码改动有自然边界,review 友好)

## Technical Notes

### 受影响文件清单
**后端:**
- `app/src-tauri/src/lib.rs`(`create_session` 简化 + 新增 3 个 Tauri command + 3 destructive 路径 in-flight cancel + system event 注入 + `delete_session` 调整 + `invoke_handler` 注册)
- `app/src-tauri/src/db.rs`(加 `worktree_state` + `last_worktree_path` 列、迁移、调整 8 个 tests、新增 helpers、新增 system event insert helper)
- `app/src-tauri/src/git/worktree.rs`(加 `check_clean` 工具函数;`create` / `destroy` 复用)
- `app/src-tauri/src/git/mod.rs`(加 `check_clean` 导出)
- `app/src-tauri/src/git/diff.rs`(无需改)
- `app/src-tauri/src/tools/*.rs`(7 工具返回 JSON 加 cwd 字段;改 ToolResult 类型)
- `app/src-tauri/src/tools/mod.rs`(ToolResult 类型变更 + 构造 helper)

**前端:**
- `app/src/stores/chat.ts`(actions: attachWorktree / detachWorktree / deleteWorktree + 缓存失效;expose isStreaming computed;system event 不需要专门 action,后端 inject 后 LLM 自动看到)
- `app/src/components/chat/ChatPanel.vue`(头部 chip 重构,整合 diff chip + 下拉 + isStreaming disabled)
- `app/src/components/chat/DeleteWorktreeConfirm.vue`(新 modal 组件,简单 confirm)
- `app/src/components/ProjectTabs.vue`(非 git 项目 tooltip 措辞调整,移除"步骤 4 worktree 不生效"字样)
- `app/src/components/chat/EmptyProjectState.vue`(非 git 项目空状态文案调整)
- `app/src/components/chat/ToolCallCard.vue`(edit_file 的 diff popover 条件适配:无 worktree 时不显示)

**测试:**
- 新增 `git::worktree::tests`(attach/detach/delete 单元测试 + check_clean 测试)
- 新增 `db::tests`(worktree_state 迁移、CRUD、system event insert helper)
- 新增 `tools::tests`(7 工具返回含 cwd 字段)
- 调整 `lib.rs::commands` 的 8 个 db::tests 调用点

**文档:**
- `docs/IMPLEMENTATION.md`(step 4 段落标记"auto-create → opt-in")
- `docs/ARCHITECTURE.md §3`(加手动 attach/detach/delete 段 + LLM 透明度机制)
- `docs/HANDOFF.md`(更新 on-session-create 步骤)

### In-flight cancel 实现细节
```rust
// lib.rs 内部 helper
async fn cancel_inflight_if_any(
    cancellations: &Arc<Mutex<HashMap<String, CancellationToken>>>,
    session_id: &str,
) {
    // session_id → request_id 需要 store 维护 session→active_request 映射
    // 简化:本 task 不持久化,只在内存里;启动后第一次 chat 时记录
    // 替代方案:cancellations map key 改成 session_id 优先(request_id 作 suffix)
    // 进一步:在前端调 destructive 时主动传 cancel_token 进来
}
```

需要在 lib.rs 启动时维护 `session_active_request: HashMap<session_id, request_id>`,每次 chat 启动填入、退出时移除。destructive 路径用这个 map 找 request_id 调 cancel。

或者更简单:让前端 destructive 时自己调 `cancel_chat(request_id)`,然后才调 `delete_session` / `detach_worktree` / `delete_worktree`。后端只做幂等保护(找不到 token 不报错)。

推荐:前端先 cancel(若 isStreaming),然后 destructive 路径无脑跑。后端在 destructive 路径加"再 cancel 一次"的兜底(防御编程)。

### System event 注入设计
```rust
// 后端 attach 成功后
db::insert_system_event(
    &pool,
    session_id,
    "worktree attached: <path> on branch session/<id>",
).await?;
```

`db::insert_system_event` helper:
- 在 `messages` 表 insert 一条 role='user' / content=`<event_text>` 的 record
- `seq` 取当前 max+1
- `metadata` 存 `{ kind: 'worktree_event', event: 'attached' }` 之类结构化标记
- 后续 LLM chat 加载 messages 时,普通 messages 跟 system events 一起进 message 数组
- LLM 在 system prompt 里有一条规则"看到 [worktree event] 前缀时,理解这是 worktree 状态变更"

需要给 LLM 的 system prompt 加一段说明,告诉 LLM 怎么理解 system event。

### 7 工具 cwd 字段实现
```rust
// tools/mod.rs 的 ToolResult
pub struct ToolResult {
    pub ok: bool,
    pub output: serde_json::Value,  // 工具特定输出
    pub cwd: String,                 // NEW: 写工具执行时的 ctx.worktree_path
}
```

每个工具在返回前:
```rust
ToolResult {
    ok: true,
    output: json!({...}),
    cwd: ctx.worktree_path.to_string_lossy().to_string(),
}
```

LLM 看到 tool_result 里有 `cwd`,理解"我这次读的是这个路径下的"。

### 关键参考
- worktree.rs 头注释(为什么用 git2-rs、为什么 libgit2 没有 worktree_remove)
- ARCH §3 的命名约定(branch `session/<id>`、路径 `~/.local/share/everlasting/worktrees/<project_uuid>/<session_uuid>`)
- 58d7852 commit message 的"name vs branch 解耦"教训(attach 时仍要遵守)
- PR1 的 worktree 创建流程(create_worktree 函数已分离,直接复用)
- PR2 的 worktree_path NULL fallback(7 工具已支持,无需改)
- PR5 的 cancel 机制(`cancel_chat(request_id)` 已有,直接调)
- `.trellis/spec/backend/project-cwd-boundary.md` 的 canonical path 约束(继续生效)
