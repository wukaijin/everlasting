# B6 Subagent — PRD + 调研 Review

- **Reviewed**: 2026-06-19
- **Documents**:
  - `.trellis/tasks/06-19-b6-subagent/prd.md`
  - `.trellis/tasks/06-19-b6-subagent/research/subagent-patterns-survey.md`
- **Cross-referenced source**:
  - `agent/chat_loop.rs` (signature, CancellationGuard, MAX_TURNS, memory injection, checklist/notification APPEND)
  - `agent/mod.rs` (MAX_TURNS const)
  - `tools/mod.rs` (builtin_tools, ToolContext, execute_tool_inner signature)
  - `.trellis/spec/backend/tool-contract.md` (update_checklist, L1a background shell)
  - `.trellis/spec/backend/agent-loop-architecture.md` (14-param rationale, RULE-A-006/007, CancellationGuard equivalence)
  - `docs/ROADMAP.md` §4.1 (B6 scope)

---

## 1. 总体判断

两份文档质量都很高。调研覆盖了 5 个业界工具的 8 维度对比，映射到 Everlasting 的 §10 具体可行。PRD 的 9 个决策点全部收敛，3-PR 拆分合理。**PRD 可以进入实现**，但有以下需要修正的技术细节。

---

## 2. 事实性错误

### 2.1 `run_chat_loop` 参数数量（贯穿全文）

PRD、spec 文档 (`agent-loop-architecture.md`)、ROADMAP 都写"14 参数"。**实际代码是 17 个**:

```rust
// agent/chat_loop.rs:96-128
pub async fn run_chat_loop(
    tool_defs: Vec<ToolDef>,                                                      // 1
    provider: Arc<dyn Provider>,                                                   // 2
    context_window: u32,                                                           // 3
    rid: String,                                                                   // 4
    session_id: String,                                                            // 5
    messages: Vec<ChatMessage>,                                                    // 6
    sink: Arc<dyn ChatEventSink>,                                                  // 7
    db: SqlitePool,                                                                // 8
    cancellations: Arc<Mutex<HashMap<String, CancellationToken>>>,                 // 9
    session_active_request: Arc<Mutex<HashMap<String, String>>>,                   // 10
    read_guard: ReadGuard,                                                         // 11
    memory_cache: Arc<MemoryCache>,                                                // 12
    skill_cache: Arc<SkillCache>,                                                  // 13
    permission_asks: crate::agent::permissions::PermissionStore,                   // 14
    token: CancellationToken,                                                      // 15
    resend_seq: Option<i64>,             // 16 — D3 (2026-06-17)
    background_shells: DefaultRegistry,  // 17 — L1a (2026-06-19)
)
```

PRD 的伪代码示例已正确列出 `background_shells`，但总数字和 DoD 约束仍写 14。需要全局替换为 17。

---

## 3. 架构风险

### 3.1 ⚠️ CancellationGuard 双重 remove（严重）

目前 `run_chat_loop:135-140` 创建的 `CancellationGuard` 在 Drop 时同时 remove `cancellations` 和 `session_active_request` 两个 map。PRD 正确指出 worker rid **不应进** `session_active_request`，但忽略了 worker 的 `CancellationGuard` 会用 `parent_session_id` 作为 key 去 remove `session_active_request`，导致**父 session 的 active request 映射被错误清理**：

```
父: session_id = "session-1", rid = "req-1"
  └─ worker: rid = "req-1-sub-abc", session_id = "session-1"  ← 复用父 session_id

worker 的 CancellationGuard::drop():
  cancellations.remove("req-1-sub-abc")       ← ✅ 正确
  session_active_request.remove("session-1")  ← ❌ 误删父的！
```

修复方案（三选一，**推荐方案 A**）:

| 方案 | 说明 |
|---|---|
| **A** `CancellationGuard` 加 `skip_session_active: bool` | worker 传 `true` 跳过 `session_active_request` 清理 |
| B | worker 不创建 `CancellationGuard`，手动管理清理（脆弱，不推荐） |
| C | worker 传入独立 dummy `session_active_request` map（浪费） |

### 3.2 ⚠️ `dispatch_subagent` 依赖传递路径（中等）

PRD 的核心架构是 `dispatch_subagent::execute` 内调 `run_chat_loop`。但 `execute_tool_inner` (`tools/mod.rs:172`) 的入参只有:

```rust
async fn execute_tool_inner(
    name, input, ctx, guard, session_id, skill_cache, cancel
) -> (String, bool, ToolContextUpdate, Option<i32>)
```

缺少 `run_chat_loop` 必需的 `provider`、`db`、`cancellations`、`session_active_request`、`read_guard`、`memory_cache`、`permission_asks`、`background_shells`。

修复方案:

| 方案 | 说明 |
|---|---|
| **A: agent loop 层拦截** | 在 `chat_loop.rs` 的 tool_use 处理循环里，识别 `dispatch_subagent`，不走 `execute_tool`，直接调专门的 subagent 函数 |
| B: 扩展 `ToolContext` | 把 `provider`/`db`/`cancellations` 等都塞进 `ToolContext`——模糊了工具层和 agent 层边界，不推荐 |
| C: `ToolContext` 加 opaque 扩展字段 | `HashMap<String, Box<dyn Any>>` extension point，过于 hacky |

**推荐方案 A**。`dispatch_subagent` 本质上是"agent 层的控制流工具"而非"文件系统/I/O 工具"，在 agent loop 层拦截是合理的。类比 Claude Code 的 Agent tool 也是 SDK 层特殊处理的，不走普通 tool execution 路径。

### 3.3 MAX_TURNS 硬编码 vs worker 需要 20（中等）

