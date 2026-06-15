# P1: 用 mtime read-through 轮询取代 notify watcher，确定性消除 memory 读 race (RULE-C-001/C-002/C-004)

## Goal

让 memory/指令文件系统兑现 PRD 承诺的「立即生效」：编辑器保存 4 个指令文件（User/Project × CLAUDE.md/AGENTS.md）后，下一条 chat 消息**确定性地**读到最新内容。做法是**删掉 notify watcher（含 debounce 状态机），改用 read-through 时的 mtime 比较**——以文件系统 mtime 为权威失效信号，read 本身确定性正确，不依赖任何后台 watcher。

## 🔴 关键现状发现（brainstorm 阶段核实，促成方案转向）

**DEBT.md RULE-C-004 原描述**："成功路径返回值被丢弃……靠 notify 内部 thread 生命周期间接维持，结构不健壮"。

**实际代码核实（state.rs:217-224）**：`start_watcher(...)` 只 match `Err`，返回的 `Ok(MemoryWatcher)` 被直接丢弃。后果链（notify `RecommendedWatcher` 标准 RAII Drop）：

```
MemoryWatcher drop → _watcher drop → inotify fd 关闭
  → callback 闭包 drop → channel tx drop
  → debounce loop 的 rx.recv() 返回 None → loop 退出
```

→ watcher **疑似完全失效**：`MemoryCache` 首次 `load_for_session` 填充 slot 后永不失效，改文件必须重启 app。这比 RULE-C-001（"概率性读旧"）严重——当前是**确定性读旧**。

** grill 转向**：既然 watcher 根本没在工作，且 read-through 加 mtime 比较能确定性消除 race（让 watcher 的 debounce `invalidate` 彻底冗余），不如**直接砍 watcher 改 mtime 轮询**——净简化，C-004/C-002 自动满足。详见 D3。

## What I already know（代码核实）

- `start_watcher` **唯一调用点** = `state.rs:219`；`MemoryWatcher` 无其他持有者 → 删除安全。
- debounce loop（watcher.rs:167-251）`tokio::select!` 三 arm；`MemoryWatcher`（watcher.rs:49-57）持 `_watcher + _abort`。
- `MemoryCache`（loader.rs:62-140）：`RwLock<UserSlot>` + `RwLock<HashMap<String,ProjectSlot>>`，slot = `[Option<MemoryLayer>; 2]`；read-through 在 `read_or_load_user/project`（loader.rs:206-233）；invalidate_* 置 slot `None`。
- memory 读取路径：`commands/memory.rs:49`（read_memory_layers IPC，preview UI）+ `agent/chat.rs:75/201`（agent loop 每 turn 调 `load_for_session`）→ **两条路径都走 load_for_session，自动享受 mtime fence**。
- `create_project`（commands/projects.rs:57）—— W 方案下无需 hook（新 project 首次 load 即 stat，自动生效）。
- `invalidate_*` 残留调用：`commands/sessions.rs:154-160`（delete_session 调 `invalidate_project`）—— mtime 方案下冗余（文件没删，下次 read stat 发现 mtime 没变仍 hit），清理。
- watcher 无集成测试；`memory/tests.rs` 只单测 cache `invalidate_*`（这些 API 随方案删除或改造）。

## Requirements

- R1：`MemoryCache` slot 存 `mtime`（文件 `modified` time，`None` = 文件不存在）；read-through 时 `tokio::fs::metadata` stat 当前 mtime，与 cached 不符则 reload。
- R2：文件保存后任意时刻 read，确定性读到新内容（race 消除）—— 首个集成测试先证明"现状读旧"，改完证明"读新"。
- R3：删除 notify watcher 全链路：`watcher.rs` 模块、`state.rs` 的 `start_watcher` 调用 + `project_paths` 收集（保留 `list_projects` 别处用）、`mod.rs` 的 `pub mod watcher` + `WATCHER_DEBOUNCE_MS`、`Cargo.toml` notify 依赖、`invalidate_*` API 及其残留调用（delete_session 的 invalidate_project）。
- R4：mtime fence 行为有测试覆盖（改文件读新 / 不改读旧 hit / 文件删除读 Missing / stat 失败 fail-safe）。

