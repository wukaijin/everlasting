# P1 — unify chat loop dispatch（消除 chat_loop.rs/chat.rs 副本，完全闭环 RULE-A-006）

> **可交接说明**:本 prd 面向独立实施 agent(无本 session 上下文)。下方"Current Code State"段给出精确代码快照(结构锚点 + 签名 + Guard 等价证明),实施 agent 读该段 + Research References 即可独立动手,无需翻阅本 session 历史。

## Goal

把 production `chat.rs` 的 agent loop spawn 闭包体改为调用 `chat_loop.rs::run_chat_loop`,消除两份忠实移植代码的副本,使集成测试(9 个 `agent_loop_*`)从"测副本"变为"测 production 真实路径",**RULE-A-006 从 partial 完全闭环**。

本 task 是 06-14-p1-agent-loop-integration-tests 的 follow-up:该 task 落地了 MockProvider + ChatEventSink + run_chat_loop(测试变体) + 9 集成测试,但因 production chat.rs 未接入 run_chat_loop(架构偏差),RULE-A-006 仅半闭环(见 DEBT.md RULE-A-006 Partial Closure Note)。本 task 消除该偏差。

## Decisions (ADR-lite)

**Context**:
- `run_chat_loop`(chat_loop.rs)是 production `chat.rs` spawn 闭包体的忠实移植副本。06-14-p1 task 的 check 子代理逐项核查 7 维度(C3 / MAX_TURNS / send / cancel / permission / audit / persist)对等,仅 1 处零行为影响日志差异(usage=None 时的 info! 日志)。
- PR4(06-14-p0-c3-tail-pair-orphan)已证明副本会触发漂移:R3 改 C3 degradation 处理时必须同步改 chat.rs + chat_loop.rs 两份,否则 production 与测试分叉。
- RULE-A-006 当前 partial:`permissions::check`(⑨关)真共享真闭环;C3 / cancel / max_turns / 错误路径半闭环(副本可测但 production 改动不同步则失效);persist 失败(A-003)/ audit 时序(A-004)/ worktree destroy(E-005)完全无保护(改 chat.rs,副本不同步)。

**Options**:
1. **A 一次性迁移**:删 chat.rs spawn 闭包的 agent loop body,替换为 `run_chat_loop(...)` 调用。production 与 test 共享同一函数。
2. **B 分阶段合并**:先抽更多共享 helper,逐步缩小两份差异,最后合并。
3. **C 不迁移**:保留副本,加强"改 agent loop 必须同步两份"的纪律。

**Decision**: 选 **A(一次性迁移)**。

理由:
- 两份代码刚经 PR4 同步,**几乎零漂移**,迁移成本最低(拖越久漂移越深,check 越难)。
- check 已验证 run_chat_loop 是忠实移植(非简化版),迁移 = 机械替换 + 验证,无需重写逻辑。
- B 引入多个半共享中间态,总复杂度 > A;C 留永久漂移风险且 RULE-A-006 永远 partial。

**Consequences**:
- RULE-A-006 完全闭环:9 个 agent_loop_* 测试覆盖 production 真实路径。
- chat.rs 从 ~1250 行缩减到 ~250 行。
- chat_loop.rs run_chat_loop 从 dead_code 变 production 路径,去 `#[allow(dead_code)]`。
- 单次 diff 大(删 ~1000 行 + 加调用),但逻辑零变化(忠实移植)。

---

## Current Code State（实施前必读快照）

> 行号是辅助锚点,会随代码漂移;**以结构锚点(函数名 / 注释 / 独特字符串)为准**。

### chat.rs 的 `chat` Tauri 命令（`#[tauri::command] pub async fn chat`）

