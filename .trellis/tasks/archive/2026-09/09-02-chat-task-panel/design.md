# Design — Chat 任务面板

前置阅读:`research/existing-architecture.md`(全部 file:line 引用不重复)。

## 1. 总体形状

```
ChatPanel.vue
  └─ <ActivityPanel :session-id :items="currentChecklist" />   (替换 ChecklistCard)
       ├─ section 子代理   ← subagentRuns.runSummaryBySession (已有,零改动)
       ├─ section 后台命令 ← 新 useBackgroundShellsStore
       └─ section 清单     ← checklist items prop (渲染自 ChecklistCard 迁移)
  └─ SubagentDrawer (已有,挂 ChatWindow,面板行点击 openDrawer 复用)
```

后端新增(唯一新面):

```
background_shell/mod.rs      trait + ShellEventPayload + emitter 类型
background_shell/in_memory.rs list_for_session + emitter 注入 + 三处发射
commands/background_shells.rs list_background_shells / kill_background_shell (_inner + tauri cmd)
daemon/routes/background_shells.rs  两条 POST route
lib.rs / daemon/server.rs    emitter 双接线 + 命令注册(commands/mod.rs 名单)
```

## 2. 后端契约

### 2.1 摘要类型(Rust,camelCase wire 对齐 SubagentRunSummary 前例)

```rust
// background_shell/mod.rs
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundShellSummary {
    pub shell_session_id: String,
    pub session_id: String,
    pub command: String,
    /// "running" | "completed" | "failed" | "killed" | "timed_out" | "spawn_failed"
    pub status: String,
    /// 进程单调 ms;仅用于时长相减,前端禁止当墙钟
    pub started_at_ms: u64,
    /// running: now-started_at(取值时算);终态: completed_at-started_at
    pub elapsed_ms: u64,
    pub exit_code: Option<i32>,
    pub stdout_preview: Option<String>,
    pub stderr_preview: Option<String>,
    pub full_output_path: Option<String>,
    pub origin_tool_use_id: Option<String>,
}
```

`list_for_session` 从 `ShellEntry` 构建:`Running → status=running, elapsed=now_ms()-started_at`;`Done → 由 notification.outcome 映射 status 字符串`,elapsed 用 `notification.completed_at - started_at`,previews 从 entry.stdout/stderr 经现有 `status_preview` 生成(`build_status_from_entry` 逻辑复用/抽公共 helper,不要两份拷贝)。排序 list 返回时 running 优先、其余按 started_at 倒序(前端不再排也行,见 §3.4)。

### 2.2 事件

- 事件名常量:`pub const EVENT_NAME: &str = "background_shell:update";`
- payload(全部 camelCase,单 struct 三 kind 共用):

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellEventPayload {
    pub kind: &'static str,   // "started" | "exited" | "pruned"
    pub session_id: String,
    pub shell_session_id: String,
    /// started/exited 必带;pruned 为 None(前端只按 id 删)
    pub shell: Option<BackgroundShellSummary>,
}
```

- emitter 注入:registry 加 `notify: StdMutex<Option<Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync>>>` + `pub fn set_event_emitter(..)`(幂等,只在 None→Some 允许,防测试串扰)。同步闭包,调用点不得持锁调用(先 drop guard)——**in_memory 里三处发射点都要审查锁作用域**。
- 发射点:
  1. `start()` 成功插入 Running 后 → `started`(SpawnFailed 分支插的是 Done → 发 `exited`);
  2. `run_background_task` 写 Done + push notification 后 → `exited`(kill/超时/正常全在此汇合,注意在 lock drop 之后发射);
  3. `sweep_completed_shells` 每删一条 → `pruned`(逐条发,prune 本来就低频)。
- `kill_all_for_session` / `kill_all`:进程组 kill 经 kill_tx 汇入 `run_background_task` → 已覆盖,不额外发射。

### 2.3 IPC 命令

```rust
// commands/background_shells.rs
list_background_shells_inner(session_id, state) -> Vec<BackgroundShellSummary>  // 无序或排序后
kill_background_shell_inner(session_id, shell_session_id, state) -> ()          // registry.kill 透传
```

tauri `#[tauri::command]` 薄包装(参数 snake_case,transport 自动 camelCase 互转);daemon routes `POST /api/v1/background_shells/list_background_shells` 与 `/kill_background_shell`(request struct 照抄 `ListSubagentRunsBySessionRequest` 模式)。注册四处:`lib.rs` invoke_handler、`commands/mod.rs` 命令名单(Thin 代理白名单)、`daemon/routes/mod.rs` nest、新 route 文件 router()。

### 2.4 emitter 接线(两进程)

