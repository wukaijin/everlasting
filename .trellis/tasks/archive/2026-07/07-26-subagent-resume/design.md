# Design: subagent resume mechanism (C1)

> 父任务：`07-26-workflow-review-plugin`（review epic）
> 配套 PRD：`07-26-subagent-resume/prd.md`
> 本 design 落定 PRD 的 Open Questions（存储方向 / worktree 策略 / 跨 session / stale context / API 形态）。

## 0. 调研事实（决策依据）

| 事实 | 位置 | 对设计的影响 |
|---|---|---|
| worker messages 由 `build_worker_messages` 构造（memory+task），传 `run_chat_loop` 累积 | `subagent/mod.rs:642`、`dispatch.rs:645/909` | resume 要拦截这个构造点 |
| **run 完成后 messages 不持久化**（只持久化 transcript） | `dispatch.rs:1124-1159`（只写 transcript_snapshot + final_text） | 方向 a 需「新增」存储，非复用 |
| `ChatMessage` 是 `Serialize+Deserialize`（`{role, content: MessageContent}`） | `llm/types.rs:185` | 可直接 JSON 序列化存储 |
| transcript 是 `{kind, payload_json}` 展示格式 ≠ LLM messages | `subagent/transcript.rs:18-27` | 方向 b（重建）易错，弃 |
| transcript >4MB 截断 head+tail | `truncate_summary.rs` | 方向 b 对长 run 实质不可行（MiniMax §4.6 属实） |
| worktree 生命周期：有改动→保留（DB 留 worktree_path）；无改动→销毁（清空） | `dispatch.rs:1271-1313` | resume 复用 worktree 受销毁策略约束 |
| 只读 worker（reviewer）isolation 本就 shared（无 worktree） | `dispatch.rs:513-535`（force_readonly→false；concurrent 只读 fall through 到 shared） | 只读 agent resume 无 worktree 可言 |
| dispatch_subagent schema properties: subagent/task/isolation/model | `subagent/mod.rs:381-422` | resume 参数加到这组 properties |
| schema required: subagent/task | `subagent/mod.rs:421` | resume 模式下 task 仍必填（作 clarification 载体） |

## 1. 存储：方向 a（持久化原始 LLM messages）

### 决策：新增 `subagent_runs.messages_json` 列，存 `Vec<ChatMessage>` 序列化 JSON

**为何弃方向 b**（transcript 重建）：
- transcript 是 `{kind, payload_json}`，kind 有 ChatEvent/ToolCall/ToolResult/PermissionAsk 等多种，映射回 ChatMessage 要做 kind→role + payload_json→ContentBlock + tool_use/tool_result id 配对——复杂且易错。
- transcript >4MB 截断 head+tail，长 run（reviewer 探索 codebase 常见）重建后残缺，resume 续接会基于残缺历史判断。

**新列 schema**（migration）：
```sql
ALTER TABLE subagent_runs ADD COLUMN messages_json TEXT;  -- Vec<ChatMessage> 序列化
ALTER TABLE subagent_runs ADD COLUMN messages_truncated INTEGER NOT NULL DEFAULT 0;  -- 同 transcript_truncated 模式
```

- `messages_json` 可空（旧 run 无此列数据；resume 旧 run 走 fallback 全新派）。
- 写入时机：worker run 完成（`update_run_finished`，`dispatch.rs:1143`），与 transcript 同批写。
- 截断策略：复用 `truncate_transcript_for_persistence` 的 head+tail 思路，但阈值更大（messages 是续接必需，宁可多存；建议 8MB，是 transcript 的 2 倍）。`messages_truncated=1` 时 resume 走 fallback（残缺历史不可续）+ warn。

### 数据流

```
worker run 完成
  ├─ update_run_finished(... transcript_json, final_text, ...)  [现有]
  └─ update_run_finished(... messages_json, messages_truncated)  [新增]
       └─ messages_json = serde_json::to_string(&accumulated_messages)

resume dispatch
  └─ load messages_json from subagent_runs by run_id
       ├─ Some(json) + !truncated → 反序列化 → 续接起点
       ├─ Some(json) + truncated → fallback 全新派 + warn（残缺不续）
       └─ None → fallback 全新派 + warn（旧 run / 持久化失败）
```

## 2. resume API：dispatch_subagent 加 resume_from + resume_clarification