分两部分:
- **pre-flight + spawn 包装**(保留,不删):命令签名 → 提取 state 字段(`tool_defs`/`db`/`catalog`/`cancellations`/`session_active_request`/`read_guard`/`memory_cache`/`permission_asks`)→ `lookup_provider_for_session`(拿 provider + context_window)→ 注册 cancel token(`cancellations` + `session_active_request` 两个 map 各 insert 一条)→ build sink(`AppHandleSink`)→ `tauri::async_runtime::spawn(async move { ... })`。
- **spawn 内的 agent loop body**(删,替换为 run_chat_loop 调用):从 `let _cancel_guard = CancellationGuard { ... }` 开始,到 spawn 闭包 `})` 结束 — 含 CancellationGuard 创建 + `load_session` + memory layers + `for turn in 1..=MAX_TURNS` turn 循环 + `provider.send` + tool 执行 + `persist_turn` + 所有 `emit`。

### chat_loop.rs 的 `run_chat_loop`（`#[allow(clippy::too_many_arguments)] #[allow(dead_code)] pub async fn run_chat_loop`）

**完整签名(14 参数,production-ready,无需改)**:
```rust
pub async fn run_chat_loop(
    tool_defs: Vec<ToolDef>,
    provider: Arc<dyn Provider>,
    context_window: u32,
    rid: String,
    session_id: String,
    messages: Vec<ChatMessage>,
    sink: Arc<dyn ChatEventSink>,
    db: SqlitePool,
    cancellations: Arc<Mutex<std::collections::HashMap<String, CancellationToken>>>,
    session_active_request: Arc<Mutex<std::collections::HashMap<String, String>>>,
    read_guard: ReadGuard,
    memory_cache: Arc<MemoryCache>,
    permission_asks: crate::agent::permissions::PermissionStore,
    token: CancellationToken,
) { ... }
```
函数体 = 上述"spawn 内 agent loop body"的忠实移植(自己创建 CancellationGuard → load_session → memory → turn loop → send → tool → persist → emit)。

### CancellationGuard（state.rs，`pub struct CancellationGuard` + `impl Drop`）

```rust
pub struct CancellationGuard {
    pub cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,
    pub session_active_request: Arc<Mutex<HashMap<String, String>>>,
    pub request_id: String,
    pub session_id: String,
}
impl Drop for CancellationGuard {
    fn drop(&mut self) {
        // spawn 一个 task:lock cancellations map remove(request_id),
        // lock session_active_request map remove(session_id)
    }
}
```

**两处 Guard 等价证明(R3 正确性依据)**:
- chat.rs spawn 内的 Guard:`CancellationGuard { cancellations, session_active_request, request_id: rid, session_id }`
- run_chat_loop 内的 Guard(`let _cancel_guard = crate::state::CancellationGuard { ... }`):字段完全相同(同 4 字段,同值)。
- **结论**:两处 Guard 是同一个 struct 的等价实例,Drop 行为完全相同。**移除 chat.rs spawn 内的 Guard、保留 run_chat_loop 内的 Guard,map 清理行为零变化**(仍清理 `cancellations[rid]` + `session_active_request[session_id]`)。这是 R3 单一化的安全依据。

---

## Requirements

### R1 — chat.rs spawn 闭包体替换为 run_chat_loop 调用
* 删 chat.rs spawn 内的 agent loop body(见 Current Code State 的"spawn 内 agent loop body"边界),替换为单次 `run_chat_loop(...).await` 调用。
* 保留 pre-flight + spawn 包装(见 Current Code State 的"pre-flight + spawn 包装")。
* production 调用参数对照 run_chat_loop 签名(14 参数),全部从 chat 命令作用域取:
  - `tool_defs` ← state.tools.clone();`provider` ← resolved.provider;`context_window` ← resolved.context_window;`rid` ← request_id;`session_id` ← session_id;`messages` ← messages(入参);`sink` ← sink(AppHandleSink);`db`/`cancellations`/`session_active_request`/`read_guard`/`memory_cache`/`permission_asks` ← state 字段 clone;`token` ← CancellationToken::new()。
  - 参照 `agent/tests.rs` 的 `agent_loop_basic_text_only_completes` 测试调用,参数顺序/类型一致。