## Acceptance Criteria

- [x] AC1：[现状反向证明] 改造前/对照——写测试证明"无 mtime fence 时改文件后 load 仍读旧"（或直接断言改造后行为，跳过反向）。
- [x] AC2：[fence 生效] load → 改文件 → 再 load，读到新内容（mtime != cached → reload）。
- [x] AC3：[hit 正确] load → 不改 → 再 load，走 cache hit（不重复 load_layer，可断言内容一致 + 无多余 IO）。
- [x] AC4：[文件删除] load(Loaded) → 删文件 → load(Missing)。
- [x] AC5：[stat fail-safe] stat 出错时当作 mtime 变化 → reload（load_layer 内部已处理 Error）。
- [x] AC6：[新 project 无需重启] 运行时 create_project → 写其 CLAUDE.md → 不重启 → chat 读取到新内容（首 load 即 stat，无需 watch）。
- [x] AC7：notify 依赖从 `Cargo.toml` 移除，`cargo check` 0 warning，`cargo test --lib` 全 pass。

## Definition of Done

- mtime fence 4 类行为（新/旧/删/错）均有测试。
- watcher 全链路删除，无残留 `#[allow(dead_code)]` 死代码。
- `cargo check` 0 warning，`PKG_CONFIG_PATH=... cargo test --lib` 全 pass。
- DEBT.md 三条 finding（C-001/C-002/C-004）标 closed + Closed At commit + Resolution Notes 说明"砍 watcher 改 mtime 轮询"。
- spec `backend/memory-contract.md`（若存在）+ ARCHITECTURE §2.5 memory 章节同步（watcher → mtime read-through）。

## Technical Approach

### mtime fence 设计

`MemoryCache` slot 从 `[Option<MemoryLayer>; 2]` 改为存 `mtime`：

```rust
#[derive(Clone)]
struct CachedLayer { layer: MemoryLayer, mtime: Option<SystemTime> }
type UserSlot = [Option<CachedLayer>; 2];
type ProjectSlot = [Option<CachedLayer>; 2];
```

read_or_load 改造（以 user 为例）：

```rust
async fn read_or_load_user(cache, source) -> MemoryLayer {
    let path = resolve_path(User, source, None);
    let mtime = tokio::fs::metadata(&path).await
        .ok().and_then(|m| m.modified().ok());        // err → None → fail-safe reload
    if let Some(cached) = cache.peek_user(source).await {
        if cached.mtime == mtime { return cached.layer; }  // hit
    }
    let layer = load_layer(User, source, None).await;       // miss / changed
    cache.store_user(source, &layer, mtime).await;
    layer
}
```

- **每次 read 4 次 stat**（4 文件 × per-turn），微秒级，相对 LLM 往返秒级可忽略。
- **mtime 精度**：WSL 9p/drvfs 可能秒级（RULE-C-009），"保存→下条消息"场景人不可能同秒保存两次还期望读到中间态，接受。
- **同秒两次写**：极端边缘，mtime 不变 → hit 读首次内容。接受（记录为已知限制）。
- **stat 失败**：metadata err → mtime None → 若 cached.mtime 非 None 则视为变化 → reload（load_layer 返回 Error/Missing）。fail-safe。
- **MemoryLayer clone**：hit 仍 `clone()`（现状 peek_user 已 clone），无变化。

### 删除清单