### schema 扩展（`subagent/mod.rs:381`）

在现有 properties（subagent/task/isolation/model）加：
```jsonc
"resume_from": {
  "type": "string",
  "description": "续接某个历史 worker run（subagent_runs.id）。新 worker 的初始 messages = 上轮 messages + resume_clarification。省略则全新派（默认行为）。"
},
"resume_clarification": {
  "type": "object",
  "description": "resume 模式下注入新 worker 的结构化澄清（stale context 处理）。",
  "properties": {
    "current_state": {"type": "string", "description": "当前状态摘要（如修订后 prd 摘要）"},
    "changes_since_last": {"type": "array", "items": {"type": "string"}, "description": "上轮以来的变更点列表"},
    "this_round_purpose": {"type": "string", "description": "本轮目的"}
  },
  "required": ["this_round_purpose"]
}
```

- required 不变（仍 subagent + task）：resume 模式下 task 仍必填，作为「本轮委托」载体（resume_clarification 是结构化补充，task 是自然语言委托，两者并存）。
- resume_from + resume_clarification 都省略 = 全新派（零回归）。

### resume 模式的 messages 构造

`run_subagent`（`dispatch.rs`）检测 `resume_from`：
```rust
let initial_messages: Vec<ChatMessage> = if let Some(run_id) = input.get("resume_from").and_then(|v| v.as_str()) {
    // resume 路径
    match load_persisted_messages(db, run_id).await {
        Some(msgs) if !msgs.truncated => {
            let mut m = msgs.data;
            // 追加 clarification 作为新 user message（让 worker 看到本轮指令）
            if let Some(clar) = build_clarification_message(input) {
                m.push(clar);
            }
            // task 作为本轮委托（保留，resume 模式下 task 是「本轮做什么」）
            m.push(ChatMessage { role: Role::User, content: MessageContent::Text(task.into()) });
            m
        }
        _ => {
            tracing::warn!(run_id, "resume: messages unavailable/truncated, falling back to fresh dispatch");
            build_worker_messages(...).await  // fallback 全新派
        }
    }
} else {
    build_worker_messages(...).await  // 默认全新派
};
```

**关键**：resume 路径**绕过 `build_worker_messages`**（不重新注入 memory——上轮 messages 已含 memory 合成消息，重注会重复）。memory 变更（用户中途改了 memory 文件）在 resume 模式下不生效——这是 trade-off，文档化为「resume 继承上轮 memory 快照」。

## 3. worktree 策略：按 agent 类型分流（PRD R3 落定）

### 决策：resume 不主动复用 worktree；写 agent resume 时若上轮 worktree 已销毁，降级全新 worktree

基于调研事实（worktree 生命周期 + 只读 worker 无 worktree），分三种情况：

| agent 类型 | 上轮 isolation | 上轮 worktree 现状 | resume 策略 |
|---|---|---|---|
| 只读（reviewer/researcher） | shared（无 worktree） | N/A | **resume 不涉及 worktree**——共享主项目根，只续接 messages |
| 写（implementer）有改动 | isolated | 保留（DB 留 worktree_path） | **可选**：resume 时读 DB worktree_path，若存在则复用（`worktree_override=Some(path)`）；实现复杂，**C1 不做，留 follow-up** |
| 写（implementer）无改动 | isolated | 已销毁 | resume 时 worktree_path=None，按正常 isolation 决策新建（或不隔离） |

**C1 范围**：只实现「只读 agent resume（无 worktree）」+「写 agent resume 时不复用 worktree（按正常决策新建）」。「写 agent 复用保留的 worktree」标为 **follow-up**（C1 验收外，记 implement.md）。

理由：
- review（C1 的主消费者）的 reviewer 是只读，「无 worktree」路径已满足。
- 写 agent 复用 worktree 的价值（保留跨轮工作产物）在 review 场景不出现（review 不让 reviewer 写）；dev 的 implementer 跨轮复用是另一个场景，可独立做。
- 强行在 C1 实现写 agent worktree 复用会拖大 scope（要改 destroy 策略 + DB 状态机 + worktree_path 生命周期）。

## 4. 跨 session resume：禁止（PRD OQ3 落定）

### 决策：resume_from 只接受同 session 的 run_id；跨 session 返回 `resume_run_other_session` 错误

