# Research: Git Auto-Commit Trigger Timing Strategy

- **Query**: 比较 4 种 git auto-commit 触发时机策略（session done / turn 结束 / 定时器 / 混合），覆盖 UX、数据安全、commit message 生成、冲突风险、与 `persist_turn` 协调、同类工具实践
- **Scope**: mixed (internal codebase + external tools reference)
- **Date**: 2026-06-07
- **Task**: `.trellis/tasks/06-07-step-4-git-worktree-auto-commit/`

---

## Findings

### 1. 项目内部上下文

#### 已有边界与触发点

| 位置 | 内容 | 行号 |
|---|---|---|
| `app/src-tauri/src/lib.rs:666` | `for turn in 1..=MAX_TURNS` agent loop 入口 | 主循环 |
| `app/src-tauri/src/lib.rs:902-920` | turn 结束、`stop_reason != "tool_use"` 时 emit `Done` 并 `return` | session 自然 done |
| `app/src-tauri/src/lib.rs:886-899` | 用户点 Stop → cancel → emit `Done { stop_reason: "cancelled" }` | cancel 路径 |
| `app/src-tauri/src/lib.rs:977-987` | 达到 `MAX_TURNS=20` → emit `Done { stop_reason: "max_turns" }` | 兜底 done |
| `app/src-tauri/src/lib.rs:658-664` | user message 落 DB 在 agent loop 之前 | persist_turn 入口 1 |
| `app/src-tauri/src/lib.rs:866-878` | assistant message 落 DB 在 turn 结束、`continue` 之前 | persist_turn 入口 2 |
| `app/src-tauri/src/lib.rs:965-972` | tool_result 落 DB 在 tool 执行完、loop 跳回 ⑥ 之前 | persist_turn 入口 3 |
| `app/src-tauri/src/db.rs:846` | `pub async fn persist_turn(...)` 定义 | DB 层落点 |

**关键观察**: `persist_turn` 在 **3 个 turn 内边界** 落 SQLite（user 提示前、assistant 回复后、tool_result 后）。这与"turn 结束 commit"策略天然对应 — 每个 turn 结束已有 1 个 DB 写点。

#### Worktree 路径与并发

- `~/.local/share/everlasting/worktrees/<project_uuid>/<session_id>`（`docs/ARCHITECTURE.md:629`）
- 分支 `session/<session_id>` — 每个 session 独立分支（`docs/ARCHITECTURE.md:629`）
- 同一 project 的两个 session 跑在不同 worktree、不同分支 → **同 project 多 session 并发不会 worktree 内冲突**
- 跨 project 并发更不会冲突（不同 repo）

#### 相关 spec / 设计

- `.trellis/tasks/06-07-step-4-git-worktree-auto-commit/prd.md:42` "Backend: auto-commit 触发" 列为 requirement，未展开时机
- `docs/ARCHITECTURE.md:420-427` ⑪ 阶段: `├─ 是否自动 commit? ├─ 是 → git add . && git commit -m "agent: <summary>" └─ 否 → 留到 session 结束统一处理` — 提到两种粒度但未锁定
- `docs/ARCHITECTURE.md:623-625` "不同 session 可能同时活跃（用户切来切去）" — 多 session 并发是显式需求

---

### 2. 四种方案 × 六维分析

#### 2.1 方案 A：session done 一次性提交

触发点: `lib.rs:919` `emit Done` 之前（自然 done）+ `lib.rs:898` cancel 之前 + `lib.rs:986` max_turns 之前。3 个 return path 都要 hook。

| 维度 | 评价 |
|---|---|
| **UX** | 用户切回 session 只看到 1 个 "agent: <session>" commit。粗粒度，diff 阅读成本高（一个 commit 可能 50+ 文件）。**负面** |
| **数据安全** | 崩在 agent loop 中段 → 0 commit，worktree 里有未提交修改。Session 删除时 `cleanup_outputs_dir` 同样 best-effort 风格 → 残留风险。**差** |
| **commit message** | 容易用 LLM 总结整段对话（agent loop 已结束，context 全在 `messages` 里），质量最高。可以引用 turn 计数、tool 计数。**最优** |
| **冲突** | 单一 commit，无 in-progress 冲突。多 session 仍独立 worktree 不冲突。**无问题** |
| **与 `persist_turn` 协调** | `persist_turn` 一直在转；commit 只在 done 时一次触发，**最简单**，无需协调 |
| **实现成本** | 3 个 return path 都要 hook。Cancel 路径要决定 "cancel 后要不要 commit"（建议：commit + marker "[cancelled]"） |

