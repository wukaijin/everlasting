# Implement — Workflow 集成:工作流引擎 + plugin(dev/review)

> 配套 `prd.md` / `design.md` / `docs/WORKFLOW-INTEGRATION.md`(§9.5 分步指导)。执行顺序按依赖关系;每步附验证命令。WSL 下 cargo 命令需带 `PKG_CONFIG_PATH`(见 CLAUDE.md HACKING-wsl)。
>
> 全程遵守:**每个小步独立提交**;碰现有稳定代码的步(2.4/2.6)做完立即跑全量测试。

## 实施状态(2026-07-08)

| Phase | 状态 | 提交 |
|---|---|---|
| Phase 0 — engine 骨架 (Step 0.1-0.5) | ✅ 完成 | 2727ef5 / 8da332c / e28f420 / e0c5657 / 788fbbb + c9f926d (clippy fix) |
| Phase 1 — skill 规范包 + plugin skill loader (Step 1.1-1.5) | ✅ 完成 | b7e8b74 / d3b8494 / 0decc2c / c2698d4 / c3bb28f |
| Phase 2 — plugin 外置 + sub-agent 角色 + 门控 + 注入 (Step 2.1-2.6) | ⏳ 待开始 | — |
| Phase 3 — hook + 沉淀闭环 (Step 3.1-3.3) | ⏳ 待开始 | — |

## 前置常量

```bash
PKG="PKG_CONFIG_PATH=/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"
CD="cd /usr/local/code/github/everlasting/app/src-tauri"
APP="cd /usr/local/code/github/everlasting/app"
```

## Phase 0 — engine 骨架(5 步)

### Step 0.1 — sessions 表加 workflow_enabled 列

- [ ] `db/migrations.rs`:`run_migrations` 内加 `add_session_column_if_missing(pool, "workflow_enabled", "INTEGER NOT NULL DEFAULT 0").await?`(跟 `mode` @ 412 同模式,放在其后)
- [ ] `db/types.rs:246` `SessionRow` 加 `pub workflow_enabled: bool` 字段(放在 `mode` 后)
- [ ] `db/sessions.rs`:所有 SQL 站点补 `workflow_enabled`——`create_session`(@ 39)INSERT + `list_sessions`(@ 90)SELECT + `load_session` + `update_*`
- **验证**:`$CD && env $PKG cargo test --lib db:: 2>&1 | tail -20`(migration + SessionRow round-trip 通过)

### Step 0.2 — 顶栏 workflow toggle UI

- [ ] 前端 Pinia store 加 `workflowEnabled` + 读写 IPC
- [ ] `commands/` 加 `set_session_workflow_enabled(session_id, enabled)` Tauri command
- [ ] 顶栏加 toggle(B7 mode 切换同级组件),绑 store
- [ ] 持久化:关 session 重开状态保持(读 SessionRow.workflow_enabled)
- **验证**:手动点 toggle → DB 列翻转(`sqlite3` 查);关重开状态保持

### Step 0.3 — WorkflowDef struct + 4 访问函数 + default_workflow()

- [ ] 新建 `agent/workflow/mod.rs`(或 `agent/workflow/def.rs`):定义 `Transition` / `WorkflowDef` / `Coordination` enum + 4 访问函数
- [ ] `default_workflow()` 返回 dev 四态硬编码常量(states: planning/implement/check/done;transitions 三条带 requires_user_confirm;roles_by_state;breadcrumb;delegation_templates;coordination=Pipeline;gather_strategy={})
- [ ] **纯 Rust,无 UI 无 IO**;Phase 2 才加 `load_workflow` serde
- **验证**:`$CD && env $PKG cargo test --lib workflow 2>&1 | tail -20`(`breadcrumb_for(planning)` 返回期望文本;`can_transition(planning, implement)=true`;`delegation_template_for("researcher")` 返回 Some)

### Step 0.4 — task.json 读写 + create_task command

- [ ] 新建 `agent/workflow/task.rs`:`TaskJson` struct(id/title/slug/status/created_at/updated_at/parent/summary/items)+ serde + read/write 函数
- [ ] `commands/` 加 `create_task(title, slug)` Tauri command:种 `.everlasting/tasks/<slug>/task.json`(空模板,status=planning,items=[])+ `prd.md` 大纲
- [ ] 目录约定:`.everlasting/tasks/<slug>/`
- **验证**:手动调 create_task → 目录 + 文件生成,JSON 合法(serde round-trip);读回来字段对

