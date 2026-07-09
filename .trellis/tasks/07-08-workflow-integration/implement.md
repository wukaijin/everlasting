# Implement — Workflow 集成:工作流引擎 + plugin(dev/review)

> 配套 `prd.md` / `design.md` / `docs/WORKFLOW-INTEGRATION.md`(§9.5 分步指导)。执行顺序按依赖关系;每步附验证命令。WSL 下 cargo 命令需带 `PKG_CONFIG_PATH`(见 CLAUDE.md HACKING-wsl)。
>
> 全程遵守:**每个小步独立提交**;碰现有稳定代码的步(2.4/2.6)做完立即跑全量测试。

## 实施状态(2026-07-08)

| Phase | 状态 | 提交 |
|---|---|---|
| Phase 0 — engine 骨架 (Step 0.1-0.5) | ✅ 完成 | 2727ef5 / 8da332c / e28f420 / e0c5657 / 788fbbb + c9f926d (clippy fix) |
| Phase 1 — skill 规范包 + plugin skill loader (Step 1.1-1.5) | ✅ 完成 | b7e8b74 / d3b8494 / 0decc2c / c2698d4 / c3bb28f |
| Phase 2 — plugin 外置 + sub-agent 角色 + 门控 + 注入 (Step 2.1-2.6) | ✅ 6/6 | 38391c1 (2.1) / b999803 (2.2) / e73f58b (2.3) / fa09858 (2.4) / 9e513bc (2.5) / 64a9972 (2.6) |
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

- [x] `WorkflowDef` 加 `#[derive(Deserialize)]`
- [x] `load_workflow(name, project_path) -> WorkflowDef`:serde_json 读 `.everlasting/workflow/<name>/workflow.json`
- [x] validate(M6):serde 失败 → warn + 回退 default;解析成功 → 校验 states 非空 / initial∈states / transitions from/to∈states / roles_by_state keys⊆states;任一失败回退 default
- [x] delegation_templates / breadcrumb 某键缺失 → 空字符串(warn),不阻塞
- [x] `default_workflow()` 降为"项目无 workflow.json 或 validate 失败时的兜底"
- [x] `.everlasting/workflow/dev/workflow.json` 落地(镜像 default_workflow 形状)
- **验证**:`$CD && env $PKG cargo test --lib workflow 2>&1 | tail -20` ✅ 45 pass(+10 new);合法 JSON 改 states → breadcrumb 跟着变(`load_workflow_valid_json_overrides_default`);malformed JSON → fallback default + warn(`load_workflow_malformed_json_falls_back_with_warn`)

#### Step 2.2 — UI plugin 切换

- [x] 顶栏 plugin 名点击 → 切换(此时只有 dev 但机制就位)
- [x] 切 plugin = 改会话内 in-memory plugin 选择 + 重新注入对应 breadcrumb
  - DB 层:新增 `sessions.plugin_name` 列(TEXT NOT NULL DEFAULT 'dev',additive migration)
  - DB 函数:`set_session_plugin_name(session_id, name)`
  - IPC:`set_session_plugin_name`(空字符串拒绝)+ `list_workflow_plugins`(discovery)
  - engine:`build_workflow_ctx` 改调 `load_workflow(plugin_name, project_path)` 替代硬编码 `default_workflow()`
  - 前端:`PluginSelect.vue`(workflow ON 时显示当前 plugin 名,点击弹 popover 列出 `list_workflow_plugins` 结果);`chat.ts` 加 `requestSetPluginName` + `listWorkflowPlugins` action
- **验证**:✅ 手动——切 plugin(`dev` ↔ 假设的 `review`)→ DB 列翻转 → 下一轮 `build_workflow_ctx` 调 `load_workflow` → breadcrumb 跟着 plugin 走。`cargo test --lib` 1412 pass(+5 new);`pnpm test` 794 pass(零回归);`vue-tsc --noEmit` clean;clippy 被改文件 0 新警告

### 批 B:sub-agent 角色 + 门控 + 注入(4 步,2.4/2.5/2.6 可并行)

#### Step 2.3 — plugin agents/ 落地 + loader 加层

- [x] 创建 `.everlasting/workflow/dev/agents/researcher.md` / `implementer.md` / `checker.md`(frontmatter + body)
- [x] `agent/subagent/loader.rs`:`SubagentSource` 加 `Plugin` 变体 + `as_str`
- [x] `plugin_agents_dir(workflow_name, project_path)` → `<project>/.everlasting/workflow/<wf>/agents/`
- [x] `SubagentCache` 加 plugin cache 层(`RwLock<HashMap<(project, wf), CachedScan>>`)
- [x] `list_with_workflow(project, workflow_name)` + `lookup_with_workflow(project, wf, name)` — workflow-aware variant,优先级 plugin > project > user > builtin
- [x] `merge_with_inheritance` 不改(source-agnostic)
- [x] legacy `list()` / `lookup()` 不动;`locate_agent_file` Plugin 分支返回 Err(插件只读)
- [x] `commands/subagents.rs` 的 `set_subagent_model` Plugin 分支返回 InvalidRequest
- **验证**:✅ `cargo test --lib subagent::loader` 63 pass(+6 new,plugin 层 / 空名回退 / lookup);`cargo test --lib` 全量 1418 pass,零回归