- daemon:`daemon/server.rs` 构造/装配 AppState 处(紧邻 `spawn_shell_sweeper` 装配点):`state.background_shells.set_event_emitter(Arc::new(move |name, payload| sse.broadcast(name, payload)))`,`sse = state.sse.clone()`。
- Tauri Full:`lib.rs` setup 中 `AppState::load`(:185)之后,用 `app_handle.clone()` 设 `|name, payload| { let _ = app_handle.emit(name, payload); }`(serde_json::Value 实现 Serialize;失败只 warn,对齐 AppHandleSink 前例)。
- Thin 模式 GUI 无 AppState:不接线,天然 no-op。

## 3. 前端契约

### 3.1 类型 + store `stores/backgroundShells.ts`

```ts
export interface BackgroundShellSummary {   // 镜像 §2.1,camelCase
  shellSessionId: string; sessionId: string; command: string;
  status: "running"|"completed"|"failed"|"killed"|"timed_out"|"spawn_failed";
  startedAtMs: number; elapsedMs: number;
  exitCode: number | null; stdoutPreview: string | null;
  stderrPreview: string | null; fullOutputPath: string | null;
  originToolUseId: string | null;
}
interface ShellEventPayload { kind: "started"|"exited"|"pruned"; sessionId: string;
  shellSessionId: string; shell: BackgroundShellSummary | null; }
```

Store(Pinia setup,仿 subagentRuns 的 listener 惯例):

- 状态:`shellsBySession: reactive(Map<sessionId, BackgroundShellSummary[]>)`。
- `fetchForSession(sessionId)`:invoke `list_background_shells` → **整表替换**(与事件增量的小竞态接受 last-write-wins,prune 复活属化妆级问题,自愈于下一条事件;design 取舍记录于此)。
- `start()`:`transport.listen("background_shell:update")` — `started/exited` 按 shellSessionId upsert;`pruned` 按 id 删。listener 只需挂一次(全局),面板 mount 不重复挂(组件 onUnmounted **不** stop,与 subagentRuns 由 ChatView 级 start/stop 管生命周期不同——本 store 若无现成全局 start 点,则在 store 内 `ensureStarted()` 幂等懒挂,首个消费组件调用)。
- `kill(sessionId, shellSessionId)`:invoke `kill_background_shell`;失败 extractErrorMessage 抛给调用方 toast。终态不本地伪造,等 `exited` 事件。
- `clearSession(sessionId)`;`stop()` 供对称(测试用)。

### 3.2 组件 `components/chat/ActivityPanel.vue`

Props:`items: ChecklistItem[] | null`(沿用)、`sessionId: string | null`。

- 直接注入 `useSubagentRunsStore()` / `useBackgroundShellsStore()`,computed 取 `sessionId` 对应数组(subagent 列表现有 `runSummaryBySession`)。
- `onMounted` / `watch(sessionId, immediate)`:两个 store 各 `fetchForSession`(subagent 的 fetch 现在只有 ToolCallCard 懒加载 + 事件 eager 路径;面板显式拉一次,保证历史 session 直接可见)。
- 可见性:`checklist !== null || subagents.length > 0 || shells.length > 0`。
- 结构:沿用 ChecklistCard 的 panel/ball 骨架与 CSS 变量;header 标题"运行状态",副区显示 `N 运行中`(=running subagents + running shells,in_progress checklist 不计入此徽标,避免与清单进度重复);每 section 一个小节头(icon + 标题 + count)。整卡 max-height 60vh 内滚动。
- 子代理行(点击 → `subagentRuns.openDrawer(s.id)`):
  - 状态图标映射:running→loader(app-spin)/ completed→check-mini / error→x / cancelled·incomplete→circle(中性色);
  - 名称 `subagentName`;模型 chip:`modelDisplay` 非空才渲染(null=继承父级,遵循 AC14-15 前例);
  - 时长:running 显示实时 elapsed(`startedAt` 是墙钟 ISO 字符串,与 shell 不同源!可用 `Date.now()` 差值,秒级刷新可接受或仅静态显示已完成时长);completed/终态显示 `finishedAt-startedAt` 静态时长。
- 后台命令行(点击 toggle 本行展开,`expandedShellId` 单选):
  - 状态图标:running→loader / completed→check-mini / failed·spawn_failed→x(红)/ killed·timed_out→square(中性);
  - 主文本 `command`(mono,truncate);右侧 chip:running 显示 elapsed(每秒 tick 或 10s tick,实现取简:30s interval 或仅在展开/重渲染时更新——取 5s interval 的轻 tick)、终态显示 `exit_code`(非 0 红)+ duration;
  - running 行 hover 显示终止按钮(x icon,stopPropagation,调 store.kill);
  - 展开区:pre-wrap mono,`stdoutPreview`/`stderrPreview` 分块,`full_output_path` 非空时提示"完整输出已落盘 + 路径";running 且无 preview 时显示"运行中,尚无输出可读"。