#### 2.2 方案 B：turn 结束提交

触发点: `lib.rs:866-878` `persist_turn(assistant)` 之后立即 commit。一次 assistant 消息 = 一个 commit。

| 维度 | 评价 |
|---|---|
| **UX** | 一个 turn 一次 commit。用户能看 "agent: read README" → "agent: edit file X" → "agent: run test" 的工作流时间线。**正面** |
| **数据安全** | 崩在 turn 内 → 上一个 turn 已 commit，损失 ≤ 1 turn 的 tool 输出。**较好** |
| **commit message** | 中等粒度。可以基于本 turn 的 tool calls 列表生成（"edit src/foo.rs, run cargo test"），不需 LLM 调用，O(1) 生成 |
| **冲突** | 多 session 仍独立 worktree 不冲突。**同 session 内无冲突**（顺序 commit）。但 cancel 后部分 turn 也 commit → 历史有 "cancelled" marker 即可 |
| **与 `persist_turn` 协调** | **天然契合** — `persist_turn` 写完就是 commit 触发点，复用一个 db 写边界，事务性强（可考虑 SQL + git 在同一 critical section） |
| **实现成本** | 在 `lib.rs:866-878` 后插入 commit hook。需要知道本 turn 改了哪些文件 → 可用 `git status --porcelain` 在 commit 前 diff，或 track `tools::execute_tool` 返回值 |

**风险**: tool 内部多次 IO 操作（一个 shell 命令 + 3 个文件写）会被打散到下一个 turn。Long shell run（5min timeout）阻塞时 commit 不发生，但 persist_turn 也不发生 → 风险对等。

#### 2.3 方案 C：定时器提交

触发点: 后台 `tokio::spawn` 一个 `interval(t).tick()` 任务，每 N 秒扫 `git status --porcelain`，有 dirty 就 commit。

| 维度 | 评价 |
|---|---|
| **UX** | commit 时间均匀分布，但**与 agent 语义边界脱钩**。一个 turn 可能被切到两个 commit（"turn 1 前半 + turn 2 前半"），diff 阅读体验差。**负面** |
| **数据安全** | 崩 → 最多丢 N 秒工作。**最优**，但对"个人 vibe coding"来说 N 秒损失 vs UX 损失不成比例 |
| **commit message** | 难生成（无 turn 边界），只能用 `agent: auto-save <timestamp>` 或 `agent: snapshot of N files`。**最差** |
| **冲突** | 定时器可能与正在执行的 `edit_file` 抢文件 → `git add` 时拿到半写状态。需要 `flock` 工具执行，或 commit 时跑 `git add --update` + LFS-style atomic write 假设（不现实） |
| **与 `persist_turn` 协调** | **脱钩** — 定时器不知道 turn 在哪结束，可能在 persist_turn 中间 commit（DB 写一半 + git commit 一半，外层观察者看到不一致） |
| **实现成本** | 需要新增全局 `tokio::spawn` 任务 + worktree path 索引 + 与 `AppState` 集成。复杂度高、收益不成比例 |

**结论**: 个人 vibe coding 工作台不需要工业级数据安全。**不推荐**。

#### 2.4 方案 D：混合（done 强制 + turn 软提交）

规则:
- **强制 commit** @ session done（lib.rs:919 / 898 / 986）— 兜底一定有 commit
- **软 commit** @ turn 结束，但仅当 (a) 距上次 commit 超过 N 分钟 或 (b) 自上次 commit 后改了 ≥M 个文件

| 维度 | 评价 |
|---|---|
| **UX** | 大 turn（agent 连续调 5 个 tool）有滚动 checkpoint；小 turn（短回答）积累到一个 commit。**最优**，但需要调 N/M |
| **数据安全** | 兜底 + checkpoint 双重保证。**最优** |
| **commit message** | 软 commit 可以用 "agent: continued work (turns 3-5)"；硬 commit 用 LLM 总结完整对话。**两段式** |
| **冲突** | 与 B 相同 — turn 内串行，无冲突 |
| **与 `persist_turn` 协调** | 与 B 相同 + done 时强制 |
| **实现成本** | 最高。需要在 `AppState` 加 `last_commit_at: HashMap<SessionId, Instant>` + `dirty_files: HashSet<PathBuf>`，turn 边界判断 "N 分钟过没 / M 文件过没" |