### Step 0.5 — state breadcrumb 注入 + 起续 task 触发点

- [ ] `agent/chat_loop.rs`:`run_chat_loop` 签名加 `workflow_ctx: Option<WorkflowCtx>`(29→30 参,现有调用点传 None)
- [ ] per-turn 注入:workflow session 且有 current task 时,把 task.json 元数据 + state breadcrumb append 到 per-turn request clone 的 messages[0](复用 `inject_recall_into_turn` 的 append 逻辑,抽共享 helper 或直接调)
- [ ] 起续 task 触发点:workflow session 收到首条 user message → agent `list_dir .everlasting/tasks/` 找 status≠done → 有则读 progress.md 问用户续上;无则判断开发意图起 task(写 task.json)
- [ ] **前置约束(S-B)**:engine 校验 messages[0] 是 user-role Blocks message;否则 warn + 降级非 workflow(不走 fallback prepend)
- **验证**:集成测试——workflow session 发开发意图消息 → task.json 生成 → breadcrumb 随 state 变化出现在 messages[0];cargo test + 手动跑一遍

**Phase 0 完成标志**:开 workflow → agent 起 task → breadcrumb 可见 → state 转移(暂用 ask_user_question,Phase 3 换专用 IPC)。

## Phase 1 — skill 规范包 + plugin skill loader(4 步)

### Step 1.1 — plugin skill loader

- [ ] `skill/loader.rs`:`SkillSource` enum 加 `Plugin` 变体(@ 46)
- [ ] 加 `plugin_skills_dir(workflow_name, project_path) -> PathBuf` → `<project>/.everlasting/workflow/<name>/skills/`
- [ ] `list_skill_infos`(@ 482) + `find_skill`(@ 503) 插入 plugin 层(优先级:plugin > project > user)
- [ ] plugin 层只在 workflow session 查;非 workflow session 不扫 plugin 目录
- **验证**:`$CD && env $PKG cargo test --lib skill:: 2>&1 | tail -20`(plugin skills 优先于全局;无 plugin 时 fallback 全局)

### Step 1.2 — wf-overview skill body

- [ ] 创建 `.everlasting/workflow/dev/skills/wf-overview/SKILL.md`(body 见设计文档 §A.4 完整版)
- [ ] 这是 agent 进 workflow 的入口 skill,建立全局意识
- **验证**:手动——workflow session agent `use_skill wf-overview` → body 返回

### Step 1.3 — wf-brainstorm / wf-before-dev / wf-check / wf-update-spec

- [ ] 创建 4 个 skill 目录 + SKILL.md(body 见 §A.5 outline 填肉,借鉴 Trellis 同名 skill 去 Python hook 依赖)
- [ ] breadcrumb 文本提示对应 state 加载哪个 skill
- **验证**:各 skill 被对应 state 的 agent 加载

### Step 1.4 — artifact 查阅机制

- [ ] task.json 元数据(title/slug/status/summary)+ state breadcrumb append messages[0](per-turn)
- [ ] prd/design/progress 全文只给 path,agent read_file 自取
- **验证**:集成测试——messages[0] 含 task meta;agent read_file prd 成功

### Step 1.5 — ToolContext 加 workflow_name + use_skill 走 plugin 层(桥接步)

- [x] `tools/mod.rs`:`ToolContext` 加 `pub workflow_name: Option<String>` + 注释
- [x] `skill/loader.rs`:移除 `find_skill_with_workflow` 上的 `#[allow(dead_code)]`,更新 doc 注释指向 use_skill 这个新 consumer
- [x] `tools/use_skill.rs::execute`:改调 `find_skill_with_workflow(skill_cache, name, Some(&project_path), ctx.workflow_name.as_deref())`
- [x] `agent/chat_loop.rs`:`ToolContext` 构造处补 `workflow_name: workflow_ctx.as_ref().map(|c| c.workflow_def.name.clone())`
- [x] 13 个 `test_ctx` helper + 5 个 tests_subagent 桩 + remember/use_ui 站点补 `workflow_name: None`
- **为什么补这步**:Phase 1 留的 `find_skill_with_workflow` 在 workflow session 里还是查不到 wf-* skills(`ToolContext` 没传 plugin 名,use_skill 走的是非 workflow 路径);Phase 2 六步里没有一步接这个,留到 Phase 2 开工前补
- **验证**:`use_skill_resolves_plugin_layer_when_workflow_name_set` + `use_skill_empty_workflow_name_treated_as_none`(新增 2 个 wiring 测试);`cargo test --lib`:1397 pass(+2 new),零回归