- `memory/watcher.rs`（整文件删）。
- `memory/mod.rs`：`pub mod watcher;`、`pub use` 相关、`WATCHER_DEBOUNCE_MS` 常量。
- `state.rs`：`start_watcher(...)` 调用块（:219-224）；`project_paths` 收集块（:205-215）若仅 watcher 用则删，`list_projects` 别处仍用（backfill）。
- `loader.rs`：`invalidate_user` / `invalidate_user_slot` / `invalidate_project` / `invalidate_project_slot` —— 删（mtime 方案下 read 自决，无外部失效需求）。
- `commands/sessions.rs:154-160`：delete_session 的 `invalidate_project` 调用 —— 删。
- `Cargo.toml`：`notify` 依赖。
- `memory/tests.rs`：`invalidate_*` 相关单测改写为 mtime fence 行为测试。

## Decision (ADR-lite)

### D1：任务范围（2026-06-15 grill 锁定）

- **Decision**：原定三件套（C-004 存活 + C-002 add_watch + C-001 fence）一次做完，证明测试先行。
- **Consequences**：被 D3 修订——三件套前提（修 watcher）推翻。

### D2：watcher 持有方式（2026-06-15 grill 锁定，后被 D3 推翻）

- **Decision**：原选 Approach A（AppState 加 `memory_watcher` 字段）。
- **Consequences**：**moot**——D3 决定砍 watcher，此决策不实施，保留归档说明思考路径。

### D3：C-001 race fence = 砍 watcher 改 mtime 轮询（2026-06-15 grill 锁定，**最终方向**）

- **Context**：watcher 疑似完全失效（C-004 真实严重性）；mtime read-through 能确定性消除 race 且让 watcher debounce `invalidate` 冗余。
- **Decision**：**W 方案**——删 notify watcher 全链路，read-through 时 stat 比较 mtime 决定 reload。C-004/C-002 自动满足（无需修存活、无需 add_watch，新 project 首 load 即 stat）。
- **Consequences**：
  - 净简化（删整个 watcher 模块 + debounce 状态机，换 slot 加 mtime 字段 + read stat）。
  - D1 范围修订为「删 watcher + mtime fence」。
  - 已知限制：WSL 9p mtime 秒级精度（RULE-C-009 留 OOS）、同秒两次写（极端边缘）。
  - harness 学习价值保留：mtime 轮询 vs notify 是真实工程权衡，记录进 ADR。

## Out of Scope

- RULE-C-003（token 估算 cache 折扣，P3）/ C-005（user_dir 对齐 ~/.claude/，P2 产品决策）/ C-006（4 文件总 cap）/ C-007（路径表 fallback）/ **C-009（WSL inotify 可靠性）—— 本任务改 mtime 后自然绕过 inotify，但 WSL 9p mtime 精度作为已知限制记录，polling fallback 不做**。
- session/runtime memory（V2 2 期，MemoryKind::Session/Runtime 仍占位）。

## Technical Notes

- `tokio::fs::metadata`（async），非 `std::fs::metadata`（会阻塞 runtime，对比 RULE-E-004 glob 教训）。
- 复用 RULE-A-006 的集成测试 harness 模式（MockEmitter）写 fence 行为测试；`memory/tests.rs` 用临时目录 + tokio::fs 写文件验证 mtime 变更。
- notify 版本见 `app/src-tauri/Cargo.toml`，移除前确认无其他模块引用（grep 确认仅 watcher.rs）。

## Implementation Plan（单 task 两逻辑阶段）

- **阶段 1（加 fence）**：`MemoryCache` slot 加 `mtime` + `read_or_load_*` stat fence + `store_*` helper + mtime 行为测试。此阶段 watcher 仍在（但不影响，fence 已让 read 正确）。`cargo test --lib` 绿。
- **阶段 2（删 watcher）**：删 `watcher.rs` + `state.rs` 调用 + `mod.rs` 导出 + `loader.rs` invalidate_* + `commands/sessions.rs` 残留调用 + `Cargo.toml` notify + 改写 `tests.rs` invalidate 测试。`cargo check` 0 warning，全测试绿。
- 收尾：DEBT.md 三条 closed + spec/ARCHITECTURE 同步。