### R2 — 去除 run_chat_loop 的 dead_code 标记
* 删 chat_loop.rs `run_chat_loop` 上方的 `#[allow(dead_code)]`(production 现在调用)。
* 保留 `#[allow(clippy::too_many_arguments)]`。
* **不加** `#[cfg(test)]`(run_chat_loop 接 `Arc<dyn Provider>`,production / test 通用)。

### R3 — CancellationGuard 单一化
* 移除 chat.rs spawn 内的 `let _cancel_guard = CancellationGuard { ... }`(连同它的注释)。
* 保留 run_chat_loop 内的 Guard(`let _cancel_guard = crate::state::CancellationGuard { ... }`)。
* **等价性已证明**(见 Current Code State):两处 Guard 同 struct 同字段同 Drop,移除一个行为不变。
* 验证:cancel 测试 `agent_loop_cancel_in_turn_2_kills_loop` pass + 迁移后手动 cancel 验证 map entry 清除(见 DoD checklist)。

### R4 — 验证 production 行为与原闭包一致
* run_chat_loop 的 emit 序列(chat-event / tool:call / tool:result / permission:ask 四通道)必须与原 chat.rs 闭包完全一致(wire contract)。
* **验证方法**(可执行):
  1. **迁移前录 baseline**:记下迁移前 chat.rs 闭包的 emit 点数量(grep `app_handle.emit` + `emit_chat_event` helper),与 run_chat_loop 的 emit 点数量(grep `emit_chat_event_via_sink` + `sink.emit_*`)对照 — 应一一对应(check 已验证 ~17 vs ~18 对应)。
  2. **迁移后跑全套**:9 个 agent_loop_* 集成测试用 MockProvider 跑固定 script,断言 emitter 事件序列 — 这些测试在迁移后覆盖 production 路径,pass = emit 序列一致。
  3. **手动验证**(见 DoD checklist):启动 app 跑 chat,前端行为对照。
* 重点路径:basic text-only / tool_use loop / cancel / max_turns / C3 degradation(PR4)/ error path — 对应 9 个 agent_loop_* 测试。

### R5 — docstring 更新
* chat_loop.rs module docstring(顶部 ~50 行)的 "Drift hazard" 段 + "pending decision" 描述更新:production 现在路由通过 run_chat_loop,副本已统一,drift hazard 消除。
* chat.rs `chat` 命令 docstring 更新:agent loop body 现在委托 run_chat_loop。
* run_chat_loop 上方 docstring 的"Test entry point...consumed ONLY by tests"改为"production + test entry point"。

### R6 — DEBT + ARCHITECTURE 收口
* DEBT.md RULE-A-006:status partial → **closed**,Partial Closure Note → Closure Note(副本已统一,漂移消除)。
* ARCHITECTURE.md 相关段:加注"agent loop body 已统一(chat.rs → run_chat_loop),RULE-A-006 完全闭环"。

---

## Acceptance Criteria

* [ ] chat.rs spawn 闭包体不再含 agent loop body(load_session / turn loop / send / tool / persist / emit),改为 `run_chat_loop` 调用
* [ ] run_chat_loop 去 `#[allow(dead_code)]`
* [ ] CancellationGuard 单一(chat.rs 移除 spawn 内 guard,run_chat_loop 内 guard 负责)
* [ ] 全套测试 pass(**预期 484,不变** — 删的是 production 代码不是测试;9 个 agent_loop_* 现覆盖 production 路径)
* [ ] `cargo check` 0 warning(生产 + test)
* [ ] DEBT.md RULE-A-006 status → closed
* [ ] chat_loop.rs docstring 更新(drift hazard 消除)
* [ ] DoD 手动 checklist 全过

---

## Definition of Done

* 上述 AC 全 ✅
* PR merge 后 DEBT.md RULE-A-006 `Status: closed` + Closure Note