#### Step 2.4 — 门控下沉 run_subagent(⚠️ 风险最高)

- [x] `run_subagent` 签名加 `workflow_ctx: Option<&WorkflowCtx>`(24→25 参) — 比 implement.md 原始 `current_state: Option<&WorkflowState>` 更通用,Step 2.5 复用同一 param 读 `workflow_def.delegation_templates`
- [x] 三处调用点补传(chat_loop.rs:1039 forced / 3016 L3b 并发 / 3365 串行)— L3b 并发 closure 内显式 `clone()`,避免与 line 1532 breadcrumb 注入处的 borrow 冲突
- [x] 抽 `check_workflow_role_gate(workflow_ctx, role, input) -> Option<String>` 纯函数(便于单测,无 IO/无 LLM),`run_subagent` 入口在 task 校验后立即调
- [x] 允许 → 继续原 dispatch(workflow_ctx 透传到 Step 2.5 模板注入)
- [x] 不允许 → 返 tool_error 含 attempted role / current state / allowed roles / 建议路径(transition or force:true);breadcrumb 在 messages[0] 可见
- [x] `input.force: true` 一次性越权 + warn!(无持久化,audit log 可见)
- [x] `workflow_ctx = None` OR `current_task = None` 短路 → legacy / bootstrap 路径零影响
- [x] **全量 cargo test 零回归**(1425 pass,+7 new gate tests)
- **验证**:✅ 集成测试 — `gate_denies_role_not_allowed_in_current_state` / `gate_allows_role_in_current_state` / `gate_force_bypass_overrides_denial` / `gate_enforcement_is_state_driven` / `gate_short_circuits_when_no_workflow_ctx` / `gate_short_circuits_when_no_current_task` / `gate_done_state_has_no_allowed_roles`;clippy 被改文件 0 新警告
- **Q3+S3 协商档** 留给 Phase 3 统一处理(`resolve_task_state_transition` IPC 落地时一并接入);Step 2.4 走同步 denial 路径就足够 engine 工作

#### Step 2.5 — delegation 模板注入(可与 2.4/2.6 并行)

- [x] `run_subagent` 门控通过后:engine 取 `delegation_template_for(def, role)` → 填占位符 `{title}`/`{summary}`/`{state}`/`{relevant_specs}`
  - `compute_delegation_template(workflow_ctx, project_path, role)` 纯函数,None role 模板时返 None
  - 占位符缺失时保持原样(LLM 可识别为 plugin-author bug)
- [x] `{relevant_specs}`:递归扫 `<project>/.everlasting/spec/` 找 .md 文件(深度优先,跳 symlink,non-.md 忽略);缺失/空 → `(auto-detect via wf-before-dev)`
  - FTS5 over `task.summary` 是 Phase 3 精化(Step 2.5 用平面 walk)
- [x] 填好的模板 append 到 worker messages[0] block(复用 `append_workflow_breadcrumb` 同款 S-B 守;`cache_control: None`)
  - `append_delegation_template(turn_messages, body)` 与 breadcrumb 同款:user-role Blocks 守 + 缺失模板 no-op
- [x] **仅 dispatch turn 追加**(M-E):parent chat_loop 的 messages[0] 不动;worker 看到模板作为初始 context 的一部分
- **验证**:✅ `cargo test --lib workflow` 33 pass(+7 new,占位符替换 / spec 递归扫描 / unknown role None / append push + skip 路径);`cargo test --lib` 全量 1432 pass,零回归

#### Step 2.6 — checklist 同步(⚠️ 风险最高,可与 2.4/2.5 并行)

- [x] `tools/update_checklist.rs:execute` 加分支:workflow session → 改写 task.json.items;非 workflow → 保持原 loop-local Vec 行为
  - 新签名 `execute(input, handle, ctx: &ToolContext)`(tools/mod.rs 调点同步更新)
  - `maybe_persist_to_task_json` helper:workflow_name Some → 读 current task.json → 持久化;None → no-op
  - legacy 完全不动(in-memory handle 始终被更新)
- [x] B12 `coerce_at_most_one_in_progress` 逻辑保留(写目标从 Vec 换成 task.json.items)
- [x] `ChecklistItem` struct 加 `id: String`(#[serde(default)])+ `tdd: Option<bool>` 字段(对应 TaskItem schema);LLM JSON schema 同步加
- [x] ChecklistStatus → TaskStatus 映射(Pending→Planning / InProgress→Implement / Done→Done)— Phase 2 简化,Phase 3 可扩
- [x] `derive_item_id` 用 FNV-1a content hash 处理 LLM 省略 id 的 legacy case
- [x] `pick_first_unfinished_task` lex-by-slug,skip Done,corrupt skip
- [x] **全量 cargo test 零回归**(1437 pass,+5 new)— B12 原测试全过
- **验证**:✅ 集成测试 — `execute_workflow_persists_items_to_task_json`(全 round-trip,含 tdd)/ `execute_non_workflow_does_not_touch_task_json`(legacy 路径不变)/ `execute_workflow_derives_id_from_content_when_missing` / `checklist_item_parses_id_and_tdd_from_json` / `checklist_item_omitted_id_and_tdd_default_cleanly`;cross-session 续 task → 读 task.json.items 拿进度 OK;worker dispatch 读 task.json.items 拿进度 OK

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