**调参风险**: N=5min × 20 turns 上限 = 最长 100min session，可能 0 个软 commit（每次 turn 都 < 5min）。N=2min 太密又退化成定时器。**MVP 不建议**，D 是 v2 优化项。

---

### 3. Commit message 生成策略

| 策略 | 适用 | 实现 |
|---|---|---|
| **LLM 总结** | 硬 commit（done 时） | 复用 `chat_stream_with_tools`，prompt: "用 ≤50 字总结本 session 的所有改动"；成本 1 次 LLM 调用，~2-5s |
| **tool 列表 + 文件列表** | 软 commit（turn 边界） | 纯本地，O(N)：`"edit src/foo.rs, src/bar.rs; run cargo test"`，零 LLM 成本 |
| **时间戳兜底** | 定时器 commit 兜底 | `"agent: auto-save 2026-06-07T15:32:00Z"`，可读性差 |
| **混合模板** | 推荐 | LLM 总结作 subject + 模板生成 body（"Turns: 5\nTools: read_file(3), edit_file(4), shell(2)\nFiles: ..."） |

**关键**: commit author 用 `agent <agent@everlasting.local>`（不冒充 user）。Co-authored-by 留给用户身份（如果想保留可追溯性）。

---

### 4. 冲突风险

#### 4.1 多 session 同 project 并发
- **场景**: User 切到 session A，再开 session B 改同文件
- **风险**: ❌ 不会冲突（独立 worktree、独立分支、独立 commit）
- **worktree 路径** `~/.local/share/everlasting/worktrees/<project_uuid>/<session_id>` 已隔离

#### 4.2 同 session 并发工具执行
- **场景**: agent loop 串行执行 tool（`for (id, name, input) in &tool_calls` at `lib.rs:924`）
- **风险**: ❌ 无并发，commit 与 tool 串行；唯一风险是 tool 内部 spawn 子进程改了 worktree 外文件（shell 跑 `cd /tmp && vim foo`）— 但 commit 只 `git add .` worktree 内，外部改动不入 commit

#### 4.3 定时器与 tool 抢文件（仅方案 C）
- **风险**: ⚠️ 定时器在 `git add` 时拿到半写文件 → 损坏 commit object
- **缓解**: 不实现 C 即可；或 commit 时 `git stash` + `git stash pop`（复杂度爆炸）

#### 4.4 用户手动改 worktree
- **场景**: 高级用户 `cd ~/.local/share/everlasting/worktrees/...` 手动改
- **风险**: 自动 commit 把手动修改也带走 → 用户预期外
- **MVP 不解决**，spec "Out of Scope: 历史 commit 的 UI 时间线" 暗示 worktree 是 agent 沙箱

#### 4.5 Git 锁冲突
- **场景**: 用户在 project root 跑 `git commit`，同时 agent 在 worktree commit
- **风险**: ❌ 无冲突（worktree 共享 `.git` 但各自 ref，`git worktree` 设计就是允许并发 commit）

---

### 5. 与 `persist_turn` 协调

| 策略 | 协调模式 | 顺序 |
|---|---|---|
| A (done 一次性) | 完全解耦 | `persist_turn` 一直在转，done 时一次 commit，**无顺序依赖** |
| B (turn 边界) | 紧耦合 | `persist_turn(assistant)` → `git add` → `git commit` **必须**这个顺序（DB 先落、git 后落，否则 git 引用了 DB 里没的 state） |
| C (定时器) | 解耦但有 race | 定时器 tick 与 `persist_turn` 并发，**无顺序保证** |
| D (混合) | B + A | turn 边界软 commit + done 硬 commit |

**推荐顺序**（B/D 适用）:
```rust
// lib.rs:866-878 之后
db::persist_turn(...).await?;          // 现有
maybe_commit_worktree(&worktree, ...).await;  // 新增, fire-and-forget
```

注意 `maybe_commit` 应 fire-and-forget 不要 block agent loop（DB 写已经阻塞过了，git 操作再阻塞会让 UX 变差）。失败只能 `tracing::warn!` 不 cascade。

---

### 6. 类似工具的实践

#### 6.1 Claude Code
- **实践**: **无自动 commit**。Claude Code 改文件后不会自动 git commit，留给用户决定。
- **替代**: 用户可手动 `/commit`（slash command 让 Claude 生成 commit message 并 commit）
- **原因**: Claude Code 是 CLI 工具，用户一直在看着；自动 commit 反而干扰