**手动验证 checklist**(迁移后执行,确认前端行为与迁移前一致):
1. `cd app && pnpm tauri dev` 启动 app
2. **text-only**:发一条普通消息 → 前端正常流式显示回复(chat-event delta/done 序列)
3. **tool_use**:发触发工具的消息(如让模型调 list_dir)→ 前端显示 tool 卡片 + tool:result + 后续回复(tool:call/tool:result 序列)
4. **cancel**:发消息中途点 Stop 按钮 → 前端显示 "[已停止]" + 干净停止(done cancelled + map 清理)
5. **error**:触发错误(如断网或无效请求)→ 前端显示 error toast + 状态一致(前端 error case 独立 reload,见 Technical Notes)
6. **多 turn**:连续 3+ 轮对话 → 前端历史与 DB 一致(persist_turn 正常)
7. **permission:ask**(若触发 Tier 3):权限弹窗正常显示 + 决策后继续(permission:ask 序列)

---

## Out of Scope

* agent loop 逻辑变更(纯架构统一,逻辑零变化 — run_chat_loop 已是忠实移植)
* 新增集成测试(本 task 让现有 9 个测试覆盖 production)
* MockProvider / ChatEventSink trait 改动(已就绪)
* 重构 run_chat_loop 签名(14 参数保持)
* 抽 chat_inner(06-14-p1 PRD R2 原方案 A)— 直接统一到 run_chat_loop,不抽中间层

---

## Technical Approach

### Step 1: 替换 chat.rs spawn 闭包体

**删/留精确清单**(对照 Current Code State):

| chat.rs 内容 | 操作 |
|---|---|
| `chat` 命令签名 + state 字段 clone | **留** |
| `lookup_provider_for_session` pre-flight | **留** |
| cancel token register(两个 map insert) | **留** |
| `let sink = AppHandleSink { ... }` | **留** |
| `tauri::async_runtime::spawn(async move {` | **留**(包装) |
| spawn 内 `let _cancel_guard = CancellationGuard { ... }` | **删**(run_chat_loop 内有等价 Guard) |
| spawn 内 `let mut messages = messages;` 到闭包结束(load_session / memory / turn loop / send / tool / persist / emit 全部) | **删**(替换为下方调用) |
| spawn 内新增 `run_chat_loop(14 参数).await;` | **加** |

替换后 spawn 体形如:
```rust
tauri::async_runtime::spawn(async move {
    // run_chat_loop owns its own CancellationGuard (cleans
    // cancellations + session_active_request maps on every exit
    // path). The chat command's pre-flight (provider lookup,
    // token registration, sink build) stays here.
    run_chat_loop(
        tool_defs, provider, context_window,
        rid.clone(), session_id.clone(), messages,
        sink_for_spawn, db, cancellations, session_active_request,
        read_guard, memory_cache, permission_asks, token,
    ).await;
});
```
注:`rid`/`session_id` 用 `.clone()`(run_chat_loop 拥有自己的 String),其余 move。

### Step 2: 去 run_chat_loop dead_code
删 chat_loop.rs `run_chat_loop` 上方 `#[allow(dead_code)]` 行。

### Step 3: docstring 更新
chat_loop.rs module docstring 重写:移除 "Drift hazard" 段,更新为"production + test 共享 entry point"。

### Step 4: 验证(见 R4 验证方法)
```bash
cd app/src-tauri
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib              # 预期 484 pass,0 failed
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo test --lib agent::tests # 9 agent_loop_* 现覆盖 production
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check                    # 0 warning
PKG_CONFIG_PATH="/usr/lib/x86_64-linux-gnu/pkgconfig:/usr/share/pkgconfig" cargo check --tests            # 0 warning
```
然后执行 DoD 手动 checklist。

---

## Technical Notes