理由：
- review 是独立 session（C3 决策 1），但 reviewing↔revising 回环发生在**同一个 review session 内**——同 session resume 已覆盖 review 需求。
- 跨 session resume 引入「session 间状态泄漏」复杂性（一个 session 的 worker 历史被另一个 session 续接，权限/memory/project 边界都要校验），C1 不值得。
- 若未来「跨 session 评审」落地（父任务 Out of Scope 的 Phase 2），再放开。

实现：`load_persisted_messages` 校验 `subagent_runs.parent_session_id == 当前 session`，不匹配返回错误码 `resume_run_other_session`。

## 5. 错误处理 + fallback（PRD R1 fallback 落定）

### 错误码

| 场景 | 错误码 | 行为 |
|---|---|---|
| run_id 不存在 | `resume_run_not_found` | fallback 全新派 + warn |
| run 仍 running | `resume_run_still_running` | fallback 全新派 + warn（不允许 resume 进行中的 run） |
| run 跨 session | `resume_run_other_session` | fallback 全新派 + warn |
| messages_json 为空（旧 run） | （无错误码） | fallback 全新派 + warn |
| messages_truncated=1 | （无错误码） | fallback 全新派 + warn（残缺不续） |

**统一原则**：resume 失败**永远 fallback 全新派**，绝不阻断功能（PRD R1 fallback）。fallback 时 tool_result 内嵌一行 `[resume: fallback to fresh dispatch, reason: <code>]` 让主 LLM 知道发生了降级。

## 6. stale context 处理（PRD R4 落定）

resume 续接的 messages 含上轮内容（含旧 prd 引用）。靠 `resume_clarification` 结构化覆盖：

- `current_state`：当前状态摘要（reviewer 场景 = 修订后 prd 摘要）
- `changes_since_last`：变更点列表（显式列出哪些变了，让 worker 知道上轮引用中哪些已过时）
- `this_round_purpose`：本轮目的（如「验证上轮 high severity findings 是否已解决」）

配合 C3 reviewer.md 的 system prompt 提示「若上轮对话引用与当前文件矛盾，以当前文件为准」（已在 C3 PRD R2）。

**不做的**：不实现「截断/标记过期 messages」的引擎功能（复杂且 reviewer.md 提示已足够引导 LLM 区分）。这是 prompt 层引导 vs 引擎层强制的 trade-off，C1 选前者（轻量）。

## 7. 影响面 + 回归风险

### 改动文件
- `db/migrations.rs`：新 migration 加 `messages_json` + `messages_truncated` 列。
- `db/subagent_runs.rs`：`update_run_finished` 加 messages 参数；新增 `load_messages_by_run_id`。
- `agent/subagent/mod.rs`：`definition_with_cache` schema 加 resume_from/resume_clarification properties。
- `agent/subagent/dispatch.rs`：`run_subagent` 加 resume 分支（messages 构造 + fallback + 错误码）；`update_run_finished` 调用点加 messages 持久化。

### 回归风险
- **零回归保证**：不传 resume_from 时，`run_subagent` 走原 `build_worker_messages` 路径，行为完全一致。
- **新列可空**：旧 run 无 messages_json，resume 旧 run 走 fallback，不影响读取。
- **现有 subagent 测试全绿**：所有现有测试不传 resume_from，走原路径。
- **transcript 持久化不变**：messages_json 是新增写入，不替代 transcript。

### 单测覆盖
- resume 续接 messages 正确性（上轮 3 条 + clarification 1 + task 1 = 5 条）
- 空 messages_json 的 resume（旧 run）→ fallback
- messages_truncated=1 的 resume → fallback
- resume 不存在 run_id → fallback + `resume_run_not_found`
- resume running 的 run → fallback + `resume_run_still_running`
- resume 跨 session run → fallback + `resume_run_other_session`
- 不传 resume_from → 行为与现状一致（snapshot 对比 messages）
- `update_run_finished` 写入 messages_json 后能读回（round-trip）

## 8. follow-up（C1 范围外，记 implement.md）

- 写 agent（implementer）resume 复用保留的 worktree（需改 destroy 策略 + worktree_path 生命周期状态机）。
- 跨 session resume（Phase 2 跨 session 评审场景需要）。
- resume 时 memory 变更检测（当前继承上轮 memory 快照，不重注）。
