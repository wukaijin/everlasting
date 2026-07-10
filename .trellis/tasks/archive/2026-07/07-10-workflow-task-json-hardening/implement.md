# Implement — workflow-task-json-hardening

执行顺序按 design.md 的 R 编号。每步独立 commit,可单独 revert。PKG_CONFIG_PATH 见 CLAUDE.md WSL 坑(下文命令已带)。

## Step 1 — R1 read_task lenient(止血,最先做)

- [ ] `task.rs`:给 `TaskStatus` 加自定义 `Deserialize`(delegate `from_str_opt`,参照 `def.rs::Coordination`);从 derive 列表去掉 `Deserialize`(保留 `Serialize` + Debug/Clone/Copy/PartialEq/Eq)。
- [ ] `task.rs`:`TaskJson.created_at` / `updated_at` 加 `#[serde(default)]`;`TaskItem.content` 加 `#[serde(default)]`。
- [ ] `task.rs` tests:加 `read_task_lenient_missing_created_at` / `_missing_updated_at` / `_item_status_in_progress` / `_item_status_pending` / `_item_missing_content` 五个 case,断言 `read_task` 返回 `Ok` 且字段被 default。
- [ ] 验证:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib workflow::task`
- [ ] **review gate**:确认 `write_task`/`create_task_init` 产出的 task.json 仍严格合法(写路径不变,只是读侧放宽)。

## Step 2 — R3+R4 读侧统一即时 resolve(合并)

- [ ] `inject.rs`:`resolve_current_task` 改 `pub`(签名不变,`async fn ... -> Option<TaskJson>`)。
- [ ] `inject.rs`:`append_workflow_breadcrumb` 签名改为接收「即时解析的 current_task」而非整个 ctx(新签名建议 `append_workflow_breadcrumb(&mut Vec<ChatMessage>, &WorkflowDef, &Option<TaskJson>)`);`build_breadcrumb_block` 同步改。
- [ ] `chat_loop.rs:3426`(transition 拦截):用 `resolve_current_task(&current_ctx.worktree_path).await` 取 `(current_state, current_slug)`,不读 `workflow_ctx.current_task`。
- [ ] `chat_loop.rs`(breadcrumb 注入点,搜 `append_workflow_breadcrumb` 调用):同样先 `resolve_current_task` 取 fresh task,再传给新签名。
- [ ] 更新 `inject.rs` tests 中 `append_workflow_breadcrumb` 相关 case 的调用方式(签名变了)。
- [ ] 验证:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`(确认 workflow/chat_loop 不回归)。
- [ ] **review gate**:确认没引入每 turn 多次 read_dir(transition 一次 + breadcrumb 一次 —— 可优化为每 turn resolve 一次复用,但 v1 接受两次;task 目录通常 ≤1 文件,成本可忽略)。

## Step 3 — R2 create_task LLM tool

- [ ] 新建 `tools/create_task.rs`:`definition()`(name/slug/title/parent schema)+ `execute(input, ctx) -> (String, bool)`(从 `ctx.worktree_path` 取 project_path → `create_task_init`)。
- [ ] `tools/mod.rs`:`pub mod create_task;` + `builtin_tools()` 注册 + `execute_tool_inner` match arm(`"create_task" => create_task::execute(...)`)。
- [ ] 确认 create_task **不进** `is_parallel_eligible`(建档串行,落进 serial path)。
- [ ] **新增 `filter_tools_for_workflow`**(`tools/mod.rs`):白名单 `{create_task, request_task_state_transition}`,`workflow_enabled=false` 时剥掉。在 `chat_loop.rs:1540` 与 `filter_tools_for_mode` 链式调用。
- [ ] worker 侧 `filter_tools_for_subagent` 的 `STRUCTURALLY_DISABLED` 集补 `create_task`。
- [ ] tests:`filter_tools_for_workflow` 在 workflow / 非 workflow / worker 三种 session 下对 create_task + request_task_state_transition 的可见性断言。
- [ ] 验证:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib tools`
- [ ] 内联 tests:成功建档 / `AlreadyExists` / `InvalidSlug` / project 路径无 `.everlasting`(create_task_init 自建目录)。
- [ ] 验证:`cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib create_task`
- [ ] **review gate**:`execute_tool_inner` 分发不破坏现有 tool 顺序;description 定位「便捷建档助手(省 token + 模板全),write_file 也允许、read_task 兜底」,**不写**"唯一合规路径"。

## Step 4 — R5 bootstrap hint 文案(软推荐)

- [ ] `inject.rs:620-625`:bootstrap branch 文案改为软推荐 `create_task` tool(**不禁止 write_file**,见 design §6 修正)。
- [ ] `inject.rs` tests:更新 `append_workflow_breadcrumb_bootstrap_branch_when_no_current_task` 等断言(去 "IPC"、含 "create_task tool",**不再断言"禁 write_file"**)。
- [ ] create_task tool `definition().description`:定位「便捷建档助手」,说明 write_file 也允许、read_task lenient 兜底。

## Step 5 — 端到端验证

- [ ] `cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib`
- [ ] `cd app && pnpm test`
- [ ] (可选,手动 e2e)清掉 `precaution-frontend/.everlasting/tasks/improve-readme/`,在 workflow session 里让 agent 用新 `create_task` tool 重建,走完 planning→implement→check→done,全程无 `no active task` / `unknown variant`。

## 验证命令汇总

```bash
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib
cd app/src-tauri && PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check
cd app && pnpm test
cd app && pnpm vue-tsc --noEmit   # 若改前端(本任务基本不碰)
```

## Rollback points

- 每个 Step 独立 commit,可单独 revert。
- **R1(Step 1)是最关键的止血** —— 若时间紧,只做 Step 1 也能让 workflow 不再崩(LLM 仍会手写,但不再致命)。Step 2/3/4 是体验/合规性优化。
- R3+R4(Step 2)若借用/性能出问题,可回退到「读冻结 ctx」并接受「跨消息才刷新」的临时行为(即第一次解封后的状态)。