- 清单 section:整体搬运 ChecklistCard 的 items 渲染(含 pending/in_progress/done 样式、app-spin fill-box 细节、empty 态文案);all-done 绿 tint 移到清单 section 头或整卡(实现取整卡 tint,保现状视觉)。
- 浮球:显示运行中徽标(>0 时数字)+ checklist `done/total`(有清单时);呼吸圈条件 = (running subagents + running shells + in_progress checklist) > 0。
- 无障碍:沿用 region/aria-label 模式;更新为"运行状态面板"。

### 3.3 ChatPanel / 清理动作接线

- `ChatPanel.vue`:替换 import + 模板节点(:1082),传 `:session-id="chatStore.currentSessionId"`;`currentChecklist` computed 不动。
- `chatSessionActions.ts:228` 附近:`backgroundShells.clearSession(sid)` 与 checklist.clearSession 并列。

### 3.4 排序

后端 list 已按 running 优先 + started_at 倒序;事件 upsert 后前端**重排一次**(同一 comparator 提为 store 内导出纯函数,vitest 直测)。subagent 列表 comparator 同样放组件内纯函数导出。

## 4. 测试设计

### Rust(`cargo test -p everlasting --lib`)

- `background_shell::in_memory::tests` 新增:
  - `list_for_session_returns_running_and_done`(spawn `echo`/`sleep`,断言 shape 与排序;SpawnFailed 条目 status=spawn_failed);
  - `emitter_fired_on_started_and_exited`(注入闭包收事件到 Arc<Mutex<Vec>>,跑 `wait_terminal` 前例,断言 kind 序列 started→exited 与 payload 字段);
  - `emitter_fired_on_prune`(插 done 条目,sweep 短 retention,断言 pruned 事件 id 正确);
  - emitter 只允许设置一次(重复 set panic 或忽略——取**忽略+warn**,测试锁定)。
- commands 层:若现有 commands 测试有 AppState harness 则补一条 `_inner` 冒烟;无现成 harness 则不强行造(注册正确性由编译 + 前端 e2e 门禁外的 dev 冒烟保证,记录在 implement 验证节)。

### vitest(`cd app && pnpm test`)

- `stores/backgroundShells.test.ts`:upsert/pruned 增量、fetch 替换、comparator 排序、clearSession;事件 payload 用 canonical transport mock(spec/test-environment.md)。
- `components/chat/ActivityPanel.test.ts`:
  - 仅 checklist → 等价渲染(条目、进度、all-done);
  - 仅 subagent summary(running/completed 各一)→ 行渲染 + 点击行调 openDrawer(mount with mocked store);
  - 仅 shell(completed + running)→ 行渲染、点击展开 preview、exit code chip、终止按钮调 kill;
  - 三者全空 → 不渲染;
  - 浮球计数与呼吸圈 class。

### 手工冒烟(implement 完成后主会话执行,不进 CI)

- daemon 起后用 `scripts/turn-smoke.sh` 类似路径人工让模型 `run_background_shell sleep 30`,面板观察 running→exited;subagent 派发一轮观察 drawer 打开。

## 5. 兼容与回滚

- 纯增量后端 + 前端组件替换,无 DB migration、无 wire 破坏性变更(新命令/新事件名)。
- 回滚 = revert 提交;ChecklistCard 若保留删除则在 revert 中整体恢复(git 层面单提交内聚)。
- `ChecklistCard.vue` 删除;`MessageItem.vue`/`MessageList.vue`/`Icon.vue`/`streamEvents.ts` 中的 ChecklistCard 字样经核实为注释/图标名,不改(实现时复核)。

## 6. 已知取舍

| 取舍 | 理由 |
|---|---|
| 事件带全量 summary 而非仅 id | 省一次 IPC 往返 + 避免拉取竞态;payload ≤2KiB |
| fetch 整表替换 + last-write-wins | prune 竞态复活是化妆级问题,下条事件自愈;避免版本向量复杂度 |
| emitter 用同步闭包而非新 sink trait | 后台 shell 完成点在无 chat-loop 上下文的 watcher task,走 AppState 级单向广播即可;新 trait 会引入 sink 生命周期管理无收益 |
| shell elapsed 前端 5s tick | 避免每行 per-second timer;面板场景秒级精度无意义 |
| 不做后台 shell 实时 stdout 流 | 后端本就无流(仅完成 preview/spill),属 Non-Goal |
