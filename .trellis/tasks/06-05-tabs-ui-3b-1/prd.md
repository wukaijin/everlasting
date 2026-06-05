# 项目基础结构 + 顶部 Tabs UI（步骤 3b-1）

## Goal

把 IMPLEMENTATION §2.4 原 "步骤 3b" 拆成 3b-1（本任务）和 3b-2（暂缓）。3b-1 落地：projects 数据模型 + 顶部 Tab UI + cwd 漂移机制 + Auto-default 兜底 migration + `pick_project_dir` 跨平台 fallback。为步骤 4（Git worktree）解锁 `<project_uuid>` 字段。

## 权威设计稿

**`docs/PROPOSAL-project-binding-and-top-tabs.md`**（11 节 + 评审消化表 + PR 拆分）

辅助 spec：`.trellis/spec/backend/project-cwd-boundary.md`（boundary 校验 edge case）

## 状态

- ✅ grill 完成（Q1-Q9）
- ✅ 评审消化完成（深 seek + GLM 两份）
- ✅ 拍板（保持 UUID / Auto-default 兜底 / turn 结束写 cwd）
- ✅ PROPOSAL 更新到 V2（含评审消化表 §11 + PR 拆分 §9）
- ✅ Spec 写好（project-cwd-boundary）

## PR 拆分

### PR1 — 数据模型 + 后端（无 UI 变更）
- `db.rs` migration + projects CRUD + sessions 改造
- `projects/` 新模块（types / store / detector / boundary）
- `tools/` 全部改造（mod.rs 签名 + 3 个 tool 实现）
- `lib.rs` `chat` + 4 个现有 commands 改造
- ARCHITECTURE §3 worktree 路径术语 `project_hash` → `project_uuid`
- Unit tests
- **PR1 完功能不变**,后端可独立 review + 测

### PR2 — 前端 Tab 栏 + 空状态 + store
- `stores/projects.ts` 新增
- `stores/chat.ts` 改造
- `ChatWindow.vue` Tab 栏 + 空状态 + "最近隐藏项目"列表
- `pick_project_dir` + 手动输入 fallback
- 端到端测试

## 验收标准(AC)

### AC-PR1(后端,无 UI 改动)
- [ ] `cargo test` 全过
- [ ] `db.rs::run_migrations` 在新版本运行:Auto-default 项目 `__default__` 被插入,旧 sessions 挂上
- [ ] `PRAGMA foreign_keys = ON` 在 db pool 初始化时设
- [ ] `tools::execute_tool(name, input, &ctx)` 签名生效;`ctx: ToolContext { project_root, cwd }`
- [ ] `boundary::assert_within_root` 7 个 edge case 单测全过
- [ ] `create_session(project_id, initial_cwd, model)` 签名生效
- [ ] `list_sessions(project_id)` 返回只含该 project 的 sessions
- [ ] `pick_project_dir(fallback: bool)` 实现
- [ ] shell tool 接 `working_directory` 参数,**过 boundary**
- [ ] read_file / write_file 相对路径按 `ctx.cwd` 解析,绝对路径过 boundary
- [ ] turn 结束**一次性**写 `session.current_cwd`,不是每次 shell 写
- [ ] ARCHITECTURE.md §3 `project_hash` 改 `project_uuid`

### AC-PR2(前端,Tab 栏 + 空状态)
- [ ] 顶部 Tab 栏渲染,selected Tab 底部 2px 蓝线
- [ ] 无项目时整个 session 侧栏不渲染;中央居中显示"添加项目"按钮
- [ ] "+ 添加项目" 弹 Tauri dialog;失败/manual 时显示手动输入 path
- [ ] 选目录后探测 `is_git_repo`,存表,自动切到新 Tab
- [ ] 同一目录加两次 → focus 已有 Tab,不重复添加
- [ ] 关闭 Tab(×)→ `hide_project`,数据保留,Tab 消失
- [ ] 关闭后切回空状态 → "最近隐藏项目"列表显示隐藏的项,每行带"重新打开"按钮
- [ ] 非 git 项目 Tab 标 `⚠️` 12px 图标,hover tooltip "未启用 git 隔离 (步骤 4 worktree 不生效)"
- [ ] Legacy 项目 Tab 标 `📦`,hover tooltip "旧数据,自动归入"
- [ ] 切 Tab → 左侧 session 列表换为 `WHERE project_id = <new>` 结果
- [ ] 切 Tab 时若有 active session,旧 Tab 上保留 `●` 红点;切回时恢复
- [ ] `chat.ts:send()` 在 `!currentProjectId` 时 toast 提示,不报错
- [ ] 启动流程:localStorage 记 last active project,启动时优先恢复
- [ ] pnpm build + vue-tsc --noEmit 通过

### AC-端到端(manual smoke)
- [ ] 加 `/repo` 项目 → 默认 name=`repo` → Tab 显示 → 探测为 git → 标为正常样式
- [ ] 加 `/tmp/hack` → 探测为非 git → Tab 标 `⚠️`
- [ ] 在 `/repo` 项目下建 session → LLM 调 `shell({"command": "cd backend && ls"})` → tool_result 成功,session.current_cwd = `/repo/backend`(turn 结束写)
- [ ] LLM 调 `shell({"command": "cd /etc", "working_directory": "/etc"})` → **tool_result is_error**: "path '/etc' is outside project root '/repo'"
- [ ] 在 `/repo` 项目下建 session 1,在 `/tmp/hack` 项目下建 session 2 → 两个 session 列表独立
- [ ] 关闭 `/tmp/hack` Tab → 切回空状态 → "最近隐藏项目"显示 hack → 点"重新打开" → 回到 Tab,带原 sessions
- [ ] 删 db 文件,重启 app → Auto-default 项目 `Legacy / 未分类` 出现,Tab 标 `📦`

## Definition of Done

- [ ] PR1 + PR2 都 merge 到 main
- [ ] `cargo test` + `pnpm build` + `pnpm tauri build` 全过
- [ ] docs/IMPLEMENTATION.md §2.4 拆 3b-1 / 3b-2 标进度
- [ ] docs/ARCHITECTURE.md §3 改名 + 加 §6 项目模型小节
- [ ] docs/CLAUDE.md "当前状态" 段更新

## Out of Scope

- ❌ 项目"真删除" UI
- ❌ "管理项目"完整面板
- ❌ Tab 拖动排序
- ❌ 项目级 memory (CLAUDE.md per-project)
- ❌ trellis ↔ projects 关联
- ❌ rig-core 迁移(3b-2)
- ❌ 完整三栏 UI(3b-2)
- ❌ update_project_path UI 暴露

## 关键风险点(实施时盯紧)

- **Auto-default 兜底 migration** 在老 db 上的兼容性(PRAGMA 设不设 / sqlite 版本)
- **boundary::assert_within_root** 7 edge case 单元测试覆盖率
- **`PRAGMA foreign_keys = ON`** 连接池初始化时一次性设
- **turn 结束一次性写 cwd** — agent loop 状态机要正确累积 `last_cwd`
- **`pick_project_dir` fallback** 在 WSLg 下的 UX(评审 GLM §4.2 提)
- **Q2 UUID vs Path-as-PK** — 保持 UUID,但评审认为 over-engineer;如果实施时发现 UUID 拖累,可改 path-as-PK 优化(成本小)
