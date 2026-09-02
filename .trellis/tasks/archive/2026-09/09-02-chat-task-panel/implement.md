# Implement — Chat 任务面板

按序执行;每步末尾的验证不过不进下一步。工作区约定:Rust 改动从仓库根 `cargo test -p everlasting --lib`(WSL 需 `PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig"`),前端 `cd app && pnpm test`。

## Step 1 后端:registry list + 事件发射

1. `background_shell/mod.rs`:新增 `BackgroundShellSummary`(camelCase,见 design §2.1)、`ShellEventPayload`、`EVENT_NAME`、emitter 类型别名;trait `BackgroundShellRegistry` 加 `async fn list_for_session(&self, session_id: &str) -> Vec<BackgroundShellSummary>`。
2. `background_shell/in_memory.rs`:
   - emitter 字段 + `set_event_emitter`(None→Some 才生效,重复忽略 + `tracing::warn!`);
   - `list_for_session` 实现(复用/抽取 `build_status_from_entry` 的 preview 逻辑,排序 running 优先 + started_at 倒序);
   - 三处发射(design §2.2):`start()` started / SpawnFailed-exited、`run_background_task` exited(**lock drop 后发射**)、`sweep_completed_shells` pruned 逐条。
3. 单测(design §4 Rust 四条;`wait_terminal` 等 helper 已在 tests mod 内可复用)。
4. 验证:`cargo test -p everlasting --lib background_shell`(一次调用带 filter,勿逐模块循环)。

## Step 2 后端:IPC 命令 + 双模式接线

1. `commands/background_shells.rs`:`list_background_shells_inner` / `kill_background_shell_inner` + 两个 `#[tauri::command]` 包装(照抄 `commands/subagent_runs.rs` 的 inner/cmd 分层)。
2. 注册三处:`lib.rs` invoke_handler、`commands/mod.rs` 命令名单、`daemon/routes/background_shells.rs` + `daemon/routes/mod.rs` nest。
3. emitter 接线(design §2.4):daemon `server.rs` 装配点 + `lib.rs` setup(Full 模式 AppState::load 后)。
4. 验证:`cargo test -p everlasting --lib` 全量绿(编译器兜底注册正确性;命令名名单漏加会导致 Thin 代理 404,人肉 diff 核对四处注册齐全)。

## Step 3 前端:backgroundShells store

1. `stores/backgroundShells.ts`:类型 + store(design §3.1);comparator 导出纯函数;`ensureStarted()` 幂等挂 listener。
2. `chatSessionActions.ts` 删除清理点并列 `clearSession`。
3. vitest `stores/backgroundShells.test.ts`(design §4;transport mock 遵循 `.trellis/spec/frontend/test-environment.md`)。
4. 验证:`cd app && pnpm test -- backgroundShells`。

## Step 4 前端:ActivityPanel 组件 + ChatPanel 替换

1. 新建 `components/chat/ActivityPanel.vue`(design §3.2;ChecklistCard 的 CSS/骨架整体搬运,清单 section 渲染原样迁移,保留 file-top 注释习惯与 08-24 btn-family 本地几何惯例)。
2. `ChatPanel.vue`:替换 import 与 :1082 模板节点,传 `:session-id`。
3. 删除 `components/chat/ChecklistCard.vue`(先 grep 确认无其他模板引用;`MessageItem.vue`/`MessageList.vue`/`Icon.vue`/`streamEvents.ts` 的字样为注释/图标名,不动)。
4. vitest `components/chat/ActivityPanel.test.ts`(design §4 五条)。
5. 验证:`cd app && pnpm test` 全量 + `pnpm lint`(若 package.json 有)或项目等价 type-check。

## Step 5 收尾自查(交给 trellis-check)

- 全量:`cargo test -p everlasting --lib` + `cd app && pnpm test`。
- diff 审查:checklist store 零改动;wire casing(camelCase struct + snake_case 命令参数)两路径一致;无锁内发射;四注册点齐全。
- 跑 `pnpm test:e2e` 确认无既有用例回归(不新增用例)。

## 回滚点

- 每 Step 独立可编译;若 Step 4 组件替换出问题,可临时保留 ChecklistCard 双挂载回退(仅本地,不提交)。
- 最终回滚 = revert 整个任务提交(无 DB/契约破坏)。