### 关键文件
* `app/src-tauri/src/agent/chat.rs` — 删 spawn 闭包体(~1000 行),保留 pre-flight + spawn 包装
* `app/src-tauri/src/agent/chat_loop.rs` — 去 dead_code,更新 docstring
* `.trellis/reviews/DEBT.md` — RULE-A-006 partial → closed
* `docs/ARCHITECTURE.md` — 加注 agent loop 统一

### 风险
1. **emit wire contract**:run_chat_loop 的 emit 必须与原闭包完全一致。check 已验证(7 维度对等),迁移后按 R4 验证方法(全套测试 + emit 点对照 + 手动 checklist)确认。
2. **CancellationGuard**:已证明两处等价(Current Code State),移除 chat.rs 的、保留 run_chat_loop 的行为不变。**不要两个都留**(冗余双清理)或**两个都删**(map 泄漏)。
3. **pre-flight 时机**:chat.rs 的 lookup_provider(spawn 前,同步)保留;run_chat_loop 的 load_session(spawn 内,异步)保留 — 职责不同(catalog vs session messages),不冲突。
4. **diff 大**:删 ~1000 行 + 加 ~20 行调用。逻辑零变化,review 聚焦"删的代码 = run_chat_loop 函数体"。

### 前端 error case 独立 reload（PR4 发现,迁移后若 emit 变化相关）
* 前端 `app/src/stores/streamController.ts` 的 error case 独立 reload:`finalizeRequest(errored=true)` → `reloadAfterFinalize` → `invoke("load_session")` 从 DB 重读。**不依赖 TurnComplete**。
* 含义:迁移后即使 error 路径的 emit 有细微差异,前端不会卡(独立 reload)。但 done / tool:call / tool:result 的 emit 时序仍需一致(影响 UI 流式显示)。
* StillOver(PR4 的 C3 超窗)路径不 emit TurnComplete 是正确的(无 persist,emit 反违反 F5),迁移后保持。

### 回滚策略
* 迁移是单次大 diff。**迁移前确保 working tree clean + 迁移在独立 commit**(不与其他改动混合),便于 `git revert <commit>` 回滚(原闭包体恢复)。
* 若部分异常(某 emit 时机差),对照 run_chat_loop 与原闭包该路径(用 git show <commit>^:app/src-tauri/src/agent/chat.rs 看原闭包),定位差异点修。

### 与 PR4 的关系
PR4 已同步两份代码(degradation match 逐行一致),本 task 迁移时 run_chat_loop 与 chat.rs 闭包**零漂移** — 迁移成本最低时机。若先做其他改 agent loop 的 task 再迁移,漂移积累,成本上升。

### 为何不抽 chat_inner(06-14-p1 PRD R2 原方案 A)
06-14-p1 原选"抽 chat_inner 函数共享",实际走了 run_chat_loop 副本路线。本 task 直接统一到 run_chat_loop(已存在且测试覆盖),不抽中间层。

---

## Research References
* `.trellis/reviews/DEBT.md` — RULE-A-006 Partial Closure Note(迁移方案 + 风险 + 完全闭环路径)
* `app/src-tauri/src/agent/chat_loop.rs` module docstring — Drift hazard 段(副本架构自述,迁移后需更新)
* `app/src-tauri/src/agent/chat_loop.rs` — run_chat_loop 完整签名 + 函数体(迁移目标)
* `app/src-tauri/src/agent/chat.rs` — chat 命令 + spawn 闭包(迁移源,删 agent loop body)
* `app/src-tauri/src/state.rs` — CancellationGuard 定义 + Drop(R3 等价性依据)+ ChatEventSink trait + AppHandleSink
* `.trellis/tasks/archive/2026-06/06-14-p1-agent-loop-integration-tests/prd.md` — 原 P1 task(副本架构起源)
* `.trellis/tasks/archive/2026-06/06-14-p0-c3-tail-pair-orphan/` — PR4(副本同步实践,证明漂移风险真实)
* `app/src/stores/streamController.ts` — 前端 SSE 处理(error case 独立 reload,R4/风险参考)