`run_chat_loop:492` 直接 `for turn in 1..=MAX_TURNS`（`agent/mod.rs:56` 的 `pub const MAX_TURNS: usize = 50`）。PRD 说 worker 需要 `max_turns=20`，但同时也说"复用 run_chat_loop 现有 MAX_TURNS 路径"。这两者不可能同时成立——不改签名就无法传不同值。PRD 自己承认"待实现时定"。

**推荐加第 18 个参数 `max_turns: Option<usize>`**（None = 默认 50）。签名已经从 14 涨到 17，再加 1 个不是原则问题。`max_turns` 是 agent loop 的核心语义参数，不属于"test-only"或"hack"。

---

## 4. 设计亮点（值得保留）

以下决策正确，实现时不应偏离:

1. **嵌套 `run_chat_loop` 不改核心逻辑** — 保持 RULE-A-006（生产=测试入口）不变量
2. **APPEND 不 prepend** — B12 + L1a 踩过的坑，B6 复习到位。Worker summary 作为 tool_result 回填天然在末尾
3. **worker rid 只进 cancellations 不进 session_active_request** — 正确识别了 `session_active_request` 的 1:1 约束
4. **结构性禁项** (`update_checklist` / `dispatch_subagent` / L1a 三件) — 对标 Claude Code 的 UI 工具排除，设计合理
5. **SubagentBufferSink** — 隔离 worker 中间过程，只把 summary 注入父 sink，保持主对话干净
6. **3-PR 拆分** — PR1 核心(无 DB) → PR2 持久化 → PR3 前端，渐进式交付
7. **MockProvider 集成测试覆盖** — worker 完整/cancel/error 三个路径，延续现有 17 个 `agent_loop_*` 测试的覆盖模式

---

## 5. 调研文档评价

`subagent-patterns-survey.md` 是一份优秀的调研:

- **覆盖全面**: Claude Code / OpenHands / Cline / Cursor / Aider(反例) 5 个工具
- **维度结构清晰**: 8 维度一句话对照表 + 逐维度展开 + 映射到 Everlasting 的 §10
- **引用有来源**: 每个工具的结论都标注了文档 URL 和具体段落
- **反例有价值**: Aider architect/editor 不是真 subagent 的判断准确，避免了设计误区
- **未取到的坦诚说明**: Continue/Hermes/SWE-agent 404 都诚实记录，不假装覆盖

可改进的点:

- Fork 模式 (§9) 的 prompt cache 共享机制——调研明确说"未在调研中验证"。如果日后要做 fork 模式，需要实测 Anthropic API 在嵌套 `run_chat_loop` 调用下是否复用 `cache_control: Ephemeral` breakpoint
- OpenHands 的 `LLMSummarizingCondenser` 只提到概念，未深入实现约束（是否需第二个 LLM call？latency cost？）

---

## 6. 调研对 PRD 决策的支撑矩阵

| PRD 决策 | 调研支持度 | 备注 |
|---|---|---|
| 决策1: 代码内置 2 个 + 同步阻塞 | ✅ 强支持 | Claude Code built-in + OpenHands TaskToolSet 都是同步模式 |
| 决策2: worker 中间过程落独立表 | ✅ 支持 | Claude Code transcript jsonl + Cline per-subagent stats |
| 决策3: 继承 main mode + 无 UI sink ask→deny | ✅ 强支持 | Claude Code background subagent auto-deny 完全同构 |
| 决策4: allowlist + 结构性禁项 | ✅ 强支持 | Claude Code 的 `tools`/`disallowedTools` + UI 工具排除 |
| 决策5: audit/token 不污染父 session | ✅ 支持 | 所有工具都隔离中间过程，只回 summary |
| 决策6: worker 加载 B5 memory | ✅ 强支持 | Claude Code subagent 加载 CLAUDE.md |
| 决策7: MVP free-text summary | ✅ 支持 | 所有工具的 summary 都是 LLM 自生成，无二次裁剪 |
| 决策8: max_turns=20 | ✅ 支持 | `maxTurns`/`max_iteration_per_run` 是普遍兜底 |
| 决策9: MVP 串行 | ✅ 支持 | 对标 OpenHands TaskToolSet |

---

## 7. 建议修改清单（在实现前完成）

| # | 问题 | 建议 |
|---|---|---|
| 1 | "14 parameters" 全文过时 | 全局替换为 "17 parameters"（PRD 代码示例 + DoD + spec 文档） |
| 2 | CancellationGuard 双重 remove | 采用方案 A: `CancellationGuard` 加 `skip_session_active: bool`，worker 传 `true` |
| 3 | dispatch_subagent 依赖传递 | 从"在 `execute` 内调 `run_chat_loop`"改为"在 agent loop 层拦截，不经过 `execute_tool_inner`"，给出 agent loop 中的拦截点伪代码 |
| 4 | max_turns 决策悬而未决 | 明确加第 18 个参数 `max_turns: Option<usize>`（None = 默认 50） |
| 5 | worker permission 检测方式模糊 | 从"sink 类型或显式 flag"改为明确的 `PermissionContext { is_worker: bool }` 字段 |
| 6 | worker context 构造细节缺失 | 明确 worker messages 顺序: `[memory_blocks_user_message, delegation_task_user_message]`——memory 在前享受 `cache_control`，task 在后 |

---

## 8. 结论

两份文档一起构成扎实的 B6 设计基础。调研充分、PRD 决策合理、3-PR 拆分务实。§3 的三个风险点都是实现层面可解决的——不影响设计正确性，但必须在第一行代码之前锁定方案。

**整体评估**: ✅ 可以进入实现。建议先花 30 分钟修正 §7 的 6 个问题，再开始写 PR1。