**Phase 1 完成标志**:进 workflow agent 读 wf-overview;planning 用 wf-brainstorm;各 skill body 可读;task meta 注入可见。

## Phase 2 — plugin 外置 + sub-agent 角色 + 门控 + 注入(6 步,分两批)

### 批 A:plugin 外置 + UI 切换(2 步)

#### Step 2.1 — workflow.json 外置 + load_workflow + validate + fallback

- [ ] `WorkflowDef` 加 `#[derive(Deserialize)]`
- [ ] `load_workflow(name, project_path) -> WorkflowDef`:serde_json 读 `.everlasting/workflow/<name>/workflow.json`
- [ ] validate(M6):serde 失败 → warn + 回退 default;解析成功 → 校验 states 非空 / initial∈states / transitions from/to∈states / roles_by_state keys⊆states;任一失败回退 default
- [ ] delegation_templates / breadcrumb 某键缺失 → 空字符串(warn),不阻塞
- [ ] `default_workflow()` 降为"项目无 workflow.json 或 validate 失败时的兜底"
- **验证**:`$CD && env $PKG cargo test --lib workflow 2>&1 | tail -20`(改 workflow.json states → breadcrumb 跟着变;malformed JSON → fallback default + warn)

#### Step 2.2 — UI plugin 切换

- [ ] 顶栏 plugin 名点击 → 切换(此时只有 dev 但机制就位)
- [ ] 切 plugin = 改会话内 in-memory plugin 选择 + 重新注入对应 breadcrumb
- **验证**:手动切 plugin → breadcrumb 重新注入

### 批 B:sub-agent 角色 + 门控 + 注入(4 步,2.4/2.5/2.6 可并行)

#### Step 2.3 — plugin agents/ 落地 + loader 加层

- [ ] 创建 `.everlasting/workflow/dev/agents/researcher.md` / `implementer.md` / `checker.md`(frontmatter + body 见 §A.3)
- [ ] `agent/subagent/loader.rs`:`SubagentSource` 加 `Plugin` 变体(@ 84)
- [ ] `list()`(@ 623)加 plugin 层 push(优先级:plugin > project > user > builtin)
- [ ] `merge_with_inheritance`(@ 943)source-agnostic 不改
- [ ] workflow session 里解析 implementer 用 plugin 的;非 workflow 用全局/builtin
- **验证**:`$CD && env $PKG cargo test --lib subagent::loader 2>&1 | tail -20`

#### Step 2.4 — 门控下沉 run_subagent(⚠️ 风险最高)

- [ ] `agent/subagent/dispatch.rs:233` `run_subagent` 签名加 `current_state: Option<&WorkflowState>`(24→25 参)
- [ ] 三处调用点补传:chat_loop.rs:1000(forced-dispatch)/ 2937(L3b 并发)/ 3286(串行)传同一 task state(workflow session)或 None(非 workflow)
- [ ] `run_subagent` 入口加门控:查 `allowed_roles(def, current_state)` 含目标 role?
  - 允许 → 注入 delegation 模板(§6.6.1)→ 继续原 dispatch
  - 不允许 → engine 调 `ask_user_question` 协商(统一协商档,Q3+S3);用户允许放行(一次性越权),拒绝返 tool_error 提示回 breadcrumb
- [ ] **做完立即跑全量测试**
- **验证**:`$CD && env $PKG cargo test 2>&1 | tail -30`(零回归);集成测试——planning 派 implementer → 弹协商;并发 dispatch path 也拦截;允许 → 放行;拒绝 → breadcrumb

#### Step 2.5 — delegation 模板注入(可与 2.4/2.6 并行)

- [ ] `run_subagent` 门控通过后:engine 取 `delegation_template_for(def, role)` → 填占位符 `{title}`/`{summary}`/`{state}`/`{relevant_specs}`
- [ ] `{relevant_specs}`:engine 按 task.summary FTS5 过滤 `.everlasting/spec/` 索引返回候选路径(无匹配填 `(auto-detect via wf-before-dev)`)
- [ ] 填好的模板 append 到 per-turn request clone 的 messages[0] block(复用 inject_recall_into_turn append 逻辑,`cache_control: None`)
- [ ] **仅 dispatch turn 追加**(M-E),非 dispatch turn 不追加
- **验证**:集成测试——dispatch 时 messages[0] 含填好的 delegation 模板;worker 读到角色规范 + relevant_specs