#### 6.2 Cursor
- **实践**: **无自动 commit**。Composer / Agent mode 改文件后由用户在 Source Control panel 手动 review + commit
- **差异**: Cursor 的 agent 输出在 side panel 实时 diff，用户能即时干预

#### 6.3 aider
- **实践**: **每次"commit"风格的 checkpoint**。aider 在每次 assistant 消息后自动 `git commit`，commit message 由 LLM 生成（基于 diff），commit author 设为 user 但带 `--trailer "Co-authored-by: aider <aider@..."`。
- **粒度**: 1 turn = 1 commit（≈方案 B + LLM 总结 message）
- **优点**: 用户切回随时 `git log` 看到 agent 工作流；崩了丢 ≤1 turn
- **缺点**: 大量小 commit 噪音（一个 30-turn session = 30+ commit）

#### 6.4 GitHub Copilot Workspace / Cody
- **实践**: 不自动 commit；提供 "explain commit" 工具让用户触发

#### 6.5 对比小结

| 工具 | 自动 commit | 粒度 | message 生成 |
|---|---|---|---|
| Claude Code | ❌ | — | — |
| Cursor | ❌ | — | — |
| **aider** | ✅ | **turn 边界** | LLM 总结 |
| Copilot Workspace | ❌ | — | — |
| Cody | ❌ | — | — |
| **本项目 (推荐)** | ✅ | turn 边界 + done 兜底 | turn 列表 + done LLM 总结 |

**aider 是唯一自动 commit 的主流参考**，其 "turn 边界" 粒度已被社区验证可行。

---

### 7. 决策矩阵

| 维度 / 方案 | A (done) | B (turn) | C (定时器) | D (混合) |
|---|---|---|---|---|
| UX 友好 | ⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐ | ⭐⭐⭐⭐⭐ |
| 数据安全 | ⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐⭐ |
| commit message 质量 | ⭐⭐⭐⭐⭐ | ⭐⭐⭐ | ⭐ | ⭐⭐⭐⭐ |
| 实现复杂度 | ⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐ | ⭐⭐ |
| 与 persist_turn 协调 | ⭐⭐⭐⭐⭐ | ⭐⭐⭐⭐ | ⭐⭐ | ⭐⭐⭐⭐ |
| 与 aider / 行业一致 | ⭐ | ⭐⭐⭐⭐⭐ | ⭐ | ⭐⭐⭐⭐ |
| **总分** | 18 | **25** | 14 | 25 |

**推荐**: **方案 B（turn 边界提交）** 作为 MVP。可选 v2 升级到 D（混合）以增加数据安全。

---

## Caveats / Not Found

1. **没找到 aider 的官方文档明确说 "turn 边界"** — 是从其 `--auto-commits` 默认行为和社区 issue 推断（aider 源码 `aider/repo.py::commit_history` 每轮 message 结束 commit）。需 spike 验证。
2. **方案 C 的"定时器周期"** 没有调参数据 — 业界常见的是 30s-5min，但本项目 agent loop 节奏未实测。
3. **方案 D 的 N/M 参数** 需要 spike 跑真实 session 测分布：平均 turn 间隔、tool 数分布。
4. **MVP 范围建议**（基于 ARCHITECTURE 已有的 `⑪ 隐式关卡` 流程图）: 先做 B（turn 边界），done 兜底可以后加（成本是改 3 个 return path 加一个 helper）。
5. **冲突测试未跑** — 多 session 同 project 改同文件的工作流还没真实跑过，理论分析基于 worktree 隔离设计，spike 需要验证。
6. **commit author 策略未定** — 用 `agent@everlasting.local` 还是 user 邮箱 + Co-authored-by? 影响 git log 可读性。

---

## Related Specs

- `.trellis/spec/backend/directory-structure.md` — `tools/git.rs` 新模块位置待定
- `.trellis/spec/backend/project-cwd-boundary.md` — worktree 路径的 boundary 语义
- `docs/ARCHITECTURE.md:418-427` ⑪ 阶段: 已描述 "是否自动 commit" 二选一，未锁定
- `docs/ARCHITECTURE.md:620-632` §3: worktree 路径约定
- `.trellis/tasks/06-07-step-4-git-worktree-auto-commit/prd.md` 父任务
- `docs/spikes/` 待新增 `auto-commit-timing.md`（本文件的副本）