#### Step 2.6 — checklist 同步(⚠️ 风险最高,可与 2.4/2.5 并行)

- [ ] `tools/update_checklist.rs:execute` 加分支:workflow session → 改写 task.json.items(非 loop-local Vec);非 workflow → 保持原 loop-local Vec 行为
- [ ] B12 `coerce_at_most_one_in_progress` 逻辑保留(写目标从 Vec 换成 task.json.items)
- [ ] `ChecklistItem` struct 加 `id: String` + `tdd: Option<bool>` 字段(对应 task.json.items)
- [ ] **做完立即跑全量测试**
- **验证**:集成测试——跨 session 续 task → items 进度持久;worker 读 task.json 拿进度;非 workflow session B12 原测试全过

**Phase 2 完成标志**:改 workflow.json 改流程;planning 只派 researcher;delegation 模板注入;checklist 跨 session 持久。

## Phase 3 — hook + 沉淀闭环(3 步)

### Step 3.1 — set_task_state + Rust 固定 hook + resolve_task_state_transition IPC

- [ ] 新建 `agent/workflow/state.rs`:`set_task_state(task, new_state)` 函数——write task.json.status + match 分支 hook
  - `(Check, Done)` → `trigger_spec_distillation(task)`(触发沉淀)
  - `(Planning, Implement)` → `preflight_implement_check(task)`(可选前置)
- [ ] `commands/question.rs` 新增 `resolve_task_state_transition` IPC(对标 `resolve_mode_change` @ 165 双 IPC pattern):
  - apply:`set_task_state(task, new_state)`(BEFORE resolve)
  - resolve:`store.resolve(Answered(true))` unblock agent
- [ ] agent 侧:ask_user_question 加 `purpose="task_state_transition"` 标记;前端按 purpose 路由到 `resolve_task_state_transition`(非 `resolve_tool_question`)
- **验证**:集成测试——Check→Done 触发 hook;planning→implement 触发 preflight;IPC apply-before-resolve 顺序对

### Step 3.2 — .everlasting/spec/ + wf-update-spec 落地

- [ ] 创建 `.everlasting/spec/` 目录约定(`<package>/<layer>/index.md` + guideline 文件,借鉴 `.trellis/spec/` 结构)
- [ ] `trigger_spec_distillation`:engine 调用 agent(在 wf-update-spec skill 指导下)把决策/坑/新 pattern 提炼写进 `.everlasting/spec/`
- [ ] wf-update-spec skill body 落地(§A.5)
- **验证**:手动——task done → spec 文件生成;下次 implement 读得到

### Step 3.3 — progress.md 交接叙述 + archive_task

- [ ] state 转移时 agent 更新 progress.md(交接叙述,a+b 方案的 b)
- [ ] `commands/` 加 `archive_task(slug, no_commit)` Tauri command:移动 `.everlasting/tasks/<slug>/` → `.everlasting/tasks/archive/<YYYY-MM>/<slug>/`;task.json.status=completed + completedAt;(默认)git add + commit
- [ ] 新 session 续 task:agent 读 progress.md + task.json.items 接上
- **验证**:手动——done → task 移到 archive/YYYY-MM/;新 session 读 progress 续上

**Phase 3 完成标志**:done 自动沉淀 spec + 归档;跨 session 续 task 完整闭环。

## 验证汇总

```bash
# 全量测试(每步做完跑,尤其 2.4/2.6)
$CD && env $PKG cargo test 2>&1 | tail -30
# 前端测试
$APP && pnpm test 2>&1 | tail -20
# lint + typecheck(Phase 2 后)
$CD && env $PKG cargo clippy 2>&1 | tail -20
$APP && pnpm typecheck 2>&1 | tail -20
```

## 风险最高步(盯紧)

- **Step 2.4**(门控下沉 run_subagent):改签名 + 三处调用点(1000/2937/3286)。做完立即全量 cargo test
- **Step 2.6**(checklist 同步):改 B12 写路径。做完立即全量 cargo test + 确认非 workflow 路径原测试全过
